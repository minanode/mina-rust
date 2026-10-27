//! The frontier store in the state machine (R20 D4 and D5): the store opens
//! once the chain id is known, each new best chain is handed to the store
//! thread, and a checkpoint of the root is taken when one is due. The store
//! itself is `crate::persist`; restoring from it on a restart is I5d1 and
//! I5d2.

mod transition_frontier_persist_state;
pub use transition_frontier_persist_state::*;

mod transition_frontier_persist_actions;
pub use transition_frontier_persist_actions::*;

mod transition_frontier_persist_reducer;
pub use transition_frontier_persist_reducer::chain_entry;

#[cfg(test)]
mod transition_frontier_persist_tests;
#[cfg(test)]
pub(crate) mod transition_frontier_restore_tests;
