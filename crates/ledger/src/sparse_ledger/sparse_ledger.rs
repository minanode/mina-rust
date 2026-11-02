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

        // The shape is checked as the tree is converted: an account only at
        // a leaf, a node only above one, and indexes within the tree. The
        // conversion back to the wire type relies on it. Every sparse ledger
        // OCaml builds has this shape.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/sparse_ledger_lib/sparse_ledger.ml#L46-L96>
        fn build_matrix(
            matrix: &mut HashesMatrix,
            addr: Address,
            depth: usize,
            node: &MinaBaseSparseLedgerBaseStableV2Tree,
            values: &mut BTreeMap<AccountIndex, crate::Account>,
        ) -> Result<(), InvalidBigInt> {
            match node {
                Account(account) => {
                    if addr.length() != depth {
                        return Err(InvalidBigInt);
                    }
                    let account: crate::Account = (&**account).try_into()?;
                    // matrix.set(&addr, account.hash()); Do not hash here, it's slow
                    values.insert(addr.to_index(), account);
                }
                Hash(hash) => {
                    matrix.set(&addr, hash.to_field()?);
                }
                Node(hash, left, right) => {
                    if addr.length() >= depth {
                        return Err(InvalidBigInt);
                    }
                    matrix.set(&addr, hash.to_field()?);
                    build_matrix(matrix, addr.child_left(), depth, left, values)?;
                    build_matrix(matrix, addr.child_right(), depth, right, values)?;
                }
            }
            Ok(())
        }

        let depth = usize::try_from(value.depth.as_u64()).map_err(|_| InvalidBigInt)?;
        if depth > Address::MAX_LENGTH {
            return Err(InvalidBigInt);
        }
        let mut indexes = HashMap::with_capacity(value.indexes.len());
        let mut indexes_list = VecDeque::with_capacity(value.indexes.len());
        let mut hashes_matrix = HashesMatrix::new(depth);
        let mut values = BTreeMap::new();

        for (account_id, account_index) in value.indexes.iter() {
            let account_id: AccountId = account_id.try_into()?;
            // `u64`, not `usize`: on a 32-bit target (the web node) a cast
            // would drop bits of an index from 2^32 up.
            let account_index = account_index.as_u64();
            if account_index >> depth != 0 {
                return Err(InvalidBigInt);
            }
            let account_index = AccountIndex(account_index);

            let addr = Address::from_index(account_index, depth);

            indexes.insert(account_id.clone(), addr);
            indexes_list.push_back(account_id);
        }

        build_matrix(
            &mut hashes_matrix,
            Address::root(),
            depth,
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

    use mina_p2p_messages::v2::{
        MinaBaseSparseLedgerBaseStableV2, MinaBaseSparseLedgerBaseStableV2Tree as Tree,
    };

    use super::*;
    use crate::{BaseLedger, Mask, TreeVersion, V2};

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

    fn wire(ledger: &SparseLedger) -> MinaBaseSparseLedgerBaseStableV2 {
        MinaBaseSparseLedgerBaseStableV2::from(ledger)
    }

    /// The root recomputed from a leaf and its Merkle path, as the
    /// transaction SNARK does, without the sparse ledger's cached hashes.
    fn root_from_path(leaf: Fp, path: &[MerklePath]) -> Fp {
        path.iter()
            .enumerate()
            .fold(leaf, |hash, (height, step)| match step {
                MerklePath::Left(right) => V2::hash_node(height, hash, *right),
                MerklePath::Right(left) => V2::hash_node(height, *left, hash),
            })
    }

    /// A subtree the sparse ledger only knows by its hash, as close to the
    /// root as possible: a sibling on the path of the leftmost accounts.
    fn hidden_subtree(tree: &mut Tree) -> Option<&mut mina_p2p_messages::v2::LedgerHash> {
        let Tree::Node(_, left, right) = tree else {
            return None;
        };
        match (&mut **left, &mut **right) {
            (_, Tree::Hash(hash)) | (Tree::Hash(hash), _) => Some(hash),
            (left, right) => hidden_subtree(left).or_else(|| hidden_subtree(right)),
        }
    }

    #[test]
    fn a_wire_ledger_converts_back_to_the_same_bytes() {
        let ledger = sparse_ledger(20);
        let want = wire(&ledger);
        let back = SparseLedger::try_from(&want).unwrap();
        assert!(wire(&back) == want);
    }

    /// A sparse ledger's root is its cached root hash, as in OCaml
    /// (`Sparse_ledger.merkle_root` returns the top node's hash): a subtree
    /// whose hash was replaced keeps the root. Such a ledger encodes to other
    /// bytes, so a scan state holding it has another hash, and the root
    /// recomputed from an account's path differs, as the SNARK computes it.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/sparse_ledger_lib/sparse_ledger.ml#L162-L172>
    #[test]
    fn a_replaced_inner_hash_keeps_the_cached_root_but_not_the_bytes() {
        let mut honest = sparse_ledger(3);
        let honest_root = honest.merkle_root();
        let honest_wire = wire(&honest);

        let mut tampered_wire = honest_wire.clone();
        let hidden = hidden_subtree(&mut tampered_wire.tree).expect("a hidden subtree");
        *hidden = to_ledger_hash(&Fp::from(12345u64));
        let mut tampered = SparseLedger::try_from(&tampered_wire).unwrap();

        assert_eq!(tampered.merkle_root(), honest_root);
        assert!(wire(&tampered) == tampered_wire);
        assert!(tampered_wire != honest_wire);

        // An account whose path passes the replaced subtree: the leftmost
        // account witnessed, at index 0. Its path from the honest ledger gives the root;
        // its path from the tampered one doesn't.
        let addr = honest
            .with(|this| this.values.keys().next().copied())
            .unwrap();
        let addr = Address::from_index(addr, honest.with(|this| this.depth));
        let leaf = honest.get_exn(&addr).hash();
        let honest_path = honest.path_exn(addr.clone());
        let tampered_path = tampered.path_exn(addr);
        assert_eq!(root_from_path(leaf, &honest_path), honest_root);
        assert_ne!(root_from_path(leaf, &tampered_path), honest_root);
    }

    /// A wire sparse ledger of any shape converts to an error or to a
    /// ledger that converts back. OCaml builds no other shape:
    /// accounts at the leaves, nodes above them, indexes in the tree.
    #[test]
    fn a_malformed_wire_ledger_is_an_error() {
        let honest = wire(&sparse_ledger(2));
        let depth = honest.depth.as_u64();
        let account = || {
            let account: mina_p2p_messages::v2::MinaBaseAccountBinableArgStableV2 =
                (&Account::empty()).into();
            Box::new(Tree::Account(Box::new(account)))
        };
        let hash = || Box::new(Tree::Hash(to_ledger_hash(&Fp::from(7u64))));
        let node = |left, right| Tree::Node(to_ledger_hash(&Fp::from(9u64)), left, right);
        // `levels` nodes, each with a hash on its right.
        let chain = |levels: usize, bottom: Box<Tree>| {
            (0..levels).fold(bottom, |below, _| Box::new(node(below, hash())))
        };

        let mut cases = Vec::new();
        // An account at the root, or one level above the leaves.
        cases.push(("account at the root", *account()));
        cases.push((
            "account above the leaves",
            *chain(depth as usize - 1, account()),
        ));
        // Nodes below the leaves, as deep as an address can go and deeper.
        for extra in [1, 6, 40] {
            cases.push((
                "node below the leaves",
                *chain(depth as usize + extra, hash()),
            ));
        }
        for (what, tree) in cases {
            let ledger = MinaBaseSparseLedgerBaseStableV2 {
                tree,
                ..honest.clone()
            };
            assert!(SparseLedger::try_from(&ledger).is_err(), "{what}");
        }

        // Depths an address can't hold.
        for depth in [36, 41, 64, u64::MAX] {
            let ledger = MinaBaseSparseLedgerBaseStableV2 {
                depth: depth.into(),
                tree: Tree::Hash(to_ledger_hash(&Fp::from(1u64))),
                ..honest.clone()
            };
            assert!(SparseLedger::try_from(&ledger).is_err(), "depth {depth}");
        }

        // An index outside the tree.
        let mut ledger = honest.clone();
        let index: mina_p2p_messages::number::UInt64 = (1u64 << depth).into();
        ledger.indexes.iter_mut().next().unwrap().1 = index;
        assert!(SparseLedger::try_from(&ledger).is_err());

        // The leaf level itself is fine: an account there, under nodes.
        let ledger = MinaBaseSparseLedgerBaseStableV2 {
            indexes: mina_p2p_messages::list::List::new(),
            tree: *chain(depth as usize, account()),
            ..honest.clone()
        };
        let converted = SparseLedger::try_from(&ledger).unwrap();
        assert!(wire(&converted) == ledger);
    }

    /// Indexes past 32 bits survive a sparse ledger, its wire form and back,
    /// at the ledger depth. OCaml's `add_path` computes the index from the
    /// path's directions, bit `i` set where step `i` from the leaf is `Right`.
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/sparse_ledger_lib/sparse_ledger.ml#L232-L246>
    #[test]
    fn indexes_past_32_bits_round_trip() {
        let depth = constraint_constants().ledger_depth as usize;
        assert_eq!(depth, 35);
        let indexes = [
            0,
            (1u64 << 32) - 1,
            1u64 << 32,
            (1u64 << 32) + 1,
            (1u64 << 34) + 5,
            (1u64 << 35) - 1,
        ];

        for &index in &indexes {
            // The path of an empty ledger: empty siblings at each height.
            let path = (0..depth)
                .map(|height| {
                    let sibling = V2::empty_hash_at_height(height);
                    if index >> height & 1 == 1 {
                        MerklePath::Right(sibling)
                    } else {
                        MerklePath::Left(sibling)
                    }
                })
                .collect::<Vec<_>>();
            let id = Account::rand().id();
            let mut alone = SparseLedger::create(depth, V2::empty_hash_at_height(depth));
            alone.add_path(&path, id.clone(), Account::empty());
            let addr = alone.find_index_exn(id.clone());
            assert_eq!(addr.to_index(), AccountIndex(index));
            assert_eq!(addr.length(), depth);
            assert_eq!(alone.path_exn(addr.clone()), path);
            assert_eq!(alone.merkle_root(), V2::empty_hash_at_height(depth));

            let (_, wire_index) = wire(&alone).indexes.iter().next().cloned().unwrap();
            assert_eq!(wire_index.as_u64(), index);
            let mut back = SparseLedger::try_from(&wire(&alone)).unwrap();
            assert_eq!(back.find_index_exn(id), addr);
            assert_eq!(back.path_exn(addr), path);
        }
    }
}
