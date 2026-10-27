use ledger::scan_state::transaction_logic::valid;
use serde::{Deserialize, Serialize};

use super::{
    block_verify::{SnarkBlockVerifyError, SnarkBlockVerifyId},
    work_verify::{SnarkWorkVerifyError, SnarkWorkVerifyId},
};
use crate::user_command_verify::SnarkUserCommandVerifyId;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SnarkEvent {
    BlockVerify(SnarkBlockVerifyId, Result<(), SnarkBlockVerifyError>),
    BlockVerifyMeasured {
        req_id: SnarkBlockVerifyId,
        result: Result<(), SnarkBlockVerifyError>,
        elapsed_ms: f64,
        skipped: bool,
        verifier_key: String,
    },
    WorkVerify(SnarkWorkVerifyId, Result<(), SnarkWorkVerifyError>),
    UserCommandVerify(
        SnarkUserCommandVerifyId,
        Result<Vec<valid::UserCommand>, String>,
    ),
}

fn res_kind<T, E>(res: &Result<T, E>) -> &'static str {
    match res {
        Err(_) => "Err",
        Ok(_) => "Ok",
    }
}

impl std::fmt::Display for SnarkEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Snark, ")?;
        match self {
            Self::BlockVerify(id, res) => {
                write!(f, "BlockVerify, {id}, {}", res_kind(res))
            }
            Self::BlockVerifyMeasured { req_id, result, .. } => {
                write!(f, "BlockVerifyMeasured, {req_id}, {}", res_kind(result))
            }
            Self::WorkVerify(id, res) => {
                write!(f, "WorkVerify, {id}, {}", res_kind(res))
            }
            Self::UserCommandVerify(id, res) => {
                //let n_failed = res.iter().filter(|res| res.is_err()).count();
                //let n_success = res.len() - n_failed;
                write!(f, "UserCommandVerify, {id}, success={}", res.is_err())
            }
        }
    }
}
