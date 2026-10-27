mod transition_frontier_sync_ledger_snarked_state;
pub use transition_frontier_sync_ledger_snarked_state::*;

mod transition_frontier_sync_ledger_snarked_actions;
pub use transition_frontier_sync_ledger_snarked_actions::*;

mod transition_frontier_sync_ledger_snarked_reducer;

mod transition_frontier_sync_ledger_snarked_effects;

mod transition_frontier_sync_ledger_snarked_service;
pub use transition_frontier_sync_ledger_snarked_service::*;

use mina_p2p_messages::v2::{LedgerHash, MinaBaseAccountBinableArgStableV2};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PeerLedgerQueryResponse {
    /// The hashes some levels below the queried address, left to right.
    ChildHashes(Vec<LedgerHash>),
    ChildAccounts(Vec<MinaBaseAccountBinableArgStableV2>),
    NumAccounts(u64, LedgerHash),
}

impl PeerLedgerQueryResponse {
    pub fn is_child_hashes(&self) -> bool {
        matches!(self, Self::ChildHashes(..))
    }

    pub fn is_child_accounts(&self) -> bool {
        matches!(self, Self::ChildAccounts(..))
    }

    pub fn is_num_accounts(&self) -> bool {
        matches!(self, Self::NumAccounts(..))
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum PeerLedgerQueryError {
    Timeout,
    Disconnected,
    DataUnavailable,
    /// The peer explicitly answered that it cannot serve this ledger query.
    LedgerUnavailable,
    /// A received answer failed validation or answered a different query.
    InvalidResponse,
}

/// Bound repeated refusals across all addresses of a ledger, including when
/// the last connected peer is retried after a full exclusion pass.
pub const LEDGER_QUERY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// A sync RPC attempt that has had no outcome for this long counts as
/// failed, so that the item is asked again. Every attempt should end in an
/// error (timeout, disconnection, invalid answer) or an accepted or rejected
/// answer within a few seconds (the ledger query timeout is 4 s). An attempt
/// that doesn't is lost: a request never sent (`Init`), an answer the
/// transport consumed but no callback marked (`Pending`), or an answer whose
/// follow-up action was disabled (`Success`). Without this, an address left
/// that way is never retried and the ledger never completes: the sync waits
/// silently, as a Raspberry Pi 3 bootstrap did for 45 minutes.
pub const LEDGER_QUERY_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
