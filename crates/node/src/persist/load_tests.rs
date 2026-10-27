//! Tests of the loader (I5d1, R20 D5): each case of its policy over the
//! in-memory backend, with stores written by `FrontierStore` itself.
//!
//! The blocks are a mainnet block with its links, heights, slots and ledger
//! hashes rewritten, so that they decode and hash like real ones: the loader
//! rehashes each to its name, follows the `previous_state_hash` links, checks
//! each body and header as a received block's, and checks the root's complete
//! staged ledger hash against the staged ledger it builds from the stored
//! diff, scan state and pending coinbase. Their proofs are the template's, for
//! another protocol state: a store of them is a store whose proofs were
//! replaced, which a load with a verifier rejects.
//!
//! The adversarial cases (the I5d1 review) tamper with a store and record the
//! new file's length and CRC32 in the manifest, so that only the checks of
//! content catch them.

use std::sync::Arc;

use ledger::{
    proofs::verifiers::{block_verifier_index_of, BlockVerifier},
    scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState},
    staged_ledger::staged_ledger::StagedLedger,
};
use mina_core::{
    block::{ArcBlockWithHash, BlockWithHash},
    constants::constraint_constants,
};
use mina_p2p_messages::list::List;

use super::*;
use crate::persist::{
    load::{
        load, BlockProofVerifier, LoadOutcome, PersistLoadRequest, RestoreGuard, Restored,
        DISCARD_AFTER_ATTEMPTS, LEDGERS_ONLY_AFTER_ATTEMPTS,
    },
    test_fixtures::{block_after, mainnet_blocks, synthetic_chain, ChainLedgers, SLOTS_PER_BLOCK},
    writer::PartsSnapshot,
};

fn chain_entries(blocks: &[ArcBlockWithHash]) -> Vec<ChainEntry> {
    blocks
        .iter()
        .map(|block| ChainEntry {
            hash: block.hash().clone(),
            height: block.height(),
            global_slot: block.global_slot(),
            just_emitted_a_proof: false,
        })
        .collect()
}

fn encoded_blocks(blocks: &[ArcBlockWithHash]) -> Vec<(v2::StateHash, Blob)> {
    blocks
        .iter()
        .map(|block| {
            let mut bytes = Vec::new();
            block.block.binprot_write(&mut bytes).unwrap();
            (block.hash().clone(), blob(bytes))
        })
        .collect()
}

/// The root's parts as a node writes them: an empty scan state and a new
/// pending coinbase, as `StagedLedger::create_exn` makes.
fn parts_bytes(staged: &Mask) -> Vec<u8> {
    let constants = constraint_constants();
    let mut pending_coinbase = PendingCoinbase::create(constants.pending_coinbase_depth);
    pending_coinbase.merkle_root();
    let parts = PartsSnapshot {
        scan_state: ScanState::empty(constants),
        pending_coinbase,
        staged_ledger_hash: hash_of(staged),
        needed_blocks: List::new(),
    }
    .into_wire();
    let mut bytes = Vec::new();
    parts.binprot_write(&mut bytes).unwrap();
    bytes
}

/// The complete staged ledger hash of `staged` with those parts.
fn complete_hash(staged: &Mask) -> v2::MinaBaseStagedLedgerHashStableV1 {
    let mut ledger = StagedLedger::create_exn(constraint_constants().clone(), staged.copy())
        .expect("a staged ledger");
    v2::MinaBaseStagedLedgerHashStableV1::from(&ledger.hash())
}

/// A checkpoint's ledgers and its chain of blocks.
struct Stored {
    fixture: Fixture,
    ledgers: ChainLedgers,
    blocks: Vec<ArcBlockWithHash>,
}

impl Stored {
    fn new(num_accounts: usize, len: usize) -> Self {
        let fixture = Fixture::new(num_accounts);
        let ledgers = ChainLedgers {
            staking: hash_of(&fixture.staking),
            next: hash_of(&fixture.staking),
            snarked: hash_of(&fixture.snarked),
            staged: complete_hash(&fixture.staged),
        };
        let blocks = synthetic_chain(state_hash(99), len, &ledgers);
        Self {
            fixture,
            ledgers,
            blocks,
        }
    }

    fn tip_slot(&self) -> u32 {
        self.blocks.last().unwrap().global_slot()
    }

    /// The checkpoint at the chain's first block, with the whole chain.
    fn write(&self) -> CheckpointWrite {
        self.write_with(&self.blocks)
    }

    fn write_with(&self, blocks: &[ArcBlockWithHash]) -> CheckpointWrite {
        let fixture = &self.fixture;
        let root = &blocks[0];
        CheckpointWrite {
            checkpoint: Checkpoint {
                root: root.hash().clone(),
                root_height: root.height(),
                root_snarked_ledger: hash_of(&fixture.snarked),
                root_staged_ledger_hash: root.staged_ledger_hashes().clone(),
            },
            ledgers: vec![
                (
                    hash_of(&fixture.staking),
                    Some(LedgerWrite::Full(fixture.staking.clone())),
                ),
                (
                    hash_of(&fixture.snarked),
                    Some(choose_ledger_write(
                        &fixture.snarked,
                        std::slice::from_ref(&fixture.staking),
                    )),
                ),
            ],
            staged: merkle_diff(&fixture.staged, &fixture.snarked).unwrap(),
            parts: blob(parts_bytes(&fixture.staged)),
            chain: chain_entries(blocks),
            blocks: encoded_blocks(blocks),
        }
    }

    /// A store holding the checkpoint: generation 1.
    fn store(&self) -> MemBackend {
        let mem = MemBackend::new();
        let mut store = reopened(&mem);
        assert_eq!(store.checkpoint(self.write()).unwrap(), 1);
        mem
    }
}

/// A genesis block the synthetic blocks belong to: named by their
/// `genesis_state_hash`, with their constants.
fn our_genesis() -> ArcBlockWithHash {
    let template = &mainnet_blocks()[0];
    BlockWithHash {
        hash: template.header().genesis_state_hash().clone(),
        block: template.block.clone(),
    }
}

/// What a node asks for when its clock is at `current_slot` (mainnet's k).
fn request(current_slot: Option<u32>) -> PersistLoadRequest {
    PersistLoadRequest {
        current_slot,
        stale_slots: 4 * 290,
        genesis: Some(our_genesis()),
    }
}

/// Mainnet's blockchain verifier.
fn mainnet_verifier() -> BlockProofVerifier {
    BlockProofVerifier {
        index: BlockVerifier::from_index(block_verifier_index_of("mainnet").unwrap()),
        srs: mina_snark::get_srs(),
    }
}

/// Replaces a stored file's content and records its new length and CRC32 in
/// every manifest slot that references it: damage the checksums don't see.
fn replace_file(mem: &MemBackend, name: &str, bytes: Vec<u8>) {
    let crc32 = crc32fast::hash(&bytes);
    for slot in MANIFEST_SLOTS {
        let Some(slot_bytes) = mem.file(slot) else {
            continue;
        };
        let Ok(mut manifest) = decode_manifest(&slot_bytes) else {
            continue;
        };
        let mut referenced = false;
        for file in &mut manifest.files {
            if file.name() == name {
                *file = FileRef::new(name, bytes.len() as u64, crc32);
                referenced = true;
            }
        }
        if referenced {
            mem.put_file(slot, encode_manifest(&manifest));
        }
    }
    mem.put_file(name, bytes);
}

/// The store as a new process opens it (the in-memory lock is only
/// released by `reopen`).
fn reopened(mem: &MemBackend) -> FrontierStore<MemBackend> {
    FrontierStore::open(mem.reopen(), config()).unwrap().0
}

fn load_from(mem: &MemBackend, current_slot: Option<u32>) -> LoadOutcome {
    load(&mut reopened(mem), &request(current_slot), None).unwrap()
}

fn restored(outcome: LoadOutcome) -> Restored {
    match outcome {
        LoadOutcome::Restored(restored) => *restored,
        other => panic!("not restored: {} ({})", other.kind(), reason_of(&other)),
    }
}

fn reason_of(outcome: &LoadOutcome) -> String {
    match outcome {
        LoadOutcome::Absent { reason }
        | LoadOutcome::LedgersOnly { reason, .. }
        | LoadOutcome::Discarded { reason } => reason.clone(),
        LoadOutcome::Restored(_) => String::new(),
    }
}

fn ledgers_only(outcome: LoadOutcome) -> (String, BTreeMap<v2::LedgerHash, Mask>) {
    match outcome {
        LoadOutcome::LedgersOnly {
            reason, ledgers, ..
        } => (reason, ledgers),
        other => panic!("not ledgers only: {}", other.kind()),
    }
}

fn discarded(outcome: LoadOutcome) -> String {
    match outcome {
        LoadOutcome::Discarded { reason } => reason,
        other => panic!("not discarded: {}", other.kind()),
    }
}

fn attempts(mem: &MemBackend) -> Option<Attempts> {
    let (mut store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
    store.attempts().unwrap()
}

#[test]
fn pc2_frontier_checkpoint_with_cached_work_proofs_restores_identical_bytes() {
    use mina_p2p_messages::{binprot::BinProtRead, proof_cache::ProofStore};
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../target/test-data/proof-cache/frontier-{}",
        std::process::id()
    ));
    let cache = ProofStore::dir(&dir).unwrap();
    let mut stored = Stored::new(12, 1);
    // The OCaml best tip has 51 work proofs. Give it the synthetic
    // checkpoint's ledgers, links and slots while preserving its body hash.
    let template = mainnet_blocks().pop().unwrap();
    let mut block = (*template.block).clone();
    let body_reference = block
        .header
        .protocol_state
        .body
        .blockchain_state
        .body_reference
        .clone();
    block.header = stored.blocks[0].header().clone();
    block
        .header
        .protocol_state
        .body
        .blockchain_state
        .body_reference = body_reference;
    // Round-trip to isolate handles from any shared test template.
    let mut bytes = Vec::new();
    block.binprot_write(&mut bytes).unwrap();
    let block = v2::MinaBlockBlockStableV2::binprot_read(&mut bytes.as_slice()).unwrap();
    stored.blocks = vec![ArcBlockWithHash::try_new(Arc::new(block)).unwrap()];
    let mut count = 0;
    for work in stored.blocks[0].completed_works_iter() {
        let proofs = match &work.proofs {
            v2::TransactionSnarkWorkTStableV2Proofs::One(p) => vec![p],
            v2::TransactionSnarkWorkTStableV2Proofs::Two((p, q)) => vec![p, q],
        };
        for p in proofs {
            assert!(!p.0.proof.is_cached());
            count += 1;
        }
    }
    assert_eq!(count, 51);
    stored.blocks[0].cache_proofs_in(&cache).unwrap();
    let mem = stored.store();
    let loaded = restored(load_from(&mem, Some(stored.tip_slot() + 3)));
    let mut restored_bytes = Vec::new();
    loaded
        .root
        .block
        .binprot_write(&mut restored_bytes)
        .unwrap();
    assert_eq!(restored_bytes, bytes);
    assert_eq!(loaded.root.hash(), stored.blocks[0].hash());
    assert_eq!(cache.stats().files, 51);
    // Cache files are process-local: the restored block is self-contained.
    drop((stored, loaded));
    assert_eq!(cache.stats().files, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_good_store_restores() {
    let stored = Stored::new(12, 5);
    let mem = stored.store();
    let restored = restored(load_from(&mem, Some(stored.tip_slot() + 3)));

    assert_eq!(restored.generation, 1);
    assert_eq!(restored.attempt, 1);
    assert_eq!(restored.root.hash(), stored.blocks[0].hash());
    assert_eq!(
        restored
            .blocks
            .iter()
            .map(|block| block.hash().clone())
            .collect::<Vec<_>>(),
        stored.blocks[1..]
            .iter()
            .map(|block| block.hash().clone())
            .collect::<Vec<_>>()
    );
    let fixture = &stored.fixture;
    assert_eq!(
        restored.ledgers.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([hash_of(&fixture.staking), hash_of(&fixture.snarked)])
    );
    assert_same_ledger(
        &restored.ledgers[&hash_of(&fixture.staking)],
        &fixture.staking,
    );
    assert_same_ledger(
        &restored.ledgers[&hash_of(&fixture.snarked)],
        &fixture.snarked,
    );
    // The root staged ledger: its complete hash is the root block's, and it
    // is a mask over the restored root snarked ledger (I5d5), as OCaml's
    // persistent frontier builds it, holding only the accounts the staged
    // ledger changed (the fixture's 0 and 3, and one new).
    assert_eq!(&restored.staged_ledger_hash, &stored.ledgers.staged);
    let mut staged = restored.staged;
    assert_eq!(
        v2::MinaBaseStagedLedgerHashStableV1::from(&staged.hash()),
        stored.ledgers.staged
    );
    let root_snarked = &restored.ledgers[&hash_of(&fixture.snarked)];
    assert!(root_snarked.is_root());
    assert!(staged.ledger().is_attached());
    assert_eq!(
        staged.ledger().get_parent().unwrap().get_uuid(),
        root_snarked.get_uuid()
    );
    assert_eq!(staged.ledger().validate_child_account_indexes(), Ok(3));
    assert_same_ledger(&staged.ledger(), &fixture.staged);
    assert_same_ledger(root_snarked, &fixture.snarked);
    assert!(restored.needed_protocol_states.is_empty());
    // The attempt is recorded before anything is decoded.
    assert_eq!(
        attempts(&mem),
        Some(Attempts {
            generation: 1,
            count: 1
        })
    );

    // A node that doesn't know the time skips the clock checks.
    assert_eq!(restored_attempt(&mem, None), 2);
}

fn restored_attempt(mem: &MemBackend, current_slot: Option<u32>) -> u32 {
    restored(load_from(mem, current_slot)).attempt
}

/// A restore that the node then commits: it clears the attempts record, as
/// the store thread's `RestoreDone` does.
fn restore_and_commit(mem: &MemBackend, current_slot: Option<u32>) -> Restored {
    let restored = restored(load_from(mem, current_slot));
    reopened(mem).clear_attempts().unwrap();
    restored
}

#[test]
fn an_empty_store_is_absent() {
    let mem = MemBackend::new();
    match load_from(&mem, Some(5)) {
        LoadOutcome::Absent { reason } => assert!(reason.contains("empty"), "{reason}"),
        other => panic!("{}", other.kind()),
    }
    // A store with only a chain (no checkpoint, so no ledgers): nothing to
    // load either.
    let stored = Stored::new(6, 3);
    {
        let mut store = reopened(&mem);
        store
            .chain_put(
                chain_entries(&stored.blocks),
                encoded_blocks(&stored.blocks),
            )
            .unwrap();
    }
    match load_from(&mem, Some(stored.tip_slot())) {
        LoadOutcome::Absent { reason } => assert!(reason.contains("checkpoint"), "{reason}"),
        other => panic!("{}", other.kind()),
    }
}

#[test]
fn a_store_of_another_chain_is_not_loaded() {
    let stored = Stored::new(6, 3);
    let mem = stored.store();
    // Another chain id: the open deletes the store before the loader runs.
    let (mut store, report) =
        FrontierStore::open(mem.reopen(), config_for(DEVNET_CHAIN_ID, genesis())).unwrap();
    assert!(matches!(report.outcome, OpenOutcome::Reset { .. }));
    assert!(matches!(
        load(&mut store, &request(Some(stored.tip_slot())), None).unwrap(),
        LoadOutcome::Absent { .. }
    ));
    assert!(data_files(&mem).is_empty());

    // Another genesis, same chain id: the same.
    let mem = stored.store();
    let (mut store, report) =
        FrontierStore::open(mem.reopen(), config_for(MAINNET_CHAIN_ID, state_hash(1))).unwrap();
    assert!(matches!(report.outcome, OpenOutcome::Reset { .. }));
    assert!(matches!(
        load(&mut store, &request(Some(stored.tip_slot())), None).unwrap(),
        LoadOutcome::Absent { .. }
    ));
}

#[test]
fn a_tip_far_behind_or_ahead_of_the_clock_gives_only_the_ledgers() {
    let stored = Stored::new(8, 4);
    let mem = stored.store();
    let tip = stored.tip_slot();
    let stale = request(None).stale_slots;

    // Exactly 4k behind still replays; one slot more doesn't.
    restore_and_commit(&mem, Some(tip + stale));
    let (reason, ledgers) = ledgers_only(load_from(&mem, Some(tip + stale + 1)));
    assert!(reason.contains("behind the clock"), "{reason}");
    assert_eq!(ledgers.len(), 2);
    assert_same_ledger(
        &ledgers[&hash_of(&stored.fixture.snarked)],
        &stored.fixture.snarked,
    );

    // Ahead of the clock by more than a few slots.
    restore_and_commit(&mem, Some(tip - 10));
    let (reason, _) = ledgers_only(load_from(&mem, Some(tip - 11)));
    assert!(reason.contains("ahead of the clock"), "{reason}");
    // The store is kept for the next start.
    assert!(load_from(&mem, Some(tip)).kind() == "restored");
}

#[test]
fn a_checkpoint_dropped_by_a_bootstrap_gives_only_the_ledgers() {
    let stored = Stored::new(8, 4);
    let mem = stored.store();
    // The node bootstrapped to another chain: its root isn't on the stored
    // chain, so the store keeps the ledgers and drops the checkpoint.
    let other = Stored::new(6, 2);
    {
        let mut store = reopened(&mem);
        let put = store
            .chain_put(chain_entries(&other.blocks), encoded_blocks(&other.blocks))
            .unwrap();
        assert!(put.checkpoint_invalidated);
    }
    let (reason, ledgers) = ledgers_only(load_from(&mem, Some(other.tip_slot())));
    assert!(reason.contains("no checkpoint"), "{reason}");
    assert_eq!(
        ledgers.keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            hash_of(&stored.fixture.staking),
            hash_of(&stored.fixture.snarked)
        ])
    );
}

#[test]
fn a_tip_whose_staking_ledger_is_not_stored_gives_only_the_ledgers() {
    let mut stored = Stored::new(8, 3);
    // The chain crossed into an epoch whose staking ledger the store
    // doesn't have.
    let other_staking = ChainLedgers {
        staking: hash_of(&stored.fixture.staged),
        ..stored.ledgers.clone()
    };
    let parent = stored.blocks.last().unwrap().clone();
    stored.blocks.push(block_after(
        parent.hash().clone(),
        parent.height() + 1,
        parent.global_slot() + 1,
        &other_staking,
    ));
    let mem = stored.store();
    let (reason, ledgers) = ledgers_only(load_from(&mem, Some(stored.tip_slot())));
    assert!(reason.contains("staking ledger"), "{reason}");
    assert_eq!(ledgers.len(), 2);
}

#[test]
fn unfinished_restores_give_the_ledgers_then_delete_the_store() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let now = Some(stored.tip_slot());
    let set = |generation, count| {
        let mut store = reopened(&mem);
        store.set_attempts(Attempts { generation, count }).unwrap();
    };

    // A record of another generation doesn't count.
    set(7, 9);
    assert_eq!(restored_attempt(&mem, now), 1);
    // One unfinished restore: still restored.
    assert_eq!(restored_attempt(&mem, now), 2);
    assert_eq!(LEDGERS_ONLY_AFTER_ATTEMPTS, 2);
    // Two: only the ledgers, and the record is left for the store thread
    // to clear once they are handed over.
    let (reason, ledgers) = ledgers_only(load_from(&mem, now));
    assert!(reason.contains("2 restores"), "{reason}");
    assert_eq!(ledgers.len(), 2);
    assert_eq!(
        attempts(&mem),
        Some(Attempts {
            generation: 1,
            count: 3
        })
    );
    // Three: the store is deleted.
    assert_eq!(DISCARD_AFTER_ATTEMPTS, 3);
    let reason = discarded(load_from(&mem, now));
    assert!(reason.contains("3 restores"), "{reason}");
    assert!(mem.files().is_empty(), "{:?}", mem.files().keys());
    assert!(matches!(load_from(&mem, now), LoadOutcome::Absent { .. }));
}

/// A load as a new process runs it, with the restore guard its signal
/// thread holds.
fn load_guarded(mem: &MemBackend, current_slot: Option<u32>) -> (LoadOutcome, RestoreGuard) {
    let mut store = reopened(mem);
    let guard = RestoreGuard::default();
    store.set_restore_guard(guard.clone());
    let outcome = load(&mut store, &request(current_slot), None).unwrap();
    (outcome, guard)
}

/// RC7-FIXES: a clean shutdown (SIGTERM, `docker stop`) during a restore
/// takes its attempt back, so stopping a node while it restores, more often
/// than the store's limit, never gives the store up.
#[test]
fn rc7_a_clean_shutdown_during_a_restore_does_not_count() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let now = Some(stored.tip_slot());
    for _ in 0..DISCARD_AFTER_ATTEMPTS + 2 {
        let (outcome, guard) = load_guarded(&mem, now);
        assert_eq!(restored(outcome).attempt, 1);
        assert!(guard.in_progress());
        assert_eq!(
            attempts(&mem),
            Some(Attempts {
                generation: 1,
                count: 1
            })
        );
        // The signal thread, while the node replays.
        guard.take_back().unwrap().unwrap();
        assert!(!guard.in_progress());
        assert_eq!(attempts(&mem), None);
    }
    // A record from before the restore is written back as it was: a crash
    // before still counts.
    reopened(&mem)
        .set_attempts(Attempts {
            generation: 1,
            count: 1,
        })
        .unwrap();
    let (outcome, guard) = load_guarded(&mem, now);
    assert_eq!(restored(outcome).attempt, 2);
    guard.take_back().unwrap().unwrap();
    assert_eq!(
        attempts(&mem),
        Some(Attempts {
            generation: 1,
            count: 1
        })
    );
    // A record of another generation too.
    reopened(&mem)
        .set_attempts(Attempts {
            generation: 7,
            count: 9,
        })
        .unwrap();
    let (outcome, guard) = load_guarded(&mem, now);
    assert_eq!(restored(outcome).attempt, 1);
    guard.take_back().unwrap().unwrap();
    assert_eq!(
        attempts(&mem),
        Some(Attempts {
            generation: 7,
            count: 9
        })
    );
}

/// RC7-FIXES review (B1): a stop that lands after the new count is written
/// beside the record but before it is committed leaves the record as it was:
/// the shutdown is ordered against the commit by the guard's lock. Three
/// such stops in a row neither count nor give the store up.
#[test]
fn rc7_a_stop_before_the_count_is_committed_leaves_the_record() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let now = Some(stored.tip_slot());
    let before = Attempts {
        generation: 1,
        count: 1,
    };
    reopened(&mem).set_attempts(before).unwrap();
    for _ in 0..DISCARD_AFTER_ATTEMPTS {
        let mut store = reopened(&mem);
        let guard = RestoreGuard::default();
        store.set_restore_guard(guard.clone());
        // The signal thread, before the loader's commit.
        assert!(guard.shutdown_if_idle());
        let error = match load(&mut store, &request(now), None) {
            Err(error) => error,
            Ok(outcome) => panic!("{}", outcome.kind()),
        };
        assert!(error.to_string().contains("shutting down"), "{error}");
        assert!(crate::persist::load::is_transient(&error));
        assert!(!guard.in_progress());
        assert_eq!(attempts(&mem), Some(before));
    }
    // The next start counts its own attempt only.
    let (outcome, guard) = load_guarded(&mem, now);
    assert_eq!(restored(outcome).attempt, 2);
    assert!(guard.in_progress());
    // Armed: the signal thread can't stop at once, it takes the count back.
    assert!(!guard.shutdown_if_idle());
    guard.take_back().unwrap().unwrap();
    assert_eq!(attempts(&mem), Some(before));
}

/// RC7-FIXES: a restore that crashes the node (a panic, an OOM kill: no
/// take-back) still counts, so such a store is still given up; and once a
/// restore is over, a shutdown takes nothing back.
#[test]
fn rc7_a_crash_during_a_restore_still_counts() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let now = Some(stored.tip_slot());
    let (outcome, _crashed) = load_guarded(&mem, now);
    assert_eq!(restored(outcome).attempt, 1);
    let (outcome, _crashed) = load_guarded(&mem, now);
    assert_eq!(restored(outcome).attempt, 2);
    // Ledgers only: the store thread clears the record once they are handed
    // over, which ends the restore.
    let (outcome, guard) = load_guarded(&mem, now);
    ledgers_only(outcome);
    assert!(guard.in_progress());
    let mut store = reopened(&mem);
    store.set_restore_guard(guard.clone());
    store.clear_attempts().unwrap();
    assert!(guard.take_back().is_none());

    // A committed restore (`RestoreDone`) is over too.
    let (outcome, guard) = load_guarded(&mem, now);
    assert_eq!(restored(outcome).attempt, 1);
    let mut store = reopened(&mem);
    store.set_restore_guard(guard.clone());
    store.clear_attempts().unwrap();
    assert!(guard.take_back().is_none());
    assert_eq!(attempts(&mem), None);

    // Three crashes: the store is deleted before any attempt is counted,
    // with nothing to take back.
    for attempt in 1..DISCARD_AFTER_ATTEMPTS {
        let (outcome, _crashed) = load_guarded(&mem, now);
        assert_eq!(restored(outcome).attempt, attempt);
    }
    let (outcome, guard) = load_guarded(&mem, now);
    ledgers_only(outcome);
    drop(guard);
    let (outcome, guard) = load_guarded(&mem, now);
    let reason = discarded(outcome);
    assert!(reason.contains("3 restores"), "{reason}");
    assert!(guard.take_back().is_none());
    assert!(mem.files().is_empty(), "{:?}", mem.files().keys());
}

/// Two generations: the checkpoint (1), then a chain put extending its chain
/// by two blocks (2). Returns the store and the extended chain.
fn two_generation_store(stored: &Stored) -> (MemBackend, Vec<ArcBlockWithHash>) {
    let mem = stored.store();
    let tip = stored.blocks.last().unwrap();
    let mut extended = stored.blocks.clone();
    for _ in 0..2 {
        let parent = extended.last().unwrap().clone();
        extended.push(block_after(
            parent.hash().clone(),
            parent.height() + 1,
            parent.global_slot() + SLOTS_PER_BLOCK,
            &stored.ledgers,
        ));
    }
    {
        let mut store = reopened(&mem);
        let new = &extended[extended.len() - 3..];
        assert_eq!(new[0].hash(), tip.hash());
        let put = store
            .chain_put(chain_entries(new), encoded_blocks(new))
            .unwrap();
        assert_eq!(put.generation, 2);
        assert!(!put.checkpoint_invalidated);
    }
    (mem, extended)
}

#[test]
fn a_damaged_file_gives_up_its_generation_then_the_store() {
    let stored = Stored::new(10, 4);
    let now = Some(stored.tip_slot() + 10);

    // The newest generation's own block flipped: the previous one loads.
    let (mem, extended) = two_generation_store(&stored);
    let newest = block_name(extended.last().unwrap().hash());
    flip(&mem, &newest, 100);
    let restored = restored(load_from(&mem, now));
    assert_eq!(restored.generation, 1);
    assert_eq!(restored.blocks.len(), stored.blocks.len() - 1);
    // The given-up generation's manifest and own files are gone.
    assert!(mem.file(&newest).is_none());
    let (store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
    assert_eq!(store.generation(), 1);
    assert!(store.previous_manifest().is_none());

    // A file both generations reference: both are given up, and the store
    // is deleted.
    let (mem, _) = two_generation_store(&stored);
    flip(&mem, &staged_name(stored.blocks[0].hash()), 60);
    let reason = discarded(load_from(&mem, now));
    assert!(reason.contains("generation 2"), "{reason}");
    assert!(reason.contains("generation 1"), "{reason}");
    assert!(mem.files().is_empty(), "{:?}", mem.files().keys());

    // A damaged ledger file, a single generation: deleted.
    let mem = stored.store();
    flip(
        &mem,
        &ledger_full_name(&hash_of(&stored.fixture.staking)),
        200,
    );
    discarded(load_from(&mem, now));
    assert!(mem.files().is_empty());

    // Damaged parts.
    let mem = stored.store();
    flip(&mem, &parts_name(stored.blocks[0].hash()), 20);
    discarded(load_from(&mem, now));
}

/// The web node's store (I5e): the same store, its files kept by a file
/// server (`persist::proxy`) as the browser's are, restores with every check,
/// and fails closed when damaged: a flipped byte of the staking ledger gives
/// up the only generation and deletes the store, as natively.
#[test]
fn a_store_on_a_file_server_restores_and_fails_closed() {
    use crate::persist::proxy::{block_on, serve, FsClient, MemFs, ProxyBackend};

    let stored = Stored::new(12, 5);
    let now = Some(stored.tip_slot() + 3);
    let fs = MemFs::new();
    for (name, bytes) in stored.store().files() {
        fs.put("frontier/chain", &name, bytes);
    }
    // Each load as a reloaded page makes it: a new server, the old page's
    // lock let go.
    let opened = |fs: &MemFs| {
        fs.release_all();
        let (client, calls) = FsClient::channel();
        let dir = fs.dir();
        std::thread::spawn(move || block_on(serve(dir, calls)));
        let path = vec!["frontier".to_owned(), "chain".to_owned()];
        let (backend, _) = ProxyBackend::mount(client, path, 0).unwrap();
        FrontierStore::open(backend, config()).unwrap().0
    };

    let restored = restored(load(&mut opened(&fs), &request(now), None).unwrap());
    assert_eq!(restored.root.hash(), stored.blocks[0].hash());
    assert_eq!(restored.blocks.len(), stored.blocks.len() - 1);
    assert_eq!(&restored.staged_ledger_hash, &stored.ledgers.staged);
    assert_same_ledger(
        &restored.ledgers[&hash_of(&stored.fixture.staking)],
        &stored.fixture.staking,
    );
    drop(restored);

    let name = ledger_full_name(&hash_of(&stored.fixture.staking));
    let mut bytes = fs.files("frontier/chain").remove(&name).unwrap();
    bytes[200] ^= 1;
    fs.put("frontier/chain", &name, bytes);
    let reason = discarded(load(&mut opened(&fs), &request(now), None).unwrap());
    assert!(reason.contains("generation 1"), "{reason}");
    assert!(
        fs.files("frontier/chain").is_empty(),
        "{:?}",
        fs.files("frontier/chain").keys()
    );
}

/// A load that gives up a damaged generation and falls back to an older one
/// without a checkpoint reports the store it leaves, before its result
/// (I5e): the node's next checkpoint must not take the given-up generation's
/// ledgers for stored, or it fails and waits for its retry. Found in the
/// browser: a bootstrap's first chain put (generation 1), its checkpoint (2),
/// the checkpoint's staking ledger damaged.
#[test]
fn a_fall_back_reports_the_store_it_leaves() {
    use std::{cell::RefCell, rc::Rc};

    use crate::persist::{
        event::{PersistEvent, PersistLoadResult},
        writer::{LoadJob, PersistJob, StoreWorker, WriterOptions},
    };

    let stored = Stored::new(10, 4);
    let mem = MemBackend::new();
    {
        let mut store = reopened(&mem);
        store
            .chain_put(
                chain_entries(&stored.blocks),
                encoded_blocks(&stored.blocks),
            )
            .unwrap();
        assert_eq!(store.checkpoint(stored.write()).unwrap(), 2);
    }
    flip(
        &mem,
        &ledger_full_name(&hash_of(&stored.fixture.staking)),
        200,
    );

    let events = Rc::new(RefCell::new(Vec::new()));
    let mut worker = {
        let events = events.clone();
        let mem = mem.reopen();
        StoreWorker::new(
            move |_: &ChainId| Ok((mem.second_handle(), "memory".to_owned())),
            WriterOptions::default(),
            move |event| events.borrow_mut().push(event),
        )
    };
    worker.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    worker.handle(PersistJob::Load(Box::new(LoadJob {
        request: request(Some(stored.tip_slot() + 3)),
        verifier: None,
        insert: Box::new(|_| -> Result<f64, String> { panic!("nothing to hand over") }),
        control: Arc::new(super::load_control::LoadControl::default()),
    })));
    match events.borrow().as_slice() {
        [PersistEvent::Opened(opened), PersistEvent::Summary(summary), PersistEvent::Loaded(result)] =>
        {
            assert_eq!(opened.store.generation, 2);
            assert_eq!(opened.store.ledgers.len(), 2);
            assert_eq!(summary.generation, 1);
            assert!(summary.ledgers.is_empty());
            assert!(!summary.has_checkpoint());
            assert!(
                matches!(**result, PersistLoadResult::Absent { .. }),
                "{result:?}"
            );
        }
        other => panic!("{other:?}"),
    };
}

#[test]
fn a_truncated_file_drops_its_generation_at_the_open() {
    let stored = Stored::new(10, 4);
    let now = Some(stored.tip_slot() + 10);

    // The newest generation's block cut short: the open skips that slot,
    // and the previous generation restores.
    let (mem, extended) = two_generation_store(&stored);
    let newest = block_name(extended.last().unwrap().hash());
    let bytes = mem.file(&newest).unwrap();
    mem.put_file(&newest, bytes[..bytes.len() / 2].to_vec());
    assert_eq!(restored(load_from(&mem, now)).generation, 1);

    // The staged diff, which both reference: nothing is left to load.
    let (mem, _) = two_generation_store(&stored);
    let staged = staged_name(stored.blocks[0].hash());
    let bytes = mem.file(&staged).unwrap();
    mem.put_file(&staged, bytes[..bytes.len() - 1].to_vec());
    assert!(matches!(load_from(&mem, now), LoadOutcome::Absent { .. }));
    assert!(data_files(&mem).is_empty());
}

#[test]
fn a_chain_that_does_not_link_or_hash_is_damage() {
    let stored = Stored::new(8, 3);
    let now = Some(stored.tip_slot());

    // A block whose parent is not the block before it.
    let mut broken = stored.blocks.clone();
    let last = broken.pop().unwrap();
    broken.push(block_after(
        state_hash(5),
        last.height(),
        last.global_slot(),
        &stored.ledgers,
    ));
    let mem = MemBackend::new();
    {
        let mut store = reopened(&mem);
        store.checkpoint(stored.write_with(&broken)).unwrap();
    }
    let reason = discarded(load_from(&mem, now));
    assert!(reason.contains("its parent is"), "{reason}");

    // A block file holding another block than its name says (the store
    // would never write it, but the loader rehashes each block).
    let mem = stored.store();
    let (first, second) = (
        block_name(stored.blocks[1].hash()),
        block_name(stored.blocks[2].hash()),
    );
    let other = mem.file(&second).unwrap();
    {
        // Rewrite the manifest's record of the first file to the second's
        // bytes, as damage that kept the checksum would.
        let (store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
        let mut manifest = store.manifest().cloned().unwrap();
        let second_ref = manifest.file(&second).unwrap().clone();
        for file in &mut manifest.files {
            if file.name() == first {
                *file = FileRef::new(&first, second_ref.len, second_ref.crc32);
            }
        }
        let slot = MANIFEST_SLOTS
            .iter()
            .find(|slot| mem.file(slot).is_some())
            .unwrap();
        mem.put_file(slot, encode_manifest(&manifest));
    }
    mem.put_file(&first, other);
    let reason = discarded(load_from(&mem, now));
    assert!(reason.contains("holds block"), "{reason}");
}

#[test]
fn a_root_staged_ledger_that_does_not_hash_to_the_root_block_is_damage() {
    // The root block's complete staged ledger hash names another pending
    // coinbase than the stored one: the ledger itself matches, so the store
    // takes the checkpoint, but the loader's complete hash differs.
    let mut stored = Stored::new(8, 3);
    let mut staged = stored.ledgers.staged.clone();
    staged.pending_coinbase_hash = v2::PendingCoinbaseHash::from_fp(Fp::from(11u64));
    stored.ledgers.staged = staged;
    stored.blocks = synthetic_chain(state_hash(99), 3, &stored.ledgers);
    let mem = stored.store();
    let reason = discarded(load_from(&mem, Some(stored.tip_slot())));
    assert!(reason.contains("complete hash"), "{reason}");
}

#[test]
fn cancellation_during_file_reads_keeps_the_generation_and_stops_the_load() {
    use super::load_control::LoadControl;
    let stored = Stored::new(32, 3);
    let mem = stored.store();
    let mut store = reopened(&mem);
    let generation = store.generation();
    let control = LoadControl::cancelling_after(1);
    let result = super::load::load_with_progress(
        &mut store,
        &request(Some(stored.tip_slot())),
        None,
        &control,
    );
    assert!(matches!(
        result,
        Err(super::PersistError::Codec(
            super::ledger_codec::CodecError::Cancelled
        ))
    ));
    assert_eq!(store.generation(), generation);
    assert!(store.manifest().unwrap().checkpoint.is_some());
    // Cancellation is not corruption, fallback, or deletion. A fresh worker
    // can still validate and restore exactly this generation.
    let restored = restored(load(&mut store, &request(Some(stored.tip_slot())), None).unwrap());
    assert_eq!(restored.generation, generation);
}

#[test]
fn a_cancelled_worker_acknowledges_without_inserting_and_accepts_the_next_job() {
    use super::{
        event::{PersistEvent, PersistLoadResult},
        load_control::LoadControl,
        writer::{LoadJob, PersistJob, StoreWorker, WriterOptions},
    };
    use std::{cell::RefCell, rc::Rc};
    let stored = Stored::new(32, 3);
    let mem = stored.store().reopen();
    let events = Rc::new(RefCell::new(Vec::new()));
    let emit = events.clone();
    let mut worker = StoreWorker::new(
        move |_: &ChainId| Ok((mem.second_handle(), "memory".into())),
        WriterOptions::default(),
        move |event| emit.borrow_mut().push(event),
    );
    worker.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    events.borrow_mut().clear();
    let control = Arc::new(LoadControl::cancelling_after(1));
    worker.handle(PersistJob::Load(Box::new(LoadJob {
        request: request(Some(stored.tip_slot())),
        verifier: None,
        insert: Box::new(|_| panic!("an abandoned load must never insert")),
        control: control.clone(),
    })));
    assert!(
        matches!(events.borrow().as_slice(), [PersistEvent::Loaded(result)] if matches!(**result, PersistLoadResult::Cancelled))
    );
    let progress = control.progress();
    events.borrow_mut().clear();
    worker.handle(PersistJob::ChainPut {
        chain: Vec::new(),
        blocks: Vec::new(),
    });
    assert!(!events.borrow().is_empty());
    assert_eq!(control.progress(), progress);
}

#[test]
fn an_io_error_keeps_the_store() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let (mut store, _) = FrontierStore::open(mem.reopen(), config()).unwrap();
    // Every call fails from here, as an unreadable disk would.
    mem.set_faults(Faults {
        fail_at_call: Some(0),
        ..Faults::default()
    });
    let error = match load(&mut store, &request(Some(stored.tip_slot())), None) {
        Err(error) => error,
        Ok(outcome) => panic!("{}", outcome.kind()),
    };
    assert!(error.to_string().contains("injected"), "{error}");
    drop(store);
    mem.set_faults(Faults::default());
    // Nothing was deleted, and no attempt was counted.
    assert_eq!(restored_attempt(&mem, Some(stored.tip_slot())), 1);
}

#[test]
fn transient_and_damage_errors_are_told_apart() {
    use crate::persist::load::is_transient;
    let io = |kind| PersistError::Io(io::Error::from(kind));
    assert!(is_transient(&io(io::ErrorKind::Other)));
    assert!(is_transient(&io(io::ErrorKind::PermissionDenied)));
    assert!(is_transient(&PersistError::Locked));
    assert!(!is_transient(&io(io::ErrorKind::UnexpectedEof)));
    assert!(!is_transient(&io(io::ErrorKind::InvalidData)));
    assert!(!is_transient(&PersistError::MissingFile("x".into())));
    assert!(!is_transient(&PersistError::corrupt("x", "bad")));
    assert!(!is_transient(&PersistError::Codec(CodecError::WrongBase)));
    assert!(is_transient(&PersistError::Codec(CodecError::Io(
        io::Error::other("busy")
    ))));
}

// Checksum-valid tampering (the I5d1 review).

/// An independently built, fully rehashed [B, A, ...] base followed by a
/// diff restoring exactly [A, B, ...]. Sequential replacement loses A's
/// lookup even though every leaf and hash is correct (follow-up review).
fn swapped_base_diff(target: &Mask) -> (Mask, LedgerDiff) {
    let mut base = if target.has_token_owners() {
        Mask::create_with_token_owners(target.depth() as usize)
    } else {
        Mask::create(target.depth() as usize)
    };
    for index in [1, 0].into_iter().chain(2..target.num_accounts() as u64) {
        let account = target.get_at_index(AccountIndex(index)).unwrap();
        base.get_or_create_account(account.id(), *account).unwrap();
    }
    let diff = merkle_diff(target, &base).unwrap();
    let mut loaded = read_bytes(&diff_bytes(&diff), 4096, Some(&base), VerifyMode::Full)
        .unwrap()
        .0;
    // Both changed-path verification and a further full rehash miss this.
    ledger_codec::verify_full(&mut loaded).unwrap();
    for index in 0..target.num_accounts() as u64 {
        assert_eq!(
            loaded.get_at_index(AccountIndex(index)),
            target.get_at_index(AccountIndex(index))
        );
    }
    assert_eq!(hash_of(&loaded), hash_of(target));
    let a = target.get_at_index(AccountIndex(0)).unwrap().id();
    assert_eq!(target.index_of_account(a.clone()), Some(AccountIndex(0)));
    assert_eq!(loaded.index_of_account(a), None);
    assert!(loaded.validate_account_indexes().is_err());
    (base, diff)
}

/// The store writer records valid lengths and checksums for the substituted
/// base and target diff. The checkpoint, target leaves and root are unchanged.
fn write_swapped_base(stored: &Stored, store: &mut FrontierStore<MemBackend>) -> u64 {
    let (base, diff) = swapped_base_diff(&stored.fixture.snarked);
    let mut write = stored.write();
    write.ledgers[1].1 = Some(LedgerWrite::Diff(diff));
    write
        .ledgers
        .push((hash_of(&base), Some(LedgerWrite::Full(base))));
    store.checkpoint(write).unwrap()
}

#[test]
fn a_swapped_base_corrupting_indexes_is_rejected_on_restore() {
    let stored = Stored::new(8, 4);
    let mem = MemBackend::new();
    write_swapped_base(&stored, &mut reopened(&mem));
    let reason = discarded(load_from(&mem, Some(stored.tip_slot())));
    assert!(reason.contains("id-to-address"), "{reason}");
    assert!(mem.files().is_empty());
}

#[test]
fn a_swapped_base_corrupting_indexes_is_rejected_on_ledgers_only() {
    let stored = Stored::new(8, 4);
    let mem = MemBackend::new();
    write_swapped_base(&stored, &mut reopened(&mem));
    let reason = discarded(load_from(&mem, Some(stored.tip_slot() + 4 * 290 + 1)));
    assert!(reason.contains("id-to-address"), "{reason}");
    assert!(mem.files().is_empty());
}

#[test]
fn a_swapped_base_corrupting_indexes_falls_back_to_the_previous_generation() {
    let stored = Stored::new(8, 4);
    let mem = MemBackend::new();
    let mut write = stored.write();
    write.ledgers[1].1 = Some(LedgerWrite::Full(stored.fixture.snarked.clone()));
    reopened(&mem).checkpoint(write).unwrap();
    let newest_slot = MANIFEST_SLOTS
        .iter()
        .find(|slot| mem.file(slot).is_none())
        .unwrap();

    // Use a separate writer: a real writer reuses an existing target's
    // immutable file rather than replacing it with another diff.
    let bad = MemBackend::new();
    write_swapped_base(&stored, &mut reopened(&bad));
    let mut manifest = reopened(&bad).manifest().unwrap().clone();
    manifest.generation = 2;
    for file in &mut manifest.files {
        let name = file.name().to_owned();
        if let Some(bytes) = mem.file(&name) {
            // The two writes may encode different non-hashed scan-state
            // metadata. Keep every shared immutable file and its checksum.
            *file = FileRef::new(&name, bytes.len() as u64, crc32fast::hash(&bytes));
        } else {
            mem.put_file(&name, bad.file(&name).unwrap());
        }
    }
    mem.put_file(newest_slot, encode_manifest(&manifest));
    let loaded = restored(load_from(&mem, Some(stored.tip_slot())));
    assert_eq!(loaded.generation, 1);
    assert_same_ledger(
        &loaded.ledgers[&stored.ledgers.snarked],
        &stored.fixture.snarked,
    );
}

#[test]
fn a_staged_diff_corrupting_indexes_is_rejected_on_restore() {
    let mut stored = Stored::new(8, 4);
    let (base, diff) = swapped_base_diff(&stored.fixture.staged);
    stored.fixture.snarked = base;
    stored.ledgers.snarked = hash_of(&stored.fixture.snarked);
    stored.blocks = synthetic_chain(state_hash(99), 4, &stored.ledgers);
    let mut write = stored.write();
    write.ledgers[1].1 = Some(LedgerWrite::Full(stored.fixture.snarked.clone()));
    write.staged = diff;
    let mem = MemBackend::new();
    reopened(&mem).checkpoint(write).unwrap();
    let reason = discarded(load_from(&mem, Some(stored.tip_slot())));
    assert!(reason.contains("id-to-address"), "{reason}");
    assert!(mem.files().is_empty());
}

/// The blockchain proofs of real mainnet blocks verify in a batch; a header
/// whose proof was swapped for another block's keeps its state hash and is
/// rejected.
#[test]
fn block_proofs_are_verified_in_a_batch() {
    use ledger::proofs::verification::verify_blocks;

    let verifier = mainnet_verifier();
    let blocks = mainnet_blocks();
    let headers = blocks
        .iter()
        .map(|block| block.header())
        .collect::<Vec<_>>();
    assert!(verify_blocks(&headers, &verifier.index, &verifier.srs));
    assert!(verify_blocks(&headers[..1], &verifier.index, &verifier.srs));

    let mut swapped = blocks[0].header().clone();
    swapped.protocol_state_proof = blocks[1].header().protocol_state_proof.clone();
    let swapped_block = ArcBlockWithHash::try_new(Arc::new(v2::MinaBlockBlockStableV2 {
        header: swapped.clone(),
        body: blocks[0].body().clone(),
    }))
    .unwrap();
    assert_eq!(swapped_block.hash(), blocks[0].hash());
    // Full batches of 32 (I5d4) and partial tails; each single bad proof
    // must fail regardless of which parallel chunk or position contains it:
    // every position of short batches, and the first, last and boundary
    // positions of long ones.
    for len in [0, 1, 2, 7, 8, 9, 31, 32, 33, 65] {
        let mut batch = vec![blocks[0].header(); len];
        assert!(
            verify_blocks(&batch, &verifier.index, &verifier.srs),
            "valid batch {len}"
        );
        let positions = if len <= 9 {
            (0..len).collect::<Vec<_>>()
        } else {
            let mut positions = vec![0, 30, 31, 32, 33, 63, 64, len - 1];
            positions.retain(|&bad| bad < len);
            positions.dedup();
            positions
        };
        for bad in positions {
            batch[bad] = &swapped;
            assert!(
                !verify_blocks(&batch, &verifier.index, &verifier.srs),
                "batch {len}, bad proof {bad}"
            );
            batch[bad] = blocks[0].header();
        }
    }
}

/// The synthetic blocks carry a proof of another protocol state, as blocks
/// whose proofs were replaced would: with the node's verifier, a chain of
/// them is damage and the store is deleted, a root-only chain too. Without a
/// verifier the same store restores, so the proofs are what is rejected.
#[test]
fn a_stored_block_whose_proof_was_replaced_is_damage() {
    let verifier = mainnet_verifier();
    for len in [4, 1] {
        let stored = Stored::new(8, len);
        let mem = stored.store();
        let now = request(Some(stored.tip_slot()));
        let reason = discarded(load(&mut reopened(&mem), &now, Some(&verifier)).unwrap());
        assert!(reason.contains("proofs"), "{len} blocks: {reason}");
        assert!(mem.files().is_empty(), "{:?}", mem.files().keys());

        let mem = stored.store();
        restored(load(&mut reopened(&mem), &now, None).unwrap());
    }
}

/// A body changed under its header: the state hash, which covers only the
/// header's protocol state, is the same, but the body no longer hashes to the
/// header's body reference. The root is checked too, which the replay never
/// applies, and a root-only chain.
#[test]
fn a_block_body_that_does_not_match_its_header_is_damage() {
    use v2::StagedLedgerDiffDiffPreDiffWithAtMostTwoCoinbaseStableV2Coinbase as Coinbase;

    let tampered = |block: &ArcBlockWithHash| {
        let mut changed = (*block.block).clone();
        let diff = &mut changed.body.staged_ledger_diff.diff.0;
        diff.coinbase = Coinbase::Zero;
        diff.completed_works = List::new();
        let changed = ArcBlockWithHash::try_new(Arc::new(changed)).unwrap();
        assert_eq!(changed.hash(), block.hash());
        changed
    };
    for (len, at) in [(4, 0), (4, 2), (1, 0)] {
        let stored = Stored::new(8, len);
        let mut blocks = stored.blocks.clone();
        blocks[at] = tampered(&blocks[at]);
        let mem = MemBackend::new();
        reopened(&mem)
            .checkpoint(stored.write_with(&blocks))
            .unwrap();
        let reason = discarded(load_from(&mem, Some(stored.tip_slot())));
        assert!(
            reason.contains("InvalidBodyReference"),
            "{len} blocks, block {at}: {reason}"
        );
        assert!(mem.files().is_empty());
    }
}

/// A block of another chain (another genesis state hash) is damage.
#[test]
fn a_stored_block_of_another_genesis_is_damage() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let mut other = request(Some(stored.tip_slot()));
    other.genesis = Some(BlockWithHash {
        hash: state_hash(77),
        block: our_genesis().block,
    });
    let reason = discarded(load(&mut reopened(&mem), &other, None).unwrap());
    assert!(reason.contains("InvalidGenesisProtocolState"), "{reason}");
}

/// A full ledger file with one account changed and every stored hash kept:
/// a sampled check can miss it; the loader rehashes every account.
fn tamper_unsampled_account(bytes: &[u8]) -> Vec<u8> {
    let (_, header) = read_bytes(bytes, 1 << 20, None, VerifyMode::Full).unwrap();
    for index in 0..header.num_accounts as usize {
        let bad = edit_full_file(bytes, |_, accounts, _| {
            let nonce = accounts[index].nonce.as_u32();
            accounts[index].nonce = nonce.wrapping_add(1).into();
        });
        // What the loader used to check (64 sampled paths) passes, the full
        // rehash doesn't.
        if read_bytes(&bad, 1 << 20, None, VerifyMode::Sample(64)).is_ok() {
            assert!(read_bytes(&bad, 1 << 20, None, VerifyMode::Full).is_err());
            return bad;
        }
    }
    panic!("every account is sampled");
}

/// An account changed under its old hashes in a stored ledger, with the
/// file's checksum recorded: rejected by a restore, and by a ledgers-only
/// load, whose ledgers would otherwise be copy origins whose cached hashes a
/// ledger sync trusts. The store is deleted and the node bootstraps.
#[test]
fn an_account_changed_under_its_old_hashes_is_damage() {
    let stored = Stored::new(200, 3);
    let name = ledger_full_name(&hash_of(&stored.fixture.staking));
    let now = Some(stored.tip_slot());

    let mem = stored.store();
    replace_file(
        &mem,
        &name,
        tamper_unsampled_account(&mem.file(&name).unwrap()),
    );
    let reason = discarded(load_from(&mem, now));
    assert!(reason.contains("stored hash of node"), "{reason}");
    assert!(mem.files().is_empty());

    // The same, when the store would give only its ledgers.
    let mem = stored.store();
    replace_file(
        &mem,
        &name,
        tamper_unsampled_account(&mem.file(&name).unwrap()),
    );
    reopened(&mem)
        .set_attempts(Attempts {
            generation: 1,
            count: LEDGERS_ONLY_AFTER_ATTEMPTS,
        })
        .unwrap();
    let reason = discarded(load_from(&mem, now));
    assert!(reason.contains("stored hash of node"), "{reason}");
    assert!(mem.files().is_empty());

    // An untouched store of the same shape restores, and its ledgers-only
    // load hands over every ledger.
    let mem = stored.store();
    restore_and_commit(&mem, now);
    reopened(&mem)
        .set_attempts(Attempts {
            generation: 1,
            count: LEDGERS_ONLY_AFTER_ATTEMPTS,
        })
        .unwrap();
    let (_, ledgers) = ledgers_only(load_from(&mem, now));
    assert_same_ledger(
        &ledgers[&hash_of(&stored.fixture.staking)],
        &stored.fixture.staking,
    );
}

/// A diff file (here the root staged ledger's) with a changed account entry
/// and its hashes kept. With more changed accounts than a sample covers, a
/// sampled check could miss it.
#[test]
fn a_diff_entry_changed_under_its_old_hashes_is_damage() {
    let mut stored = Stored::new(200, 3);
    // 150 accounts changed between the root snarked and staged ledgers.
    stored.fixture.staged = modified(&stored.fixture.snarked, &(0..150).collect::<Vec<_>>(), 0);
    stored.ledgers.staged = complete_hash(&stored.fixture.staged);
    let parent = state_hash(99);
    stored.blocks = synthetic_chain(parent, 3, &stored.ledgers);
    let mem = stored.store();
    let name = staged_name(stored.blocks[0].hash());
    let bytes = mem.file(&name).unwrap();

    let base = &stored.fixture.snarked;
    let bad = (0..150)
        .map(|entry| edit_diff_entry(&bytes, entry))
        .find(|bad| read_bytes(bad, 1 << 20, Some(base), VerifyMode::Sample(64)).is_ok())
        .expect("an entry the sample misses");
    assert!(read_bytes(&bad, 1 << 20, Some(base), VerifyMode::Full).is_err());
    replace_file(&mem, &name, bad);
    let reason = discarded(load_from(&mem, Some(stored.tip_slot())));
    assert!(reason.contains("stored hash of node"), "{reason}");
}

/// A checksum-valid full ledger with a wrong non-root Merkle hash must fail
/// the full check, even though its stored root still matches its file name.
#[test]
fn a_stored_ledger_with_a_wrong_inner_hash_is_damage() {
    let stored = Stored::new(8, 3);
    let mem = stored.store();
    let name = ledger_full_name(&hash_of(&stored.fixture.staking));
    let bytes = mem.file(&name).unwrap();
    let mut reader = bytes.as_slice();
    let header = read_header(&mut reader).unwrap();
    let accounts = (0..header.num_entries)
        .map(|_| v2::MinaBaseAccountBinableArgStableV2::binprot_read(&mut reader).unwrap())
        .collect::<Vec<_>>();
    let mut hashes = (0..header.num_hashes)
        .map(|_| {
            (
                u64::binprot_read(&mut reader).unwrap(),
                v2::LedgerHash::binprot_read(&mut reader).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    assert!(reader.is_empty());
    // The first hash is the root. Change another stored node, leaving the
    // root and every account intact, and update the manifest's CRC.
    assert!(hashes.len() > 1);
    hashes[1].1 = v2::LedgerHash::from_fp(Fp::from(5u64));
    let mut bad = Vec::new();
    bad.extend_from_slice(&LEDGER_MAGIC);
    bad.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    header.binprot_write(&mut bad).unwrap();
    for account in accounts {
        account.binprot_write(&mut bad).unwrap();
    }
    for (index, hash) in hashes {
        index.binprot_write(&mut bad).unwrap();
        hash.binprot_write(&mut bad).unwrap();
    }
    replace_file(&mem, &name, bad);
    let reason = discarded(load_from(&mem, Some(stored.tip_slot())));
    assert!(reason.contains("stored hash of node"), "{reason}");
}

/// A diff file with the nonce of its `entry`-th account entry changed, every
/// hash kept.
fn edit_diff_entry(bytes: &[u8], entry: usize) -> Vec<u8> {
    let mut reader = bytes;
    let header = read_header(&mut reader).unwrap();
    let mut entries = (0..header.num_entries)
        .map(|_| {
            (
                u64::binprot_read(&mut reader).unwrap(),
                v2::MinaBaseAccountBinableArgStableV2::binprot_read(&mut reader).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    let hashes = reader.to_vec();
    let nonce = entries[entry].1.nonce.as_u32();
    entries[entry].1.nonce = nonce.wrapping_add(1).into();
    let mut out = Vec::new();
    out.extend_from_slice(&LEDGER_MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    header.binprot_write(&mut out).unwrap();
    for (index, account) in &entries {
        index.binprot_write(&mut out).unwrap();
        account.binprot_write(&mut out).unwrap();
    }
    out.extend_from_slice(&hashes);
    out
}

/// A full load of a diff rehashes its changed accounts and the nodes above
/// them (`verify_changed`), and agrees with a rehash of the whole ledger: a
/// changed node's hash replaced in the file (the root's kept) passes a
/// root-only load and fails the full one.
#[test]
fn a_full_load_of_a_diff_rehashes_its_changed_paths() {
    use crate::persist::ledger_codec::verify_full;

    let fixture = Fixture::new(300);
    let changed = (0..120).step_by(3).collect::<Vec<_>>();
    let staged = modified(&fixture.snarked, &changed, 5);
    let diff = merkle_diff(&staged, &fixture.snarked).unwrap();
    let base = Some(&fixture.snarked);
    let (mut loaded, _) = read_bytes(&diff_bytes(&diff), 4096, base, VerifyMode::Full).unwrap();
    assert_same_ledger(&loaded, &staged);
    verify_full(&mut loaded).unwrap();

    let mut tampered = diff.clone();
    let last = tampered.hashes.len() - 1;
    tampered.hashes[last].1 += Fp::from(1u64);
    let bytes = diff_bytes(&tampered);
    let (mut unchecked, _) = read_bytes(&bytes, 4096, base, VerifyMode::RootOnly).unwrap();
    assert!(verify_full(&mut unchecked).is_err());
    assert!(matches!(
        read_bytes(&bytes, 4096, base, VerifyMode::Full),
        Err(CodecError::HashMismatch(_))
    ));
}

#[test]
fn ledger_0201_a_stalled_read_is_abandoned_after_the_cancellation_cap() {
    use super::{
        event::{PersistEvent, PersistLoadResult},
        load_control::LoadControl,
        writer::{LoadJob, PersistJob, StoreWorker, WriterOptions},
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Mutex,
    };
    let stored = Stored::new(32, 3);
    let mem = stored.store().reopen();
    let inspect = mem.second_handle();
    let control = Arc::new(LoadControl::default());
    let worker_control = control.clone();
    let slot = stored.tip_slot();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let (opened_tx, opened_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || {
            let mut worker = StoreWorker::new(
                move |_: &ChainId| Ok((mem.second_handle(), "memory".into())),
                WriterOptions::default(),
                move |event| {
                    done_tx.send(event).unwrap();
                },
            );
            worker.handle(PersistJob::Open {
                chain_id: MAINNET_CHAIN_ID,
                genesis_state_hash: genesis(),
            });
            // Install after opening, so the injected read is in the load.
            let first = AtomicBool::new(true);
            let release = Mutex::new(release_rx);
            inspect.set_before_read(Arc::new(move || {
                if first.swap(false, Ordering::SeqCst) {
                    entered_tx.send(()).unwrap();
                    let _ = release.lock().unwrap().recv();
                }
            }));
            opened_tx.send(()).unwrap();
            worker.handle(PersistJob::Load(Box::new(LoadJob {
                request: request(Some(slot)),
                verifier: None,
                insert: Box::new(|_| panic!("a late cancelled load must never insert")),
                control: worker_control,
            })));
        });
        // Drop releases the read even if a regression assertion panics.
        struct Release(Option<mpsc::Sender<()>>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.take();
            }
        }
        let release = Release(Some(release_tx));
        let wait = std::time::Duration::from_secs(30);
        opened_rx.recv_timeout(wait).unwrap();
        assert!(matches!(
            done_rx.recv_timeout(wait).unwrap(),
            PersistEvent::Opened(_)
        ));
        entered_rx.recv_timeout(wait).unwrap();
        crate::transition_frontier::persist::transition_frontier_restore_tests::stalled_load_cancellation_deadline(&control);
        assert!(done_rx.try_recv().is_err(), "the read is still blocked");
        drop(release);
        worker.join().unwrap();
        assert!(
            matches!(done_rx.recv_timeout(wait).unwrap(), PersistEvent::Loaded(result)
            if matches!(*result, PersistLoadResult::Cancelled))
        );
        assert!(!control.begin_handoff());
    });
}
