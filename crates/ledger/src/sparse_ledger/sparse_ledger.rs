use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
};

use ark_ff::Zero;
use mina_core::constants::constraint_constants;
use mina_curves::pasta::Fp;
use mina_p2p_messages::bigint::InvalidBigInt;

use crate::{
    scan_state::{
        conv::to_ledger_hash,
        currency::{Amount, Signed, Slot},
        transaction_logic::{
            local_state::{
                apply_zkapp_command_first_pass_aux, apply_zkapp_command_second_pass_aux,
                LocalStateEnv,
            },
            protocol_state::{GlobalState, ProtocolStateView},
            transaction_applied::ZkappCommandApplied,
            transaction_partially_applied::ZkappCommandPartiallyApplied,
            zkapp_command::ZkAppCommand,
            AccountState,
        },
    },
    Account, AccountId, AccountIndex, Address, HashesMatrix, Mask, MerklePath,
};

use super::{sparse_ledger_impl::SparseLedgerImpl, LedgerIntf};

/// A sparse ledger. Clones share it: a clone of a scan state (its
/// `Arc<TransactionWithWitness>` jobs hold the witnesses) is shallow.
///
/// Its lock is taken with a blocking `lock()`, never `try_lock()`: a scan
/// state and its clones can be read from two threads at once. The frontier
/// store's thread converts a checkpoint's clone of the root's scan state to
/// wire types while the ledger thread may convert the same jobs (a peer's
/// staged ledger request, a commit's available jobs) or copy them
/// (`copy_content`), and a `try_lock` would panic one of the two threads. No
/// path holds two of these locks at once except `eq`, which takes them in
/// address order, so blocking cannot deadlock.
#[derive(Clone, Debug)]
pub struct SparseLedger {
    // A mutex, not a `RefCell`: clones are used from more than one thread.
    inner: Arc<Mutex<SparseLedgerImpl<AccountId, Account>>>,
}

impl PartialEq for SparseLedger {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.inner, &other.inner) {
            return true;
        }

        // Both locks are held at once, so they are taken in address order:
        // two threads comparing the same two ledgers either way round can't
        // deadlock.
        let (this, other) = if Arc::as_ptr(&self.inner) < Arc::as_ptr(&other.inner) {
            let this = self.lock();
            (this, other.lock())
        } else {
            let other = other.lock();
            (self.lock(), other)
        };

        this.eq(&other)
    }
}

impl SparseLedger {
    /// Blocks while another thread holds the lock (see [`SparseLedger`]).
    fn lock(&self) -> MutexGuard<'_, SparseLedgerImpl<AccountId, Account>> {
        self.inner.lock().expect("sparse ledger lock poisoned")
    }

    fn with<F, R>(&self, fun: F) -> R
    where
        F: FnOnce(&mut SparseLedgerImpl<AccountId, Account>) -> R,
    {
        let mut inner = self.lock();
        fun(&mut inner)
    }
}

impl SparseLedger {
    pub fn create(depth: usize, root_hash: Fp) -> Self {
        let inner = SparseLedgerImpl::create(depth, root_hash);
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn of_ledger_subset_exn(oledger: Mask, keys: &[AccountId]) -> Self {
        let inner = SparseLedgerImpl::of_ledger_subset_exn(oledger, keys);
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn copy_content(&self) -> Self {
        let inner = self.with(|this| this.clone());
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn has_locked_tokens_exn(&self, global_slot: Slot, account_id: AccountId) -> bool {
        self.with(|this| this.has_locked_tokens_exn(global_slot, account_id))
    }

    pub fn iteri<F>(&self, fun: F)
    where
        F: Fn(Address, &Account),
    {
        self.with(|this| this.iteri(fun))
    }

    pub fn add_path(
        &mut self,
        merkle_path: &[MerklePath],
        account_id: AccountId,
        account: Account,
    ) {
        self.with(|this| this.add_path(merkle_path, account_id, account))
    }

    #[inline(never)]
    pub fn get_exn(&self, addr: &Address) -> Box<Account> {
        self.with(|this| Box::new(this.get_exn(addr).clone()))
    }

    pub fn set_exn(&mut self, addr: Address, value: Box<Account>) {
        self.with(|this| this.set_exn(addr, value))
    }

    pub fn find_index_exn(&self, key: AccountId) -> Address {
        self.with(|this| this.find_index_exn(key))
    }

    pub fn path_exn(&mut self, addr: Address) -> Vec<MerklePath> {
        self.with(|this| this.path_exn(addr))
    }

    pub fn merkle_root(&mut self) -> Fp {
        self.with(|this| this.merkle_root())
    }

    /// Read an account through its sparse-ledger index, including an empty
    /// account at a witnessed new-account location. Unlike `get_account`,
    /// absence of a witness entry is distinguishable from an empty account.
    pub fn get_account_if_present(&self, key: &AccountId) -> Option<Box<Account>> {
        self.with(|this| this.get_account_if_present(key))
    }

    pub fn get_account(&self, key: &AccountId) -> Box<Account> {
        let account = self.with(|this| {
            let addr = this.get_index(key)?;
            this.get(addr)
        });
        account.unwrap_or_else(|| Box::new(Account::empty()))
    }

    pub fn apply_zkapp_first_pass_unchecked_with_states(
        &mut self,
        states: &mut Vec<(GlobalState<SparseLedger>, LocalStateEnv<SparseLedger>)>,
        global_slot: Slot,
        state_view: &ProtocolStateView,
        fee_excess: Signed<Amount>,
        supply_increase: Signed<Amount>,
        second_pass_ledger: &Self,
        zkapp_command: &ZkAppCommand,
    ) -> Result<ZkappCommandPartiallyApplied<SparseLedger>, String> {
        apply_zkapp_command_first_pass_aux(
            constraint_constants(),
            global_slot,
            state_view,
            states,
            |acc, global_state, local_state| {
                let GlobalState {
                    first_pass_ledger,
                    second_pass_ledger: _,
                    fee_excess,
                    supply_increase,
                    protocol_state,
                    block_global_slot,
                } = global_state.clone();

                let mut local_state = local_state.clone();
                local_state.ledger = local_state.ledger.copy_content();

                acc.insert(
                    0,
                    (
                        GlobalState {
                            first_pass_ledger: first_pass_ledger.copy_content(),
                            second_pass_ledger: second_pass_ledger.copy_content(),
                            fee_excess,
                            supply_increase,
                            protocol_state,
                            block_global_slot,
                        },
                        local_state,
                    ),
                );
            },
            Some(fee_excess),
            Some(supply_increase),
            self,
            zkapp_command,
        )
    }

    pub fn apply_zkapp_second_pass_unchecked_with_states(
        &mut self,
        init: &mut Vec<(GlobalState<SparseLedger>, LocalStateEnv<SparseLedger>)>,
        c: ZkappCommandPartiallyApplied<Self>,
    ) -> Result<ZkappCommandApplied, String> {
        let account_update_applied = apply_zkapp_command_second_pass_aux(
            constraint_constants(),
            init,
            |acc, global_state, local_state| {
                let GlobalState {
                    first_pass_ledger,
                    second_pass_ledger,
                    fee_excess,
                    supply_increase,
                    protocol_state,
                    block_global_slot,
                } = global_state.clone();

                let mut local_state = local_state.clone();
                local_state.ledger = local_state.ledger.copy_content();

                acc.insert(
                    0,
                    (
                        GlobalState {
                            first_pass_ledger: first_pass_ledger.copy_content(),
                            second_pass_ledger: second_pass_ledger.copy_content(),
                            fee_excess,
                            supply_increase,
                            protocol_state,
                            block_global_slot,
                        },
                        local_state,
                    ),
                );
                // acc
            },
            self,
            c,
        )?;

        let rev_states = init;
        let will_succeed = account_update_applied.command.status.is_applied();

        rev_states.reverse();
        // All but first and last
        let nintermediate = rev_states.len().saturating_sub(2);

        for (_global_state, local_state) in rev_states.iter_mut().skip(1).take(nintermediate) {
            local_state.will_succeed = will_succeed;
        }

        // let states = rev_states;
        Ok(account_update_applied)
    }
}

impl LedgerIntf for SparseLedger {
    type Location = Address;

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L58>
    fn get(&self, addr: &Self::Location) -> Option<Box<Account>> {
        self.with(|this| this.get(addr))
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L66>
    fn location_of_account(&self, account_id: &AccountId) -> Option<Self::Location> {
        self.with(|this| this.location_of_account(account_id))
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L75>
    fn set(&mut self, addr: &Self::Location, account: Box<Account>) {
        self.with(|this| this.set(addr, account))
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L96>
    fn get_or_create(
        &mut self,
        account_id: &AccountId,
    ) -> Result<(AccountState, Box<Account>, Self::Location), String> {
        self.with(|this| this.get_or_create(account_id))
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L109>
    fn create_new_account(&mut self, account_id: AccountId, to_set: Account) -> Result<(), ()> {
        self.with(|this| this.create_new_account(account_id, to_set))
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L112>
    fn remove_accounts_exn(&mut self, _account_ids: &[AccountId]) {
        unimplemented!("remove_accounts_exn: not implemented")
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L115>
    fn merkle_root(&mut self) -> Fp {
        self.merkle_root()
    }

    /// <https://github.com/MinaProtocol/mina/blob/05c2f73d0f6e4f1341286843814ce02dcb3919e0/src/lib/mina_base/sparse_ledger_base.ml#L142>
    fn empty(depth: usize) -> Self {
        Self::create(depth, Fp::zero())
    }

    fn create_masked(&self) -> Self {
        self.copy_content()
    }

    fn apply_mask(&mut self, mask: Self) {
        let mask_inner = mask.with(|this| this.clone());
        self.with(|this| *this = mask_inner);
    }

    fn account_locations(&self) -> Vec<Self::Location> {
        self.with(|this| this.account_locations())
    }
}

impl From<&SparseLedger> for mina_p2p_messages::v2::MinaBaseSparseLedgerBaseStableV2 {
    fn from(value: &SparseLedger) -> Self {
        use mina_p2p_messages::v2::{
            MinaBaseAccountBinableArgStableV2, MinaBaseAccountIdStableV2,
            MinaBaseSparseLedgerBaseStableV2Tree,
        };

        // Blocking: the store thread converts a scan state's clone while the
        // ledger thread may convert the same witness (see `SparseLedger`).
        let value = value.lock();

        assert!(value.hashes_matrix.get(&Address::root()).is_some());

        let indexes: Vec<_> = value
            .indexes_list
            .iter()
            .map(|id| {
                let addr = value.indexes.get(id).unwrap();

                let index = addr.to_index();
                let index: mina_p2p_messages::number::UInt64 = index.as_u64().into();

                let id: MinaBaseAccountIdStableV2 = id.clone().into();

                (id, index)
            })
            .collect();

        fn build_tree(
            addr: Address,
            matrix: &HashesMatrix,
            ledger_depth: usize,
            values: &BTreeMap<AccountIndex, Account>,
        ) -> MinaBaseSparseLedgerBaseStableV2Tree {
            if addr.length() == ledger_depth {
                let account_index = addr.to_index();

                return match values.get(&account_index).cloned() {
                    Some(account) => {
                        let account: MinaBaseAccountBinableArgStableV2 = (&account).into();
                        MinaBaseSparseLedgerBaseStableV2Tree::Account(Box::new(account))
                    }
                    None => {
                        let hash = matrix.get(&addr).unwrap();
                        MinaBaseSparseLedgerBaseStableV2Tree::Hash(to_ledger_hash(hash))
                    }
                };
            }

            let child_left = addr.child_left();
            let child_right = addr.child_right();

            let is_level_above_accounts = addr.length() == ledger_depth - 1;

            let is_left = matrix.get(&child_left).is_some()
                || (is_level_above_accounts && values.get(&child_left.to_index()).is_some());
            let is_right = matrix.get(&child_right).is_some()
                || (is_level_above_accounts && values.get(&child_right.to_index()).is_some());

            if is_left && is_right {
                let hash = matrix.get(&addr).unwrap();
                let left_node = build_tree(child_left, matrix, ledger_depth, values);
                let right_node = build_tree(child_right, matrix, ledger_depth, values);

                MinaBaseSparseLedgerBaseStableV2Tree::Node(
                    to_ledger_hash(hash),
                    Box::new(left_node),
                    Box::new(right_node),
                )
            } else {
                assert!(!is_left && !is_right);
                let hash = matrix.get(&addr).unwrap();
                MinaBaseSparseLedgerBaseStableV2Tree::Hash(to_ledger_hash(hash))
            }
        }

        let tree = build_tree(
            Address::root(),
            &value.hashes_matrix,
            value.depth,
            &value.values,
        );

        let depth: u64 = value.depth.try_into().unwrap();

        Self {
            indexes: indexes.into_iter().collect(),
            depth: depth.into(),
            tree,
        }
    }
}

impl TryFrom<&mina_p2p_messages::v2::MinaBaseSparseLedgerBaseStableV2> for SparseLedger {
    type Error = InvalidBigInt;

    fn try_from(
        value: &mina_p2p_messages::v2::MinaBaseSparseLedgerBaseStableV2,
    ) -> Result<Self, Self::Error> {
        use mina_p2p_messages::v2::{
            MinaBaseSparseLedgerBaseStableV2Tree,
            MinaBaseSparseLedgerBaseStableV2Tree::{Account, Hash, Node},
        };

        fn build_matrix(
            matrix: &mut HashesMatrix,
            addr: Address,
            node: &MinaBaseSparseLedgerBaseStableV2Tree,
            values: &mut BTreeMap<AccountIndex, crate::Account>,
        ) -> Result<(), InvalidBigInt> {
            match node {
                Account(account) => {
                    let account: crate::Account = (&**account).try_into()?;
                    // matrix.set(&addr, account.hash()); Do not hash here, it's slow
                    values.insert(addr.to_index(), account);
                }
                Hash(hash) => {
                    matrix.set(&addr, hash.to_field()?);
                }
                Node(hash, left, right) => {
                    matrix.set(&addr, hash.to_field()?);
                    build_matrix(matrix, addr.child_left(), left, values)?;
                    build_matrix(matrix, addr.child_right(), right, values)?;
                }
            }
            Ok(())
        }

        let depth = value.depth.as_u64() as usize;
        let mut indexes = HashMap::with_capacity(value.indexes.len());
        let mut indexes_list = VecDeque::with_capacity(value.indexes.len());
        let mut hashes_matrix = HashesMatrix::new(depth);
        let mut values = BTreeMap::new();

        for (account_id, account_index) in value.indexes.iter() {
            let account_id: AccountId = account_id.try_into()?;
            let account_index = AccountIndex::from(account_index.as_u64() as usize);

            let addr = Address::from_index(account_index, depth);

            indexes.insert(account_id.clone(), addr);
            indexes_list.push_back(account_id);
        }

        build_matrix(
            &mut hashes_matrix,
            Address::root(),
            &value.tree,
            &mut values,
        )?;

        Ok(Self {
            inner: Arc::new(Mutex::new(SparseLedgerImpl {
                values,
                indexes,
                hashes_matrix,
                depth,
                indexes_list,
            })),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Barrier;

    use mina_p2p_messages::v2::MinaBaseSparseLedgerBaseStableV2;

    use super::*;
    use crate::{BaseLedger, Mask};

    fn sparse_ledger(num_accounts: usize) -> SparseLedger {
        let mut mask = Mask::new_unattached(constraint_constants().ledger_depth as usize);
        let ids = (0..num_accounts)
            .map(|_| {
                let account = Account::rand();
                let id = account.id();
                mask.get_or_create_account(id.clone(), account).unwrap();
                id
            })
            .collect::<Vec<_>>();
        let mut ledger = SparseLedger::of_ledger_subset_exn(mask, &ids);
        ledger.merkle_root();
        ledger
    }

    /// Clones share the lock, and two threads may use them at once: each
    /// waits its turn (with `try_lock`, one of them panicked). Comparing two
    /// ledgers takes both locks, in address order, so two threads comparing
    /// them either way round don't deadlock.
    #[test]
    fn clones_are_used_from_two_threads_at_once() {
        let ledger = sparse_ledger(50);
        let other = ledger.copy_content();
        let want = MinaBaseSparseLedgerBaseStableV2::from(&ledger);
        let start = &Barrier::new(2);
        for _ in 0..200 {
            let (clone, other_clone) = (ledger.clone(), other.clone());
            std::thread::scope(|scope| {
                let thread = scope.spawn(move || {
                    start.wait();
                    let converted = MinaBaseSparseLedgerBaseStableV2::from(&clone);
                    let equal = clone == other_clone;
                    (converted, equal, clone.copy_content().merkle_root())
                });
                start.wait();
                let converted = MinaBaseSparseLedgerBaseStableV2::from(&ledger);
                let equal = other == ledger;
                let root = ledger.copy_content().merkle_root();
                let (thread_converted, thread_equal, thread_root) = thread.join().unwrap();
                assert!(converted == want && thread_converted == want);
                assert!(equal && thread_equal);
                assert_eq!(root, thread_root);
            });
        }
    }
}
