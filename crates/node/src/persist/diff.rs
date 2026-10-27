//! Ledger diffs by Merkle walk (R20 D2).
//!
//! [`merkle_diff`] descends both ledgers' hash matrices from the root and
//! enters a subtree only where the two differ, so it costs O(changed × depth)
//! lookups and no hashing, as long as both ledgers' hashes are computed (it
//! forces their roots first). The diff holds the target's accounts at the
//! changed leaves and the target's hashes of every node on their paths: exactly
//! the nodes whose hashes setting those accounts on a copy of the base clears.

use std::{
    collections::HashSet,
    sync::{Arc, OnceLock},
};

use ledger::{Account, AccountIndex, Address, BaseLedger, HashesMatrix, Mask};
use mina_curves::pasta::Fp;

use super::ledger_codec::{check_depth, CodecError, FullLedgerSource, MAX_DEPTH};

/// Above this share of the target's accounts, a full file is written instead.
pub const FULL_ABOVE: f64 = 0.4;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DiffError {
    #[error("unsupported ledger depth {0}")]
    UnsupportedDepth(u8),
    #[error("the ledgers have different depths, {0} and {1}")]
    Depth(u8, u8),
    #[error("the ledgers keep token owners differently")]
    TokenOwners,
    #[error("account {0} is in the base but not in the target, and a diff can't remove accounts")]
    Removed(u64),
}

/// A ledger as its differences from another one.
#[derive(Debug, Clone)]
pub struct LedgerDiff {
    pub base: Fp,
    pub target: Fp,
    pub depth: u8,
    pub num_accounts: u64,
    pub token_owners: bool,
    /// The target's accounts at the leaves that differ, by index.
    pub accounts: Vec<(AccountIndex, Box<Account>)>,
    /// The target's hashes of the nodes that differ, leaves included, by
    /// linear index (`Address::to_linear_index`).
    pub hashes: Vec<(u64, Fp)>,
}

/// What to write for a ledger: all of it, or a diff over a stored ledger.
#[derive(Debug, Clone)]
pub enum LedgerWrite {
    /// All of a ledger the writing thread owns: no other thread may use the
    /// mask or its ancestors while it is written (see [`FullLedgerSource`]).
    Full(Mask),
    /// All of a ledger another thread owns, a piece at a time.
    Stream(Arc<dyn FullLedgerSource>),
    Diff(LedgerDiff),
}

impl LedgerWrite {
    /// Whether this writes a whole ledger rather than a diff.
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full(_) | Self::Stream(_))
    }
}

/// The hash of an empty subtree of the given height (0 for an empty leaf).
pub fn empty_hash(height: usize) -> Fp {
    static EMPTY: OnceLock<Vec<Fp>> = OnceLock::new();
    let empty = EMPTY.get_or_init(|| {
        let mut matrix = HashesMatrix::new((MAX_DEPTH + 1) as usize);
        (0..=MAX_DEPTH as usize)
            .map(|h| matrix.empty_hash_at_height(h))
            .collect()
    });
    empty[height]
}

/// The index of the first leaf under `addr`.
pub(crate) fn first_leaf(addr: &Address, depth: u8) -> u64 {
    addr.to_index().0 << (depth as usize - addr.length())
}

/// A node's hash, without hashing for subtrees past the last account.
fn node_hash(ledger: &mut Mask, addr: &Address, depth: u8, num_accounts: u64) -> Fp {
    if first_leaf(addr, depth) >= num_accounts {
        return empty_hash(depth as usize - addr.length());
    }
    ledger
        .get_inner_hash_at_addr(addr.clone())
        .expect("the address is within the ledger's depth")
}

/// The diff that turns `base` into `target`.
pub fn merkle_diff(target: &Mask, base: &Mask) -> Result<LedgerDiff, DiffError> {
    Ok(merkle_diff_bounded(target, base, usize::MAX)?.expect("unbounded"))
}

/// As [`merkle_diff`], but gives up with `None` once more than `max_accounts`
/// accounts differ.
pub fn merkle_diff_bounded(
    target: &Mask,
    base: &Mask,
    max_accounts: usize,
) -> Result<Option<LedgerDiff>, DiffError> {
    let mut target = target.clone();
    let mut base = base.clone();
    let depth = target.depth();
    for depth in [depth, base.depth()] {
        if check_depth(depth as u32).is_err() {
            return Err(DiffError::UnsupportedDepth(depth));
        }
    }
    if base.depth() != depth {
        return Err(DiffError::Depth(depth, base.depth()));
    }
    if target.has_token_owners() != base.has_token_owners() {
        return Err(DiffError::TokenOwners);
    }
    let target_hash = target.merkle_root();
    let base_hash = base.merkle_root();
    let target_accounts = target.num_accounts() as u64;
    let base_accounts = base.num_accounts() as u64;

    let mut accounts = Vec::new();
    let mut hashes = Vec::new();
    let mut stack = vec![Address::root()];
    while let Some(addr) = stack.pop() {
        let target_node = node_hash(&mut target, &addr, depth, target_accounts);
        let base_node = node_hash(&mut base, &addr, depth, base_accounts);
        if target_node == base_node {
            continue;
        }
        hashes.push((addr.to_linear_index(), target_node));
        if addr.length() == depth as usize {
            let index = addr.to_index();
            let account = target
                .get_at_index(index)
                .ok_or(DiffError::Removed(index.0))?;
            accounts.push((index, account));
            if accounts.len() > max_accounts {
                return Ok(None);
            }
        } else {
            // Left first, so that leaves come out in index order.
            stack.push(addr.child_right());
            stack.push(addr.child_left());
        }
    }
    hashes.sort_unstable_by_key(|(index, _)| *index);

    Ok(Some(LedgerDiff {
        base: base_hash,
        target: target_hash,
        depth,
        num_accounts: target_accounts,
        token_owners: target.has_token_owners(),
        accounts,
        hashes,
    }))
}

/// A full file, or the smallest diff over one of `bases` (ledgers that are
/// stored and in memory) if it holds at most [`FULL_ABOVE`] of the accounts.
/// A base a diff can't be taken against (another depth, accounts removed) is
/// skipped.
pub fn choose_ledger_write(target: &Mask, bases: &[Mask]) -> LedgerWrite {
    let mut limit = (target.num_accounts() as f64 * FULL_ABOVE) as usize;
    let mut best = None;
    for base in bases {
        if let Ok(Some(diff)) = merkle_diff_bounded(target, base, limit) {
            limit = diff.accounts.len().saturating_sub(1);
            best = Some(diff);
            if limit == 0 {
                break;
            }
        }
    }
    match best {
        Some(diff) => LedgerWrite::Diff(diff),
        None => LedgerWrite::Full(target.clone()),
    }
}

/// The address of a node by its linear index, if the ledger has it.
pub(crate) fn address_of_linear(linear: u64, depth: u8) -> Option<Address> {
    check_depth(depth as u32).ok()?;
    let level = 63 - (linear.checked_add(1)?).leading_zeros() as usize;
    if level > depth as usize {
        return None;
    }
    let index = linear + 1 - (1u64 << level);
    Some(Address::from_index(AccountIndex(index), level))
}

/// Builds a ledger from its base and the diff's entries, as they are read:
/// accounts by increasing index, then hashes by increasing linear index. The
/// ledger is a copy of the base ([`DiffApplier::new`]) or a child mask of it
/// ([`DiffApplier::over_copy`]). [`DiffApplier::finish`] checks that the
/// hashes are exactly those of the nodes the accounts changed, so the result
/// has no stale hash and needs no hashing.
pub(crate) struct DiffApplier {
    ledger: Mask,
    depth: u8,
    /// The target's account count, from the diff's header.
    num_accounts: u64,
    last_index: Option<u64>,
    last_linear: Option<u64>,
    changed: Vec<u64>,
    /// The linear indices of the nodes on the changed accounts' paths, once
    /// the first hash is read (the accounts come first).
    on_paths: Option<HashSet<u64>>,
    hashes: usize,
}

impl DiffApplier {
    pub(crate) fn new(
        base: &Mask,
        token_owners: bool,
        num_accounts: u64,
    ) -> Result<Self, CodecError> {
        Self::check_base(base, token_owners, num_accounts)?;
        // A root mask's copy is a copy of its database.
        Ok(Self::over_copy(base.copy(), num_accounts))
    }

    /// Over `ledger`, a copy ([`Self::new`]) or a child mask of a base
    /// [`Self::check_base`] accepted, made by the caller: the root staged
    /// ledger is a child mask of the root snarked ledger, which keeps it as
    /// its child (I5d5; OCaml's persistent frontier builds the restored root
    /// staged ledger over a mask of the root ledger database,
    /// `Root_ledger.as_masked`, and the mask holds only the diff's accounts
    /// and hashes), and the copies of a round's bases are made on one thread
    /// before its diffs are read in parallel (I5d4).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transition_frontier/persistent_frontier/persistent_frontier.ml#L59-L73>
    pub(crate) fn over_copy(ledger: Mask, num_accounts: u64) -> Self {
        Self {
            depth: ledger.depth(),
            num_accounts,
            ledger,
            last_index: None,
            last_linear: None,
            changed: Vec::new(),
            on_paths: None,
            hashes: 0,
        }
    }

    /// Whether a diff of `num_accounts` accounts can apply over `base`.
    pub(crate) fn check_base(
        base: &Mask,
        token_owners: bool,
        num_accounts: u64,
    ) -> Result<(), CodecError> {
        check_depth(base.depth() as u32)?;
        if num_accounts > 1u64 << base.depth() {
            return Err(CodecError::Invalid("account count out of range".into()));
        }
        if !base.is_root() {
            return Err(CodecError::Invalid(
                "a diff's base must be a root ledger".into(),
            ));
        }
        if base.has_token_owners() != token_owners {
            return Err(CodecError::Invalid(
                "the diff and its base keep token owners differently".into(),
            ));
        }
        if num_accounts < base.num_accounts() as u64 {
            return Err(CodecError::Invalid(
                "the diff has fewer accounts than its base".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn account(&mut self, index: u64, account: Box<Account>) -> Result<(), CodecError> {
        if self.on_paths.is_some() {
            return Err(CodecError::Invalid(format!(
                "account index {index} after the hashes"
            )));
        }
        if self.last_index.is_some_and(|last| index <= last) {
            return Err(CodecError::Invalid(format!(
                "account index {index} out of order"
            )));
        }
        // Within the target, and no further than the next free index: the
        // ledger grows its account list up to the index it sets, so an index
        // from a damaged file must not reach far past the end.
        if index >= self.num_accounts || index > self.ledger.num_accounts() as u64 {
            return Err(CodecError::Invalid(format!(
                "account index {index} past the ledger's accounts"
            )));
        }
        self.last_index = Some(index);
        self.changed.push(index);
        self.ledger
            .set_at_index(AccountIndex(index), account)
            .map_err(|()| CodecError::Invalid(format!("can't set account {index}")))
    }

    pub(crate) fn hash(&mut self, linear: u64, hash: Fp) -> Result<(), CodecError> {
        if self.last_linear.is_some_and(|last| linear <= last) {
            return Err(CodecError::Invalid(format!(
                "hash index {linear} out of order"
            )));
        }
        self.last_linear = Some(linear);
        let addr = address_of_linear(linear, self.depth)
            .ok_or_else(|| CodecError::Invalid(format!("hash index {linear} out of range")))?;
        // Any node off the changed paths has the base's hash, which a diff
        // must not replace. (A copy's hashes there are still cached, a child
        // mask's are its parent's: the paths are checked, not the cache.)
        let (changed, depth) = (&self.changed, self.depth);
        let on_paths = self
            .on_paths
            .get_or_insert_with(|| changed_paths(changed, depth));
        if !on_paths.contains(&linear) {
            return Err(CodecError::Invalid(format!(
                "hash index {linear} is not on a changed path"
            )));
        }
        self.ledger.set_cached_hash_unchecked(&addr, hash);
        self.hashes += 1;
        Ok(())
    }

    /// The ledger, and the indices of the accounts the diff set.
    pub(crate) fn finish(self) -> Result<(Mask, Vec<u64>), CodecError> {
        let on_paths = self
            .on_paths
            .unwrap_or_else(|| changed_paths(&self.changed, self.depth));
        // Every hash set was on a changed path (checked in `hash`), and all
        // were distinct, so equal counts mean every changed node got its hash.
        if on_paths.len() != self.hashes {
            return Err(CodecError::Invalid(format!(
                "the diff has {} hashes for {} changed nodes",
                self.hashes,
                on_paths.len()
            )));
        }
        Ok((self.ledger, self.changed))
    }
}

/// The linear indices of the nodes on the paths from the leaves `changed` to
/// the root.
fn changed_paths(changed: &[u64], depth: u8) -> HashSet<u64> {
    let mut on_paths = HashSet::new();
    for index in changed {
        let mut addr = Address::from_index(AccountIndex(*index), depth as usize);
        loop {
            if !on_paths.insert(addr.to_linear_index()) {
                break;
            }
            match addr.parent() {
                Some(parent) => addr = parent,
                None => break,
            }
        }
    }
    on_paths
}

/// `base` with `diff` applied, as a new root ledger.
pub fn apply_diff(base: &Mask, diff: &LedgerDiff) -> Result<Mask, CodecError> {
    check_depth(diff.depth as u32)?;
    check_depth(base.depth() as u32)?;
    if diff.depth != base.depth() {
        return Err(CodecError::Invalid("the base has another depth".into()));
    }
    let mut base_root = base.clone();
    if base_root.merkle_root() != diff.base {
        return Err(CodecError::WrongBase);
    }
    let mut applier = DiffApplier::new(base, diff.token_owners, diff.num_accounts)?;
    for (index, account) in &diff.accounts {
        applier.account(index.0, account.clone())?;
    }
    for (linear, hash) in &diff.hashes {
        applier.hash(*linear, *hash)?;
    }
    let (mut ledger, _) = applier.finish()?;
    if ledger.merkle_root() != diff.target {
        return Err(CodecError::RootMismatch);
    }
    Ok(ledger)
}
