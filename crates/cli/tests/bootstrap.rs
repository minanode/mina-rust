use mina_core::log::system_time;
use mina_node::stats::sync::{SyncSnarkedLedger, SyncStagedLedger, SyncStatsSnapshot};
use redux::Timestamp;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
#[ignore = "public devnet: set MINA_TEST_PUBLIC_NETWORK=1 and run --ignored --exact bootstrap"]
fn bootstrap() -> anyhow::Result<()> {
    anyhow::ensure!(
        std::env::var("MINA_TEST_PUBLIC_NETWORK").as_deref() == Ok("1"),
        "public devnet requires MINA_TEST_PUBLIC_NETWORK=1"
    );
    let work_dir = tempfile::tempdir()?;
    let child = fork_node(work_dir.path())?;
    monitor_node(child)
}

const HTTP_PORT: u16 = 49998;
const P2P_PORT: u16 = 49999;
const COMMAND_VAR: &str = "MINA_COMMAND";

// Reap the node on success, timeout, or unwinding before its tempdir is removed.
struct NodeChild(Child);

impl Drop for NodeChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn fork_node(work_dir: &Path) -> anyhow::Result<NodeChild> {
    let (stdout_file, stderr_file) = if let Ok(path) = std::env::var("OUT_PATH") {
        let path = PathBuf::from(path);
        (
            File::create(path.with_extension("stdout"))?,
            File::create(path.with_extension("stderr"))?,
        )
    } else {
        (
            File::create(work_dir.join("bootstrap.stdout"))?,
            File::create(work_dir.join("bootstrap.stderr"))?,
        )
    };
    let mut child = NodeChild(node_command(work_dir).spawn()?);
    let stdout = child.0.stdout.take().ok_or(anyhow::anyhow!("no stdout"))?;
    let stderr = child.0.stderr.take().ok_or(anyhow::anyhow!("no stderr"))?;
    std::thread::spawn(move || pipe(stdout, stdout_file).unwrap());
    std::thread::spawn(move || pipe(stderr, stderr_file).unwrap());

    Ok(child)
}

fn pipe<R: Read, W: Write>(mut read: R, mut write: W) -> std::io::Result<()> {
    let mut buf = [0; 4096];
    loop {
        let len = read.read(&mut buf)?;
        if len == 0 {
            return Ok(());
        }
        write.write_all(&buf[..len])?;
    }
}

fn node_command(work_dir: &Path) -> Command {
    let exe = std::env::var_os(COMMAND_VAR).unwrap_or_else(|| env!("CARGO_BIN_EXE_mina").into());
    let mut command = Command::new(exe);
    // Do not inherit producer keys, config paths, or storage paths from the user.
    command.env_clear();
    for name in ["PATH", "BPF_ALIAS", "NO_PEER_DISCOVERY", "MINA_RECORD"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .args(["--network", "devnet", "node", "--work-dir"])
        .arg(work_dir)
        .args(["--port", &HTTP_PORT.to_string()])
        .args(["--libp2p-port", &P2P_PORT.to_string()])
        .env("MINA_HOME", work_dir)
        .env("RUST_MIN_STACK", "10000000")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn is_alive(child: &mut Child) -> anyhow::Result<()> {
    if child.try_wait()?.is_some() {
        anyhow::bail!("node finished unexpectedly");
    }
    Ok(())
}

const PAUSE: Duration = Duration::from_secs(10);
const HEALTHY: Duration = Duration::from_secs(60);
const READY: Duration = Duration::from_secs(20 * 60);

fn is_healthy() -> bool {
    reqwest::blocking::get(format!("http://localhost:{HTTP_PORT}/healthz"))
        .is_ok_and(|res| res.status().is_success())
}

fn is_ready() -> bool {
    let ready = reqwest::blocking::get(format!("http://localhost:{HTTP_PORT}/readyz"))
        .is_ok_and(|res| res.status().is_success());

    if let Err(err) = sync_stats() {
        println!("error getting stats: {err}");
        return false;
    }

    ready
}

fn sync_stats() -> anyhow::Result<()> {
    let stats: Vec<SyncStatsSnapshot> =
        reqwest::blocking::get(format!("http://localhost:{HTTP_PORT}/stats/sync?limit=1"))?
            .json()?;
    let stats = stats.first().ok_or(anyhow::anyhow!("no bootstrap stats"))?;

    let blocks = &stats.blocks;
    let mut fetched: u16 = 0;
    let mut applied: u16 = 0;
    for block in blocks {
        match block.status {
            mina_node::stats::sync::SyncBlockStatus::Fetched => fetched += 1,
            mina_node::stats::sync::SyncBlockStatus::Applied => {
                fetched += 1;
                applied += 1;
            }
            _ => {}
        }
    }
    println!("======================");
    println!(
        "fetched: {fetched} ({}%), applied: {applied} ({}%)",
        fetched * 100 / 290,
        applied * 100 / 290,
    );
    snarked_ledger_sync_stat(
        "staking ledger        ",
        stats.ledgers.staking_epoch.as_ref().map(|l| &l.snarked),
    );
    snarked_ledger_sync_stat(
        "next epoch ledger     ",
        stats.ledgers.next_epoch.as_ref().map(|l| &l.snarked),
    );
    snarked_ledger_sync_stat(
        "snarked ledger at root",
        stats.ledgers.root.as_ref().map(|l| &l.snarked),
    );
    staged_ledger_sync_stat(
        "staged ledger at root ",
        stats.ledgers.root.as_ref().map(|l| &l.staged),
    );

    Ok(())
}

fn snarked_ledger_sync_stat(name: &str, ledger: Option<&SyncSnarkedLedger>) {
    let Some(ledger) = ledger else {
        println!("{name}: no ledger");
        return;
    };
    let hashes = dur(ledger.fetch_hashes_start, ledger.fetch_hashes_end).unwrap();
    let accounts = dur(ledger.fetch_accounts_start, ledger.fetch_accounts_end).unwrap();
    println!("{name}: hashes: {hashes}, accounts: {accounts}");
}

fn staged_ledger_sync_stat(name: &str, ledger: Option<&SyncStagedLedger>) {
    let Some(ledger) = ledger else {
        println!("{name}: no ledger");
        return;
    };
    let parts = dur(ledger.fetch_parts_start, ledger.fetch_parts_end).unwrap();
    let reconstruct = dur(ledger.reconstruct_start, ledger.reconstruct_end).unwrap();
    println!("{name}: parts: {parts}, reconstruct: {reconstruct}");
}

fn dur(s: Option<Timestamp>, e: Option<Timestamp>) -> Option<String> {
    Some(match (s, e) {
        (None, None) => "not started".to_string(),
        (Some(s), None) => format!("{:?} (in progress)", system_time().checked_sub(s)?),
        (Some(s), Some(e)) => format!("{:?}", e.checked_sub(s)?),
        _ => unreachable!(),
    })
}

fn is_ready3() -> bool {
    is_ready()
        && {
            thread::sleep(PAUSE);
            is_ready()
        }
        && {
            thread::sleep(PAUSE);
            is_ready()
        }
}

fn wait_for(child: &mut Child, f: fn() -> bool, time: Duration) -> anyhow::Result<()> {
    let timeout = Instant::now() + time;
    while Instant::now() < timeout {
        is_alive(child)?;
        if f() {
            return Ok(());
        }
        thread::sleep(PAUSE);
    }
    Err(anyhow::anyhow!("node is not healthy within {time:?}"))
}

fn monitor_node(mut child: NodeChild) -> anyhow::Result<()> {
    wait_for(&mut child.0, is_healthy, HEALTHY)?;
    wait_for(&mut child.0, is_ready3, READY)?;
    Ok(())
}
