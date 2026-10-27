//! The ledger thread's part of persistence (R20 D3): a checkpoint's snapshot,
//! the pins that keep the ledgers being written whole from changing, and the
//! pieces of those ledgers the store thread asks for.
//!
//! The snapshot runs as a ledger write, right after a commit, so no other write
//! runs meanwhile. It takes the root staged ledger's diff over the root
//! snarked ledger and the diffs of the snarked ledgers to store, by Merkle walk
//! (tens of milliseconds each on mainnet), and clones the root's scan state and
//! pending coinbase, handed to the store thread with the job.
//!
//! The store thread may only be given plain values, encoded bytes, or objects
//! whose locks block. The diffs and the pending coinbase are plain values. The
//! scan state's clone is shallow: its jobs' witnesses are `SparseLedger`s the
//! ledger thread's staged ledger shares, which the ledger thread may convert
//! or copy while the store thread converts the clone; their lock blocks (it
//! used to be a `try_lock`, which panicked one of the two threads).
//!
//! A ledger written whole is not handed over: a `Mask` must only be used by
//! one thread (`Mask::with` takes its lock with `try_lock().expect(..)`, so a
//! read on the store thread while the ledger thread touches the same mask, or
//! any ancestor, panics one of them; `Database` is the same). Instead it is
//! pinned, and the store thread asks the ledger thread for it a piece at a
//! time (`LedgerRequest::PersistRead`), between the ledger thread's other
//! requests, as encoded bytes.
//! A pin keeps the ledger and all its ancestors in the commit's kept set, so
//! that no commit folds a mask into the pinned ledger or the pinned ledger into
//! its parent until the store thread unpins it (`LedgerRequest::PersistUnpin`),
//! when the job ends, on success or error.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use ledger::{BaseLedger, Mask, UnregisterBehavior};
// `std::time::Instant` isn't there in the browser (I5e).
use mina_p2p_messages::v2::{self, LedgerHash, MinaStateProtocolStateValueStableV2, StateHash};
use redux::Instant;
use serde::{Deserialize, Serialize};

use super::{merkle_root, ordered_required_protocol_states, LedgerCtx};
use crate::{
    ledger::{
        ledger_manager::{LedgerCaller, LedgerRequest, LedgerResponse},
        write::LedgersToKeep,
    },
    persist::{
        choose_ledger_write,
        event::{PersistLedgerWriteInfo, PersistLedgerWriteKind, PersistSnapshotInfo},
        ledger_codec::{
            encode_full_part, full_header, CodecError, FullLedgerSource, FullPart, STREAM_ACCOUNTS,
            STREAM_HASHES,
        },
        merkle_diff,
        writer::{
            CheckpointJob, DoneGuard, PartsSnapshot, PersistInsert, PersistJob, PersistWriter,
            SharedStreamStats,
        },
        ChainEntry, Checkpoint, LedgerFileHeader, LedgerWrite,
    },
};

/// What the node asks the ledger thread to snapshot for a checkpoint (D4).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PersistCheckpointRequest {
    pub checkpoint: Checkpoint,
    /// The best chain from the root to the tip.
    pub chain: Vec<ChainEntry>,
    /// The snarked ledgers to store: the root's, and the best tip's staking
    /// and next epoch ledgers. Those the ledger thread doesn't hold are left
    /// out (a next epoch ledger is built only once the root reaches the block
    /// that emitted it).
    pub ledgers: Vec<LedgerHash>,
    /// The ledgers the store holds, each with the base its file is a diff
    /// over, if it is one, as the store last reported.
    pub stored: Vec<(LedgerHash, Option<LedgerHash>)>,
    /// The protocol states the root's scan state needs.
    pub needed_protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
}

/// A ledger being written whole: kept as it is, with its ancestors, until the
/// store thread unpins it.
struct PersistPin {
    mask: Mask,
    header: LedgerFileHeader,
    /// The ledger's hash, and the hashes of all its ancestors.
    keep: Vec<LedgerHash>,
}

/// The ledger thread's persistence state, in `LedgerCtx`.
pub(crate) struct LedgerPersist {
    writer: Option<PersistWriter>,
    pins: BTreeMap<u64, PersistPin>,
    next_pin: u64,
    /// Accounts and hashes per piece of a streamed ledger.
    part_sizes: (u64, u64),
    #[cfg(test)]
    after_read_part: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Default for LedgerPersist {
    fn default() -> Self {
        Self {
            writer: None,
            pins: BTreeMap::new(),
            next_pin: 0,
            part_sizes: (STREAM_ACCOUNTS, STREAM_HASHES),
            #[cfg(test)]
            after_read_part: None,
        }
    }
}

/// A pinned ledger, read a piece at a time through the ledger thread.
struct PinnedLedger {
    caller: LedgerCaller,
    pin: u64,
    header: LedgerFileHeader,
    part_sizes: (u64, u64),
    stats: SharedStreamStats,
    #[cfg(test)]
    after_read_part: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for PinnedLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedLedger")
            .field("pin", &self.pin)
            .field("ledger_hash", &self.header.ledger_hash)
            .finish()
    }
}

impl FullLedgerSource for PinnedLedger {
    fn header(&self) -> &LedgerFileHeader {
        &self.header
    }

    fn read_part(&self, part: FullPart) -> Result<Vec<u8>, CodecError> {
        // Not `call_sync`: on the store thread, a stopped ledger thread must
        // be an error (persistence goes off), not the process exiting.
        let response = self
            .caller
            .try_call_sync(LedgerRequest::PersistRead {
                pin: self.pin,
                part,
            })
            .map_err(|phase| CodecError::SourceGone(format!("ledger thread stopped ({phase})")))?;
        match response {
            LedgerResponse::PersistRead(Ok((bytes, ms))) => {
                self.stats.lock().unwrap().add(ms);
                // Runs on the store thread, after a real streamed ledger read.
                #[cfg(test)]
                if let Some(hook) = &self.after_read_part {
                    hook();
                }
                Ok(bytes)
            }
            LedgerResponse::PersistRead(Err(error)) => Err(CodecError::Invalid(error)),
            _ => Err(CodecError::Invalid(
                "unexpected answer to a ledger read".into(),
            )),
        }
    }

    fn part_sizes(&self) -> (u64, u64) {
        self.part_sizes
    }
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

impl LedgerCtx {
    /// Where checkpoints go, if persistence is on.
    pub fn set_persist_writer(&mut self, writer: PersistWriter) {
        self.persist.writer = Some(writer);
    }

    /// Adds the pinned ledgers and their ancestors to what a commit keeps.
    pub(crate) fn persist_keep_pinned(&self, ledgers_to_keep: &mut LedgersToKeep) {
        for pin in self.persist.pins.values() {
            for hash in &pin.keep {
                ledgers_to_keep.add_snarked(hash.clone());
            }
        }
    }

    /// Pins a ledger: it and its ancestors stay as they are until unpinned.
    fn persist_pin(&mut self, mask: &Mask) -> Result<(u64, LedgerFileHeader), CodecError> {
        let header = full_header(mask)?;
        let mut keep = vec![header.ledger_hash.clone()];
        let mut parent = mask.get_parent();
        while let Some(mut ancestor) = parent {
            keep.push(merkle_root(&mut ancestor));
            parent = ancestor.get_parent();
        }
        self.persist.next_pin += 1;
        let pin = self.persist.next_pin;
        self.persist.pins.insert(
            pin,
            PersistPin {
                mask: mask.clone(),
                header: header.clone(),
                keep,
            },
        );
        Ok((pin, header))
    }

    pub(crate) fn persist_unpin(&mut self, pins: &[u64]) {
        for pin in pins {
            self.persist.pins.remove(pin);
        }
    }

    /// Takes a load's ledgers for a restore (R20 D3's `PersistInsert`, I5d1):
    /// the snarked ledgers among the committed ones, and the root staged
    /// ledger among the staged ones, each where its hash is absent (a ledger
    /// we hold already, such as a genesis ledger, stays as it is). The root
    /// staged ledger is a mask over the loaded root snarked ledger (I5d5); if
    /// that one was not taken, the mask moves to the one held
    /// ([`Self::persist_reparent_onto_held`]). The store thread built them and
    /// doesn't touch them again. Returns the time taken (milliseconds): R20
    /// asks for under 100 ms.
    pub(crate) fn persist_insert(&mut self, insert: PersistInsert) -> f64 {
        let started = Instant::now();
        let PersistInsert { snarked, staged } = insert;
        let mut inserted = 0usize;
        let mut kept = 0usize;
        for (hash, mask) in snarked {
            if self.snarked_ledgers.contains_key(&hash) {
                kept += 1;
                continue;
            }
            self.snarked_ledgers.insert(hash, mask);
            inserted += 1;
        }
        let mut staged_reparented = false;
        let staged_inserted = match staged {
            Some((hash, staged)) if self.staged_ledgers.get(&hash).is_none() => {
                staged_reparented = self.persist_reparent_onto_held(&staged.ledger());
                self.staged_ledgers.insert(Arc::new(hash), staged);
                true
            }
            _ => false,
        };
        let ms = elapsed_ms(started);
        mina_core::info!(
            mina_core::log::system_time();
            kind = "LedgerService::persist_insert",
            summary = format!("{inserted} snarked ledgers and {} staged ledger from the frontier store", u8::from(staged_inserted)),
            already_held = kept,
            staged_reparented = staged_reparented,
            ledger_thread_ms = ms,
        );
        ms
    }

    /// The restored root staged ledger is a mask over the loaded root snarked
    /// ledger. When this thread already held a ledger of that hash (a genesis
    /// ledger, for a store rooted at genesis) and kept it instead of the
    /// loaded one, the mask moves to the one held: the root staged ledger
    /// must hang off the snarked ledger the commits fold the next snarked
    /// ledger into (`compact_snarked_ledgers`) and stop at
    /// (`commit_root_staged_ledger`). The two hold the same accounts (same
    /// root), so the mask reads the same; its cached hashes are dropped with
    /// the move and computed again when read. True if it moved.
    pub(crate) fn persist_reparent_onto_held(&self, staged: &Mask) -> bool {
        let Some(mut parent) = staged.get_parent() else {
            return false;
        };
        let Some(held) = self.snarked_ledgers.get(&merkle_root(&mut parent)) else {
            return false;
        };
        if held.get_uuid() == parent.get_uuid() {
            return false;
        }
        staged.unregister_mask(UnregisterBehavior::IPromiseIAmReparentingThisMask);
        held.register_mask(staged.clone());
        true
    }

    #[cfg(test)]
    pub(crate) fn persist_pin_count(&self) -> usize {
        self.persist.pins.len()
    }

    /// Smaller pieces, so that tests stream small ledgers in many of them.
    #[cfg(test)]
    pub(crate) fn set_persist_part_sizes(&mut self, accounts: u64, hashes: u64) {
        self.persist.part_sizes = (accounts, hashes);
    }

    /// A piece of a pinned ledger's full file, and the time it took.
    pub(crate) fn persist_read(&self, pin: u64, part: FullPart) -> Result<(Vec<u8>, f64), String> {
        let started = Instant::now();
        let pin = self
            .persist
            .pins
            .get(&pin)
            .ok_or_else(|| format!("ledger pin {pin} was released"))?;
        let mut bytes = Vec::with_capacity(4 << 20);
        encode_full_part(&pin.mask, &pin.header, part, &mut bytes).map_err(|e| e.to_string())?;
        Ok((bytes, elapsed_ms(started)))
    }

    /// Takes a checkpoint's snapshot and hands the job to the store thread.
    pub(in crate::ledger) fn persist_checkpoint(
        &mut self,
        request: PersistCheckpointRequest,
        caller: &LedgerCaller,
    ) -> Result<PersistSnapshotInfo, String> {
        let started = Instant::now();
        let writer = self.persist.writer.clone().ok_or("persistence is off")?;
        if !self.persist.pins.is_empty() {
            // A checkpoint is only asked for when none is being written, and
            // the store thread unpins before it reports. Whatever is left is
            // stale: a job reading it would fail cleanly.
            mina_core::warn!(
                mina_core::log::system_time();
                kind = "LedgerService::persist_checkpoint",
                summary = "released stale ledger pins",
                pins = self.persist.pins.len(),
            );
            self.persist.pins.clear();
        }

        let PersistCheckpointRequest {
            checkpoint,
            chain,
            ledgers,
            stored,
            needed_protocol_states,
        } = request;
        let root_snarked = self
            .snarked_ledgers
            .get(&checkpoint.root_snarked_ledger)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "the root snarked ledger {} is missing",
                    checkpoint.root_snarked_ledger
                )
            })?;
        let staged_ledger = self
            .staged_ledgers
            .get_mut(&checkpoint.root_staged_ledger_hash)
            .ok_or_else(|| {
                format!(
                    "the root staged ledger {} is missing",
                    checkpoint.root_staged_ledger_hash.non_snark.ledger_hash
                )
            })?;
        // The conversion to wire types needs it computed.
        staged_ledger.pending_coinbase_collection_merkle_root();
        let scan_state = staged_ledger.scan_state().clone();
        let pending_coinbase = staged_ledger.pending_coinbase_collection().clone();
        let staged_mask = staged_ledger.ledger();
        let needed_blocks = ordered_required_protocol_states(
            scan_state.required_state_hashes(),
            &needed_protocol_states,
        )
        .ok_or("a protocol state the root's scan state needs is missing")?;

        let diff_started = Instant::now();
        let staged = merkle_diff(&staged_mask, &root_snarked).map_err(|e| e.to_string())?;
        let staged_diff_ms = elapsed_ms(diff_started);

        // The ledgers to store, smallest first: ledgers only grow, so a diff
        // is only ever over a smaller one. Every diff is over a ledger stored
        // whole that this checkpoint keeps, so that a dropped ledger never
        // stays on disk as a base, and diff chains stay one deep.
        let stored = stored.into_iter().collect::<BTreeMap<_, _>>();
        let mut keep = ledgers
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|hash| {
                let mask = self.snarked_ledgers.get(&hash)?.clone();
                Some((mask.num_accounts(), hash, mask))
            })
            .collect::<Vec<_>>();
        if !keep
            .iter()
            .any(|(_, hash, _)| hash == &checkpoint.root_snarked_ledger)
        {
            keep.push((
                root_snarked.num_accounts(),
                checkpoint.root_snarked_ledger.clone(),
                root_snarked.clone(),
            ));
        }
        keep.sort_by(|(a, a_hash, _), (b, b_hash, _)| (a, a_hash).cmp(&(b, b_hash)));
        let kept = keep
            .iter()
            .map(|(_, hash, _)| hash.clone())
            .collect::<BTreeSet<_>>();
        let stored_full = |hash: &LedgerHash| matches!(stored.get(hash), Some(None));

        let diffs_started = Instant::now();
        let stream_stats = SharedStreamStats::default();
        let mut full_bases = Vec::new();
        let mut writes = Vec::new();
        let mut infos = Vec::new();
        // Made before the first pin, so that an error partway through the
        // loop releases the pins already taken; otherwise handed to the job.
        let mut unpin = Unpin {
            caller: caller.clone(),
            pins: Vec::new(),
        };
        for (_, hash, mask) in keep {
            let reuse = match stored.get(&hash) {
                Some(None) => true,
                Some(Some(base)) => kept.contains(base) && stored_full(base),
                None => false,
            };
            if reuse {
                if stored_full(&hash) {
                    full_bases.push(mask);
                }
                writes.push((hash.clone(), None));
                infos.push(PersistLedgerWriteInfo {
                    hash,
                    kind: PersistLedgerWriteKind::Stored,
                });
                continue;
            }
            // A ledger stored as a diff over a base this checkpoint drops (or
            // doesn't hold whole) is written whole: the store keeps a stored
            // ledger's file unless it is given the ledger whole, so a new diff
            // would be ignored and the old base would stay on disk.
            let write = match stored.get(&hash) {
                Some(Some(_)) => LedgerWrite::Full(mask.clone()),
                _ => choose_ledger_write(&mask, &full_bases),
            };
            match write {
                LedgerWrite::Diff(diff) => {
                    infos.push(PersistLedgerWriteInfo {
                        hash: hash.clone(),
                        kind: PersistLedgerWriteKind::Diff {
                            base: v2::LedgerHash::from_fp(diff.base),
                            accounts: diff.accounts.len() as u64,
                        },
                    });
                    writes.push((hash, Some(LedgerWrite::Diff(diff))));
                }
                LedgerWrite::Full(_) | LedgerWrite::Stream(_) => {
                    let (pin, header) = self.persist_pin(&mask).map_err(|e| e.to_string())?;
                    unpin.pins.push(pin);
                    let source = PinnedLedger {
                        caller: caller.clone(),
                        pin,
                        header,
                        part_sizes: self.persist.part_sizes,
                        stats: stream_stats.clone(),
                        #[cfg(test)]
                        after_read_part: self.persist.after_read_part.clone(),
                    };
                    infos.push(PersistLedgerWriteInfo {
                        hash: hash.clone(),
                        kind: PersistLedgerWriteKind::Full,
                    });
                    writes.push((hash, Some(LedgerWrite::Stream(Arc::new(source)))));
                    full_bases.push(mask);
                }
            }
        }
        let ledger_diffs_ms = elapsed_ms(diffs_started);

        let snapshot = PersistSnapshotInfo {
            ledgers: infos,
            staged_accounts: staged.accounts.len() as u64,
            staged_hashes: staged.hashes.len() as u64,
            staged_diff_ms,
            ledger_diffs_ms,
            total_ms: elapsed_ms(started),
        };
        let job = CheckpointJob {
            checkpoint,
            ledgers: writes,
            staged,
            parts: PartsSnapshot {
                scan_state,
                pending_coinbase,
                staged_ledger_hash: staged_mask_hash(&staged_mask),
                needed_blocks,
            },
            chain,
            snapshot: snapshot.clone(),
            stream_stats,
            on_done: Some(DoneGuard::new(move || drop(unpin))),
        };
        if !writer.send(PersistJob::Checkpoint(Box::new(job))) {
            return Err("the store thread has stopped".into());
        }
        Ok(snapshot)
    }
}

/// Releases its pins when dropped: by the store thread once the job is done,
/// or on the ledger thread if the snapshot fails after taking some. Through a
/// request, so that the store thread never touches the ledger thread's state,
/// and one that doesn't exit the process if the ledger thread is gone.
struct Unpin {
    caller: LedgerCaller,
    pins: Vec<u64>,
}

impl Drop for Unpin {
    fn drop(&mut self) {
        let pins = std::mem::take(&mut self.pins);
        if !pins.is_empty() {
            self.caller.try_call(LedgerRequest::PersistUnpin { pins });
        }
    }
}

fn staged_mask_hash(mask: &Mask) -> LedgerHash {
    let mut mask = mask.clone();
    merkle_root(&mut mask)
}

#[cfg(test)]
#[path = "ledger_persist_tests.rs"]
mod tests;
