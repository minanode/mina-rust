mod ledger_read_actions;
use ledger::{Account, AccountId};
pub use ledger_read_actions::*;

mod ledger_read_state;
use crate::p2p::{channels::rpc::P2pRpcId, PeerId};
pub use ledger_read_state::*;
use mina_core::{
    block::AppliedBlock,
    requests::{RequestId, RpcId, RpcIdType},
};
use redux::Callback;

mod ledger_read_reducer;

use std::{collections::BTreeMap, sync::Arc};

use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use crate::{
    account::AccountPublicKey,
    block_producer::vrf_evaluator::DelegatorTable,
    ledger::LedgerAddress,
    p2p::channels::rpc::StagedLedgerAuxAndPendingCoinbases,
    rpc::{
        AccountQuery, DelegatorsQuery, HardForkExportRequest, LedgerExportRequest,
        LightAccountRead, RpcHardForkExportResponse, RpcLedgerExportResponse,
        RpcLightAccountResponse, RpcScanStateSummaryScanStateJob,
    },
};

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum LedgerReadKind {
    SnarkPoolProver,
    WorkCoverage,
    DelegatorTable,
    GetNumAccounts,
    GetAccounts,
    GetChildHashesAtAddr,
    GetChildAccountsAtAddr,
    GetStagedLedgerAuxAndPendingCoinbases,
    ScanStateSummary,
    AccountsForRpc,
    GetLedgerStatus,
    GetAccountDelegators,
    LedgerExport,
    HardForkExport,
    LightAccount,
}

/// Most levels of hashes served for one `WhatChildHashes` query, as OCaml
/// mainnet nodes serve.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L381-L389>
pub use crate::p2p::channels::rpc::{
    MAX_SERVED_ACCOUNT_SUBTREE_HEIGHT, MAX_SERVED_CHILD_HASHES_DEPTH,
};

/// Clamp to OCaml's configured maximum, then reject addresses past the leaves.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/syncable_ledger/syncable_ledger.ml#L381-L389>
pub fn served_child_hashes_depth(addr: &LedgerAddress, depth: u64) -> Option<usize> {
    let depth = usize::try_from(depth)
        .unwrap_or(usize::MAX)
        .min(MAX_SERVED_CHILD_HASHES_DEPTH);
    (depth > 0 && addr.length().checked_add(depth)? <= super::LEDGER_DEPTH).then_some(depth)
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub enum LedgerReadRequest {
    SnarkPoolProver(Box<crate::snark_pool::ProverAccountRequest>),
    WorkCoverage(Box<crate::snark_pool::WorkCoverageRequest>),
    /// Delegator table requested by vrf state machine.
    DelegatorTable(v2::LedgerHash, AccountPublicKey),
    // p2p rpcs
    GetNumAccounts(v2::LedgerHash),
    GetAccounts(v2::LedgerHash, Vec<AccountId>, Option<RpcId>),
    /// The hashes this many levels below the address.
    GetChildHashesAtAddr(v2::LedgerHash, LedgerAddress, usize),
    GetChildAccountsAtAddr(v2::LedgerHash, LedgerAddress),
    GetStagedLedgerAuxAndPendingCoinbases(LedgerReadStagedLedgerAuxAndPendingCoinbases),
    // rpcs
    ScanStateSummary(v2::MinaBaseStagedLedgerHashStableV1),
    AccountsForRpc(RpcId, v2::LedgerHash, AccountQuery),
    GetLedgerStatus(RpcId, v2::LedgerHash),
    GetAccountDelegators(RpcId, DelegatorsQuery, AccountId),
    LedgerExport(RpcId, Box<LedgerExportRequest>),
    HardForkExport(RpcId, Box<HardForkExportRequest>),
    /// LN2: one account with its Merkle path.
    LightAccount(RpcId, Box<LightAccountRead>),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct LedgerStatus {
    pub num_accounts: u64,
    pub best_tip_staged_ledger_hash: v2::LedgerHash,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LedgerReadResponse {
    SnarkPoolProver(Result<crate::snark_pool::ProverAccount, String>),
    WorkCoverage(Result<crate::snark_pool::WorkCoverage, String>),
    /// Delegator table requested by vrf state machine.
    DelegatorTable(Option<DelegatorTable>),
    // p2p rpcs
    GetNumAccounts(Option<(u64, v2::LedgerHash)>),
    GetAccounts(Vec<Account>, Option<RpcId>),
    GetChildHashesAtAddr(Option<Vec<v2::LedgerHash>>),
    GetChildAccountsAtAddr(Result<Vec<v2::MinaBaseAccountBinableArgStableV2>, String>),
    GetStagedLedgerAuxAndPendingCoinbases(Option<Arc<StagedLedgerAuxAndPendingCoinbases>>),
    // rpcs
    ScanStateSummary(Result<Vec<Vec<RpcScanStateSummaryScanStateJob>>, String>),
    AccountsForRpc(RpcId, Vec<Account>, AccountQuery),
    GetLedgerStatus(RpcId, Option<LedgerStatus>),
    GetAccountDelegators(RpcId, Option<Vec<Account>>),
    LedgerExport(RpcId, RpcLedgerExportResponse),
    HardForkExport(RpcId, RpcHardForkExportResponse),
    LightAccount(RpcId, RpcLightAccountResponse),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LedgerReadStagedLedgerAuxAndPendingCoinbases {
    pub block_hash: v2::StateHash,
    pub ledger_hash: v2::MinaBaseStagedLedgerHashStableV1,
    pub protocol_states: BTreeMap<v2::StateHash, v2::MinaStateProtocolStateValueStableV2>,
}

impl LedgerReadRequest {
    pub fn kind(&self) -> LedgerReadKind {
        match self {
            Self::SnarkPoolProver(..) => LedgerReadKind::SnarkPoolProver,
            Self::WorkCoverage(..) => LedgerReadKind::WorkCoverage,
            Self::DelegatorTable(..) => LedgerReadKind::DelegatorTable,
            Self::GetNumAccounts(..) => LedgerReadKind::GetNumAccounts,
            Self::GetAccounts(..) => LedgerReadKind::GetAccounts,
            Self::GetChildAccountsAtAddr(..) => LedgerReadKind::GetChildAccountsAtAddr,
            Self::GetChildHashesAtAddr(..) => LedgerReadKind::GetChildHashesAtAddr,
            Self::GetStagedLedgerAuxAndPendingCoinbases(..) => {
                LedgerReadKind::GetStagedLedgerAuxAndPendingCoinbases
            }
            Self::ScanStateSummary(..) => LedgerReadKind::ScanStateSummary,
            Self::AccountsForRpc(..) => LedgerReadKind::AccountsForRpc,
            Self::GetLedgerStatus(..) => LedgerReadKind::GetLedgerStatus,
            Self::GetAccountDelegators(..) => LedgerReadKind::GetAccountDelegators,
            Self::LedgerExport(..) => LedgerReadKind::LedgerExport,
            Self::HardForkExport(..) => LedgerReadKind::HardForkExport,
            Self::LightAccount(..) => LedgerReadKind::LightAccount,
        }
    }

    pub fn cost(&self) -> usize {
        let cost = match self {
            Self::SnarkPoolProver(..) => 1,
            Self::WorkCoverage(..) => 100,
            Self::DelegatorTable(..) => 100,
            Self::GetNumAccounts(..) => 1,
            Self::GetAccounts(..) => 10, // Not sure if 10 is a good number here
            Self::GetChildAccountsAtAddr(_, addr) => super::LEDGER_DEPTH
                .checked_sub(addr.length())
                .filter(|height| *height <= MAX_SERVED_ACCOUNT_SUBTREE_HEIGHT)
                .and_then(|height| 1usize.checked_shl(height as u32))
                .map(|accounts| accounts / 4)
                .unwrap_or(usize::MAX),
            Self::GetChildHashesAtAddr(_, addr, depth) => (1..=MAX_SERVED_CHILD_HASHES_DEPTH)
                .contains(depth)
                .then(|| addr.length().checked_add(*depth))
                .flatten()
                .filter(|length| *length <= super::LEDGER_DEPTH)
                .and_then(|_| 1usize.checked_shl(*depth as u32))
                .map(|hashes| hashes.div_ceil(32))
                .unwrap_or(usize::MAX),
            Self::GetStagedLedgerAuxAndPendingCoinbases(..) => 100,
            Self::ScanStateSummary(..) => 100,
            // TODO(adonagy): not sure
            // Full-ledger work, not an indexed account lookup. TokenOwner's
            // first read per ledger hash also derives a token per account.
            // Neither scan is admitted by the public GraphQL policy.
            Self::AccountsForRpc(
                _,
                _,
                AccountQuery::TokenOwner(..)
                | AccountQuery::All
                | AccountQuery::SinglePublicKey(..),
            ) => 100,
            Self::AccountsForRpc(..) => 10,
            Self::GetLedgerStatus(..) => 1,
            // The ledger service's tables, no scan.
            Self::GetAccountDelegators(_, DelegatorsQuery::LastEpoch { .. }, _) => 1,
            Self::GetAccountDelegators(..) => 10,
            // A whole ledger, possibly rebuilt: the heaviest read there is.
            Self::LedgerExport(..) => 100,
            // Three whole ledgers.
            Self::HardForkExport(..) => 300,
            // One indexed lookup and one path (35 hashes, mostly cached) per
            // account, at most MAX_LIGHT_ACCOUNTS_PER_REQUEST.
            Self::LightAccount(_, read) => read.account_ids.len(),
        };
        cost.max(1)
    }
}

impl LedgerReadResponse {
    pub fn kind(&self) -> LedgerReadKind {
        match self {
            Self::SnarkPoolProver(..) => LedgerReadKind::SnarkPoolProver,
            Self::WorkCoverage(..) => LedgerReadKind::WorkCoverage,
            Self::DelegatorTable(..) => LedgerReadKind::DelegatorTable,
            Self::GetNumAccounts(..) => LedgerReadKind::GetNumAccounts,
            Self::GetAccounts(..) => LedgerReadKind::GetAccounts,
            Self::GetChildAccountsAtAddr(..) => LedgerReadKind::GetChildAccountsAtAddr,
            Self::GetChildHashesAtAddr(..) => LedgerReadKind::GetChildHashesAtAddr,
            Self::GetStagedLedgerAuxAndPendingCoinbases(..) => {
                LedgerReadKind::GetStagedLedgerAuxAndPendingCoinbases
            }
            Self::ScanStateSummary(..) => LedgerReadKind::ScanStateSummary,
            Self::AccountsForRpc(..) => LedgerReadKind::AccountsForRpc,
            Self::GetLedgerStatus(..) => LedgerReadKind::GetLedgerStatus,
            Self::GetAccountDelegators(..) => LedgerReadKind::GetAccountDelegators,
            Self::LedgerExport(..) => LedgerReadKind::LedgerExport,
            Self::HardForkExport(..) => LedgerReadKind::HardForkExport,
            Self::LightAccount(..) => LedgerReadKind::LightAccount,
        }
    }
}

impl PartialEq for LedgerReadStagedLedgerAuxAndPendingCoinbases {
    fn eq(&self, other: &Self) -> bool {
        self.block_hash == other.block_hash && self.ledger_hash == other.ledger_hash
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum LedgerReadInitCallback {
    RpcLedgerAccountsGetPending {
        callback: Callback<RequestId<RpcIdType>>,
        args: RequestId<RpcIdType>,
    },
    RpcScanStateSummaryGetPending {
        callback: Callback<(RequestId<RpcIdType>, AppliedBlock)>,
        args: (RequestId<RpcIdType>, AppliedBlock),
    },
    P2pChannelsResponsePending {
        callback: Callback<(bool, P2pRpcId, PeerId)>,
        args: (bool, P2pRpcId, PeerId),
    },
    RpcLedgerStatusGetPending {
        callback: Callback<RequestId<RpcIdType>>,
        args: RequestId<RpcIdType>,
    },
    RpcLedgerAccountDelegatorsGetPending {
        callback: Callback<RequestId<RpcIdType>>,
        args: RequestId<RpcIdType>,
    },
    RpcLedgerExportPending {
        callback: Callback<RequestId<RpcIdType>>,
        args: RequestId<RpcIdType>,
    },
    RpcHardForkConfigPending {
        callback: Callback<RequestId<RpcIdType>>,
        args: RequestId<RpcIdType>,
    },
    RpcLightAccountPending {
        callback: Callback<RequestId<RpcIdType>>,
        args: RequestId<RpcIdType>,
    },
    None,
}
