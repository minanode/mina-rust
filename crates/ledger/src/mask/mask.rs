use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use mina_curves::pasta::Fp;
use mina_signer::CompressedPubKey;

use crate::{
    account::{Account, AccountId, TokenId},
    address::Address,
    base::{next_uuid, AccountIndex, BaseLedger, GetOrCreated, MerklePath, Uuid},
    database::{Database, DatabaseError},
    tree_version::V2,
    HashesMatrix,
};

use super::mask_impl::{MaskImpl, MaskImplShort};

#[derive(Clone, Debug)]
pub struct Mask {
    // Using a mutex for now but this can be replaced with a RefCell
    pub inner: Arc<Mutex<MaskImpl>>,
}

impl Drop for Mask {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) > 2 {
            // Don't drop because of counter
            return;
        }

        let Ok(inner) = self.inner.try_lock() else {
            // The Mask is used somewhere else
            return;
        };

        if inner.any_child_alive() {
            // Still got childs, don't do anything
            return;
        }

        let Some(parent) = inner.get_parent() else {
            // No parent, we don't need to do anything
            return;
        };

        // We reached a point where we don't have childs, and it remains at most 2
        // pointers of our mask:
        // - 1 pointer from the parent (having us in its `MaskImpl::childs` )
        // - 1 currently dropping pointer
        //
        // Unregister our mask from the parent (remove us from its `MaskImpl::childs`)
        // It will recursively drop/deallocate any parent with the same conditions

        // Note:
        // There is a case where the parent does not have any pointer of us:
        // During transaction application, we don't call `register_mask`
        // In that case, the `remove_child_uuid` below has no effect
        // <https://github.com/MinaProtocol/mina/blob/f6756507ff7380a691516ce02a3cf7d9d32915ae/src/lib/mina_ledger/ledger.ml#L204>

        parent.remove_child_uuid(inner.get_uuid());
    }
}

#[derive(Debug)]
pub enum UnregisterBehavior {
    Check,
    Recursive,
    IPromiseIAmReparentingThisMask,
}

impl Mask {
    pub(super) fn with<F, R>(&self, fun: F) -> R
    where
        F: FnOnce(&mut MaskImpl) -> R,
    {
        let mut inner = self.inner.try_lock().expect("lock failed");
        fun(&mut inner)
    }
}

impl Mask {
    pub fn new_root(db: Database<V2>) -> Self {
        let uuid = db.get_uuid();
        let mask = Self {
            inner: Arc::new(Mutex::new(MaskImpl::Root {
                database: db,
                childs: HashMap::with_capacity(2),
            })),
        };
        super::alive_add(&uuid);
        mask
    }

    pub fn new_unattached(depth: usize) -> Self {
        let uuid = next_uuid();

        let mask = Self {
            inner: Arc::new(Mutex::new(MaskImpl::Unattached {
                owning_account: Default::default(),
                token_owners: Default::default(),
                id_to_addr: Default::default(),
                last_location: None,
                depth: depth as u8,
                childs: HashMap::with_capacity(2),
                uuid: uuid.clone(),
                hashes: HashesMatrix::new(depth),
            })),
        };

        super::alive_add(&uuid);

        mask
    }

    pub fn create(depth: usize) -> Self {
        Self::new_root(Database::create(depth as u8))
    }

    pub fn create_with_token_owners(depth: usize) -> Self {
        Self::new_root(Database::create_with_token_owners(depth as u8))
    }

    pub fn set_token_owners(&mut self) {
        self.with(|this| this.set_token_owners());
    }

    // Note: This should be only called on startup
    pub fn unset_token_owners(&mut self) {
        self.with(|this| this.unset_token_owners());
    }

    pub fn make_child(&self) -> Mask {
        let mut new_mask = Mask::new_unattached(self.depth() as usize);

        if self.has_token_owners() {
            new_mask.set_token_owners();
        }

        self.register_mask(new_mask)
    }

    pub fn set_parent(&self, parent: Mask, parent_last_filled: Option<Option<Address>>) -> Mask {
        let this = self.clone();
        self.with(|this| this.set_parent(parent, parent_last_filled));
        this
    }

    pub(crate) fn nmasks_to_root(&self) -> usize {
        self.with(|this| this.nmasks_to_root())
    }

    pub fn copy(&self) -> Mask {
        let mask = self.with(|this| this.clone());
        Self {
            inner: Arc::new(Mutex::new(mask)),
        }
    }

    /// Make `mask` a child of `self`
    pub fn register_mask(&self, mask: Mask) -> Mask {
        // elog!("self={:p} mask={:p}", &self.inner, &mask.inner);

        let self_mask = self.clone();
        self.with(|this| this.register_mask(self_mask, mask))
    }

    /// Detach this mask from its parent
    pub fn unregister_mask(&self, behavior: UnregisterBehavior) -> Mask {
        self.unregister_mask_impl(behavior, true)
    }

    /// Moves this attached mask onto `parent`, keeping its accounts, its
    /// children, its uuid and its cached hashes, where `unregister_mask` and
    /// `register_mask` would drop the hashes. Only for a parent that holds
    /// the same account as the current one at every account this mask
    /// doesn't own ([`MaskImpl::replace_parent`]). False if this mask isn't
    /// attached.
    pub fn move_onto_parent(&self, parent: &Mask) -> bool {
        let uuid = self.get_uuid();
        let Some(old) = self.with(|this| this.replace_parent(parent.clone())) else {
            return false;
        };
        // A copy made by `Mask::copy` isn't registered with its parent.
        old.remove_child_uuid(uuid);
        parent.with(|this| this.adopt_child(self.clone()));
        true
    }

    pub(super) fn unregister_mask_impl(
        &self,
        behavior: UnregisterBehavior,
        remove_from_parent: bool,
    ) -> Mask {
        let this = self.clone();
        self.with(|this| this.unregister_mask(behavior, remove_from_parent));
        this
    }

    pub(super) fn remove_child_uuid(&self, uuid: Uuid) -> Option<Mask> {
        self.with(|this| this.remove_child_uuid(uuid))
    }

    pub fn is_root(&self) -> bool {
        self.with(|this| this.is_root())
    }

    pub fn is_attached(&self) -> bool {
        self.with(|this| this.is_attached())
    }

    fn uuid(&self) -> Uuid {
        self.with(|this| this.get_uuid())
    }

    pub fn get_parent(&self) -> Option<Mask> {
        self.with(|this| this.get_parent())
    }

    pub fn unset_parent(&self, trigger_detach_signal: bool) {
        self.with(|this| this.unset_parent(trigger_detach_signal))
    }

    /// //             o
    /// //            /
    /// //           /
    /// //  o --- o -
    /// //  ^     ^  \
    /// // parent |   \
    /// //       mask  o
    /// //           children
    ///
    /// Removes the attached mask from its parent and attaches the children to the
    /// parent instead. Raises an exception if the merkle roots of the mask and the
    /// parent are not the same.
    pub fn remove_and_reparent(&self) -> Option<Mask> {
        self.with(|this| this.remove_and_reparent())
    }

    /// get hash from mask, if present, else from its parent
    pub fn get_hash(&self, addr: Address) -> Option<Fp> {
        self.with(|this| this.get_hash(addr))
    }

    /// commit all state to the parent, flush state locally
    pub fn commit(&self) {
        self.with(|this| this.commit())
    }

    /// commit all the masks from this mask all the way upto the root
    /// and return root mask while also detaching all intermediary masks.
    pub fn commit_and_reparent_to_root(&mut self) -> Option<Mask> {
        self.with(|this| this.commit_and_reparent_to_root())
    }

    /// Commits this mask into its parent and removes it, its children moving
    /// to the parent, then does the same with the parent, and so on up the
    /// chain. Stops at the root, or at the first mask whose parent
    /// `is_base_parent` accepts, and returns the mask it stopped at, which now
    /// holds what this one did.
    ///
    /// [`Self::commit_and_reparent_to_root`] is this with a predicate that
    /// accepts nothing. A staged ledger's chain stops at its base mask, whose
    /// parent is a snarked ledger that must not take the staged changes.
    /// OCaml's `move_root` likewise commits the new root's mask into the old
    /// root's, which is parented off the root snarked ledger.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/full_frontier/full_frontier.ml#L457-L479>
    pub fn commit_and_reparent_to_base<F>(&self, is_base_parent: F) -> Mask
    where
        F: Fn(&Mask) -> bool,
    {
        let mut mask = self.clone();
        loop {
            let Some(parent) = mask.get_parent() else {
                return mask;
            };
            if is_base_parent(&parent) {
                return mask;
            }
            mask.commit();
            mask.remove_and_reparent();
            mask = parent;
        }
    }

    /// called when parent sets an account; update local state
    pub fn parent_set_notify(&self, account_index: AccountIndex, account: &Account) {
        self.with(|this| this.parent_set_notify(account_index, account))
    }

    pub fn remove_parent(&self) -> Option<Mask> {
        self.with(|this| this.remove_parent())
    }

    pub fn depth(&self) -> u8 {
        self.with(|this| this.depth())
    }

    pub fn get_cached_hash(&mut self, addr: &Address) -> Option<Fp> {
        self.with(|this| this.get_cached_hash(addr))
    }

    pub fn set_cached_hash_unchecked(&mut self, addr: &Address, hash: Fp) {
        self.with(|this| this.set_cached_hash_unchecked(addr, hash))
    }

    pub(super) fn set_impl(&mut self, addr: Address, account: Box<Account>, ignore: Option<Uuid>) {
        self.with(|this| this.set_impl(addr, account, ignore))
    }

    pub(super) fn transfert_hashes(&mut self, hashes: HashesMatrix) {
        self.with(|this| this.transfert_hashes(hashes))
    }

    pub(super) fn remove_accounts_without_notif(&mut self, ids: &[AccountId]) {
        self.with(|this| this.remove_accounts_without_notif(ids))
    }

    pub fn short(&self) -> MaskImplShort {
        self.with(|this| this.short())
    }

    /// Validate inner hashes by rehashing everything.
    /// Returns `Ok(())` if recalculated hashes matched the existing ones.
    ///
    /// Warning: Heavy operation.
    pub fn validate_inner_hashes(&mut self) -> Result<(), ()> {
        self.with(|this| this.validate_inner_hashes())
    }

    /// Returns a vector of tuples of (index, hash) of all the hashes contained
    /// in this mask.
    pub fn get_raw_inner_hashes(&self) -> Vec<(u64, Fp)> {
        self.with(|this| this.get_raw_inner_hashes())
    }

    /// Sets the contents of this mask's hash matrix using raw data.
    pub fn set_raw_inner_hashes(&self, hashes: Vec<(u64, Fp)>) {
        self.with(|this| this.set_raw_inner_hashes(hashes))
    }
    pub fn has_token_owners(&self) -> bool {
        self.with(|this| this.has_token_owners())
    }

    /// Moves a root database's accounts to a file in `dir`, where they stay
    /// readable and writable through this mask and the masks above it (PC3a).
    /// `Ok(None)` for a mask that isn't a root, or a root already frozen with
    /// no change since.
    pub fn freeze_to_disk(
        &self,
        dir: &std::path::Path,
    ) -> std::io::Result<Option<crate::database::FreezeReport>> {
        self.with(|this| match this {
            MaskImpl::Root { database, .. } => database.freeze_to_disk(dir),
            _ => Ok(None),
        })
    }

    /// For a root, calls `fun` with each account and its address, in slot
    /// order, and returns true; false for a mask that isn't a root.
    pub fn root_iter_with_addr<F>(&self, fun: F) -> bool
    where
        F: FnMut(Address, &Account),
    {
        self.with(|this| match this {
            MaskImpl::Root { database, .. } => {
                database.iter_with_addr(fun);
                true
            }
            _ => false,
        })
    }

    /// For a root whose accounts are in a file, whether its changes are
    /// enough to write the file again (PC3b).
    pub fn refreeze_due(&self) -> bool {
        self.with(|this| match this {
            MaskImpl::Root { database, .. } => database.refreeze_due(),
            _ => false,
        })
    }

    /// For a root whose accounts are in a file, the accounts its read cache
    /// holds.
    pub fn disk_cached(&self) -> Option<usize> {
        self.with(|this| match this {
            MaskImpl::Root { database, .. } => database.disk_cached(),
            _ => None,
        })
    }

    /// The accounts an attached or unattached mask holds itself; a root's
    /// count.
    pub fn num_own_accounts(&self) -> usize {
        self.with(|this| match this {
            MaskImpl::Root { .. } => this.num_accounts(),
            MaskImpl::Attached { owning_account, .. }
            | MaskImpl::Unattached { owning_account, .. } => owning_account.len(),
        })
    }

    /// Makes an attached mask over a root a root of its own, a copy of its
    /// parent's database with its changes committed in; see
    /// [`MaskImpl::detach_into_root`]. The accounts moved, or None.
    pub fn detach_into_root(&self) -> Option<usize> {
        self.with(|this| this.detach_into_root())
    }

    /// A copy of this ledger that is a database of its own, with no parent:
    /// for a root the same as [`Self::copy`] (which shares the file of a
    /// database on disk); for a mask, its parent's independent copy with the
    /// mask's own accounts set in. [`Self::copy`] of a mask reads through the
    /// same parent without being registered as its child, so folding it into
    /// that parent later (`remove_and_reparent`) panics, and so would a fold
    /// of another mask into the parent under it (PC3c review).
    pub fn copy_independent(&self) -> Mask {
        let Some(parent) = self.get_parent() else {
            return self.copy();
        };
        let mut base = parent.copy_independent();
        for index in self.own_account_indexes() {
            if let Some(account) = self.get_at_index(index) {
                base.set_at_index(index, account)
                    .expect("a database takes an account at any index");
            }
        }
        base
    }

    /// Whether masks are registered over this one.
    pub fn has_children(&self) -> bool {
        self.with(|this| this.any_child_alive())
    }

    /// For a root whose accounts are in a file, the slots changed since.
    pub fn disk_state(&self) -> Option<usize> {
        self.with(|this| match this {
            MaskImpl::Root { database, .. } => database.disk_state(),
            _ => None,
        })
    }

    /// Checks a root database's derived indexes against its leaves without
    /// changing it. Persistence loads independent roots, not attached masks.
    /// Returns the number of filled leaves checked.
    pub fn validate_account_indexes(&self) -> Result<usize, String> {
        self.with(|this| match this {
            MaskImpl::Root { database, .. } => database.with(|db| db.validate_account_indexes()),
            _ => Err("account index validation requires a root ledger".into()),
        })
    }

    /// As [`Self::validate_account_indexes`], for a mask over a root ledger
    /// (a restored root staged ledger over its snarked ledger, I5d5): its own
    /// indexes against its own accounts, and its accounts against the
    /// parent's (see `MaskImpl::validate_child_account_indexes`). The
    /// parent's own indexes are checked separately. Returns the number of the
    /// mask's own accounts checked.
    pub fn validate_child_account_indexes(&self) -> Result<usize, String> {
        self.with(|this| this.validate_child_account_indexes())
    }

    /// Makes a mask hold its own copy of each of these accounts it reads from
    /// its parent, without changing what it reads (see
    /// `MaskImpl::hold_parent_accounts`). Returns how many it now holds that
    /// it didn't.
    pub fn hold_parent_accounts(&self, account_ids: impl IntoIterator<Item = AccountId>) -> usize {
        self.with(|this| this.hold_parent_accounts(account_ids))
    }

    /// Release own accounts equal to the parent's resident account hashes,
    /// except those referenced by pending work. Only call for the root staged
    /// mask after the snarked fold and rebase, with all pending scan-state
    /// references (including incomplete second passes). No disk reads.
    pub fn reclaim_unreferenced_accounts(&self, pending: &HashSet<AccountId>) -> usize {
        #[cfg(debug_assertions)]
        let before = self.clone().merkle_root();
        let reclaimed = self.with(|this| this.reclaim_unreferenced_accounts(pending));
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            before,
            self.clone().merkle_root(),
            "reclamation changed the root"
        );
        reclaimed
    }

    /// The indices of the accounts the mask holds itself.
    pub fn own_account_indexes(&self) -> Vec<AccountIndex> {
        self.with(|this| this.own_account_indexes())
    }

    /// For tests only, check if the address is in the mask, without checking parent
    #[cfg(test)]
    fn test_is_in_mask(&self, addr: &Address) -> bool {
        self.with(|this| this.test_is_in_mask(addr))
    }

    /// For tests only
    #[cfg(test)]
    fn test_matrix(&self) -> HashesMatrix {
        self.with(|this| this.test_matrix())
    }

    /// Use for fuzzing only
    #[cfg(feature = "fuzzing")]
    pub fn fuzzing_to_root(&self) -> Mask {
        let accounts = self.to_list();
        let mut new_root = Self::create(self.depth() as usize);

        for account in accounts {
            new_root
                .get_or_create_account(account.id(), account)
                .unwrap();
        }

        new_root
    }
}

impl BaseLedger for Mask {
    fn to_list(&self) -> Vec<Account> {
        self.with(|this| this.to_list())
    }

    fn iter<F>(&self, fun: F)
    where
        F: FnMut(&Account),
    {
        self.with(|this| this.iter(fun))
    }

    fn fold<B, F>(&self, init: B, fun: F) -> B
    where
        F: FnMut(B, &Account) -> B,
    {
        self.with(|this| this.fold(init, fun))
    }

    fn fold_with_ignored_accounts<B, F>(&self, ignoreds: HashSet<AccountId>, init: B, fun: F) -> B
    where
        F: FnMut(B, &Account) -> B,
    {
        self.with(|this| this.fold_with_ignored_accounts(ignoreds, init, fun))
    }

    fn fold_until<B, F>(&self, init: B, fun: F) -> B
    where
        F: FnMut(B, &Account) -> std::ops::ControlFlow<B, B>,
    {
        self.with(|this| this.fold_until(init, fun))
    }

    fn accounts(&self) -> HashSet<AccountId> {
        self.with(|this| this.accounts())
    }

    fn token_owner(&self, token_id: TokenId) -> Option<AccountId> {
        self.with(|this| this.token_owner(token_id))
    }

    fn tokens(&self, public_key: CompressedPubKey) -> HashSet<TokenId> {
        self.with(|this| this.tokens(public_key))
    }

    fn location_of_account(&self, account_id: &AccountId) -> Option<Address> {
        self.with(|this| this.location_of_account(account_id))
    }

    fn location_of_account_batch(
        &self,
        account_ids: &[AccountId],
    ) -> Vec<(AccountId, Option<Address>)> {
        self.with(|this| this.location_of_account_batch(account_ids))
    }

    fn get_or_create_account(
        &mut self,
        account_id: AccountId,
        account: Account,
    ) -> Result<GetOrCreated, DatabaseError> {
        self.with(|this| this.get_or_create_account(account_id, account))
    }

    fn close(&self) {
        // Drop self
    }

    fn last_filled(&self) -> Option<Address> {
        self.with(|this| this.last_filled())
    }

    fn get_uuid(&self) -> Uuid {
        self.with(|this| this.get_uuid())
    }

    fn get_directory(&self) -> Option<PathBuf> {
        self.with(|this| this.get_directory())
    }

    fn get_account_hash(&mut self, account_index: AccountIndex) -> Option<Fp> {
        self.with(|this| this.get_account_hash(account_index))
    }

    fn get(&self, addr: Address) -> Option<Box<Account>> {
        self.with(|this| this.get(addr))
    }

    fn get_batch(&self, addr: &[Address]) -> Vec<(Address, Option<Box<Account>>)> {
        self.with(|this| this.get_batch(addr))
    }

    fn set(&mut self, addr: Address, account: Box<Account>) {
        self.with(|this| this.set(addr, account))
    }

    fn set_batch(&mut self, list: &[(Address, Box<Account>)]) {
        self.with(|this| this.set_batch(list))
    }

    fn get_at_index(&self, index: AccountIndex) -> Option<Box<Account>> {
        self.with(|this| this.get_at_index(index))
    }

    fn set_at_index(&mut self, index: AccountIndex, account: Box<Account>) -> Result<(), ()> {
        self.with(|this| this.set_at_index(index, account))
    }

    fn index_of_account(&self, account_id: AccountId) -> Option<AccountIndex> {
        self.with(|this| this.index_of_account(account_id))
    }

    fn merkle_root(&mut self) -> Fp {
        self.with(|this| this.merkle_root())
    }

    fn merkle_path(&mut self, addr: Address) -> Vec<MerklePath> {
        let addr_length = addr.length();
        let res = self.with(|this| this.merkle_path(addr.clone()));
        assert_eq!(res.len(), addr_length);
        res
    }

    fn merkle_path_at_index(&mut self, index: AccountIndex) -> Vec<MerklePath> {
        self.with(|this| this.merkle_path_at_index(index))
    }

    fn remove_accounts(&mut self, ids: &[AccountId]) {
        self.with(|this| this.remove_accounts(ids))
    }

    fn detached_signal(&mut self) {
        self.with(|this| this.detached_signal())
    }

    fn depth(&self) -> u8 {
        self.with(|this| this.depth())
    }

    fn num_accounts(&self) -> usize {
        self.with(|this| this.num_accounts())
    }

    fn merkle_path_at_addr(&mut self, addr: Address) -> Vec<MerklePath> {
        self.with(|this| this.merkle_path_at_addr(addr))
    }

    fn get_inner_hash_at_addr(&mut self, addr: Address) -> Result<Fp, String> {
        self.with(|this| this.get_inner_hash_at_addr(addr))
    }

    fn set_inner_hash_at_addr(&mut self, addr: Address, hash: Fp) -> Result<(), ()> {
        self.with(|this| this.set_inner_hash_at_addr(addr, hash))
    }

    fn set_all_accounts_rooted_at(
        &mut self,
        addr: Address,
        accounts: &[Box<Account>],
    ) -> Result<(), ()> {
        self.with(|this| this.set_all_accounts_rooted_at(addr, accounts))
    }

    fn get_all_accounts_rooted_at(&self, addr: Address) -> Option<Vec<(Address, Box<Account>)>> {
        self.with(|this| this.get_all_accounts_rooted_at(addr))
    }

    fn make_space_for(&mut self, space: usize) {
        self.with(|this| this.make_space_for(space))
    }

    fn commit(&mut self) {
        self.with(|this| this.commit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tests_mask_ocaml::*;

    #[cfg(target_family = "wasm")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    // OCaml 4.0.2 `mina ledger hash` of the empty mainnet ledger.
    const EMPTY_ROOT: &str = "jwkaDMeSMeL94hwJg6EYtSRD6gxB1BHpXnGKnZ1VW6jthKzgcef";

    #[test]
    fn empty_unattached_mask_hashes_without_parent() {
        let mut mask = Mask::new_unattached(35);
        assert_eq!(
            mina_p2p_messages::v2::LedgerHash::from_fp(mask.merkle_root()).to_string(),
            EMPTY_ROOT
        );
    }

    #[test]
    fn empty_unattached_mask_merkle_path_without_parent() {
        // Start with a cold cache, independently of the root-hash test.
        let mut mask = Mask::new_unattached(35);
        let path = mask.merkle_path(Address::first(35));
        assert_eq!(path.len(), 35);
        assert_eq!(
            mina_p2p_messages::v2::LedgerHash::from_fp(mask.merkle_root()).to_string(),
            EMPTY_ROOT
        );
    }

    // Build the reference directly in a database from the input accounts, never
    // from the mask's root or path. Check every leaf, including absent accounts,
    // and both root-first and path-first cache population orders.
    fn assert_mask_view(mask: &mut Mask, accounts: &[Option<Account>], root_first: bool) {
        let depth = 4;
        let mut reference = Database::<V2>::create(depth as u8);
        for (index, account) in accounts.iter().enumerate() {
            if let Some(account) = account {
                reference.set(
                    Address::from_index(AccountIndex(index as u64), depth),
                    Box::new(account.clone()),
                );
            }
        }
        if root_first {
            assert_eq!(mask.merkle_root(), reference.merkle_root());
        }
        for index in 0..(1 << depth) {
            let addr = Address::from_index(AccountIndex(index), depth);
            assert_eq!(mask.get(addr.clone()), reference.get(addr.clone()));
            assert_eq!(mask.merkle_path(addr.clone()), reference.merkle_path(addr));
        }
        for account in accounts.iter().flatten() {
            assert_eq!(
                mask.index_of_account(account.id()),
                reference.index_of_account(account.id())
            );
        }
        assert_eq!(mask.merkle_root(), reference.merkle_root());
    }

    fn set_fixture(mask: &mut Mask, index: u64, account: &Account) {
        mask.set(
            Address::from_index(AccountIndex(index), 4),
            Box::new(account.clone()),
        );
    }

    #[test]
    fn parent_change_attach_invalidates_cached_roots_and_paths() {
        for root_first in [false, true] {
            for owns_account in [false, true] {
                let inherited = Account::rand();
                let owned = Account::rand();
                let descendant = Account::rand();
                let mut parent = Mask::create(4);
                set_fixture(&mut parent, 0, &inherited);
                let mut mask = Mask::new_unattached(4);
                let own = owns_account.then_some(owned);
                if let Some(account) = &own {
                    set_fixture(&mut mask, 1, account);
                }
                let mut child = mask.make_child();
                set_fixture(&mut child, 2, &descendant);
                assert_mask_view(&mut mask, &[None, own.clone()], root_first);
                assert_mask_view(
                    &mut child,
                    &[None, own.clone(), Some(descendant.clone())],
                    root_first,
                );

                parent.register_mask(mask.clone());
                assert_mask_view(
                    &mut mask,
                    &[Some(inherited.clone()), own.clone()],
                    root_first,
                );
                assert_mask_view(
                    &mut child,
                    &[Some(inherited), own, Some(descendant)],
                    root_first,
                );
            }
        }
    }

    #[test]
    fn parent_change_detach_invalidates_cached_roots_and_paths() {
        for root_first in [false, true] {
            let inherited = Account::rand();
            let owned = Account::rand();
            let descendant = Account::rand();
            let mut parent = Mask::create(4);
            set_fixture(&mut parent, 0, &inherited);
            let mut mask = parent.make_child();
            set_fixture(&mut mask, 1, &owned);
            let mut child = mask.make_child();
            set_fixture(&mut child, 2, &descendant);
            assert_mask_view(
                &mut mask,
                &[Some(inherited.clone()), Some(owned.clone())],
                root_first,
            );
            assert_mask_view(
                &mut child,
                &[
                    Some(inherited.clone()),
                    Some(owned.clone()),
                    Some(descendant.clone()),
                ],
                root_first,
            );

            mask.unregister_mask(UnregisterBehavior::IPromiseIAmReparentingThisMask);
            assert_mask_view(&mut mask, &[None, Some(owned.clone())], root_first);
            assert_mask_view(
                &mut child,
                &[None, Some(owned), Some(descendant)],
                root_first,
            );
            assert!(mask.index_of_account(inherited.id()).is_none());
            assert!(child.index_of_account(inherited.id()).is_none());
            parent.register_mask(mask);
        }
    }

    #[test]
    fn parent_change_reparent_invalidates_cached_roots_and_paths() {
        for root_first in [false, true] {
            let inherited = Account::rand();
            let replacement = Account::rand();
            let owned = Account::rand();
            let descendant = Account::rand();
            let mut parent = Mask::create(4);
            let mut next_parent = Mask::create(4);
            set_fixture(&mut parent, 0, &inherited);
            set_fixture(&mut next_parent, 0, &replacement);
            let mut mask = parent.make_child();
            set_fixture(&mut mask, 1, &owned);
            let mut child = mask.make_child();
            set_fixture(&mut child, 2, &descendant);
            assert_mask_view(
                &mut mask,
                &[Some(inherited.clone()), Some(owned.clone())],
                root_first,
            );
            assert_mask_view(
                &mut child,
                &[
                    Some(inherited.clone()),
                    Some(owned.clone()),
                    Some(descendant.clone()),
                ],
                root_first,
            );

            mask.unregister_mask(UnregisterBehavior::IPromiseIAmReparentingThisMask);
            next_parent.register_mask(mask.clone());
            assert_mask_view(
                &mut mask,
                &[Some(replacement.clone()), Some(owned.clone())],
                root_first,
            );
            assert_mask_view(
                &mut child,
                &[Some(replacement), Some(owned), Some(descendant)],
                root_first,
            );
            assert!(mask.index_of_account(inherited.id()).is_none());
            assert!(child.index_of_account(inherited.id()).is_none());
        }
    }

    #[test]
    fn parent_change_commit_orders_preserve_roots_and_paths() {
        for root_first in [false, true] {
            for child_first in [false, true] {
                let inherited = Account::rand();
                let owned = Account::rand();
                let descendant = Account::rand();
                let mut root = Mask::create(4);
                set_fixture(&mut root, 0, &inherited);
                let mut mask = root.make_child();
                set_fixture(&mut mask, 1, &owned);
                let mut child = mask.make_child();
                set_fixture(&mut child, 2, &descendant);
                let mut sibling = root.make_child();
                let root_view = [Some(inherited.clone())];
                let mask_view = [Some(inherited.clone()), Some(owned.clone())];
                let full_view = [Some(inherited), Some(owned), Some(descendant)];
                assert_mask_view(&mut root, &root_view, root_first);
                assert_mask_view(&mut sibling, &root_view, root_first);
                assert_mask_view(&mut mask, &mask_view, root_first);
                assert_mask_view(&mut child, &full_view, root_first);

                if child_first {
                    child.commit();
                    assert_mask_view(&mut mask, &full_view, root_first);
                    assert_mask_view(&mut child, &full_view, root_first);
                }
                mask.commit();
                let committed = if child_first {
                    &full_view[..]
                } else {
                    &mask_view[..]
                };
                assert_mask_view(&mut root, committed, root_first);
                assert_mask_view(&mut sibling, committed, root_first);
                assert_mask_view(&mut mask, committed, root_first);
                assert_mask_view(&mut child, &full_view, root_first);
                mask.remove_and_reparent();
                assert_mask_view(&mut mask, &[], root_first);
                assert_eq!(child.get_parent().unwrap().get_uuid(), root.get_uuid());
                assert_mask_view(&mut child, &full_view, root_first);
                child.commit();
                assert_mask_view(&mut root, &full_view, root_first);
                assert_mask_view(&mut sibling, &full_view, root_first);
                assert_mask_view(&mut child, &full_view, root_first);
                child.unregister_mask(UnregisterBehavior::Check);
                assert_mask_view(&mut child, &[], root_first);
            }
        }
    }

    #[test]
    fn copied_mask_does_not_retain_original_children() {
        let root = Mask::create(4);
        // All three variants may have registered children. A copy must not
        // retain those children or change their actual parent.
        for parent in [root.clone(), root.make_child(), Mask::new_unattached(4)] {
            let child = parent.make_child();
            let child_id = child.get_uuid();
            let copy = parent.copy();
            assert!(!copy.with(|inner| inner.any_child_alive()));
            assert_eq!(child.get_parent().unwrap().get_uuid(), parent.get_uuid());
            drop(child);
            assert!(!crate::mask::is_alive(&child_id));
            drop(copy);
            drop(parent);
        }
        assert!(!root.with(|inner| inner.any_child_alive()));
    }

    #[test]
    fn frontier_length_mask_chains_are_released_after_compaction() {
        // Exercise more than 16,000 generations while bounding the live chain
        // at mainnet k + a root. Track our UUIDs, not the process-global count,
        // because other ledger tests run concurrently.
        let mut tip = Mask::create(35);
        for _ in 0..56 {
            let mut ids = Vec::new();
            for _ in 0..290 {
                tip = tip.make_child();
                ids.push(tip.get_uuid());
            }
            assert!(tip.get(Address::first(35)).is_none());
            let root = tip.commit_and_reparent_to_root().unwrap();
            tip = root;
            assert!(ids.iter().all(|id| !crate::mask::is_alive(id)));
            assert_eq!(tip.nmasks_to_root(), 0);
        }
    }

    #[test]
    fn test_drop_mask() {
        let root_uuid;
        let child1_uuid;
        let child2_uuid;

        let child = {
            let child = {
                println!("A");
                let root = Mask::new_unattached(25);
                root_uuid = root.get_uuid();
                println!("root={:?}", root.get_uuid());
                // let root = Mask::new_root(crate::Database::create(35.try_into().unwrap()));
                println!("B");
                let child = root.make_child();
                println!("child={:?}", child.get_uuid());
                child1_uuid = child.get_uuid();
                child
            };
            println!("C");
            assert!(child.is_attached());

            let child = child.make_child();
            child2_uuid = child.get_uuid();
            child
        };

        println!("D");
        println!("child2={:?}", child.get_uuid());
        assert!(child.is_attached());

        {
            let parent = child.get_parent().unwrap();
            let parent = parent.get_parent().unwrap();
            assert_eq!(parent.get_uuid(), root_uuid);
        }

        // The 3 masks should still be alive
        assert!(crate::mask::is_alive(&root_uuid));
        assert!(crate::mask::is_alive(&child1_uuid));
        assert!(crate::mask::is_alive(&child2_uuid));

        std::mem::drop(child);

        // Now they are all drop/deallocated
        assert!(!crate::mask::is_alive(&root_uuid));
        assert!(!crate::mask::is_alive(&child1_uuid));
        assert!(!crate::mask::is_alive(&child2_uuid));
    }

    #[test]
    fn test_merkle_path_one_account() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Account::rand();

        let addr = root
            .get_or_create_account(account.id(), account)
            .unwrap()
            .addr();

        let path = mask.merkle_path(addr);
        assert_eq!(path.len(), DEPTH);
    }

    #[test]
    fn test_masks() {
        const DEPTH: usize = 20;

        let root = Mask::new_unattached(DEPTH);
        let mask = Mask::new_unattached(DEPTH);

        let mut mask = root.register_mask(mask);

        let accounts: Vec<_> = (0..18).map(|_| Account::rand()).collect();

        for account in &accounts {
            mask.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let mask_paths: Vec<_> = (0..18)
            .map(|index| {
                let index: AccountIndex = index.into();
                let addr = Address::from_index(index, DEPTH);
                mask.merkle_path(addr)
            })
            .collect();

        let mask_root_hash = mask.merkle_root();

        let mut db = Database::create(DEPTH as u8);
        for account in &accounts {
            db.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let db_paths: Vec<_> = (0..18)
            .map(|index| {
                let index: AccountIndex = index.into();
                let addr = Address::from_index(index, DEPTH);
                mask.merkle_path(addr)
            })
            .collect();

        let db_root_hash = db.merkle_root();

        assert_eq!(mask_root_hash, db_root_hash);
        assert_eq!(mask_paths, db_paths);
    }

    #[test]
    fn test_masks_unregister_recursive() {
        let (_root, layer1, layer2) = new_chain(DEPTH);

        let layer3 = layer2.make_child();
        let layer4 = layer2.make_child();

        for mask in [&layer1, &layer2, &layer3, &layer4] {
            assert!(mask.get_parent().is_some());
        }

        // This should not panic
        layer1.unregister_mask(UnregisterBehavior::Recursive);

        for mask in [&layer1, &layer2, &layer3, &layer4] {
            assert!(mask.get_parent().is_none());
        }
    }

    // Make sure hashes are correctly invalided in masks (parents/childs)
    #[test]
    fn test_masks_cached_hashes() {
        for case in 0..2 {
            let (mut root, mut layer1, mut layer2) = new_chain(DEPTH);

            let acc1 = Account::rand();
            let acc2 = Account::rand();
            let acc3 = Account::rand();

            let _loc1 = root.get_or_create_account(acc1.id(), acc1).unwrap().addr();
            let _loc2 = layer1
                .get_or_create_account(acc2.id(), acc2.clone())
                .unwrap()
                .addr();
            let _loc3 = layer2
                .get_or_create_account(acc3.id(), acc3)
                .unwrap()
                .addr();

            let root_hash = layer2.merkle_root();

            // Different cases where is should result in a different hash for the childs

            if case == 0 {
                layer1.remove_accounts(&[acc2.id()]);
            } else if case == 1 {
                let account_index = AccountIndex::from(1);
                let addr = Address::from_index(account_index, DEPTH);
                let new_account = Account::rand();

                assert_ne!(*layer1.get(addr.clone()).unwrap(), new_account);

                layer1.set(addr, Box::new(new_account));
            }

            assert_ne!(root_hash, layer2.merkle_root(), "case {:?}", case);
        }
    }

    #[test]
    fn test_cached_merkle_path() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Account::rand();
        let addr = Address::first(DEPTH);

        mask.set(addr.clone(), Box::new(account.clone()));
        mask.merkle_root();
        let mask_merkle_path = mask.merkle_path(addr.clone());

        root.set(addr.clone(), Box::new(account));
        root.merkle_root();
        let root_merkle_path = root.merkle_path(addr);

        assert!(!mask_merkle_path.is_empty());
        assert_eq!(mask_merkle_path, root_merkle_path);
        elog!("path={:?}", mask_merkle_path);
    }
}

/// PC3b: an attached mask over a frozen root made a root of its own.
#[cfg(all(test, not(target_family = "wasm")))]
mod tests_detach {
    use crate::{scan_state::currency::Balance, Account, AccountIndex, BaseLedger, Database, Mask};

    const DEPTH: usize = 20;

    fn set_balance(mask: &mut Mask, index: u64, balance: u64) {
        let mut account = mask.get_at_index(AccountIndex(index)).unwrap();
        account.balance = Balance::from_u64(balance);
        mask.set_at_index(AccountIndex(index), account).unwrap();
    }

    #[test]
    fn detach_into_root_keeps_the_ledger_parent_and_children() {
        let dir = tempfile::tempdir().unwrap();
        let mut parent = Mask::new_root(Database::create_with_token_owners(DEPTH as u8));
        for _ in 0..100 {
            let account = Account::rand();
            parent.get_or_create_account(account.id(), account).unwrap();
        }
        parent.freeze_to_disk(dir.path()).unwrap().unwrap();
        let parent_root = parent.merkle_root();
        let parent_list = parent.to_list();

        let mut mask = parent.make_child();
        for index in [0, 3, 99] {
            set_balance(&mut mask, index, index + 1000);
        }
        let added = Account::rand();
        mask.get_or_create_account(added.id(), added.clone())
            .unwrap();
        let mut child = mask.make_child();
        set_balance(&mut child, 3, 7);
        set_balance(&mut child, 4, 8);
        let mask_root = mask.merkle_root();
        let mask_list = mask.to_list();
        let child_root = child.merkle_root();
        let child_list = child.to_list();
        let uuid = mask.get_uuid();

        assert_eq!(mask.detach_into_root(), Some(4));
        assert!(mask.is_root());
        assert_eq!(mask.get_uuid(), uuid);
        assert!(crate::mask::is_alive(&uuid));
        // The copy shares the parent's file, its changes in its overlay.
        assert_eq!(mask.disk_state(), Some(4));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(mask.merkle_root(), mask_root);
        assert_eq!(mask.to_list(), mask_list);
        assert_eq!(
            mask.location_of_account(&added.id()),
            Some(crate::Address::from_index(AccountIndex(100), DEPTH))
        );
        // The child is still the child of the same mask, and sees the same.
        assert_eq!(child.get_parent().unwrap().get_uuid(), uuid);
        assert_eq!(child.merkle_root(), child_root);
        assert_eq!(child.to_list(), child_list);
        // The parent lost a child and nothing else.
        assert_eq!(parent.merkle_root(), parent_root);
        assert_eq!(parent.to_list(), parent_list);
        assert!(parent.remove_child_uuid(uuid.clone()).is_none());
        // The root of its own takes commits and writes its own file.
        child.commit();
        let mut expected = parent.copy();
        for (index, balance) in [(0, 1000), (3, 7), (4, 8), (99, 1099)] {
            set_balance(&mut expected, index, balance);
        }
        expected.get_or_create_account(added.id(), added).unwrap();
        assert_eq!(mask.merkle_root(), expected.merkle_root());
        mask.freeze_to_disk(dir.path()).unwrap().unwrap();
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        assert_eq!(mask.to_list(), expected.to_list());
        // A root, or a mask over another mask, isn't detached.
        assert_eq!(parent.detach_into_root(), None);
        let grandchild = child.make_child().make_child();
        assert_eq!(grandchild.detach_into_root(), None);
    }

    /// An independent copy of a mask over a mask over a frozen database is a
    /// database of its own holding the same ledger; the originals can then
    /// be folded into their parents, or changed, without touching it.
    #[test]
    fn copy_independent_of_a_chain_of_masks() {
        let dir = tempfile::tempdir().unwrap();
        let mut root = Mask::new_root(Database::create_with_token_owners(DEPTH as u8));
        for _ in 0..40 {
            let account = Account::rand();
            root.get_or_create_account(account.id(), account).unwrap();
        }
        root.freeze_to_disk(dir.path()).unwrap().unwrap();
        let mut middle = root.make_child();
        set_balance(&mut middle, 1, 1001);
        let mut top = middle.make_child();
        set_balance(&mut top, 2, 2002);
        let added = Account::rand();
        top.get_or_create_account(added.id(), added.clone())
            .unwrap();
        let (top_root, top_list) = (top.merkle_root(), top.to_list());

        let mut copy = top.copy_independent();
        assert!(copy.is_root());
        assert!(copy.get_parent().is_none());
        assert_eq!(copy.merkle_root(), top_root);
        assert_eq!(copy.to_list(), top_list);
        // Folding the originals down and changing them leaves the copy as it was.
        top.commit();
        set_balance(&mut middle, 3, 3003);
        middle.commit();
        assert_eq!(copy.merkle_root(), top_root);
        assert_eq!(copy.to_list(), top_list);
        // A root's independent copy is its copy.
        assert!(root.copy_independent().is_root());
    }

    /// A copy of a mask reads through the same parent without being its
    /// registered child (PC3c: a sync ledger copied from a mask). It detaches
    /// as a registered one does, and the original is untouched.
    #[test]
    fn detach_into_root_of_an_unregistered_copy() {
        let dir = tempfile::tempdir().unwrap();
        let mut parent = Mask::new_root(Database::create_with_token_owners(DEPTH as u8));
        for _ in 0..50 {
            let account = Account::rand();
            parent.get_or_create_account(account.id(), account).unwrap();
        }
        parent.freeze_to_disk(dir.path()).unwrap().unwrap();
        let mut mask = parent.make_child();
        set_balance(&mut mask, 1, 1001);
        let mut copy = mask.copy();
        set_balance(&mut copy, 2, 2002);
        let (mask_root, copy_root) = (mask.merkle_root(), copy.merkle_root());
        let copy_list = copy.to_list();

        assert_eq!(copy.detach_into_root(), Some(2));
        assert!(copy.is_root());
        assert_eq!(copy.merkle_root(), copy_root);
        assert_eq!(copy.to_list(), copy_list);
        // The registered original still reads through the parent.
        assert!(!mask.is_root());
        assert_eq!(mask.merkle_root(), mask_root);
        assert!(parent.has_children());
    }
}

#[cfg(test)]
mod tests_mask_ocaml {
    use crate::scan_state::currency::{Balance, Magnitude};

    use super::*;

    use rand::{thread_rng, Rng};

    #[cfg(target_family = "wasm")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    pub const DEPTH: usize = 4;
    pub const FIRST_LOC: Address = Address::first(DEPTH);

    pub fn new_instances(depth: usize) -> (Mask, Mask) {
        let db = Database::<V2>::create(depth as u8);
        (Mask::new_root(db), Mask::new_unattached(depth))
    }

    pub fn new_chain(depth: usize) -> (Mask, Mask, Mask) {
        let db = Database::<V2>::create(depth as u8);
        let layer1 = Mask::new_unattached(depth);
        let layer2 = Mask::new_unattached(depth);

        let root = Mask::new_root(db);
        let layer1 = root.register_mask(layer1);
        let layer2 = layer1.register_mask(layer2);

        (root, layer1, layer2)
    }

    fn make_full_accounts(depth: usize) -> Vec<Account> {
        (0..2u64.pow(depth as u32))
            .map(|_| Account::rand())
            .collect()
    }

    // "parent, mask agree on set"
    #[test]
    fn test_parent_mask_agree_on_set() {
        let (mut root, mask) = new_instances(DEPTH);
        let mask = root.register_mask(mask);

        root.set(FIRST_LOC, Box::new(Account::rand()));

        let root_account = root.get(FIRST_LOC).unwrap();
        let mask_account = mask.get(FIRST_LOC).unwrap();

        assert_eq!(root_account, mask_account);
    }

    // "parent, mask agree on set"
    #[test]
    fn test_parent_mask_agree_on_set2() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Box::new(Account::rand());
        root.set(FIRST_LOC, account.clone());
        mask.set(FIRST_LOC, account);

        let root_account = root.get(FIRST_LOC).unwrap();
        let mask_account = mask.get(FIRST_LOC).unwrap();

        assert_eq!(root_account, mask_account);
    }

    // "parent, mask agree on hashes; set in both mask and parent"
    #[test]
    fn test_parent_mask_agree_on_hashes() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Box::new(Account::rand());
        root.set(FIRST_LOC, account.clone());
        mask.set(FIRST_LOC, account);

        assert_eq!(root.merkle_root(), mask.merkle_root());
    }

    // "parent, mask agree on hashes; set only in parent"
    #[test]
    fn test_parent_mask_agree_on_hashes_set_parent_only() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Box::new(Account::rand());
        root.set(FIRST_LOC, account);

        assert_eq!(root.merkle_root(), mask.merkle_root());
    }

    // "mask delegates to parent"
    #[test]
    fn test_mask_delegate_to_parent() {
        let (mut root, mask) = new_instances(DEPTH);
        let mask = root.register_mask(mask);

        let account = Box::new(Account::rand());
        root.set(FIRST_LOC, account.clone());

        let child_account = mask.get(FIRST_LOC).unwrap();

        assert_eq!(account, child_account);
    }

    // "mask prune after parent notification", inverted: OCaml's test calls
    // `Maskable.set`, which no production path does (PRUNE-1). A mask keeps
    // its own account when the parent's becomes the same.
    #[test]
    fn test_mask_keeps_own_account_after_parent_notif() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        // Set in mask
        let account = Box::new(Account::rand());
        mask.set(FIRST_LOC, account.clone());

        assert!(mask.test_is_in_mask(&FIRST_LOC));

        root.set(FIRST_LOC, account);

        // The address is still in the mask
        assert!(mask.test_is_in_mask(&FIRST_LOC));
    }

    // "commit puts mask contents in parent, flushes mask"
    #[test]
    fn test_commit_puts_mask_in_parent_and_flush_mask() {
        let (root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Box::new(Account::rand());
        mask.set(FIRST_LOC, account);

        assert!(mask.test_is_in_mask(&FIRST_LOC));

        mask.commit();

        // No more in mask
        assert!(!mask.test_is_in_mask(&FIRST_LOC));
        // The parent get the account
        assert!(root.get(FIRST_LOC).is_some());
    }

    // "commit at layer2, dumps to layer1, not in base"
    #[test]
    fn test_commit_layer2_dumps_to_layer1_not_in_base() {
        let (root, layer1, mut layer2) = new_chain(DEPTH);

        let account = Box::new(Account::rand());

        layer2.set(FIRST_LOC, account);
        assert!(layer2.test_is_in_mask(&FIRST_LOC));
        assert!(!layer1.test_is_in_mask(&FIRST_LOC));

        layer2.commit();
        assert!(!layer2.test_is_in_mask(&FIRST_LOC));
        assert!(layer1.test_is_in_mask(&FIRST_LOC));
        assert!(!root.test_is_in_mask(&FIRST_LOC));
    }

    // "commit at layer2, dumps to layer1, not in base"
    #[test]
    fn test_commit_layer2_to_root_dumps_to_base_not_in_layer1() {
        let (root, mut layer1, mut layer2) = new_chain(DEPTH);

        let (addr1, addr2) = (FIRST_LOC, FIRST_LOC.next().unwrap());
        let (account1, account2) = (Box::new(Account::rand()), Box::new(Account::rand()));

        layer1.set(addr1.clone(), account1);
        layer2.set(addr2.clone(), account2);

        assert!(layer1.test_is_in_mask(&addr1));
        assert!(!layer1.test_is_in_mask(&addr2));
        assert!(layer2.test_is_in_mask(&addr2));
        assert!(!layer2.test_is_in_mask(&addr1));

        layer2.commit_and_reparent_to_root();
        assert!(!layer1.test_is_in_mask(&addr1));
        assert!(!layer1.test_is_in_mask(&addr2));
        assert!(!layer2.test_is_in_mask(&addr1));
        assert!(!layer2.test_is_in_mask(&addr2));

        assert!(!layer1.is_attached());
        assert!(!layer2.is_attached());
        assert!(root.test_is_in_mask(&addr1));
        assert!(root.test_is_in_mask(&addr2));
    }

    // "register and unregister mask"
    #[test]
    fn test_register_unregister_mask() {
        let (root, mask) = new_instances(DEPTH);
        let mask = root.register_mask(mask);
        mask.unregister_mask(UnregisterBehavior::Recursive);
    }

    // "mask and parent agree on Merkle path"
    #[test]
    fn test_mask_and_parent_agree_on_merkle_path() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Box::new(Account::rand());
        let addr = Address::first(DEPTH);

        mask.set(addr.clone(), account.clone());
        let mask_merkle_path = mask.merkle_path(addr.clone());

        root.set(addr.clone(), account);
        let root_merkle_path = root.merkle_path(addr);

        assert!(!mask_merkle_path.is_empty());
        assert_eq!(mask_merkle_path, root_merkle_path);
        elog!("path={:?}", mask_merkle_path);
    }

    // "mask and parent agree on Merkle root before set"
    #[test]
    fn test_agree_on_root_hash_before_set() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        assert_eq!(root.merkle_root(), mask.merkle_root());
    }

    // "mask and parent agree on Merkle root after set"
    #[test]
    fn test_agree_on_root_hash_after_set() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let account = Box::new(Account::rand());

        // the order of sets matters here; if we set in the mask first,
        // the set in the maskable notifies the mask, which then removes
        // the account, changing the Merkle root to what it was before the set

        root.set(FIRST_LOC, account.clone());
        mask.set(FIRST_LOC, account);

        assert!(root.test_is_in_mask(&FIRST_LOC));
        assert!(mask.test_is_in_mask(&FIRST_LOC));
        assert_eq!(root.merkle_root(), mask.merkle_root());
    }

    // "add and retrieve a block of accounts"
    #[test]
    fn test_add_retrieve_block_of_accounts() {
        let (root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = make_full_accounts(DEPTH);

        for account in &accounts {
            let account_id = account.id();
            let res = mask
                .get_or_create_account(account_id, account.clone())
                .unwrap();
            assert!(matches!(res, GetOrCreated::Added(_)));
        }

        let retrieved_accounts = mask
            .get_all_accounts_rooted_at(Address::root())
            .unwrap()
            .into_iter()
            .map(|(_, acc)| acc)
            .collect::<Vec<_>>();

        assert_eq!(
            accounts.into_iter().map(Box::new).collect::<Vec<_>>(),
            retrieved_accounts
        );
    }

    // "removing accounts from mask restores Merkle root"
    #[test]
    fn test_removing_accounts_from_mask_restore_root_hash() {
        let (root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = (0..5).map(|_| Account::rand()).collect::<Vec<_>>();
        let accounts_ids = accounts.iter().map(Account::id).collect::<Vec<_>>();
        let root_hash0 = mask.merkle_root();

        for account in accounts {
            mask.get_or_create_account(account.id(), account).unwrap();
        }
        assert_ne!(root_hash0, mask.merkle_root());

        mask.remove_accounts(&accounts_ids);
        assert_eq!(root_hash0, mask.merkle_root());
    }

    // "removing accounts from parent restores Merkle root"
    #[test]
    fn test_removing_accounts_from_parent_restore_root_hash() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = (0..5).map(|_| Account::rand()).collect::<Vec<_>>();
        let accounts_ids = accounts.iter().map(Account::id).collect::<Vec<_>>();
        let root_hash0 = mask.merkle_root();

        for account in accounts {
            root.get_or_create_account(account.id(), account).unwrap();
        }
        assert_ne!(root_hash0, mask.merkle_root());

        mask.remove_accounts(&accounts_ids);
        assert_eq!(root_hash0, mask.merkle_root());
    }

    // "removing accounts from parent and mask restores Merkle root"
    #[test]
    fn test_removing_accounts_from_parent_and_mask_restore_root_hash() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = (0..10).map(|_| Account::rand()).collect::<Vec<_>>();
        let (accounts_parent, accounts_mask) = accounts.split_at(5);
        let accounts_ids = accounts.iter().map(Account::id).collect::<Vec<_>>();

        let root_hash0 = mask.merkle_root();

        for account in accounts_parent {
            root.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }
        for account in accounts_mask {
            mask.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }
        assert_ne!(root_hash0, mask.merkle_root());

        mask.remove_accounts(&accounts_ids);
        assert_eq!(root_hash0, mask.merkle_root());
    }

    // "fold of addition over account balances in parent and mask"
    #[test]
    fn test_fold_of_addition_over_account_balance_in_parent_and_mask() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = (0..10).map(|_| Account::rand()).collect::<Vec<_>>();
        let balance = accounts
            .iter()
            .fold(0u128, |acc, account| acc + account.balance.as_u64() as u128);

        let (accounts_parent, accounts_mask) = accounts.split_at(5);

        for account in accounts_parent {
            root.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }
        for account in accounts_mask {
            mask.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let retrieved_balance =
            mask.fold(0u128, |acc, account| acc + account.balance.as_u64() as u128);
        assert_eq!(balance, retrieved_balance);
    }

    fn create_existing_account(mask: &mut Mask, account: Account) {
        match mask
            .get_or_create_account(account.id(), account.clone())
            .unwrap()
        {
            GetOrCreated::Added(_) => panic!("Should add an existing account"),
            GetOrCreated::Existed(addr) => {
                mask.set(addr, Box::new(account));
            }
        }
    }

    // "masking in to_list"
    #[test]
    fn test_masking_in_to_list() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let mut accounts = (0..10).map(|_| Account::rand()).collect::<Vec<_>>();
        // Make balances non-zero
        let one = Balance::from_u64(1);
        accounts
            .iter_mut()
            .for_each(|account| account.balance = account.balance.checked_add(&one).unwrap_or(one));

        for account in &accounts {
            root.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let parent_list = root.to_list();

        // Make balances to zero for those same account
        accounts
            .iter_mut()
            .for_each(|account| account.balance = Balance::zero());

        for account in accounts {
            create_existing_account(&mut mask, account);
        }

        let mask_list = mask.to_list();

        assert_eq!(parent_list.len(), mask_list.len());
        // Same accounts and order
        assert_eq!(
            parent_list.iter().map(Account::id).collect::<Vec<_>>(),
            mask_list.iter().map(Account::id).collect::<Vec<_>>(),
        );
        // Balances of mask are zero
        assert_eq!(
            mask_list
                .iter()
                .fold(0u128, |acc, account| acc + account.balance.as_u64() as u128),
            0
        );
    }

    // "masking in foldi"
    #[test]
    fn test_masking_in_to_foldi() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let mut accounts = (0..10).map(|_| Account::rand()).collect::<Vec<_>>();
        // Make balances non-zero
        let one = Balance::from_u64(1);
        accounts
            .iter_mut()
            .for_each(|account| account.balance = account.balance.checked_add(&one).unwrap_or(one));

        for account in &accounts {
            root.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let parent_sum_balance =
            root.fold(0u128, |acc, account| acc + account.balance.as_u64() as u128);
        assert_ne!(parent_sum_balance, 0);

        // Make balances to zero for those same account
        accounts
            .iter_mut()
            .for_each(|account| account.balance = Balance::zero());

        for account in accounts {
            create_existing_account(&mut mask, account);
        }

        let mask_sum_balance =
            mask.fold(0u128, |acc, account| acc + account.balance.as_u64() as u128);
        assert_eq!(mask_sum_balance, 0);
    }

    // "create_empty doesn't modify the hash"
    #[test]
    fn test_create_empty_doesnt_modify_the_hash() {
        let (root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let start_hash = mask.merkle_root();

        let account = Account::empty();
        mask.get_or_create_account(account.id(), account).unwrap();

        assert_eq!(mask.num_accounts(), 1);
        assert_eq!(start_hash, mask.merkle_root());
    }

    // "reuse of locations for removed accounts"
    #[test]
    fn test_reuse_of_locations_for_removed_accounts() {
        let (root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = (0..10).map(|_| Account::rand()).collect::<Vec<_>>();
        let accounts_ids = accounts.iter().map(Account::id).collect::<Vec<_>>();

        assert!(mask.last_filled().is_none());
        for account in accounts {
            mask.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }
        assert!(mask.last_filled().is_some());

        mask.remove_accounts(&accounts_ids);
        assert!(mask.last_filled().is_none());
    }

    // "num_accounts for unique keys in mask and parent"
    #[test]
    fn test_num_accounts_for_unique_keys_in_mask_and_parent() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let accounts = (0..10).map(|_| Account::rand()).collect::<Vec<_>>();

        for account in &accounts {
            mask.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let mask_num_accounts_before = mask.num_accounts();

        // Add same accounts to parent
        for account in &accounts {
            root.get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let parent_num_accounts = root.num_accounts();
        let mask_num_accounts_after = mask.num_accounts();

        assert_eq!(accounts.len(), parent_num_accounts);
        assert_eq!(parent_num_accounts, mask_num_accounts_before);
        assert_eq!(parent_num_accounts, mask_num_accounts_after);
    }

    // "Mask reparenting works"
    #[test]
    fn test_mask_reparenting_works() {
        let (mut root, mut layer1, mut layer2) = new_chain(DEPTH);

        let acc1 = Account::rand();
        let acc2 = Account::rand();
        let acc3 = Account::rand();

        let loc1 = root.get_or_create_account(acc1.id(), acc1).unwrap().addr();
        let loc2 = layer1
            .get_or_create_account(acc2.id(), acc2)
            .unwrap()
            .addr();
        let loc3 = layer2
            .get_or_create_account(acc3.id(), acc3)
            .unwrap()
            .addr();

        // All accounts are accessible from layer2
        assert!(layer2.get(loc1.clone()).is_some());
        assert!(layer2.get(loc2.clone()).is_some());
        assert!(layer2.get(loc3.clone()).is_some());

        // acc1 is in root
        assert!(root.get(loc1.clone()).is_some());

        layer1.commit();

        // acc2 is in root
        assert!(root.get(loc2.clone()).is_some());

        layer1.remove_and_reparent();

        // acc1, acc2 are in root
        assert!(root.get(loc1.clone()).is_some());
        assert!(root.get(loc2.clone()).is_some());

        // acc3 not in root
        assert!(root.get(loc3.clone()).is_none());

        // All accounts are accessible from layer2
        assert!(layer2.get(loc1).is_some());
        assert!(layer2.get(loc2).is_some());
        assert!(layer2.get(loc3).is_some());
    }

    // "setting an account in the parent doesn't remove the masked
    // copy if the mask is still dirty for that account"
    #[test]
    fn test_set_account_in_parent_doesnt_remove_if_mask_is_dirty() {
        let (mut root, mask) = new_instances(DEPTH);
        let mut mask = root.register_mask(mask);

        let mut account = Box::new(Account::rand());
        let mut account2 = account.clone();

        account.balance = Balance::from_u64(10);
        account2.balance = Balance::from_u64(5);

        let loc = mask
            .get_or_create_account(account.id(), *account.clone())
            .unwrap()
            .addr();

        root.set(loc.clone(), account2);

        assert_eq!(mask.get(loc).unwrap(), account);
    }

    // "get_all_accounts should preserve the ordering of accounts by
    // location with noncontiguous updates of accounts on the mask"
    #[test]
    fn test_get_all_accounts_should_preserve_ordering() {
        let (_root, mut layer1, mut layer2) = new_chain(DEPTH);

        let accounts = make_full_accounts(DEPTH);

        for account in &accounts {
            layer1
                .get_or_create_account(account.id(), account.clone())
                .unwrap();
        }

        let mut updated_accounts = accounts.clone();
        let mut rng = thread_rng();
        let mut nmodified = 0;

        for account in updated_accounts.iter_mut() {
            if rng.gen::<u8>() >= 150 {
                continue;
            }
            account.balance = rng.gen();

            create_existing_account(&mut layer2, account.clone());
            nmodified += 1;
        }

        assert!(nmodified > 0);
        assert_eq!(
            updated_accounts
                .into_iter()
                .map(Box::new)
                .collect::<Vec<_>>(),
            layer2
                .get_all_accounts_rooted_at(Address::root())
                .unwrap()
                .into_iter()
                .map(|(_, account)| account)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            accounts.into_iter().map(Box::new).collect::<Vec<_>>(),
            layer1
                .get_all_accounts_rooted_at(Address::root())
                .unwrap()
                .into_iter()
                .map(|(_, account)| account)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_validate_inner_hashes() {
        let l = Address::first(1);
        assert_eq!(l.parent().unwrap(), Address::root());
        assert_eq!(l.parent(), l.next().unwrap().parent());
        let (root, layer1, layer2) = new_chain(DEPTH);

        let accounts = make_full_accounts(DEPTH);

        for (i, mut mask) in [root, layer1, layer2].into_iter().enumerate() {
            for account in accounts.iter().skip(i) {
                mask.get_or_create_account(account.id(), account.clone())
                    .unwrap();
            }
            dbg!(mask.merkle_root());
            mask.validate_inner_hashes().unwrap();
        }
    }

    #[test]
    fn test_nmasks_to_root() {
        let (root, layer1, layer2) = new_chain(DEPTH);
        assert_eq!(root.nmasks_to_root(), 0);
        assert_eq!(layer1.nmasks_to_root(), 1);
        assert_eq!(layer2.nmasks_to_root(), 2);

        let mut mask = layer2;
        for index in 0..300 {
            assert_eq!(mask.nmasks_to_root(), 2 + index);
            mask = mask.make_child();
        }
    }
}

/// Masks over a ledger that changes under them, and chains of masks committed
/// up to a base, as the root snarked ledger, the root staged ledger's mask over
/// it and the masks of the blocks after it are (R38).
#[cfg(test)]
mod tests_mask_chain {
    use std::collections::BTreeMap;

    use crate::scan_state::currency::Balance;

    use super::*;

    #[cfg(target_family = "wasm")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    const DEPTH: usize = 4;
    const SIZE: u64 = 1 << DEPTH;

    /// What a ledger should hold: accounts by index.
    type View = BTreeMap<u64, Account>;

    fn addr(index: u64) -> Address {
        Address::from_index(AccountIndex(index), DEPTH)
    }

    /// A database of `n` random accounts, and what it holds.
    fn database(n: u64) -> (Mask, View) {
        let mut db = Mask::new_root(Database::create(DEPTH as u8));
        let mut view = View::new();
        for index in 0..n {
            let account = Account::rand();
            let at = db
                .get_or_create_account(account.id(), account.clone())
                .unwrap()
                .addr();
            assert_eq!(at, addr(index));
            view.insert(index, account);
        }
        (db, view)
    }

    /// Gives the accounts at `indexes` of `view` new balances in `ledger`
    /// and returns what `ledger` then holds.
    fn set_balances(ledger: &mut Mask, view: &View, balances: &[(u64, u64)]) -> View {
        let mut view = view.clone();
        for (index, balance) in balances {
            let account = view.get_mut(index).unwrap();
            account.balance = Balance::from_u64(*balance);
            ledger.set(addr(*index), Box::new(account.clone()));
        }
        view
    }

    /// Creates an account in `ledger`, at the next free index.
    fn create(ledger: &mut Mask, view: &View) -> View {
        let mut view = view.clone();
        let account = Account::rand();
        let at = ledger
            .get_or_create_account(account.id(), account.clone())
            .unwrap()
            .addr();
        view.insert(at.to_index().as_u64(), account);
        view
    }

    /// Reads every hash of `ledger`, so that it caches what it reads from its
    /// parents.
    fn warm(ledger: &mut Mask) {
        ledger.merkle_root();
        for index in 0..SIZE {
            ledger.merkle_path(addr(index));
        }
    }

    /// `ledger` reads the accounts, root hash and Merkle paths of a database
    /// holding `view`.
    fn assert_reads(name: &str, ledger: &Mask, view: &View) {
        let mut ledger = ledger.clone();
        let mut expected = Mask::new_root(Database::create(DEPTH as u8));
        for (index, account) in view {
            expected.set(addr(*index), Box::new(account.clone()));
        }
        for index in 0..SIZE {
            assert_eq!(
                ledger.get(addr(index)).map(|account| *account),
                view.get(&index).cloned(),
                "{name}: account {index}"
            );
        }
        assert_eq!(ledger.merkle_root(), expected.merkle_root(), "{name}: root");
        for index in 0..SIZE {
            assert_eq!(
                ledger.merkle_path(addr(index)),
                expected.merkle_path(addr(index)),
                "{name}: path {index}"
            );
        }
    }

    /// The root snarked ledger changes under the root staged ledger's mask
    /// and the masks of the blocks after it, as `compact_snarked_ledgers`
    /// commits the new snarked ledger into it: every mask reads what it did,
    /// except at an index that neither it nor a mask between it and the
    /// database holds. Notifying every descendant made `block2` prune index 0
    /// (the database now has its account) and read the root's instead.
    #[test]
    fn a_database_changing_under_a_chain_of_masks() {
        let (mut db, base) = database(10);
        let mut root = db.make_child();
        let root_view = set_balances(&mut root, &base, &[(0, 100), (1, 101), (2, 102), (3, 103)]);
        let mut block1 = root.make_child();
        let block1_view = set_balances(&mut block1, &root_view, &[(2, 202), (4, 204), (5, 205)]);
        let mut block2 = block1.make_child();
        let block2_view = set_balances(&mut block2, &block1_view, &[(0, 300), (4, 304), (6, 306)]);
        let block2_view = create(&mut block2, &block2_view);
        let mut fork = root.make_child();
        let fork_view = set_balances(&mut fork, &root_view, &[(1, 401), (7, 407)]);
        for ledger in [&mut db, &mut root, &mut block1, &mut block2, &mut fork] {
            warm(ledger);
        }

        // 0: as `block2` has it, `root` has another. 1: as `root` has it,
        // `fork` has another. 5: `block1` has its own. 7: `fork` has its own.
        // 8: nobody has its own.
        let mut snarked = db.make_child();
        let new_base = set_balances(
            &mut snarked,
            &base,
            &[(0, 300), (1, 101), (5, 605), (7, 507), (8, 508)],
        );
        snarked.commit();
        snarked.remove_and_reparent();

        let changed = |view: &View| {
            let mut view = view.clone();
            for index in [5, 7, 8] {
                if view[&index].balance == base[&index].balance {
                    view.insert(index, new_base[&index].clone());
                }
            }
            view
        };
        assert_reads("db", &db, &new_base);
        assert_reads("root", &root, &changed(&root_view));
        assert_reads("block1", &block1, &changed(&block1_view));
        assert_reads("block2", &block2, &changed(&block2_view));
        assert_reads("fork", &fork, &changed(&fork_view));
        // `root` keeps the account the database now holds too (PRUNE-1).
        assert!(root.test_is_in_mask(&addr(1)));
        assert!(root.test_is_in_mask(&addr(0)));
    }

    /// A chain committed up to a base: the new root's mask (`block2`) into
    /// the old root's (`root`), whose parent is the snarked ledger, which
    /// keeps its hash. The masks of the blocks after the new root move to the
    /// root's mask and read what they did.
    #[test]
    fn commit_and_reparent_to_base_stops_under_the_base() {
        let (mut db, base) = database(8);
        let mut root = db.make_child();
        let root_view = set_balances(&mut root, &base, &[(0, 100), (1, 101)]);
        let mut block1 = root.make_child();
        let block1_view = set_balances(&mut block1, &root_view, &[(1, 201), (2, 202)]);
        let mut block2 = block1.make_child();
        let block2_view = set_balances(&mut block2, &block1_view, &[(2, 302), (3, 303)]);
        let block2_view = create(&mut block2, &block2_view);
        let mut next = block2.make_child();
        let next_view = set_balances(&mut next, &block2_view, &[(0, 400), (4, 404)]);
        let mut other = block2.make_child();
        let other_view = set_balances(&mut other, &block2_view, &[(0, 500)]);
        for ledger in [
            &mut db,
            &mut root,
            &mut block1,
            &mut block2,
            &mut next,
            &mut other,
        ] {
            warm(ledger);
        }

        let db_uuid = db.get_uuid();
        let new_root = block2.commit_and_reparent_to_base(|parent| parent.get_uuid() == db_uuid);

        assert_eq!(new_root.get_uuid(), root.get_uuid());
        assert_eq!(new_root.get_parent().unwrap().get_uuid(), db_uuid);
        assert!(!block1.is_attached());
        assert!(!block2.is_attached());
        for child in [&next, &other] {
            assert_eq!(child.get_parent().unwrap().get_uuid(), root.get_uuid());
        }
        assert_reads("db", &db, &base);
        assert_reads("new root", &new_root, &block2_view);
        assert_reads("next", &next, &next_view);
        assert_reads("other", &other, &other_view);

        // Already at the base, or a root: nothing to do.
        let same = new_root.commit_and_reparent_to_base(|parent| parent.get_uuid() == db_uuid);
        assert_eq!(same.get_uuid(), root.get_uuid());
        assert_eq!(db.commit_and_reparent_to_base(|_| true).get_uuid(), db_uuid);
        assert_reads("new root again", &same, &block2_view);
    }

    /// With no base among its parents the chain goes into the database, as
    /// with `commit_and_reparent_to_root`: a staged ledger over a database of
    /// its own, as the genesis staged ledger's copy is.
    #[test]
    fn commit_and_reparent_to_base_stops_at_a_root() {
        let (db, base) = database(6);
        let mut block1 = db.make_child();
        let block1_view = set_balances(&mut block1, &base, &[(0, 100), (1, 101)]);
        let mut block2 = block1.make_child();
        let block2_view = set_balances(&mut block2, &block1_view, &[(1, 201), (2, 202)]);
        let mut next = block2.make_child();
        let next_view = set_balances(&mut next, &block2_view, &[(3, 303)]);

        let new_root = block2.commit_and_reparent_to_base(|_| false);

        assert_eq!(new_root.get_uuid(), db.get_uuid());
        assert!(new_root.is_root());
        assert!(!block1.is_attached());
        assert!(!block2.is_attached());
        assert_eq!(next.get_parent().unwrap().get_uuid(), db.get_uuid());
        assert_reads("db", &db, &block2_view);
        assert_reads("next", &next, &next_view);
    }

    /// A mask over a database whose accounts are updated in place or new
    /// (I5d5's restored root staged ledger) passes; one that puts an account
    /// the database holds somewhere else, or another account where the
    /// database holds one, fails, though each lookup through the mask alone
    /// would agree with its leaves.
    #[test]
    fn child_account_indexes_agree_with_the_parent() {
        let (db, base) = database(6);
        assert!(db.validate_child_account_indexes().is_err());
        let mut staged = db.make_child();
        let view = set_balances(&mut staged, &base, &[(1, 101), (4, 104)]);
        create(&mut staged, &view);
        assert_eq!(staged.validate_child_account_indexes(), Ok(3));
        assert_eq!(db.validate_account_indexes(), Ok(6));
        // Nothing of its own: nothing to check.
        assert_eq!(db.make_child().validate_child_account_indexes(), Ok(0));

        // The database's accounts 0 and 1, swapped.
        let mut swapped = db.make_child();
        swapped.set(addr(0), Box::new(base[&1].clone()));
        swapped.set(addr(1), Box::new(base[&0].clone()));
        let error = swapped.validate_child_account_indexes().unwrap_err();
        assert!(error.contains("id-to-address"), "{error}");
        assert_eq!(swapped.location_of_account(&base[&1].id()), Some(addr(0)));

        // Another account over one the database holds.
        let mut replaced = db.make_child();
        replaced.set(addr(2), Box::new(Account::rand()));
        let error = replaced.validate_child_account_indexes().unwrap_err();
        assert!(error.contains("id-to-address"), "{error}");

        // An account the database holds, again past its last account.
        let mut duplicated = db.make_child();
        duplicated.set(addr(6), Box::new(base[&3].clone()));
        let error = duplicated.validate_child_account_indexes().unwrap_err();
        assert!(error.contains("id-to-address"), "{error}");
    }

    #[test]
    fn reclamation_keeps_equal_pending_accounts() {
        let (mut db, base) = database(6);
        db.set_token_owners();
        let mut staged = db.make_child();
        staged.hold_parent_accounts([base[&0].id(), base[&5].id()]);
        let view = set_balances(&mut staged, &base, &[(2, 102)]);
        let child = staged.make_child();
        warm(&mut db);
        warm(&mut staged);
        let pending = HashSet::from([base[&0].id()]);
        assert_eq!(staged.reclaim_unreferenced_accounts(&pending), 1);
        assert_eq!(
            staged.own_account_indexes(),
            vec![AccountIndex(0), AccountIndex(2)]
        );
        assert_eq!(staged.validate_child_account_indexes(), Ok(2));
        assert_reads("reclaimed", &staged, &view);
        assert_reads("child", &child, &view);
        // A later partial proof must still be hidden by the pending account.
        set_balances(&mut db, &base, &[(0, 500)]);
        assert_reads("pending after parent change", &staged, &view);
        assert_reads("child after parent change", &child, &view);
    }

    #[test]
    fn reclamation_drops_equal_finalized_accounts_without_disk_reads() {
        let dir = tempfile::tempdir().unwrap();
        let (mut db, base) = database(6);
        db.set_token_owners();
        let mut staged = db.make_child();
        staged.hold_parent_accounts(base.values().map(Account::id));
        let mut child = staged.make_child();
        warm(&mut staged);
        warm(&mut child);
        let root = staged.merkle_root();
        db.freeze_to_disk(dir.path()).unwrap().unwrap();
        assert_eq!(db.disk_cached(), Some(0));
        assert_eq!(staged.reclaim_unreferenced_accounts(&HashSet::new()), 6);
        assert_eq!(staged.num_own_accounts(), 0);
        assert!(staged.get_raw_inner_hashes().is_empty());
        assert_eq!(staged.merkle_root(), root);
        assert_eq!(child.merkle_root(), root);
        // The cold account cache remains empty: only resident hashes were read.
        assert_eq!(db.disk_cached(), Some(0));
        assert_eq!(staged.validate_child_account_indexes(), Ok(0));
        assert_reads("finalized", &staged, &base);
        assert_reads("child", &child, &base);
        // Index removal must not lose the parent's last location or tokens.
        assert_eq!(staged.last_filled(), db.last_filled());
        for account in base.values() {
            assert_eq!(
                staged.token_owner(account.id().derive_token_id()),
                db.token_owner(account.id().derive_token_id())
            );
        }
        set_balances(&mut db, &base, &[(5, 505)]);
        let mut changed = base.clone();
        changed.get_mut(&5).unwrap().balance = Balance::from_u64(505);
        assert_reads("inherited after reclaim", &staged, &changed);
        assert_reads("child after reclaim", &child, &changed);
    }

    #[test]
    fn reclamation_keeps_accounts_when_parent_leaf_hash_is_missing() {
        let (mut db, base) = database(6);
        warm(&mut db);
        let mut parent = db.make_child();
        let parent_view = set_balances(&mut parent, &base, &[(0, 700)]);
        // The parent's owned leaf has no cached hash. It must not fall
        // through to the old database hash, which equals the staged value.
        let mut staged = parent.make_child();
        staged.set(addr(0), Box::new(base[&0].clone()));
        assert_eq!(staged.reclaim_unreferenced_accounts(&HashSet::new()), 0);
        assert_eq!(staged.num_own_accounts(), 1);
        assert_reads("missing hash", &staged, &base);
        assert_reads("parent", &parent, &parent_view);
    }

    /// A mask holding its own copy of accounts it reads from its database
    /// (I5d5): what it reads and its hashes stay as they were; a later change
    /// of a held account in the database, folded in from a snarked mask as
    /// `compact_snarked_ledgers` does, doesn't show through, and one to the
    /// same account leaves it held; an account it doesn't hold does show
    /// through.
    #[test]
    fn held_parent_accounts_stop_the_parent_showing_through() {
        let (db, base) = database(6);
        let mut staged = db.make_child();
        let view = set_balances(&mut staged, &base, &[(1, 101)]);
        warm(&mut staged);
        let mut unknown = Account::rand();
        unknown.balance = Balance::from_u64(9);
        let ids = [0, 1, 2, 4].map(|index| view[&index].id());
        let held = staged.hold_parent_accounts(ids.clone().into_iter().chain([unknown.id()]));
        assert_eq!(held, 3);
        assert_eq!(staged.hold_parent_accounts(ids.clone()), 0);
        assert_eq!(
            staged
                .own_account_indexes()
                .into_iter()
                .map(|index| index.as_u64())
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 4]
        );
        assert_eq!(staged.validate_child_account_indexes(), Ok(4));
        assert_reads("held", &staged, &view);

        // A snarked ledger part way: 0 as the staged mask holds it, 2 and 3
        // otherwise.
        let mut snarked = db.make_child();
        let mut snarked_view = set_balances(&mut snarked, &base, &[(2, 502), (3, 503)]);
        snarked.set(addr(0), Box::new(view[&0].clone()));
        snarked_view.insert(0, view[&0].clone());
        snarked.commit();
        snarked.remove_and_reparent();
        assert_reads("db", &db, &snarked_view);
        // 2 is held: unchanged; 3 isn't: the database's.
        let mut expected = view.clone();
        expected.insert(3, snarked_view[&3].clone());
        assert_reads("staged", &staged, &expected);
        // 0 was the same: still held, with 1, 2 and 4.
        assert_eq!(
            staged
                .own_account_indexes()
                .into_iter()
                .map(|index| index.as_u64())
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 4]
        );
        assert_eq!(db.hold_parent_accounts(ids), 0);
    }
}
