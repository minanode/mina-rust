use mina_core::ActionEvent;
use mina_p2p_messages::v2::{CurrencyFeeStableV1, NonZeroCurvePoint};
use redux::EnablingCondition;
use serde::{Deserialize, Serialize};

use crate::{external_snark_worker::SnarkWorkSpec, SnarkerSetting, State};

#[derive(Debug, Clone, Serialize, Deserialize, ActionEvent)]
pub enum ExternalSnarkWorkerEffectfulAction {
    Start {
        public_key: NonZeroCurvePoint,
        fee: CurrencyFeeStableV1,
    },
    Kill,
    SubmitWork {
        spec: Box<SnarkWorkSpec>,
    },
    CancelWork,
    /// Keeps the operator's setting in the work directory, for restarts
    /// (P5.4); `None` forgets it.
    SaveSetting {
        setting: Option<SnarkerSetting>,
    },
}

impl EnablingCondition<State> for ExternalSnarkWorkerEffectfulAction {
    fn is_enabled(&self, _state: &State, _time: redux::Timestamp) -> bool {
        true
    }
}
