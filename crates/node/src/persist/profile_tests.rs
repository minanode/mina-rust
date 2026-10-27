//! A profile of a restore over a real store (I5d4): the load, with the
//! node's verifier, then the replay of its blocks through the ledger
//! service, as the node does it, with timings. Ignored: it needs a mainnet
//! store, `I5D4_STORE=<dir>/frontier` (copied first; the copy is used and
//! deleted), and runs with `--ignored --nocapture`.

use std::{path::Path, time::Instant};

use ledger::proofs::verifiers::{block_verifier_index_of, BlockVerifier, TransactionVerifier};
use mina_core::block::AppliedBlock;

use crate::{
    ledger::LedgerCtx,
    persist::{
        load::{load, BlockProofVerifier, LoadOutcome, PersistLoadRequest},
        writer::PersistInsert,
        FrontierStore, FsBackend,
    },
};

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
}

#[test]
#[ignore]
fn profile_a_restore_of_a_real_store() {
    let Ok(source) = std::env::var("I5D4_STORE") else {
        eprintln!("I5D4_STORE is not set");
        return;
    };
    let network = std::env::var("I5D4_NETWORK").unwrap_or_else(|_| "mainnet".into());
    let _ = mina_core::NetworkConfig::init(&network);
    let runs = std::env::var("I5D4_RUNS")
        .ok()
        .and_then(|runs| runs.parse().ok())
        .unwrap_or(1usize);
    let replay = std::env::var("I5D4_REPLAY").is_ok_and(|replay| replay == "1");

    let started = Instant::now();
    let verifier = BlockProofVerifier {
        index: BlockVerifier::from_index(block_verifier_index_of(&network).unwrap()),
        srs: mina_snark::get_srs(),
    };
    TransactionVerifier::make();
    eprintln!("verifiers: {:.1} ms", started.elapsed().as_secs_f64() * 1e3);
    let request = PersistLoadRequest {
        current_slot: None,
        stale_slots: 4 * 290,
        genesis: None,
    };
    for run in 0..runs {
        let dir = std::env::temp_dir().join(format!("i5d4-profile-{}-{run}", std::process::id()));
        copy_dir(Path::new(&source), &dir);
        let (mut store, _) = FrontierStore::inspect(FsBackend::new(&dir).unwrap(), true).unwrap();
        let started = Instant::now();
        let outcome = load(&mut store, &request, Some(&verifier)).unwrap();
        let load_ms = started.elapsed().as_secs_f64() * 1e3;
        let LoadOutcome::Restored(restored) = outcome else {
            panic!("not restored: {}", outcome.kind());
        };
        eprintln!(
            "run {run}: load {load_ms:.1} ms, {} blocks: {:?}",
            restored.blocks.len(),
            restored.timings
        );
        drop(store);
        if std::env::var_os("I5D4_OVERLAP").is_some() {
            overlap(&restored);
        }
        if replay && run + 1 == runs {
            replay_blocks(*restored);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

/// The replay as the node's: each block after the root applied over its
/// parent with the staged ledger hash check, the proofs of the last
/// `CATCHUP_BLOCK_VERIFY_TAIL_LENGTH` verified.
fn replay_blocks(restored: crate::persist::load::Restored) {
    let crate::persist::load::Restored {
        ledgers,
        staged_ledger_hash,
        staged,
        root,
        blocks,
        ..
    } = restored;
    let mut ctx = LedgerCtx::default();
    ctx.persist_insert(PersistInsert {
        snarked: ledgers.into_iter().collect(),
        staged: Some((staged_ledger_hash, staged)),
    });
    let total = blocks.len();
    let mut pred = AppliedBlock {
        block: root,
        just_emitted_a_proof: false,
    };
    let started = Instant::now();
    let mut times = Vec::with_capacity(total);
    for (index, block) in blocks.into_iter().enumerate() {
        let pending = total - index;
        let skip = (pending > crate::transition_frontier::sync::CATCHUP_BLOCK_VERIFY_TAIL_LENGTH)
            .then_some(ledger::staged_ledger::staged_ledger::SkipVerification::All);
        let applying = Instant::now();
        let result = ctx
            .block_apply_with(block, pred, skip, true)
            .expect("a restored block applies");
        times.push(applying.elapsed().as_secs_f64() * 1e3);
        pred = AppliedBlock {
            block: result.block,
            just_emitted_a_proof: result.just_emitted_a_proof,
        };
    }
    let all = started.elapsed().as_secs_f64() * 1e3;
    let mut sorted = times.clone();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    eprintln!(
        "replay {total} blocks: {all:.1} ms; per block p10 {:.1} p50 {:.1} p90 {:.1} max {:.1} ms; last 5: {:?}",
        sorted[n / 10],
        sorted[n / 2],
        sorted[9 * n / 10],
        sorted[n - 1],
        &times[n.saturating_sub(5)..]
    );
}

/// How the stored snarked ledgers differ from each other, account by account.
fn overlap(restored: &crate::persist::load::Restored) {
    use ledger::{AccountIndex, BaseLedger};
    let ledgers = restored.ledgers.iter().collect::<Vec<_>>();
    let n = ledgers
        .iter()
        .map(|(_, l)| l.num_accounts())
        .max()
        .unwrap_or(0);
    for (i, (a_hash, a)) in ledgers.iter().enumerate() {
        for (b_hash, b) in ledgers.iter().skip(i + 1) {
            let differ = (0..n)
                .filter(|&k| {
                    a.get_at_index(AccountIndex(k as u64)) != b.get_at_index(AccountIndex(k as u64))
                })
                .count();
            eprintln!("OVERLAP {a_hash} vs {b_hash}: {differ} of {n} accounts differ");
        }
    }
}
