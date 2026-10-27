//! Tests of what I5c adds to the store (R20 D3, D8): full ledgers streamed a
//! piece at a time give the bytes `write_full` writes, the store thread's jobs
//! over the in-memory backend, and crashes in checkpoints that stream.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use ledger::scan_state::{pending_coinbase::PendingCoinbase, scan_state::ScanState};
use mina_core::{block::ArcBlockWithHash, constants::constraint_constants};
use mina_p2p_messages::list::List;

use super::*;
use crate::{
    p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases,
    persist::{
        backend::Faults,
        event::{
            PersistEvent, PersistSnapshotInfo, PersistStoreSummary, PersistStoredLedger,
            PersistWriteError,
        },
        ledger_codec::{write_full_from, STREAM_ACCOUNTS, STREAM_HASHES},
        test_fixtures::{mainnet_blocks, MaskSource},
        writer::{
            is_out_of_space, CheckpointJob, DoneGuard, PartsSnapshot, PersistJob, StoreWorker,
            WriterOptions, MAX_IO_ERRORS_IN_A_ROW,
        },
    },
};

/// `ledger`'s full file streamed in pieces of `accounts` accounts and
/// `hashes` hashes, and the number of pieces read.
fn streamed(ledger: &Mask, accounts: u64, hashes: u64) -> (Vec<u8>, u64) {
    let source = MaskSource::new(ledger, accounts, hashes);
    let mut bytes = Vec::new();
    write_full_from(&mut bytes, &source).unwrap();
    (bytes, source.parts.load(Ordering::SeqCst) as u64)
}

#[test]
fn streamed_full_files_are_the_bytes_write_full_writes() {
    for (num_accounts, token_owners) in [
        (0, false),
        (1, false),
        (37, false),
        (37, true),
        (300, false),
    ] {
        let ledger = random_ledger_with(num_accounts, token_owners);
        let want = full_bytes(&ledger);
        let num_hashes = count_nonempty_nodes(num_accounts as u64, DEPTH as u32);
        for (accounts, hashes) in [(1, 1), (7, 13), (64, 1), (STREAM_ACCOUNTS, STREAM_HASHES)] {
            let (bytes, parts) = streamed(&ledger, accounts, hashes);
            assert!(
                bytes == want,
                "{num_accounts} accounts, pieces of {accounts} accounts and {hashes} hashes"
            );
            assert_eq!(
                parts,
                (num_accounts as u64).div_ceil(accounts) + num_hashes.div_ceil(hashes)
            );
        }
        let (loaded, header) = read_bytes(&want, 4096, None, VerifyMode::Full).unwrap();
        assert_eq!(header.kind, LedgerFileKind::Full);
        assert_same_ledger(&loaded, &ledger);
    }
}

#[test]
fn a_ledger_that_changes_while_it_is_streamed_is_an_error() {
    // An account changed after the header was taken.
    let mut ledger = random_ledger(20);
    let source = MaskSource::new(&ledger, 5, 7);
    let mut account = ledger.get_at_index(AccountIndex(3)).unwrap();
    account.nonce = account.nonce.incr();
    ledger.set_at_index(AccountIndex(3), account).unwrap();
    let error = write_full_from(&mut Vec::new(), &source).unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");

    // An account appended.
    let mut ledger = random_ledger(20);
    let source = MaskSource::new(&ledger, 5, 7);
    let account = Account::rand();
    ledger.get_or_create_account(account.id(), account).unwrap();
    let error = write_full_from(&mut Vec::new(), &source).unwrap_err();
    assert!(error.to_string().contains("changed"), "{error}");
}

fn parts_snapshot(staged: &Mask) -> PartsSnapshot {
    let constants = constraint_constants();
    let mut pending_coinbase = PendingCoinbase::create(constants.pending_coinbase_depth);
    // The conversion to wire types needs it computed, as on the ledger thread.
    pending_coinbase.merkle_root();
    PartsSnapshot {
        scan_state: ScanState::empty(constants),
        pending_coinbase,
        staged_ledger_hash: hash_of(staged),
        needed_blocks: List::new(),
    }
}

fn chain_of(blocks: &[ArcBlockWithHash]) -> Vec<ChainEntry> {
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

fn encoded(block: &ArcBlockWithHash) -> Vec<u8> {
    let mut bytes = Vec::new();
    block.block.binprot_write(&mut bytes).unwrap();
    bytes
}

/// A checkpoint job as the ledger thread hands it over, at `chain`'s root,
/// whose guard sets `released`.
fn checkpoint_job(
    fixture: &Fixture,
    chain: &[ChainEntry],
    ledgers: Vec<(v2::LedgerHash, Option<LedgerWrite>)>,
    released: &Arc<AtomicBool>,
) -> PersistJob {
    let released = released.clone();
    PersistJob::Checkpoint(Box::new(CheckpointJob {
        checkpoint: Checkpoint {
            root: chain[0].hash.clone(),
            root_height: chain[0].height,
            root_snarked_ledger: hash_of(&fixture.snarked),
            root_staged_ledger_hash: staged_ledger_hash(&fixture.staged),
        },
        ledgers,
        staged: merkle_diff(&fixture.staged, &fixture.snarked).unwrap(),
        parts: parts_snapshot(&fixture.staged),
        chain: chain.to_vec(),
        snapshot: PersistSnapshotInfo::default(),
        stream_stats: Default::default(),
        on_done: Some(DoneGuard::new(move || {
            released.store(true, Ordering::SeqCst);
        })),
    }))
}

/// The store thread's jobs, run by a [`StoreWorker`] on the test's thread
/// over the in-memory backend: open, chain puts, a checkpoint whose staking
/// ledger is streamed, a checkpoint whose stream fails, and a reset.
#[test]
fn the_store_thread_opens_puts_chains_and_writes_streamed_checkpoints() {
    let mem = MemBackend::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    let worker = |mem: MemBackend, reset: bool| {
        let events = events.clone();
        StoreWorker::new(
            move |chain_id: &ChainId| {
                assert_eq!(chain_id, &MAINNET_CHAIN_ID);
                Ok((mem.second_handle(), "memory".to_owned()))
            },
            WriterOptions {
                reset,
                node_commit: "i5c-tests".to_owned(),
                ..Default::default()
            },
            move |event| events.borrow_mut().push(event),
        )
    };
    let take = || events.borrow_mut().drain(..).collect::<Vec<_>>();
    let mut writer = worker(mem.second_handle(), false);
    let blocks = mainnet_blocks();
    let chain = chain_of(&blocks);
    let chain_put = || PersistJob::ChainPut {
        chain: chain.clone(),
        blocks: blocks.clone(),
    };

    // Nothing is written before the store is open.
    writer.handle(chain_put());
    match take().as_slice() {
        [PersistEvent::ChainPut(Err(error))] => {
            assert!(error.error.contains("not open"), "{}", error.error);
            assert!(!error.disable);
        }
        other => panic!("{other:?}"),
    }
    writer.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    match take().as_slice() {
        [PersistEvent::Opened(opened)] => {
            assert_eq!(opened.outcome, "empty");
            assert_eq!(opened.dir, "memory");
            assert_eq!(opened.store, PersistStoreSummary::default());
        }
        other => panic!("{other:?}"),
    }

    // The blocks are written once.
    writer.handle(chain_put());
    match take().as_slice() {
        [PersistEvent::ChainPut(Ok(done))] => {
            assert_eq!(done.generation, 1);
            assert!(!done.checkpoint_invalidated);
            assert_eq!(done.written.files, 2);
            assert_eq!(
                done.written.bytes,
                blocks.iter().map(|b| encoded(b).len() as u64).sum::<u64>()
            );
            assert!(done.written.manifest_bytes > 0);
            assert_eq!(done.store.chain_len, 2);
            assert_eq!(done.store.tip.as_ref(), Some(blocks[1].hash()));
            assert!(!done.store.has_checkpoint());
        }
        other => panic!("{other:?}"),
    }
    writer.handle(chain_put());
    match take().as_slice() {
        [PersistEvent::ChainPut(Ok(done))] => {
            assert_eq!(done.generation, 1);
            assert_eq!(done.written.files, 0);
        }
        other => panic!("{other:?}"),
    }

    // A checkpoint at the first block: the staking ledger streamed whole, the
    // root snarked ledger as a diff over it.
    let fixture = Fixture::new(40);
    let (staking, snarked) = (hash_of(&fixture.staking), hash_of(&fixture.snarked));
    let source = Arc::new(MaskSource::new(&fixture.staking, 7, 13));
    let released = Arc::new(AtomicBool::new(false));
    writer.handle(checkpoint_job(
        &fixture,
        &chain,
        vec![
            (staking.clone(), Some(LedgerWrite::Stream(source.clone()))),
            (
                snarked.clone(),
                Some(choose_ledger_write(
                    &fixture.snarked,
                    std::slice::from_ref(&fixture.staking),
                )),
            ),
        ],
        &released,
    ));
    let done = match take().as_slice() {
        [PersistEvent::Checkpoint(Ok(done))] => done.clone(),
        other => panic!("{other:?}"),
    };
    assert!(released.load(Ordering::SeqCst), "the pins are released");
    assert_eq!(
        source.parts.load(Ordering::SeqCst) as u64,
        40u64.div_ceil(7) + count_nonempty_nodes(40, DEPTH as u32).div_ceil(13)
    );
    assert_eq!(done.generation, 2);
    assert_eq!(done.root, chain[0].hash);
    assert_eq!(
        done.store
            .ledgers
            .iter()
            .map(|PersistStoredLedger { hash, base }| (hash.clone(), base.clone()))
            .collect::<BTreeMap<_, _>>(),
        BTreeMap::from([
            (staking.clone(), None),
            (snarked.clone(), Some(staking.clone())),
        ])
    );
    assert_eq!(done.store.checkpoint_root.as_ref(), Some(&chain[0].hash));

    let (mut store, report) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
    assert!(matches!(
        report.outcome,
        OpenOutcome::Opened { generation: 2 }
    ));
    let manifest = store.manifest().cloned().unwrap();
    // Written: the staking ledger whole, the diff, the staged diff and the
    // parts, with these sizes.
    let new_files = [
        ledger_full_name(&staking),
        ledger_diff_name(&snarked),
        staged_name(&chain[0].hash),
        parts_name(&chain[0].hash),
    ];
    assert_eq!(done.written.files, 4);
    assert_eq!(
        done.written.bytes,
        new_files
            .iter()
            .map(|name| manifest.file(name).unwrap().len)
            .sum::<u64>()
    );
    assert_eq!(
        done.parts_bytes,
        manifest.file(&parts_name(&chain[0].hash)).unwrap().len
    );
    assert_eq!(
        done.store.bytes,
        manifest.files.iter().map(|f| f.len).sum::<u64>()
    );
    // And it all loads back.
    let ledgers = store.read_ledgers(&manifest, VerifyMode::Full).unwrap();
    assert_same_ledger(&ledgers[&staking], &fixture.staking);
    assert_same_ledger(&ledgers[&snarked], &fixture.snarked);
    let staged = store
        .read_staged(&manifest, &ledgers[&snarked], VerifyMode::Full)
        .unwrap();
    assert_same_ledger(&staged, &fixture.staged);
    let parts = store
        .read_blob(manifest.file(&parts_name(&chain[0].hash)).unwrap())
        .unwrap();
    let parts = StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut parts.as_slice()).unwrap();
    assert_eq!(parts.staged_ledger_hash, hash_of(&fixture.staged));
    let scan_state = ScanState::try_from(&parts.scan_state).unwrap();
    assert!(scan_state.required_state_hashes().is_empty());
    for block in &blocks {
        let bytes = store
            .read_blob(manifest.file(&block_name(block.hash())).unwrap())
            .unwrap();
        assert_eq!(bytes, encoded(block));
    }

    // A checkpoint whose stream fails midway (its pin was released): an
    // error, the pins released all the same, the store as it was and the torn
    // file gone.
    let other = Fixture::new(30);
    let failing = Arc::new(MaskSource::new(&other.staking, 7, 13).failing_at(3));
    let released = Arc::new(AtomicBool::new(false));
    writer.handle(checkpoint_job(
        &other,
        &chain[1..],
        vec![
            (
                hash_of(&other.staking),
                Some(LedgerWrite::Stream(failing.clone())),
            ),
            (
                hash_of(&other.snarked),
                Some(choose_ledger_write(
                    &other.snarked,
                    std::slice::from_ref(&other.staking),
                )),
            ),
        ],
        &released,
    ));
    match take().as_slice() {
        [PersistEvent::Checkpoint(Err(error))] => {
            assert!(error.error.contains("released"), "{}", error.error);
            assert!(!error.disable);
        }
        other => panic!("{other:?}"),
    }
    assert!(released.load(Ordering::SeqCst));
    assert_eq!(failing.parts.load(Ordering::SeqCst), 4);
    let (store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
    assert_eq!(store.manifest().unwrap().generation, 2);
    for name in data_files(&mem) {
        assert!(
            store.manifest().unwrap().references(&name),
            "{name} is left over"
        );
    }

    // A reset at the next open deletes everything.
    let mut writer = worker(mem.reopen(), true);
    writer.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    match take().as_slice() {
        [PersistEvent::Opened(opened)] => {
            assert_eq!(opened.outcome, "reset: requested");
            assert!(opened.deleted >= 6, "{}", opened.deleted);
            assert_eq!(opened.store, PersistStoreSummary::default());
        }
        other => panic!("{other:?}"),
    }
    assert!(data_files(&mem).is_empty());
}

#[test]
fn a_full_disk_turns_persistence_off() {
    let full = |kind| PersistError::Io(io::Error::from(kind));
    assert!(is_out_of_space(&full(io::ErrorKind::StorageFull)));
    assert!(is_out_of_space(&full(io::ErrorKind::QuotaExceeded)));
    assert!(is_out_of_space(&PersistError::Codec(CodecError::Io(
        io::Error::from(io::ErrorKind::StorageFull)
    ))));
    assert!(!is_out_of_space(&PersistError::Io(io::Error::other(
        "injected failure"
    ))));
    assert!(!is_out_of_space(&PersistError::Locked));
}

/// Failures that would repeat at every block turn persistence off for the run
/// rather than being retried: [`MAX_IO_ERRORS_IN_A_ROW`] I/O errors in a row
/// (a job that succeeds starts the count again), and at once a streamed
/// ledger whose ledger thread is gone. A full disk is the test above.
#[test]
fn failures_that_repeat_turn_persistence_off() {
    let mem = MemBackend::new();
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut writer = {
        let (mem, events) = (mem.second_handle(), events.clone());
        StoreWorker::new(
            move |_: &ChainId| Ok((mem.second_handle(), "memory".to_owned())),
            WriterOptions::default(),
            move |event| events.borrow_mut().push(event),
        )
    };
    let take = || events.borrow_mut().drain(..).collect::<Vec<_>>();
    let blocks = mainnet_blocks();
    let chain = chain_of(&blocks);
    let chain_put = |len: usize| PersistJob::ChainPut {
        chain: chain[..len].to_vec(),
        blocks: blocks[..len].to_vec(),
    };
    let failing = |fail: bool| {
        mem.set_faults(Faults {
            fail_at_call: fail.then_some(0),
            ..Faults::default()
        })
    };
    let error_of = |events: Vec<PersistEvent>| -> PersistWriteError {
        match events.as_slice() {
            [PersistEvent::ChainPut(Err(error)) | PersistEvent::Checkpoint(Err(error))] => {
                error.clone()
            }
            other => panic!("{other:?}"),
        }
    };
    writer.handle(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis(),
    });
    assert!(matches!(take().as_slice(), [PersistEvent::Opened(_)]));

    // One error short of the limit, then a success: the count starts again.
    failing(true);
    for _ in 1..MAX_IO_ERRORS_IN_A_ROW {
        writer.handle(chain_put(1));
        let error = error_of(take());
        assert!(error.error.contains("injected"), "{}", error.error);
        assert!(!error.disable);
    }
    failing(false);
    writer.handle(chain_put(1));
    assert!(matches!(take().as_slice(), [PersistEvent::ChainPut(Ok(_))]));
    failing(true);
    for _ in 1..MAX_IO_ERRORS_IN_A_ROW {
        writer.handle(chain_put(2));
        assert!(!error_of(take()).disable);
    }
    writer.handle(chain_put(2));
    let error = error_of(take());
    assert!(error.disable);
    assert!(
        error
            .error
            .contains(&format!("{MAX_IO_ERRORS_IN_A_ROW} I/O errors in a row")),
        "{}",
        error.error
    );

    // A stream whose ledger thread is gone: off at the first error.
    failing(false);
    writer.handle(chain_put(2));
    assert!(matches!(take().as_slice(), [PersistEvent::ChainPut(Ok(_))]));
    let fixture = Fixture::new(20);
    let gone = Arc::new(MaskSource::new(&fixture.staking, 7, 13).gone_at(1));
    let released = Arc::new(AtomicBool::new(false));
    writer.handle(checkpoint_job(
        &fixture,
        &chain,
        vec![
            (hash_of(&fixture.staking), Some(LedgerWrite::Stream(gone))),
            (
                hash_of(&fixture.snarked),
                Some(choose_ledger_write(
                    &fixture.snarked,
                    std::slice::from_ref(&fixture.staking),
                )),
            ),
        ],
        &released,
    ));
    let error = error_of(take());
    assert!(error.disable);
    assert!(error.error.contains("gone"), "{}", error.error);
    assert!(released.load(Ordering::SeqCst));
}

/// The parts as the store thread encodes them. Encoded once: the encoding
/// of a pending coinbase is not byte for byte the same from one conversion
/// to the next (its stack indexes come from a hash map), and the crash test
/// compares whole manifests, checksums included.
fn parts_bytes(staged: &Mask) -> Vec<u8> {
    let parts = parts_snapshot(staged).into_wire();
    let mut bytes = Vec::new();
    parts.binprot_write(&mut bytes).unwrap();
    bytes
}

/// A checkpoint as the store thread writes it: every ledger streamed (a
/// ledger stored whole already is not read again), the parts a real encoding.
fn streamed_write(fixture: &Fixture, parts: &[u8], root: u64, tip: u64) -> CheckpointWrite {
    let mut write = fixture.write(root, tip);
    for (hash, ledger_write) in &mut write.ledgers {
        let mask = if *hash == hash_of(&fixture.staking) {
            &fixture.staking
        } else {
            &fixture.snarked
        };
        *ledger_write = Some(LedgerWrite::Stream(Arc::new(MaskSource::new(mask, 2, 3))));
    }
    write.parts = blob(parts.to_vec());
    write
}

#[test]
fn a_crash_anywhere_in_a_streamed_checkpoint_leaves_a_loadable_store() {
    let fixture = Fixture::new(6);
    let next = fixture.next();
    let before = two_generations(&fixture);
    let (parts, next_parts) = (parts_bytes(&fixture.staged), parts_bytes(&next.staged));
    crash_everywhere("streamed checkpoint", &before, &|store| {
        store
            .checkpoint(streamed_write(&next, &next_parts, 11, 14))
            .map(|_| ())
    });
    crash_everywhere("first streamed checkpoint", &Files::new(), &|store| {
        store
            .checkpoint(streamed_write(&fixture, &parts, 10, 12))
            .map(|_| ())
    });
}

/// The I5c review's race, on mainnet's scan state: the store thread converts
/// a checkpoint's parts (a shallow clone of the root's scan state, sharing its
/// witnesses) while the ledger thread converts the same scan state for a peer
/// or a commit's available jobs. With the witnesses' `try_lock` one of the two
/// threads panicked in 20 rounds of 20; now neither may, and both get what one
/// thread alone gets. Needs the parts file of a mainnet node's checkpoint:
///
/// ```text
/// MINA_PERSIST_PARTS=<store>/<chain id>/frontier/parts-<root>.bin \
///     cargo test --release -p mina-node --lib mainnet_parts -- --ignored
/// ```
#[test]
#[ignore = "needs a mainnet parts file in MINA_PERSIST_PARTS"]
fn mainnet_parts_convert_on_two_threads_at_once() {
    use ledger::scan_state::scan_state::AvailableJobMessage;

    const ROUNDS: usize = 20;

    let path = std::env::var("MINA_PERSIST_PARTS").expect("MINA_PERSIST_PARTS");
    let bytes = std::fs::read(&path).unwrap();
    let parts = StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut bytes.as_slice()).unwrap();
    let scan_state = ScanState::try_from(&parts.scan_state).unwrap();
    let mut pending_coinbase = PendingCoinbase::try_from(&parts.pending_coinbase).unwrap();
    pending_coinbase.merkle_root();
    let base_jobs = scan_state.scan_state.pending_data().concat().len();
    assert!(base_jobs > 0);
    let snapshot = |scan_state: ScanState| PartsSnapshot {
        scan_state,
        pending_coinbase: pending_coinbase.clone(),
        staged_ledger_hash: parts.staged_ledger_hash.clone(),
        needed_blocks: List::new(),
    };
    let available_jobs = |scan_state: &ScanState| {
        let jobs = scan_state
            .all_job_pairs_iter()
            .map(|job| job.map(|single| AvailableJobMessage::from(single)))
            .collect::<Vec<_>>();
        serde_json::to_vec(&jobs).unwrap()
    };
    let want = snapshot(scan_state.clone()).into_wire().scan_state;
    let want_jobs = available_jobs(&scan_state);
    eprintln!("{base_jobs} base jobs, {ROUNDS} rounds each way");

    for round in 0..2 * ROUNDS {
        let store_parts = snapshot(scan_state.clone());
        let start = &std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let store_thread = scope.spawn(move || {
                start.wait();
                store_parts.into_wire().scan_state
            });
            start.wait();
            if round < ROUNDS {
                // A peer's staged ledger request.
                let converted = v2::TransactionSnarkScanStateStableV2::from(&scan_state);
                assert!(converted == want, "round {round}");
            } else {
                // A commit's available jobs.
                assert!(available_jobs(&scan_state) == want_jobs, "round {round}");
            }
            assert!(store_thread.join().unwrap() == want, "round {round}");
        });
    }
}
