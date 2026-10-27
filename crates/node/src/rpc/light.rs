//! LN2: what a light client needs to check the chain against this node.
//!
//! - [`RpcRequest::LightBestTip`](super::RpcRequest::LightBestTip): the
//!   best chain, from which the HTTP layer serves the tip header (protocol
//!   state and proof), the root's protocol state and the body hashes that
//!   link the two, without block bodies.
//! - [`RpcRequest::LightAccount`](super::RpcRequest::LightAccount): one
//!   account with its index, leaf hash and Merkle path in a ledger named by a
//!   best-chain block, read on the ledger thread.
//!
//! Neither changes any state but the RPC request table. Values are labelled
//! by where their hash sits in the block, never as "proven": see
//! <https://minanode.org/docs/light-endpoints/> for client trust limits.

use std::str::FromStr;

use ledger::AccountId;
use mina_core::block::ArcBlockWithHash;
use mina_p2p_messages::{
    bigint::BigInt,
    v2::{LedgerHash, MinaBaseAccountBinableArgStableV2, StateHash},
};
use serde::{Deserialize, Serialize};

/// Which ledger of a block an account is read from.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LightLedgerKind {
    /// The block's staged ledger: balances after the block's transactions.
    Staged,
    /// The block's snarked ledger: the ledger after the last transactions
    /// SNARK workers finished proving.
    Snarked,
    /// The block's staking epoch ledger.
    Staking,
    /// The block's next epoch ledger.
    Next,
}

impl LightLedgerKind {
    pub const ALL: [Self; 4] = [Self::Staged, Self::Snarked, Self::Staking, Self::Next];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Snarked => "snarked",
            Self::Staking => "staking",
            Self::Next => "next",
        }
    }

    /// Where the ledger hash sits in the block's protocol state, so a client
    /// can compare it with the state it verified.
    pub fn protocol_state_field(self) -> &'static str {
        match self {
            Self::Staged => "body.blockchain_state.staged_ledger_hash.non_snark.ledger_hash",
            Self::Snarked => {
                "body.blockchain_state.ledger_proof_statement.target.first_pass_ledger"
            }
            Self::Staking => "body.consensus_state.staking_epoch_data.ledger.hash",
            Self::Next => "body.consensus_state.next_epoch_data.ledger.hash",
        }
    }

    /// The ledger hash this kind names in a block.
    pub fn ledger_hash(self, block: &ArcBlockWithHash) -> &LedgerHash {
        match self {
            Self::Staged => block.merkle_root_hash(),
            Self::Snarked => block.snarked_ledger_hash(),
            Self::Staking => block.staking_epoch_ledger_hash(),
            Self::Next => block.next_epoch_ledger_hash(),
        }
    }
}

impl std::fmt::Display for LightLedgerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LightLedgerKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| {
                format!("unknown ledger '{s}', expected one of: staged, snarked, staking, next")
            })
    }
}

/// Light account requests the node keeps at once, answered or not:
/// beyond it a new one is refused as busy, whatever the HTTP or GraphQL
/// side does. [`MAX_PENDING_LIGHT_ACCOUNTS`] bounds the ledger reads they
/// can queue ahead of block work.
pub const MAX_PENDING_LIGHT_REQUESTS: usize = 16;

/// Accounts one light request reads, in one ledger call. OCaml's
/// `encodedSnarkedLedgerAccountMembership` has no limit.
pub const MAX_LIGHT_ACCOUNTS_PER_REQUEST: usize = 32;

/// Accounts all pending light requests may read together. The ledger thread
/// takes reads and block application from one queue in order, so this is
/// the most light work a block can find ahead of it: about 32 path reads,
/// some 40 ms on devnet today. A request beyond it is refused as busy.
pub const MAX_PENDING_LIGHT_ACCOUNTS: usize = 32;

/// How long a light request may wait for the ledger read quota before the
/// node answers it with [`RpcLightError::Timeout`] and drops it. One the
/// ledger thread has taken is answered when the thread gets to it.
pub const LIGHT_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Accounts in one ledger of one best-chain block, read from one snapshot.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RpcLightAccountQuery {
    pub ledger: LightLedgerKind,
    /// 1 to [`MAX_LIGHT_ACCOUNTS_PER_REQUEST`] accounts.
    pub account_ids: Vec<AccountId>,
    /// A best-chain block by state hash; the best tip if `None`.
    pub block: Option<StateHash>,
}

/// The block whose ledger an answer was read from.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LightBlockRef {
    pub state_hash: StateHash,
    pub height: u32,
    pub global_slot_since_genesis: u32,
}

impl LightBlockRef {
    pub fn of(block: &ArcBlockWithHash) -> Self {
        Self {
            state_hash: block.hash().clone(),
            height: block.height(),
            global_slot_since_genesis: block.global_slot_since_genesis(),
        }
    }
}

/// What the ledger thread needs for a light account read: the query
/// resolved against the best chain when it was admitted.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LightAccountRead {
    pub ledger: LightLedgerKind,
    pub ledger_hash: LedgerHash,
    pub block: LightBlockRef,
    pub account_ids: Vec<AccountId>,
}

/// One element of a Merkle path, leaf first, in OCaml's GraphQL
/// `MerklePathElement` shape: `Left(h)` means the path node is a left child
/// and `h`, its sibling, is on the right.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L208-L223>
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum LightMerklePathElement {
    Left(BigInt),
    Right(BigInt),
}

impl From<&ledger::MerklePath> for LightMerklePathElement {
    fn from(value: &ledger::MerklePath) -> Self {
        match value {
            ledger::MerklePath::Left(hash) => Self::Left(hash.into()),
            ledger::MerklePath::Right(hash) => Self::Right(hash.into()),
        }
    }
}

/// An account and the path that ties it to `ledger_hash`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RpcLightAccount {
    pub ledger: LightLedgerKind,
    pub ledger_hash: LedgerHash,
    pub block: LightBlockRef,
    pub index: u64,
    /// The account's hash, the path's leaf.
    pub leaf_hash: BigInt,
    pub account: MinaBaseAccountBinableArgStableV2,
    /// Leaf first: [`crate::ledger::LEDGER_DEPTH`] elements.
    pub merkle_path: Vec<LightMerklePathElement>,
}

/// Why a light request has no answer.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpcLightError {
    #[error("the node is not synced yet")]
    NotSynced,
    #[error("block {0} is not in the best chain")]
    BlockNotInBestChain(StateHash),
    #[error("the node does not hold the {kind} ledger {hash}")]
    LedgerUnavailable {
        kind: LightLedgerKind,
        hash: LedgerHash,
    },
    #[error("no such account in the {0} ledger")]
    AccountNotFound(LightLedgerKind),
    #[error("ledger read failed: {0}")]
    Internal(String),
    #[error("too many light requests in flight, try again shortly")]
    Busy,
    #[error("the ledger read did not start in time")]
    Timeout,
    #[error("give 1 to {0} accounts")]
    TooManyAccounts(usize),
}

/// One account of a light request: boxed, as an account is too large to
/// travel inline in an action.
pub type RpcLightAccountEntry = Result<Box<RpcLightAccount>, RpcLightError>;

/// One entry per requested account, in order, all from the same ledger
/// snapshot; or why there is none.
pub type RpcLightAccountResponse = Result<Vec<RpcLightAccountEntry>, RpcLightError>;

/// The best chain, root first, while the node is synced.
pub type RpcLightBestTipResponse = Result<Vec<ArcBlockWithHash>, RpcLightError>;
