use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

use mina_curves::pasta::Fp;
use mina_signer::CompressedPubKey;

use crate::{
    account::{Account, AccountId, TokenId},
    address::{Address, AddressIterator, Direction},
    base::{AccountIndex, BaseLedger, GetOrCreated, MerklePath, Uuid},
    database::{Database, DatabaseError},
    mask::UnregisterBehavior,
    next_uuid,
    tree_version::{TreeVersion, V2},
    HashesMatrix,
};

use super::Mask;

pub enum MaskImpl {
    Root {
        database: Database<V2>,
        childs: HashMap<Uuid, Mask>,
    },
    Attached {
        parent: Mask,
        /// Sorted, so whether the mask has accounts under an address is a
        /// range lookup ([`owns_accounts_under`]).
        owning_account: BTreeMap<AccountIndex, Account>,
        token_owners: Option<HashMap<TokenId, AccountId>>,
        id_to_addr: HashMap<AccountId, Address>,
        last_location: Option<Address>,
        depth: u8,
        childs: HashMap<Uuid, Mask>,
        hashes: HashesMatrix,
        uuid: Uuid,
    },
    Unattached {
        depth: u8,
        childs: HashMap<Uuid, Mask>,
        /// Sorted, so whether the mask has accounts under an address is a
        /// range lookup ([`owns_accounts_under`]).
        owning_account: BTreeMap<AccountIndex, Account>,
        token_owners: Option<HashMap<TokenId, AccountId>>,
        id_to_addr: HashMap<AccountId, Address>,
        last_location: Option<Address>,
        hashes: HashesMatrix,
        uuid: Uuid,
    },
}

impl Drop for MaskImpl {
    fn drop(&mut self) {
        if self.uuid().starts_with("temporary") {
            return;
        }
        super::alive_remove(&self.get_uuid());
    }
}

impl Clone for MaskImpl {
    fn clone(&self) -> Self {
        // Copy ledger contents, not ownership edges: existing children still
        // have the original mask as their parent.
        // OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/merkle_mask/masking_merkle_tree.ml#L642-L659>
        let copy = match self {
            Self::Root { database, .. } => Self::Root {
                database: database.clone_db(database.get_directory().unwrap()),
                childs: HashMap::with_capacity(2),
            },
            Self::Attached {
                parent,
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                depth,
                childs: _,
                hashes,
                uuid: _,
            } => Self::Attached {
                parent: parent.clone(),
                owning_account: owning_account.clone(),
                token_owners: token_owners.clone(),
                id_to_addr: id_to_addr.clone(),
                last_location: last_location.clone(),
                depth: *depth,
                childs: HashMap::with_capacity(2),
                hashes: hashes.clone(),
                uuid: next_uuid(),
            },
            Self::Unattached {
                depth,
                childs: _,
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                hashes,
                uuid: _,
            } => Self::Unattached {
                depth: *depth,
                childs: HashMap::with_capacity(2),
                owning_account: owning_account.clone(),
                token_owners: token_owners.clone(),
                id_to_addr: id_to_addr.clone(),
                last_location: last_location.clone(),
                hashes: hashes.clone(),
                uuid: next_uuid(),
            },
        };
        super::alive_add(&copy.uuid());
        copy
    }
}

impl std::fmt::Debug for MaskImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Root { database, childs } => f
                .debug_struct("Root")
                .field("database_uuid", &database.get_uuid())
                .field("database", &database)
                .field("childs", &childs.len())
                .finish(),
            Self::Attached {
                parent,
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                depth,
                childs,
                hashes,
                uuid,
            } => f
                .debug_struct("Attached")
                .field("uuid", uuid)
                .field("parent", &parent.get_uuid())
                .field("owning_account", &owning_account.len())
                .field(
                    "token_owners",
                    &token_owners.as_ref().map(|to| to.len()).unwrap_or(0),
                )
                .field("id_to_addr", &id_to_addr.len())
                .field("last_location", last_location)
                .field("depth", depth)
                .field("num_accounts", &self.num_accounts())
                .field("childs", &childs.len())
                .field("hashes_matrix", &hashes)
                .finish(),
            Self::Unattached {
                depth,
                childs,
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                hashes,
                uuid,
            } => f
                .debug_struct("Unattached")
                .field("depth", depth)
                .field("childs", &childs.len())
                .field("owning_account", &owning_account.len())
                .field(
                    "token_owners",
                    &token_owners.as_ref().map(|to| to.len()).unwrap_or(0),
                )
                .field("id_to_addr", &id_to_addr.len())
                .field("last_location", last_location)
                .field("uuid", uuid)
                .field("hashes_matrix", &hashes)
                .finish(),
        }
    }
}

use MaskImpl::*;

/// For debug purpose only
#[derive(Debug)]
pub enum MaskImplShort {
    Root______(Uuid),
    Attached__(Uuid),
    Unattached(Uuid),
}

impl MaskImpl {
    /// For debug purpose only
    pub fn short(&self) -> MaskImplShort {
        match self {
            Root { database, .. } => MaskImplShort::Root______(database.get_uuid()),
            Attached { uuid, .. } => MaskImplShort::Attached__(uuid.clone()),
            Unattached { uuid, .. } => MaskImplShort::Unattached(uuid.clone()),
        }
    }

    pub fn is_root(&self) -> bool {
        match self {
            Root { .. } => true,
            Attached { .. } | Unattached { .. } => false,
        }
    }

    pub fn is_attached(&self) -> bool {
        match self {
            Attached { .. } => true,
            Root { .. } | Unattached { .. } => false,
        }
    }

    pub(super) fn any_child_alive(&self) -> bool {
        let childs = match self {
            Root { childs, .. } => childs,
            Attached { childs, .. } => childs,
            Unattached { childs, .. } => childs,
        };

        !childs.is_empty()
    }

    pub fn set_token_owners(&mut self) {
        match self {
            Root { database, .. } => database.set_token_owners(),
            Attached { token_owners, .. } | Unattached { token_owners, .. } => {
                if token_owners.is_none() {
                    *token_owners = Some(Default::default());
                }
            }
        }
    }

    pub fn unset_token_owners(&mut self) {
        match self {
            Root { database, .. } => {
                database.unset_token_owners();
            }
            Attached { token_owners, .. } | Unattached { token_owners, .. } => {
                *token_owners = None;
            }
        }
    }

    pub fn has_token_owners(&self) -> bool {
        match self {
            Root { database, .. } => database.has_token_owners(),
            Attached { token_owners, .. } => token_owners.is_some(),
            Unattached { token_owners, .. } => token_owners.is_some(),
        }
    }

    /// Make `mask` a child of `self`
    pub fn register_mask(&mut self, self_mask: Mask, mask: Mask) -> Mask {
        let childs = self.childs();

        let old = childs.insert(mask.get_uuid(), mask.clone());
        assert!(old.is_none(), "mask is already registered");

        let parent_last_filled = self.last_filled();

        mask.set_parent(self_mask, Some(parent_last_filled));
        mask
    }

    /// Detach this mask from its parent
    pub fn unregister_mask(&mut self, behavior: UnregisterBehavior, remove_from_parent: bool) {
        use UnregisterBehavior::*;

        let parent = match self.get_parent() {
            Some(parent) => parent,
            None => return,
        };

        let trigger_detach_signal = matches!(behavior, Check | Recursive);

        match behavior {
            Check => {
                assert!(
                    self.childs().is_empty(),
                    "mask has {} children that must be unregistered first",
                    self.childs().len()
                );
            }
            IPromiseIAmReparentingThisMask => (),
            Recursive => {
                for child in self.childs().values_mut() {
                    child.unregister_mask_impl(Recursive, false);
                }
            }
        }

        // Remove only when our parent is not unregistering us
        if remove_from_parent {
            let removed = parent.remove_child_uuid(self.uuid());
            assert!(removed.is_some(), "Mask not a child of the parent");
        }

        self.unset_parent(trigger_detach_signal);
    }

    pub fn remove_and_reparent(&mut self) -> Option<Mask> {
        // let root_hash = self.merkle_root();

        let (parent, childs, uuid) = match self {
            Root { .. } => panic!("Cannot reparent a root mask"),
            Unattached { .. } => panic!("Cannot reparent a unattached mask"),
            Attached {
                parent,
                childs,
                uuid,
                ..
            } => (parent, childs, uuid.clone()),
        };

        let childs = std::mem::take(childs);

        // we can only reparent if merkle roots are the same
        // assert_eq!(parent.merkle_root(), root_hash);

        parent
            .remove_child_uuid(uuid)
            .expect("Parent doesn't have this mask as child");

        for child in childs.values() {
            child.remove_parent();
            parent.register_mask(child.clone());
        }

        self.remove_parent()
    }

    pub fn set_parent(&mut self, parent: Mask, parent_last_filled: Option<Option<Address>>) {
        match self {
            Root { .. } => panic!("set_parent() on a root"),
            Attached { .. } => panic!("mask is already attached"),
            Unattached {
                depth,
                childs,
                uuid,
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                hashes,
            } => {
                use std::mem::{replace, take};

                *self = Attached {
                    parent,
                    owning_account: take(owning_account),
                    token_owners: take(token_owners),
                    id_to_addr: take(id_to_addr),
                    last_location: take(last_location),
                    depth: *depth,
                    childs: take(childs),
                    hashes: replace(hashes, HashesMatrix::new(*depth as usize)),
                    uuid: replace(uuid, "temporary_set_parent".to_string()),
                };

                let last_filled = match parent_last_filled {
                    Some(last_filled) => last_filled,
                    None => self.last_filled(), // This will lock the parent,
                };

                if let Attached { last_location, .. } = self {
                    // Reparenting must retain accounts beyond the parent's
                    // last location, just as Attached.last_filled does.
                    // OCaml: <https://github.com/MinaProtocol/mina/blob/8250f62a9bb6450f6c9fdc9a3af72da62c2f5e39/src/lib/merkle_mask/masking_merkle_tree.ml#L1038-L1043>
                    *last_location = last_location.take().max(last_filled);
                };
            }
        }
        self.invalidate_hash_caches();
    }

    /// A different parent changes inherited subtrees in this mask and every
    /// descendant. Invalidate at topology changes, with no extra hash-read work.
    fn invalidate_hash_caches(&mut self) {
        match self {
            Root { .. } => unreachable!("root masks cannot change parent"),
            Attached { hashes, .. } | Unattached { hashes, .. } => hashes.invalidate_all(),
        }
        for child in self.childs().values() {
            child.with(Self::invalidate_hash_caches);
        }
    }

    fn uuid(&self) -> Uuid {
        self.get_uuid()
    }

    pub fn get_parent(&self) -> Option<Mask> {
        match self {
            Root { .. } | Unattached { .. } => None,
            Attached { parent, .. } => Some(parent.clone()),
        }
    }

    pub fn unset_parent(&mut self, trigger_detach_signal: bool) {
        let parent = self.remove_parent();

        assert!(
            parent.is_some(),
            "unset_parent called on a non-attached mask"
        );

        if trigger_detach_signal {
            // TODO: Async.Ivar.fill_if_empty t.detached_parent_signal () ;
        }
    }

    pub fn nmasks_to_root(&self) -> usize {
        match self {
            Root { .. } => 0,
            Attached { parent, .. } => 1 + parent.with(|parent| parent.nmasks_to_root()),
            Unattached { .. } => panic!(),
        }
    }

    /// get hash from mask, if present, else from its parent
    pub fn get_hash(&mut self, addr: Address) -> Option<Fp> {
        self.get_inner_hash_at_addr(addr).ok()
    }

    /// commit all state to the parent, flush state locally
    pub fn commit(&mut self) {
        let depth = self.depth() as usize;
        let self_uuid = self.uuid();
        // let old_root_hash = self.merkle_root();

        match self {
            Root { .. } => panic!("commit on a root"),
            Unattached { .. } => panic!("commit on a unattached mask"),
            Attached {
                parent,
                owning_account,
                token_owners,
                id_to_addr,
                hashes,
                ..
            } => {
                assert_ne!(parent.get_uuid(), self_uuid);

                let (accounts, hashes) = {
                    if let Some(to) = token_owners.as_mut() {
                        to.clear();
                    }
                    id_to_addr.clear();
                    (std::mem::take(owning_account), hashes.take())
                };

                for (index, account) in accounts {
                    let addr = Address::from_index(index, depth);
                    parent.set_impl(addr, Box::new(account), Some(self_uuid.clone()));
                }

                parent.transfert_hashes(hashes);

                // Parent merkle root after committing should be the same as the \
                // old one in the mask
                // assert_eq!(old_root_hash, parent.merkle_root()); // TODO: Assert this only in #[cfg(test)]
            }
        }
    }

    /// Makes this attached mask, whose parent is a root, a root of its own:
    /// a copy of the parent's database with this mask's accounts and hashes
    /// committed into it, as [`Self::commit`] commits them into the parent.
    /// The parent is unchanged and loses this child; this mask keeps its uuid
    /// and its children, which see the same ledger. For a frozen parent the
    /// copy shares its file, and the accounts go to the copy's overlay
    /// (PC3b), so the mask's accounts and the hashes on their paths leave the
    /// heap for what the overlay costs. The number of accounts moved, or None
    /// if this isn't an attached mask over a root.
    ///
    /// OCaml keeps the root snarked ledger as a database of its own and takes
    /// the next epoch ledger as a checkpoint (a copy) of it, so the root never
    /// reads through an epoch ledger:
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/consensus/proof_of_stake.ml#L3030-L3076>
    pub fn detach_into_root(&mut self) -> Option<usize> {
        let depth = self.depth() as usize;
        let uuid = self.uuid();
        let Attached { parent, .. } = self else {
            return None;
        };
        let database = parent.with(|parent| match parent {
            Root { database, .. } => Some(database.clone_db_with_uuid(uuid.clone())),
            _ => None,
        })?;
        // Every hash on the paths of the mask's accounts, so that the copy
        // needn't rehash them.
        self.merkle_root();
        let Attached {
            parent,
            owning_account,
            hashes,
            childs,
            ..
        } = self
        else {
            unreachable!("checked above");
        };
        let mut database = database;
        let accounts = std::mem::take(owning_account);
        let moved = accounts.len();
        for (index, account) in accounts {
            database.set(Address::from_index(index, depth), Box::new(account));
        }
        database.transfert_hashes(hashes.take());
        // A mask made by `Mask::copy` reads through its parent without being
        // registered as its child (PC3c: a sync ledger copied from a mask), so
        // there may be nothing to remove.
        parent.remove_child_uuid(uuid.clone());
        let childs = std::mem::take(childs);
        // The old value's drop takes the uuid off the alive list.
        *self = Root { database, childs };
        super::alive_add(&uuid);
        Some(moved)
    }

    pub fn commit_and_reparent(&mut self) -> Option<Mask> {
        self.commit();
        self.remove_and_reparent()
    }

    /// commit all the masks from this mask all the way upto the root
    /// and return root mask while also detaching all intermediary masks.
    pub fn commit_and_reparent_to_root(&mut self) -> Option<Mask> {
        if !self.is_attached() {
            return None;
        }

        let mut parent = self.commit_and_reparent()?;
        loop {
            let binding = parent.with(|parent| {
                if !parent.is_attached() {
                    return None;
                }
                parent.commit_and_reparent()
            });
            parent = match binding {
                Some(new_parent) => new_parent,
                None => return Some(parent),
            };
        }
    }

    /// called when parent sets an account; update local state
    ///
    /// A mask that has its own account at the index keeps it, and keeps
    /// seeing it, whatever the parent holds there, so neither it nor its
    /// descendants need to know: the notification stops there. It keeps it
    /// even when the parent's becomes the same (PRUNE-1). The root staged
    /// ledger's mask hangs from the root snarked ledger, and an account that
    /// pending transactions changed and changed back can equal the snarked
    /// one at a proof boundary; the next proof can end part way through those
    /// transactions, and a pruned mask would then read the snarked ledger's
    /// value. OCaml's `parent_set_notify` prunes, but only
    /// `Maskable_merkle_tree`'s setters call it, and no production path
    /// does: a mask commits through `Any_ledger.M.set_batch`, which on the
    /// root snarked ledger is `Ledger.Db`'s and notifies no one. Only a mask that reads the index from its parent sees a
    /// new account; it drops its cached hashes on that path and tells its
    /// children.
    ///
    /// OCaml notifies only the parent's direct children
    /// (`Maskable_merkle_tree.set`), and a mask there keeps hashes only on its
    /// own accounts' paths. Ours also cache hashes they read from their
    /// parent, so the notification goes down to every mask that reads the
    /// index through its parents, as when the root snarked ledger changes
    /// under the root staged ledger's mask and the masks of the blocks after
    /// it (R38). It used to go to every descendant: a block's mask then pruned
    /// its own account because the database now held the same one while a
    /// mask between them held another, and read that other account.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_mask/masking_merkle_tree.ml#L566-L576>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/merkle_mask/maskable_merkle_tree.ml#L210-L227>
    pub fn parent_set_notify(&mut self, account_index: AccountIndex, account: &Account) {
        assert!(self.is_attached());

        match self {
            Root { .. } => panic!("parent_set_notify on a root"),
            Unattached { .. } => panic!("parent_set_notify on an unattached"),
            Attached {
                owning_account,
                hashes,
                ..
            } => {
                if owning_account.contains_key(&account_index) {
                    return;
                }
                hashes.invalidate_hashes(account_index);
            }
        }

        for child in self.childs().values() {
            child.parent_set_notify(account_index, account)
        }
    }

    pub fn remove_parent(&mut self) -> Option<Mask> {
        match self {
            Root { .. } => panic!("remove_parent on a root"),
            Unattached { .. } => panic!("remove_parent on an unattached"),
            Attached { .. } => (),
        }

        let Self::Attached {
            parent,
            owning_account,
            token_owners,
            id_to_addr,
            last_location,
            depth,
            childs,
            hashes,
            uuid,
        } = self
        else {
            // We previously checked it's an attached mask
            unreachable!()
        };

        let parent = parent.clone();
        let owning_account = std::mem::take(owning_account);
        let depth = std::mem::take(depth);
        let childs = std::mem::take(childs);
        let token_owners = std::mem::take(token_owners);
        let id_to_addr = std::mem::take(id_to_addr);
        let last_location = std::mem::take(last_location);
        let hashes = std::mem::replace(hashes, HashesMatrix::new(depth as usize));
        let uuid = std::mem::replace(uuid, "temporary".to_string());

        *self = Self::Unattached {
            owning_account,
            token_owners,
            id_to_addr,
            last_location,
            depth,
            childs,
            hashes,
            uuid,
        };

        self.invalidate_hash_caches();
        Some(parent)
    }

    pub fn remove_child_uuid(&mut self, uuid: Uuid) -> Option<Mask> {
        self.childs().remove(&uuid)
    }

    /// Puts `mask`, attached to this mask already, among its children.
    pub fn adopt_child(&mut self, mask: Mask) {
        let old = self.childs().insert(mask.get_uuid(), mask);
        assert!(old.is_none(), "mask is already registered");
    }

    /// Makes `parent` this attached mask's parent in place of the current
    /// one, keeping its accounts, children, uuid and cached hashes, and
    /// returns the one replaced; None for a mask that isn't attached. For a
    /// parent that holds the same account as the current one at every
    /// account this mask doesn't own, which the caller checks, every cached
    /// hash stays right: a cached hash is of a path through an own account,
    /// computed from own leaves and the parent's hashes of the subtrees
    /// beside the path, which hold no own account.
    pub fn replace_parent(&mut self, parent: Mask) -> Option<Mask> {
        match self {
            Attached {
                parent: current, ..
            } => Some(std::mem::replace(current, parent)),
            Root { .. } | Unattached { .. } => None,
        }
    }

    fn childs(&mut self) -> &mut HashMap<Uuid, Mask> {
        match self {
            Root { childs, .. } => childs,
            Attached { childs, .. } => childs,
            Unattached { childs, .. } => childs,
        }
    }

    pub fn get_cached_hash(&self, addr: &Address) -> Option<Fp> {
        let matrix = match self {
            Root { database, .. } => return database.get_cached_hash(addr),
            Attached { hashes, .. } => hashes,
            Unattached { hashes, .. } => hashes,
        };

        matrix.get(addr).copied()
    }

    pub fn set_cached_hash_unchecked(&mut self, addr: &Address, hash: Fp) {
        self.set_cached_hash(addr, hash)
    }

    fn set_cached_hash(&mut self, addr: &Address, hash: Fp) {
        let matrix = match self {
            Root { database, .. } => return database.set_cached_hash(addr, hash),
            Attached { hashes, .. } => hashes,
            Unattached { hashes, .. } => hashes,
        };

        matrix.set(addr, hash);
    }

    pub fn empty_hash_at_height(&mut self, height: usize) -> Fp {
        let matrix = match self {
            Root { database, .. } => return database.empty_hash_at_height(height),
            Attached { hashes, .. } => hashes,
            Unattached { hashes, .. } => hashes,
        };

        matrix.empty_hash_at_height(height)
    }

    fn invalidate_hashes(&mut self, account_index: AccountIndex) {
        let matrix = match self {
            Root { database, .. } => return database.invalidate_hashes(account_index),
            Attached { hashes, .. } => hashes,
            Unattached { hashes, .. } => hashes,
        };

        matrix.invalidate_hashes(account_index)
    }

    pub fn compute_hash_or_parent(&mut self, addr: Address, last_account: &Address) -> Fp {
        let (matrix, own, parent, depth) = match self {
            Root { database, .. } => {
                return database.with(|db| db.emulate_tree_recursive(addr, last_account));
            }
            Attached {
                hashes,
                owning_account,
                parent,
                depth,
                ..
            } => (hashes, owning_account, Some(parent), *depth),
            Unattached {
                hashes,
                owning_account,
                depth,
                ..
            } => (hashes, owning_account, None, *depth),
        };

        if let Some(hash) = matrix.get(&addr).cloned() {
            return hash;
        }

        // Check if we have any children accounts in our mask
        // When we don't have accounts here, delegate to parent
        // An unattached mask is also a valid empty ledger. With no parent,
        // compute its empty subtrees locally instead of unwrapping None.
        let hash = if parent.is_none() || owns_accounts_under(own, &addr, depth) {
            self.emulate_tree_recursive(addr, last_account)
        } else {
            // Recurse to parents until we found a mask having accounts on this address
            let parent = parent.as_ref().unwrap();
            parent.with(|parent| parent.compute_hash_or_parent(addr.clone(), last_account))
        };

        hash
    }

    pub fn compute_hash_or_parent_for_merkle_path(
        &mut self,
        addr: Address,
        last_account: &Address,
        path: &mut AddressIterator,
        merkle_path: &mut Vec<MerklePath>,
        first: bool,
    ) -> Fp {
        let (matrix, own, parent, depth) = match self {
            Root { database, .. } => {
                return database
                    .with(|db| db.emulate_tree_to_get_path(addr, last_account, path, merkle_path));
            }
            Attached {
                hashes,
                owning_account,
                parent,
                depth,
                ..
            } => (hashes, owning_account, Some(parent), *depth),
            Unattached {
                hashes,
                owning_account,
                depth,
                ..
            } => (hashes, owning_account, None, *depth),
        };

        if !first {
            if let Some(hash) = matrix.get(&addr).cloned() {
                return hash;
            }
        }

        // Check if we have any children accounts in our mask
        // When we don't have accounts here, delegate to parent
        // An unattached mask is also a valid empty ledger. With no parent,
        // compute its empty subtrees locally instead of unwrapping None.
        let hash = if parent.is_none() || owns_accounts_under(own, &addr, depth) {
            self.emulate_merkle_path_recursive(addr, last_account, path, merkle_path)
        } else {
            // Recurse to parents until we found a mask having accounts on this address
            let parent = parent.as_ref().unwrap();
            parent.with(|parent| {
                parent.compute_hash_or_parent_for_merkle_path(
                    addr,
                    last_account,
                    path,
                    merkle_path,
                    first,
                )
            })
        };

        hash
    }

    pub fn depth(&self) -> u8 {
        match self {
            Root { database, .. } => database.depth(),
            Attached { depth, .. } => *depth,
            Unattached { depth, .. } => *depth,
        }
    }

    fn emulate_tree_to_get_hash_at(&mut self, addr: Address) -> Fp {
        if let Some(hash) = self.get_cached_hash(&addr) {
            return hash;
        };

        let last_account = self
            .last_filled()
            .unwrap_or_else(|| Address::first(self.depth() as usize));

        self.compute_hash_or_parent(addr, &last_account)
        // self.emulate_tree_recursive(addr, &last_account)
    }

    // fn emulate_recursive(&mut self, addr: Address, nremaining: &mut usize) -> Fp {
    fn emulate_tree_recursive(&mut self, addr: Address, last_account: &Address) -> Fp {
        let tree_depth = self.depth() as usize;
        let current_depth = tree_depth - addr.length();

        if current_depth == 0 {
            return self
                .get_account_hash(addr.to_index())
                .unwrap_or_else(|| self.empty_hash_at_height(0));
        }

        let mut get_child_hash = |addr: Address| {
            if let Some(hash) = self.get_cached_hash(&addr) {
                hash
            } else if addr.is_before(last_account) {
                self.compute_hash_or_parent(addr, last_account)
            } else {
                self.empty_hash_at_height(current_depth - 1)
            }
        };

        let left_hash = get_child_hash(addr.child_left());
        let right_hash = get_child_hash(addr.child_right());

        match self.get_cached_hash(&addr) {
            Some(hash) => hash,
            None => {
                let hash = V2::hash_node(current_depth - 1, left_hash, right_hash);
                self.set_cached_hash(&addr, hash);
                hash
            }
        }
    }

    fn emulate_merkle_path_recursive(
        &mut self,
        addr: Address,
        last_account: &Address,
        path: &mut AddressIterator,
        merkle_path: &mut Vec<MerklePath>,
    ) -> Fp {
        let tree_depth = self.depth() as usize;

        if addr.length() == tree_depth {
            return self
                .get_account_hash(addr.to_index())
                .unwrap_or_else(|| self.empty_hash_at_height(0));
        }

        let next_direction = path.next();

        // We go until the end of the path
        if let Some(direction) = next_direction.as_ref() {
            let child = match direction {
                Direction::Left => addr.child_left(),
                Direction::Right => addr.child_right(),
            };
            self.emulate_merkle_path_recursive(child, last_account, path, merkle_path);
        };

        let depth_in_tree = tree_depth - addr.length();

        let mut get_child_hash = |addr: Address| match self.get_cached_hash(&addr) {
            Some(hash) => hash,
            None => {
                if addr.is_before(last_account) {
                    self.compute_hash_or_parent_for_merkle_path(
                        addr,
                        last_account,
                        path,
                        merkle_path,
                        false,
                    )
                } else {
                    self.empty_hash_at_height(depth_in_tree - 1)
                }
            }
        };

        let left = get_child_hash(addr.child_left());
        let right = get_child_hash(addr.child_right());

        if let Some(direction) = next_direction {
            let hash = match direction {
                Direction::Left => MerklePath::Left(right),
                Direction::Right => MerklePath::Right(left),
            };
            merkle_path.push(hash);
        };

        match self.get_cached_hash(&addr) {
            Some(hash) => hash,
            None => {
                let hash = V2::hash_node(depth_in_tree - 1, left, right);
                self.set_cached_hash(&addr, hash);
                hash
            }
        }
    }

    fn remove_own_account(&mut self, ids: &[AccountId]) {
        match self {
            Root { .. } => todo!(),
            Unattached {
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                hashes,
                ..
            }
            | Attached {
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                hashes,
                ..
            } => {
                let mut addrs = ids
                    .iter()
                    .map(|account_id| id_to_addr.remove(account_id).unwrap())
                    .collect::<Vec<_>>();
                addrs.sort_by_key(Address::to_index);

                for addr in addrs.iter().rev() {
                    let account_index = addr.to_index();
                    hashes.invalidate_hashes(account_index);

                    let account = owning_account.remove(&account_index).unwrap();
                    token_owners
                        .as_mut()
                        .map(|to| to.remove(&account.id().derive_token_id()))
                        .unwrap_or_default();

                    if last_location
                        .as_ref()
                        .map(|last| last == addr)
                        .unwrap_or(false)
                    {
                        *last_location = addr.prev();
                    }
                }

                if owning_account.is_empty() {
                    *last_location = None;
                }
            }
        }
    }

    pub(super) fn set_impl(
        &mut self,
        addr: Address,
        account: Box<Account>,
        child_to_ignore: Option<Uuid>,
    ) {
        let account_index = addr.to_index();

        for (uuid, child) in self.childs() {
            if Some(uuid) == child_to_ignore.as_ref() {
                continue;
            }
            child.parent_set_notify(account_index, &account)
        }

        match self {
            Root { database, .. } => database.set(addr, account),
            Unattached {
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                ..
            }
            | Attached {
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                ..
            } => {
                let account_id = account.id();

                owning_account.insert(account_index, *account);
                id_to_addr.insert(account_id.clone(), addr.clone());
                token_owners
                    .as_mut()
                    .map(|to| to.insert(account_id.derive_token_id(), account_id));

                if last_location
                    .as_ref()
                    .map(|l| l.to_index() < addr.to_index())
                    .unwrap_or(true)
                {
                    *last_location = Some(addr);
                }

                self.invalidate_hashes(account_index);
            }
        }
    }

    pub(super) fn transfert_hashes(&mut self, new_hashes: HashesMatrix) {
        match self {
            Root { database, .. } => database.transfert_hashes(new_hashes),
            Attached { hashes, .. } | Unattached { hashes, .. } => {
                hashes.transfert_hashes(new_hashes)
            }
        };
    }

    pub(super) fn remove_accounts_without_notif(&mut self, ids: &[AccountId]) {
        match self {
            Root { database, .. } => database.remove_accounts(ids),
            Unattached { .. } => self.remove_own_account(ids),
            Attached {
                parent, id_to_addr, ..
            } => {
                let (mask_keys, parent_keys): (Vec<_>, Vec<_>) = ids
                    .iter()
                    .cloned()
                    .partition(|id| id_to_addr.contains_key(id));

                if !parent_keys.is_empty() {
                    parent.remove_accounts_without_notif(&parent_keys);
                }

                self.remove_own_account(&mask_keys);
            }
        }
    }

    fn recurse_on_childs<F>(&mut self, fun: &mut F)
    where
        F: FnMut(&mut MaskImpl),
    {
        for child in self.childs().values_mut() {
            child.with(|child| {
                fun(child);
                child.recurse_on_childs(fun)
            });
        }
    }

    pub fn validate_inner_hashes(&mut self) -> Result<(), ()> {
        use std::collections::VecDeque;

        let tree_depth = self.depth() as usize;
        let empty_account_hash = self.empty_hash_at_height(0);

        let mut queue = VecDeque::new();

        for index in 0..(2u64.pow(tree_depth as u32)) {
            let index = AccountIndex(index);
            match self
                .get_account_hash(index)
                .filter(|hash| hash != &empty_account_hash)
            {
                None => break,
                Some(hash) => {
                    let addr = Address::from_index(index, tree_depth);
                    let parent = addr.parent().unwrap();
                    queue.push_back((parent, hash));
                }
            }
        }

        while let Some((addr, left_hash)) = queue.pop_front() {
            let height = tree_depth - addr.length() - 1;
            let right_hash = match queue.front().filter(|(addr2, _)| &addr == addr2) {
                Some(_) => queue.pop_front().unwrap().1,
                None => self.empty_hash_at_height(height),
            };

            assert_eq!(self.get_hash(addr.child_left()).unwrap(), left_hash);
            assert_eq!(self.get_hash(addr.child_right()).unwrap(), right_hash);

            let parent = addr.parent();
            let hash = V2::hash_node(height, left_hash, right_hash);
            if Some(hash) != self.get_hash(addr) {
                return Err(());
            }
            if let Some(parent) = parent {
                queue.push_back((parent, hash));
            }
        }

        Ok(())
    }

    /// Checks an attached mask's derived indexes (account ID to address,
    /// token owners, last filled location) against its own accounts, and
    /// each of its accounts against its parent: an account is either where
    /// the parent has it, or new to the parent, past its last account. A
    /// staged ledger over a snarked ledger only updates accounts in place and
    /// adds new ones; an account the mask puts elsewhere would leave the
    /// parent's index for it, or for the account it covers, pointing at the
    /// wrong leaf. With the parent's own indexes checked
    /// (`Database::validate_account_indexes`), every lookup through the mask
    /// then agrees with its leaves. Returns the number of own accounts.
    pub fn validate_child_account_indexes(&self) -> Result<usize, String> {
        let Attached {
            parent,
            owning_account,
            token_owners,
            id_to_addr,
            depth,
            ..
        } = self
        else {
            return Err("child index validation requires an attached mask".into());
        };
        let depth = *depth as usize;
        let parent_last = parent.last_filled();
        let parent_accounts = parent_last.as_ref().map_or(0, |last| last.to_index().0 + 1);
        for (index, account) in owning_account {
            let addr = Address::from_index(*index, depth);
            let id = account.id();
            if id_to_addr.get(&id) != Some(&addr) {
                return Err(format!(
                    "account {}: incorrect id-to-address index",
                    index.0
                ));
            }
            if let Some(owners) = token_owners {
                if owners.get(&id.derive_token_id()) != Some(&id) {
                    return Err(format!("account {}: incorrect token-owner index", index.0));
                }
            }
            let in_parent = parent.location_of_account(&id);
            let expected = (index.0 < parent_accounts).then_some(&addr);
            if in_parent.as_ref() != expected {
                return Err(format!(
                    "account {}: incorrect id-to-address index (the parent has it at {:?})",
                    index.0,
                    in_parent.map(|addr| addr.to_index().0)
                ));
            }
        }
        if id_to_addr.len() != owning_account.len() {
            return Err("id-to-address index count does not match own accounts".into());
        }
        if token_owners
            .as_ref()
            .is_some_and(|owners| owners.len() != owning_account.len())
        {
            return Err("token-owner index count does not match own accounts".into());
        }
        let own_last = owning_account.keys().next_back().map(|index| index.0);
        let expected_last = own_last.max(parent_last.map(|last| last.to_index().0));
        if self.last_filled().map(|last| last.to_index().0) != expected_last {
            return Err("last-filled location does not match filled leaves".into());
        }
        Ok(owning_account.len())
    }

    /// Makes an attached mask hold its own copy of each of `account_ids` it
    /// reads from its parent: the parent's account, at the parent's address.
    /// What the mask reads doesn't change, so neither do its cached hashes;
    /// but a later change of that account in the parent no longer shows
    /// through the mask (`parent_set_notify` stops at an account the mask
    /// holds). Accounts the
    /// mask holds already, or the parent doesn't have, are left alone.
    /// Returns how many it now holds that it didn't.
    pub fn hold_parent_accounts(
        &mut self,
        account_ids: impl IntoIterator<Item = AccountId>,
    ) -> usize {
        let Attached {
            parent,
            owning_account,
            token_owners,
            id_to_addr,
            last_location,
            ..
        } = self
        else {
            return 0;
        };
        let mut held = 0;
        for id in account_ids {
            if id_to_addr.contains_key(&id) {
                continue;
            }
            let Some(addr) = parent.location_of_account(&id) else {
                continue;
            };
            let index = addr.to_index();
            if owning_account.contains_key(&index) {
                continue;
            }
            let Some(account) = parent.get(addr.clone()) else {
                continue;
            };
            if let Some(owners) = token_owners.as_mut() {
                owners.insert(id.derive_token_id(), id.clone());
            }
            id_to_addr.insert(id, addr.clone());
            owning_account.insert(index, *account);
            if last_location
                .as_ref()
                .is_none_or(|last| last.to_index() < index)
            {
                *last_location = Some(addr);
            }
            held += 1;
        }
        held
    }

    /// A resident leaf hash only. Inherited leaves are looked up in the
    /// parent, but an owned leaf with no cached hash must not fall through.
    /// Frozen databases keep their hash matrices in memory.
    fn cached_account_hash(&self, index: AccountIndex) -> Option<Fp> {
        let addr = Address::from_index(index, self.depth() as usize);
        match self {
            Root { database, .. } => database.get_cached_hash(&addr),
            Attached {
                parent,
                owning_account,
                hashes,
                ..
            } => {
                if owning_account.contains_key(&index) {
                    hashes.get(&addr).copied()
                } else {
                    parent.with(|parent| parent.cached_account_hash(index))
                }
            }
            Unattached { hashes, .. } => hashes.get(&addr).copied(),
        }
    }

    /// Release finalized duplicates only at the root staged ledger's commit,
    /// after its parent becomes the current root snarked ledger. Equality is
    /// checked with the ledger's account hashes, already resident after the
    /// commit's root checks. Missing parent hashes keep the account; never
    /// read an account from disk to decide whether to release it.
    pub fn reclaim_unreferenced_accounts(&mut self, pending: &HashSet<AccountId>) -> usize {
        let Attached {
            parent,
            owning_account,
            token_owners,
            id_to_addr,
            last_location,
            hashes,
            depth,
            ..
        } = self
        else {
            return 0;
        };
        let before = owning_account.len();
        owning_account.retain(|index, account| {
            let id = account.id();
            if pending.contains(&id) {
                return true;
            }
            let Some(parent_hash) = parent.with(|parent| parent.cached_account_hash(*index)) else {
                return true;
            };
            let addr = Address::from_index(*index, *depth as usize);
            let own_hash = hashes.get(&addr).copied().unwrap_or_else(|| account.hash());
            if own_hash != parent_hash {
                return true;
            }
            id_to_addr.remove(&id);
            if let Some(owners) = token_owners.as_mut() {
                owners.remove(&id.derive_token_id());
            }
            // The value is unchanged, so descendants' hashes stay valid.
            // Drop this path so historical leaves don't retain hash memory.
            hashes.invalidate_hashes(*index);
            false
        });
        let reclaimed = before - owning_account.len();
        if reclaimed > 0 {
            if owning_account.is_empty() {
                hashes.invalidate_all();
            }
            *last_location = owning_account
                .last_key_value()
                .map(|(index, _)| Address::from_index(*index, *depth as usize));
            // A node upgraded with a large history must release its index
            // capacity too. Hysteresis avoids rebuilding it on every commit.
            if id_to_addr.capacity() > id_to_addr.len().saturating_mul(4) {
                id_to_addr.shrink_to_fit();
            }
            if let Some(owners) = token_owners.as_mut() {
                if owners.capacity() > owners.len().saturating_mul(4) {
                    owners.shrink_to_fit();
                }
            }
        }
        reclaimed
    }

    /// The indices of the accounts the mask holds itself (all of a root's).
    pub fn own_account_indexes(&self) -> Vec<AccountIndex> {
        match self {
            Root { .. } => (0..self.num_accounts() as u64).map(AccountIndex).collect(),
            Attached { owning_account, .. } | Unattached { owning_account, .. } => {
                owning_account.keys().copied().collect()
            }
        }
    }

    pub fn get_raw_inner_hashes(&self) -> Vec<(u64, Fp)> {
        match self {
            Root { database, .. } => {
                database.with(|this| this.hashes_matrix.get_raw_inner_hashes())
            }
            Attached { hashes, .. } => hashes.clone().get_raw_inner_hashes(),
            Unattached { hashes, .. } => hashes.clone().get_raw_inner_hashes(),
        }
    }

    pub fn set_raw_inner_hashes(&self, raw_hashes: Vec<(u64, Fp)>) {
        match self {
            Root { database, .. } => {
                database.with(|this| this.hashes_matrix.set_raw_inner_hashes(raw_hashes))
            }
            Attached { hashes, .. } => hashes.clone().set_raw_inner_hashes(raw_hashes),
            Unattached { hashes, .. } => hashes.clone().set_raw_inner_hashes(raw_hashes),
        }
    }

    /// For tests only, check if the address is in the mask, without checking parent
    #[cfg(test)]
    pub fn test_is_in_mask(&self, addr: &Address) -> bool {
        match self {
            Root { database, .. } => database.get(addr.clone()).is_some(),
            Unattached { owning_account, .. } | Attached { owning_account, .. } => {
                let index = addr.to_index();
                owning_account.contains_key(&index)
            }
        }
    }

    /// For tests only
    #[cfg(test)]
    pub fn test_matrix(&self) -> HashesMatrix {
        match self {
            Root { database, .. } => database.test_matrix(),
            Unattached { hashes, .. } | Attached { hashes, .. } => hashes.clone(),
        }
    }
}

impl BaseLedger for MaskImpl {
    fn to_list(&self) -> Vec<Account> {
        let depth = self.depth() as usize;
        let num_accounts = self.num_accounts() as u64;

        (0..num_accounts)
            .map(AccountIndex)
            .filter_map(|index| self.get(Address::from_index(index, depth)))
            .map(|account| *account)
            .collect()
    }

    fn iter<F>(&self, mut fun: F)
    where
        F: FnMut(&Account),
    {
        let depth = self.depth() as usize;
        let num_accounts = self.num_accounts() as u64;

        (0..num_accounts)
            .map(AccountIndex)
            .filter_map(|index| self.get(Address::from_index(index, depth)))
            .for_each(|account| fun(&account));
    }

    fn fold<B, F>(&self, init: B, mut fun: F) -> B
    where
        F: FnMut(B, &Account) -> B,
    {
        let depth = self.depth() as usize;
        let num_accounts = self.num_accounts() as u64;
        let mut accum = init;

        for account in (0..num_accounts)
            .map(AccountIndex)
            .filter_map(|index| self.get(Address::from_index(index, depth)))
        {
            accum = fun(accum, &account)
        }

        accum
    }

    fn fold_with_ignored_accounts<B, F>(
        &self,
        ignoreds: HashSet<AccountId>,
        init: B,
        mut fun: F,
    ) -> B
    where
        F: FnMut(B, &Account) -> B,
    {
        self.fold(init, |accum, account| {
            if !ignoreds.contains(&account.id()) {
                fun(accum, account)
            } else {
                accum
            }
        })
    }

    fn fold_until<B, F>(&self, init: B, mut fun: F) -> B
    where
        F: FnMut(B, &Account) -> std::ops::ControlFlow<B, B>,
    {
        use std::ops::ControlFlow::*;

        let depth = self.depth() as usize;
        let num_accounts = self.num_accounts() as u64;
        let mut accum = init;

        for account in (0..num_accounts)
            .map(AccountIndex)
            .filter_map(|index| self.get(Address::from_index(index, depth)))
        {
            match fun(accum, &account) {
                Continue(acc) => accum = acc,
                Break(acc) => {
                    accum = acc;
                    break;
                }
            }
        }

        accum
    }

    fn accounts(&self) -> HashSet<AccountId> {
        let mut set = HashSet::with_capacity(self.num_accounts());

        self.iter(|account| {
            set.insert(account.id());
        });

        set
    }

    fn token_owner(&self, token_id: TokenId) -> Option<AccountId> {
        let (parent, token_owners) = match self {
            Root { database, .. } => return database.token_owner(token_id),
            Attached {
                parent,
                token_owners,
                ..
            } => (Some(parent), token_owners),
            Unattached { token_owners, .. } => (None, token_owners),
        };

        if let Some(account_id) = token_owners
            .as_ref()
            .and_then(|to| to.get(&token_id).cloned())
        {
            return Some(account_id);
        };

        parent.as_ref()?.token_owner(token_id)
    }

    fn tokens(&self, public_key: CompressedPubKey) -> HashSet<TokenId> {
        let mut set = HashSet::with_capacity(1024);

        for account in self.to_list() {
            if account.public_key == public_key {
                set.insert(account.token_id);
            }
        }

        set
    }

    fn location_of_account(&self, account_id: &AccountId) -> Option<Address> {
        let (parent, id_to_addr) = match self {
            Root { database, .. } => return database.location_of_account(account_id),
            Attached {
                parent, id_to_addr, ..
            } => (Some(parent), id_to_addr),
            Unattached { id_to_addr, .. } => (None, id_to_addr),
        };

        if let Some(addr) = id_to_addr.get(account_id).cloned() {
            return Some(addr);
        }

        parent.as_ref()?.location_of_account(account_id)
    }

    fn location_of_account_batch(
        &self,
        account_ids: &[AccountId],
    ) -> Vec<(AccountId, Option<Address>)> {
        account_ids
            .iter()
            .map(|account_id| {
                let addr = self.location_of_account(account_id);
                (account_id.clone(), addr)
            })
            .collect()
    }

    fn get_or_create_account(
        &mut self,
        account_id: AccountId,
        account: Account,
    ) -> Result<GetOrCreated, DatabaseError> {
        if let Some(addr) = self.location_of_account(&account_id) {
            return Ok(GetOrCreated::Existed(addr));
        }

        let last_filled = self.last_filled();

        let result = match self {
            Root { database, .. } => database.get_or_create_account(account_id, account)?,
            Unattached {
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                depth,
                ..
            }
            | Attached {
                owning_account,
                token_owners,
                id_to_addr,
                last_location,
                depth,
                ..
            } => {
                let location = match last_filled {
                    Some(last) => last.next().ok_or(DatabaseError::OutOfLeaves)?,
                    None => Address::first(*depth as usize),
                };

                let account_index: AccountIndex = location.to_index();

                id_to_addr.insert(account_id.clone(), location.clone());
                *last_location = Some(location.clone());
                token_owners
                    .as_mut()
                    .map(|to| to.insert(account_id.derive_token_id(), account_id));
                owning_account.insert(account_index, account);

                self.invalidate_hashes(account_index);

                GetOrCreated::Added(location)
            }
        };

        elog!("get_or_create_account added");

        // let addr = result.clone();
        // let account_index = addr.to_index();
        // self.recurse_on_childs(&mut |child| {
        //     child.with(|child| {
        //         child.invalidate_hashes(account_index.clone());
        //     })
        // });

        Ok(result)
    }

    fn close(&self) {
        // Drop
    }

    fn last_filled(&self) -> Option<Address> {
        match self {
            Root { database, .. } => database.last_filled(),
            Unattached { last_location, .. } => last_location.clone(),
            Attached {
                parent,
                last_location,
                ..
            } => {
                let last_filled_parent = match parent.last_filled() {
                    Some(last) => last,
                    None => return last_location.clone(),
                };

                let last_filled = match last_location {
                    Some(last) => last,
                    None => return Some(last_filled_parent),
                };

                let last_filled_parent_index = last_filled_parent.to_index();
                let last_filled_index = last_filled.to_index();

                if last_filled_index > last_filled_parent_index {
                    Some(last_filled.clone())
                } else {
                    Some(last_filled_parent)
                }
            }
        }
    }

    fn get_uuid(&self) -> Uuid {
        match self {
            Root { database, .. } => database.get_uuid(),
            Attached { uuid, .. } | Unattached { uuid, .. } => uuid.clone(),
        }
    }

    fn get_directory(&self) -> Option<PathBuf> {
        match self {
            Root { database, .. } => database.get_directory(),
            Attached { parent, .. } => parent.get_directory(),
            Unattached { .. } => None,
        }
    }

    fn get_account_hash(&mut self, account_index: AccountIndex) -> Option<Fp> {
        let (mut parent, owning_account, matrix, depth) = match self {
            Root { database, .. } => return database.get_account_hash(account_index),
            Attached {
                parent,
                owning_account,
                hashes,
                depth,
                ..
            } => (Some(parent), owning_account, hashes, depth),
            Unattached {
                owning_account,
                hashes,
                depth,
                ..
            } => (None, owning_account, hashes, depth),
        };

        if let Some(account) = owning_account.get(&account_index) {
            let addr = Address::from_index(account_index, *depth as usize);

            if let Some(hash) = matrix.get(&addr).cloned() {
                return Some(hash);
            }

            let hash = account.hash();
            matrix.set(&addr, hash);

            return Some(hash);
        }

        parent.as_mut()?.get_account_hash(account_index)
    }

    fn get(&self, addr: Address) -> Option<Box<Account>> {
        // Avoid stack overflow
        #[inline(never)]
        fn get_account(
            addr: &Address,
            owning_account: &BTreeMap<AccountIndex, Account>,
        ) -> Option<Box<Account>> {
            owning_account.get(&addr.to_index()).cloned().map(Box::new)
        }

        let (parent, owning_account) = match self {
            Root { database, .. } => return database.get(addr),
            Attached {
                parent,
                owning_account,
                ..
            } => (Some(parent), owning_account),
            Unattached { owning_account, .. } => (None, owning_account),
        };

        if let Some(account) = get_account(&addr, owning_account) {
            return Some(account);
        }

        parent.as_ref()?.get(addr)
    }

    fn get_batch(&self, addr: &[Address]) -> Vec<(Address, Option<Box<Account>>)> {
        addr.iter()
            .map(|addr| (addr.clone(), self.get(addr.clone())))
            .collect()
    }

    fn set(&mut self, addr: Address, account: Box<Account>) {
        self.set_impl(addr, account, None)
    }

    fn set_batch(&mut self, list: &[(Address, Box<Account>)]) {
        for (addr, account) in list {
            self.set(addr.clone(), account.clone())
        }
    }

    fn get_at_index(&self, index: AccountIndex) -> Option<Box<Account>> {
        let addr = Address::from_index(index, self.depth() as usize);
        self.get(addr)
    }

    fn set_at_index(&mut self, index: AccountIndex, account: Box<Account>) -> Result<(), ()> {
        let addr = Address::from_index(index, self.depth() as usize);
        self.set(addr, account);
        Ok(())
    }

    fn index_of_account(&self, account_id: AccountId) -> Option<AccountIndex> {
        let (parent, id_to_addr) = match self {
            Root { database, .. } => return database.index_of_account(account_id),
            Attached {
                parent, id_to_addr, ..
            } => (Some(parent), id_to_addr),
            Unattached { id_to_addr, .. } => (None, id_to_addr),
        };

        if let Some(addr) = id_to_addr.get(&account_id).cloned() {
            return Some(addr.to_index());
        };

        parent.as_ref()?.index_of_account(account_id)
    }

    fn merkle_root(&mut self) -> Fp {
        // elog!("MERKLE_ROOT={:?}", self.short());
        let hash = self.emulate_tree_to_get_hash_at(Address::root());
        // self.emulate_tree_to_get_hash()

        let num_accounts = self.num_accounts();
        elog!("merkle_root={} num_accounts={:?}", hash, num_accounts);

        hash
    }

    fn merkle_path(&mut self, addr: Address) -> Vec<MerklePath> {
        elog!("merkle_path short={:?}", self.short());
        // elog!("merkle_path num_accounts={:?} addr={:?}", self.num_accounts(), addr);

        if let Root { database, .. } = self {
            return database.merkle_path(addr);
        };

        let mut merkle_path = Vec::with_capacity(addr.length());
        let mut path = addr.into_iter();
        let addr = Address::root();

        let last_account = self
            .last_filled()
            .unwrap_or_else(|| Address::first(self.depth() as usize));

        // elog!("merkle_path last_account={:?}", last_account);

        self.compute_hash_or_parent_for_merkle_path(
            addr,
            &last_account,
            &mut path,
            &mut merkle_path,
            true,
        );
        // self.emulate_merkle_path_recursive(addr, &last_account, &mut path, &mut merkle_path);

        merkle_path
    }

    fn merkle_path_at_index(&mut self, index: AccountIndex) -> Vec<MerklePath> {
        let addr = Address::from_index(index, self.depth() as usize);
        self.merkle_path(addr)
    }

    fn remove_accounts(&mut self, ids: &[AccountId]) {
        let indexes: Vec<_> = ids
            .iter()
            .filter_map(|id| {
                let addr = self.location_of_account(id)?;
                Some(addr.to_index())
            })
            .collect();

        self.remove_accounts_without_notif(ids);

        self.recurse_on_childs(&mut |child| {
            for index in &indexes {
                child.invalidate_hashes(*index);
            }
        });
    }

    fn detached_signal(&mut self) {
        todo!()
    }

    fn depth(&self) -> u8 {
        self.depth()
    }

    fn num_accounts(&self) -> usize {
        self.last_filled()
            .map(|addr| addr.to_index().0 as usize + 1)
            .unwrap_or(0)
    }

    fn merkle_path_at_addr(&mut self, addr: Address) -> Vec<MerklePath> {
        self.merkle_path(addr)
    }

    fn get_inner_hash_at_addr(&mut self, addr: Address) -> Result<Fp, String> {
        let self_depth = self.depth() as usize;

        if addr.length() > self_depth {
            return Err("Inner hash not found at address".into());
        }

        Ok(self.emulate_tree_to_get_hash_at(addr))
    }

    fn set_inner_hash_at_addr(&mut self, _addr: Address, _hash: Fp) -> Result<(), ()> {
        todo!()
    }

    fn set_all_accounts_rooted_at(
        &mut self,
        addr: Address,
        accounts: &[Box<Account>],
    ) -> Result<(), ()> {
        let depth = self.depth() as usize;

        if addr.length() > depth {
            return Err(());
        }

        for (child_addr, account) in addr.iter_children(depth).zip(accounts) {
            self.set(child_addr, account.clone());
        }

        Ok(())
    }

    fn get_all_accounts_rooted_at(&self, addr: Address) -> Option<Vec<(Address, Box<Account>)>> {
        let self_depth = self.depth() as usize;

        if addr.length() > self_depth {
            return None;
        }

        let accounts = addr
            .iter_children(self_depth)
            .filter_map(|addr| Some((addr.clone(), self.get(addr)?)))
            .collect::<Vec<_>>();

        if accounts.is_empty() {
            None
        } else {
            Some(accounts)
        }
    }

    fn make_space_for(&mut self, _space: usize) {
        // No op, we're in memory
    }

    fn commit(&mut self) {
        self.commit()
    }
}

/// Whether any of a mask's own accounts is in the subtree at `addr`. A range
/// lookup over the subtree's leaf indices: scanning them all made computing a
/// merkle root quadratic in the accounts a mask holds, a sixth of the time to
/// reconstruct a mainnet staged ledger.
fn owns_accounts_under(
    owning_account: &BTreeMap<AccountIndex, Account>,
    addr: &Address,
    depth: u8,
) -> bool {
    let height = (depth as usize)
        .checked_sub(addr.length())
        .expect("address deeper than the ledger");
    let first = addr.to_index().0 << height;
    let last = first | ((1u64 << height) - 1);
    owning_account
        .range(AccountIndex(first)..=AccountIndex(last))
        .next()
        .is_some()
}
