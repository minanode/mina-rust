//! Collections that share one allocation per pooled command.
//!
//! The pool indexes each command in up to five maps (by sender, by fee, by
//! hash, applicable by fee and by expiry), as OCaml's `Indexed_pool` does.
//! OCaml values are shared by reference, so each index holds a pointer to the
//! same command. These collections hold the same `Arc` for that reason: a
//! command is copied once when it enters the pool, and every index points to
//! that copy.
//!
//! They serialize exactly as the plain collections of commands they replace.
//! A deserializer gives each occurrence its own allocation; the pool shares
//! them again afterwards (`IndexedPool::share_commands`).

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
};

use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use super::ValidCommandWithHash;

/// A command in the pool. Every index that holds the command holds a clone of
/// this `Arc`, so the command itself is stored once.
pub type PooledCommand = Arc<ValidCommandWithHash>;

fn command(cmd: &PooledCommand) -> &ValidCommandWithHash {
    cmd
}

fn entry<'a>(
    (hash, cmd): (&'a v2::TransactionHash, &'a PooledCommand),
) -> (&'a v2::TransactionHash, &'a ValidCommandWithHash) {
    (hash, cmd)
}

/// Commands in order: a sender's queue, ordered by nonce, or a list of
/// commands the pool returns.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandQueue(VecDeque<PooledCommand>);

impl CommandQueue {
    pub fn with_capacity(capacity: usize) -> Self {
        Self(VecDeque::with_capacity(capacity))
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn front(&self) -> Option<&ValidCommandWithHash> {
        self.0.front().map(|cmd| &**cmd)
    }

    pub fn back(&self) -> Option<&ValidCommandWithHash> {
        self.0.back().map(|cmd| &**cmd)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &ValidCommandWithHash> + Clone {
        self.0.iter().map(|cmd| &**cmd)
    }

    pub fn contains(&self, cmd: &ValidCommandWithHash) -> bool {
        self.iter().any(|c| c == cmd)
    }

    pub(super) fn front_shared(&self) -> Option<&PooledCommand> {
        self.0.front()
    }

    pub(super) fn iter_shared(&self) -> impl DoubleEndedIterator<Item = &PooledCommand> {
        self.0.iter()
    }

    pub(super) fn iter_shared_mut(&mut self) -> impl Iterator<Item = &mut PooledCommand> {
        self.0.iter_mut()
    }

    pub(super) fn push_back(&mut self, cmd: PooledCommand) {
        self.0.push_back(cmd)
    }

    pub(super) fn push_front(&mut self, cmd: PooledCommand) {
        self.0.push_front(cmd)
    }

    pub(super) fn pop_back(&mut self) -> Option<PooledCommand> {
        self.0.pop_back()
    }

    pub(super) fn pop_front(&mut self) -> Option<PooledCommand> {
        self.0.pop_front()
    }

    pub(super) fn split_off(&mut self, at: usize) -> Self {
        Self(self.0.split_off(at))
    }
}

impl std::fmt::Debug for CommandQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq<VecDeque<ValidCommandWithHash>> for CommandQueue {
    fn eq(&self, other: &VecDeque<ValidCommandWithHash>) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl PartialEq<Vec<ValidCommandWithHash>> for CommandQueue {
    fn eq(&self, other: &Vec<ValidCommandWithHash>) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl IntoIterator for CommandQueue {
    type Item = PooledCommand;
    type IntoIter = std::collections::vec_deque::IntoIter<PooledCommand>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a CommandQueue {
    type Item = &'a ValidCommandWithHash;
    type IntoIter = std::iter::Map<
        std::collections::vec_deque::Iter<'a, PooledCommand>,
        fn(&'a PooledCommand) -> &'a ValidCommandWithHash,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(command as _)
    }
}

impl FromIterator<PooledCommand> for CommandQueue {
    fn from_iter<I: IntoIterator<Item = PooledCommand>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Extend<PooledCommand> for CommandQueue {
    fn extend<I: IntoIterator<Item = PooledCommand>>(&mut self, iter: I) {
        self.0.extend(iter)
    }
}

/// A set of commands, compared by hash.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct CommandSet(HashSet<PooledCommand>);

impl CommandSet {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn insert(&mut self, cmd: PooledCommand) -> bool {
        self.0.insert(cmd)
    }

    pub(super) fn remove(&mut self, cmd: &ValidCommandWithHash) -> bool {
        self.0.remove(cmd)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = &ValidCommandWithHash> {
        self.0.iter().map(|cmd| &**cmd)
    }

    pub(super) fn iter_shared(&self) -> impl Iterator<Item = &PooledCommand> {
        self.0.iter()
    }

    /// Replace each command with the result of `f`, which must return a command
    /// with the same hash.
    pub(super) fn map_shared(&mut self, mut f: impl FnMut(PooledCommand) -> PooledCommand) {
        self.0 = std::mem::take(&mut self.0)
            .into_iter()
            .map(&mut f)
            .collect();
    }
}

impl std::fmt::Debug for CommandSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<'a> IntoIterator for &'a CommandSet {
    type Item = &'a ValidCommandWithHash;
    type IntoIter = std::iter::Map<
        std::collections::hash_set::Iter<'a, PooledCommand>,
        fn(&'a PooledCommand) -> &'a ValidCommandWithHash,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(command as _)
    }
}

/// Commands by their hash. The pool's canonical copy of each command is the
/// one held here.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct CommandsByHash(HashMap<v2::TransactionHash, PooledCommand>);

impl CommandsByHash {
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }

    pub(super) fn contains_key(&self, hash: &v2::TransactionHash) -> bool {
        self.0.contains_key(hash)
    }

    pub(super) fn get(&self, hash: &v2::TransactionHash) -> Option<&ValidCommandWithHash> {
        self.0.get(hash).map(|cmd| &**cmd)
    }

    pub(super) fn get_shared(&self, hash: &v2::TransactionHash) -> Option<&PooledCommand> {
        self.0.get(hash)
    }

    pub(super) fn insert(&mut self, hash: v2::TransactionHash, cmd: PooledCommand) {
        self.0.insert(hash, cmd);
    }

    pub(super) fn remove(&mut self, hash: &v2::TransactionHash) -> Option<PooledCommand> {
        self.0.remove(hash)
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &ValidCommandWithHash> {
        self.0.values().map(|cmd| &**cmd)
    }

    /// The copy of `cmd` held here if there is one, otherwise `cmd`.
    pub(super) fn share(&self, cmd: &PooledCommand) -> PooledCommand {
        self.0.get(&cmd.hash).unwrap_or(cmd).clone()
    }
}

impl std::fmt::Debug for CommandsByHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self).finish()
    }
}

impl<'a> IntoIterator for &'a CommandsByHash {
    type Item = (&'a v2::TransactionHash, &'a ValidCommandWithHash);
    type IntoIter = std::iter::Map<
        std::collections::hash_map::Iter<'a, v2::TransactionHash, PooledCommand>,
        fn(
            (&'a v2::TransactionHash, &'a PooledCommand),
        ) -> (&'a v2::TransactionHash, &'a ValidCommandWithHash),
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(entry as _)
    }
}
