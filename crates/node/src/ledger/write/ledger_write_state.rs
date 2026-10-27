use serde::{Deserialize, Serialize};

use super::{LedgerWriteKind, LedgerWriteRequest};

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub enum LedgerWriteState {
    #[default]
    Idle,
    Init {
        time: redux::Timestamp,
        request: LedgerWriteRequest,
    },
    Pending {
        time: redux::Timestamp,
        request: LedgerWriteRequest,
    },
    Success {
        time: redux::Timestamp,
        /// Completion is recorded for diagnostics only. Follow-up actions
        /// receive the response directly; retaining it or the request here
        /// would keep blocks, scan states and checkpoint inputs alive.
        kind: LedgerWriteKind,
    },
}

impl LedgerWriteState {
    pub fn kind(&self) -> Option<LedgerWriteKind> {
        match self {
            Self::Idle => None,
            Self::Init { request, .. } | Self::Pending { request, .. } => Some(request.kind()),
            Self::Success { kind, .. } => Some(*kind),
        }
    }

    pub fn pending_requests(
        &self,
    ) -> impl Iterator<Item = (&LedgerWriteRequest, redux::Timestamp)> {
        std::iter::once(match self {
            Self::Pending { time, request } => Some((request, *time)),
            _ => None,
        })
        .flatten()
    }

    pub fn is_busy(&self) -> bool {
        self.pending_requests().peekable().peek().is_some()
    }
}
