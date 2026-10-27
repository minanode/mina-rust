//! Blocks, as OCaml's `Block` type shows them: the protocol state, the
//! transactions (user commands, zkApp commands, fee transfers and the
//! coinbase) and the completed snark work.

use std::sync::Arc;

use crate::graphql::{account::GraphQLAccount, zkapp::GraphQLZkapp};
use juniper::{graphql_object, FieldResult, GraphQLObject};
use ledger::scan_state::{
    currency::{Amount, Magnitude},
    transaction_logic::{Transaction, TransactionFailure},
};
use mina_core::{
    block::{AppliedBlock, ArcBlockWithHash},
    constants::constraint_constants,
};
use mina_p2p_messages::{
    b58::{Base58CheckOfBinProt, Base58CheckOfBytes},
    string::ByteString,
    v2::{
        ConsensusProofOfStakeDataConsensusStateValueStableV2, MinaBaseProofStableV2,
        MinaBaseSignedCommandPayloadBodyStableV2, MinaBaseSignedCommandStableV2,
        MinaBaseStakeDelegationStableV2, MinaBaseTransactionStatusStableV2,
        MinaBaseUserCommandStableV2, MinaStateBlockchainStateValueStableV2, NonZeroCurvePoint,
        PrecomputedBlockProof, StagedLedgerDiffDiffStableV2, TokenIdKeyHash,
        TransactionSnarkWorkTStableV2, TransactionSnarkWorkTStableV2Proofs,
    },
};
use mina_signer::CompressedPubKey;

use super::{scalars::*, Context, ConversionError};

/// The version byte of a truncated VRF output in Base58Check.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/base58_check/version_bytes.ml#L49>
const VRF_TRUNCATED_OUTPUT_VERSION: u8 = 0x15;

pub(crate) fn compressed(
    public_key: &NonZeroCurvePoint,
) -> Result<CompressedPubKey, ConversionError> {
    CompressedPubKey::try_from(public_key).map_err(|_| ConversionError::InvalidBigInt)
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2135-L2191>
#[derive(Debug)]
pub(crate) struct GraphQLBlock {
    proof: Arc<MinaBaseProofStableV2>,
    creator: CompressedPubKey,
    winner: CompressedPubKey,
    state_hash: String,
    state_hash_field: String,
    protocol_state: GraphQLProtocolState,
    transactions: GraphQLTransactions,
    command_transaction_count: i32,
    snark_jobs: Vec<GraphQLSnarkJob>,
}

#[graphql_object(context = Context, name = "Block")]
#[graphql(description = "A Mina block")]
impl GraphQLBlock {
    /// Public key of account that produced this block
    #[graphql(deprecated = "use creatorAccount field instead")]
    fn creator(&self) -> String {
        self.creator.into_address()
    }

    /// Account that produced this block
    async fn creator_account(&self, context: &Context) -> FieldResult<GraphQLAccount> {
        Ok(GraphQLAccount::load_or_empty(context, self.creator.clone()).await?)
    }

    /// Account that won the slot (Delegator/Staker)
    async fn winner_account(&self, context: &Context) -> FieldResult<GraphQLAccount> {
        Ok(GraphQLAccount::load_or_empty(context, self.winner.clone()).await?)
    }

    /// Base58Check-encoded hash of the state after this block
    fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// Experimental: Bigint field-element representation of stateHash
    fn state_hash_field(&self) -> &str {
        &self.state_hash_field
    }

    fn protocol_state(&self) -> &GraphQLProtocolState {
        &self.protocol_state
    }

    /// Snark proof of blockchain state
    fn protocol_state_proof(&self) -> GraphQLProtocolStateProof {
        GraphQLProtocolStateProof(self.proof.clone())
    }

    fn transactions(&self) -> &GraphQLTransactions {
        &self.transactions
    }

    /// Count of user command transactions in the block
    fn command_transaction_count(&self) -> i32 {
        self.command_transaction_count
    }

    fn snark_jobs(&self) -> &[GraphQLSnarkJob] {
        &self.snark_jobs
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2122-L2133>
pub(crate) struct GraphQLProtocolStateProof(Arc<MinaBaseProofStableV2>);

/// A proof in base64, as precomputed blocks write it.
pub(crate) fn proof_to_base64(proof: &MinaBaseProofStableV2) -> Result<String, ConversionError> {
    match serde_json::to_value(PrecomputedBlockProof(proof.clone()))? {
        serde_json::Value::String(base64) => Ok(base64),
        _ => Err(ConversionError::Custom("proof is not a string".to_owned())),
    }
}

#[graphql_object(context = Context, name = "protocolStateProof")]
impl GraphQLProtocolStateProof {
    /// Base-64 encoded proof
    fn base64(&self) -> FieldResult<String> {
        Ok(proof_to_base64(&self.0)?)
    }

    /// JSON-encoded proof. Not written by this node yet, so null.
    fn json(&self) -> Option<String> {
        None
    }
}

/// Completed snark work.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L498-L513>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "CompletedWork")]
pub struct GraphQLSnarkJob {
    /// Amount the prover is paid for the snark work
    pub fee: String,
    /// Public key of the prover
    pub prover: String,
    /// Unique identifier for the snark work purchased: OCaml's
    /// `Transaction_snark.Statement.hash` of each proof's statement.
    pub work_ids: Vec<i32>,
}

impl From<&TransactionSnarkWorkTStableV2> for GraphQLSnarkJob {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_work/transaction_snark_work.ml#L110-L119>
    fn from(value: &TransactionSnarkWorkTStableV2) -> Self {
        let work_ids = match &value.proofs {
            TransactionSnarkWorkTStableV2Proofs::One(proof) => {
                vec![super::work_id::work_id_with_sok(&proof.0.statement)]
            }
            TransactionSnarkWorkTStableV2Proofs::Two((first, second)) => vec![
                super::work_id::work_id_with_sok(&first.0.statement),
                super::work_id::work_id_with_sok(&second.0.statement),
            ],
        };
        Self {
            fee: value.fee.to_string(),
            prover: value.prover.to_string(),
            work_ids,
        }
    }
}

/// Whether a fee transfer is paid from the transaction fees or from the
/// coinbase.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/filtered_external_transition/filtered_external_transition.ml#L6-L16>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeeTransferType {
    FeeTransfer,
    FeeTransferViaCoinbase,
}

impl FeeTransferType {
    fn as_str(self) -> &'static str {
        match self {
            Self::FeeTransfer => "Fee_transfer",
            Self::FeeTransferViaCoinbase => "Fee_transfer_via_coinbase",
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L425-L446>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "FeeTransfer")]
pub struct GraphQLFeeTransfer {
    /// Public key of fee transfer recipient
    pub recipient: String,
    /// Amount that the recipient is paid in this fee transfer
    pub fee: String,
    /// Fee_transfer|Fee_transfer_via_coinbase Snark worker fees deducted from
    /// the coinbase amount are of type 'Fee_transfer_via_coinbase', rest are
    /// deducted from transaction fees
    #[graphql(name = "type")]
    pub transfer_type: String,
}

impl GraphQLFeeTransfer {
    fn new(recipient: &CompressedPubKey, fee: u64, transfer_type: FeeTransferType) -> Self {
        Self {
            recipient: recipient.into_address(),
            fee: fee.to_string(),
            transfer_type: transfer_type.as_str().to_owned(),
        }
    }
}

/// Different types of transactions in a block.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2058-L2119>
#[derive(Debug)]
pub(crate) struct GraphQLTransactions {
    user_commands: Vec<GraphQLUserCommands>,
    zkapp_commands: Vec<GraphQLZkapp>,
    fee_transfer: Vec<GraphQLFeeTransfer>,
    coinbase: String,
    coinbase_receiver: Option<CompressedPubKey>,
}

#[graphql_object(context = Context, name = "Transactions")]
#[graphql(description = "Different types of transactions in a block")]
impl GraphQLTransactions {
    /// List of user commands (payments and stake delegations) included in
    /// this block
    fn user_commands(&self) -> &[GraphQLUserCommands] {
        &self.user_commands
    }

    /// List of zkApp commands included in this block
    fn zkapp_commands(&self) -> &[GraphQLZkapp] {
        &self.zkapp_commands
    }

    /// List of fee transfers included in this block
    fn fee_transfer(&self) -> &[GraphQLFeeTransfer] {
        &self.fee_transfer
    }

    /// Amount of MINA granted to the producer of this block
    fn coinbase(&self) -> &str {
        &self.coinbase
    }

    /// Account to which the coinbase for this block was granted
    async fn coinbase_receiver_account(
        &self,
        context: &Context,
    ) -> FieldResult<Option<GraphQLAccount>> {
        match &self.coinbase_receiver {
            Some(public_key) => Ok(Some(
                GraphQLAccount::load_or_empty(context, public_key.clone()).await?,
            )),
            None => Ok(None),
        }
    }
}

impl GraphQLTransactions {
    /// The transactions of a block, as OCaml's `Filtered_external_transition`
    /// lists them: the transactions in the order they apply, folded so that
    /// each one goes in front of the previous ones. Commands and fee
    /// transfers therefore come last applied first, and the coinbase is the
    /// sum of the block's coinbase parts.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/filtered_external_transition/filtered_external_transition.ml#L144-L229>
    pub(crate) fn new(
        diff: &StagedLedgerDiffDiffStableV2,
        consensus_state: &ConsensusProofOfStakeDataConsensusStateValueStableV2,
    ) -> Result<Self, ConversionError> {
        let commands = diff
            .diff
            .0
            .commands
            .iter()
            .chain(diff.diff.1.iter().flat_map(|second| second.commands.iter()))
            .rev();

        let mut user_commands = Vec::new();
        let mut zkapp_commands = Vec::new();
        for command in commands {
            match &command.data {
                MinaBaseUserCommandStableV2::SignedCommand(signed) => {
                    user_commands.push(GraphQLUserCommands::new(signed, Some(&command.status))?);
                }
                MinaBaseUserCommandStableV2::ZkappCommand(zkapp) => {
                    zkapp_commands.push(GraphQLZkapp::new(zkapp, Some(&command.status))?);
                }
            }
        }

        // Fee transfers and coinbases are not in the diff: they follow from
        // it, as when the staged ledger applies it.
        let ledger_diff = ledger::staged_ledger::diff::Diff::try_from(diff)
            .map_err(|_| ConversionError::InvalidBigInt)?;
        let transactions = ledger_diff
            .get_transactions(
                constraint_constants(),
                compressed(&consensus_state.coinbase_receiver)?,
                consensus_state.supercharge_coinbase,
            )
            .map_err(|e| ConversionError::Custom(format!("{e:?}")))?;

        let mut fee_transfer: Vec<GraphQLFeeTransfer> = Vec::new();
        let mut coinbase = Amount::zero();
        let mut coinbase_receiver = None;
        for transaction in transactions {
            match transaction.data {
                Transaction::Command(_) => {}
                Transaction::FeeTransfer(transfers) => {
                    let mut new: Vec<_> = transfers
                        .iter()
                        .map(|transfer| {
                            GraphQLFeeTransfer::new(
                                &transfer.receiver_pk,
                                transfer.fee.as_u64(),
                                FeeTransferType::FeeTransfer,
                            )
                        })
                        .collect();
                    new.append(&mut fee_transfer);
                    fee_transfer = new;
                }
                Transaction::Coinbase(part) => {
                    if let Some(transfer) = &part.fee_transfer {
                        fee_transfer.insert(
                            0,
                            GraphQLFeeTransfer::new(
                                &transfer.receiver_pk,
                                transfer.fee.as_u64(),
                                FeeTransferType::FeeTransferViaCoinbase,
                            ),
                        );
                    }
                    coinbase = coinbase
                        .checked_add(&part.amount)
                        .ok_or_else(|| ConversionError::Custom("coinbase overflow".to_owned()))?;
                    coinbase_receiver = Some(part.receiver);
                }
            }
        }

        Ok(Self {
            user_commands,
            zkapp_commands,
            fee_transfer,
            coinbase: coinbase.as_u64().to_string(),
            coinbase_receiver,
        })
    }
}

impl TryFrom<AppliedBlock> for GraphQLBlock {
    type Error = ConversionError;
    fn try_from(value: AppliedBlock) -> Result<Self, Self::Error> {
        let block = &value.block;
        let transactions = GraphQLTransactions::new(
            &block.body().staged_ledger_diff,
            &block.header().protocol_state.body.consensus_state,
        )?;
        let winner = compressed(
            &block
                .header()
                .protocol_state
                .body
                .consensus_state
                .block_stake_winner,
        )?;
        Self::with_transactions(
            value.block,
            Some(value.just_emitted_a_proof),
            winner,
            transactions,
        )
    }
}

impl GraphQLBlock {
    /// The genesis block as OCaml shows it: no transactions, but a coinbase
    /// to the genesis winner, who is also the creator and the winner.
    /// `just_emitted_a_proof` is that of the genesis block in the transition
    /// frontier, `None` when it is not there (after a hard fork, or once the
    /// root has moved past it).
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L2092-L2141>
    pub(crate) fn genesis(
        block: ArcBlockWithHash,
        just_emitted_a_proof: Option<bool>,
    ) -> Result<Self, ConversionError> {
        let genesis_winner = compressed(
            &block
                .header()
                .protocol_state
                .body
                .consensus_state
                .block_creator,
        )?;
        let transactions = GraphQLTransactions {
            user_commands: vec![],
            zkapp_commands: vec![],
            fee_transfer: vec![],
            coinbase: constraint_constants().coinbase_amount.to_string(),
            coinbase_receiver: Some(genesis_winner.clone()),
        };
        let mut genesis =
            Self::with_transactions(block, just_emitted_a_proof, genesis_winner, transactions)?;
        genesis.snark_jobs = vec![];
        genesis.command_transaction_count = 0;
        Ok(genesis)
    }

    fn with_transactions(
        block: ArcBlockWithHash,
        just_emitted_a_proof: Option<bool>,
        winner: CompressedPubKey,
        transactions: GraphQLTransactions,
    ) -> Result<Self, ConversionError> {
        let protocol_state = &block.header().protocol_state;
        let consensus_state = &protocol_state.body.consensus_state;

        Ok(Self {
            proof: block.header().protocol_state_proof.clone(),
            creator: compressed(&consensus_state.block_creator)?,
            winner,
            state_hash: block.hash.to_string(),
            state_hash_field: block.hash.to_decimal(),
            protocol_state: GraphQLProtocolState {
                previous_state_hash: protocol_state.previous_state_hash.to_string(),
                blockchain_state: GraphQLBlockchainState::new(
                    &protocol_state.body.blockchain_state,
                    just_emitted_a_proof,
                ),
                consensus_state: consensus_state.into(),
            },
            transactions,
            command_transaction_count: block.body().commands_iter().count().try_into()?,
            snark_jobs: block
                .body()
                .completed_works_iter()
                .map(GraphQLSnarkJob::from)
                .collect(),
        })
    }
}

#[derive(GraphQLObject, Debug)]
#[graphql(name = "ProtocolState")]
pub struct GraphQLProtocolState {
    /// Base58Check-encoded hash of the previous state
    pub previous_state_hash: String,
    /// State which is agnostic of a particular consensus algorithm
    pub blockchain_state: GraphQLBlockchainState,
    /// State specific to the minaboros Proof of Stake consensus algorithm
    pub consensus_state: GraphQLConsensusState,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L920-L1018>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "BlockchainState")]
pub struct GraphQLBlockchainState {
    /// Base58Check-encoded hash of the snarked ledger
    pub snarked_ledger_hash: String,
    /// Base58Check-encoded hash of the staged ledger hash's main ledger hash
    pub staged_ledger_hash: String,
    /// Base58Check-encoded hash of the staged ledger hash's aux_hash
    pub staged_ledger_aux_hash: String,
    /// Base58Check-encoded staged ledger hash's pending_coinbase_aux
    pub staged_ledger_pending_coinbase_aux: String,
    /// Base58Check-encoded hash of the staged ledger hash's
    /// pending_coinbase_hash
    pub staged_ledger_pending_coinbase_hash: String,
    /// date (stringified Unix time - number of milliseconds since January 1,
    /// 1970)
    pub date: String,
    /// utcDate (stringified Unix time - number of milliseconds since January
    /// 1, 1970)
    pub utc_date: String,
    /// Block finished a staged ledger, and a proof was emitted from it and
    /// included into this block's proof. If there is no transition frontier
    /// available or no block found, this will return null.
    pub staged_ledger_proof_emitted: Option<bool>,
    /// A reference to how the block header refers to the body of the block
    /// as a hex-encoded string
    pub body_reference: String,
}

/// The Base58Check form of a hash kept as bytes.
fn base58check_of_bytes<const V: u8>(hash: &Base58CheckOfBytes<ByteString, V>) -> String {
    mina_p2p_messages::b58::encode(hash.as_ref(), V)
}

impl GraphQLBlockchainState {
    fn new(
        state: &MinaStateBlockchainStateValueStableV2,
        just_emitted_a_proof: Option<bool>,
    ) -> Self {
        let staged_ledger_hash = &state.staged_ledger_hash;
        Self {
            snarked_ledger_hash: state
                .ledger_proof_statement
                .target
                .first_pass_ledger
                .to_string(),
            staged_ledger_hash: staged_ledger_hash.non_snark.ledger_hash.to_string(),
            staged_ledger_aux_hash: base58check_of_bytes(&staged_ledger_hash.non_snark.aux_hash),
            staged_ledger_pending_coinbase_aux: base58check_of_bytes(
                &staged_ledger_hash.non_snark.pending_coinbase_aux,
            ),
            staged_ledger_pending_coinbase_hash: staged_ledger_hash
                .pending_coinbase_hash
                .to_string(),
            date: state.timestamp.to_string(),
            utc_date: state.timestamp.to_string(),
            staged_ledger_proof_emitted: just_emitted_a_proof,
            body_reference: hex::encode(state.body_reference.0.as_ref()),
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/graphql_lib/consensus/graphql_scalars.ml#L121-L203>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "ConsensusState")]
pub struct GraphQLConsensusState {
    /// Length of the blockchain at this block
    #[graphql(deprecated = "use blockHeight instead")]
    pub blockchain_length: String,
    /// Height of the blockchain at this block
    pub block_height: String,
    pub epoch_count: String,
    pub min_window_density: String,
    pub last_vrf_output: String,
    /// Total currency in circulation at this block
    pub total_currency: String,
    pub staking_epoch_data: GraphQLEpochData,
    pub next_epoch_data: GraphQLEpochData,
    pub has_ancestor_in_same_checkpoint_window: bool,
    /// Slot in which this block was created
    pub slot: String,
    /// Slot since genesis (across all hard-forks)
    pub slot_since_genesis: String,
    /// Epoch in which this block was created
    pub epoch: String,
    /// Whether or not this coinbase was "supercharged", ie. created by an
    /// account that has no locked tokens
    pub supercharged_coinbase: bool,
    /// The public key that is responsible for winning this block (including
    /// delegations)
    pub block_stake_winner: String,
    /// The block producer public key that created this block
    pub block_creator: String,
    pub coinbase_receiever: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLEpochData {
    pub ledger: GraphQLLedger,
    pub seed: String,
    pub start_checkpoint: String,
    pub lock_checkpoint: String,
    pub epoch_length: String,
}

#[derive(GraphQLObject, Debug)]
#[graphql(name = "epochLedger")]
pub struct GraphQLLedger {
    pub hash: String,
    pub total_currency: String,
}

impl From<&mina_p2p_messages::v2::MinaBaseEpochLedgerValueStableV1> for GraphQLLedger {
    fn from(value: &mina_p2p_messages::v2::MinaBaseEpochLedgerValueStableV1) -> Self {
        Self {
            hash: value.hash.to_string(),
            total_currency: value.total_currency.as_u64().to_string(),
        }
    }
}

impl From<&mina_p2p_messages::v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1>
    for GraphQLEpochData
{
    fn from(
        value: &mina_p2p_messages::v2::ConsensusProofOfStakeDataEpochDataNextValueVersionedValueStableV1,
    ) -> Self {
        Self {
            ledger: (&value.ledger).into(),
            seed: value.seed.to_string(),
            start_checkpoint: value.start_checkpoint.to_string(),
            lock_checkpoint: value.lock_checkpoint.to_string(),
            epoch_length: value.epoch_length.as_u32().to_string(),
        }
    }
}

impl
    From<
        &mina_p2p_messages::v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1,
    > for GraphQLEpochData
{
    fn from(
        value: &mina_p2p_messages::v2::ConsensusProofOfStakeDataEpochDataStakingValueVersionedValueStableV1,
    ) -> Self {
        Self {
            ledger: (&value.ledger).into(),
            seed: value.seed.to_string(),
            start_checkpoint: value.start_checkpoint.to_string(),
            lock_checkpoint: value.lock_checkpoint.to_string(),
            epoch_length: value.epoch_length.as_u32().to_string(),
        }
    }
}

impl From<&ConsensusProofOfStakeDataConsensusStateValueStableV2> for GraphQLConsensusState {
    /// The slot and epoch are those of the global slot since the hard fork,
    /// as OCaml's `curr_slot` and `curr_epoch`.
    fn from(value: &ConsensusProofOfStakeDataConsensusStateValueStableV2) -> Self {
        let slot_number = value.curr_global_slot_since_hard_fork.slot_number.as_u32();
        let slots_per_epoch = value
            .curr_global_slot_since_hard_fork
            .slots_per_epoch
            .as_u32()
            .max(1);
        let last_vrf_output =
            Base58CheckOfBinProt::<ByteString, ByteString, VRF_TRUNCATED_OUTPUT_VERSION>::from(
                value.last_vrf_output.0.clone(),
            )
            .to_string();

        Self {
            blockchain_length: value.blockchain_length.as_u32().to_string(),
            block_height: value.blockchain_length.as_u32().to_string(),
            epoch_count: value.epoch_count.as_u32().to_string(),
            min_window_density: value.min_window_density.as_u32().to_string(),
            last_vrf_output,
            total_currency: value.total_currency.as_u64().to_string(),
            staking_epoch_data: (&value.staking_epoch_data).into(),
            next_epoch_data: (&value.next_epoch_data).into(),
            has_ancestor_in_same_checkpoint_window: value.has_ancestor_in_same_checkpoint_window,
            slot: (slot_number % slots_per_epoch).to_string(),
            slot_since_genesis: value.global_slot_since_genesis.as_u32().to_string(),
            epoch: (slot_number / slots_per_epoch).to_string(),
            supercharged_coinbase: value.supercharge_coinbase,
            block_stake_winner: value.block_stake_winner.to_string(),
            block_creator: value.block_creator.to_string(),
            coinbase_receiever: value.coinbase_receiver.to_string(),
        }
    }
}

/// The kind of user command.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1757-L1762>
#[derive(Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
pub enum GraphQLUserCommandsKind {
    PAYMENT,
    STAKE_DELEGATION,
}

/// A payment or a stake delegation: the fields of OCaml's `UserCommand`
/// interface, which its `UserCommandPayment` and `UserCommandDelegation`
/// share.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1850-L2005>
#[derive(Debug)]
pub(crate) struct UserCommandData {
    id: String,
    hash: String,
    kind: GraphQLUserCommandsKind,
    nonce: u32,
    fee_payer: CompressedPubKey,
    receiver: CompressedPubKey,
    valid_until: String,
    amount: String,
    fee: String,
    memo: String,
    failure_reason: Option<String>,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1771-L1843>
#[juniper::graphql_interface(
    name = "UserCommand", context = Context,
    for = [UserCommandPayment, UserCommandDelegation], enum = GraphQLUserCommands
)]
#[allow(dead_code)] // Juniper dispatches through the generated enum, not this trait.
pub(crate) trait UserCommand {
    fn id(&self) -> TransactionIdScalar;
    fn hash(&self) -> TransactionHashScalar;
    fn kind(&self) -> UserCommandKindScalar;
    fn nonce(&self) -> CommandNonceScalar;
    async fn source(&self, context: &Context) -> FieldResult<GraphQLAccount>;
    async fn receiver(&self, context: &Context) -> FieldResult<GraphQLAccount>;
    async fn fee_payer(&self, context: &Context) -> FieldResult<GraphQLAccount>;
    fn valid_until(&self) -> GlobalSlotScalar;
    fn token(&self) -> TokenIdScalar;
    fn amount(&self) -> AmountScalar;
    fn fee_token(&self) -> TokenIdScalar;
    fn fee(&self) -> FeeScalar;
    fn memo(&self) -> &str;
    #[graphql(deprecated = "use kind field instead")]
    fn is_delegation(&self) -> bool;
    #[graphql(name = "from", deprecated = "use feePayer field instead")]
    fn sender(&self) -> PublicKeyScalar;
    #[graphql(name = "fromAccount", deprecated = "use feePayer field instead")]
    async fn sender_account(&self, context: &Context) -> FieldResult<GraphQLAccount>;
    #[graphql(deprecated = "use receiver field instead")]
    fn to(&self) -> PublicKeyScalar;
    #[graphql(deprecated = "use receiver field instead")]
    async fn to_account(&self, context: &Context) -> FieldResult<GraphQLAccount>;
    fn failure_reason(&self) -> Option<TransactionStatusFailureScalar>;
}

#[derive(Debug)]
pub(crate) struct UserCommandPayment(UserCommandData);
#[derive(Debug)]
pub(crate) struct UserCommandDelegation(UserCommandData);

// The concrete types expose exactly the same common fields. Keeping their
// resolvers together also keeps send, block and pool responses in agreement.
macro_rules! user_command_object {
    ($ty:ident, $($extra:item),* $(,)?) => {
        #[graphql_object(context = Context, impl = GraphQLUserCommands)]
        impl $ty {
            fn id(&self) -> TransactionIdScalar {
                TransactionIdScalar(self.0.id.clone())
            }

            fn hash(&self) -> TransactionHashScalar {
                TransactionHashScalar(self.0.hash.clone())
            }

            /// String describing the kind of user command
            fn kind(&self) -> UserCommandKindScalar {
                UserCommandKindScalar(match self.0.kind {
                    GraphQLUserCommandsKind::PAYMENT => "PAYMENT",
                    GraphQLUserCommandsKind::STAKE_DELEGATION => "STAKE_DELEGATION",
                }.to_owned())
            }

            /// Sequence number of command for the fee-payer's account
            fn nonce(&self) -> CommandNonceScalar {
                CommandNonceScalar(i64::from(self.0.nonce))
            }

            /// Account that the command is sent from
            async fn source(&self, context: &Context) -> FieldResult<GraphQLAccount> {
                Ok(GraphQLAccount::load_or_empty(context, self.0.fee_payer.clone()).await?)
            }

            /// Account that the command applies to
            async fn receiver(&self, context: &Context) -> FieldResult<GraphQLAccount> {
                Ok(GraphQLAccount::load_or_empty(context, self.0.receiver.clone()).await?)
            }

            /// Account that pays the fees for the command
            #[graphql(deprecated = "use source field instead")]
            async fn fee_payer(&self, context: &Context) -> FieldResult<GraphQLAccount> {
                Ok(GraphQLAccount::load_or_empty(context, self.0.fee_payer.clone()).await?)
            }

            /// The global slot number after which this transaction cannot be applied
            fn valid_until(&self) -> GlobalSlotScalar {
                GlobalSlotScalar(self.0.valid_until.clone())
            }

            /// Token used for the transaction
            fn token(&self) -> TokenIdScalar {
                TokenIdScalar(TokenIdKeyHash::default().to_string())
            }

            /// Amount that the source is sending to receiver; 0 for commands without
            /// an associated amount
            fn amount(&self) -> AmountScalar {
                AmountScalar(self.0.amount.clone())
            }

            /// Token used to pay the fee
            fn fee_token(&self) -> TokenIdScalar {
                TokenIdScalar(TokenIdKeyHash::default().to_string())
            }

            /// Fee that the fee-payer is willing to pay for making the transaction
            fn fee(&self) -> FeeScalar {
                FeeScalar(self.0.fee.clone())
            }

            /// A short message from the sender, encoded with Base58Check, version
            /// byte=0x14; byte 2 of the decoding is the message length
            fn memo(&self) -> &str {
                &self.0.memo
            }

            /// If true, this command represents a delegation of stake
            #[graphql(deprecated = "use kind field instead")]
            fn is_delegation(&self) -> bool {
                matches!(self.0.kind, GraphQLUserCommandsKind::STAKE_DELEGATION)
            }

            /// Public key of the sender
            #[graphql(name = "from", deprecated = "use feePayer field instead")]
            fn sender(&self) -> PublicKeyScalar {
                PublicKeyScalar(self.0.fee_payer.into_address())
            }

            /// Account of the sender
            #[graphql(name = "fromAccount", deprecated = "use feePayer field instead")]
            async fn sender_account(&self, context: &Context) -> FieldResult<GraphQLAccount> {
                Ok(GraphQLAccount::load_or_empty(context, self.0.fee_payer.clone()).await?)
            }

            /// Public key of the receiver
            #[graphql(deprecated = "use receiver field instead")]
            fn to(&self) -> PublicKeyScalar {
                PublicKeyScalar(self.0.receiver.into_address())
            }

            /// Account of the receiver
            #[graphql(deprecated = "use receiver field instead")]
            async fn to_account(&self, context: &Context) -> FieldResult<GraphQLAccount> {
                Ok(GraphQLAccount::load_or_empty(context, self.0.receiver.clone()).await?)
            }

            /// null is no failure or status unknown, reason for failure otherwise.
            fn failure_reason(&self) -> Option<TransactionStatusFailureScalar> {
                self.0.failure_reason.clone().map(TransactionStatusFailureScalar)
            }
            $($extra)*
        }
    };
}
user_command_object!(UserCommandPayment,);
user_command_object!(
    UserCommandDelegation,
    async fn delegator(&self, context: &Context) -> FieldResult<GraphQLAccount> {
        self.source(context).await
    },
    async fn delegatee(&self, context: &Context) -> FieldResult<GraphQLAccount> {
        self.receiver(context).await
    }
);

impl GraphQLUserCommands {
    /// A signed command, with its status when a block includes it, or `None`
    /// for a command in the transaction pool. The failure reason is the
    /// command's first failure, as OCaml shows it.
    pub(crate) fn new(
        user_command: &MinaBaseSignedCommandStableV2,
        status: Option<&MinaBaseTransactionStatusStableV2>,
    ) -> Result<Self, ConversionError> {
        let common = &user_command.payload.common;
        let (receiver, amount, kind) = match &user_command.payload.body {
            MinaBaseSignedCommandPayloadBodyStableV2::Payment(payment) => (
                &payment.receiver_pk,
                payment.amount.as_u64(),
                GraphQLUserCommandsKind::PAYMENT,
            ),
            MinaBaseSignedCommandPayloadBodyStableV2::StakeDelegation(
                MinaBaseStakeDelegationStableV2::SetDelegate { new_delegate },
            ) => (new_delegate, 0, GraphQLUserCommandsKind::STAKE_DELEGATION),
        };
        let failure_reason = match status {
            Some(MinaBaseTransactionStatusStableV2::Failed(failures)) => failures
                .0
                .iter()
                .flat_map(|failures| failures.iter())
                .next()
                .map(|failure| TransactionFailure::from(failure).to_string()),
            Some(MinaBaseTransactionStatusStableV2::Applied) | None => None,
        };

        let data = UserCommandData {
            id: user_command.to_base64()?,
            hash: user_command.hash()?.to_string(),
            kind,
            nonce: common.nonce.as_u32(),
            fee_payer: compressed(&common.fee_payer_pk)?,
            receiver: compressed(receiver)?,
            valid_until: common.valid_until.as_u32().to_string(),
            amount: amount.to_string(),
            fee: common.fee.as_u64().to_string(),
            memo: common.memo.to_base58check(),
            failure_reason,
        };
        Ok(match kind {
            GraphQLUserCommandsKind::PAYMENT => UserCommandPayment(data).into(),
            GraphQLUserCommandsKind::STAKE_DELEGATION => UserCommandDelegation(data).into(),
        })
    }
}

impl TryFrom<MinaBaseSignedCommandStableV2> for GraphQLUserCommands {
    type Error = ConversionError;

    fn try_from(user_command: MinaBaseSignedCommandStableV2) -> Result<Self, Self::Error> {
        Self::new(&user_command, None)
    }
}
