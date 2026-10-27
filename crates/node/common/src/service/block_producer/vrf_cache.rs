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
    block_producer::vrf_evaluator::{VrfEvaluatorInput, SLOTS_PER_EPOCH},
};
use mina_signer::Keypair;
use mina_vrf::{VrfEvaluationOutput, VrfWonSlot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct CacheKey {
    producer: String,
    epoch: u32,
    seed: String,
    staking_ledger_hash: String,
    total_currency: u64,
}

impl CacheKey {
    fn new(keypair: &Keypair, input: &VrfEvaluatorInput) -> Self {
        let producer: AccountPublicKey = keypair.public.clone().into();
        Self {
            producer: producer.to_string(),
            epoch: input.global_slot / SLOTS_PER_EPOCH,
            seed: input.epoch_seed.to_string(),
            staking_ledger_hash: input.staking_ledger_hash.to_string(),
            total_currency: input.total_currency,
        }
    }
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
    pub fn open(dir: &Path, keypair: &Keypair, input: &VrfEvaluatorInput) -> Self {
        let key = CacheKey::new(keypair, input);
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

    pub fn matches(&self, keypair: &Keypair, input: &VrfEvaluatorInput) -> bool {
        self.saved.key == CacheKey::new(keypair, input)
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
        if slot
            != self
                .saved
                .last_slot
                .map_or(self.saved.first_slot, |last| last.saturating_add(1))
        {
            // A request outside the contiguous range is evaluated but does not
            // make an unevaluated gap appear as a run of lost slots.
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

    use super::VrfCache;

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
        let mut cache = VrfCache::open(dir.path(), &key, &input);
        assert!(cache.get(7140).is_none());
        cache.record(&VrfEvaluationOutput::SlotLost(7140)).unwrap();
        cache.record(&VrfEvaluationOutput::SlotLost(7141)).unwrap();
        let path = cache.path.clone();
        drop(cache);

        let cache = VrfCache::open(dir.path(), &key, &input);
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
        assert!(VrfCache::open(dir.path(), &key, &input).get(7140).is_none());

        std::fs::write(&path, b"{partial").unwrap();
        assert!(VrfCache::open(dir.path(), &key, &input).get(7140).is_none());
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
        let mut cache = VrfCache::open(dir.path(), &key, &input);
        cache.record(&VrfEvaluationOutput::SlotLost(7140)).unwrap();
        let changed = VrfEvaluatorInput {
            total_currency: 43,
            ..input.clone()
        };
        assert!(VrfCache::open(dir.path(), &key, &changed)
            .get(7140)
            .is_none());
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
        let mut cache = VrfCache::open(dir.path(), &key, &input);
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
        let reopened = VrfCache::open(dir.path(), &key, &input);
        assert_eq!(reopened.get(slot), Some(result));
    }
}
