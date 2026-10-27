//! Replays that start from a staged ledger dump instead of genesis.
//!
//! - `replay_from_dump`: the last 1,000 blocks of the replay chain, from a
//!   dump of block 563044 to 564044, checking every staged ledger hash. It
//!   covers the chain's last zkApp-heavy stretch, from 563859, without the
//!   15,000 blocks before it.
//! - `dump_round_trip`: writes dumps from short replays near genesis and
//!   checks that a replay started from one ends in the same dump, byte for
//!   byte, as a replay from genesis. Its first ledger proofs come at 548415.
//!
//! They need data that is not in the repo, so they are ignored by default:
//!
//! ```text
//! MINA_REPLAY_DIR=<dir with chain.txt, blocks in it or in blocks/> \
//! MINA_REPLAY_DUMP=<dump of block 563044> \
//! MINA_GENESIS_BIN=<mesa-mainnet.bin> \
//! cargo test --release -p mina-block-replay --test replay_from_dump -- --ignored --nocapture
//! ```
//!
//! `replay_from_dump` needs the first two, `dump_round_trip` the first and the
//! last. `tools/regression/README.md` says how to get them, and
//! `tools/regression/inputs/` lists them with their checksums.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

/// The block the dump belongs to, and the last block replayed: the replay
/// chain's tip when this was written.
const FROM: (u32, &str) = (
    563044,
    "3NLxQsoFuJC2oBWyCwRNBXqJy32Ywang7MHQ9RosrVWFHnKsFuhL",
);
const TO: (u32, &str) = (
    564044,
    "3NL9ixEeoyrPa8nLyyKsRnHNuMg6yoaztdFdK5ZbRmJkPqqBE5Hd",
);
/// Report, without failing, a replay slower than it is meant to be.
const BUDGET: Duration = Duration::from_secs(180);

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {name}; see tools/regression/README.md"))
}

/// The chain file and the blocks directory in `MINA_REPLAY_DIR`.
fn replay_dir() -> (PathBuf, PathBuf) {
    let dir = env_path("MINA_REPLAY_DIR");
    let blocks = match dir.join("blocks") {
        blocks if blocks.is_dir() => blocks,
        _ => dir.clone(),
    };
    (dir.join("chain.txt"), blocks)
}

/// The state hash of the block at `height` in the chain file.
fn chain_hash(height: u32) -> String {
    let chain = std::fs::read_to_string(replay_dir().0).expect("read the chain file");
    chain
        .lines()
        .find_map(|line| {
            let (at, hash) = line.split_once(' ')?;
            (at.parse() == Ok(height)).then(|| hash.trim().to_owned())
        })
        .unwrap_or_else(|| panic!("no block {height} in the chain file"))
}

fn check_release() {
    if cfg!(debug_assertions) {
        panic!("run with --release: an unoptimised replay takes hours");
    }
}

/// Runs `mina-block-replay` with `args` and the replay directory's chain and
/// blocks, and returns its output, failing on an error.
fn replay(args: &[&str], dump_or_genesis: (&str, &Path)) -> String {
    let (chain, blocks) = replay_dir();
    let output = Command::new(env!("CARGO_BIN_EXE_mina-block-replay"))
        .args(["--network", "mainnet"])
        .arg(dump_or_genesis.0)
        .arg(dump_or_genesis.1)
        .arg("--chain")
        .arg(chain)
        .arg("--blocks")
        .arg(blocks)
        .args(args)
        .output()
        .expect("run mina-block-replay");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "replay {args:?} failed ({}):\n{}",
        output.status,
        tail(&stderr)
    );
    stdout
}

fn tail(text: &str) -> String {
    let lines: Vec<_> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

/// The summary line, checked to say `blocks` blocks were applied to `to`.
fn check_summary(stdout: &str, blocks: u32, to: &str) -> String {
    let expected = format!("applied {blocks} blocks to {to} in");
    let summary = stdout
        .lines()
        .find(|line| line.starts_with("applied "))
        .unwrap_or_else(|| panic!("no summary line in the output:\n{}", tail(stdout)));
    assert!(
        summary.starts_with(&expected) && summary.ends_with("every staged ledger hash matching"),
        "expected `{expected} ...`, got `{summary}`"
    );
    summary.to_owned()
}

#[test]
#[ignore = "needs replay data: set MINA_REPLAY_DIR and MINA_REPLAY_DUMP, and use --release"]
fn replay_from_dump() {
    check_release();
    let dump = env_path("MINA_REPLAY_DUMP");

    let started = Instant::now();
    let stdout = replay(&["--to", &TO.0.to_string()], ("--from-dump", &dump));
    let elapsed = started.elapsed();

    let start = format!("starting after block {} {}", FROM.0, FROM.1);
    assert!(
        stdout.lines().any(|line| line.starts_with(&start)),
        "expected `{start} ...` in the output:\n{}",
        tail(&stdout)
    );
    let summary = check_summary(&stdout, TO.0 - FROM.0, TO.1);
    println!("{summary}");
    println!(
        "replay from the dump of {} to {} took {:.0?}",
        FROM.0, TO.0, elapsed
    );
    if elapsed > BUDGET {
        println!("note: slower than the {BUDGET:?} it is meant to take");
    }
}

#[test]
#[ignore = "needs replay data: set MINA_REPLAY_DIR and MINA_GENESIS_BIN, and use --release"]
fn dump_round_trip() {
    /// Mesa mainnet's genesis height, and the two dumps' blocks: after the
    /// first ledger proofs, which come from 548415 on.
    const GENESIS_HEIGHT: u32 = 548147;
    const FIRST: u32 = 548450;
    const SECOND: u32 = 548550;

    check_release();
    let genesis = env_path("MINA_GENESIS_BIN");
    let dir = std::env::temp_dir().join(format!("mina-dump-round-trip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = |name: &str| dir.join(name).to_str().unwrap().to_owned();

    let stdout = replay(
        &[
            "--to",
            &FIRST.to_string(),
            "--write-dump",
            &path("first.binprot"),
        ],
        ("--genesis", &genesis),
    );
    assert!(
        stdout.contains(&format!("wrote the staged ledger of {FIRST} ")),
        "no dump written:\n{}",
        tail(&stdout)
    );
    let from_dump = replay(
        &[
            "--to",
            &SECOND.to_string(),
            "--write-dump",
            &path("second-from-dump.binprot"),
        ],
        ("--from-dump", Path::new(&path("first.binprot"))),
    );
    let from_genesis = replay(
        &[
            "--to",
            &SECOND.to_string(),
            "--write-dump",
            &path("second-from-genesis.binprot"),
        ],
        ("--genesis", &genesis),
    );
    let tip = chain_hash(SECOND);
    check_summary(&from_genesis, SECOND - GENESIS_HEIGHT, &tip);
    check_summary(&from_dump, SECOND - FIRST, &tip);

    let second_from_dump = std::fs::read(path("second-from-dump.binprot")).unwrap();
    let second_from_genesis = std::fs::read(path("second-from-genesis.binprot")).unwrap();
    assert!(
        second_from_dump == second_from_genesis,
        "the dumps of {SECOND} differ: {} bytes from the dump of {FIRST}, {} from genesis",
        second_from_dump.len(),
        second_from_genesis.len()
    );
    println!(
        "dumps of {SECOND} from genesis and from {FIRST} are the same {} bytes",
        second_from_dump.len()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
