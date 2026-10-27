//! Which accounts of an epoch ledger on disk delegate to which key (PC3a).
//!
//! The VRF evaluator's delegator table and GraphQL's `delegators` look for the
//! accounts that delegate to one key. In memory a scan of the ledger takes
//! about 50 ms on devnet; from the file it decodes every account, about
//! 300 ms. A frozen ledger doesn't change, so the slots of its delegating
//! accounts are indexed once, when it is frozen, by a keyed 64-bit hash of the
//! delegate: 12 bytes per delegating account. A lookup reads only the
//! accounts its hash finds, and the caller checks each one's delegate, so a
//! collision costs a read, never a wrong answer.

use std::{collections::hash_map::RandomState, hash::BuildHasher};

use ledger::{Account, AccountIndex, BaseLedger, Mask};
use mina_signer::CompressedPubKey;

pub(crate) struct DelegateIndex {
    hasher: RandomState,
    /// The delegates' hashes, sorted, and in `slots` each account's slot;
    /// for one hash the slots are in ledger order.
    hashes: Box<[u64]>,
    slots: Box<[u32]>,
    /// The slots the ledger's overlay held when the index was made: the index
    /// answers for the ledger only while that is still so (PC3c).
    pub(crate) overlay: usize,
}

impl DelegateIndex {
    /// The index of a root ledger's accounts; `None` for a mask that isn't a
    /// root, or a ledger too large for 32-bit slots.
    pub(crate) fn build(mask: &Mask) -> Option<Self> {
        let hasher = RandomState::new();
        let mut entries = Vec::new();
        let mut fits = true;
        let is_root = mask.root_iter_with_addr(|addr, account| {
            if let Some(delegate) = &account.delegate {
                match u32::try_from(addr.to_index().0) {
                    Ok(slot) => entries.push((key_hash(&hasher, delegate), slot)),
                    Err(_) => fits = false,
                }
            }
        });
        if !is_root || !fits {
            return None;
        }
        entries.sort_unstable();
        let (hashes, slots): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
        Some(Self {
            hasher,
            hashes: hashes.into_boxed_slice(),
            slots: slots.into_boxed_slice(),
            overlay: 0,
        })
    }

    /// The index of a ledger that is this one's with the accounts in
    /// `changed` slots replaced, each with its delegate or none (PC3c: an epoch
    /// ledger detached from the ledger it was a mask over). It costs a pass
    /// over the index, not a read of the ledger.
    pub(crate) fn derive(&self, changed: &[(u32, Option<CompressedPubKey>)]) -> Self {
        let replaced = changed
            .iter()
            .map(|(slot, _)| *slot)
            .collect::<std::collections::BTreeSet<_>>();
        let mut entries = self
            .hashes
            .iter()
            .zip(&*self.slots)
            .filter(|(_, slot)| !replaced.contains(slot))
            .map(|(hash, slot)| (*hash, *slot))
            .collect::<Vec<_>>();
        entries.extend(changed.iter().filter_map(|(slot, delegate)| {
            Some((key_hash(&self.hasher, delegate.as_ref()?), *slot))
        }));
        entries.sort_unstable();
        let (hashes, slots): (Vec<_>, Vec<_>) = entries.into_iter().unzip();
        Self {
            hasher: self.hasher.clone(),
            hashes: hashes.into_boxed_slice(),
            slots: slots.into_boxed_slice(),
            overlay: 0,
        }
    }

    pub(crate) fn heap_bytes(&self) -> usize {
        self.hashes.len() * 8 + self.slots.len() * 4
    }

    /// The accounts of `mask`, the ledger indexed, that may delegate to
    /// `delegate`, in ledger order, with their slots. The caller checks the
    /// delegate.
    pub(crate) fn candidates(
        &self,
        mask: &Mask,
        delegate: &CompressedPubKey,
    ) -> Vec<(AccountIndex, Box<Account>)> {
        let hash = key_hash(&self.hasher, delegate);
        let start = self.hashes.partition_point(|h| *h < hash);
        self.hashes[start..]
            .iter()
            .zip(&self.slots[start..])
            .take_while(|(h, _)| **h == hash)
            .filter_map(|(_, slot)| {
                let index = AccountIndex(*slot as u64);
                Some((index, mask.get_at_index(index)?))
            })
            .collect()
    }
}

fn key_hash(hasher: &RandomState, key: &CompressedPubKey) -> u64 {
    hasher.hash_one((&key.x, key.is_odd))
}
