//! `mina ledger export`: a whole ledger at the best tip, in OCaml's
//! runtime-config accounts JSON.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L774-L839>

use std::{collections::BTreeMap, str::FromStr, sync::Arc};

use mina_p2p_messages::v2::{
    LedgerHash, MinaBaseStagedLedgerHashStableV1, MinaStateProtocolStateValueStableV2, StateHash,
};
use serde::{Deserialize, Serialize};

/// The ledgers OCaml's `mina ledger export` takes, by the same names.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LedgerExportKind {
    /// The best tip's staged ledger.
    StagedLedger,
    /// The best tip's snarked ledger.
    SnarkedLedger,
    /// The ledger the best tip's epoch stakes from.
    StakingEpochLedger,
    /// The ledger the next epoch will stake from.
    NextEpochLedger,
}

impl LedgerExportKind {
    pub const ALL: [Self; 4] = [
        Self::StagedLedger,
        Self::SnarkedLedger,
        Self::StakingEpochLedger,
        Self::NextEpochLedger,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::StagedLedger => "staged-ledger",
            Self::SnarkedLedger => "snarked-ledger",
            Self::StakingEpochLedger => "staking-epoch-ledger",
            Self::NextEpochLedger => "next-epoch-ledger",
        }
    }
}

impl std::fmt::Display for LedgerExportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for LedgerExportKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown ledger '{s}', expected one of: staged-ledger, snarked-ledger, \
                     staking-epoch-ledger, next-epoch-ledger"
                )
            })
    }
}

/// What the ledger service needs to export one ledger.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LedgerExportRequest {
    pub kind: LedgerExportKind,
    pub ledger_hash: LedgerHash,
    pub best_tip: LedgerExportBestTip,
    /// The best chain, root first: each block's snarked ledger hash and
    /// staged ledger hashes. A snarked ledger the service doesn't hold is
    /// rebuilt from the nearest one it holds, as OCaml's
    /// `get_snarked_ledger_full` does from the root.
    pub chain: Vec<(LedgerHash, MinaBaseStagedLedgerHashStableV1)>,
    /// Protocol states the chain's scan states refer to.
    pub protocol_states: BTreeMap<StateHash, MinaStateProtocolStateValueStableV2>,
}

/// The block an export was taken at.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LedgerExportBestTip {
    pub state_hash: StateHash,
    pub blockchain_length: u32,
    pub global_slot_since_genesis: u32,
    pub epoch: u32,
}

/// An exported ledger. `accounts_json` is JSON- and hash-compatible with
/// OCaml `mina ledger export`: accounts in ledger index order, with compact
/// account lines instead of OCaml's pretty-printed whitespace.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcLedgerExport {
    pub kind: LedgerExportKind,
    /// The ledger's Merkle root, recomputed by the node from the exported
    /// accounts' ledger.
    pub ledger_hash: LedgerHash,
    pub best_tip: LedgerExportBestTip,
    pub num_accounts: u64,
    pub accounts_json: String,
}

pub type RpcLedgerExportResponse = Result<Arc<RpcLedgerExport>, String>;
