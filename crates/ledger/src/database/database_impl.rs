use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
    ops::ControlFlow,
    path::{Path, PathBuf},
    sync::Arc,
};

use mina_curves::pasta::Fp;
use mina_signer::CompressedPubKey;

use crate::{
    next_uuid, Account, AccountId, AccountIndex, AccountLegacy, Address, AddressIterator,
    BaseLedger, Direction, GetOrCreated, HashesMatrix, MerklePath, TokenId, TreeVersion, Uuid, V1,
    V2,
};

use super::{
    disk::{DiskAccounts, DiskError, DiskWriter},
    DatabaseError,
};

#[derive(Clone)]
pub struct DatabaseImpl<T: TreeVersion> {
    /// The accounts by slot, while they are in memory. Empty once frozen.
    accounts: Vec<Option<T::Account>>,
    /// The accounts in a file, once frozen ([`Self::freeze_to_disk`], PC3a).
    /// Clones share it. Slots changed since are in `overlay`, the ids set
    /// since in `id_to_addr`, and the ids removed from the file's in
    /// `disk_ids_removed`: together they answer as `accounts` and
    /// `id_to_addr` would.
    disk: Option<Arc<DiskAccounts>>,
    /// Failed writes back off across root moves and independent copies.
    /// Deferred attempts do no I/O and return no error to log again.
    freeze_retry_at: Option<redux::Instant>,
    freeze_retry_secs: u64,
    overlay: BTreeMap<u64, Option<T::Account>>,
    disk_ids_removed: HashSet<AccountId>,
    pub hashes_matrix: HashesMatrix,
    /// Every account's id while in memory; once frozen, only those set since.
    id_to_addr: HashMap<AccountId, Address>,
    token_owners: Option<HashMap<T::TokenId, AccountId>>,
    depth: u8,
    last_location: Option<Address>,
    naccounts: usize,
    uuid: Uuid,
    directory: PathBuf,
}

impl<T: TreeVersion> std::fmt::Debug for DatabaseImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            // .field("accounts", &self.accounts)
            .field("hashes_matrix", &self.hashes_matrix)
            // .field("id_to_addr", &self.id_to_addr)
            // .field("token_owners", &self.token_owners)
            // .field("depth", &self.depth)
            // .field("last_location", &self.last_location)
            .field("naccounts", &self.naccounts)
            .field("disk", &self.disk)
            .field("overlay", &self.overlay.len())
            .field("uuid", &self.uuid)
            .field("directory", &self.directory)
            .finish()
    }
}

/// Makes room in `accounts` for `len` slots, growing it by an eighth instead of
/// doubling it as `Vec` does.
///
/// A mainnet ledger has about 290,000 accounts, and a slot (`Option<Account>`)
/// takes 264 bytes on wasm32, so one account vector is about 73 MiB in the
/// browser, where the web node holds four full ledgers. Doubling left each
/// vector with room for about 581,000 accounts: 293 MiB unused in all, against
/// 36 MiB when growing by an eighth.
/// Growth stays geometric, so a push still costs amortised constant time.
///
/// A request past the next eighth, as when a ledger synced from empty sets an
/// account near its end first, gets exactly the slots it asks for. Only the
/// capacity changes: lengths, contents and hashes are the same either way.
fn reserve_accounts<A>(accounts: &mut Vec<A>, len: usize) {
    let capacity = accounts.capacity();
    if len > capacity {
        let target = len.max(capacity + capacity / 8);
        accounts.reserve_exact(target - accounts.len());
    }
}

impl DatabaseImpl<V2> {
    /// Checks the metadata that Merkle hashes do not cover, without changing
    /// the database. Used before a persisted root ledger becomes usable.
    pub(crate) fn validate_account_indexes(&self) -> Result<usize, String> {
        let mut filled = 0;
        let mut owned_tokens = 0;
        let mut last = None;
        let mut error = None;
        self.for_each_slot(|index, account| {
            let addr = Address::from_index(AccountIndex(index), self.depth as usize);
            let id = account.id();
            if self.location_of(&id).as_ref() != Some(&addr) {
                error = Some(format!("account {index}: incorrect id-to-address index"));
                return ControlFlow::Break(());
            }
            if !id.token_id.is_default() {
                if let Some(owners) = &self.token_owners {
                    if owners.get(&id.derive_token_id()) != Some(&id) {
                        error = Some(format!("account {index}: incorrect token-owner index"));
                        return ControlFlow::Break(());
                    }
                    owned_tokens += 1;
                }
            }
            filled += 1;
            last = Some(addr);
            ControlFlow::Continue(())
        });
        if let Some(error) = error {
            return Err(error);
        }
        if self.num_indexed_ids() != filled {
            return Err("id-to-address index count does not match filled leaves".into());
        }
        if self
            .token_owners
            .as_ref()
            .is_some_and(|owners| owners.len() != owned_tokens)
        {
            return Err("token-owner index count does not match filled leaves".into());
        }
        if self.naccounts != filled {
            return Err("account count does not match filled leaves".into());
        }
        if self.last_location != last {
            return Err("last-filled location does not match filled leaves".into());
        }
        Ok(filled)
    }

    pub fn clone_db(&self, new_directory: PathBuf) -> Self {
        Self {
            accounts: self.accounts.clone(),
            disk: self.disk.clone(),
            freeze_retry_at: self.freeze_retry_at,
            freeze_retry_secs: self.freeze_retry_secs,
            overlay: self.overlay.clone(),
            disk_ids_removed: self.disk_ids_removed.clone(),
            id_to_addr: self.id_to_addr.clone(),
            token_owners: self.token_owners.clone(),
            depth: self.depth,
            last_location: self.last_location.clone(),
            naccounts: self.naccounts,
            uuid: next_uuid(),
            directory: new_directory,
            hashes_matrix: self.hashes_matrix.clone(),
        }
    }

    pub(crate) fn set_uuid(&mut self, uuid: crate::base::Uuid) {
        self.uuid = uuid;
    }

    fn remove(&mut self, addr: Address) -> Option<Account> {
        let index = addr.to_index().0;
        if index >= self.num_slots() {
            return None;
        }
        self.put_slot(index, None)
    }

    /// The account in slot `index`, from memory, the overlay or the file.
    fn slot(&self, index: u64) -> Option<Cow<'_, Account>> {
        match &self.disk {
            None => self
                .accounts
                .get(index as usize)?
                .as_ref()
                .map(Cow::Borrowed),
            Some(disk) => match self.overlay.get(&index) {
                Some(slot) => slot.as_ref().map(Cow::Borrowed),
                None => disk
                    .get(index)
                    .unwrap_or_else(|e| disk_failed(disk, e))
                    .map(Cow::Owned),
            },
        }
    }

    /// The slots, as `accounts.len()` counts them in memory.
    fn num_slots(&self) -> u64 {
        match &self.disk {
            None => self.accounts.len() as u64,
            Some(disk) => {
                let overlay_end = self.overlay.keys().next_back().map_or(0, |i| i + 1);
                disk.len().max(overlay_end)
            }
        }
    }

    /// Puts `account` in slot `index`, growing the slots to it, and returns
    /// what was there.
    fn put_slot(&mut self, index: u64, account: Option<Account>) -> Option<Account> {
        if self.disk.is_none() {
            let index = index as usize;
            if self.accounts.len() <= index {
                reserve_accounts(&mut self.accounts, index + 1);
                self.accounts.resize(index + 1, None);
            }
            return std::mem::replace(&mut self.accounts[index], account);
        }
        let previous = match self.overlay.get(&index) {
            Some(_) => None,
            None => self.slot(index).map(Cow::into_owned),
        };
        match self.overlay.insert(index, account) {
            Some(previous) => previous,
            None => previous,
        }
    }

    /// Where `id` is, as `id_to_addr.get` answers in memory.
    fn location_of(&self, id: &AccountId) -> Option<Address> {
        if let Some(addr) = self.id_to_addr.get(id) {
            return Some(addr.clone());
        }
        let disk = self.disk.as_ref()?;
        if self.disk_ids_removed.contains(id) {
            return None;
        }
        disk.find(id)
            .unwrap_or_else(|e| disk_failed(disk, e))
            .map(|index| Address::from_index(AccountIndex(index), self.depth as usize))
    }

    /// `id_to_addr.remove`.
    fn unindex_id(&mut self, id: &AccountId) -> Option<Address> {
        if self.disk.is_none() {
            return self.id_to_addr.remove(id);
        }
        let addr = self.location_of(id);
        if addr.is_some() {
            self.id_to_addr.remove(id);
            self.disk_ids_removed.insert(id.clone());
        }
        addr
    }

    /// `id_to_addr.insert`.
    fn index_id(&mut self, id: AccountId, addr: Address) {
        if self.disk.is_some() {
            self.disk_ids_removed.remove(&id);
        }
        self.id_to_addr.insert(id, addr);
    }

    /// `id_to_addr.len()`.
    fn num_indexed_ids(&self) -> usize {
        match self.disk {
            None => self.id_to_addr.len(),
            Some(_) => self.indexed_ids().len(),
        }
    }

    /// `id_to_addr.keys()`, the file's ids read from it.
    fn indexed_ids(&self) -> HashSet<AccountId> {
        let Some(disk) = &self.disk else {
            return self.id_to_addr.keys().cloned().collect();
        };
        let mut ids: HashSet<AccountId> = self.id_to_addr.keys().cloned().collect();
        disk.scan(
            |_| true,
            |index, account| {
                if let Some(account) = account {
                    let id = account.id();
                    if !self.disk_ids_removed.contains(&id)
                        && !self.id_to_addr.contains_key(&id)
                        && disk.find(&id).unwrap_or_else(|e| disk_failed(disk, e)) == Some(index)
                    {
                        ids.insert(id);
                    }
                }
                true
            },
        )
        .unwrap_or_else(|e| disk_failed(disk, e));
        ids
    }

    /// Calls `fun` with each account in slot order, as iterating `accounts`
    /// does in memory. A frozen database reads its file once from the start.
    fn for_each_slot<B, F>(&self, mut fun: F) -> Option<B>
    where
        F: FnMut(u64, &Account) -> ControlFlow<B>,
    {
        let Some(disk) = &self.disk else {
            for (index, account) in self.accounts.iter().enumerate() {
                if let Some(account) = account {
                    if let ControlFlow::Break(b) = fun(index as u64, account) {
                        return Some(b);
                    }
                }
            }
            return None;
        };
        let mut result = None;
        disk.scan(
            |index| !self.overlay.contains_key(&index),
            |index, account| {
                let flow = match self.overlay.get(&index) {
                    Some(Some(account)) => fun(index, account),
                    Some(None) => ControlFlow::Continue(()),
                    None => match &account {
                        Some(account) => fun(index, account),
                        None => ControlFlow::Continue(()),
                    },
                };
                match flow {
                    ControlFlow::Continue(()) => true,
                    ControlFlow::Break(b) => {
                        result = Some(b);
                        false
                    }
                }
            },
        )
        .unwrap_or_else(|e| disk_failed(disk, e));
        if result.is_some() {
            return result;
        }
        for (index, account) in self.overlay.range(disk.len()..) {
            if let Some(account) = account {
                if let ControlFlow::Break(b) = fun(*index, account) {
                    return Some(b);
                }
            }
        }
        None
    }

    /// Whether the accounts are in a file.
    pub fn is_on_disk(&self) -> bool {
        self.disk.is_some()
    }

    /// The slots changed since the database was frozen.
    pub fn disk_overlay_len(&self) -> usize {
        self.overlay.len()
    }

    /// Whether the overlay holds enough changes for [`Self::freeze_to_disk`]
    /// to write the file again ([`super::disk::refreeze_threshold`]).
    pub fn refreeze_due(&self) -> bool {
        self.disk.is_some()
            && self.overlay.len() >= super::disk::refreeze_threshold(self.num_slots())
    }

    /// The accounts the file's read cache holds, if frozen.
    pub fn disk_cached(&self) -> Option<usize> {
        self.disk.as_ref().map(|disk| disk.cached())
    }

    /// Moves the accounts to a new file in `dir`: those in memory, or, for a
    /// frozen database with changes, the file's merged with them
    /// ([`DiskAccounts::merge`], PC3b), which copies the unchanged records as
    /// they are. The hash matrix is computed first and stays in memory,
    /// compacted, so that hashes, Merkle paths and the root need no read.
    /// Nothing to do for a frozen database without changes (`Ok(None)`). On an
    /// error the database is unchanged, and so are its clones, which keep the
    /// old file. Failed writes are retried after 60 seconds, doubling up to
    /// an hour. A deferred attempt returns `Ok(None)`, so the caller logs only
    /// actual failures. Success resets the delay. No filesystem probe runs
    /// while waiting, including on the block producer's ledger thread.
    pub fn freeze_to_disk(&mut self, dir: &Path) -> std::io::Result<Option<FreezeReport>> {
        self.freeze_to_disk_at(dir, redux::Instant::now())
    }

    fn freeze_to_disk_at(
        &mut self,
        dir: &Path,
        now: redux::Instant,
    ) -> std::io::Result<Option<FreezeReport>> {
        if self.freeze_retry_at.is_some_and(|deadline| now < deadline) {
            return Ok(None);
        }
        if self.disk.is_some() && self.overlay.is_empty() && !super::disk::rewrite_unchanged() {
            return Ok(None);
        }
        let started = redux::Instant::now();
        self.root_hash();
        let slots = self.num_slots();
        let changed = self.overlay.len() as u64;
        let written = match &self.disk {
            Some(disk) => disk.merge(dir, slots, &self.overlay),
            None => self.write_accounts(dir, slots),
        };
        let disk = match written {
            Ok(disk) => disk,
            Err(error) => {
                self.freeze_retry_secs = (self.freeze_retry_secs * 2).clamp(60, 3600);
                self.freeze_retry_at =
                    Some(now + std::time::Duration::from_secs(self.freeze_retry_secs));
                return Err(error);
            }
        };
        self.freeze_retry_at = None;
        self.freeze_retry_secs = 0;
        let report = FreezeReport {
            accounts: disk.num_accounts(),
            file_bytes: disk.file_len(),
            index_bytes: disk.heap_bytes() as u64,
            refreeze: self.disk.is_some(),
            changed,
            ms: started.elapsed().as_secs_f64() * 1000.0,
        };
        self.hashes_matrix.compact();
        self.accounts = Vec::new();
        self.overlay = BTreeMap::new();
        self.disk_ids_removed = HashSet::new();
        self.id_to_addr = HashMap::new();
        super::disk::kill_point("freeze-swap");
        self.disk = Some(Arc::new(disk));
        Ok(Some(report))
    }

    /// A new file of the accounts in memory, `slots` slots.
    fn write_accounts(&self, dir: &Path, slots: u64) -> std::io::Result<DiskAccounts> {
        let mut writer = DiskWriter::create(dir, slots as usize)?;
        let mut error = None;
        let mut next = 0u64;
        self.for_each_slot(|index, account| {
            let result = (next..index)
                .try_for_each(|_| writer.push(None))
                .and_then(|()| writer.push(Some(account)));
            next = index + 1;
            match result {
                Ok(()) => ControlFlow::Continue(()),
                Err(e) => {
                    error = Some(e);
                    ControlFlow::Break(())
                }
            }
        });
        if let Some(error) = error {
            writer.abandon();
            return Err(error);
        }
        for _ in next..slots {
            if let Err(error) = writer.push(None) {
                writer.abandon();
                return Err(error);
            }
        }
        writer.finish()
    }

    fn create_account(
        &mut self,
        account_id: AccountId,
        account: Account,
    ) -> Result<GetOrCreated, DatabaseError> {
        if let Some(addr) = self.location_of(&account_id) {
            return Ok(GetOrCreated::Existed(addr));
        }

        let token_id = account.token_id.clone();
        let location = match self.last_location.as_ref() {
            Some(last) => last.next().ok_or(DatabaseError::OutOfLeaves)?,
            None => Address::first(self.depth as usize),
        };

        assert_eq!(location.to_index().0, self.num_slots());
        self.put_slot(location.to_index().0, Some(account));

        self.last_location = Some(location.clone());
        self.naccounts += 1;

        if !token_id.is_default() {
            if let Some(token_owners) = self.token_owners.as_mut() {
                token_owners.insert(account_id.derive_token_id(), account_id.clone());
            }
        }
        self.index_id(account_id, location.clone());

        Ok(GetOrCreated::Added(location))
    }

    pub fn iter_with_addr<F>(&self, mut fun: F)
    where
        F: FnMut(Address, &Account),
    {
        let depth = self.depth as usize;

        self.for_each_slot(|index, account| {
            let addr = Address::from_index(AccountIndex(index), depth);
            fun(addr, account);
            ControlFlow::<()>::Continue(())
        });
    }

    fn emulate_tree_to_get_hash_at(&mut self, addr: Address) -> Fp {
        if let Some(hash) = self.hashes_matrix.get(&addr) {
            return *hash;
        };

        let last_account = self
            .last_filled()
            .unwrap_or_else(|| Address::first(self.depth as usize));

        self.emulate_tree_recursive(addr, &last_account)
    }

    pub fn emulate_tree_recursive(&mut self, addr: Address, last_account: &Address) -> Fp {
        let tree_depth = self.depth as usize;
        let current_depth = tree_depth - addr.length();

        if current_depth == 0 {
            return self
                .get_account_hash(addr.to_index())
                .unwrap_or_else(|| self.hashes_matrix.empty_hash_at_height(0));
        }

        let mut get_child_hash = |addr: Address| {
            if let Some(hash) = self.hashes_matrix.get(&addr) {
                *hash
            } else if addr.is_before(last_account) {
                self.emulate_tree_recursive(addr, last_account)
            } else {
                self.hashes_matrix.empty_hash_at_height(current_depth - 1)
            }
        };

        let left_hash = get_child_hash(addr.child_left());
        let right_hash = get_child_hash(addr.child_right());

        match self.hashes_matrix.get(&addr) {
            Some(hash) => *hash,
            None => {
                let hash = V2::hash_node(current_depth - 1, left_hash, right_hash);
                self.hashes_matrix.set(&addr, hash);
                hash
            }
        }
    }

    pub fn emulate_tree_to_get_path(
        &mut self,
        addr: Address,
        last_account: &Address,
        path: &mut AddressIterator,
        merkle_path: &mut Vec<MerklePath>,
    ) -> Fp {
        let tree_depth = self.depth as usize;

        if addr.length() == self.depth as usize {
            return self
                .get_account_hash(addr.to_index())
                .unwrap_or_else(|| self.hashes_matrix.empty_hash_at_height(0));
        }

        let next_direction = path.next();

        // We go until the end of the path
        if let Some(direction) = next_direction.as_ref() {
            let child = match direction {
                Direction::Left => addr.child_left(),
                Direction::Right => addr.child_right(),
            };
            self.emulate_tree_to_get_path(child, last_account, path, merkle_path);
        };

        let depth_in_tree = tree_depth - addr.length();

        let mut get_child_hash = |addr: Address| match self.hashes_matrix.get(&addr) {
            Some(hash) => *hash,
            None => {
                if let Some(hash) = self.hashes_matrix.get(&addr) {
                    *hash
                } else if addr.is_before(last_account) {
                    self.emulate_tree_to_get_path(addr, last_account, path, merkle_path)
                } else {
                    self.hashes_matrix.empty_hash_at_height(depth_in_tree - 1)
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

        match self.hashes_matrix.get(&addr) {
            Some(hash) => *hash,
            None => {
                let hash = V2::hash_node(depth_in_tree - 1, left, right);
                self.hashes_matrix.set(&addr, hash);
                hash
            }
        }
    }

    pub fn create_checkpoint(&self, directory_name: String) {
        elog!("create_checkpoint {}", directory_name);
    }

    pub fn make_checkpoint(&self, directory_name: String) {
        elog!("make_checkpoint {}", directory_name);
    }

    pub fn get_cached_hash(&self, addr: &Address) -> Option<Fp> {
        self.hashes_matrix.get(addr).copied()
    }

    pub fn set_cached_hash(&mut self, addr: &Address, hash: Fp) {
        self.hashes_matrix.set(addr, hash);
    }

    pub fn empty_hash_at_height(&mut self, height: usize) -> Fp {
        self.hashes_matrix.empty_hash_at_height(height)
    }

    pub fn invalidate_hashes(&mut self, account_index: AccountIndex) {
        self.hashes_matrix.invalidate_hashes(account_index)
    }

    pub fn transfert_hashes(&mut self, hashes: HashesMatrix) {
        self.hashes_matrix.transfert_hashes(hashes)
    }

    pub fn has_token_owners(&self) -> bool {
        self.token_owners.is_some()
    }
}

impl DatabaseImpl<V1> {
    pub fn create_account(
        &mut self,
        _account_id: (),
        account: AccountLegacy,
    ) -> Result<Address, DatabaseError> {
        let location = match self.last_location.as_ref() {
            Some(last) => last.next().ok_or(DatabaseError::OutOfLeaves)?,
            None => Address::first(self.depth as usize),
        };

        assert_eq!(location.to_index(), self.accounts.len());
        self.accounts.push(Some(account));

        self.last_location = Some(location.clone());
        self.naccounts += 1;

        Ok(location)
    }
}

impl DatabaseImpl<V2> {
    const NACCOUNTS: usize = 10_000;
    const NTOKENS: usize = 10;

    pub fn create_with_dir(depth: u8, dir_name: Option<PathBuf>) -> Self {
        assert!((1..0xfe).contains(&depth));

        let uuid = next_uuid();

        let path = match dir_name {
            Some(dir_name) => dir_name,
            None => {
                let directory = format!("minadb-{uuid}");

                let mut path = PathBuf::from("/tmp");
                path.push(&directory);
                path
            }
        };

        Self {
            depth,
            accounts: Vec::with_capacity(Self::NACCOUNTS),
            disk: None,
            freeze_retry_at: None,
            freeze_retry_secs: 0,
            overlay: BTreeMap::new(),
            disk_ids_removed: HashSet::new(),
            last_location: None,
            naccounts: 0,
            id_to_addr: HashMap::with_capacity(Self::NACCOUNTS),
            token_owners: None,
            uuid,
            directory: path,
            hashes_matrix: HashesMatrix::new(depth as usize),
        }
    }

    pub fn create(depth: u8) -> Self {
        Self::create_with_dir(depth, None)
    }

    pub fn create_with_token_owners(depth: u8) -> Self {
        let mut db = Self::create_with_dir(depth, None);
        db.set_token_owners();
        db
    }

    pub fn set_token_owners(&mut self) {
        if self.token_owners.is_none() {
            self.token_owners = Some(HashMap::with_capacity(Self::NTOKENS));
        }
    }

    pub fn unset_token_owners(&mut self) {
        self.token_owners = None;
    }

    pub fn root_hash(&mut self) -> Fp {
        self.emulate_tree_to_get_hash_at(Address::root())
    }

    // Do not use
    pub fn naccounts(&self) -> usize {
        let mut count = 0;
        self.for_each_slot(|_, _| {
            count += 1;
            ControlFlow::<()>::Continue(())
        });
        count
    }

    fn get_account_ref(&self, addr: Address) -> Option<Cow<'_, Account>> {
        self.slot(addr.to_index().0)
    }
}

/// A read of a frozen database's file failed, after its retries: the file
/// holds the only copy of those accounts, so this is as fatal as losing them
/// from memory. The node logs it and exits ([`super::disk::unreadable`]).
#[cold]
fn disk_failed(disk: &DiskAccounts, error: DiskError) -> ! {
    super::disk::unreadable(disk.path(), &error)
}

/// What [`DatabaseImpl::freeze_to_disk`] did.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FreezeReport {
    pub accounts: u64,
    pub file_bytes: u64,
    /// The offsets, checksums and id index kept in memory.
    pub index_bytes: u64,
    /// Whether it rewrote a frozen database's file with its changes.
    pub refreeze: bool,
    /// The slots changed since the last freeze, for a refreeze.
    pub changed: u64,
    pub ms: f64,
}

impl BaseLedger for DatabaseImpl<V2> {
    fn to_list(&self) -> Vec<Account> {
        let mut list = Vec::with_capacity(self.naccounts);
        self.iter(|account| list.push(account.clone()));
        list
    }

    fn iter<F>(&self, mut fun: F)
    where
        F: FnMut(&Account),
    {
        self.for_each_slot(|_, account| {
            fun(account);
            ControlFlow::<()>::Continue(())
        });
    }

    fn fold<B, F>(&self, init: B, mut fun: F) -> B
    where
        F: FnMut(B, &Account) -> B,
    {
        let mut accum = Some(init);
        self.iter(|account| accum = Some(fun(accum.take().unwrap(), account)));
        accum.unwrap()
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
            if ignoreds.contains(&account.id()) {
                accum
            } else {
                fun(accum, account)
            }
        })
    }

    fn fold_until<B, F>(&self, init: B, mut fun: F) -> B
    where
        F: FnMut(B, &Account) -> ControlFlow<B, B>,
    {
        let mut accum = Some(init);
        self.for_each_slot(|_, account| match fun(accum.take().unwrap(), account) {
            ControlFlow::Continue(v) => {
                accum = Some(v);
                ControlFlow::Continue(())
            }
            ControlFlow::Break(v) => {
                accum = Some(v);
                ControlFlow::Break(())
            }
        });
        accum.unwrap()
    }

    fn accounts(&self) -> HashSet<AccountId> {
        self.indexed_ids()
    }

    fn token_owner(&self, token_id: TokenId) -> Option<AccountId> {
        self.token_owners
            .as_ref()
            .and_then(|token_owners| token_owners.get(&token_id).cloned())
    }

    fn tokens(&self, public_key: CompressedPubKey) -> HashSet<TokenId> {
        let mut set = HashSet::with_capacity(100);

        self.iter(|account| {
            if account.public_key == public_key {
                set.insert(account.token_id.clone());
            }
        });
        set
    }

    fn location_of_account(&self, account_id: &AccountId) -> Option<Address> {
        self.location_of(account_id)
    }

    fn location_of_account_batch(
        &self,
        account_ids: &[AccountId],
    ) -> Vec<(AccountId, Option<Address>)> {
        let res: Vec<_> = account_ids
            .iter()
            .map(|account_id| {
                let addr = self.location_of(account_id);
                (account_id.clone(), addr)
            })
            .collect();

        elog!(
            "location_of_account_batch ids={:?}\nres={:?}={:?}",
            account_ids,
            res.len(),
            res
        );

        res
    }

    fn get_or_create_account(
        &mut self,
        account_id: AccountId,
        account: Account,
    ) -> Result<GetOrCreated, DatabaseError> {
        let result = self.create_account(account_id, account);

        if let Ok(GetOrCreated::Added(addr)) = result.as_ref() {
            let account_index = addr.to_index();
            self.hashes_matrix.invalidate_hashes(account_index);
        };

        result
    }

    fn close(&self) {
        elog!(
            "close pid={:?} uuid={:?} path={:?}",
            crate::util::pid(),
            self.uuid,
            self.directory
        );
        // Drop
    }

    fn last_filled(&self) -> Option<Address> {
        self.last_location.clone()
    }

    fn get_uuid(&self) -> crate::base::Uuid {
        self.uuid.clone()
    }

    fn get_directory(&self) -> Option<PathBuf> {
        Some(self.directory.clone())
    }

    fn get_account_hash(&mut self, account_index: AccountIndex) -> Option<Fp> {
        let addr = Address::from_index(account_index, self.depth as usize);

        if let Some(hash) = self.hashes_matrix.get(&addr) {
            return Some(*hash);
        }

        let hash = self.get_account_ref(addr.clone())?.hash();

        self.hashes_matrix.set(&addr, hash);

        Some(hash)
    }

    #[inline(never)]
    fn get(&self, addr: Address) -> Option<Box<Account>> {
        self.get_account_ref(addr)
            .map(|account| Box::new(account.into_owned()))
    }

    fn get_batch(&self, addr: &[Address]) -> Vec<(Address, Option<Box<Account>>)> {
        let res: Vec<_> = addr
            .iter()
            .map(|addr| (addr.clone(), self.get(addr.clone())))
            .collect();

        elog!("get_batch addrs={:?}\nres={:?}={:?}", addr, res.len(), res);

        res
    }

    fn set(&mut self, addr: Address, account: Box<Account>) {
        let index = addr.to_index();

        self.hashes_matrix.invalidate_hashes(index);

        let index = index.0;

        let id = account.id();

        // Remove account at the address and it's index
        if let Some(account) = self.get(addr.clone()) {
            let id = account.id();
            self.unindex_id(&id);
            if !id.token_id.is_default() {
                if let Some(token_owners) = self.token_owners.as_mut() {
                    token_owners.remove(&id.derive_token_id());
                }
            }
        } else {
            self.naccounts += 1;
        }

        if !account.token_id.is_default() {
            if let Some(token_owners) = self.token_owners.as_mut() {
                token_owners.insert(account.id().derive_token_id(), id.clone());
            }
        }
        self.index_id(id, addr.clone());
        self.put_slot(index, Some(*account));

        if self
            .last_location
            .as_ref()
            .map(|l| l.to_index() < addr.to_index())
            .unwrap_or(true)
        {
            self.last_location = Some(addr);
        }
    }

    fn set_batch(&mut self, list: &[(Address, Box<Account>)]) {
        elog!("SET_BATCH {:?}", list.len());
        for (addr, account) in list {
            assert_eq!(addr.length(), self.depth as usize, "addr={:?}", addr);
            self.set(addr.clone(), account.clone());
        }
    }

    fn get_at_index(&self, index: AccountIndex) -> Option<Box<Account>> {
        let addr = Address::from_index(index, self.depth as usize);
        self.get(addr)
    }

    fn set_at_index(&mut self, index: AccountIndex, account: Box<Account>) -> Result<(), ()> {
        let addr = Address::from_index(index, self.depth as usize);
        self.set(addr, account);

        Ok(())
    }

    fn index_of_account(&self, account_id: AccountId) -> Option<AccountIndex> {
        self.location_of(&account_id)
            .as_ref()
            .map(Address::to_index)
    }

    fn merkle_root(&mut self) -> Fp {
        self.root_hash()
    }

    fn merkle_path(&mut self, addr: Address) -> Vec<MerklePath> {
        elog!("merkle_path called depth={:?} addr={:?}", self.depth, addr);

        let mut merkle_path = Vec::with_capacity(addr.length());
        let mut path = addr.into_iter();
        let addr = Address::root();

        let last_account = self
            .last_filled()
            .unwrap_or_else(|| Address::first(self.depth as usize));

        self.emulate_tree_to_get_path(addr, &last_account, &mut path, &mut merkle_path);

        merkle_path
    }

    fn merkle_path_at_index(&mut self, index: AccountIndex) -> Vec<MerklePath> {
        let addr = Address::from_index(index, self.depth as usize);
        self.merkle_path(addr)
    }

    fn remove_accounts(&mut self, ids: &[AccountId]) {
        let mut addrs = ids
            .iter()
            .map(|accound_id| self.unindex_id(accound_id).unwrap())
            .collect::<Vec<_>>();
        addrs.sort_by_key(Address::to_index);

        for addr in addrs.iter().rev() {
            let account_index = addr.to_index();
            self.hashes_matrix.invalidate_hashes(account_index);

            let account = match self.remove(addr.clone()) {
                Some(account) => account,
                None => continue,
            };

            let id = account.id();
            self.unindex_id(&id);
            if !id.token_id.is_default() {
                if let Some(token_owners) = self.token_owners.as_mut() {
                    token_owners.remove(&id.derive_token_id());
                }
            }

            self.naccounts = self
                .naccounts
                .checked_sub(1)
                .expect("invalid naccounts counter");

            if self
                .last_location
                .as_ref()
                .map(|last| last == addr)
                .unwrap_or(false)
            {
                self.last_location = addr.prev();
            }
        }
    }

    fn detached_signal(&mut self) {
        todo!()
    }

    fn depth(&self) -> u8 {
        self.depth
    }

    fn num_accounts(&self) -> usize {
        self.naccounts
    }

    fn merkle_path_at_addr(&mut self, addr: Address) -> Vec<MerklePath> {
        self.merkle_path(addr)
    }

    fn get_inner_hash_at_addr(&mut self, addr: Address) -> Result<Fp, String> {
        let res = self.emulate_tree_to_get_hash_at(addr.clone());

        elog!("get_inner_hash_at_addr addr={:?} hash={}", addr, res);

        Ok(res)
    }

    fn set_inner_hash_at_addr(&mut self, _addr: Address, _hash: Fp) -> Result<(), ()> {
        // No-op for now, because we don't store the hashes anywhere
        Ok(())
    }

    fn set_all_accounts_rooted_at(
        &mut self,
        addr: Address,
        accounts: &[Box<Account>],
    ) -> Result<(), ()> {
        if addr.length() > self.depth as usize {
            return Err(());
        }

        for (child_addr, account) in addr.iter_children(self.depth as usize).zip(accounts) {
            self.set(child_addr, account.clone());
        }

        Ok(())
    }

    fn get_all_accounts_rooted_at(&self, addr: Address) -> Option<Vec<(Address, Box<Account>)>> {
        if addr.length() > self.depth as usize {
            return None;
        }

        let children = addr.iter_children(self.depth as usize);
        let mut accounts = Vec::with_capacity(children.len());

        for child_addr in children {
            let account = match self.get(child_addr.clone()) {
                Some(account) => account,
                None => continue,
            };
            accounts.push((child_addr, account));
        }

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
        // no-op
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_family = "wasm")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    use crate::scan_state::currency::Balance;

    use super::*;

    const DEPTH: u8 = 20;

    /// An account with its own public key, cheap to make: the database checks
    /// neither keys nor balances.
    fn account(i: u64) -> Box<Account> {
        let public_key = CompressedPubKey {
            x: Fp::from(i),
            is_odd: false,
        };
        let id = AccountId::new(public_key, TokenId::default());
        Box::new(Account::create_with(id, Balance::from_u64(i)))
    }

    fn add(db: &mut DatabaseImpl<V2>, i: u64) {
        let added = db.get_or_create_account(account(i).id(), *account(i));
        assert!(matches!(added, Ok(GetOrCreated::Added(_))));
    }

    fn capacity(db: &DatabaseImpl<V2>) -> usize {
        db.accounts.capacity()
    }

    #[test]
    fn account_index_validation_covers_all_derived_metadata() {
        let mut db = DatabaseImpl::<V2>::create_with_token_owners(DEPTH);
        assert_eq!(db.validate_account_indexes(), Ok(0));
        add(&mut db, 1);
        let mut token_account = account(2);
        token_account.token_id = TokenId(Fp::from(42));
        let id = token_account.id();
        db.set_at_index(AccountIndex(3), token_account).unwrap();
        assert_eq!(db.validate_account_indexes(), Ok(2));

        let mut broken = db.clone();
        broken.id_to_addr.remove(&id);
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("id-to-address"));
        let mut broken = db.clone();
        broken
            .id_to_addr
            .insert(id.clone(), Address::first(DEPTH as usize));
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("id-to-address"));
        let mut broken = db.clone();
        broken
            .id_to_addr
            .insert(account(9).id(), Address::first(DEPTH as usize));
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("index count"));
        let mut broken = db.clone();
        broken
            .token_owners
            .as_mut()
            .unwrap()
            .remove(&id.derive_token_id());
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("token-owner"));
        let mut broken = db.clone();
        broken
            .token_owners
            .as_mut()
            .unwrap()
            .insert(id.derive_token_id(), account(9).id());
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("token-owner"));
        let mut broken = db.clone();
        broken
            .token_owners
            .as_mut()
            .unwrap()
            .insert(TokenId(Fp::from(99)), id);
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("token-owner index count"));
        let mut broken = db.clone();
        broken.naccounts += 1;
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("account count"));
        let mut broken = db.clone();
        broken.last_location = Some(Address::first(DEPTH as usize));
        assert!(broken
            .validate_account_indexes()
            .unwrap_err()
            .contains("last-filled"));
        // The check never repairs or mutates the database being inspected.
        assert_eq!(db.validate_account_indexes(), Ok(2));
    }

    /// The same answers from a database in memory and from a frozen copy of
    /// it, through random changes, a refreeze and a diverging clone (PC3a).
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn frozen_database_answers_as_in_memory() {
        use rand::{rngs::StdRng, Rng, SeedableRng};

        fn check(a: &mut DatabaseImpl<V2>, b: &mut DatabaseImpl<V2>, ids: &[AccountId]) {
            assert_eq!(a.num_accounts(), b.num_accounts());
            assert_eq!(a.naccounts(), b.naccounts());
            assert_eq!(a.last_filled(), b.last_filled());
            assert_eq!(a.num_slots(), b.num_slots());
            for id in ids {
                assert_eq!(a.location_of_account(id), b.location_of_account(id));
                assert_eq!(
                    a.index_of_account(id.clone()),
                    b.index_of_account(id.clone())
                );
            }
            for index in 0..a.num_slots() + 2 {
                let addr = Address::from_index(AccountIndex(index), DEPTH as usize);
                assert_eq!(a.get(addr.clone()), b.get(addr));
            }
            assert_eq!(a.to_list(), b.to_list());
            assert_eq!(BaseLedger::accounts(a), BaseLedger::accounts(b));
            let mut a_list = vec![];
            a.iter_with_addr(|addr, account| a_list.push((addr, account.clone())));
            let mut b_list = vec![];
            b.iter_with_addr(|addr, account| b_list.push((addr, account.clone())));
            assert_eq!(a_list, b_list);
            let stop = |n: usize, _: &Account| {
                if n == 7 {
                    ControlFlow::Break(n)
                } else {
                    ControlFlow::Continue(n + 1)
                }
            };
            assert_eq!(a.fold_until(0, stop), b.fold_until(0, stop));
            if let Some(id) = ids.first() {
                assert_eq!(
                    a.tokens(id.public_key.clone()),
                    b.tokens(id.public_key.clone())
                );
            }
            assert_eq!(a.validate_account_indexes(), b.validate_account_indexes());
            assert_eq!(a.merkle_root(), b.merkle_root());
            let last = AccountIndex(a.num_slots().saturating_sub(1));
            assert_eq!(a.merkle_path_at_index(last), b.merkle_path_at_index(last));
        }

        let dir = tempfile::tempdir().unwrap();
        let mut rng = StdRng::seed_from_u64(0x9c3a);
        let mut a = DatabaseImpl::<V2>::create_with_token_owners(DEPTH);
        let mut ids = vec![];
        for _ in 0..300 {
            let account = Account::rand();
            ids.push(account.id());
            a.get_or_create_account(account.id(), account).unwrap();
        }
        // An id never in the ledger, and an empty slot past a gap.
        ids.push(Account::rand().id());
        let gap = Account::rand();
        ids.push(gap.id());
        a.set_at_index(AccountIndex(310), Box::new(gap)).unwrap();

        let mut b = a.clone_db(dir.path().join("b"));
        let report = b.freeze_to_disk(dir.path()).unwrap().unwrap();
        assert_eq!(report.accounts, 301);
        assert!(!report.refreeze);
        assert!(b.accounts.is_empty() && b.id_to_addr.is_empty());
        assert!(b.freeze_to_disk(dir.path()).unwrap().is_none());
        check(&mut a, &mut b, &ids);

        let mut clone = None;
        for round in 0..6 {
            for _ in 0..60 {
                let slots = a.num_slots();
                match rng.gen_range(0..6) {
                    // A new account, or one that exists.
                    0 => {
                        let account = if rng.gen_bool(0.8) {
                            Account::rand()
                        } else {
                            let id = &ids[rng.gen_range(0..ids.len())];
                            let mut account = Account::rand();
                            account.public_key = id.public_key.clone();
                            account.token_id = id.token_id.clone();
                            account
                        };
                        ids.push(account.id());
                        let x = a.get_or_create_account(account.id(), account.clone());
                        let y = b.get_or_create_account(account.id(), account);
                        assert_eq!(format!("{x:?}"), format!("{y:?}"));
                    }
                    // An account changed in place.
                    1 | 2 => {
                        let index = AccountIndex(rng.gen_range(0..slots));
                        if let Some(mut account) = a.get_at_index(index) {
                            account.balance = Balance::from_u64(rng.gen());
                            a.set_at_index(index, account.clone()).unwrap();
                            b.set_at_index(index, account).unwrap();
                        }
                    }
                    // Another account in an existing slot, or past the end.
                    3 => {
                        let index = AccountIndex(rng.gen_range(0..slots + 3));
                        let account = Box::new(Account::rand());
                        ids.push(account.id());
                        a.set_at_index(index, account.clone()).unwrap();
                        b.set_at_index(index, account).unwrap();
                    }
                    // Removed accounts, then the hashes recomputed.
                    4 => {
                        let index = AccountIndex(rng.gen_range(0..slots));
                        if let Some(account) = a.get_at_index(index) {
                            a.remove_accounts(&[account.id()]);
                            b.remove_accounts(&[account.id()]);
                        }
                    }
                    _ => assert_eq!(a.merkle_root(), b.merkle_root()),
                }
            }
            check(&mut a, &mut b, &ids);
            match round {
                // Changes written into a new file.
                2 => {
                    let report = b.freeze_to_disk(dir.path()).unwrap().unwrap();
                    assert!(report.refreeze);
                    assert_eq!(b.disk_overlay_len(), 0);
                    check(&mut a, &mut b, &ids);
                    clone = Some((a.clone(), b.clone_db(dir.path().join("c"))));
                }
                // A clone sharing the file goes its own way.
                4 => {
                    let (mut a2, mut b2) = clone.take().unwrap();
                    let account = Box::new(Account::rand());
                    a2.set_at_index(AccountIndex(0), account.clone()).unwrap();
                    b2.set_at_index(AccountIndex(0), account).unwrap();
                    check(&mut a2, &mut b2, &ids);
                    assert_ne!(a2.merkle_root(), a.merkle_root());
                }
                _ => {}
            }
        }

        // Two files were written; the first went with the refreeze, and the
        // second goes with the last database that shares it.
        let files = || std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(files(), 1);
        drop(b);
        assert_eq!(files(), 0);
    }

    /// PC3b: a database written again and again, as the root snarked ledger
    /// is, answers as one in memory each time. The records that didn't
    /// change are copied as they are; repeated reads come from the cache;
    /// each refreeze's old file goes with its last holder.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn repeated_refreezes_answer_as_in_memory() {
        use rand::{rngs::StdRng, Rng, SeedableRng};

        let dir = tempfile::tempdir().unwrap();
        let files = || std::fs::read_dir(dir.path()).unwrap().count();
        let mut rng = StdRng::seed_from_u64(0x9c3b);
        let mut a = DatabaseImpl::<V2>::create_with_token_owners(DEPTH);
        for i in 0..400 {
            add(&mut a, i);
        }
        let mut b = a.clone_db(dir.path().join("b"));
        b.freeze_to_disk(dir.path()).unwrap().unwrap();
        let mut next = 400;
        for round in 0..8 {
            for _ in 0..40 {
                let slots = a.num_slots();
                if rng.gen_bool(0.2) {
                    add(&mut a, next);
                    add(&mut b, next);
                    next += 1;
                } else {
                    let index = AccountIndex(rng.gen_range(0..slots));
                    let mut account = a.get_at_index(index).unwrap();
                    account.balance = Balance::from_u64(rng.gen());
                    a.set_at_index(index, account.clone()).unwrap();
                    b.set_at_index(index, account).unwrap();
                }
            }
            let changed = b.disk_overlay_len() as u64;
            let copies = super::super::disk::stats().raw_copies;
            let report = b.freeze_to_disk(dir.path()).unwrap().unwrap();
            assert!(report.refreeze);
            assert_eq!(report.changed, changed);
            assert_eq!(b.disk_overlay_len(), 0);
            // Every other record was copied, not encoded (other tests'
            // refreezes may add to the process-wide count).
            assert!(super::super::disk::stats().raw_copies - copies >= b.num_slots() - changed);
            assert_eq!(files(), 1, "round {round}: the old file went");
            assert_eq!(a.merkle_root(), b.merkle_root());
            assert_eq!(a.to_list(), b.to_list());
            for i in 0..next + 2 {
                let id = account(i).id();
                assert_eq!(a.location_of_account(&id), b.location_of_account(&id));
            }
            assert_eq!(a.validate_account_indexes(), b.validate_account_indexes());
        }
        // A slot read twice is decoded once.
        let hits = super::super::disk::stats().cache_hits;
        let index = AccountIndex(7);
        assert_eq!(b.get_at_index(index), a.get_at_index(index));
        assert_eq!(b.get_at_index(index), a.get_at_index(index));
        assert!(super::super::disk::stats().cache_hits > hits);
        assert!(b.disk_cached().unwrap() >= 1);
    }

    /// PC3b: a refreeze that can't write its file leaves the database, its
    /// file and its overlay as they were, and a later one succeeds.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn a_failed_refreeze_leaves_the_database_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = DatabaseImpl::<V2>::create(DEPTH);
        for i in 0..50 {
            add(&mut a, i);
        }
        let mut b = a.clone_db(dir.path().join("b"));
        b.freeze_to_disk(dir.path()).unwrap().unwrap();
        for i in 50..60 {
            add(&mut a, i);
            add(&mut b, i);
        }
        let missing = dir.path().join("missing");
        assert!(b.freeze_to_disk(&missing).is_err());
        assert_eq!(b.disk_overlay_len(), 10);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(a.to_list(), b.to_list());
        assert_eq!(a.merkle_root(), b.merkle_root());
        let retry_at = b.freeze_retry_at.unwrap();
        assert!(
            b.freeze_to_disk_at(dir.path(), retry_at)
                .unwrap()
                .unwrap()
                .refreeze
        );
        assert_eq!(a.to_list(), b.to_list());
        assert_eq!(a.merkle_root(), b.merkle_root());
    }

    /// REL-REVIEW's file-size-limit reproduction with LEDGER-0201 backoff:
    /// failed writes leave the database and clones intact, remove the partial
    /// file and do not count a freeze. Idle retries do no I/O; after the retry
    /// deadline, the refreeze succeeds once there is room again.
    #[cfg(target_os = "linux")]
    #[test]
    fn ledger_0201_refreeze_backs_off_after_a_partial_write() {
        const CHILD: &str = "MINA_TREE_LEDGER_0201_FSIZE";
        if let Some(dir) = std::env::var_os(CHILD) {
            let dir = PathBuf::from(dir);
            let mut memory = DatabaseImpl::<V2>::create(DEPTH);
            for i in 0..12_000 {
                add(&mut memory, i);
            }
            let mut disk = memory.clone_db(dir.join("disk"));
            let report = disk.freeze_to_disk(&dir).unwrap().unwrap();
            let mut old = disk.clone_db(dir.join("old"));
            let mut old_memory = memory.clone();
            let stats = super::super::disk::stats;
            for i in 12_000..12_010 {
                add(&mut memory, i);
                add(&mut disk, i);
            }
            let set_limit = |bytes| {
                // SAFETY: only this isolated child changes its own limit/signal.
                unsafe {
                    libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
                    let limit = libc::rlimit {
                        rlim_cur: bytes,
                        rlim_max: libc::RLIM_INFINITY,
                    };
                    assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
                }
            };
            let read_bytes = || -> u64 {
                std::fs::read_to_string("/proc/self/io")
                    .unwrap()
                    .lines()
                    .find_map(|line| line.strip_prefix("rchar: "))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap()
            };
            let start = redux::Instant::now();
            set_limit(report.file_bytes / 2);
            let before_stats = stats();
            let before = read_bytes();
            assert!(disk.freeze_to_disk_at(&dir, start).is_err());
            let failed_reads = read_bytes() - before;
            assert!(failed_reads >= report.file_bytes / 4);
            assert_eq!(stats().freezes, before_stats.freezes, "no freeze counted");
            assert_eq!(
                std::fs::read_dir(&dir).unwrap().count(),
                1,
                "the first partial file is removed"
            );
            let mut copy = disk.clone_db(dir.join("copy"));
            let before = read_bytes();
            for seconds in [0, 1, 10, 30, 59] {
                let now = start + std::time::Duration::from_secs(seconds);
                assert!(disk.freeze_to_disk_at(&dir, now).unwrap().is_none());
                assert!(copy.freeze_to_disk_at(&dir, now).unwrap().is_none());
            }
            let deferred_reads = read_bytes() - before;
            assert!(
                deferred_reads < 1024,
                "deferred attempts read {deferred_reads} bytes"
            );
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
            assert_eq!(disk.disk_overlay_len(), 10);
            assert_eq!(disk.to_list(), memory.to_list());
            assert_eq!(copy.to_list(), memory.to_list());
            assert_eq!(disk.merkle_root(), memory.merkle_root());
            assert_eq!(copy.merkle_root(), memory.merkle_root());
            assert_eq!(old.to_list(), old_memory.to_list());
            assert_eq!(old.merkle_root(), old_memory.merkle_root());
            // An eligible retry still fails and doubles the delay.
            assert!(disk
                .freeze_to_disk_at(&dir, start + std::time::Duration::from_secs(60))
                .is_err());
            assert_eq!(stats().freezes, before_stats.freezes, "no freeze counted");
            assert_eq!(
                std::fs::read_dir(&dir).unwrap().count(),
                1,
                "the second partial file is removed"
            );
            assert_eq!(disk.disk_overlay_len(), 10);
            assert_eq!(disk.to_list(), memory.to_list());
            assert_eq!(disk.merkle_root(), memory.merkle_root());
            set_limit(libc::RLIM_INFINITY);
            assert!(disk
                .freeze_to_disk_at(&dir, start + std::time::Duration::from_secs(179))
                .unwrap()
                .is_none());
            assert!(
                disk.freeze_to_disk_at(&dir, start + std::time::Duration::from_secs(180))
                    .unwrap()
                    .unwrap()
                    .refreeze
            );
            assert_eq!(disk.disk_overlay_len(), 0);
            assert_eq!(stats().freezes, before_stats.freezes + 1);
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
            assert_eq!(disk.to_list(), memory.to_list());
            assert_eq!(disk.merkle_root(), memory.merkle_root());
            assert_eq!(copy.to_list(), memory.to_list());
            assert_eq!(copy.merkle_root(), memory.merkle_root());
            assert_eq!(old.to_list(), old_memory.to_list());
            assert_eq!(old.merkle_root(), old_memory.merkle_root());
            println!("partial write read {failed_reads} bytes; ten deferred calls read {deferred_reads} bytes; recovery and clones correct");
            drop((old, copy, disk));
            assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
            println!("CHILD done");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let name = format!(
            "{}::ledger_0201_refreeze_backs_off_after_a_partial_write",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture", "--test-threads", "1"])
            .env(CHILD, dir.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stdout}\n{stderr}");
        assert!(stdout.contains("CHILD done"), "{stdout}");
        println!("{stdout}");
    }

    #[test]
    fn ledger_0201_initial_freeze_retry_is_capped_and_resets_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        add(&mut db, 0);
        let mut now = redux::Instant::now();
        for delay in [60, 120, 240, 480, 960, 1920, 3600, 3600] {
            assert!(db.freeze_to_disk_at(&missing, now).is_err());
            assert!(db
                .freeze_to_disk_at(dir.path(), now + std::time::Duration::from_secs(delay - 1))
                .unwrap()
                .is_none());
            now += std::time::Duration::from_secs(delay);
        }
        assert!(db.freeze_to_disk_at(dir.path(), now).unwrap().is_some());
        add(&mut db, 1);
        assert!(db.freeze_to_disk_at(&missing, now).is_err());
        now += std::time::Duration::from_secs(60);
        assert!(db.freeze_to_disk_at(dir.path(), now).unwrap().is_some());
    }

    /// A frozen database reads through the descriptor it wrote its file with,
    /// so a file removed while the node runs is still read whole: single
    /// reads, scans and a refreeze (PC3c: no read the block producer relies on
    /// may fail on a healthy node).
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn frozen_database_reads_a_removed_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        for i in 0..20 {
            add(&mut db, i);
        }
        let expected = db.to_list();
        let root = db.merkle_root();
        db.freeze_to_disk(dir.path()).unwrap().unwrap();
        std::fs::remove_file(db.disk.as_ref().unwrap().path()).unwrap();
        assert_eq!(db.get_at_index(AccountIndex(7)).as_deref(), expected.get(7));
        assert_eq!(db.to_list(), expected);
        db.set_at_index(AccountIndex(3), account(1003)).unwrap();
        db.freeze_to_disk(dir.path()).unwrap().unwrap();
        assert_eq!(db.to_list().len(), expected.len());
        assert_ne!(db.merkle_root(), root);
    }

    /// A read the disk got wrong once is read again, from the disk, and gives
    /// the account: single reads, lookups by id, scans and a refreeze's copy.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn frozen_database_reads_again_what_the_disk_got_wrong() {
        use std::sync::atomic::Ordering::Relaxed;

        let dir = tempfile::tempdir().unwrap();
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        for i in 0..20 {
            add(&mut db, i);
        }
        let expected = db.to_list();
        db.freeze_to_disk(dir.path()).unwrap().unwrap();
        fn fail(db: &DatabaseImpl<V2>, reads: u32) {
            db.disk.as_ref().unwrap().fail_reads.store(reads, Relaxed);
        }
        let retries = super::super::disk::stats().read_retries;

        // Two bad reads, then a good one.
        fail(&db, super::super::disk::READ_ATTEMPTS - 1);
        assert_eq!(db.get_at_index(AccountIndex(7)).as_deref(), expected.get(7));
        fail(&db, 1);
        assert_eq!(
            db.location_of_account(&account(11).id()),
            Some(Address::from_index(AccountIndex(11), DEPTH as usize))
        );
        fail(&db, 1);
        assert_eq!(db.to_list(), expected);
        assert!(super::super::disk::stats().read_retries >= retries + 4);

        db.set_at_index(AccountIndex(3), account(1003)).unwrap();
        fail(&db, 1);
        db.freeze_to_disk(dir.path()).unwrap().unwrap();
        let mut changed = expected.clone();
        changed[3] = *account(1003);
        assert_eq!(db.to_list(), changed);
    }

    /// A record that stays unreadable, here cut short, ends a node's process
    /// with status 1 and one line saying why, rather than a panic unwinding
    /// through the ledger. Run in a child process of this test binary.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn an_unreadable_record_ends_the_process_without_a_panic() {
        const CHILD: &str = "MINA_TREE_TEST_UNREADABLE_DIR";
        if let Some(dir) = std::env::var_os(CHILD) {
            super::super::disk::set_exit_on_unreadable(true);
            super::super::disk::set_before_unreadable_exit(|| eprintln!("before the exit"));
            let mut db = DatabaseImpl::<V2>::create(DEPTH);
            for i in 0..10 {
                add(&mut db, i);
            }
            db.freeze_to_disk(std::path::Path::new(&dir))
                .unwrap()
                .unwrap();
            let path = db.disk.as_ref().unwrap().path().to_owned();
            let len = std::fs::metadata(&path).unwrap().len();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(len / 2).unwrap();
            drop(file);
            db.get_at_index(AccountIndex(9));
            unreachable!("the read returned");
        }
        let dir = tempfile::tempdir().unwrap();
        let name = format!(
            "{}::an_unreadable_record_ends_the_process_without_a_panic",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture", "--test-threads", "1"])
            .env(CHILD, dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("Fatal: ledger accounts file"), "{stderr}");
        // The node drains its file logger there.
        let fatal = stderr.find("Fatal: ledger accounts file").unwrap();
        assert!(stderr[fatal..].contains("before the exit"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
    }

    /// A refreeze, which copies the unchanged records from the file, whose
    /// source record stays unreadable ends the process as any other read
    /// does, status 1 and one line saying why, and leaves no new file: an
    /// error would leave the node running on a file it can't read. Here the
    /// file is cut short after a change; the child exits 0 if the refreeze
    /// returns. Run in a child process of this test binary.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    fn an_unreadable_record_ends_a_refreeze_without_a_panic() {
        const CHILD: &str = "MINA_TREE_TEST_UNREADABLE_REFREEZE_DIR";
        if let Some(dir) = std::env::var_os(CHILD) {
            super::super::disk::set_exit_on_unreadable(true);
            let dir = std::path::Path::new(&dir);
            let mut db = DatabaseImpl::<V2>::create(DEPTH);
            for i in 0..10 {
                add(&mut db, i);
            }
            db.freeze_to_disk(dir).unwrap().unwrap();
            db.set_at_index(AccountIndex(2), account(1002)).unwrap();
            let path = db.disk.as_ref().unwrap().path().to_owned();
            let len = std::fs::metadata(&path).unwrap().len();
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(len / 2).unwrap();
            drop(file);
            let result = db.freeze_to_disk(dir);
            eprintln!("the refreeze returned: {result:?}");
            std::process::exit(0);
        }
        let dir = tempfile::tempdir().unwrap();
        let name = format!(
            "{}::an_unreadable_record_ends_a_refreeze_without_a_panic",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture", "--test-threads", "1"])
            .env(CHILD, dir.path())
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains("Fatal: ledger accounts file"), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
        // The old file only: the new one was removed.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// A record whose bytes changed on disk is never decoded: the read fails.
    #[cfg(not(target_family = "wasm"))]
    #[test]
    #[should_panic(expected = "checksum mismatch")]
    fn frozen_database_refuses_a_changed_record() {
        use std::io::{Seek, SeekFrom, Write};

        let dir = tempfile::tempdir().unwrap();
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        for i in 0..10 {
            add(&mut db, i);
        }
        db.freeze_to_disk(dir.path()).unwrap().unwrap();
        let path = db.disk.as_ref().unwrap().path().to_owned();
        let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.seek(SeekFrom::Start(3)).unwrap();
        file.write_all(&[0xff]).unwrap();
        drop(file);
        db.get_at_index(AccountIndex(0));
    }

    #[test]
    fn reserve_accounts_grows_by_an_eighth() {
        let mut accounts: Vec<u64> = Vec::with_capacity(800);
        accounts.resize(800, 0);

        reserve_accounts(&mut accounts, 801);
        assert_eq!(accounts.capacity(), 900);
        assert_eq!(accounts.len(), 800);

        // Within the capacity, nothing moves.
        reserve_accounts(&mut accounts, 900);
        assert_eq!(accounts.capacity(), 900);

        // Past the next eighth, exactly what is asked for.
        reserve_accounts(&mut accounts, 5_000);
        assert_eq!(accounts.capacity(), 5_000);
        assert_eq!(accounts.len(), 800);
    }

    #[test]
    fn reserve_accounts_from_empty() {
        let mut accounts: Vec<u64> = Vec::new();
        reserve_accounts(&mut accounts, 1);
        assert_eq!(accounts.capacity(), 1);

        // Below eight slots an eighth rounds down to nothing, so each push
        // takes one slot more; a new database starts with room for 10,000.
        accounts.push(0);
        reserve_accounts(&mut accounts, 2);
        assert_eq!(accounts.capacity(), 2);
    }

    #[test]
    fn reserve_accounts_leaves_at_most_an_eighth_unused() {
        let mut accounts: Vec<u64> = Vec::with_capacity(10_000);
        let mut grown = 0;

        for i in 0..300_000 {
            let before = accounts.capacity();
            let len = accounts.len() + 1;
            reserve_accounts(&mut accounts, len);
            if accounts.capacity() != before {
                assert_eq!(accounts.capacity(), before + before / 8);
                grown += 1;
            }
            accounts.push(i);
        }

        // Doubling would have grown it 5 times, to 320,000 slots.
        assert_eq!(grown, 29);
        assert_eq!(accounts.capacity(), 304_303);
        assert!(accounts.capacity() <= accounts.len() + accounts.len() / 8);
    }

    #[test]
    fn new_accounts_grow_by_an_eighth() {
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        assert_eq!(capacity(&db), 10_000);

        for i in 0..10_001 {
            add(&mut db, i);
        }

        // Doubling gave 20,000.
        assert_eq!(capacity(&db), 11_250);
        assert_eq!(db.num_accounts(), 10_001);
    }

    #[test]
    fn set_accounts_grow_by_an_eighth() {
        // A ledger synced from empty may get an account near its end first.
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        db.set_at_index(AccountIndex(20_000), account(20_000))
            .unwrap();
        assert_eq!(capacity(&db), 20_001);

        // The next one past the end grows it by an eighth: doubling gave
        // 40,002.
        db.set_at_index(AccountIndex(20_001), account(20_001))
            .unwrap();
        assert_eq!(capacity(&db), 22_501);

        // Filling in below the end changes nothing.
        db.set_at_index(AccountIndex(5), account(5)).unwrap();
        db.set_at_index(AccountIndex(22_500), account(22_500))
            .unwrap();
        assert_eq!(capacity(&db), 22_501);
        assert_eq!(db.num_accounts(), 4);
    }

    #[test]
    fn copies_grow_by_an_eighth() {
        // A copy has room for exactly its accounts until a new one comes.
        let mut db = DatabaseImpl::<V2>::create(DEPTH);
        db.set_at_index(AccountIndex(15_999), account(15_999))
            .unwrap();
        let mut copy = db.clone_db(PathBuf::from("/tmp/copies_grow_by_an_eighth"));
        assert_eq!(capacity(&copy), 16_000);

        add(&mut copy, 16_000);
        // Doubling gave 32,000.
        assert_eq!(capacity(&copy), 18_000);
        assert_eq!(capacity(&db), 16_000);
    }

    #[test]
    fn growth_leaves_hashes_alone() {
        // Accounts added in order to a copy of an empty database, which has
        // no room at all and so grows one slot at a time at first, and the
        // same accounts set from the last to the first, as a sync might, give
        // the same ledger.
        const N: u64 = 300;
        let empty = DatabaseImpl::<V2>::create(DEPTH);

        let mut added = empty.clone_db(PathBuf::from("/tmp/growth_leaves_hashes_alone"));
        assert_eq!(capacity(&added), 0);
        for i in 0..N {
            add(&mut added, i);
        }
        assert_eq!(capacity(&added), 325);

        let mut set = DatabaseImpl::<V2>::create(DEPTH);
        for i in (0..N).rev() {
            set.set_at_index(AccountIndex(i), account(i)).unwrap();
        }

        assert_eq!(added.num_accounts(), N as usize);
        assert_eq!(set.num_accounts(), N as usize);
        assert_eq!(added.merkle_root(), set.merkle_root());
        for i in 0..N {
            let addr = Address::from_index(AccountIndex(i), DEPTH as usize);
            assert_eq!(added.get(addr.clone()), set.get(addr));
        }
    }
}
