//! The accounts that delegate to this node's block producer key in the
//! staking ledgers, as OCaml's consensus local state keeps them, for GraphQL's
//! `lastEpochDelegators`.

use std::{collections::BTreeMap, sync::Arc};

use ledger::{Account, AccountId};
use mina_core::block::ArcBlockWithHash;
use mina_p2p_messages::v2::LedgerHash;
use mina_signer::CompressedPubKey;

/// OCaml's delegatee tables (`Local_state` in `proof_of_stake.ml`).
///
/// OCaml keeps, for its block production keys only, the accounts delegating
/// to them in the staking epoch snapshot. When the frontier root crosses an
/// epoch (`frontier_root_transition`) that table becomes
/// `last_epoch_delegatee_table`, which is `None` until the root first does so
/// while the daemon runs, and after a change of block production keys
/// (`block_production_keys_swap`).
///
/// Here the tables come from the ledger scans the VRF evaluator already asks
/// for, one per staking ledger it evaluates an epoch with
/// ([`super::LedgerCtx::producer_delegator_table`]), so answering GraphQL
/// scans nothing; commits only follow the root's epoch. A node without a
/// block producer records no table.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L352-L405>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L3030-L3042>
#[derive(Default)]
pub(crate) struct EpochDelegatees {
    /// The key the tables are for.
    producer: Option<CompressedPubKey>,
    /// The producer's delegators in each staking ledger the VRF evaluator
    /// scanned and a commit still needs, in ledger order.
    tables: BTreeMap<LedgerHash, Arc<Vec<Account>>>,
    /// The root's epoch and staking ledger at the last commit.
    root: Option<(u32, LedgerHash)>,
    /// The staking ledger of the root's epoch before its current one: `None`
    /// until the root has crossed an epoch.
    last: Option<LedgerHash>,
}

impl EpochDelegatees {
    /// Records the producer's delegators in a staking ledger.
    pub(crate) fn record(
        &mut self,
        producer: &CompressedPubKey,
        ledger: LedgerHash,
        delegators: Vec<Account>,
    ) {
        if self.producer.as_ref() != Some(producer) {
            // OCaml's `block_production_keys_swap`: the tables are the new
            // key's, and there is no last epoch one.
            self.producer = Some(producer.clone());
            self.tables.clear();
            self.last = None;
        }
        self.tables.insert(ledger, Arc::new(delegators));
    }

    /// Follows the root of a commit, and keeps only the tables of the root's
    /// staking ledger, the last epoch's, and the best tip's staking and next
    /// epoch ledgers, which the VRF evaluator scans ahead of time.
    pub(crate) fn commit(&mut self, new_root: &ArcBlockWithHash, new_best_tip: &ArcBlockWithHash) {
        self.follow_root(
            new_root.consensus_state().epoch_count.as_u32(),
            new_root.staking_epoch_ledger_hash(),
            [
                new_best_tip.staking_epoch_ledger_hash(),
                new_best_tip.next_epoch_ledger_hash(),
            ],
        );
    }

    fn follow_root(&mut self, epoch: u32, staking: &LedgerHash, ahead: [&LedgerHash; 2]) {
        if let Some((root_epoch, root_staking)) = self.root.take() {
            if root_epoch != epoch {
                // `frontier_root_transition`: the staking snapshot's table
                // becomes the last epoch's.
                self.last = Some(root_staking);
            }
        }
        self.root = Some((epoch, staking.clone()));
        let last = self.last.as_ref();
        self.tables
            .retain(|hash, _| hash == staking || Some(hash) == last || ahead.contains(&hash));
    }

    /// OCaml's `Mina_lib.last_epoch_delegators`: `None` until the root has
    /// crossed an epoch; then the producer's delegators in the last epoch's
    /// staking ledger (`None` if that ledger was not scanned while the node
    /// ran), and nothing for any other key. Like OCaml, only the public key
    /// counts, not the token.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L1102-L1127>
    pub(crate) fn last_epoch_delegators(
        &self,
        account: &AccountId,
        limit: Option<usize>,
    ) -> Option<Vec<Account>> {
        let last = self.last.as_ref()?;
        if self.producer.as_ref() != Some(&account.public_key) {
            return Some(Vec::new());
        }
        // Public requests bound the clone before RPC transfer; private requests
        // retain OCaml's complete producer list.
        self.tables.get(last).map(|table| {
            table
                .iter()
                .take(limit.unwrap_or(usize::MAX))
                .cloned()
                .collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use ledger::{scan_state::currency::Balance, AccountId};
    use mina_curves::pasta::Fp;
    use mina_p2p_messages::v2::MinaBaseLedgerHash0StableV1;

    use super::*;

    fn ledger(n: u64) -> LedgerHash {
        MinaBaseLedgerHash0StableV1(Fp::from(n).into()).into()
    }

    /// Keys 0 to 15, the same within a test.
    fn keys() -> Vec<CompressedPubKey> {
        (0..16).map(|_| ledger::gen_compressed()).collect()
    }

    fn account(keys: &[CompressedPubKey], owner: usize, delegate: usize) -> Account {
        let id = AccountId::new_with_default_token(keys[owner].clone());
        let mut account = Account::create_with(id, Balance::from_u64(1_000_000_000));
        account.delegate = Some(keys[delegate].clone());
        account
    }

    /// OCaml's `last_epoch_delegatee_table` is `None` from the start until
    /// `frontier_root_transition` sees the root change epoch, then holds the
    /// staking snapshot's table, for block production keys only
    /// (`proof_of_stake.ml` L584, L3036-L3041; `mina_lib.ml` L1102-L1127).
    #[test]
    fn last_epoch_delegators_follow_the_root_across_epochs() {
        let keys = keys();
        let producer = keys[1].clone();
        let other = AccountId::new_with_default_token(keys[2].clone());
        let producer_id = AccountId::new_with_default_token(producer.clone());
        let mut tables = EpochDelegatees::default();

        // An observer node: no table, so nothing before and empty after.
        tables.follow_root(3, &ledger(3), [&ledger(3), &ledger(4)]);
        assert_eq!(tables.last_epoch_delegators(&producer_id, None), None);
        tables.follow_root(4, &ledger(4), [&ledger(4), &ledger(5)]);
        assert_eq!(
            tables.last_epoch_delegators(&producer_id, None),
            Some(vec![])
        );

        // A producer: the VRF evaluator scans the staking ledgers of epochs
        // 5 and 6 (the latter as epoch 5's next epoch ledger).
        let mut tables = EpochDelegatees::default();
        let in_5 = vec![account(&keys, 10, 1), account(&keys, 11, 1)];
        let in_6 = vec![account(&keys, 10, 1)];
        tables.follow_root(5, &ledger(5), [&ledger(5), &ledger(6)]);
        tables.record(&producer, ledger(5), in_5.clone());
        tables.record(&producer, ledger(6), in_6.clone());
        // Root still in epoch 5, best tip in 6.
        tables.follow_root(5, &ledger(5), [&ledger(6), &ledger(7)]);
        assert_eq!(tables.last_epoch_delegators(&producer_id, None), None);

        // The root enters epoch 6: the last epoch is 5.
        tables.follow_root(6, &ledger(6), [&ledger(6), &ledger(7)]);
        assert_eq!(tables.last_epoch_delegators(&producer_id, None), Some(in_5));
        assert_eq!(tables.last_epoch_delegators(&other, None), Some(vec![]));
        // Only the key counts, as OCaml's table is by public key.
        let token = AccountId::new(producer.clone(), ledger::TokenId::from(7u64));
        assert!(tables
            .last_epoch_delegators(&token, None)
            .is_some_and(|d| d.len() == 2));

        // The root enters epoch 7, whose staking ledger was never scanned:
        // the last epoch's table is epoch 6's.
        tables.follow_root(7, &ledger(7), [&ledger(7), &ledger(8)]);
        assert_eq!(tables.last_epoch_delegators(&producer_id, None), Some(in_6));

        // Epoch 8: epoch 7's ledger was not scanned, so it is unknown.
        tables.follow_root(8, &ledger(8), [&ledger(8), &ledger(9)]);
        assert_eq!(tables.last_epoch_delegators(&producer_id, None), None);
        assert_eq!(tables.last_epoch_delegators(&other, None), Some(vec![]));
    }

    #[test]
    fn last_epoch_delegators_bounds_the_rpc_clone() {
        let keys = keys();
        let producer_id = AccountId::new_with_default_token(keys[1].clone());
        let mut tables = EpochDelegatees::default();
        tables.follow_root(5, &ledger(5), [&ledger(5), &ledger(6)]);
        let entries: Vec<_> = (2..16).map(|i| account(&keys, i, 1)).collect();
        tables.record(&keys[1], ledger(5), entries.clone());
        tables.follow_root(6, &ledger(6), [&ledger(6), &ledger(7)]);
        // The security bound is eight, independently of the table's real size.
        assert_eq!(
            tables
                .last_epoch_delegators(&producer_id, Some(crate::rpc::MAX_LAST_EPOCH_DELEGATORS))
                .unwrap(),
            entries[..8]
        );
        assert_eq!(
            tables.last_epoch_delegators(&producer_id, None).unwrap(),
            entries
        );
        assert_eq!(tables.tables[&ledger(5)].len(), 14);
    }

    /// `block_production_keys_swap` recomputes the tables for the new keys
    /// and sets `last_epoch_delegatee_table` to `None`
    /// (`proof_of_stake.ml` L590-L620).
    #[test]
    fn a_new_producer_key_has_no_last_epoch_yet() {
        let keys = keys();
        let mut tables = EpochDelegatees::default();
        tables.follow_root(5, &ledger(5), [&ledger(5), &ledger(6)]);
        tables.record(&keys[1], ledger(5), vec![account(&keys, 10, 1)]);
        tables.follow_root(6, &ledger(6), [&ledger(6), &ledger(7)]);
        let new_key = AccountId::new_with_default_token(keys[2].clone());
        tables.record(&keys[2], ledger(6), vec![account(&keys, 11, 2)]);
        assert_eq!(tables.last_epoch_delegators(&new_key, None), None);
    }
}
