//! I5a's bench (R20 D1 and section 5): writes the three ledgers of a prebuilt
//! genesis file (`mesa-mainnet.bin`: the genesis, staking epoch and next
//! epoch ledgers) with the frontier store's ledger codec, as full files and as
//! diffs over each other, reads them back, and prints sizes and times. Then it
//! writes them as a store on disk and loads that as a restart would, checksums
//! included. The store's ledgers are loaded together, as on restart.
//!
//! ```text
//! cargo build --release -p mina-node --example persist_bench
//! target/release/examples/persist_bench /path/to/mesa-mainnet.bin /tmp/i5ab/bench
//! ```
//!
//! The output directory is created if needed; the files in it are replaced.

use std::{
    fs::File,
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context};
use ledger::{BaseLedger, Database, Mask};
use mina_core::ChainId;
use mina_node::{
    persist::{
        blob, choose_ledger_write,
        ledger_codec::{read_diff, read_full, read_header, write_diff, write_full},
        merkle_diff, ChainEntry, Checkpoint, CheckpointWrite, FrontierStore, FsBackend,
        LedgerWrite, StoreConfig, VerifyMode,
    },
    ProtocolConstants,
};
use mina_p2p_messages::{
    bigint::InvalidBigInt,
    binprot::{BinProtRead, Nat0},
    v2,
};

const NAMES: [&str; 3] = ["genesis", "staking", "next"];

/// Buffers as the store uses them.
const IO_BUFFER: usize = 4 << 20;

fn main() -> anyhow::Result<()> {
    let usage = "usage: persist_bench <prebuilt genesis .bin> <output directory>";
    let mut args = std::env::args_os().skip(1);
    let input = PathBuf::from(args.next().context(usage)?);
    let out = PathBuf::from(args.next().context(usage)?);
    std::fs::create_dir_all(&out)?;
    mina_core::NetworkConfig::init("mainnet").map_err(|error| anyhow!(error))?;
    println!(
        "{} threads, load average {}",
        rayon::current_num_threads(),
        std::fs::read_to_string("/proc/loadavg")
            .unwrap_or_default()
            .trim()
    );

    let started = Instant::now();
    let bytes = std::fs::read(&input).with_context(|| input.display().to_string())?;
    let ledgers = read_prebuilt(&bytes)?;
    drop(bytes);
    println!(
        "read {} ({} accounts) in {:.2?}",
        input.display(),
        ledgers
            .iter()
            .map(|ledger| ledger.num_accounts().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        started.elapsed()
    );

    println!("\nfull files");
    let mut full_files = Vec::new();
    for (name, ledger) in NAMES.iter().zip(&ledgers) {
        let path = out.join(format!("{name}.full"));
        let started = Instant::now();
        let mut writer = BufWriter::with_capacity(IO_BUFFER, File::create(&path)?);
        write_full(&mut writer, ledger)?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        let write = started.elapsed();
        let size = std::fs::metadata(&path)?.len();
        let mut times = Vec::new();
        for verify in [
            VerifyMode::RootOnly,
            VerifyMode::default(),
            VerifyMode::Full,
        ] {
            let started = Instant::now();
            let loaded = read_full_file(&path, verify)?;
            times.push(started.elapsed());
            if root(&loaded) != root(ledger) {
                bail!("{name}: read back with another root");
            }
        }
        println!(
            "  {name:8} {:>8} accounts  {}  write {:>8.2?}  read: root only {:>8.2?} ({}), \
             64 paths {:>8.2?}, rehash all {:>8.2?}",
            ledger.num_accounts(),
            mb(size),
            write,
            times[0],
            rate(size, times[0]),
            times[1],
            times[2],
        );
        full_files.push(path);
    }

    println!("\nall three full files read at once, 64 paths each");
    let started = Instant::now();
    std::thread::scope(|scope| {
        let reads = full_files
            .iter()
            .map(|path| scope.spawn(move || read_full_file(path, VerifyMode::default())))
            .collect::<Vec<_>>();
        reads
            .into_iter()
            .map(|read| read.join().unwrap().map(|_| ()))
            .collect::<anyhow::Result<()>>()
    })?;
    println!("  {:.2?}", started.elapsed());

    println!("\ndiffs");
    for (target, base) in [(1, 0), (2, 1), (2, 0), (0, 2)] {
        let (target_name, base_name) = (NAMES[target], NAMES[base]);
        let started = Instant::now();
        let diff = match merkle_diff(&ledgers[target], &ledgers[base]) {
            Ok(diff) => diff,
            Err(error) => {
                println!("  {target_name} over {base_name}: no diff: {error}");
                continue;
            }
        };
        let walk = started.elapsed();
        let appended = diff
            .accounts
            .iter()
            .filter(|(index, _)| index.0 >= ledgers[base].num_accounts() as u64)
            .count();
        let path = out.join(format!("{target_name}-over-{base_name}.diff"));
        let started = Instant::now();
        let mut writer = BufWriter::with_capacity(IO_BUFFER, File::create(&path)?);
        write_diff(&mut writer, &diff)?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        let write = started.elapsed();
        let size = std::fs::metadata(&path)?.len();
        let started = Instant::now();
        let mut reader = BufReader::with_capacity(IO_BUFFER, File::open(&path)?);
        let header = read_header(&mut reader)?;
        let loaded = read_diff(&mut reader, &header, &ledgers[base], VerifyMode::default())?;
        let read = started.elapsed();
        if root(&loaded) != root(&ledgers[target]) {
            bail!("{target_name} over {base_name}: another root");
        }
        println!(
            "  {target_name} over {base_name}: {} accounts differ ({:.1}%, {appended} appended), \
             {} hashes, {}; walk {walk:.2?}, write {write:.2?}, read with 64 paths {read:.2?}",
            diff.accounts.len(),
            100.0 * diff.accounts.len() as f64 / ledgers[target].num_accounts() as f64,
            diff.hashes.len(),
            mb(size),
        );
    }

    println!("\na store with the three ledgers (next as the root snarked ledger)");
    let dir = out.join("store");
    let _ = std::fs::remove_dir_all(&dir);
    let root_hash = v2::StateHash::from_fp(1u64.into());
    let hashes = ledgers
        .iter()
        .map(|ledger| v2::LedgerHash::from_fp(root(ledger)))
        .collect::<Vec<_>>();
    let started = Instant::now();
    let writes = vec![
        (
            hashes[0].clone(),
            Some(LedgerWrite::Full(ledgers[0].clone())),
        ),
        (
            hashes[1].clone(),
            Some(choose_ledger_write(&ledgers[1], &ledgers[..1])),
        ),
        (
            hashes[2].clone(),
            Some(choose_ledger_write(&ledgers[2], &ledgers[..2])),
        ),
    ];
    let choose = started.elapsed();
    let kinds = writes
        .iter()
        .zip(NAMES)
        .map(|((_, write), name)| match write {
            Some(LedgerWrite::Full(_)) => format!("{name} full"),
            _ => format!("{name} as a diff"),
        })
        .collect::<Vec<_>>();
    let write = CheckpointWrite {
        checkpoint: Checkpoint {
            root: root_hash.clone(),
            root_height: 0,
            root_snarked_ledger: hashes[2].clone(),
            root_staged_ledger_hash: v2::MinaBaseStagedLedgerHashStableV1::zero(
                hashes[2].clone(),
                v2::PendingCoinbaseHash::from_fp(1u64.into()),
            ),
        },
        ledgers: writes,
        staged: merkle_diff(&ledgers[2], &ledgers[2])?,
        parts: blob(Vec::new()),
        chain: vec![ChainEntry {
            hash: root_hash.clone(),
            height: 0,
            global_slot: 0,
            just_emitted_a_proof: false,
        }],
        blocks: vec![(root_hash, blob(Vec::new()))],
    };
    let started = Instant::now();
    let (mut store, _) = FrontierStore::open(FsBackend::new(&dir)?, config())?;
    store.checkpoint(write)?;
    let manifest = store.manifest().context("no manifest")?.clone();
    drop(store);
    println!(
        "  written in {:.2?} (choosing diffs {choose:.2?}): {}; {} in all",
        started.elapsed(),
        kinds.join(", "),
        mb(manifest.files.iter().map(|file| file.len).sum())
    );

    let started = Instant::now();
    let (mut store, _) = FrontierStore::open(FsBackend::new(&dir)?, config())?;
    let open = started.elapsed();
    let manifest = store.manifest().context("no manifest")?.clone();
    let started = Instant::now();
    let loaded = store.read_ledgers(&manifest, VerifyMode::default())?;
    let ledgers = started.elapsed();
    let started = Instant::now();
    store.read_staged(&manifest, &loaded[&hashes[2]], VerifyMode::default())?;
    let staged = started.elapsed();
    println!(
        "  loaded: open {open:.2?}, ledgers {ledgers:.2?}, staged {staged:.2?}; total {:.2?}",
        open + ledgers + staged
    );
    for file in &manifest.files {
        println!("    {:72} {}", file.name(), mb(file.len));
    }
    drop(loaded);
    drop(store);

    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines().filter(|line| line.starts_with("VmHWM")) {
            println!(
                "\npeak memory: {}",
                line.trim_start_matches("VmHWM:").trim()
            );
        }
    }
    Ok(())
}

fn config() -> StoreConfig {
    StoreConfig {
        chain_id: ChainId::from_bytes(&[0; 32]),
        genesis_state_hash: v2::StateHash::from_fp(2u64.into()),
        node_commit: "persist_bench".to_owned(),
        now_ms: Box::new(|| 0),
    }
}

fn root(ledger: &Mask) -> mina_curves::pasta::Fp {
    ledger.clone().merkle_root()
}

fn mb(bytes: u64) -> String {
    format!("{:>7.1} MB", bytes as f64 / 1e6)
}

fn rate(bytes: u64, time: Duration) -> String {
    format!("{:.0} MB/s", bytes as f64 / 1e6 / time.as_secs_f64())
}

fn read_full_file(path: &Path, verify: VerifyMode) -> anyhow::Result<Mask> {
    let mut reader = BufReader::with_capacity(IO_BUFFER, File::open(path)?);
    let header = read_header(&mut reader)?;
    Ok(read_full(&mut reader, &header, verify)?)
}

/// The three ledgers of a prebuilt genesis file: after the `MINAGEN1` magic
/// and its runtime header, if it has them, the protocol constants, then each
/// ledger as its accounts, its hash and its inner hashes, the epoch ledgers
/// each followed by a seed (`PrebuiltGenesisConfig::load_from_bytes`).
fn read_prebuilt(mut bytes: &[u8]) -> anyhow::Result<Vec<Mask>> {
    if let Some(rest) = bytes.strip_prefix(b"MINAGEN1") {
        let len = u32::from_le_bytes(rest.get(..4).context("truncated")?.try_into()?) as usize;
        bytes = rest.get(4 + len..).context("truncated")?;
    }
    ProtocolConstants::binprot_read(&mut bytes)?;
    let genesis = read_ledger(&mut bytes)?;
    let staking = read_ledger(&mut bytes)?;
    v2::EpochSeed::binprot_read(&mut bytes)?;
    let next = read_ledger(&mut bytes)?;
    v2::EpochSeed::binprot_read(&mut bytes)?;
    if !bytes.is_empty() {
        bail!("{} bytes after the ledgers", bytes.len());
    }
    Ok(vec![genesis, staking, next])
}

fn read_ledger(bytes: &mut &[u8]) -> anyhow::Result<Mask> {
    let depth = mina_core::constants::constraint_constants().ledger_depth as u8;
    let len = Nat0::binprot_read(bytes)?.0;
    let mut ledger = Mask::new_root(Database::create_with_token_owners(depth));
    for _ in 0..len {
        let account = v2::MinaBaseAccountBinableArgStableV2::binprot_read(bytes)?;
        let account = ledger::Account::try_from(&account).map_err(|e| anyhow!("{e:?}"))?;
        ledger
            .get_or_create_account(account.id(), account)
            .map_err(|e| anyhow!("{e:?}"))?;
    }
    let hash = v2::LedgerHash::binprot_read(bytes)?;
    let hashes = Vec::<(u64, v2::LedgerHash)>::binprot_read(bytes)?
        .into_iter()
        .map(|(index, hash)| Ok((index, hash.to_field()?)))
        .collect::<Result<Vec<_>, InvalidBigInt>>()
        .map_err(|e| anyhow!("{e:?}"))?;
    // After the accounts, which would clear them.
    ledger.set_raw_inner_hashes(hashes);
    if v2::LedgerHash::from_fp(root(&ledger)) != hash {
        bail!("a ledger's inner hashes don't give its hash {hash}");
    }
    Ok(ledger)
}
