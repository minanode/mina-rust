//! Tests of the ledger thread's part of persistence (R20 D3, D8):
//!
//! - why the store thread may not read a ledger itself: a `Mask` used from two
//!   threads at once panics;
//! - the pins: a pinned ledger and its ancestors survive a commit's drop and
//!   compaction, and fold as before once unpinned;
//! - checkpoints taken through a real ledger thread and written by a real
//!   store thread, while other requests read the same ledgers: what each
//!   checkpoint writes for each ledger, and that the store loads back.

use std::{
    collections::BTreeMap,
    panic::AssertUnwindSafe,
    sync::{mpsc, Barrier, Mutex},
    time::Duration,
};

use ledger::{
    staged_ledger::staged_ledger::StagedLedger, Account, AccountId, AccountIndex, BaseLedger, Mask,
};
use mina_core::{block::ArcBlockWithHash, constants::constraint_constants, MAINNET_CHAIN_ID};
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2::{self, LedgerHash, MinaBaseStagedLedgerHashStableV1},
};

use super::*;
use crate::{
    ledger::{
        ledger_manager::LedgerManager,
        write::{LedgerWriteRequest, LedgerWriteResponse},
        LedgerAddress,
    },
    p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases,
    persist::{
        event::{PersistCheckpointDone, PersistEvent, PersistLedgerWriteKind},
        format::{block_name, parts_name},
        ledger_codec::{count_nonempty_nodes, write_full, MAX_DEPTH},
        test_fixtures::{append, change, child, hash_of, ledger_with, mainnet_blocks},
        writer::WriterOptions,
        FrontierStore, MemBackend, VerifyMode,
    },
};

/// The finding behind the streaming (D3 as built): `Mask::with` takes the
/// mask's lock with `try_lock().expect(..)`, so a second thread reading a
/// mask another thread is inside panics, where a `lock()` would have waited.
#[test]
fn a_mask_used_by_two_threads_at_once_panics() {
    let mask = ledger_with(3);
    let inside = Barrier::new(2);
    let done = Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut first = true;
            mask.iter(|_| {
                if std::mem::take(&mut first) {
                    inside.wait();
                    done.wait();
                }
            });
        });
        inside.wait();
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| mask.num_accounts()));
        done.wait();
        let panic = result.expect_err("a read of a mask in use must panic");
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default();
        assert!(message.contains("lock failed"), "{message}");
    });
    // A child mask reads through its parent's lock, so the same holds for a
    // read of any descendant of a mask in use.
    let child = mask.make_child();
    let inside = Barrier::new(2);
    let done = Barrier::new(2);
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let mut first = true;
            mask.iter(|_| {
                if std::mem::take(&mut first) {
                    inside.wait();
                    done.wait();
                }
            });
        });
        inside.wait();
        let result =
            std::panic::catch_unwind(AssertUnwindSafe(|| child.get_at_index(AccountIndex(0))));
        done.wait();
        assert!(result.is_err(), "a read through a parent in use must panic");
    });
}

/// The body of a pinned ledger's full file, read a piece at a time as the
/// store thread asks for it.
fn read_all_parts(ctx: &LedgerCtx, pin: u64, header: &LedgerFileHeader) -> Vec<u8> {
    let (accounts, hashes) = ctx.persist.part_sizes;
    let mut body = Vec::new();
    let mut start = 0;
    while start < header.num_accounts {
        let end = (start + accounts).min(header.num_accounts);
        body.extend(
            ctx.persist_read(pin, FullPart::Accounts { start, end })
                .unwrap()
                .0,
        );
        start = end;
    }
    let mut start = 0;
    while start < header.num_hashes {
        let end = (start + hashes).min(header.num_hashes);
        body.extend(
            ctx.persist_read(pin, FullPart::Hashes { start, end })
                .unwrap()
                .0,
        );
        start = end;
    }
    body
}

/// A snapshot that fails after it pinned a ledger releases that pin: the
/// guard that unpins is made before the first pin. Here the second ledger to
/// write whole can't be pinned (its depth is beyond what a file holds).
#[test]
fn a_snapshot_that_fails_partway_releases_the_pins_it_took() {
    let small = ledger_with(10);
    let mut deep = Mask::new_unattached(MAX_DEPTH as usize + 1);
    for _ in 0..20 {
        let account = Account::rand();
        deep.get_or_create_account(account.id(), account).unwrap();
    }
    let mut staged =
        StagedLedger::create_exn(constraint_constants().clone(), small.copy().make_child())
            .expect("a staged ledger");
    let staged_hash = MinaBaseStagedLedgerHashStableV1::from(&staged.hash());
    let (caller, mut next_request) = LedgerCaller::queued();
    let (writer, jobs) = PersistWriter::channel();
    let mut ctx = LedgerCtx::default();
    ctx.set_persist_writer(writer);
    for mask in [&small, &deep] {
        ctx.snarked_ledgers.insert(hash_of(mask), mask.clone());
    }
    ctx.staged_ledgers.insert_by_recomputing_hash(staged);
    let request = PersistCheckpointRequest {
        checkpoint: Checkpoint {
            root: v2::StateHash::from_fp(1u64.into()),
            root_height: 1,
            root_snarked_ledger: hash_of(&small),
            root_staged_ledger_hash: staged_hash,
        },
        chain: Vec::new(),
        ledgers: vec![hash_of(&small), hash_of(&deep)],
        stored: Vec::new(),
        needed_protocol_states: BTreeMap::new(),
    };

    let error = ctx.persist_checkpoint(request, &caller).unwrap_err();
    assert!(error.contains("depth"), "{error}");
    assert!(jobs.try_recv().is_err(), "no job is handed over");
    // The small ledger was pinned before the deep one failed, and the guard
    // asked the ledger thread (itself) to release it.
    assert_eq!(ctx.persist_pin_count(), 1);
    let Some(LedgerRequest::PersistUnpin { pins }) = next_request() else {
        panic!("no unpin request");
    };
    assert_eq!(pins.len(), 1);
    ctx.persist_unpin(&pins);
    assert_eq!(ctx.persist_pin_count(), 0);
    assert!(next_request().is_none());
}

/// On the store thread, a ledger thread that has stopped (before taking a
/// read, or without answering it) is an error that turns persistence off,
/// not the process exiting as the node's own ledger calls do.
#[test]
fn a_stopped_ledger_thread_is_an_error_on_the_store_thread() {
    let header = full_header(&ledger_with(3)).unwrap();
    for before_request in [true, false] {
        let source = PinnedLedger {
            caller: LedgerCaller::stopped(before_request),
            pin: 1,
            header: header.clone(),
            part_sizes: (2, 3),
            stats: Default::default(),
            after_read_part: None,
        };
        let error = source
            .read_part(FullPart::Accounts { start: 0, end: 2 })
            .unwrap_err();
        assert!(matches!(error, CodecError::SourceGone(_)), "{error}");
    }
}

/// D8's pin test: a mask over a parent, pinned, then a commit's drop and
/// compaction (`commit` runs `persist_keep_pinned`, `drop_snarked_ledgers` and
/// `compact_snarked_ledgers` in this order) keeping only the mask.
#[test]
fn a_pinned_ledger_and_its_ancestors_stay_until_unpinned() {
    let root = ledger_with(12);
    let parent = child(&root, &[1, 2], 3);
    let mask = child(&parent, &[4], 2);
    let (parent_hash, mask_hash) = (hash_of(&parent), hash_of(&mask));
    let mut expected = Vec::new();
    write_full(&mut expected, &mask).unwrap();
    let accounts = (0..17)
        .map(|i| mask.get_at_index(AccountIndex(i)).unwrap())
        .collect::<Vec<_>>();

    let commit = |ctx: &mut LedgerCtx| {
        let mut keep = LedgersToKeep::new();
        keep.add_snarked(mask_hash.clone());
        ctx.persist_keep_pinned(&mut keep);
        ctx.drop_snarked_ledgers(&keep);
        ctx.compact_snarked_ledgers(&keep);
    };
    let ctx_with = |parent: &Mask, mask: &Mask| {
        let mut ctx = LedgerCtx::default();
        ctx.set_persist_part_sizes(3, 5);
        ctx.snarked_ledgers
            .insert(parent_hash.clone(), parent.clone());
        ctx.snarked_ledgers.insert(mask_hash.clone(), mask.clone());
        ctx
    };

    // Without a pin, that commit folds the mask into its ancestors: a reader
    // of the old mask would read an emptied, unregistered mask. (Another
    // chain, the same shape.)
    {
        let other_root = ledger_with(4);
        let other_parent = child(&other_root, &[1], 1);
        let other = child(&other_parent, &[2], 1);
        let other_hash = hash_of(&other);
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers
            .insert(hash_of(&other_parent), other_parent.clone());
        ctx.snarked_ledgers
            .insert(other_hash.clone(), other.clone());
        let mut keep = LedgersToKeep::new();
        keep.add_snarked(other_hash.clone());
        ctx.persist_keep_pinned(&mut keep);
        ctx.drop_snarked_ledgers(&keep);
        ctx.compact_snarked_ledgers(&keep);
        assert_eq!(
            ctx.snarked_ledgers[&other_hash].get_uuid(),
            other_root.get_uuid(),
            "without a pin the mask folds"
        );
    }

    let mut ctx = ctx_with(&parent, &mask);
    let (pin, header) = ctx.persist_pin(&mask).unwrap();
    assert_eq!(header.ledger_hash, mask_hash);
    assert_eq!(header.num_accounts, 17);
    assert_eq!(ctx.persist_pin_count(), 1);

    commit(&mut ctx);
    // Nothing folded: the parent is kept for the pin, the mask is still the
    // mask over it.
    assert_eq!(ctx.snarked_ledgers[&mask_hash].get_uuid(), mask.get_uuid());
    assert_eq!(
        ctx.snarked_ledgers[&parent_hash].get_uuid(),
        parent.get_uuid()
    );
    assert_eq!(mask.get_parent().unwrap().get_uuid(), parent.get_uuid());
    // The stream gives the mask's file, byte for byte.
    let prefix = expected.len() - read_all_parts(&ctx, pin, &header).len();
    let mut streamed = expected[..prefix].to_vec();
    streamed.extend(read_all_parts(&ctx, pin, &header));
    assert_eq!(streamed, expected);
    // A second commit with the pin still there changes nothing either.
    commit(&mut ctx);
    assert_eq!(ctx.snarked_ledgers[&mask_hash].get_uuid(), mask.get_uuid());

    // Unpinned, the next commit folds the mask into its ancestors as before:
    // down to the root database, whose mask is then under the mask's hash,
    // with the mask's accounts.
    ctx.persist_unpin(&[pin]);
    assert_eq!(ctx.persist_pin_count(), 0);
    assert!(ctx
        .persist_read(pin, FullPart::Accounts { start: 0, end: 1 })
        .unwrap_err()
        .contains("released"));
    commit(&mut ctx);
    assert!(!ctx.snarked_ledgers.contains_key(&parent_hash));
    let folded = ctx.snarked_ledgers[&mask_hash].clone();
    assert_eq!(folded.get_uuid(), root.get_uuid());
    assert_eq!(hash_of(&folded), mask_hash);
    assert_eq!(folded.num_accounts(), 17);
    for (i, account) in accounts.iter().enumerate() {
        assert_eq!(
            folded.get_at_index(AccountIndex(i as u64)).as_ref(),
            Some(account)
        );
    }
}

/// A ledger as the test expects to load it back: its hash and accounts,
/// taken before the ledger thread owns it.
struct Expected {
    hash: LedgerHash,
    accounts: Vec<Box<Account>>,
}

fn expected(mask: &Mask) -> Expected {
    let mut mask = mask.clone();
    let accounts = (0..mask.num_accounts() as u64)
        .map(|i| mask.get_at_index(AccountIndex(i)).unwrap())
        .collect();
    Expected {
        hash: v2::LedgerHash::from_fp(mask.merkle_root()),
        accounts,
    }
}

fn assert_loads_as(loaded: &Mask, expected: &Expected) {
    let mut loaded = loaded.clone();
    assert_eq!(v2::LedgerHash::from_fp(loaded.merkle_root()), expected.hash);
    assert_eq!(loaded.num_accounts(), expected.accounts.len());
    for (i, account) in expected.accounts.iter().enumerate() {
        assert_eq!(
            loaded.get_at_index(AccountIndex(i as u64)).as_ref(),
            Some(account)
        );
    }
}

fn entry(block: &ArcBlockWithHash) -> ChainEntry {
    ChainEntry {
        hash: block.hash().clone(),
        height: block.height(),
        global_slot: block.global_slot(),
        just_emitted_a_proof: false,
    }
}

fn wait_event(events: &mpsc::Receiver<PersistEvent>) -> PersistEvent {
    events
        .recv_timeout(Duration::from_secs(120))
        .expect("an event from the store thread")
}

// Armed once per checkpoint. The first streamed part consumes the pause;
// dropping the release sender also releases the store thread on test failure.
type StreamGate = Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>;

/// Holds a checkpoint after its first streamed part, proves the ledger thread
/// serves a complete batch of reads before release, then waits for the result.
/// A checkpoint reusing all stored ledgers has no stream to pause.
fn checkpoint(
    manager: &LedgerManager,
    events: &mpsc::Receiver<PersistEvent>,
    stream_gate: &StreamGate,
    request: PersistCheckpointRequest,
    reads: &[(LedgerHash, Vec<AccountId>, Vec<Box<Account>>)],
) -> (PersistSnapshotInfo, PersistCheckpointDone, u32) {
    let (entered_tx, entered) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    *stream_gate.lock().unwrap() = Some((entered_tx, resume));
    let response = manager.call_sync(LedgerRequest::Write(LedgerWriteRequest::PersistCheckpoint(
        Box::new(request),
    )));
    let LedgerResponse::Write(LedgerWriteResponse::PersistCheckpoint { result, .. }) = response
    else {
        panic!("unexpected answer: {response:?}");
    };
    let snapshot = result.expect("the snapshot");
    let count = if snapshot
        .ledgers
        .iter()
        .any(|info| info.kind == PersistLedgerWriteKind::Full)
    {
        entered
            .recv_timeout(Duration::from_secs(120))
            .expect("checkpoint reached its first streamed part");
        std::thread::scope(|scope| {
            let (served_tx, served) = mpsc::channel();
            let reader = scope.spawn(move || {
                let mut count = 0;
                for (hash, ids, accounts) in reads {
                    match manager.call_sync(LedgerRequest::AccountsGet {
                        ledger_hash: hash.clone(),
                        account_ids: ids.clone(),
                    }) {
                        LedgerResponse::AccountsGet(Ok(got)) => {
                            assert_eq!(got.len(), accounts.len());
                            for (got, want) in got.iter().zip(accounts) {
                                assert_eq!(got, want.as_ref());
                            }
                        }
                        other => panic!("unexpected answer: {other:?}"),
                    }
                    match manager.call_sync(LedgerRequest::ChildHashesGet {
                        snarked_ledger_hash: hash.clone(),
                        parent: LedgerAddress::root(),
                        depth: 1,
                    }) {
                        LedgerResponse::ChildHashes(Some(hashes)) => assert_eq!(hashes.len(), 2),
                        other => panic!("unexpected answer: {other:?}"),
                    }
                    count += 2;
                }
                // A whole-ledger read, as the block producer's delegator table does.
                match manager.call_sync(LedgerRequest::GetProducersWithDelegates {
                    ledger_hash: reads[0].0.clone(),
                    filter: |_| true,
                }) {
                    LedgerResponse::ProducersWithDelegatesMap(Some(_)) => {}
                    other => panic!("unexpected answer: {other:?}"),
                }
                count += 1;
                served_tx.send(count).unwrap();
            });
            // Receive before release: a ledger thread that waits for the
            // checkpoint cannot satisfy this. Release even on timeout so the
            // scoped reader can finish and the test reports a failure.
            let result = served.recv_timeout(Duration::from_secs(120));
            let pending = events.try_recv();
            drop(release);
            let count = result.expect("ledger reads served while checkpoint is held");
            assert!(
                matches!(pending, Err(mpsc::TryRecvError::Empty)),
                "checkpoint completed before release: {pending:?}"
            );
            reader.join().unwrap();
            count
        })
    } else {
        stream_gate.lock().unwrap().take();
        0
    };
    let PersistEvent::Checkpoint(result) = wait_event(events) else {
        panic!("expected checkpoint result");
    };
    let done = result.unwrap_or_else(|error| panic!("checkpoint: {}", error.error));
    (snapshot, done, count)
}

fn kinds(snapshot: &PersistSnapshotInfo) -> BTreeMap<LedgerHash, PersistLedgerWriteKind> {
    snapshot
        .ledgers
        .iter()
        .map(|info| (info.hash.clone(), info.kind.clone()))
        .collect()
}

fn full_parts(num_accounts: u64, (accounts, hashes): (u64, u64)) -> u32 {
    (num_accounts.div_ceil(accounts) + count_nonempty_nodes(num_accounts, 35).div_ceil(hashes))
        as u32
}

/// Three checkpoints through a real ledger thread and a real store thread
/// over the in-memory backend, while the test reads the same ledgers through
/// the ledger thread (the reads that would panic if the store thread read the
/// masks itself):
///
/// 1. the first: the smallest ledger (the staking ledger) streamed whole, the
///    others as diffs over it;
/// 2. after the staking ledger is dropped: the ledgers stored as diffs over it
///    are written whole again, the new root snarked ledger as a diff over the
///    smallest diff it can take;
/// 3. nothing changed: nothing is written.
#[test]
fn checkpoints_through_the_ledger_thread_while_it_serves_reads() {
    const PARTS: (u64, u64) = (7, 13);
    let staking = ledger_with(40);
    let next = child(&staking, &[1, 2], 5);
    let root = child(&next, &[3], 4);
    let root2 = child(&root, &[7], 1);
    let mut staged_mask = root.copy().make_child();
    change(&mut staged_mask, &[0, 5]);
    append(&mut staged_mask, 2);
    let mut staged = StagedLedger::create_exn(constraint_constants().clone(), staged_mask.clone())
        .expect("a staged ledger");
    let staged_hash = MinaBaseStagedLedgerHashStableV1::from(&staged.hash());
    let ledgers = [&staking, &next, &root, &root2]
        .map(expected)
        .map(|e| (e.hash.clone(), e));
    let ledgers = BTreeMap::from(ledgers);
    let [s, n, r, r2] = [&staking, &next, &root, &root2].map(hash_of);
    let staged_expected = expected(&staged_mask);
    let reads = [&s, &n, &r]
        .into_iter()
        .map(|hash| {
            let accounts = ledgers[hash].accounts[..5].to_vec();
            let ids = accounts.iter().map(|account| account.id()).collect();
            (hash.clone(), ids, accounts)
        })
        .collect::<Vec<_>>();

    let mem = MemBackend::new();
    let (events_tx, events) = mpsc::channel();
    let writer = {
        let mem = mem.second_handle();
        PersistWriter::spawn(
            move |_| Ok((mem.second_handle(), "memory".to_owned())),
            WriterOptions {
                reset: false,
                node_commit: "i5c-tests".to_owned(),
                ..Default::default()
            },
            move |event| {
                let _ = events_tx.send(event);
            },
        )
    };
    let genesis = v2::StateHash::from_fp(7u64.into());
    assert!(writer.send(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis.clone(),
    }));
    let PersistEvent::Opened(opened) = wait_event(&events) else {
        panic!("not opened");
    };
    assert_eq!(opened.outcome, "empty");
    let blocks = mainnet_blocks();
    let chain = blocks.iter().map(entry).collect::<Vec<_>>();
    assert!(writer.send(PersistJob::ChainPut {
        chain: chain.clone(),
        blocks: blocks.clone(),
    }));
    let PersistEvent::ChainPut(Ok(put)) = wait_event(&events) else {
        panic!("chain put failed");
    };
    assert_eq!(put.written.files, 2);

    let stream_gate = Arc::new(StreamGate::default());
    let mut ctx = LedgerCtx::default();
    ctx.persist.after_read_part = Some(Arc::new({
        let stream_gate = stream_gate.clone();
        move || {
            let pause = stream_gate.lock().unwrap().take();
            if let Some((entered, resume)) = pause {
                entered.send(()).unwrap();
                let _ = resume.recv();
            }
        }
    }));
    ctx.set_persist_part_sizes(PARTS.0, PARTS.1);
    ctx.set_persist_writer(writer);
    for mask in [&staking, &next, &root, &root2] {
        ctx.snarked_ledgers.insert(hash_of(mask), mask.clone());
    }
    ctx.staged_ledgers.insert_by_recomputing_hash(staged);
    // From here the masks are the ledger thread's: the test only reads
    // through it.
    let manager = LedgerManager::spawn(ctx);

    let request = |root_block: usize,
                   root_snarked: &LedgerHash,
                   wanted: Vec<LedgerHash>,
                   stored: &PersistCheckpointDone| {
        PersistCheckpointRequest {
            checkpoint: Checkpoint {
                root: chain[root_block].hash.clone(),
                root_height: chain[root_block].height,
                root_snarked_ledger: root_snarked.clone(),
                root_staged_ledger_hash: staged_hash.clone(),
            },
            chain: chain[root_block..].to_vec(),
            ledgers: wanted,
            stored: stored
                .store
                .ledgers
                .iter()
                .map(|ledger| (ledger.hash.clone(), ledger.base.clone()))
                .collect(),
            needed_protocol_states: BTreeMap::new(),
        }
    };
    let nothing_stored = PersistCheckpointDone {
        root: genesis.clone(),
        root_height: 0,
        generation: 0,
        written: Default::default(),
        store: Default::default(),
        parts_bytes: 0,
        parts_encode_ms: 0.0,
        stream_parts: 0,
        stream_ms: 0.0,
        stream_max_ms: 0.0,
        elapsed_ms: 0.0,
        snapshot: Default::default(),
    };

    // 1. The first checkpoint: the root snarked, staking and next epoch
    // ledgers.
    let (snapshot, done, reads_made) = checkpoint(
        &manager,
        &events,
        &stream_gate,
        request(
            0,
            &r,
            vec![r.clone(), s.clone(), n.clone()],
            &nothing_stored,
        ),
        &reads,
    );
    eprintln!("first checkpoint: {reads_made} reads served meanwhile, {done:?}");
    assert_eq!(
        kinds(&snapshot),
        BTreeMap::from([
            (s.clone(), PersistLedgerWriteKind::Full),
            (
                n.clone(),
                PersistLedgerWriteKind::Diff {
                    base: s.clone(),
                    accounts: 7
                }
            ),
            (
                r.clone(),
                PersistLedgerWriteKind::Diff {
                    base: s.clone(),
                    accounts: 12
                }
            ),
        ])
    );
    assert_eq!(done.stream_parts, full_parts(40, PARTS));
    assert_eq!(reads_made, 2 * reads.len() as u32 + 1);
    assert_eq!(done.generation, 2);
    // Files: the staking ledger whole, two diffs, the staged diff and the
    // parts; the blocks were stored by the chain put.
    assert_eq!(done.written.files, 5);
    // The pins went before the result: the pin is gone.
    match manager.call_sync(LedgerRequest::PersistRead {
        pin: 1,
        part: FullPart::Accounts { start: 0, end: 1 },
    }) {
        LedgerResponse::PersistRead(Err(error)) => assert!(error.contains("released"), "{error}"),
        other => panic!("unexpected answer: {other:?}"),
    }

    // The store loads back as the ledgers were.
    {
        // Read only: the store thread holds the store's lock.
        let (mut store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
        let manifest = store.manifest().cloned().unwrap();
        assert_eq!(manifest.chain_id, MAINNET_CHAIN_ID);
        assert_eq!(manifest.genesis_state_hash, genesis);
        let loaded = store.read_ledgers(&manifest, VerifyMode::Full).unwrap();
        for hash in [&s, &n, &r] {
            assert_loads_as(&loaded[hash], &ledgers[hash]);
        }
        let staged = store
            .read_staged(&manifest, &loaded[&r], VerifyMode::Full)
            .unwrap();
        assert_loads_as(&staged, &staged_expected);
        let parts = store
            .read_blob(manifest.file(&parts_name(&chain[0].hash)).unwrap())
            .unwrap();
        let parts =
            StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut parts.as_slice()).unwrap();
        assert_eq!(parts.staged_ledger_hash, staged_hash.non_snark.ledger_hash);
        assert!(parts.needed_blocks.is_empty());
        for block in &blocks {
            let bytes = store
                .read_blob(manifest.file(&block_name(block.hash())).unwrap())
                .unwrap();
            let mut want = Vec::new();
            block.block.binprot_write(&mut want).unwrap();
            assert_eq!(bytes, want);
        }
    }

    // 2. The staking ledger is dropped, the next epoch ledger becomes the
    // staking ledger and the root moves on. The two ledgers stored as diffs
    // over the dropped ledger are written whole; the new root snarked ledger
    // is a diff over the closest.
    let (snapshot, done2, reads_made) = checkpoint(
        &manager,
        &events,
        &stream_gate,
        request(1, &r2, vec![r2.clone(), n.clone(), r.clone()], &done),
        &reads[1..],
    );
    assert_eq!(
        kinds(&snapshot),
        BTreeMap::from([
            (n.clone(), PersistLedgerWriteKind::Full),
            (r.clone(), PersistLedgerWriteKind::Full),
            (
                r2.clone(),
                PersistLedgerWriteKind::Diff {
                    base: r.clone(),
                    accounts: 2
                }
            ),
        ])
    );
    assert_eq!(reads_made, 2 * reads[1..].len() as u32 + 1);
    assert_eq!(
        done2.stream_parts,
        full_parts(45, PARTS) + full_parts(49, PARTS)
    );
    assert_eq!(done2.generation, 3);
    let stored = done2
        .store
        .ledgers
        .iter()
        .map(|ledger| (ledger.hash.clone(), ledger.base.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        stored,
        BTreeMap::from([
            (n.clone(), None),
            (r.clone(), None),
            (r2.clone(), Some(r.clone()))
        ])
    );

    // 3. The same again: every ledger is stored, nothing is pinned or
    // written, and the dropped ledger's files are gone once neither manifest
    // slot lists them.
    let (snapshot, done3, _) = checkpoint(
        &manager,
        &events,
        &stream_gate,
        request(1, &r2, vec![r2.clone(), n.clone(), r.clone()], &done2),
        &reads[1..],
    );
    assert!(snapshot
        .ledgers
        .iter()
        .all(|info| info.kind == PersistLedgerWriteKind::Stored));
    assert_eq!(done3.stream_parts, 0);
    assert_eq!(done3.written.files, 0);
    assert_eq!(done3.generation, 4);
    let names = mem.files().into_keys().collect::<Vec<_>>();
    assert!(
        !names.iter().any(|name| name.contains(&s.to_string())),
        "{names:?}"
    );
    let (mut store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
    let manifest = store.manifest().cloned().unwrap();
    let loaded = store.read_ledgers(&manifest, VerifyMode::Full).unwrap();
    for hash in [&n, &r, &r2] {
        assert_loads_as(&loaded[hash], &ledgers[hash]);
    }
}

/// A ledger's accounts read through the ledger thread.
fn accounts_through(
    manager: &LedgerManager,
    hash: &LedgerHash,
    expected: &Expected,
) -> Result<usize, String> {
    let ids = expected
        .accounts
        .iter()
        .map(|account| account.id())
        .collect::<Vec<_>>();
    match manager.call_sync(LedgerRequest::AccountsGet {
        ledger_hash: hash.clone(),
        account_ids: ids,
    }) {
        LedgerResponse::AccountsGet(Ok(got)) => {
            if !got.is_empty() {
                assert_eq!(got.len(), expected.accounts.len(), "{hash}");
                for (got, want) in got.iter().zip(&expected.accounts) {
                    assert_eq!(got, want.as_ref(), "{hash}");
                }
            }
            Ok(got.len())
        }
        LedgerResponse::AccountsGet(Err(error)) => Err(error),
        other => panic!("unexpected answer: {other:?}"),
    }
}

/// A store thread over `mem` (as a new process opens it), opened.
fn opened_writer(
    mem: &MemBackend,
    genesis: &v2::StateHash,
) -> (PersistWriter, mpsc::Receiver<PersistEvent>) {
    let (events_tx, events) = mpsc::channel();
    let writer = {
        // A new process: the in-memory lock is released.
        let mem = mem.reopen();
        PersistWriter::spawn(
            move |_| Ok((mem.second_handle(), "memory".to_owned())),
            WriterOptions {
                reset: false,
                node_commit: "i5d1-tests".to_owned(),
                ..Default::default()
            },
            move |event| {
                let _ = events_tx.send(event);
            },
        )
    };
    assert!(writer.send(PersistJob::Open {
        chain_id: MAINNET_CHAIN_ID,
        genesis_state_hash: genesis.clone(),
    }));
    assert!(matches!(wait_event(&events), PersistEvent::Opened(_)));
    (writer, events)
}

fn load_through(
    writer: &PersistWriter,
    events: &mpsc::Receiver<PersistEvent>,
    manager: &LedgerManager,
    current_slot: u32,
    cancelled: bool,
) -> crate::persist::event::PersistLoadResult {
    use crate::persist::{load::PersistLoadRequest, writer::LoadJob};
    let control = std::sync::Arc::new(crate::persist::load_control::LoadControl::default());
    if cancelled {
        assert!(control.cancel());
    }
    assert!(writer.send(PersistJob::Load(Box::new(LoadJob {
        request: PersistLoadRequest {
            current_slot: Some(current_slot),
            stale_slots: 4 * 290,
            genesis: None,
        },
        verifier: None,
        insert: manager.persist_inserter(),
        control,
    }))));
    loop {
        match wait_event(events) {
            PersistEvent::Loaded(result) => return *result,
            // The store's summary, before a load that didn't restore.
            PersistEvent::Summary(_) => {}
            other => panic!("unexpected event: {other:?}"),
        }
    }
}

/// I5c's writes, read back by I5d1's load: a checkpoint taken through a real
/// ledger thread and written by a real store thread; then, as after a
/// restart, a new store thread loads the store and hands the ledgers to a new
/// ledger thread, which serves them as they were. Then a load the node gave
/// up on hands nothing over, and a store too old to replay gives its ledgers
/// as copy origins: the sync of a ledger the node holds starts from it.
#[test]
fn a_restore_loads_what_the_ledger_thread_checkpointed() {
    use crate::persist::{
        event::PersistLoadResult,
        test_fixtures::{synthetic_chain, ChainLedgers},
    };

    let staking = ledger_with(40);
    let next = child(&staking, &[1, 2], 5);
    let root = child(&next, &[3], 4);
    let mut staged_mask = root.copy().make_child();
    change(&mut staged_mask, &[0, 5]);
    append(&mut staged_mask, 2);
    let mut staged = StagedLedger::create_exn(constraint_constants().clone(), staged_mask.clone())
        .expect("a staged ledger");
    let staged_hash = MinaBaseStagedLedgerHashStableV1::from(&staged.hash());
    let [s, n, r] = [&staking, &next, &root].map(hash_of);
    let expected_ledgers =
        BTreeMap::from([&staking, &next, &root].map(|mask| (hash_of(mask), expected(mask))));
    let staged_expected = expected(&staged_mask);
    let genesis = v2::StateHash::from_fp(7u64.into());
    let blocks = synthetic_chain(
        v2::StateHash::from_fp(8u64.into()),
        4,
        &ChainLedgers {
            staking: s.clone(),
            next: n.clone(),
            snarked: r.clone(),
            staged: staged_hash.clone(),
        },
    );
    let chain = blocks.iter().map(entry).collect::<Vec<_>>();
    let tip_slot = blocks.last().unwrap().global_slot();

    // The first run: a chain put, then a checkpoint at the first block.
    let mem = MemBackend::new();
    let (writer, events) = opened_writer(&mem, &genesis);
    assert!(writer.send(PersistJob::ChainPut {
        chain: chain.clone(),
        blocks: blocks.clone(),
    }));
    assert!(matches!(wait_event(&events), PersistEvent::ChainPut(Ok(_))));
    let stream_gate = Arc::new(StreamGate::default());
    let mut ctx = LedgerCtx::default();
    ctx.persist.after_read_part = Some(Arc::new({
        let stream_gate = stream_gate.clone();
        move || {
            let pause = stream_gate.lock().unwrap().take();
            if let Some((entered, resume)) = pause {
                entered.send(()).unwrap();
                let _ = resume.recv();
            }
        }
    }));
    ctx.set_persist_part_sizes(7, 13);
    ctx.set_persist_writer(writer);
    for mask in [&staking, &next, &root] {
        ctx.snarked_ledgers.insert(hash_of(mask), mask.clone());
    }
    ctx.staged_ledgers.insert_by_recomputing_hash(staged);
    let manager = LedgerManager::spawn(ctx);
    let reads = vec![(
        s.clone(),
        expected_ledgers[&s].accounts[..3]
            .iter()
            .map(|account| account.id())
            .collect(),
        expected_ledgers[&s].accounts[..3].to_vec(),
    )];
    let (_, done, _) = checkpoint(
        &manager,
        &events,
        &stream_gate,
        PersistCheckpointRequest {
            checkpoint: Checkpoint {
                root: chain[0].hash.clone(),
                root_height: chain[0].height,
                root_snarked_ledger: r.clone(),
                root_staged_ledger_hash: staged_hash.clone(),
            },
            chain: chain.clone(),
            ledgers: vec![r.clone(), s.clone(), n.clone()],
            stored: Vec::new(),
            needed_protocol_states: BTreeMap::new(),
        },
        &reads,
    );
    assert_eq!(done.generation, 2);
    drop(manager);

    // The restart: a new ledger thread holding nothing, a new store thread.
    let manager = LedgerManager::spawn(LedgerCtx::default());
    let (writer, events) = opened_writer(&mem, &genesis);
    let restored = match load_through(&writer, &events, &manager, tip_slot + 5, false) {
        PersistLoadResult::Restored(restored) => restored,
        other => panic!("not restored: {other:?}"),
    };
    assert_eq!(restored.generation, 2);
    assert_eq!(restored.attempt, 1);
    assert_eq!(restored.root.hash(), blocks[0].hash());
    assert_eq!(
        restored
            .blocks
            .iter()
            .map(|b| b.hash().clone())
            .collect::<Vec<_>>(),
        blocks[1..]
            .iter()
            .map(|b| b.hash().clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        restored.ledgers.iter().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([s.clone(), n.clone(), r.clone()])
    );
    assert!(restored.timings.ledger_thread_ms >= 0.0);
    for (hash, expected) in &expected_ledgers {
        assert_eq!(
            accounts_through(&manager, hash, expected).unwrap(),
            expected.accounts.len()
        );
    }
    assert_eq!(
        accounts_through(&manager, &staged_expected.hash, &staged_expected).unwrap(),
        staged_expected.accounts.len()
    );
    // The restore is committed: the attempts record goes.
    let attempts = || {
        let (mut store, _) = FrontierStore::inspect(mem.second_handle(), false).unwrap();
        store.attempts().unwrap()
    };
    assert_eq!(attempts().map(|a| a.count), Some(1));
    assert!(writer.send(PersistJob::RestoreDone));
    // Jobs run in order: once a later job is answered, the record is gone.
    assert!(writer.send(PersistJob::ChainPut {
        chain: chain.clone(),
        blocks: Vec::new(),
    }));
    assert!(matches!(wait_event(&events), PersistEvent::ChainPut(Ok(_))));
    assert_eq!(attempts(), None);
    drop(manager);

    // A load the node stopped waiting for: nothing is handed over.
    let manager = LedgerManager::spawn(LedgerCtx::default());
    let (writer, events) = opened_writer(&mem, &genesis);
    assert!(matches!(
        load_through(&writer, &events, &manager, tip_slot, true),
        PersistLoadResult::Cancelled
    ));
    assert_eq!(
        accounts_through(&manager, &r, &expected_ledgers[&r]),
        Err("Ledger unavailable".to_owned())
    );
    drop(manager);

    // A store too old to replay: its ledgers only, as copy origins.
    let manager = LedgerManager::spawn(LedgerCtx::default());
    let (writer, events) = opened_writer(&mem, &genesis);
    match load_through(&writer, &events, &manager, tip_slot + 4 * 290 + 1, false) {
        PersistLoadResult::LedgersOnly {
            reason, ledgers, ..
        } => {
            assert!(reason.contains("behind the clock"), "{reason}");
            assert_eq!(ledgers.len(), 3);
        }
        other => panic!("not ledgers only: {other:?}"),
    }
    assert_eq!(
        accounts_through(&manager, &r, &expected_ledgers[&r]).unwrap(),
        expected_ledgers[&r].accounts.len()
    );
    assert_eq!(
        accounts_through(&manager, &staged_expected.hash, &staged_expected),
        Err("Ledger unavailable".to_owned())
    );
    // The store thread cleared the record once it handed them over.
    assert_eq!(attempts(), None);
    // The sync of the root snarked ledger starts from a copy of it: the
    // target itself, ahead of the candidates given.
    match manager.call_sync(LedgerRequest::CopySnarkedLedgerContentsForSync {
        origin_snarked_ledger_hash: vec![s.clone()],
        target_snarked_ledger_hash: r.clone(),
        overwrite: false,
    }) {
        LedgerResponse::SnarkedLedgerContentsCopied(Ok(true)) => {}
        other => panic!("unexpected answer: {other:?}"),
    }
    match manager.call_sync(LedgerRequest::ChildHashesGet {
        snarked_ledger_hash: r.clone(),
        parent: LedgerAddress::root(),
        depth: 1,
    }) {
        LedgerResponse::ChildHashes(Some(hashes)) => assert_eq!(hashes.len(), 2),
        other => panic!("unexpected answer: {other:?}"),
    }
}

/// The copy a ledger sync starts from (I5d1): the target itself when it is
/// committed, as after a ledgers-only load; otherwise the first candidate.
#[test]
fn a_ledger_sync_starts_from_the_target_when_it_is_held() {
    let staking = ledger_with(20);
    let root = child(&staking, &[1], 2);
    let (s, r) = (hash_of(&staking), hash_of(&root));
    let mut ctx = LedgerCtx::default();
    ctx.snarked_ledgers.insert(s.clone(), staking.clone());
    ctx.snarked_ledgers.insert(r.clone(), root.clone());
    assert_eq!(
        ctx.copy_snarked_ledger_contents_for_sync(std::slice::from_ref(&s), r.clone(), false),
        Ok(true)
    );
    let (mut copy, committed) = ctx.sync_ledger_for_tests(&r).expect("the sync ledger");
    assert!(!committed);
    assert_eq!(v2::LedgerHash::from_fp(copy.merkle_root()), r);

    // Not held: the first candidate.
    let other = child(&root, &[2], 0);
    let o = hash_of(&other);
    assert_eq!(
        ctx.copy_snarked_ledger_contents_for_sync(std::slice::from_ref(&s), o.clone(), false),
        Ok(true)
    );
    let (mut copy, _) = ctx.sync_ledger_for_tests(&o).expect("the sync ledger");
    assert_eq!(v2::LedgerHash::from_fp(copy.merkle_root()), s);
}

/// What a restore hands the ledger thread, taken on the calling thread: a
/// real store thread loads the store in `mem`, and its inserter passes the
/// ledgers back, for a `LedgerCtx` of the test's own. Then the restore is
/// done (`RestoreDone`, as once the node commits it), so that the next load
/// isn't counted as a third unfinished one.
fn restored_insert(
    mem: &MemBackend,
    genesis: &v2::StateHash,
    current_slot: u32,
    chain: &[ChainEntry],
) -> crate::persist::writer::PersistInsert {
    use crate::persist::{event::PersistLoadResult, load::PersistLoadRequest, writer::LoadJob};
    let (writer, events) = opened_writer(mem, genesis);
    let (inserts, inserted) = mpsc::channel();
    assert!(writer.send(PersistJob::Load(Box::new(LoadJob {
        request: PersistLoadRequest {
            current_slot: Some(current_slot),
            stale_slots: 4 * 290,
            genesis: None,
        },
        verifier: None,
        insert: Box::new(move |insert| {
            inserts.send(insert).map_err(|error| error.to_string())?;
            Ok(0.0)
        }),
        control: std::sync::Arc::new(crate::persist::load_control::LoadControl::default()),
    }))));
    match wait_event(&events) {
        PersistEvent::Loaded(result) => {
            assert!(
                matches!(*result, PersistLoadResult::Restored(_)),
                "not restored: {result:?}"
            );
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert!(writer.send(PersistJob::RestoreDone));
    // Jobs run in order: once a later one is answered, the record is gone.
    assert!(writer.send(PersistJob::ChainPut {
        chain: chain.to_vec(),
        blocks: Vec::new(),
    }));
    assert!(matches!(wait_event(&events), PersistEvent::ChainPut(Ok(_))));
    inserted.try_recv().expect("the restored ledgers")
}

/// I5d5: a restored root staged ledger is a mask over the restored root
/// snarked ledger, as R38's bootstrap and OCaml's persistent frontier build
/// it, and moves with the root as a bootstrapped one does.
///
/// A ledger thread whose root staged ledger is a mask over its root snarked
/// ledger (R38's bootstrap) writes a checkpoint; a real store thread loads
/// it, and the ledgers go into a new `LedgerCtx` (`persist_insert`), and into
/// another that already held a ledger of the root snarked ledger's hash, as a
/// node holding the genesis ledger does for a store rooted at genesis. In
/// both the root staged ledger is a mask whose parent is the root snarked
/// ledger the ledger thread keeps, holding the diff's accounts. The first
/// commit, the replay's, leaves it so. The bootstrapped mask also holds
/// account 7, which its transactions changed and changed back, so the diff
/// leaves it out; the loader makes a restored mask hold every account the
/// root's scan state references (`hold_referenced_accounts`; the fixture's
/// scan state is empty, so the test does it for account 7). Then the same
/// root move in them and in a freshly bootstrapped `LedgerCtx` (R38's commit
/// steps, a new snarked ledger folded into the old one, holding account 7
/// part way through those transactions, then the new root's mask folded into
/// the root staged ledger's): every ledger has the hash, accounts and Merkle
/// paths of the bootstrapped one's, the root snarked ledger takes none of the
/// staged changes, and the root staged ledger is still a mask over it. A
/// restored mask that doesn't hold account 7 reads the new snarked ledger's.
#[test]
fn a_restored_root_staged_ledger_is_a_mask_and_moves_as_a_bootstrapped_one() {
    use std::sync::Arc;

    use crate::persist::test_fixtures::{synthetic_chain, ChainLedgers};

    fn set_balance(mask: &mut Mask, index: u64, balance: u64) {
        let mut account = mask.get_at_index(AccountIndex(index)).unwrap();
        account.balance = ledger::scan_state::currency::Balance::from_u64(balance);
        mask.set_at_index(AccountIndex(index), account).unwrap();
    }
    fn staged_ledger(mask: &Mask) -> (Arc<MinaBaseStagedLedgerHashStableV1>, StagedLedger) {
        let mut staged_ledger =
            StagedLedger::create_exn(constraint_constants().clone(), mask.clone()).unwrap();
        let hash = MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash());
        (Arc::new(hash), staged_ledger)
    }
    /// A `LedgerCtx` holding copies of the ledgers as R38's bootstrap leaves
    /// them: the staking and root snarked ledgers databases of their own, the
    /// root staged ledger a mask over the root snarked ledger with `staged`'s
    /// changes. And the root staged ledger's hash.
    fn bootstrapped(
        staking: &Mask,
        snarked: &Mask,
        staged: &Mask,
    ) -> (LedgerCtx, MinaBaseStagedLedgerHashStableV1) {
        let snarked_copy = snarked.copy();
        let mut staged_copy = snarked_copy.make_child();
        for index in staged.own_account_indexes() {
            let account = staged.get_at_index(index).unwrap();
            staged_copy.set_at_index(index, account).unwrap();
        }
        let mut ctx = LedgerCtx::default();
        ctx.snarked_ledgers.insert(hash_of(staking), staking.copy());
        ctx.snarked_ledgers.insert(hash_of(snarked), snarked_copy);
        let (hash, staged_ledger) = staged_ledger(&staged_copy);
        ctx.staged_ledgers.insert(hash.clone(), staged_ledger);
        (ctx, (*hash).clone())
    }
    /// The root staged ledger's mask, its parent being the root snarked
    /// ledger `ctx` keeps under `snarked`.
    fn root_staged(
        ctx: &LedgerCtx,
        staged: &MinaBaseStagedLedgerHashStableV1,
        snarked: &LedgerHash,
    ) -> Mask {
        let mask = ctx.staged_ledgers.get(staged).unwrap().ledger();
        assert!(mask.is_attached());
        assert_eq!(
            mask.get_parent().unwrap().get_uuid(),
            ctx.snarked_ledgers[snarked].get_uuid()
        );
        assert!(ctx.snarked_ledgers[snarked].is_root());
        mask
    }
    /// What a root move left: the root snarked ledger, the root staged
    /// ledger and the best tip's.
    struct Moved {
        snarked: Mask,
        staged: Mask,
        best_tip: Mask,
        hashes: [LedgerHash; 3],
    }
    /// One block's root move with a new snarked ledger: `commit`'s steps on
    /// the ledgers.
    fn move_root(
        ctx: &mut LedgerCtx,
        staking: &LedgerHash,
        snarked: &LedgerHash,
        staged: &MinaBaseStagedLedgerHashStableV1,
        new_account: &Account,
    ) -> Moved {
        let old_root = root_staged(ctx, staged, snarked);
        let old_root_id = old_root.get_uuid();
        let old_snarked_id = ctx.snarked_ledgers[snarked].get_uuid();
        let mut new_root = old_root.make_child();
        set_balance(&mut new_root, 1, 201);
        set_balance(&mut new_root, 31, 231);
        let mut best_tip = new_root.make_child();
        set_balance(&mut best_tip, 3, 303);
        best_tip
            .get_or_create_account(new_account.id(), new_account.clone())
            .unwrap();
        // The new root's snarked ledger, as `push_snarked_ledger` makes it
        // from part of the old root's transactions: its accounts 0 and 5
        // (the root staged ledger then holds them as the database does, and
        // prunes them), and 31 and 7 as those transactions left them part
        // way through (the root staged ledger holds its own).
        let mut new_snarked = ctx.snarked_ledgers[snarked].make_child();
        for index in [0, 5] {
            let account = old_root.get_at_index(AccountIndex(index)).unwrap();
            new_snarked
                .set_at_index(AccountIndex(index), account)
                .unwrap();
        }
        set_balance(&mut new_snarked, 31, 731);
        set_balance(&mut new_snarked, 7, 707);
        let hashes = [&new_snarked, &new_root, &best_tip].map(hash_of);
        ctx.snarked_ledgers
            .insert(hashes[0].clone(), new_snarked.clone());

        let mut keep = LedgersToKeep::new();
        keep.add_snarked(staking.clone());
        keep.add_snarked(hashes[0].clone());
        let mut new_root_staged = None;
        for mask in [&new_root, &best_tip] {
            let (hash, staged_ledger) = staged_ledger(mask);
            keep.add_staged(hash.clone());
            new_root_staged.get_or_insert(hash.clone());
            ctx.staged_ledgers.insert(hash, staged_ledger);
        }
        let new_root_staged = new_root_staged.unwrap();
        ctx.drop_snarked_ledgers(&keep);
        ctx.staged_ledgers.retain(|hash| keep.contains(hash));
        ctx.compact_snarked_ledgers(&keep);
        assert!(ctx.commit_root_staged_ledger(&new_root_staged, &hashes[0]));

        // The new snarked ledger went into the old database; the new root's
        // mask into the old root staged ledger's, still a mask over it.
        let snarked = ctx.snarked_ledgers[&hashes[0]].clone();
        assert_eq!(snarked.get_uuid(), old_snarked_id);
        let staged = root_staged(ctx, &new_root_staged, &hashes[0]);
        assert_eq!(staged.get_uuid(), old_root_id);
        assert!(!new_root.is_attached());
        assert_eq!(best_tip.get_parent().unwrap().get_uuid(), old_root_id);
        assert_eq!(hash_of(&snarked), hashes[0]);
        Moved {
            snarked,
            staged,
            best_tip,
            hashes,
        }
    }
    fn assert_same(name: &str, got: &Mask, want: &Mask) {
        let (mut got, mut want) = (got.clone(), want.clone());
        assert_eq!(got.merkle_root(), want.merkle_root(), "{name}");
        assert_eq!(got.num_accounts(), want.num_accounts(), "{name}");
        for index in 0..want.num_accounts() as u64 {
            let account = want.get_at_index(AccountIndex(index)).unwrap();
            assert_eq!(
                got.get_at_index(AccountIndex(index)).as_ref(),
                Some(&account),
                "{name}: account {index}"
            );
            assert_eq!(
                got.index_of_account(account.id()),
                Some(AccountIndex(index)),
                "{name}: account {index}"
            );
            assert_eq!(
                got.merkle_path_at_index(AccountIndex(index)),
                want.merkle_path_at_index(AccountIndex(index)),
                "{name}: path {index}"
            );
        }
    }

    // The ledgers before the restart: a staking ledger, the root snarked
    // ledger (30 + 3 accounts) and the root staged ledger over it (6 of its
    // own: 0, 5 and 31 changed, 2 new, and 7 changed back).
    let staking = ledger_with(30);
    let mut snarked = staking.copy();
    change(&mut snarked, &[1, 2]);
    append(&mut snarked, 3);
    let mut staged = child(&snarked, &[0, 5, 31], 2);
    let changed_back = snarked.get_at_index(AccountIndex(7)).unwrap();
    staged
        .set_at_index(AccountIndex(7), changed_back.clone())
        .unwrap();
    assert_eq!(staged.own_account_indexes().len(), 6);
    let [s, r] = [&staking, &snarked].map(hash_of);
    let staking_expected = expected(&staking);
    let new_account = Account::rand();

    // The bootstrapped node, which never restarts.
    let (mut fresh, staged_hash) = bootstrapped(&staking, &snarked, &staged);
    assert_eq!(
        root_staged(&fresh, &staged_hash, &r).validate_child_account_indexes(),
        Ok(6)
    );

    // The node that restarts: a checkpoint at the first block, written by a
    // real ledger thread and store thread.
    let genesis = v2::StateHash::from_fp(7u64.into());
    let blocks = synthetic_chain(
        v2::StateHash::from_fp(8u64.into()),
        3,
        &ChainLedgers {
            staking: s.clone(),
            next: s.clone(),
            snarked: r.clone(),
            staged: staged_hash.clone(),
        },
    );
    let chain = blocks.iter().map(entry).collect::<Vec<_>>();
    let tip_slot = blocks.last().unwrap().global_slot();
    let mem = MemBackend::new();
    let (writer, events) = opened_writer(&mem, &genesis);
    assert!(writer.send(PersistJob::ChainPut {
        chain: chain.clone(),
        blocks: blocks.clone(),
    }));
    assert!(matches!(wait_event(&events), PersistEvent::ChainPut(Ok(_))));
    let (mut ctx, _) = bootstrapped(&staking, &snarked, &staged);
    let stream_gate = Arc::new(StreamGate::default());
    ctx.persist.after_read_part = Some(Arc::new({
        let stream_gate = stream_gate.clone();
        move || {
            let pause = stream_gate.lock().unwrap().take();
            if let Some((entered, resume)) = pause {
                entered.send(()).unwrap();
                let _ = resume.recv();
            }
        }
    }));
    ctx.set_persist_writer(writer);
    let manager = LedgerManager::spawn(ctx);
    let reads = vec![(
        s.clone(),
        staking_expected.accounts[..3]
            .iter()
            .map(|account| account.id())
            .collect(),
        staking_expected.accounts[..3].to_vec(),
    )];
    let (snapshot, done, _) = checkpoint(
        &manager,
        &events,
        &stream_gate,
        PersistCheckpointRequest {
            checkpoint: Checkpoint {
                root: chain[0].hash.clone(),
                root_height: chain[0].height,
                root_snarked_ledger: r.clone(),
                root_staged_ledger_hash: staged_hash.clone(),
            },
            chain: chain.clone(),
            ledgers: vec![r.clone(), s.clone()],
            stored: Vec::new(),
            needed_protocol_states: BTreeMap::new(),
        },
        &reads,
    );
    assert_eq!(done.generation, 2);
    assert_eq!(snapshot.staged_accounts, 5);
    drop(manager);

    // The restart, into a ledger thread holding nothing, and into one that
    // held a copy of the root snarked ledger; and one more whose mask is
    // left without account 7.
    let mut restored = LedgerCtx::default();
    restored.persist_insert(restored_insert(&mem, &genesis, tip_slot, &chain));
    let held = snarked.copy();
    let mut holding = LedgerCtx::default();
    holding.snarked_ledgers.insert(r.clone(), held.clone());
    holding.persist_insert(restored_insert(&mem, &genesis, tip_slot, &chain));
    assert_eq!(holding.snarked_ledgers[&r].get_uuid(), held.get_uuid());
    let mut unheld = LedgerCtx::default();
    unheld.persist_insert(restored_insert(&mem, &genesis, tip_slot, &chain));
    // The persistence fixture has an empty scan state. Model the pending
    // reference explicitly before exercising commit-time reclamation, so
    // account 7 is protected for the same reason as in a real root commit.
    let with_pending = |ctx: &mut LedgerCtx| {
        let old = ctx.staged_ledgers.get(&staged_hash).unwrap();
        let mut ledger = StagedLedger::from_persisted_unchecked(
            constraint_constants(),
            super::super::ledger_reclamation_tests::pending_scan([*changed_back.clone()]),
            old.ledger(),
            old.pending_coinbase_collection().clone(),
        );
        let hash = MinaBaseStagedLedgerHashStableV1::from(&ledger.hash());
        ctx.staged_ledgers.insert(Arc::new(hash.clone()), ledger);
        hash
    };
    let pending_hashes = [&mut fresh, &mut restored, &mut holding, &mut unheld].map(with_pending);
    assert!(pending_hashes.iter().all(|hash| hash == &pending_hashes[0]));
    let staged_hash = pending_hashes[0].clone();
    for (name, ctx) in [
        ("restored", &mut restored),
        ("holding", &mut holding),
        ("unheld", &mut unheld),
    ] {
        let mask = root_staged(ctx, &staged_hash, &r);
        assert_eq!(mask.validate_child_account_indexes(), Ok(5), "{name}");
        if name != "unheld" {
            // What `hold_referenced_accounts` does for the accounts the
            // root's scan state references.
            assert_eq!(mask.hold_parent_accounts([changed_back.id()]), 1);
            assert_eq!(mask.hold_parent_accounts([changed_back.id()]), 0);
            assert_eq!(mask.validate_child_account_indexes(), Ok(6), "{name}");
        }
        assert_same(name, &mask, &staged);
        assert_same(name, &ctx.snarked_ledgers[&r], &snarked);
        // The replay's commit keeps the root: nothing moves.
        assert!(ctx.commit_root_staged_ledger(&staged_hash, &r));
        let after = root_staged(ctx, &staged_hash, &r);
        assert_eq!(after.get_uuid(), mask.get_uuid());
        assert_eq!(hash_of(&ctx.snarked_ledgers[&r]), r, "{name}");
        assert_eq!(hash_of(&after), hash_of(&staged), "{name}");
    }

    // The same root move in all of them.
    let want = move_root(&mut fresh, &s, &r, &staged_hash, &new_account);
    assert_eq!(hash_of(&want.staged), want.hashes[1]);
    assert_eq!(hash_of(&want.best_tip), want.hashes[2]);
    let lost = move_root(&mut unheld, &s, &r, &staged_hash, &new_account);
    assert_eq!(lost.hashes, want.hashes);
    assert_ne!(hash_of(&lost.staged), want.hashes[1]);
    assert_eq!(
        lost.staged.get_at_index(AccountIndex(7)).unwrap().balance,
        want.snarked.get_at_index(AccountIndex(7)).unwrap().balance
    );
    for (name, ctx) in [("restored", &mut restored), ("holding", &mut holding)] {
        let got = move_root(ctx, &s, &r, &staged_hash, &new_account);
        assert_eq!(got.hashes, want.hashes, "{name}");
        assert_eq!(hash_of(&got.snarked), want.hashes[0], "{name}");
        assert_eq!(hash_of(&got.staged), want.hashes[1], "{name}");
        assert_eq!(hash_of(&got.best_tip), want.hashes[2], "{name}");
        assert_same(&format!("{name} root snarked"), &got.snarked, &want.snarked);
        assert_same(&format!("{name} root staged"), &got.staged, &want.staged);
        assert_same(&format!("{name} best tip"), &got.best_tip, &want.best_tip);
        assert_eq!(
            got.staged.validate_child_account_indexes(),
            want.staged.validate_child_account_indexes(),
            "{name}"
        );
    }
    // The root snarked ledger took none of the staged changes: account 1 has
    // the new snarked ledger's balance, not the new root's 201.
    let account = want.snarked.get_at_index(AccountIndex(1)).unwrap();
    assert_eq!(
        account.balance,
        snarked.get_at_index(AccountIndex(1)).unwrap().balance
    );
}
