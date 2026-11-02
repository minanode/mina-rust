//! What a best-tip uptime submission carries: the block alone, the block
//! with SNARK work, or nothing, as OCaml's `send_block_and_transaction_snark`
//! decides. Produced-block submissions always carry the block alone.
//!
//! Proofs are made by a dedicated native worker when uptime is configured.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L203-L399>

use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures::future::BoxFuture;
use mina_core::{block::ArcBlockWithHash, snark::SnarkJobId};
use mina_node::{account::AccountPublicKey, rpc::RpcReportingUptimeJob};
use mina_p2p_messages::{
    binprot::{BinProtRead, BinProtWrite},
    v2,
};

/// What OCaml sends for the best tip.
#[derive(Clone, Debug, PartialEq)]
pub enum BestTipWork {
    /// The block without SNARK work, for this reason.
    BlockOnly(&'static str),
    /// Nothing: OCaml logs an error and sends nothing this iteration.
    Skip(String),
    /// The block with SNARK work for this job.
    Prove(UptimeJob),
}

/// The scan-state job whose proof goes with the best tip.
#[derive(Clone, Debug, PartialEq)]
pub struct UptimeJob {
    /// The job's pair in the SNARK pool.
    pub pair: SnarkJobId,
    /// Its place in the pair.
    pub index: usize,
    /// A zkApp command, proved as segments.
    pub zkapp: bool,
}

pub const NO_TRANSACTIONS: &str = "no transactions in the block";
pub const NO_JOBS: &str = "no SNARK jobs available";
pub const NO_MATCHING_TRANSITION: &str =
    "no transaction in the block matches its staged ledger hash";

/// OCaml's choice, in its order: a block without transactions goes alone;
/// then the job list, whose failure skips the submission; then no jobs, or
/// no Transition job ending at the block's ledger, send the block alone.
/// `job` is the node's [`RpcReportingUptimeJob`] for `best_tip`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L224-L328>
pub fn best_tip_work(
    best_tip: &ArcBlockWithHash,
    job: Option<&RpcReportingUptimeJob>,
) -> BestTipWork {
    match has_transactions(best_tip) {
        Err(error) => return BestTipWork::Skip(format!("the block's transactions: {error}")),
        Ok(false) => return BestTipWork::BlockOnly(NO_TRANSACTIONS),
        Ok(true) => {}
    }
    match job {
        None => BestTipWork::Skip("the node did not list the best tip's SNARK jobs".to_owned()),
        Some(RpcReportingUptimeJob::Unavailable(reason)) => {
            BestTipWork::Skip(format!("could not get the SNARK work: {reason}"))
        }
        Some(RpcReportingUptimeJob::NoJobs) => BestTipWork::BlockOnly(NO_JOBS),
        Some(RpcReportingUptimeJob::NoMatchingTransition) => {
            BestTipWork::BlockOnly(NO_MATCHING_TRANSITION)
        }
        Some(RpcReportingUptimeJob::Transition { pair, index, zkapp }) => {
            BestTipWork::Prove(UptimeJob {
                pair: pair.clone(),
                index: *index,
                zkapp: *zkapp,
            })
        }
    }
}

/// Whether `Mina_block.transactions` is not empty: the block's user
/// commands, fee transfers and coinbase, as the staged ledger applies them.
/// OCaml raises when they can't be computed; here the submission is
/// skipped.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/mina_block/block.ml#L5-L19>
pub fn has_transactions(block: &ArcBlockWithHash) -> Result<bool, String> {
    let consensus_state = block.consensus_state();
    let coinbase_receiver: mina_signer::CompressedPubKey = (&consensus_state.coinbase_receiver)
        .try_into()
        .map_err(|error| format!("bad coinbase receiver: {error:?}"))?;
    let diff = ledger::staged_ledger::diff::Diff::try_from(&block.body().staged_ledger_diff)
        .map_err(|error| format!("bad staged ledger diff: {error:?}"))?;
    let transactions = diff
        .get_transactions(
            mina_core::constants::constraint_constants(),
            coinbase_receiver,
            consensus_state.supercharge_coinbase,
        )
        .map_err(|error| format!("{error:?}"))?;
    Ok(!transactions.is_empty())
}

/// What the prover is asked for: a proof of `job` from the best tip's scan
/// state, bound to the submitter's key as the prover.
#[derive(Clone, Debug)]
pub struct UptimeProofRequest {
    pub best_tip: ArcBlockWithHash,
    pub job: UptimeJob,
    pub submitter: AccountPublicKey,
    pub snark_work_fee: v2::CurrencyFeeStableV1,
    pub spec: Option<Box<v2::SnarkWorkerWorkerRpcsVersionedGetWorkV2TResponseA0Single>>,
}

#[derive(Clone, Debug)]
pub enum ProofOutcome {
    /// Attached to the submission.
    Proved(Box<ProofData>),
    /// OCaml logs an error and sends nothing this iteration.
    Failed(String),
    /// Circuit loading already logged this failure state; count the skip only.
    CircuitsUnavailable(String),
}

/// Where the SNARK work of uptime submissions comes from. OCaml runs a
/// dedicated full-proof worker: `perform_single` with
/// `Sok_message(fee, submitter)` for a transaction, `perform_partitioned`
/// for a zkApp command. The next iteration drops the future it is given.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L329-L399>
pub trait UptimeProver: Send + Sync + 'static {
    fn prove(&self, request: UptimeProofRequest) -> BoxFuture<'_, ProofOutcome>;
}

/// Tests that should never ask for a proof.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub struct NoUptimeProver;

#[cfg(test)]
impl UptimeProver for NoUptimeProver {
    fn prove(&self, _request: UptimeProofRequest) -> BoxFuture<'_, ProofOutcome> {
        Box::pin(async { ProofOutcome::Failed("no test prover".to_owned()) })
    }
}

/// OCaml's `Proof_data`, the `snark_work` field once Base64 encoded. It is
/// `bin_io_unversioned`: the three fields in order, with no version tag.
/// `Time.Span.t` is a float of seconds.
///
/// The three OCaml 4.0.3 captures in `tools/uptime-fixtures/full-proof/`
/// decode and re-encode exactly.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/uptime_service/uptime_service.ml#L13-L23>
#[derive(Clone, Debug, PartialEq)]
pub struct ProofData {
    pub proof: v2::LedgerProofProdStableV2,
    /// How long the proof took, in seconds.
    pub proof_time: f64,
    pub snark_work_fee: v2::CurrencyFeeStableV1,
}

impl BinProtWrite for ProofData {
    fn binprot_write<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        self.proof.binprot_write(w)?;
        self.proof_time.binprot_write(w)?;
        self.snark_work_fee.binprot_write(w)
    }
}

impl BinProtRead for ProofData {
    fn binprot_read<R: std::io::Read + ?Sized>(
        r: &mut R,
    ) -> Result<Self, mina_p2p_messages::binprot::Error>
    where
        Self: Sized,
    {
        Ok(Self {
            proof: BinProtRead::binprot_read(r)?,
            proof_time: BinProtRead::binprot_read(r)?,
            snark_work_fee: BinProtRead::binprot_read(r)?,
        })
    }
}

impl ProofData {
    /// The `snark_work` field: standard padded Base64 of the Bin_prot bytes.
    pub fn to_base64(&self) -> std::io::Result<String> {
        let mut bytes = Vec::new();
        self.binprot_write(&mut bytes)?;
        Ok(STANDARD.encode(bytes))
    }
}
