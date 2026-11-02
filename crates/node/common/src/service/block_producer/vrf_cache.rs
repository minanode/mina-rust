//! Public VRF results and progress. The file discloses future winning slots.

use std::{
    collections::BTreeMap,
    fs::{self, DirBuilder, OpenOptions},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use mina_node::{
    account::AccountPublicKey,
    block_producer::vrf_evaluator::{DelegatorTable, VrfEvaluatorInput, SLOTS_PER_EPOCH},
};
use mina_signer::Keypair;
use mina_vrf::{VrfEvaluationOutput, VrfWonSlot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Versions 1 and 2 may contain losses evaluated with an empty table where the
// staking ledger was unavailable. Version 3 also keys results on the table
// they were evaluated with, so they are recomputed instead of trusted.
const VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CacheKey {
    producer: String,
    epoch: u32,
    seed: String,
    staking_ledger_hash: String,
    total_currency: u64,
    /// SHA-256 of the delegator table ([`table_digest`]).
    delegator_table: String,
}

impl CacheKey {
    fn new(keypair: &Keypair, input: &VrfEvaluatorInput, table_digest: &str) -> Self {
        let producer: AccountPublicKey = keypair.public.clone().into();
        Self {
            producer: producer.to_string(),
            epoch: input.global_slot / SLOTS_PER_EPOCH,
            seed: input.epoch_seed.to_string(),
            staking_ledger_hash: input.staking_ledger_hash.to_string(),
            total_currency: input.total_currency,
            delegator_table: table_digest.to_owned(),
        }
    }
}

/// A digest of every delegator's index, key and stake, in ledger order.
pub(super) fn table_digest(table: &DelegatorTable) -> String {
    let mut hasher = Sha256::new();
    hasher.update((table.len() as u64).to_le_bytes());
    for (index, (public_key, stake)) in table {
        let key = public_key.to_string();
        hasher.update(index.0.to_le_bytes());
        hasher.update((key.len() as u64).to_le_bytes());
        hasher.update(key.as_bytes());
        hasher.update(stake.to_le_bytes());
    }
    format!("{:x}", hasher.finalize())
}

#[derive(Serialize, Deserialize)]
struct SavedCache {
    version: u32,
    key: CacheKey,
    first_slot: u32,
    last_slot: Option<u32>,
    wins: BTreeMap<u32, VrfWonSlot>,
    checksum: String,
}

impl SavedCache {
    fn checksum(&self) -> io::Result<String> {
        let bytes = serde_json::to_vec(&(
            self.version,
            &self.key,
            self.first_slot,
            self.last_slot,
            &self.wins,
        ))
        .map_err(io::Error::other)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}

/// One ledger/seed/epoch at a time; a key change opens another file.
pub(super) struct VrfCache {
    path: PathBuf,
    saved: SavedCache,
}

impl VrfCache {
    pub fn open(
        dir: &Path,
        keypair: &Keypair,
        input: &VrfEvaluatorInput,
        table_digest: &str,
    ) -> Self {
        let key = CacheKey::new(keypair, input, table_digest);
        let path = dir
            .join("vrf-evaluation")
            .join(format!("{}-{}.json", key.producer, key.epoch));
        let saved = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SavedCache>(&bytes).ok())
            .filter(|saved| {
                saved.version == VERSION
                    && saved.key == key
                    && saved
                        .checksum()
                        .is_ok_and(|checksum| checksum == saved.checksum)
                    && saved.first_slot / SLOTS_PER_EPOCH == key.epoch
                    && saved.last_slot.is_none_or(|last| {
                        last >= saved.first_slot && last / SLOTS_PER_EPOCH == key.epoch
                    })
                    && saved.wins.iter().all(|(slot, won)| {
                        (*slot >= saved.first_slot)
                            && saved.last_slot.is_some_and(|last| *slot <= last)
                            && won.global_slot == *slot
                    })
            })
            .unwrap_or_else(|| SavedCache {
                version: VERSION,
                key,
                first_slot: input.global_slot,
                last_slot: None,
                wins: BTreeMap::new(),
                checksum: String::new(),
            });
        Self { path, saved }
    }

    pub fn matches(
        &self,
        keypair: &Keypair,
        input: &VrfEvaluatorInput,
        table_digest: &str,
    ) -> bool {
        self.saved.key == CacheKey::new(keypair, input, table_digest)
    }

    pub fn get(&self, slot: u32) -> Option<VrfEvaluationOutput> {
        if slot < self.saved.first_slot || self.saved.last_slot.is_none_or(|last| slot > last) {
            return None;
        }
        Some(match self.saved.wins.get(&slot) {
            Some(won) => VrfEvaluationOutput::SlotWon(won.clone()),
            None => VrfEvaluationOutput::SlotLost(slot),
        })
    }

    pub fn record(&mut self, result: &VrfEvaluationOutput) -> io::Result<()> {
        let slot = result.global_slot();
        let next = self
            .saved
            .last_slot
            .map_or(self.saved.first_slot, |last| last.saturating_add(1));
        if slot > next {
            // Evaluation resumed past the saved range, as after a restart
            // later in the epoch: start a new range here. The unevaluated gap
            // is never recorded as lost slots, and the earlier range, all in
            // the past, is dropped.
            self.saved.first_slot = slot;
            self.saved.last_slot = None;
            self.saved.wins.clear();
        } else if slot != next {
            // Already in the range, or before it.
            return Ok(());
        }
        if let VrfEvaluationOutput::SlotWon(won) = result {
            self.saved.wins.insert(slot, won.clone());
        }
        self.saved.last_slot = Some(slot);
        self.saved.checksum = self.saved.checksum()?;
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let dir = self.path.parent().expect("cache path has a parent");
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(&self.saved).map_err(io::Error::other)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(tmp, &self.path)?;
        fs::File::open(dir)?.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt, sync::Arc};

    use ledger::AccountIndex;
    use mina_node::block_producer::vrf_evaluator::{DelegatorTable, VrfEvaluatorInput};
    use mina_p2p_messages::v2::{EpochSeed, LedgerHash};
    use mina_signer::Keypair;
    use mina_vrf::VrfEvaluationOutput;

    use super::{table_digest, VrfCache};

    fn open(dir: &std::path::Path, key: &Keypair, input: &VrfEvaluatorInput) -> VrfCache {
        VrfCache::open(dir, key, input, &table_digest(&input.delegator_table))
    }

    /// An empty-table loss, as the producer used to evaluate an unavailable
    /// staking ledger, cannot suppress the real table's win after a restart:
    /// legacy files are ignored whatever their checksum, and a current file
    /// is keyed on the table its results were evaluated with.
    #[test]
    fn legacy_empty_table_loss_cannot_suppress_a_win_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        // Public synthetic scalar 1, never an operator key. Compiled OCaml
        // 4.0.3 wins at slot 7140, seed 0, delegator 0, stake/total 42/42;
        // tools/consensus-diff/regressions.json retains its exact output.
        let key =
            Keypair::from_secret_key(mina_signer::SecKey::new(mina_curves::pasta::Fq::from(1u64)))
                .unwrap();
        let input = VrfEvaluatorInput::new(
            EpochSeed::zero(),
            Arc::new(DelegatorTable::new()),
            7140,
            42,
            LedgerHash::zero(),
        );
        let lost = super::super::vrf_evaluator::evaluate_slot(&key, &input);
        assert_eq!(lost, VrfEvaluationOutput::SlotLost(7140));
        let producer = mina_node::account::AccountPublicKey::from(key.public.clone());
        let table = [(AccountIndex(0), (producer, 42))].into_iter().collect();
        let populated = VrfEvaluatorInput {
            delegator_table: Arc::new(table),
            ..input.clone()
        };
        let uncached = super::super::vrf_evaluator::evaluate_slot(&key, &populated);
        assert!(matches!(uncached, VrfEvaluationOutput::SlotWon(_)));

        // Version 1 (withdrawn early path) and version 2 (unavailable
        // ledger evaluated as empty) files, checksum-valid, with the old key
        // whose table digest matches the populated table's.
        for version in [1, 2] {
            let mut old = open(dir.path(), &key, &populated);
            old.saved.first_slot = 7140;
            old.saved.last_slot = None;
            old.saved.wins.clear();
            old.record(&lost).unwrap();
            old.saved.version = version;
            old.saved.checksum = old.saved.checksum().unwrap();
            old.save().unwrap();
            drop(old);
            assert!(
                open(dir.path(), &key, &populated).get(7140).is_none(),
                "version {version} progress must be reevaluated"
            );
        }

        // Current format: a loss recorded with the empty table.
        let mut empty = open(dir.path(), &key, &input);
        empty.saved.first_slot = 7140;
        empty.saved.last_slot = None;
        empty.saved.wins.clear();
        empty.record(&lost).unwrap();
        drop(empty);
        assert_eq!(open(dir.path(), &key, &input).get(7140), Some(lost));
        let mut reopened = open(dir.path(), &key, &populated);
        assert!(
            reopened.get(7140).is_none(),
            "an empty table's loss must not answer for the real table"
        );
        let actual = reopened
            .get(7140)
            .unwrap_or_else(|| super::super::vrf_evaluator::evaluate_slot(&key, &populated));
        assert!(matches!(actual, VrfEvaluationOutput::SlotWon(_)));
        reopened.record(&actual).unwrap();
        drop(reopened);
        assert_eq!(open(dir.path(), &key, &populated).get(7140), Some(actual));
    }

    #[test]
    fn restart_reuses_progress_and_corruption_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let key = Keypair::rand(&mut rand::thread_rng()).unwrap();
        let input = VrfEvaluatorInput::new(
            EpochSeed::zero(),
            Arc::new(DelegatorTable::new()),
            7140,
            42,
            LedgerHash::zero(),
        );
        let mut cache = open(dir.path(), &key, &input);
        assert!(cache.get(7140).is_none());
        cache.record(&VrfEvaluationOutput::SlotLost(7140)).unwrap();
        cache.record(&VrfEvaluationOutput::SlotLost(7141)).unwrap();
        let path = cache.path.clone();
        drop(cache);

        let cache = open(dir.path(), &key, &input);
        assert_eq!(cache.get(7140), Some(VrfEvaluationOutput::SlotLost(7140)));
        assert_eq!(cache.get(7141), Some(VrfEvaluationOutput::SlotLost(7141)));
        assert!(cache.get(7142).is_none());
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(cache);

        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            text.replace("\"last_slot\":7141", "\"last_slot\":7140"),
        )
        .unwrap();
        assert!(open(dir.path(), &key, &input).get(7140).is_none());

        std::fs::write(&path, b"{partial").unwrap();
        assert!(open(dir.path(), &key, &input).get(7140).is_none());
    }

    /// #242 review P2: a restart past the saved range used to stop saving
    /// for the key. It starts a new range instead, and the gap stays unknown.
    #[test]
    fn a_restart_past_the_saved_range_starts_a_new_range() {
        let dir = tempfile::tempdir().unwrap();
        let key = Keypair::rand(&mut rand::thread_rng()).unwrap();
        let input = VrfEvaluatorInput::new(
            EpochSeed::zero(),
            Arc::new(DelegatorTable::new()),
            7140,
            42,
            LedgerHash::zero(),
        );
        let mut cache = open(dir.path(), &key, &input);
        cache.record(&VrfEvaluationOutput::SlotLost(7140)).unwrap();
        cache.record(&VrfEvaluationOutput::SlotLost(7141)).unwrap();
        drop(cache);
        let mut cache = open(dir.path(), &key, &input);
        assert_eq!(cache.get(7141), Some(VrfEvaluationOutput::SlotLost(7141)));
        assert!(cache.get(7150).is_none());
        cache.record(&VrfEvaluationOutput::SlotLost(7150)).unwrap();
        cache.record(&VrfEvaluationOutput::SlotLost(7151)).unwrap();
        // An earlier slot does not reopen the dropped range.
        cache.record(&VrfEvaluationOutput::SlotLost(7142)).unwrap();
        drop(cache);
        let cache = open(dir.path(), &key, &input);
        assert_eq!(cache.get(7150), Some(VrfEvaluationOutput::SlotLost(7150)));
        assert_eq!(cache.get(7151), Some(VrfEvaluationOutput::SlotLost(7151)));
        for unknown in [7140, 7142, 7145, 7149, 7152] {
            assert!(cache.get(unknown).is_none(), "slot {unknown}");
        }
    }

    #[test]
    fn changed_key_does_not_reuse_results() {
        let dir = tempfile::tempdir().unwrap();
        let key = Keypair::rand(&mut rand::thread_rng()).unwrap();
        let input = VrfEvaluatorInput::new(
            EpochSeed::zero(),
            Arc::new(DelegatorTable::new()),
            7140,
            42,
            LedgerHash::zero(),
        );
        let mut cache = open(dir.path(), &key, &input);
        cache.record(&VrfEvaluationOutput::SlotLost(7140)).unwrap();
        let changed = VrfEvaluatorInput {
            total_currency: 43,
            ..input.clone()
        };
        assert!(open(dir.path(), &key, &changed).get(7140).is_none());
    }

    #[test]
    fn won_slot_round_trips_with_output() {
        let dir = tempfile::tempdir().unwrap();
        let key = Keypair::rand(&mut rand::thread_rng()).unwrap();
        let mut table = DelegatorTable::new();
        let producer = mina_node::account::AccountPublicKey::from(key.public.clone());
        table.insert(AccountIndex(3), (producer, 1_000_000));
        let mut input = VrfEvaluatorInput::new(
            EpochSeed::zero(),
            Arc::new(table),
            7140,
            1_000_000,
            LedgerHash::zero(),
        );
        let mut cache = open(dir.path(), &key, &input);
        let mut winner = None;
        for slot in 7140..7160 {
            input.global_slot = slot;
            let result = super::super::vrf_evaluator::evaluate_slot(&key, &input);
            cache.record(&result).unwrap();
            if matches!(result, VrfEvaluationOutput::SlotWon(_)) {
                winner = Some((slot, result));
                break;
            }
        }
        let (slot, result) = winner.expect("full-stake delegator should win a slot");
        let reopened = open(dir.path(), &key, &input);
        assert_eq!(reopened.get(slot), Some(result));
    }
}
