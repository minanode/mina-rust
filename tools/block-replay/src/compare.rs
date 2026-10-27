//! Compare precomputed blocks with the public bucket's, field by field.
//!
//! The bucket's blocks are written by OCaml nodes, so they are the oracle for
//! the JSON we write: [`round_trip`] reads one and writes it back, and
//! [`Replay`] applies blocks as the node does and builds the precomputed block
//! the node's archive service would write for each.

use std::{collections::BTreeMap, path::Path, sync::Arc};

use anyhow::{bail, Context};
use ledger::{
    proofs::block::ProtocolState,
    scan_state::{currency::Slot, transaction_logic::protocol_state::protocol_state_view},
    staged_ledger::{
        diff::Diff,
        staged_ledger::{SkipVerification, StagedLedger},
    },
    verifier::Verifier,
};
use mina_node::{
    core::block::ArcBlockWithHash,
    ledger::write::{BlockApplyResult, BlockApplyResultArchive, LedgerHashes},
    transition_frontier::genesis::PrebuiltGenesisConfig,
};
use mina_p2p_messages::v2::{
    self, precomputed, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2,
    PrecomputedBlock, PrecomputedBlockData,
};
use serde_json::Value;

/// A field whose value differs.
#[derive(Debug, Clone)]
pub struct Difference {
    /// Where, as `.data.staged_ledger_diff.diff[0]`.
    pub path: String,
    /// The bucket's value.
    pub expected: String,
    /// Ours.
    pub found: String,
}

impl Difference {
    /// The path without array indices, to count differences of a kind.
    pub fn kind(&self) -> String {
        let mut kind = String::with_capacity(self.path.len());
        let mut in_index = false;
        for c in self.path.chars() {
            match c {
                '[' => {
                    in_index = true;
                    kind.push_str("[]");
                }
                ']' => in_index = false,
                _ if in_index => {}
                c => kind.push(c),
            }
        }
        kind
    }
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {} -> {}", self.path, self.expected, self.found)
    }
}

fn short(value: &Value) -> String {
    let s = value.to_string();
    match s.char_indices().nth(100) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s,
    }
}

/// Where `found` differs from `expected`, field by field.
pub fn diff(expected: &Value, found: &Value) -> Vec<Difference> {
    fn go(expected: &Value, found: &Value, path: &mut String, out: &mut Vec<Difference>) {
        let len = path.len();
        match (expected, found) {
            (Value::Object(x), Value::Object(y)) => {
                for (key, value) in x {
                    path.push('.');
                    path.push_str(key);
                    match y.get(key) {
                        Some(other) => go(value, other, path, out),
                        None => out.push(Difference {
                            path: path.clone(),
                            expected: short(value),
                            found: "(missing)".to_owned(),
                        }),
                    }
                    path.truncate(len);
                }
                for (key, value) in y.iter().filter(|(key, _)| !x.contains_key(*key)) {
                    out.push(Difference {
                        path: format!("{path}.{key}"),
                        expected: "(missing)".to_owned(),
                        found: short(value),
                    });
                }
            }
            (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
                for (i, (value, other)) in x.iter().zip(y).enumerate() {
                    path.push_str(&format!("[{i}]"));
                    go(value, other, path, out);
                    path.truncate(len);
                }
            }
            _ if expected != found => out.push(Difference {
                path: path.clone(),
                expected: short(expected),
                found: short(found),
            }),
            _ => {}
        }
    }
    let mut out = Vec::new();
    go(expected, found, &mut String::new(), &mut out);
    out
}

/// Where `found`, a precomputed block's JSON as we write it, differs from
/// `expected`, the bucket's: nothing if the bytes are equal. Values that are
/// equal but written differently are one difference at `(bytes)`.
pub fn compare(expected: &[u8], found: &[u8]) -> anyhow::Result<Vec<Difference>> {
    if expected == found {
        return Ok(Vec::new());
    }
    let differences = diff(
        &precomputed::ocaml_json_to_value(expected)?,
        &precomputed::ocaml_json_to_value(found)?,
    );
    if !differences.is_empty() {
        return Ok(differences);
    }
    let at = expected
        .iter()
        .zip(found)
        .position(|(a, b)| a != b)
        .unwrap_or(expected.len().min(found.len()));
    let around = |bytes: &[u8]| {
        String::from_utf8_lossy(&bytes[at.saturating_sub(30)..(at + 30).min(bytes.len())])
            .into_owned()
    };
    Ok(vec![Difference {
        path: "(bytes)".to_owned(),
        expected: format!("at byte {at}: {}", around(expected)),
        found: around(found),
    }])
}

/// Reads a precomputed block as OCaml writes it, writes it back, and returns
/// where the result differs from the original.
pub fn round_trip(bytes: &[u8]) -> anyhow::Result<Vec<Difference>> {
    let block = read(bytes)?;
    compare(bytes, &block.to_ocaml_json(&LedgerHashes)?)
}

/// [`PrecomputedBlockData::from_ocaml_json`], saying where it failed.
pub fn read(bytes: &[u8]) -> anyhow::Result<PrecomputedBlockData> {
    PrecomputedBlockData::from_ocaml_json(bytes).or_else(|e| {
        let mut value = precomputed::ocaml_json_to_value(bytes)?;
        precomputed::ocaml_forms_to_ours(&mut value)?;
        match serde_path_to_error::deserialize::<_, PrecomputedBlockData>(&value) {
            Err(e) => bail!("at `{}`: {}", e.path(), e.inner()),
            Ok(_) => Err(e.into()),
        }
    })
}

/// Differences over many blocks, by kind.
#[derive(Default)]
pub struct Summary {
    pub blocks: usize,
    /// Blocks written back byte for byte.
    pub identical: usize,
    /// Blocks that failed to read or apply, with the error.
    pub errors: Vec<(String, String)>,
    /// Per kind of difference: how many, in how many blocks, and the first.
    pub kinds: BTreeMap<String, (usize, usize, String, Difference)>,
}

impl Summary {
    pub fn add(&mut self, block: &str, result: anyhow::Result<Vec<Difference>>) {
        self.blocks += 1;
        let differences = match result {
            Ok(differences) => differences,
            Err(e) => {
                self.errors.push((block.to_owned(), format!("{e:#}")));
                return;
            }
        };
        if differences.is_empty() {
            self.identical += 1;
        }
        let mut seen = std::collections::BTreeSet::new();
        for difference in differences {
            let kind = difference.kind();
            let entry = self
                .kinds
                .entry(kind.clone())
                .or_insert_with(|| (0, 0, block.to_owned(), difference));
            entry.0 += 1;
            if seen.insert(kind) {
                entry.1 += 1;
            }
        }
    }

    pub fn merge(&mut self, other: Summary) {
        self.blocks += other.blocks;
        self.identical += other.identical;
        self.errors.extend(other.errors);
        for (kind, (count, blocks, first_block, first)) in other.kinds {
            let entry = self.kinds.entry(kind).or_insert((0, 0, first_block, first));
            entry.0 += count;
            entry.1 += blocks;
        }
    }

    pub fn is_clean(&self) -> bool {
        self.errors.is_empty() && self.identical == self.blocks
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{} blocks: {} identical, {} with differences, {} errors",
            self.blocks,
            self.identical,
            self.blocks - self.identical - self.errors.len(),
            self.errors.len()
        )?;
        for (kind, (count, blocks, first_block, first)) in &self.kinds {
            writeln!(
                f,
                "  {kind}: {count} in {blocks} blocks, first in {first_block}"
            )?;
            writeln!(f, "    {first}")?;
        }
        for (block, error) in self.errors.iter().take(20) {
            writeln!(f, "  error in {block}: {error}")?;
        }
        Ok(())
    }
}

/// Applies Mesa blocks from genesis as the node does, and builds what the
/// node's archive service writes for each.
pub struct Replay {
    staged_ledger: StagedLedger,
    pred_state: MinaStateProtocolStateValueStableV2,
    pred_hash: v2::StateHash,
}

impl Replay {
    /// Starts from a prebuilt genesis config, as `ledger-tool` writes it.
    /// `mina_core::NetworkConfig` must be initialised first.
    pub fn from_genesis(genesis: &Path) -> anyhow::Result<Self> {
        let (mut masks, loaded) = PrebuiltGenesisConfig::load_from_bytes(
            &std::fs::read(genesis).with_context(|| format!("{}", genesis.display()))?,
        )?;
        let (_, pred_state, pred_hash) = loaded.protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )?;
        let staged_ledger = StagedLedger::create_exn(
            mina_core::constants::constraint_constants().clone(),
            masks.remove(0),
        )
        .map_err(anyhow::Error::msg)?;
        Ok(Self {
            staged_ledger,
            pred_state,
            pred_hash,
        })
    }

    /// Applies the next block, checking its parent and staged ledger hash, and
    /// returns the precomputed block the node writes for it, with
    /// `scheduled_time`, which is when the node writes it, set to the given
    /// block's.
    pub fn apply(&mut self, block: &PrecomputedBlock) -> anyhow::Result<PrecomputedBlock> {
        let constraint_constants = mina_core::constants::constraint_constants();
        let state = &block.protocol_state;
        if state.previous_state_hash != self.pred_hash {
            bail!(
                "parent {} is not the previous block {}",
                state.previous_state_hash,
                self.pred_hash
            );
        }
        let block_with_hash = ArcBlockWithHash::try_new(Arc::new(block.block()))?;

        let pred_view = protocol_state_view(&self.pred_state)?;
        let pred: ProtocolState = (&self.pred_state).try_into()?;
        let consensus = &state.body.consensus_state;
        let diff: Diff = (&block.staged_ledger_diff).try_into()?;
        let result = self
            .staged_ledger
            .apply(
                Some(SkipVerification::All),
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
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let found = MinaBaseStagedLedgerHashStableV1::from(&result.hash_after_applying);
        if found != state.body.blockchain_state.staged_ledger_hash {
            bail!("staged ledger hash mismatch: found {found:?}");
        }

        let archive_data = BlockApplyResultArchive::new(
            &block_with_hash,
            &self.staged_ledger,
            &result.accounts_created,
        )
        .map_err(anyhow::Error::msg)?;
        let applied = BlockApplyResult {
            block: block_with_hash,
            just_emitted_a_proof: result.ledger_proof.is_some(),
            archive_data: Some(archive_data),
        };
        let precomputed = applied
            .precomputed_block(block.scheduled_time)
            .map_err(anyhow::Error::msg)?;

        self.staged_ledger.commit_and_reparent_to_root();
        self.pred_state = state.clone();
        self.pred_hash = applied.block.hash().clone();
        Ok(precomputed)
    }
}
