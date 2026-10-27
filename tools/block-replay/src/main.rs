//! Apply a chain of precomputed blocks, starting from genesis or from a staged
//! ledger dump, the way a node applies blocks it receives, and check each
//! block's staged ledger hash.
//!
//! The blocks are the JSON files nodes publish (for example to the public
//! `mina_network_block_data` bucket), optionally zstd-compressed, named
//! `<height>-<state hash>.json[.zst]`. The chain file lists `<height> <state
//! hash>` per line, oldest first, starting with the first block after genesis.
//! `fetch_chain.py` downloads both. For Mesa mainnet, whose genesis is height
//! 548147, right after the Berkeley fork block:
//!
//!   tools/block-replay/fetch_chain.py --from-height 548148 \
//!       --tip-height <height> --tip <state hash> --out blocks
//!   mina-block-replay --network mainnet --genesis mesa-mainnet.bin \
//!       --chain blocks/chain.txt --blocks blocks
//!
//! `mesa-mainnet.bin` is the genesis config `ledger-tool` writes from the Mesa
//! `daemon.json`.
//!
//! `--from-dump` starts from a staged ledger dump instead (`dump.rs`): at the
//! block whose staged ledger it holds, which has to be in the chain file and
//! the blocks directory too. `--write-dump` writes one for the last block
//! applied, so that a later replay can start there:
//!
//!   mina-block-replay --genesis mesa-mainnet.bin --chain blocks/chain.txt \
//!       --blocks blocks --to 563044 --write-dump 563044.binprot
//!   mina-block-replay --from-dump 563044.binprot --chain blocks/chain.txt \
//!       --blocks blocks
//!
//! `--dump DUMP --root HEIGHT` selects an explicit root, for example on
//! devnet. Both dump modes check the root block and its ledger hashes.

use std::{
    collections::HashMap,
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::Instant,
};

#[cfg(feature = "peak-alloc")]
use std::{collections::HashSet, sync::Arc};

use anyhow::{bail, Context};
use clap::Parser;
use ledger::{
    proofs::block::ProtocolState,
    scan_state::{
        currency::Slot,
        scan_state::Pass,
        transaction_logic::{
            apply_transaction_first_pass, apply_transaction_second_pass,
            protocol_state::{protocol_state_view, ProtocolStateView},
            transaction_partially_applied::TransactionPartiallyApplied,
            Transaction,
        },
    },
    sparse_ledger::SparseLedger,
    staged_ledger::{
        diff::Diff,
        staged_ledger::{SkipVerification, StagedLedger},
    },
    verifier::Verifier,
    BaseLedger, Mask,
};
use mina_block_replay::{dump::StagedLedgerDump, read_block};
use mina_curves::pasta::Fp;
use mina_node::transition_frontier::genesis::PrebuiltGenesisConfig;
use mina_p2p_messages::v2::{
    self, LedgerHash, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2,
};

#[cfg(feature = "peak-alloc")]
mod peak_alloc;
#[cfg(feature = "peak-alloc")]
#[global_allocator]
static ALLOCATOR: peak_alloc::CountingAllocator = peak_alloc::CountingAllocator;

#[derive(Parser)]
struct Args {
    /// Cache retained scan-state proofs in files (PC2 replay gate).
    #[arg(long)]
    proof_cache_dir: Option<PathBuf>,
    /// Network whose constants and signature prefixes apply.
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// Start at genesis: the prebuilt genesis config, as written by
    /// `ledger-tool`.
    #[arg(long, required_unless_present_any = ["from_dump", "dump"], conflicts_with_all = ["from_dump", "dump"])]
    genesis: Option<PathBuf>,
    /// Start from a staged ledger dump instead: the format a node writes with
    /// `MINA_DUMP_RECONSTRUCT`, or `--write-dump`.
    #[arg(long, conflicts_with_all = ["genesis", "dump", "root"])]
    from_dump: Option<PathBuf>,
    /// Start from a node's root dump at an explicit height.
    #[arg(long, requires = "root", conflicts_with_all = ["genesis", "from_dump"])]
    dump: Option<PathBuf>,
    /// Root height in the chain file, checked against the rebuilt dump.
    #[arg(long, requires = "dump")]
    root: Option<u32>,
    /// `<height> <state hash>` per line, oldest first.
    #[arg(long)]
    chain: PathBuf,
    /// Directory with the precomputed blocks.
    #[arg(long)]
    blocks: PathBuf,
    /// Stop after this height.
    #[arg(long)]
    to: Option<u32>,
    /// Write a staged ledger dump of the last block applied to this file. The
    /// replay then keeps the snarked ledger too, as a node does, which takes
    /// longer.
    #[arg(long)]
    write_dump: Option<PathBuf>,
    /// Move the genesis ledger's accounts to a file in this directory before
    /// the first block, as a node keeps its epoch ledgers (PC3a) and its root
    /// snarked ledger (PC3b): every block then reads the accounts it doesn't
    /// change from the file, and commits what it changes into the overlay,
    /// which is written into a new file once it holds enough
    /// (`MINA_LEDGER_REFREEZE_SLOTS` overrides how many).
    #[arg(long, requires = "genesis")]
    ledger_disk: Option<PathBuf>,
    /// Verify completed work and commands at this height, for allocation
    /// profiling. Other blocks retain the replay's hash-checking mode.
    #[cfg(feature = "peak-alloc")]
    #[arg(long)]
    verify_apply_at: Option<u32>,
    /// Estimate retained scan-state base-job heap by cloning each unique job
    /// and its two sparse ledgers at these heights.
    #[cfg(feature = "peak-alloc")]
    #[arg(long)]
    probe_witness_at: Vec<u32>,
    /// Sample from this height on and print where the time went.
    #[cfg(feature = "profile")]
    #[arg(long)]
    profile_from: Option<u32>,
}

/// Where a replay starts: the staged ledger of a block, and that block.
struct Start {
    staged_ledger: StagedLedger,
    state: MinaStateProtocolStateValueStableV2,
    hash: v2::StateHash,
    /// The block's height, or `None` at genesis, before the chain file.
    height: Option<u32>,
    snarked: Option<SnarkedLedger>,
}

/// The snarked ledger, kept up to date for `--write-dump`.
struct SnarkedLedger {
    ledger: Mask,
    hash: LedgerHash,
    /// Every protocol state seen, which the transactions the snarked ledger
    /// takes from the scan state need: a few MB for the whole chain.
    states: HashMap<Fp, MinaStateProtocolStateValueStableV2>,
    updates: u32,
}

impl SnarkedLedger {
    fn new(
        mut ledger: Mask,
        state: &MinaStateProtocolStateValueStableV2,
        states: HashMap<Fp, MinaStateProtocolStateValueStableV2>,
    ) -> anyhow::Result<Self> {
        let hash = snarked_ledger_hash(state).clone();
        let found = LedgerHash::from_fp(ledger.merkle_root());
        if found != hash {
            bail!("the snarked ledger is {found}, not the block's {hash}");
        }
        let mut snarked = Self {
            ledger,
            hash,
            states,
            updates: 0,
        };
        snarked.add_state(state)?;
        Ok(snarked)
    }

    fn add_state(&mut self, state: &MinaStateProtocolStateValueStableV2) -> anyhow::Result<()> {
        self.states
            .insert(state.try_hash()?.to_field()?, state.clone());
        Ok(())
    }

    /// After a block is applied: when it emitted a ledger proof, applies the
    /// proof's transactions to the snarked ledger, as a node does when the
    /// block becomes its root, and checks the result against the block's
    /// snarked ledger hash.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/full_frontier/full_frontier.ml#L483-L552>
    fn after_block(
        &mut self,
        staged_ledger: &StagedLedger,
        state: &MinaStateProtocolStateValueStableV2,
        emitted_a_proof: bool,
    ) -> anyhow::Result<()> {
        self.add_state(state)?;
        let target = snarked_ledger_hash(state);
        if !emitted_a_proof {
            if *target != self.hash {
                bail!("the snarked ledger moved to {target} without a ledger proof");
            }
            return Ok(());
        }
        let constraint_constants = mina_core::constants::constraint_constants();
        let states = &self.states;
        let Pass::FirstPassLedgerHash(_) = staged_ledger
            .scan_state()
            .get_snarked_ledger_sync(
                &mut self.ledger,
                |hash: Fp| {
                    states.get(&hash).cloned().ok_or_else(|| {
                        format!("no protocol state {}", v2::StateHash::from_fp(hash))
                    })
                },
                |global_slot: Slot,
                 view: &ProtocolStateView,
                 ledger: &mut Mask,
                 transaction: &Transaction| {
                    apply_transaction_first_pass(
                        constraint_constants,
                        global_slot,
                        view,
                        ledger,
                        transaction,
                    )
                },
                |ledger: &mut Mask, partially_applied: TransactionPartiallyApplied<Mask>| {
                    apply_transaction_second_pass(constraint_constants, ledger, partially_applied)
                },
                |global_slot: Slot,
                 view: &ProtocolStateView,
                 ledger: &mut SparseLedger,
                 transaction: &Transaction| {
                    apply_transaction_first_pass(
                        constraint_constants,
                        global_slot,
                        view,
                        ledger,
                        transaction,
                    )
                },
            )
            .map_err(anyhow::Error::msg)
            .context("applying a ledger proof's transactions to the snarked ledger")?;
        let found = LedgerHash::from_fp(self.ledger.merkle_root());
        if found != *target {
            bail!("snarked ledger hash mismatch: expected {target}, found {found}");
        }
        self.hash = target.clone();
        self.updates += 1;
        Ok(())
    }
}

fn snarked_ledger_hash(state: &MinaStateProtocolStateValueStableV2) -> &LedgerHash {
    &state
        .body
        .blockchain_state
        .ledger_proof_statement
        .target
        .first_pass_ledger
}

fn read_chain(path: &Path) -> anyhow::Result<Vec<(u32, String)>> {
    let mut chain = Vec::new();
    for line in
        BufReader::new(File::open(path).with_context(|| format!("{}", path.display()))?).lines()
    {
        let line = line?;
        let mut fields = line.split_whitespace();
        if let (Some(height), Some(hash)) = (fields.next(), fields.next()) {
            chain.push((height.parse()?, hash.to_owned()));
        }
    }
    Ok(chain)
}

fn start_at_genesis(
    path: &Path,
    keep_snarked: bool,
    ledger_disk: Option<&Path>,
) -> anyhow::Result<Start> {
    let started = Instant::now();
    let (mut masks, loaded) = PrebuiltGenesisConfig::load_from_bytes(&std::fs::read(path)?)?;
    let (_, state, hash) = loaded.protocol_states(
        loaded.genesis_total_currency.clone(),
        loaded.staking_epoch_total_currency.clone(),
        loaded.next_epoch_total_currency.clone(),
    )?;
    println!("genesis {hash} loaded in {:.1?}", started.elapsed());

    let ledger = masks.remove(0);
    drop(masks);
    if let Some(dir) = ledger_disk {
        // PC3a: the requested heap with the genesis ledger in memory, then in
        // a file.
        #[cfg(feature = "peak-alloc")]
        println!("peak_alloc_memory live_bytes {}", peak_alloc::start().0);
        std::fs::create_dir_all(dir)?;
        if let Some(slots) = std::env::var("MINA_LEDGER_REFREEZE_SLOTS")
            .ok()
            .and_then(|slots| slots.parse().ok())
        {
            ledger::database::disk::set_refreeze_slots(Some(slots));
        }
        let report = ledger
            .freeze_to_disk(dir)?
            .ok_or_else(|| anyhow::anyhow!("the genesis ledger is not a database"))?;
        println!(
            "genesis ledger moved to disk: {} accounts, {} bytes in the file, {} bytes of index, in {:.0} ms",
            report.accounts, report.file_bytes, report.index_bytes, report.ms
        );
        #[cfg(feature = "peak-alloc")]
        println!("peak_alloc_frozen live_bytes {}", peak_alloc::start().0);
    }
    let snarked = keep_snarked
        .then(|| SnarkedLedger::new(ledger.copy(), &state, HashMap::new()))
        .transpose()?;
    let staged_ledger =
        StagedLedger::create_exn(mina_core::constants::constraint_constants().clone(), ledger)
            .map_err(anyhow::Error::msg)?;
    Ok(Start {
        staged_ledger,
        state,
        hash,
        height: None,
        snarked,
    })
}

/// Rebuilds the dump's staged ledger and finds its block in the chain: the
/// first, from the height of the newest state the dump holds, with the
/// rebuilt staged ledger's hash.
fn start_from_dump(
    path: &Path,
    chain: &[(u32, String)],
    blocks: &Path,
    keep_snarked: bool,
    root: Option<u32>,
) -> anyhow::Result<Start> {
    /// Blocks to look at for the dump's own; it is normally the first.
    const SEARCH: usize = 8;

    if let Some(root) = root {
        if !chain.iter().any(|(height, _)| *height == root) {
            bail!("the root {root} is not in the chain file");
        }
    }
    let rebuild_started = Instant::now();
    let started = Instant::now();
    let dump = StagedLedgerDump::read(path)?;
    let states = dump.states_by_hash()?;
    let heights = dump
        .states
        .iter()
        .map(|state| state.body.consensus_state.blockchain_length.as_u32());
    let (oldest, newest) = (heights.clone().min(), heights.max());
    println!(
        "dump {}: {} accounts, {} protocol states ({}), read in {:.1?}",
        path.display(),
        dump.accounts.len(),
        states.len(),
        match (oldest, newest) {
            (Some(oldest), Some(newest)) => format!("{oldest} to {newest}"),
            _ => "none".to_owned(),
        },
        started.elapsed()
    );

    let started = Instant::now();
    let mut snarked_ledger = dump.snarked_ledger()?;
    let snarked_hash = LedgerHash::from_fp(snarked_ledger.merkle_root());
    println!(
        "snarked ledger {snarked_hash} built in {:.1?}",
        started.elapsed()
    );
    // A copy, before the staged ledger's mask is made: the replay commits it
    // into its parent after each block.
    let snarked_copy = keep_snarked.then(|| snarked_ledger.copy());

    let started = Instant::now();
    let mut staged_ledger = dump
        .staged_ledger(&snarked_ledger, &states)
        .map_err(|error| {
            // A dump taken after the staged ledger was committed into the
            // snarked ledger: the rebuild applies the scan state's
            // transactions a second time, and fails.
            if snarked_hash == dump.staged_ledger_hash {
                error.context(format!(
                    "the dump's snarked ledger {snarked_hash} is its staged ledger already: \
                     it was taken after the staged ledger was committed"
                ))
            } else {
                error
            }
        })?;
    let staged_hash = MinaBaseStagedLedgerHashStableV1::from(&staged_ledger.hash());
    println!("staged ledger rebuilt in {:.1?}", started.elapsed());

    // The newest state the dump holds is normally its block's parent's.
    let newest = root.unwrap_or_else(|| newest.unwrap_or(0));
    let from = chain
        .iter()
        .position(|(height, _)| *height >= newest)
        .unwrap_or(chain.len());
    let mut looked_at = Vec::new();
    let search = if root.is_some() { 1 } else { SEARCH };
    for (height, hash) in chain.iter().skip(from).take(search) {
        let block = read_block(blocks, *height, hash)?;
        let block_hash = block.protocol_state.try_hash()?;
        if block_hash.to_string() != *hash {
            bail!("block {height}: protocol state hashes to {block_hash}, not {hash}");
        }
        if block
            .protocol_state
            .body
            .blockchain_state
            .staged_ledger_hash
            != staged_hash
        {
            if root.is_some() {
                bail!("the dump is not the staged ledger of {height} {hash}");
            }
            looked_at.push(*height);
            continue;
        }
        let state = block.protocol_state;
        if *snarked_ledger_hash(&state) != snarked_hash {
            bail!(
                "block {height} {hash} has the dump's staged ledger, but its snarked ledger is {}, not the dump's {snarked_hash}",
                snarked_ledger_hash(&state)
            );
        }
        if root.is_some() {
            println!(
                "root {height} {hash} rebuilt from {} accounts in {:.1?}",
                dump.accounts.len(),
                rebuild_started.elapsed()
            );
        } else {
            println!("starting after block {height} {hash}, whose staged ledger the dump holds");
        }
        let snarked = snarked_copy
            .map(|ledger| SnarkedLedger::new(ledger, &state, states))
            .transpose()?;
        staged_ledger.commit_and_reparent_to_root();
        return Ok(Start {
            staged_ledger,
            state,
            hash: block_hash,
            height: Some(*height),
            snarked,
        });
    }
    bail!(
        "no block in the chain file has the dump's staged ledger {staged_hash:?}: looked at {looked_at:?}"
    )
}

fn main() -> anyhow::Result<()> {
    // Keep replay evidence tied to the same build identity as the node.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "build-info")
    {
        println!("Commit SHA: {}", mina_node::BuildEnv::get().git.commit_hash);
        return Ok(());
    }
    let args = Args::parse();
    if let Some(dir) = &args.proof_cache_dir {
        mina_core::snark::proof_cache::set_global_dir(dir)?;
    }
    mina_core::NetworkConfig::init(&args.network).map_err(anyhow::Error::msg)?;
    #[cfg(feature = "peak-alloc")]
    ledger::staged_ledger::staged_ledger::ALLOC_CHECKPOINT
        .set(peak_alloc::checkpoint)
        .map_err(|_| anyhow::anyhow!("allocation checkpoint already installed"))?;

    let chain = read_chain(&args.chain)?;
    let keep_snarked = args.write_dump.is_some();
    let start = match (&args.from_dump, &args.dump, &args.genesis, args.root) {
        (Some(dump), None, None, None) => {
            start_from_dump(dump, &chain, &args.blocks, keep_snarked, None)?
        }
        (None, Some(dump), None, Some(root)) => {
            start_from_dump(dump, &chain, &args.blocks, keep_snarked, Some(root))?
        }
        (None, None, Some(genesis), None) => {
            start_at_genesis(genesis, keep_snarked, args.ledger_disk.as_deref())?
        }
        _ => bail!("give --genesis, --from-dump, or --dump with --root"),
    };
    let Start {
        mut staged_ledger,
        state: mut pred_state,
        hash: mut pred_hash,
        height: start_height,
        mut snarked,
    } = start;
    let constraint_constants = mina_core::constants::constraint_constants();
    // PC1: the requested heap the loaded ledgers and staged ledger hold.
    #[cfg(feature = "peak-alloc")]
    println!("peak_alloc_loaded live_bytes {}", peak_alloc::start().0);

    let started = Instant::now();
    let mut applied = 0u32;
    let mut refreezes = 0u32;
    let mut last_height = start_height;
    #[cfg(feature = "peak-alloc")]
    println!("peak_alloc height clone_live clone_peak clone_total clone_largest apply_live apply_peak apply_total apply_largest apply_ms");
    #[cfg(feature = "profile")]
    let mut profiler = None;
    for (height, hash) in &chain {
        let height = *height;
        if start_height.is_some_and(|start| height <= start) {
            continue;
        }
        if args.to.is_some_and(|to| height > to) {
            break;
        }

        #[cfg(feature = "profile")]
        if args.profile_from == Some(height) {
            profiler = Some(
                pprof::ProfilerGuardBuilder::default()
                    .frequency(1000)
                    .build()?,
            );
        }

        let block = read_block(&args.blocks, height, hash)?;
        let state = &block.protocol_state;
        if state.previous_state_hash != pred_hash {
            bail!(
                "block {height} {hash}: parent {} is not the previous block {pred_hash}",
                state.previous_state_hash
            );
        }

        let pred_view = protocol_state_view(&pred_state)?;
        let pred: ProtocolState = (&pred_state).try_into()?;
        let consensus = &state.body.consensus_state;
        let diff: Diff = (&block.staged_ledger_diff).try_into()?;
        #[cfg(feature = "peak-alloc")]
        if args.verify_apply_at == Some(height) {
            let init_start = peak_alloc::start();
            ledger::proofs::verifiers::TransactionVerifier::make();
            let (live, peak, total, largest) = peak_alloc::sample(init_start);
            println!("peak_verifier_init {height} {live} {peak} {total} {largest}");
        }
        // Match LedgerCtx::block_apply_with: retain the parent while applying
        // to a cloned staged ledger. The ledger mask itself is shared; apply
        // creates its own child mask.
        #[cfg(feature = "peak-alloc")]
        let clone_start = peak_alloc::start();
        let mut applying_ledger = staged_ledger.clone();
        #[cfg(feature = "peak-alloc")]
        let (clone_live, clone_peak, clone_total, clone_largest) = peak_alloc::sample(clone_start);
        #[cfg(feature = "peak-alloc")]
        let apply_start = peak_alloc::start();
        #[cfg(feature = "peak-alloc")]
        let apply_started = Instant::now();
        let result = applying_ledger
            .apply(
                {
                    #[cfg(feature = "peak-alloc")]
                    if args.verify_apply_at == Some(height) {
                        None
                    } else {
                        Some(SkipVerification::All)
                    }
                    #[cfg(not(feature = "peak-alloc"))]
                    Some(SkipVerification::All)
                },
                constraint_constants,
                Slot::from_u32(consensus.global_slot_since_genesis.as_u32()),
                diff,
                (),
                &Verifier,
                &pred_view,
                pred.hashes(),
                (&consensus.coinbase_receiver).try_into()?,
                consensus.supercharge_coinbase,
            )
            .map_err(|e| anyhow::anyhow!("block {height} {hash}: {e:?}"))?;
        #[cfg(feature = "peak-alloc")]
        let apply_ms = apply_started.elapsed().as_secs_f64() * 1000.0;
        #[cfg(feature = "peak-alloc")]
        let (apply_live, apply_peak, apply_total, apply_largest) = peak_alloc::sample(apply_start);

        let found = MinaBaseStagedLedgerHashStableV1::from(&result.hash_after_applying);
        let expected = &state.body.blockchain_state.staged_ledger_hash;
        if &found != expected {
            bail!(
                "block {height} {hash}: staged ledger hash mismatch\n  expected {expected:?}\n  found    {found:?}"
            );
        }
        #[cfg(feature = "peak-alloc")]
        if args.probe_witness_at.contains(&height) {
            let scan = applying_ledger.scan_state();
            let mut jobs = scan.scan_state.retained_base_jobs();
            jobs.extend(scan.previous_incomplete_zkapp_updates.0.iter().cloned());

            let mut seen = HashSet::new();
            let mut bytes = 0usize;
            let mut largest = 0usize;
            for job in jobs {
                if !seen.insert(Arc::as_ptr(&job)) {
                    continue;
                }
                let started = peak_alloc::start();
                let clone = (*job).clone();
                let first = job.first_pass_ledger_witness.copy_content();
                let second = job.second_pass_ledger_witness.copy_content();
                let (live, _, _, _) = peak_alloc::sample(started);
                bytes += live.max(0) as usize;
                largest = largest.max(live.max(0) as usize);
                drop((clone, first, second));
            }
            println!("witness_probe {height} {} {bytes} {largest}", seen.len());
        }
        staged_ledger = applying_ledger;
        staged_ledger.commit_and_reparent_to_root();
        // PC3b: the genesis ledger on disk is the root the blocks commit
        // into, as a node's root snarked ledger is: once its overlay holds
        // enough changes, the file is written again with them.
        if let Some(dir) = &args.ledger_disk {
            let root = staged_ledger.ledger_ref();
            if root.refreeze_due() {
                let report = root
                    .freeze_to_disk(dir)?
                    .ok_or_else(|| anyhow::anyhow!("the root ledger is not on disk"))?;
                refreezes += 1;
                println!(
                    "refreeze {height}: {} changed, {} accounts, {} bytes, in {:.0} ms",
                    report.changed, report.accounts, report.file_bytes, report.ms
                );
            }
        }
        #[cfg(feature = "peak-alloc")]
        println!(
            "peak_alloc {height} {clone_live} {clone_peak} {clone_total} {clone_largest} {apply_live} {apply_peak} {apply_total} {apply_largest} {apply_ms:.3}"
        );
        #[cfg(feature = "peak-alloc")]
        {
            let phases = peak_alloc::phase_rows();
            print!("peak_phase {height}");
            for (live, peak, total) in phases {
                print!(" {live} {peak} {total}");
            }
            println!();
        }

        let block_hash = state.try_hash()?;
        if block_hash.to_string() != *hash {
            bail!("block {height}: protocol state hashes to {block_hash}, not {hash}");
        }
        if let Some(snarked) = &mut snarked {
            snarked
                .after_block(&staged_ledger, state, result.ledger_proof.is_some())
                .with_context(|| format!("block {height} {hash}"))?;
        }

        applied += 1;
        if applied.is_multiple_of(250) {
            println!(
                "{height} {hash}: {applied} blocks in {:.0?} ({:.2} s/block)",
                started.elapsed(),
                started.elapsed().as_secs_f64() / applied as f64
            );
        }
        pred_state = block.protocol_state;
        pred_hash = block_hash;
        last_height = Some(height);
    }

    println!(
        "applied {applied} blocks to {pred_hash} in {:.0?}, every staged ledger hash matching",
        started.elapsed()
    );
    // PC1: and after the last block, with its scan state.
    #[cfg(feature = "peak-alloc")]
    println!("peak_alloc_end live_bytes {}", peak_alloc::start().0);
    if args.ledger_disk.is_some() {
        println!(
            "ledger disk: {:?}, {refreezes} refreezes, {} changed since the last, {} cached",
            ledger::database::disk::stats(),
            staged_ledger.ledger_ref().disk_state().unwrap_or(0),
            staged_ledger.ledger_ref().disk_cached().unwrap_or(0),
        );
    }

    if let (Some(path), Some(snarked)) = (&args.write_dump, &snarked) {
        let started = Instant::now();
        StagedLedgerDump::of_staged_ledger(&staged_ledger, &snarked.ledger, &snarked.states)?
            .write(path)?;
        println!(
            "wrote the staged ledger of {} {pred_hash} to {} in {:.1?}; the snarked ledger moved {} times, to {}",
            last_height.map_or("genesis".to_owned(), |height| height.to_string()),
            path.display(),
            started.elapsed(),
            snarked.updates,
            snarked.hash
        );
    }

    #[cfg(feature = "profile")]
    if let Some(profiler) = profiler {
        mina_block_replay::print_profile(&profiler.report().build()?);
    }
    Ok(())
}
