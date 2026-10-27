use std::collections::BTreeMap;

use mina_core::block::AppliedBlock;
use mina_p2p_messages::v2;
use serde::{Deserialize, Serialize};

use super::{AccountQuery, RpcId, RpcRequest};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RpcRequestState {
    pub req: RpcRequest,
    pub status: RpcRequestStatus,
    /// Extra data for the request.
    pub data: RpcRequestExtraData,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RpcRequestStatus {
    Init {
        time: redux::Timestamp,
    },
    Pending {
        time: redux::Timestamp,
    },
    Error {
        time: redux::Timestamp,
        error: String,
    },
    Success {
        time: redux::Timestamp,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum RpcRequestExtraData {
    #[default]
    None,
    FullBlockOpt(Option<AppliedBlock>),
}

impl RpcRequestStatus {
    pub fn is_init(&self) -> bool {
        matches!(self, Self::Init { .. })
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending { .. })
    }

    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Error { .. } | Self::Success { .. })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct RpcState {
    pub requests: BTreeMap<RpcId, RpcRequestState>,
    /// LN2: light account requests not yet answered, and when each was
    /// admitted. At most [`super::MAX_PENDING_LIGHT_REQUESTS`].
    #[serde(default)]
    pub light_pending: BTreeMap<RpcId, redux::Timestamp>,
}

impl RpcState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accounts the pending light requests read together.
    pub fn light_pending_accounts(&self) -> usize {
        self.light_pending
            .keys()
            .filter_map(|id| self.requests.get(id))
            .map(|request| match &request.req {
                RpcRequest::LightAccount(query) => query.account_ids.len(),
                _ => 0,
            })
            .sum()
    }

    /// Light requests still waiting for the ledger read quota (in Init)
    /// since before `now - LIGHT_REQUEST_TIMEOUT`. One the ledger thread
    /// has taken (Pending) is left for its answer.
    pub fn light_expired(&self, now: redux::Timestamp) -> impl Iterator<Item = RpcId> + '_ {
        self.light_pending
            .iter()
            .filter(move |(_, admitted)| {
                now.checked_sub(**admitted)
                    .is_some_and(|waited| waited >= super::LIGHT_REQUEST_TIMEOUT)
            })
            .filter(|(id, _)| self.requests.get(id).is_some_and(|r| r.status.is_init()))
            .map(|(id, _)| *id)
    }

    pub fn scan_state_summary_rpc_ids(
        &self,
    ) -> impl Iterator<
        Item = (
            RpcId,
            &v2::MinaBaseStagedLedgerHashStableV1,
            &RpcRequestStatus,
        ),
    > {
        self.requests
            .iter()
            .filter(|(_, req)| matches!(req.req, RpcRequest::ScanStateSummaryGet(_)))
            .filter_map(|(id, req)| {
                let block = match &req.data {
                    RpcRequestExtraData::FullBlockOpt(block) => block.as_ref()?,
                    _ => return None,
                };
                Some((*id, block.staged_ledger_hashes(), &req.status))
            })
    }

    pub fn accounts_request_rpc_ids(
        &self,
    ) -> impl Iterator<Item = (RpcId, AccountQuery, &RpcRequestStatus)> + '_ {
        self.requests.iter().filter_map(|(id, req)| {
            if let RpcRequest::LedgerAccountsGet(account) = &req.req {
                Some((*id, account.clone(), &req.status))
            } else {
                None
            }
        })
    }
}
