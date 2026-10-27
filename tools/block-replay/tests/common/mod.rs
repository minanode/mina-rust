//! Shared by the replay subset tests: runs `mina-block-replay` from a
//! network's genesis, or from a node's root dump, to one block of its chain and
//! checks what it reports.

use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

/// Where a subset starts.
// Each test crate that includes this module uses one of the two.
#[allow(dead_code)]
pub enum Start {
    /// From genesis, with the prebuilt genesis config `ledger-tool` writes,
    /// named by an environment variable. The config must give the genesis
    /// block this height and state hash.
    Genesis {
        bin_var: &'static str,
        height: u32,
        hash: &'static str,
    },
    /// From the staged ledger a node rebuilt at its root, which the replay
    /// directory holds as `root-<height>-<state hash>.binprot`.
    Dump { height: u32, hash: &'static str },
}

/// A replay from a start to one block of a network's canonical chain.
pub struct Subset {
    /// `--network` for `mina-block-replay`.
    pub network: &'static str,
    /// Names the replay directory.
    pub replay_dir_var: &'static str,
    /// The directory below it with `chain.txt` and the blocks, in it or in
    /// `blocks/`; empty for the replay directory itself.
    pub replay_subdir: &'static str,
    pub start: Start,
    /// The subset's last block, and its state hash on the canonical chain.
    pub to: (u32, &'static str),
    /// Report, without failing, a replay slower than the subset is meant to
    /// be.
    pub budget: Duration,
}

fn env_path(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("set {name}; see tools/regression/README.md"))
}

/// Replays every block after the start to `subset.to` and checks that each
/// staged ledger hash matched, that the replay started where it should and
/// that it ended at the expected block.
pub fn replay(subset: &Subset) {
    assert!(
        !cfg!(debug_assertions),
        "run with --release: an unoptimised replay takes hours"
    );
    let dir = env_path(subset.replay_dir_var).join(subset.replay_subdir);
    let blocks = match dir.join("blocks") {
        blocks if blocks.is_dir() => blocks,
        _ => dir.clone(),
    };

    let mut command = Command::new(env!("CARGO_BIN_EXE_mina-block-replay"));
    command.args(["--network", subset.network]);
    let (start_height, started_line) = match subset.start {
        Start::Genesis {
            bin_var,
            height,
            hash,
        } => {
            command.arg("--genesis").arg(env_path(bin_var));
            (height, format!("genesis {hash} loaded in"))
        }
        Start::Dump { height, hash } => {
            command
                .arg("--dump")
                .arg(dir.join(format!("root-{height}-{hash}.binprot")))
                .args(["--root", &height.to_string()]);
            (height, format!("root {height} {hash} rebuilt from"))
        }
    };
    command
        .arg("--chain")
        .arg(dir.join("chain.txt"))
        .arg("--blocks")
        .arg(&blocks)
        .args(["--to", &subset.to.0.to_string()]);

    let started = Instant::now();
    let output = command.output().expect("run mina-block-replay");
    let elapsed = started.elapsed();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail = |text: &str| {
        let lines: Vec<_> = text.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    };
    assert!(
        output.status.success(),
        "replay failed ({}):\n{}",
        output.status,
        tail(&stderr)
    );

    assert!(
        stdout.lines().any(|line| line.starts_with(&started_line)),
        "expected `{started_line} ...` in the output:\n{}",
        tail(&stdout)
    );
    let expected = format!(
        "applied {} blocks to {} in",
        subset.to.0 - start_height,
        subset.to.1
    );
    let summary = stdout
        .lines()
        .find(|line| line.starts_with("applied "))
        .unwrap_or_else(|| panic!("no summary line in the output:\n{}", tail(&stdout)));
    assert!(
        summary.starts_with(&expected) && summary.ends_with("every staged ledger hash matching"),
        "expected `{expected} ...`, got `{summary}`"
    );

    println!("{summary}");
    println!(
        "{} replay subset to {} took {:.0?}",
        subset.network, subset.to.0, elapsed
    );
    if elapsed > subset.budget {
        println!(
            "note: slower than the {:?} the subset is meant to take",
            subset.budget
        );
    }
}
