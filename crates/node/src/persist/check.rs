//! The offline store checker (R20 D8): loads a store directory as a restore
//! would, but rehashing every ledger (`PERSIST_VERIFY=full`), and checks what
//! a restore relies on without a node:
//!
//! - every file the newest valid manifest references has its length and
//!   CRC32;
//! - every listed ledger loads, with its diff bases, and hashes to its name;
//! - every loaded ledger's derived indexes agree with its leaves;
//! - the checkpoint's root staged ledger loads as a diff over the root snarked
//!   ledger, and with the parts, through `StagedLedger::from_persisted_unchecked`,
//!   has the complete staged ledger hash of the checkpoint and of the root
//!   block (I5d1's hash gate);
//! - the parts hold every protocol state the root's scan state needs;
//! - every block of the chain decodes, hashes to its name, has the recorded
//!   height and slot, and links to the one before; the first is the
//!   checkpoint's root, with its ledger hashes;
//! - optionally (`rebuild_staged`), the root staged ledger rebuilt the slow
//!   way, by applying the scan state's transactions again, has the same hash.
//!
//! It never writes: no garbage collection, no reset. Data files no manifest
//! references (what a crash leaves) are listed; the node's next open deletes
//! them.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io::Write,
    path::Path,
    sync::Arc,
    time::Instant,
};

use anyhow::Context;
use ledger::{
    scan_state::{
        pending_coinbase::PendingCoinbase, scan_state::ScanState,
        transaction_logic::local_state::LocalState,
    },
    staged_ledger::staged_ledger::StagedLedger,
    verifier::Verifier,
    BaseLedger,
};
use mina_core::block::ArcBlockWithHash;
use mina_curves::pasta::Fp;
use mina_p2p_messages::{binprot::BinProtRead, v2};

use super::{
    format::{block_name, parts_name, staged_name},
    FrontierStore, FsBackend, OpenOutcome, VerifyMode,
};
use crate::p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases;

pub struct CheckOptions {
    pub verify: VerifyMode,
    /// Also rebuild the root staged ledger from the snarked ledger and the
    /// scan state, as a bootstrap does: tens of seconds on mainnet.
    pub rebuild_staged: bool,
    /// Take the store's lock, so that no node writes it meanwhile.
    pub lock: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            verify: VerifyMode::Full,
            rebuild_staged: false,
            lock: true,
        }
    }
}

#[derive(Debug, Default)]
pub struct CheckSummary {
    pub generation: Option<u64>,
    pub has_checkpoint: bool,
    pub ledgers: usize,
    pub blocks: usize,
    pub files: usize,
    pub bytes: u64,
    /// Data files no valid manifest references.
    pub unreferenced: Vec<String>,
    /// Checks that failed. Empty means the store passed.
    pub problems: Vec<String>,
    /// What a restore would want but can do without.
    pub warnings: Vec<String>,
}

impl CheckSummary {
    pub fn passed(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Checks the store in `dir` (a chain's `.../<chain_id>/frontier`
/// directory), printing what it does to `out`.
pub fn check_store(
    dir: &Path,
    options: &CheckOptions,
    out: &mut dyn Write,
) -> anyhow::Result<CheckSummary> {
    let mut summary = CheckSummary::default();
    let started = Instant::now();
    anyhow::ensure!(dir.is_dir(), "{} is not a directory", dir.display());
    let backend = FsBackend::new(dir).context("opening the directory")?;
    let (mut store, report) =
        FrontierStore::inspect(backend, options.lock).context("reading the manifests")?;
    for (slot, status) in super::format::MANIFEST_SLOTS.iter().zip(&report.slots) {
        writeln!(out, "{slot}: {status:?}")?;
    }
    summary.unreferenced = store.unreferenced()?;
    if !summary.unreferenced.is_empty() {
        writeln!(
            out,
            "{} data files no manifest references (left by a crash; the next open deletes them):",
            summary.unreferenced.len()
        )?;
        for name in &summary.unreferenced {
            writeln!(out, "  {name}")?;
        }
    }
    let OpenOutcome::Opened { generation } = report.outcome else {
        writeln!(out, "no valid manifest: the store is empty")?;
        return Ok(summary);
    };
    let manifest = store.manifest().cloned().context("the newest manifest")?;
    summary.generation = Some(generation);
    summary.has_checkpoint = manifest.checkpoint.is_some();
    summary.files = manifest.files.len();
    summary.bytes = manifest.files.iter().map(|file| file.len).sum();
    writeln!(
        out,
        "generation {generation}, chain id {}, genesis {}, written at {} ms, by {}",
        manifest.chain_id.to_hex(),
        manifest.genesis_state_hash,
        manifest.written_at_ms,
        manifest.node_commit.0,
    )?;

    // Sizes, by kind, and every file's checksum.
    let mut by_kind = BTreeMap::<&str, (usize, u64)>::new();
    for file in &manifest.files {
        let kind = file.name().split('-').next().unwrap_or("?");
        let entry = by_kind.entry(kind).or_default();
        entry.0 += 1;
        entry.1 += file.len;
        writeln!(out, "  {:>12} {}", file.len, file.name())?;
    }
    for (kind, (count, bytes)) in &by_kind {
        writeln!(out, "{kind}: {count} files, {bytes} bytes")?;
    }
    writeln!(
        out,
        "in all: {} files, {} bytes",
        summary.files, summary.bytes
    )?;
    let checksums = Instant::now();
    for file in &manifest.files {
        if let Err(error) = store.verify_file(file) {
            summary.problems.push(format!("{}: {error}", file.name()));
        }
    }
    writeln!(
        out,
        "checksums of {} files: {:.2?}",
        manifest.files.len(),
        checksums.elapsed()
    )?;
    if !summary.passed() {
        return Ok(summary);
    }

    // Ledgers.
    let loading = Instant::now();
    let ledgers = match store.read_ledgers(&manifest, options.verify) {
        Ok(ledgers) => ledgers,
        Err(error) => {
            summary.problems.push(format!("ledgers: {error}"));
            return Ok(summary);
        }
    };
    summary.ledgers = manifest.ledgers.len();
    for (hash, ledger) in &ledgers {
        let checking = Instant::now();
        match ledger.validate_account_indexes() {
            Ok(count) => writeln!(
                out,
                "ledger {hash}: derived indexes: {count} accounts checked in {:.3} ms",
                checking.elapsed().as_secs_f64() * 1000.0
            )?,
            Err(error) => {
                summary
                    .problems
                    .push(format!("ledger {hash}: derived indexes: {error}"));
                return Ok(summary);
            }
        }
    }
    for hash in &manifest.ledgers {
        let mut ledger = ledgers[hash].clone();
        let kind = match store.ledger_base(hash)? {
            Some(base) => format!("diff over {base}"),
            None => "full".to_owned(),
        };
        writeln!(
            out,
            "ledger {hash}: {} accounts, {kind}",
            ledger.num_accounts()
        )?;
        if v2::LedgerHash::from_fp(ledger.merkle_root()) != *hash {
            summary.problems.push(format!("ledger {hash}: wrong root"));
        }
    }
    writeln!(
        out,
        "{} ledgers loaded ({:?}): {:.2?}",
        ledgers.len(),
        options.verify,
        loading.elapsed()
    )?;

    // Blocks.
    let reading = Instant::now();
    let mut blocks = Vec::with_capacity(manifest.chain.len());
    for (i, entry) in manifest.chain.iter().enumerate() {
        let name = block_name(&entry.hash);
        let Some(file) = manifest.file(&name).cloned() else {
            summary.problems.push(format!("{name} is not referenced"));
            continue;
        };
        let bytes = match store.read_blob(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                summary.problems.push(format!("{name}: {error}"));
                continue;
            }
        };
        let block = match v2::MinaBlockBlockStableV2::binprot_read(&mut bytes.as_slice())
            .map_err(|e| e.to_string())
            .and_then(|block| ArcBlockWithHash::try_new(Arc::new(block)).map_err(|e| e.to_string()))
        {
            Ok(block) => block,
            Err(error) => {
                summary.problems.push(format!("{name}: {error}"));
                continue;
            }
        };
        if block.hash() != &entry.hash {
            summary
                .problems
                .push(format!("{name}: holds block {}", block.hash()));
        }
        if block.height() != entry.height || block.global_slot() != entry.global_slot {
            summary.problems.push(format!(
                "{name}: height {} and slot {}, recorded {} and {}",
                block.height(),
                block.global_slot(),
                entry.height,
                entry.global_slot
            ));
        }
        if i > 0 {
            if let Some(previous) = manifest.chain.get(i - 1) {
                if block.pred_hash() != &previous.hash {
                    summary.problems.push(format!(
                        "{name}: its parent is {}, not {}",
                        block.pred_hash(),
                        previous.hash
                    ));
                }
            }
        }
        blocks.push(block);
    }
    summary.blocks = blocks.len();
    if let (Some(first), Some(last)) = (manifest.chain.first(), manifest.chain.last()) {
        writeln!(
            out,
            "chain: {} blocks, heights {} to {}, slots {} to {}: {:.2?}",
            manifest.chain.len(),
            first.height,
            last.height,
            first.global_slot,
            last.global_slot,
            reading.elapsed()
        )?;
    }
    if let Some(tip) = blocks.last() {
        if !manifest.ledgers.contains(tip.staking_epoch_ledger_hash()) {
            summary.warnings.push(format!(
                "the tip's staking ledger {} is not stored; a restore needs it",
                tip.staking_epoch_ledger_hash()
            ));
        }
    }

    let Some(checkpoint) = manifest.checkpoint.clone() else {
        writeln!(out, "no checkpoint: a restore would bootstrap")?;
        return Ok(summary);
    };
    writeln!(
        out,
        "checkpoint: root {} at {}, snarked ledger {}, staged ledger {}",
        checkpoint.root,
        checkpoint.root_height,
        checkpoint.root_snarked_ledger,
        checkpoint.root_staged_ledger_hash.non_snark.ledger_hash
    )?;
    match blocks.first() {
        Some(root) if root.hash() == &checkpoint.root => {
            if root.snarked_ledger_hash() != &checkpoint.root_snarked_ledger
                || root.staged_ledger_hashes() != &checkpoint.root_staged_ledger_hash
                || root.height() != checkpoint.root_height
            {
                summary.problems.push(
                    "the root block's ledger hashes or height differ from the checkpoint's".into(),
                );
            }
        }
        _ => summary
            .problems
            .push("the chain doesn't start at the checkpoint's root".into()),
    }

    // The root staged ledger.
    let Some(root_snarked) = ledgers.get(&checkpoint.root_snarked_ledger) else {
        summary
            .problems
            .push("the root snarked ledger is not stored".into());
        return Ok(summary);
    };
    let loading = Instant::now();
    let staged = match store.read_staged(&manifest, root_snarked, options.verify) {
        Ok(staged) => staged,
        Err(error) => {
            summary
                .problems
                .push(format!("{}: {error}", staged_name(&checkpoint.root)));
            return Ok(summary);
        }
    };
    let staged_time = loading.elapsed();
    let checking = Instant::now();
    // A mask over the root snarked ledger (I5d5): its own accounts.
    match staged.validate_child_account_indexes() {
        Ok(count) => writeln!(
            out,
            "root staged ledger: derived indexes: {count} accounts checked in {:.3} ms",
            checking.elapsed().as_secs_f64() * 1000.0
        )?,
        Err(error) => {
            summary
                .problems
                .push(format!("root staged ledger: derived indexes: {error}"));
            return Ok(summary);
        }
    }
    let loading = Instant::now();
    let parts_file = manifest
        .file(&parts_name(&checkpoint.root))
        .cloned()
        .context("the parts file")?;
    let parts_bytes = store.read_blob(&parts_file)?;
    let parts = match StagedLedgerAuxAndPendingCoinbases::binprot_read(&mut parts_bytes.as_slice())
    {
        Ok(parts) => parts,
        Err(error) => {
            summary
                .problems
                .push(format!("{}: {error}", parts_file.name()));
            return Ok(summary);
        }
    };
    drop(parts_bytes);
    let converted = (|| -> Result<(ScanState, PendingCoinbase), String> {
        let scan_state: ScanState = (&parts.scan_state)
            .try_into()
            .map_err(|e| format!("{e:?}"))?;
        let pending_coinbase: PendingCoinbase = (&parts.pending_coinbase)
            .try_into()
            .map_err(|e| format!("{e:?}"))?;
        Ok((scan_state, pending_coinbase))
    })();
    let (scan_state, pending_coinbase) = match converted {
        Ok(parts) => parts,
        Err(error) => {
            summary
                .problems
                .push(format!("{}: {error}", parts_file.name()));
            return Ok(summary);
        }
    };
    let parts_time = loading.elapsed();
    if parts.staged_ledger_hash != checkpoint.root_staged_ledger_hash.non_snark.ledger_hash {
        summary
            .problems
            .push("the parts are of another staged ledger".into());
    }
    let states = parts
        .needed_blocks
        .iter()
        .map(|state| {
            let hash = state.try_hash().map_err(|e| format!("{e:?}"))?;
            Ok((
                hash.to_field::<Fp>().map_err(|e| format!("{e:?}"))?,
                state.clone(),
            ))
        })
        .collect::<Result<HashMap<_, _>, String>>();
    let states = match states {
        Ok(states) => states,
        Err(error) => {
            summary.problems.push(format!("protocol states: {error}"));
            return Ok(summary);
        }
    };
    let required = scan_state.required_state_hashes();
    let missing = required
        .iter()
        .filter(|hash| !states.contains_key(hash))
        .count();
    if missing > 0 {
        summary.problems.push(format!(
            "the parts lack {missing} of the {} protocol states the scan state needs",
            required.len()
        ));
    }
    writeln!(
        out,
        "parts: {} bytes, {} protocol states, {} needed",
        parts_file.len,
        parts.needed_blocks.len(),
        required.iter().collect::<BTreeSet<_>>().len()
    )?;

    let constants = mina_core::constants::constraint_constants();
    let hashing = Instant::now();
    let mut persisted = StagedLedger::from_persisted_unchecked(
        constants,
        scan_state.clone(),
        staged,
        pending_coinbase.clone(),
    );
    let hash = v2::MinaBaseStagedLedgerHashStableV1::from(&persisted.hash());
    let hash_time = hashing.elapsed();
    if hash != checkpoint.root_staged_ledger_hash {
        summary.problems.push(
            "the persisted root staged ledger's complete hash is not the checkpoint's".into(),
        );
    }
    if blocks
        .first()
        .is_some_and(|root| root.staged_ledger_hashes() != &hash)
    {
        summary.problems.push(
            "the persisted root staged ledger's complete hash is not the root block's".into(),
        );
    }
    writeln!(
        out,
        "root staged ledger: diff loaded in {staged_time:.2?}, parts in {parts_time:.2?}, \
         from_persisted_unchecked and hash in {hash_time:.2?}: {}",
        if hash == checkpoint.root_staged_ledger_hash {
            "hash matches"
        } else {
            "HASH DIFFERS"
        }
    )?;

    if options.rebuild_staged && missing == 0 {
        let rebuilding = Instant::now();
        let rebuilt = StagedLedger::of_scan_state_pending_coinbases_and_snarked_ledger(
            (),
            constants,
            Verifier,
            scan_state,
            root_snarked.make_child(),
            LocalState::empty(),
            checkpoint
                .root_staged_ledger_hash
                .non_snark
                .ledger_hash
                .to_field()
                .context("the staged ledger hash")?,
            pending_coinbase,
            |hash| states[&hash].clone(),
        );
        match rebuilt {
            Ok(mut rebuilt) => {
                let rebuilt_hash = v2::MinaBaseStagedLedgerHashStableV1::from(&rebuilt.hash());
                if rebuilt_hash != checkpoint.root_staged_ledger_hash {
                    summary
                        .problems
                        .push("the rebuilt root staged ledger's hash differs".into());
                }
                writeln!(
                    out,
                    "root staged ledger rebuilt the slow way in {:.2?}: {}",
                    rebuilding.elapsed(),
                    if rebuilt_hash == hash {
                        "same hash"
                    } else {
                        "HASH DIFFERS"
                    }
                )?;
            }
            Err(error) => summary
                .problems
                .push(format!("rebuilding the staged ledger: {error}")),
        }
    }
    writeln!(out, "checked in {:.2?}", started.elapsed())?;
    Ok(summary)
}
