//! Offline heap and producer-path measurement on an OCaml reconstruction dump.
//! Usage: scan_state_memory <reconstruction.binprot> [iterations] [--interactive]
//! Or append --write-after <path> to capture the applied producer scan state.
//! --describe prints the fixture's protocol-state height range without rebuilding.
//! --completed-block <uncompressed-precomputed.json> supplies real archived work.
//! Add --with-transactions to also offer that block's real user commands.
//! Run the same source on main and the sharing branch. No node or keys needed.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeMap,
    hint::black_box,
    io::{BufRead, Write},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    time::Instant,
};

use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    binprot::{self, macros::BinProtRead, BinProtRead, BinProtWrite},
    list::List,
    v2,
};
use mina_tree::{
    scan_state::{
        currency::Slot,
        scan_state::{transaction_snark::work, ScanState},
        transaction_logic::{local_state::LocalState, protocol_state::protocol_state_view, valid},
    },
    staged_ledger::staged_ledger::StagedLedger,
    verifier::Verifier,
    Account, BaseLedger, Mask,
};

struct Counted;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counted {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size(), Relaxed);
            REQUESTED.fetch_add(l.size(), Relaxed);
            CALLS.fetch_add(1, Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Relaxed);
        unsafe { System.dealloc(p, l) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            LIVE.fetch_add(n, Relaxed);
            LIVE.fetch_sub(l.size(), Relaxed);
            REQUESTED.fetch_add(n, Relaxed);
            CALLS.fetch_add(1, Relaxed);
        }
        q
    }
}
#[global_allocator]
static ALLOC: Counted = Counted;

#[derive(BinProtRead)]
struct Dump {
    accounts: Vec<v2::MinaBaseAccountBinableArgStableV2>,
    scan_state: v2::TransactionSnarkScanStateStableV2,
    pending_coinbase: v2::MinaBasePendingCoinbaseStableV2,
    staged_ledger_hash: v2::LedgerHash,
    states: List<v2::MinaStateProtocolStateValueStableV2>,
}

fn cpu_ns() -> u64 {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) },
        0
    );
    t.tv_sec as u64 * 1_000_000_000 + t.tv_nsec as u64
}

fn measure(n: usize, mut f: impl FnMut()) -> serde_json::Value {
    for _ in 0..10 {
        f();
    }
    let mut wall = Vec::with_capacity(n);
    let mut cpu = Vec::with_capacity(n);
    let requested = REQUESTED.load(Relaxed);
    let calls = CALLS.load(Relaxed);
    for _ in 0..n {
        let c = cpu_ns();
        let w = Instant::now();
        f();
        wall.push(w.elapsed().as_nanos() as u64);
        cpu.push(cpu_ns() - c);
    }
    let bytes = REQUESTED.load(Relaxed) - requested;
    let calls = CALLS.load(Relaxed) - calls;
    wall.sort_unstable();
    cpu.sort_unstable();
    let stats = |v: &[u64]| serde_json::json!({"mean_ns": v.iter().sum::<u64>() as f64 / n as f64, "p50_ns":v[n/2], "p95_ns":v[(n*95).div_ceil(100)-1]});
    serde_json::json!({"iterations":n,"wall":stats(&wall),"thread_cpu":stats(&cpu),"requested_bytes_per_iteration":bytes as f64/n as f64,"allocation_calls_per_iteration":calls as f64/n as f64})
}

fn main() {
    mina_core::NetworkConfig::init("mainnet").unwrap();
    let mut args = std::env::args().skip(1);
    let dump = Dump::binprot_read(&mut std::io::BufReader::new(
        std::fs::File::open(args.next().expect("dump path")).unwrap(),
    ))
    .unwrap();
    let n: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(500);
    assert!(n >= 20);
    let mut interactive = false;
    let mut describe = false;
    let mut write_after = None;
    let mut completed_block = None;
    let mut with_transactions = false;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--interactive" => interactive = true,
            "--describe" => describe = true,
            "--with-transactions" => with_transactions = true,
            "--write-after" => write_after = Some(args.next().expect("producer scan output path")),
            "--completed-block" => completed_block = Some(args.next().expect("block JSON path")),
            _ => panic!("unknown option: {flag}"),
        }
    }
    let heights: Vec<_> = dump
        .states
        .iter()
        .map(|s| s.body.consensus_state.blockchain_length.as_u32())
        .collect();
    if describe {
        println!(
            "{}",
            serde_json::json!({"protocol_state_min_height":heights.iter().min(),"protocol_state_max_height":heights.iter().max(),"fixture_accounts":dump.accounts.len()})
        );
        return;
    }
    let cc = mina_core::constants::constraint_constants();
    let scan = ScanState::try_from(&dump.scan_state).unwrap();
    let before = LIVE.load(Relaxed);
    let copied = black_box(scan.clone());
    let clone_live = LIVE.load(Relaxed) - before;
    drop(copied);
    let wire = {
        let mut out = Vec::new();
        dump.scan_state.binprot_write(&mut out).unwrap();
        out
    };
    let encoded = {
        let mut out = Vec::new();
        v2::TransactionSnarkScanStateStableV2::from(&scan)
            .binprot_write(&mut out)
            .unwrap();
        out
    };
    assert_eq!(
        wire, encoded,
        "scan-state bytes differ from the OCaml fixture"
    );
    let clone_bench = measure(n, || {
        black_box(scan.clone());
    });
    let hash_bench = measure(n, || {
        black_box(scan.hash());
    });
    let mut mask = Mask::create(35);
    for a in &dump.accounts {
        let a: Account = a.try_into().unwrap();
        mask.get_or_create_account(a.id(), a).unwrap();
    }
    let states: BTreeMap<Fp, _> = dump
        .states
        .iter()
        .map(|s| (s.try_hash().unwrap().to_field().unwrap(), s.clone()))
        .collect();
    let parent = dump
        .states
        .iter()
        .max_by_key(|s| s.body.consensus_state.blockchain_length.as_u32())
        .unwrap()
        .clone();
    let mut staged = StagedLedger::of_scan_state_pending_coinbases_and_snarked_ledger(
        (),
        cc,
        Verifier,
        scan.clone(),
        mask.make_child(),
        LocalState::empty(),
        dump.staged_ledger_hash.to_field().unwrap(),
        (&dump.pending_coinbase).try_into().unwrap(),
        |k| states[&k].clone(),
    )
    .unwrap();
    staged.hash();
    let view = protocol_state_view(&parent).unwrap();
    let receiver: mina_signer::CompressedPubKey = (&parent.body.consensus_state.coinbase_receiver)
        .try_into()
        .unwrap();
    let slot = Slot::from_u32(
        parent
            .body
            .consensus_state
            .global_slot_since_genesis
            .as_u32()
            + 1,
    );
    let state_hashes = (
        parent.try_hash().unwrap().to_field().unwrap(),
        parent.body.try_hash().unwrap().0.to_field().unwrap(),
    );
    let mut transactions: Vec<valid::UserCommand> = Vec::new();
    let completed: Vec<work::Checked> = if let Some(path) = &completed_block {
        let bytes = std::fs::read(path).unwrap();
        let mut value = v2::precomputed::ocaml_json_to_value(&bytes).unwrap();
        if value.get("data").is_some() {
            value = value["data"].take();
        }
        v2::precomputed::ocaml_forms_to_ours(&mut value).unwrap();
        let diff: v2::StagedLedgerDiffDiffStableV2 =
            serde_json::from_value(value["staged_ledger_diff"].take()).unwrap();
        if with_transactions {
            transactions = diff
                .diff
                .0
                .commands
                .iter()
                .chain(diff.diff.1.iter().flat_map(|d| d.commands.iter()))
                .map(|c| (&c.data).try_into().unwrap())
                .collect();
        }
        diff.diff
            .0
            .completed_works
            .iter()
            .chain(diff.diff.1.iter().flat_map(|d| d.completed_works.iter()))
            .map(|w| w.try_into().unwrap())
            .collect()
    } else {
        Vec::new()
    };
    // Match statements, rather than assuming the selected archive block follows
    // the dump. Unrelated completed proofs must never satisfy a work request.
    let completed: Vec<_> = completed.into_iter().map(|w| (w.statement(), w)).collect();
    let matched_work = staged
        .scan_state()
        .work_statements_for_new_diff()
        .iter()
        .filter(|s| completed.iter().any(|(statement, _)| statement == *s))
        .count();
    let make_diff = |s: &StagedLedger| {
        s.create_diff(
            cc,
            slot,
            Some(false),
            receiver.clone(),
            (),
            &view,
            transactions.clone(),
            |statement| {
                completed
                    .iter()
                    .find(|(s, _)| s == statement)
                    .map(|(_, w)| w.clone())
            },
            false,
        )
        .unwrap()
        .0
    };
    let trial_diff = make_diff(&staged);
    let included_work = trial_diff.diff.0.completed_works.len()
        + trial_diff
            .diff
            .1
            .as_ref()
            .map_or(0, |d| d.completed_works.len());
    let included_transactions = trial_diff.commands().len();
    let mut trial_staged = staged.clone();
    let trial_apply = trial_staged.apply_diff_unchecked(
        cc,
        slot,
        trial_diff,
        (),
        &view,
        state_hashes,
        receiver.clone(),
        false,
    );
    let apply_ok = trial_apply.is_ok();
    if let Some(path) = write_after {
        assert!(
            apply_ok,
            "producer trial must apply before capturing its scan state"
        );
        v2::TransactionSnarkScanStateStableV2::from(trial_staged.scan_state())
            .binprot_write(&mut std::io::BufWriter::new(
                std::fs::File::create(path).unwrap(),
            ))
            .unwrap();
    }
    drop(trial_staged);
    let apply_error = trial_apply.as_ref().err().map(|e| format!("{e:?}"));
    let mut produce = || {
        let mut s = black_box(staged.clone());
        black_box(s.hash());
        let d = make_diff(&s);
        if apply_ok {
            black_box(
                s.apply_diff_unchecked(
                    cc,
                    slot,
                    d,
                    (),
                    &view,
                    state_hashes,
                    receiver.clone(),
                    false,
                )
                .unwrap(),
            );
        } else {
            black_box(d);
        }
    };
    let producer = if interactive {
        println!(
            "{}",
            serde_json::json!({"ready":true,"trial_apply_succeeded":apply_ok,"scan_clone_live_requested_bytes":clone_live,"matched_work":matched_work,"included_work":included_work,"included_transactions":included_transactions})
        );
        std::io::stdout().flush().unwrap();
        for line in std::io::stdin().lock().lines() {
            let count: usize = line.unwrap().parse().unwrap();
            assert!(count >= 20);
            println!("{}", measure(count, &mut produce));
            std::io::stdout().flush().unwrap();
        }
        serde_json::Value::Null
    } else {
        measure(n, produce)
    };
    let mut frozen_after_production = Vec::new();
    v2::TransactionSnarkScanStateStableV2::from(&scan)
        .binprot_write(&mut frozen_after_production)
        .unwrap();
    assert_eq!(
        wire, frozen_after_production,
        "producer updates changed a retained snapshot"
    );
    println!(
        "{}",
        serde_json::json!({"scan_wire_bytes":wire.len(),"scan_clone_live_requested_bytes":clone_live,"scan_tree_reachable_bytes":scan.scan_state.owned_tree_bytes(),"clone":clone_bench,"scan_hash":hash_bench,"producer_clone_hash_create_diff_apply":producer,"trial_apply_succeeded":apply_ok,"trial_apply_error":apply_error,"completed_work_block":completed_block,"matched_work":matched_work,"included_work":included_work,"offered_transactions":transactions.len(),"included_transactions":included_transactions,"fixture_accounts":dump.accounts.len()})
    );
}
