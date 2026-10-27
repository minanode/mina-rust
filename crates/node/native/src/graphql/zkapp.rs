use crate::input_limits::{self as limits, bounded};
use std::str::FromStr;

use juniper::{GraphQLInputObject, GraphQLObject};
use ledger::{scan_state::transaction_logic::TransactionFailure, FpExt, VerificationKey};
use mina_p2p_messages::{
    bigint::BigInt,
    list::List,
    pseq::PaddedSeq,
    string::{TokenSymbol, ZkAppUri},
    v2::{
        CurrencyAmountStableV1, CurrencyBalanceStableV1, CurrencyFeeStableV1,
        MinaBaseAccountUpdateAccountPreconditionStableV1,
        MinaBaseAccountUpdateAuthorizationKindStableV1, MinaBaseAccountUpdateBodyEventsStableV1,
        MinaBaseAccountUpdateBodyFeePayerStableV1, MinaBaseAccountUpdateBodyStableV1,
        MinaBaseAccountUpdateFeePayerStableV1, MinaBaseAccountUpdateMayUseTokenStableV1,
        MinaBaseAccountUpdatePreconditionsStableV1, MinaBaseAccountUpdateTStableV1,
        MinaBaseAccountUpdateUpdateStableV1, MinaBaseAccountUpdateUpdateStableV1AppStateA,
        MinaBaseAccountUpdateUpdateStableV1Delegate,
        MinaBaseAccountUpdateUpdateStableV1Permissions, MinaBaseAccountUpdateUpdateStableV1Timing,
        MinaBaseAccountUpdateUpdateStableV1TokenSymbol,
        MinaBaseAccountUpdateUpdateStableV1VerificationKey,
        MinaBaseAccountUpdateUpdateStableV1VotingFor, MinaBaseAccountUpdateUpdateStableV1ZkappUri,
        MinaBaseAccountUpdateUpdateTimingInfoStableV1, MinaBaseControlStableV2,
        MinaBasePermissionsStableV2, MinaBaseReceiptChainHashStableV1,
        MinaBaseSignedCommandMemoStableV1, MinaBaseTransactionStatusFailureCollectionStableV1,
        MinaBaseTransactionStatusStableV2, MinaBaseUserCommandStableV2,
        MinaBaseVerificationKeyWireStableV1, MinaBaseZkappCommandTStableV1WireStableV1,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesA,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
        MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAACallsA,
        MinaBaseZkappPreconditionAccountStableV2, MinaBaseZkappPreconditionAccountStableV2Balance,
        MinaBaseZkappPreconditionAccountStableV2BalanceA,
        MinaBaseZkappPreconditionAccountStableV2Delegate,
        MinaBaseZkappPreconditionAccountStableV2ProvedState,
        MinaBaseZkappPreconditionAccountStableV2ReceiptChainHash,
        MinaBaseZkappPreconditionAccountStableV2StateA,
        MinaBaseZkappPreconditionProtocolStateEpochDataStableV1,
        MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochLedger,
        MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochSeed,
        MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint,
        MinaBaseZkappPreconditionProtocolStateStableV1,
        MinaBaseZkappPreconditionProtocolStateStableV1Amount,
        MinaBaseZkappPreconditionProtocolStateStableV1AmountA,
        MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot,
        MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlotA,
        MinaBaseZkappPreconditionProtocolStateStableV1Length,
        MinaBaseZkappPreconditionProtocolStateStableV1LengthA,
        MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash,
        MinaNumbersGlobalSlotSinceGenesisMStableV1, MinaNumbersGlobalSlotSpanStableV1,
        MinaStateBlockchainStateValueStableV2SignedAmount, PicklesProofProofsVerifiedMaxStableV2,
        StateHash,
    },
};

use mina_node::account::AccountPublicKey;
use serde::Deserialize;

fn decode_memo(input: &str) -> Result<MinaBaseSignedCommandMemoStableV1, ConversionError> {
    let bytes = mina_p2p_messages::b58::decode(
        bounded(input, limits::MEMO)?,
        mina_p2p_messages::b58version::USER_COMMAND_MEMO,
    )?;
    if bytes.len() != 35 {
        return Err(ConversionError::InvalidLength);
    }
    Ok(MinaBaseSignedCommandMemoStableV1(bytes[1..].into()))
}

fn decimal_field(input: &str) -> Result<BigInt, ConversionError> {
    Ok(BigInt::from_decimal(bounded(
        input,
        limits::FIELD_DECIMAL,
    )?)?)
}

use super::{
    account::{GraphQLTiming, InputGraphQLTiming},
    ConversionError,
};

#[derive(GraphQLInputObject, Debug)]
pub struct SendZkappInput {
    pub zkapp_command: InputGraphQLZkappCommand,
}

impl TryFrom<SendZkappInput> for MinaBaseUserCommandStableV2 {
    type Error = ConversionError;
    fn try_from(value: SendZkappInput) -> Result<Self, Self::Error> {
        value.zkapp_command.try_into()
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLSendZkappResponse {
    pub zkapp: GraphQLZkapp,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2019-L2055>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "ZkappCommandResult")]
pub struct GraphQLZkapp {
    /// A cryptographic hash of the zkApp command
    pub hash: String,
    /// The reason for the zkApp transaction failure; null means success or
    /// the status is unknown
    pub failure_reason: Option<Vec<GraphQLFailureReason>>,
    /// A Base64 string representing the zkApp command
    pub id: String,
    /// zkApp command representing the transaction
    pub zkapp_command: GraphQLZkappCommand,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLZkapp {
    // pub hash: String,
    // pub failure_reason: Option<Vec<GraphQLFailureReason>>,
    /// Zkapp represented as base64 string
    // pub id: String,
    pub zkapp_command: InputGraphQLZkappCommand,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLZkappCommand {
    pub memo: String,
    pub account_updates: Vec<GraphQLAccountUpdate>,
    pub fee_payer: GraphQLFeePayer,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLZkappCommand {
    pub memo: Option<String>,
    pub account_updates: Vec<InputGraphQLAccountUpdate>,
    pub fee_payer: InputGraphQLFeePayer,
}

impl GraphQLZkapp {
    /// A zkApp command, with its status when a block includes it, or `None`
    /// for a command in the transaction pool.
    pub(crate) fn new(
        zkapp: &MinaBaseZkappCommandTStableV1WireStableV1,
        status: Option<&MinaBaseTransactionStatusStableV2>,
    ) -> Result<Self, ConversionError> {
        let failure_reason = match status {
            Some(MinaBaseTransactionStatusStableV2::Failed(failures)) => {
                Some(GraphQLFailureReason::from_collection(failures))
            }
            Some(MinaBaseTransactionStatusStableV2::Applied) | None => None,
        };

        Ok(GraphQLZkapp {
            hash: zkapp.hash()?.to_string(),
            failure_reason,
            id: zkapp.to_base64()?,
            zkapp_command: GraphQLZkappCommand {
                memo: zkapp.memo.to_base58check(),
                account_updates: flatten_account_updates(&zkapp.account_updates)?,
                fee_payer: GraphQLFeePayer::from(zkapp.fee_payer.clone()),
            },
        })
    }
}

/// The account updates of a call forest as one list, each parent before its
/// calls, with the depth of each in `callDepth`: the form OCaml's GraphQL
/// and JSON give a zkApp command.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_command.ml#L881-L895>
pub(crate) fn flatten_account_updates(
    forest: &List<MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesA>,
) -> Result<Vec<GraphQLAccountUpdate>, ConversionError> {
    fn calls(
        forest: &List<MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAACallsA>,
        depth: i32,
        out: &mut Vec<GraphQLAccountUpdate>,
    ) -> Result<(), ConversionError> {
        for tree in forest.iter() {
            tree_into(&tree.elt, depth, out)?;
        }
        Ok(())
    }
    fn tree_into(
        tree: &MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA,
        depth: i32,
        out: &mut Vec<GraphQLAccountUpdate>,
    ) -> Result<(), ConversionError> {
        let mut update = GraphQLAccountUpdate::try_from(tree.account_update.clone())?;
        update.body.call_depth = depth;
        out.push(update);
        calls(&tree.calls, depth + 1, out)
    }

    let mut out = Vec::new();
    for tree in forest.iter() {
        tree_into(&tree.elt, 0, &mut out)?;
    }
    Ok(out)
}

impl TryFrom<MinaBaseZkappCommandTStableV1WireStableV1> for GraphQLZkapp {
    type Error = ConversionError;

    fn try_from(zkapp: MinaBaseZkappCommandTStableV1WireStableV1) -> Result<Self, Self::Error> {
        Self::new(&zkapp, None)
    }
}

impl TryFrom<MinaBaseUserCommandStableV2> for GraphQLSendZkappResponse {
    type Error = ConversionError;
    fn try_from(value: MinaBaseUserCommandStableV2) -> Result<Self, Self::Error> {
        if let MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = value {
            Ok(GraphQLSendZkappResponse {
                zkapp: GraphQLZkapp::try_from(zkapp)?,
            })
        } else {
            Err(ConversionError::WrongVariant)
        }
    }
}

impl TryFrom<InputGraphQLZkappCommand> for MinaBaseUserCommandStableV2 {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLZkappCommand) -> Result<Self, Self::Error> {
        Ok(MinaBaseUserCommandStableV2::ZkappCommand(
            MinaBaseZkappCommandTStableV1WireStableV1 {
                fee_payer: value.fee_payer.try_into()?,
                account_updates: try_tree_from_account_updates(List::from_iter(
                    value.account_updates,
                ))?,
                memo: if let Some(memo) = value.memo {
                    decode_memo(&memo)?
                } else {
                    let empty_memo = ledger::scan_state::transaction_logic::Memo::empty();
                    MinaBaseSignedCommandMemoStableV1::from(&empty_memo)
                },
            },
        ))
    }
}

/// Recursively builds a tree of account updates based on their depth.
fn try_tree_from_account_updates(
    updates: List<InputGraphQLAccountUpdate>,
) -> Result<List<MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesA>, ConversionError> {
    let result =
        try_tree_from_account_updates_aux(updates)?
            .into_iter()
            .map(|update| {
                let MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAACallsA {
                    elt,
                    stack_hash,
                } = update;
                let MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA {
                    account_update,
                    account_update_digest,
                    calls,
                } = *elt;
                let elt = MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA {
                    account_update,
                    account_update_digest,
                    calls,
                };
                MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesA { elt, stack_hash }
            })
            .collect();
    Ok(result)
}

fn try_tree_from_account_updates_aux(
    updates: List<InputGraphQLAccountUpdate>,
) -> Result<List<MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAACallsA>, ConversionError>
{
    let mut result = List::new();
    let mut iter = updates.into_iter().peekable();

    while let Some(account_update) = iter.next() {
        let depth = account_update.body.call_depth;
        let mut children = List::new();

        while let Some(next_p) = iter.peek() {
            if next_p.body.call_depth > depth {
                // unwrap cannot fail, we just peeked it
                children.push_back(iter.next().unwrap());
            } else {
                break;
            }
        }

        let calls = try_tree_from_account_updates_aux(children)?;

        result.push_back(
            MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAACallsA {
                elt: Box::new(MinaBaseZkappCommandTStableV1WireStableV1AccountUpdatesAA {
                    account_update: account_update.try_into()?, // FIXME: remove unwrap
                    account_update_digest: (),
                    calls,
                }),
                stack_hash: (),
            },
        );
    }

    Ok(result)
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLFeePayer {
    pub body: GraphQLFeePayerBody,
    pub authorization: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLFeePayer {
    pub body: InputGraphQLFeePayerBody,
    pub authorization: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLFeePayerBody {
    pub public_key: String,
    pub fee: String,
    pub valid_until: Option<String>,
    pub nonce: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLFeePayerBody {
    pub public_key: String,
    pub fee: String,
    pub valid_until: Option<String>,
    pub nonce: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAccountUpdate {
    pub body: GraphQLAccountUpdateBody,
    pub authorization: GraphQLAuthorization,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAccountUpdate {
    pub body: InputGraphQLAccountUpdateBody,
    pub authorization: InputGraphQLAuthorization,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAuthorization {
    pub proof: Option<String>,
    pub signature: Option<String>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAuthorization {
    pub proof: Option<String>,
    pub signature: Option<String>,
}

impl TryFrom<MinaBaseControlStableV2> for GraphQLAuthorization {
    type Error = ConversionError;

    fn try_from(value: MinaBaseControlStableV2) -> Result<Self, Self::Error> {
        let auth = match value {
            MinaBaseControlStableV2::Signature(signature) => GraphQLAuthorization {
                proof: None,
                signature: Some(signature.to_string()),
            },
            MinaBaseControlStableV2::Proof(proof) => GraphQLAuthorization {
                proof: Some(
                    serde_json::to_string_pretty(&proof)?
                        .trim_matches('"')
                        .to_string(),
                ),
                signature: None,
            },
            MinaBaseControlStableV2::NoneGiven => GraphQLAuthorization {
                proof: None,
                signature: None,
            },
        };
        Ok(auth)
    }
}

impl TryFrom<InputGraphQLAuthorization> for MinaBaseControlStableV2 {
    type Error = ConversionError;

    fn try_from(value: InputGraphQLAuthorization) -> Result<Self, Self::Error> {
        match (value.signature, value.proof) {
            (Some(signature), None) => {
                // Handle signature case
                Ok(MinaBaseControlStableV2::Signature(
                    bounded(&signature, limits::SIGNATURE)?.parse()?,
                ))
            }
            (None, Some(proof)) => {
                bounded(&proof, limits::OPAQUE_BASE64)?;
                // Handle proof case
                let proof = PicklesProofProofsVerifiedMaxStableV2::deserialize(
                    serde_json::Value::String(proof),
                )?;
                Ok(MinaBaseControlStableV2::Proof(proof.into()))
            }
            (None, None) => Ok(MinaBaseControlStableV2::NoneGiven),
            _ => Err(ConversionError::Custom(
                "Either signature or proof must be provided, but not both".into(),
            )),
        }
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAccountUpdateBody {
    pub public_key: String,
    pub token_id: String,
    pub use_full_commitment: bool,
    pub increment_nonce: bool,
    pub update: GraphQLAccountUpdateUpdate,
    pub balance_change: GraphQLBalanceChange,
    pub events: Vec<Vec<String>>,
    pub actions: Vec<Vec<String>>,
    pub call_data: String,
    pub call_depth: i32,
    pub preconditions: GraphQLPreconditions,
    pub may_use_token: GraphQLMayUseToken,
    pub authorization_kind: GraphQLAuthorizationKind,
    pub implicit_account_creation_fee: bool,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAccountUpdateBody {
    pub public_key: String,
    pub token_id: String,
    pub use_full_commitment: bool,
    pub increment_nonce: bool,
    pub update: InputGraphQLAccountUpdateUpdate,
    pub balance_change: InputGraphQLBalanceChange,
    pub events: Vec<Vec<String>>,
    pub actions: Vec<Vec<String>>,
    pub call_data: String,
    pub call_depth: i32,
    pub preconditions: InputGraphQLPreconditions,
    pub may_use_token: InputGraphQLMayUseToken,
    pub authorization_kind: InputGraphQLAuthorizationKind,
    pub implicit_account_creation_fee: bool,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAuthorizationKind {
    pub is_signed: bool,
    pub is_proved: bool,
    pub verification_key_hash: Option<String>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAuthorizationKind {
    pub is_signed: bool,
    pub is_proved: bool,
    pub verification_key_hash: Option<String>,
}

impl From<MinaBaseAccountUpdateAuthorizationKindStableV1> for GraphQLAuthorizationKind {
    fn from(value: MinaBaseAccountUpdateAuthorizationKindStableV1) -> Self {
        match value {
            MinaBaseAccountUpdateAuthorizationKindStableV1::Signature => GraphQLAuthorizationKind {
                is_signed: true,
                is_proved: false,
                verification_key_hash: None,
            },
            MinaBaseAccountUpdateAuthorizationKindStableV1::Proof(proof) => {
                GraphQLAuthorizationKind {
                    is_signed: false,
                    is_proved: true,
                    verification_key_hash: Some(proof.to_decimal()),
                }
            }
            MinaBaseAccountUpdateAuthorizationKindStableV1::NoneGiven => GraphQLAuthorizationKind {
                is_signed: false,
                is_proved: false,
                verification_key_hash: None,
            },
        }
    }
}

impl TryFrom<InputGraphQLAuthorizationKind> for MinaBaseAccountUpdateAuthorizationKindStableV1 {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLAuthorizationKind) -> Result<Self, Self::Error> {
        if value.is_signed {
            return Ok(MinaBaseAccountUpdateAuthorizationKindStableV1::Signature);
        }

        if value.is_proved {
            match &value.verification_key_hash {
                Some(vk_hash) => {
                    let big_int = decimal_field(vk_hash)?;
                    return Ok(MinaBaseAccountUpdateAuthorizationKindStableV1::Proof(
                        big_int,
                    ));
                }
                None => {
                    return Err(ConversionError::MissingField(
                        "verification_key_hash".to_string(),
                    ));
                }
            }
        }

        Ok(MinaBaseAccountUpdateAuthorizationKindStableV1::NoneGiven)
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLMayUseToken {
    pub parents_own_token: bool,
    pub inherit_from_parent: bool,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLMayUseToken {
    pub parents_own_token: bool,
    pub inherit_from_parent: bool,
}

impl From<MinaBaseAccountUpdateMayUseTokenStableV1> for GraphQLMayUseToken {
    fn from(value: MinaBaseAccountUpdateMayUseTokenStableV1) -> Self {
        match value {
            MinaBaseAccountUpdateMayUseTokenStableV1::ParentsOwnToken => GraphQLMayUseToken {
                parents_own_token: true,
                inherit_from_parent: false,
            },
            MinaBaseAccountUpdateMayUseTokenStableV1::InheritFromParent => GraphQLMayUseToken {
                parents_own_token: false,
                inherit_from_parent: true,
            },
            MinaBaseAccountUpdateMayUseTokenStableV1::No => GraphQLMayUseToken {
                parents_own_token: false,
                inherit_from_parent: false,
            },
        }
    }
}

impl From<InputGraphQLMayUseToken> for MinaBaseAccountUpdateMayUseTokenStableV1 {
    fn from(value: InputGraphQLMayUseToken) -> Self {
        if value.parents_own_token {
            MinaBaseAccountUpdateMayUseTokenStableV1::ParentsOwnToken
        } else if value.inherit_from_parent {
            MinaBaseAccountUpdateMayUseTokenStableV1::InheritFromParent
        } else {
            MinaBaseAccountUpdateMayUseTokenStableV1::No
        }
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLEvent {
    pub event: String,
    pub data: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLEvent {
    pub event: String,
    pub data: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAction {
    pub action: String,
    pub data: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAction {
    pub action: String,
    pub data: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLPreconditions {
    pub network: GraphQLPreconditionsNetwork,
    pub account: GraphQLPreconditionsAccount,
    pub valid_while: Option<GraphQLPreconditionsNetworkBounds>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLPreconditions {
    pub network: InputGraphQLPreconditionsNetwork,
    pub account: InputGraphQLPreconditionsAccount,
    pub valid_while: Option<InputGraphQLPreconditionsNetworkBounds>,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLPreconditionsAccount {
    pub balance: Option<GraphQLPreconditionsNetworkBounds>,
    pub nonce: Option<GraphQLPreconditionsNetworkBounds>,
    pub receipt_chain_hash: Option<String>,
    pub delegate: Option<String>,
    pub state: Vec<Option<String>>,
    pub action_state: Option<String>,
    pub proved_state: Option<bool>,
    pub is_new: Option<bool>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLPreconditionsAccount {
    pub balance: Option<InputGraphQLPreconditionsNetworkBounds>,
    pub nonce: Option<InputGraphQLPreconditionsNetworkBounds>,
    pub receipt_chain_hash: Option<String>,
    pub delegate: Option<String>,
    pub state: Vec<Option<String>>,
    pub action_state: Option<String>,
    pub proved_state: Option<bool>,
    pub is_new: Option<bool>,
}

impl From<MinaBaseAccountUpdateAccountPreconditionStableV1> for GraphQLPreconditionsAccount {
    fn from(value: MinaBaseAccountUpdateAccountPreconditionStableV1) -> Self {
        Self {
            balance: if let MinaBaseZkappPreconditionAccountStableV2Balance::Check(v) =
                value.0.balance
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u64().to_string(),
                    lower: v.lower.as_u64().to_string(),
                })
            } else {
                None
            },
            nonce: if let MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(v) =
                value.0.nonce
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u32().to_string(),
                    lower: v.lower.as_u32().to_string(),
                })
            } else {
                None
            },
            receipt_chain_hash:
                if let MinaBaseZkappPreconditionAccountStableV2ReceiptChainHash::Check(v) =
                    value.0.receipt_chain_hash
                {
                    Some(v.to_decimal())
                } else {
                    None
                },
            delegate: if let MinaBaseZkappPreconditionAccountStableV2Delegate::Check(v) =
                value.0.delegate
            {
                Some(v.to_string())
            } else {
                None
            },
            state: value
                .0
                .state
                .clone()
                .iter()
                .map(|v| {
                    if let MinaBaseZkappPreconditionAccountStableV2StateA::Check(state_value) = v {
                        Some(state_value.to_decimal())
                    } else {
                        None
                    }
                })
                .collect(),
            action_state: if let MinaBaseZkappPreconditionAccountStableV2StateA::Check(value) =
                value.0.action_state
            {
                Some(value.to_decimal())
            } else {
                None
            },
            proved_state: if let MinaBaseZkappPreconditionAccountStableV2ProvedState::Check(v) =
                value.0.proved_state
            {
                Some(v)
            } else {
                None
            },
            is_new: if let MinaBaseZkappPreconditionAccountStableV2ProvedState::Check(v) =
                value.0.is_new
            {
                Some(v)
            } else {
                None
            },
        }
    }
}

impl TryFrom<InputGraphQLPreconditionsAccount>
    for MinaBaseAccountUpdateAccountPreconditionStableV1
{
    type Error = ConversionError;
    fn try_from(value: InputGraphQLPreconditionsAccount) -> Result<Self, Self::Error> {
        let state: Result<Vec<_>, _> = value
            .state
            .iter()
            .map(|v| {
                if let Some(state) = v {
                    decimal_field(state).map(MinaBaseZkappPreconditionAccountStableV2StateA::Check)
                } else {
                    Ok(MinaBaseZkappPreconditionAccountStableV2StateA::Ignore)
                }
            })
            .collect();

        let state = state?;
        Ok(Self(MinaBaseZkappPreconditionAccountStableV2 {
            balance: if let Some(balance) = value.balance {
                MinaBaseZkappPreconditionAccountStableV2Balance::Check(
                    MinaBaseZkappPreconditionAccountStableV2BalanceA {
                        lower: CurrencyBalanceStableV1(CurrencyAmountStableV1(
                            bounded(&balance.lower, limits::U64_DECIMAL)?
                                .parse::<u64>()?
                                .into(),
                        )),
                        upper: CurrencyBalanceStableV1(CurrencyAmountStableV1(
                            bounded(&balance.upper, limits::U64_DECIMAL)?
                                .parse::<u64>()?
                                .into(),
                        )),
                    },
                )
            } else {
                MinaBaseZkappPreconditionAccountStableV2Balance::Ignore
            },
            nonce: if let Some(nonce) = value.nonce {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1LengthA {
                        lower: (bounded(&nonce.lower, limits::U32_DECIMAL)?.parse::<u32>()?).into(),
                        upper: (bounded(&nonce.upper, limits::U32_DECIMAL)?.parse::<u32>()?).into(),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Ignore
            },
            receipt_chain_hash: if let Some(receipt_chain_hash) = value.receipt_chain_hash {
                MinaBaseZkappPreconditionAccountStableV2ReceiptChainHash::Check(
                    MinaBaseReceiptChainHashStableV1(decimal_field(&receipt_chain_hash)?),
                )
            } else {
                MinaBaseZkappPreconditionAccountStableV2ReceiptChainHash::Ignore
            },
            delegate: if let Some(delegate) = value.delegate {
                MinaBaseZkappPreconditionAccountStableV2Delegate::Check(
                    AccountPublicKey::from_str(bounded(&delegate, limits::PUBLIC_KEY)?)?.into(),
                )
            } else {
                MinaBaseZkappPreconditionAccountStableV2Delegate::Ignore
            },
            state: PaddedSeq(
                state
                    .try_into()
                    .map_err(|_| ConversionError::InvalidLength)?,
            ),
            action_state: if let Some(action_state) = value.action_state {
                MinaBaseZkappPreconditionAccountStableV2StateA::Check(decimal_field(&action_state)?)
            } else {
                MinaBaseZkappPreconditionAccountStableV2StateA::Ignore
            },
            proved_state: if let Some(proved_state) = value.proved_state {
                MinaBaseZkappPreconditionAccountStableV2ProvedState::Check(proved_state)
            } else {
                MinaBaseZkappPreconditionAccountStableV2ProvedState::Ignore
            },
            is_new: if let Some(is_new) = value.is_new {
                MinaBaseZkappPreconditionAccountStableV2ProvedState::Check(is_new)
            } else {
                MinaBaseZkappPreconditionAccountStableV2ProvedState::Ignore
            },
        }))
    }
}
#[derive(GraphQLObject, Debug)]
pub struct GraphQLPreconditionsNetwork {
    pub snarked_ledger_hash: Option<String>,
    pub blockchain_length: Option<GraphQLPreconditionsNetworkBounds>,
    pub min_window_density: Option<GraphQLPreconditionsNetworkBounds>,
    pub total_currency: Option<GraphQLPreconditionsNetworkBounds>,
    pub global_slot_since_genesis: Option<GraphQLPreconditionsNetworkBounds>,
    pub staking_epoch_data: GraphQLPreconditionsNetworkEpochData,
    pub next_epoch_data: GraphQLPreconditionsNetworkEpochData,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLPreconditionsNetwork {
    pub snarked_ledger_hash: Option<String>,
    pub blockchain_length: Option<InputGraphQLPreconditionsNetworkBounds>,
    pub min_window_density: Option<InputGraphQLPreconditionsNetworkBounds>,
    pub total_currency: Option<InputGraphQLPreconditionsNetworkBounds>,
    pub global_slot_since_genesis: Option<InputGraphQLPreconditionsNetworkBounds>,
    pub staking_epoch_data: InputGraphQLPreconditionsNetworkEpochData,
    pub next_epoch_data: InputGraphQLPreconditionsNetworkEpochData,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLPreconditionsNetworkEpochData {
    pub ledger: GraphQLPreconditionsNetworkLedger,
    pub seed: Option<String>,
    pub start_checkpoint: Option<String>,
    pub lock_checkpoint: Option<String>,
    pub epoch_length: Option<GraphQLPreconditionsNetworkBounds>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLPreconditionsNetworkEpochData {
    pub ledger: InputGraphQLPreconditionsNetworkLedger,
    pub seed: Option<String>,
    pub start_checkpoint: Option<String>,
    pub lock_checkpoint: Option<String>,
    pub epoch_length: Option<InputGraphQLPreconditionsNetworkBounds>,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLPreconditionsNetworkLedger {
    pub hash: Option<String>,
    pub total_currency: Option<GraphQLPreconditionsNetworkBounds>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLPreconditionsNetworkLedger {
    pub hash: Option<String>,
    pub total_currency: Option<InputGraphQLPreconditionsNetworkBounds>,
}
#[derive(GraphQLObject, Debug)]
pub struct GraphQLPreconditionsNetworkBounds {
    pub upper: String,
    pub lower: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLPreconditionsNetworkBounds {
    pub upper: String,
    pub lower: String,
}

impl From<MinaBaseZkappPreconditionProtocolStateEpochDataStableV1>
    for GraphQLPreconditionsNetworkEpochData
{
    fn from(value: MinaBaseZkappPreconditionProtocolStateEpochDataStableV1) -> Self {
        Self {
            ledger: GraphQLPreconditionsNetworkLedger::from(value.ledger),
            seed: if let MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochSeed::Check(v) = value.seed {
                Some(v.to_string())
            } else {
                None
            },
            start_checkpoint: if let MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint::Check(v) = value.start_checkpoint {
                Some(v.to_string())
            } else {
                None
            },
            lock_checkpoint: if let MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint::Check(v) = value.lock_checkpoint {
                Some(v.to_string())
            } else {
                None
            },
            epoch_length: if let MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(v) = value.epoch_length {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u32().to_string(),
                    lower: v.lower.as_u32().to_string(),
                })
            } else {
                None
            },
        }
    }
}

impl TryFrom<InputGraphQLPreconditionsNetworkEpochData>
    for MinaBaseZkappPreconditionProtocolStateEpochDataStableV1
{
    type Error = ConversionError;
    fn try_from(value: InputGraphQLPreconditionsNetworkEpochData) -> Result<Self, Self::Error> {
        Ok(Self {
            ledger: value.ledger.try_into()?,
            seed: if let Some(seed) = value.seed {
                MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochSeed::Check(
                    bounded(&seed, limits::EPOCH_SEED)?.parse()?,
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochSeed::Ignore
            },
            start_checkpoint: if let Some(start_checkpoint) = value.start_checkpoint {
                MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint::Check(
                    bounded(&start_checkpoint, limits::STATE_HASH)?.parse()?,
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint::Ignore
            },
            lock_checkpoint: if let Some(lock_checkpoint) = value.lock_checkpoint {
                MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint::Check(
                    bounded(&lock_checkpoint, limits::STATE_HASH)?.parse()?,
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateEpochDataStableV1StartCheckpoint::Ignore
            },
            epoch_length: if let Some(epoch_length) = value.epoch_length {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1LengthA {
                        lower: (bounded(&epoch_length.lower, limits::U32_DECIMAL)?
                            .parse::<u32>()?)
                        .into(),
                        upper: (bounded(&epoch_length.upper, limits::U32_DECIMAL)?
                            .parse::<u32>()?)
                        .into(),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Ignore
            },
        })
    }
}
impl From<MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochLedger>
    for GraphQLPreconditionsNetworkLedger
{
    fn from(value: MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochLedger) -> Self {
        Self {
            hash: if let MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash::Check(v) =
                value.hash
            {
                Some(v.to_string())
            } else {
                None
            },
            total_currency: if let MinaBaseZkappPreconditionProtocolStateStableV1Amount::Check(v) =
                value.total_currency
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u64().to_string(),
                    lower: v.lower.as_u64().to_string(),
                })
            } else {
                None
            },
        }
    }
}

impl TryFrom<InputGraphQLPreconditionsNetworkLedger>
    for MinaBaseZkappPreconditionProtocolStateEpochDataStableV1EpochLedger
{
    type Error = ConversionError;
    fn try_from(value: InputGraphQLPreconditionsNetworkLedger) -> Result<Self, Self::Error> {
        Ok(Self {
            hash: if let Some(hash) = value.hash {
                MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash::Check(
                    bounded(&hash, limits::LEDGER_HASH)?.parse()?,
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash::Ignore
            },
            total_currency: if let Some(total_currency) = value.total_currency {
                MinaBaseZkappPreconditionProtocolStateStableV1Amount::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1AmountA {
                        lower: CurrencyAmountStableV1(
                            (bounded(&total_currency.lower, limits::U64_DECIMAL)?
                                .parse::<u64>()?)
                            .into(),
                        ),
                        upper: CurrencyAmountStableV1(
                            (bounded(&total_currency.upper, limits::U64_DECIMAL)?
                                .parse::<u64>()?)
                            .into(),
                        ),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1Amount::Ignore
            },
        })
    }
}

impl From<MinaBaseZkappPreconditionProtocolStateStableV1> for GraphQLPreconditionsNetwork {
    fn from(value: MinaBaseZkappPreconditionProtocolStateStableV1) -> Self {
        Self {
            snarked_ledger_hash:
                if let MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash::Check(v) =
                    value.snarked_ledger_hash
                {
                    Some(v.to_string())
                } else {
                    None
                },
            blockchain_length: if let MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(
                v,
            ) = value.blockchain_length
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u32().to_string(),
                    lower: v.lower.as_u32().to_string(),
                })
            } else {
                None
            },
            min_window_density: if let MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(
                v,
            ) = value.min_window_density
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u32().to_string(),
                    lower: v.lower.as_u32().to_string(),
                })
            } else {
                None
            },
            total_currency: if let MinaBaseZkappPreconditionProtocolStateStableV1Amount::Check(v) =
                value.total_currency
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u64().to_string(),
                    lower: v.lower.as_u64().to_string(),
                })
            } else {
                None
            },
            global_slot_since_genesis:
                if let MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot::Check(v) =
                    value.global_slot_since_genesis
                {
                    Some(GraphQLPreconditionsNetworkBounds {
                        upper: v.upper.as_u32().to_string(),
                        lower: v.lower.as_u32().to_string(),
                    })
                } else {
                    None
                },
            staking_epoch_data: GraphQLPreconditionsNetworkEpochData::from(
                value.staking_epoch_data,
            ),
            next_epoch_data: GraphQLPreconditionsNetworkEpochData::from(value.next_epoch_data),
        }
    }
}

impl TryFrom<InputGraphQLPreconditionsNetwork> for MinaBaseZkappPreconditionProtocolStateStableV1 {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLPreconditionsNetwork) -> Result<Self, Self::Error> {
        Ok(Self {
            snarked_ledger_hash: if let Some(snarked_ledger_hash) = value.snarked_ledger_hash {
                MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash::Check(
                    bounded(&snarked_ledger_hash, limits::LEDGER_HASH)?.parse()?,
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1SnarkedLedgerHash::Ignore
            },
            blockchain_length: if let Some(blockchain_length) = value.blockchain_length {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1LengthA {
                        lower: (bounded(&blockchain_length.lower, limits::U32_DECIMAL)?
                            .parse::<u32>()?)
                        .into(),
                        upper: (bounded(&blockchain_length.upper, limits::U32_DECIMAL)?
                            .parse::<u32>()?)
                        .into(),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Ignore
            },
            min_window_density: if let Some(min_window_density) = value.min_window_density {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1LengthA {
                        lower: (bounded(&min_window_density.lower, limits::U32_DECIMAL)?
                            .parse::<u32>()?)
                        .into(),
                        upper: (bounded(&min_window_density.upper, limits::U32_DECIMAL)?
                            .parse::<u32>()?)
                        .into(),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1Length::Ignore
            },
            total_currency: if let Some(total_currency) = value.total_currency {
                MinaBaseZkappPreconditionProtocolStateStableV1Amount::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1AmountA {
                        lower: CurrencyAmountStableV1(
                            (bounded(&total_currency.lower, limits::U64_DECIMAL)?
                                .parse::<u64>()?)
                            .into(),
                        ),
                        upper: CurrencyAmountStableV1(
                            (bounded(&total_currency.upper, limits::U64_DECIMAL)?
                                .parse::<u64>()?)
                            .into(),
                        ),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1Amount::Ignore
            },
            global_slot_since_genesis: if let Some(global_slot_since_genesis) =
                value.global_slot_since_genesis
            {
                MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlotA {
                        lower: MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
                            (bounded(&global_slot_since_genesis.lower, limits::U32_DECIMAL)?
                                .parse::<u32>()?)
                            .into(),
                        ),
                        upper: MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
                            (bounded(&global_slot_since_genesis.upper, limits::U32_DECIMAL)?
                                .parse::<u32>()?)
                            .into(),
                        ),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot::Ignore
            },
            staking_epoch_data: value.staking_epoch_data.try_into()?,
            next_epoch_data: value.next_epoch_data.try_into()?,
        })
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAccountUpdateUpdate {
    pub app_state: Vec<Option<String>>,
    pub delegate: Option<String>,
    pub verification_key: Option<GraphQLVerificationKey>,
    pub permissions: Option<GraphQLAccountUpdateUpdatePermissions>,
    pub zkapp_uri: Option<String>,
    pub token_symbol: Option<String>,
    pub timing: Option<GraphQLTiming>,
    pub voting_for: Option<String>,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLVerificationKey {
    pub data: String,
    pub hash: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAccountUpdateUpdate {
    pub app_state: Vec<Option<String>>,
    pub delegate: Option<String>,
    pub verification_key: Option<InputGraphQLVerificationKey>,
    pub permissions: Option<InputGraphQLAccountUpdateUpdatePermissions>,
    pub zkapp_uri: Option<String>,
    pub token_symbol: Option<String>,
    pub timing: Option<InputGraphQLTiming>,
    pub voting_for: Option<String>,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLVerificationKey {
    pub data: String,
    pub hash: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLAccountUpdateUpdatePermissions {
    pub edit_state: String,
    pub access: String,
    pub send: String,
    pub receive: String,
    pub set_delegate: String,
    pub set_permissions: String,
    pub set_verification_key: GraphQLSetVerificationKeyPermissions,
    pub set_zkapp_uri: String,
    pub edit_action_state: String,
    pub set_token_symbol: String,
    pub set_timing: String,
    pub set_voting_for: String,
    pub increment_nonce: String,
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLSetVerificationKeyPermissions {
    pub auth: String,
    pub txn_version: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLAccountUpdateUpdatePermissions {
    pub edit_state: String,
    pub access: String,
    pub send: String,
    pub receive: String,
    pub set_delegate: String,
    pub set_permissions: String,
    pub set_verification_key: InputGraphQLSetVerificationKeyPermissions,
    pub set_zkapp_uri: String,
    pub edit_action_state: String,
    pub set_token_symbol: String,
    pub set_timing: String,
    pub set_voting_for: String,
    pub increment_nonce: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLSetVerificationKeyPermissions {
    pub auth: String,
    pub txn_version: String,
}

impl From<MinaBasePermissionsStableV2> for GraphQLAccountUpdateUpdatePermissions {
    fn from(value: MinaBasePermissionsStableV2) -> Self {
        Self {
            edit_state: value.edit_state.to_string(),
            access: value.access.to_string(),
            send: value.send.to_string(),
            receive: value.receive.to_string(),
            set_delegate: value.set_delegate.to_string(),
            set_permissions: value.set_permissions.to_string(),
            set_verification_key: GraphQLSetVerificationKeyPermissions {
                auth: value.set_verification_key.0.to_string(),
                txn_version: value.set_verification_key.1.as_u32().to_string(),
            },
            set_zkapp_uri: value.set_zkapp_uri.to_string(),
            edit_action_state: value.edit_action_state.to_string(),
            set_token_symbol: value.set_token_symbol.to_string(),
            set_timing: value.set_timing.to_string(),
            set_voting_for: value.set_voting_for.to_string(),
            increment_nonce: value.increment_nonce.to_string(),
        }
    }
}

impl TryFrom<InputGraphQLAccountUpdateUpdatePermissions> for MinaBasePermissionsStableV2 {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLAccountUpdateUpdatePermissions) -> Result<Self, Self::Error> {
        Ok(Self {
            edit_state: bounded(&value.edit_state, limits::PERMISSION)?.parse()?,
            access: bounded(&value.access, limits::PERMISSION)?.parse()?,
            send: bounded(&value.send, limits::PERMISSION)?.parse()?,
            receive: bounded(&value.receive, limits::PERMISSION)?.parse()?,
            set_delegate: bounded(&value.set_delegate, limits::PERMISSION)?.parse()?,
            set_permissions: bounded(&value.set_permissions, limits::PERMISSION)?.parse()?,
            set_verification_key: (
                bounded(&value.set_verification_key.auth, limits::PERMISSION)?.parse()?,
                bounded(&value.set_verification_key.txn_version, limits::U32_DECIMAL)?
                    .parse::<u32>()?
                    .into(),
            ),
            set_zkapp_uri: bounded(&value.set_zkapp_uri, limits::PERMISSION)?.parse()?,
            edit_action_state: bounded(&value.edit_action_state, limits::PERMISSION)?.parse()?,
            set_token_symbol: bounded(&value.set_token_symbol, limits::PERMISSION)?.parse()?,
            set_timing: bounded(&value.set_timing, limits::PERMISSION)?.parse()?,
            set_voting_for: bounded(&value.set_voting_for, limits::PERMISSION)?.parse()?,
            increment_nonce: bounded(&value.increment_nonce, limits::PERMISSION)?.parse()?,
        })
    }
}

#[derive(GraphQLObject, Debug)]
pub struct GraphQLBalanceChange {
    pub magnitude: String,
    pub sgn: String,
}

#[derive(GraphQLInputObject, Debug)]
pub struct InputGraphQLBalanceChange {
    pub magnitude: String,
    pub sgn: String,
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L1738-L1753>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "ZkappCommandFailureReason")]
pub struct GraphQLFailureReason {
    /// List index of the account update that failed
    pub index: String,
    /// Failure reason for the account update or any nested zkapp command
    pub failures: Vec<String>,
}

impl GraphQLFailureReason {
    /// The failures of a zkApp command as OCaml displays them: one entry
    /// for each account update that failed, the fee payer being index 0,
    /// from the last to the first.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/transaction_status.ml#L82-L88>
    pub(crate) fn from_collection(
        failures: &MinaBaseTransactionStatusFailureCollectionStableV1,
    ) -> Vec<Self> {
        failures
            .0
            .iter()
            .enumerate()
            .filter(|(_, bucket)| !bucket.is_empty())
            .map(|(index, bucket)| Self {
                index: index.to_string(),
                failures: bucket
                    .iter()
                    .map(|failure| TransactionFailure::from(failure).to_string())
                    .collect(),
            })
            .rev()
            .collect()
    }
}

impl From<MinaStateBlockchainStateValueStableV2SignedAmount> for GraphQLBalanceChange {
    fn from(value: MinaStateBlockchainStateValueStableV2SignedAmount) -> Self {
        Self {
            magnitude: value.magnitude.as_u64().to_string(),
            sgn: value.sgn.to_string(),
        }
    }
}

impl TryFrom<InputGraphQLBalanceChange> for MinaStateBlockchainStateValueStableV2SignedAmount {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLBalanceChange) -> Result<Self, Self::Error> {
        Ok(Self {
            magnitude: CurrencyAmountStableV1(
                bounded(&value.magnitude, limits::U64_DECIMAL)?
                    .parse::<u64>()?
                    .into(),
            ),
            sgn: bounded(&value.sgn, limits::SIGN)?
                .parse()
                .map_err(|_| ConversionError::WrongVariant)?,
        })
    }
}

impl From<MinaBaseAccountUpdatePreconditionsStableV1> for GraphQLPreconditions {
    fn from(value: MinaBaseAccountUpdatePreconditionsStableV1) -> Self {
        Self {
            network: GraphQLPreconditionsNetwork::from(value.network),
            account: GraphQLPreconditionsAccount::from(value.account),
            valid_while: if let MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot::Check(v) =
                value.valid_while
            {
                Some(GraphQLPreconditionsNetworkBounds {
                    upper: v.upper.as_u32().to_string(),
                    lower: v.lower.as_u32().to_string(),
                })
            } else {
                None
            },
        }
    }
}

impl TryFrom<InputGraphQLPreconditions> for MinaBaseAccountUpdatePreconditionsStableV1 {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLPreconditions) -> Result<Self, Self::Error> {
        Ok(Self {
            network: value.network.try_into()?,
            account: value.account.try_into()?,
            valid_while: if let Some(v) = value.valid_while {
                MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot::Check(
                    MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlotA {
                        upper: MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
                            (bounded(&v.upper, limits::U32_DECIMAL)?.parse::<u32>()?).into(),
                        ),
                        lower: MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
                            (bounded(&v.lower, limits::U32_DECIMAL)?.parse::<u32>()?).into(),
                        ),
                    },
                )
            } else {
                MinaBaseZkappPreconditionProtocolStateStableV1GlobalSlot::Ignore
            },
        })
    }
}

impl TryFrom<MinaBaseVerificationKeyWireStableV1> for GraphQLVerificationKey {
    type Error = ConversionError;

    fn try_from(value: MinaBaseVerificationKeyWireStableV1) -> Result<Self, Self::Error> {
        Ok(Self {
            data: value.to_base64()?,
            hash: VerificationKey::try_from(&value)
                .map_err(|_| ConversionError::InvalidBigInt)?
                .hash()
                .to_decimal(),
        })
    }
}

impl TryFrom<InputGraphQLVerificationKey> for MinaBaseVerificationKeyWireStableV1 {
    type Error = ConversionError;

    fn try_from(value: InputGraphQLVerificationKey) -> Result<Self, Self::Error> {
        Ok(Self::from_base64(bounded(
            &value.data,
            limits::VERIFICATION_KEY,
        )?)?)
    }
}

impl TryFrom<MinaBaseAccountUpdateUpdateStableV1> for GraphQLAccountUpdateUpdate {
    type Error = ConversionError;

    fn try_from(value: MinaBaseAccountUpdateUpdateStableV1) -> Result<Self, Self::Error> {
        Ok(Self {
            app_state: value
                .app_state
                .0
                .into_iter()
                .map(|v| {
                    if let MinaBaseAccountUpdateUpdateStableV1AppStateA::Set(value) = v {
                        Some(value.to_decimal())
                    } else {
                        None
                    }
                })
                .collect(),
            delegate: if let MinaBaseAccountUpdateUpdateStableV1Delegate::Set(v) = value.delegate {
                Some(v.to_string())
            } else {
                None
            },
            verification_key: if let MinaBaseAccountUpdateUpdateStableV1VerificationKey::Set(v) =
                value.verification_key
            {
                Some(GraphQLVerificationKey::try_from(*v)?)
            } else {
                None
            },
            permissions: if let MinaBaseAccountUpdateUpdateStableV1Permissions::Set(v) =
                value.permissions
            {
                Some(GraphQLAccountUpdateUpdatePermissions::from(*v))
            } else {
                None
            },
            zkapp_uri: if let MinaBaseAccountUpdateUpdateStableV1ZkappUri::Set(v) = value.zkapp_uri
            {
                Some(v.to_string())
            } else {
                None
            },
            token_symbol: if let MinaBaseAccountUpdateUpdateStableV1TokenSymbol::Set(v) =
                value.token_symbol
            {
                Some(v.to_string())
            } else {
                None
            },
            timing: if let MinaBaseAccountUpdateUpdateStableV1Timing::Set(v) = value.timing {
                Some(GraphQLTiming::from(*v))
            } else {
                None
            },
            voting_for: if let MinaBaseAccountUpdateUpdateStableV1VotingFor::Set(v) =
                value.voting_for
            {
                Some(v.to_string())
            } else {
                None
            },
        })
    }
}

impl TryFrom<InputGraphQLAccountUpdateUpdate> for MinaBaseAccountUpdateUpdateStableV1 {
    type Error = ConversionError;
    fn try_from(value: InputGraphQLAccountUpdateUpdate) -> Result<Self, Self::Error> {
        let app_state: Vec<_> = value
            .app_state
            .iter()
            .map(|v| {
                if let Some(v) = v {
                    Ok(MinaBaseAccountUpdateUpdateStableV1AppStateA::Set(
                        decimal_field(v)?,
                    ))
                } else {
                    Ok(MinaBaseAccountUpdateUpdateStableV1AppStateA::Keep)
                }
            })
            .collect::<Result<Vec<_>, ConversionError>>()?;
        Ok(Self {
            app_state: PaddedSeq(
                app_state
                    .try_into()
                    .map_err(|_| ConversionError::InvalidLength)?,
            ),
            delegate: if let Some(delegate) = value.delegate {
                MinaBaseAccountUpdateUpdateStableV1Delegate::Set(
                    AccountPublicKey::from_str(bounded(&delegate, limits::PUBLIC_KEY)?)?.into(),
                )
            } else {
                MinaBaseAccountUpdateUpdateStableV1Delegate::Keep
            },
            verification_key: if let Some(vk) = value.verification_key {
                MinaBaseAccountUpdateUpdateStableV1VerificationKey::Set(Box::new(
                    MinaBaseVerificationKeyWireStableV1::try_from(vk)?,
                ))
            } else {
                MinaBaseAccountUpdateUpdateStableV1VerificationKey::Keep
            },
            permissions: if let Some(permissions) = value.permissions {
                MinaBaseAccountUpdateUpdateStableV1Permissions::Set(Box::new(
                    MinaBasePermissionsStableV2::try_from(permissions)?,
                ))
            } else {
                MinaBaseAccountUpdateUpdateStableV1Permissions::Keep
            },
            zkapp_uri: if let Some(zkapp_uri) = value.zkapp_uri {
                MinaBaseAccountUpdateUpdateStableV1ZkappUri::Set(ZkAppUri::from(bounded(
                    &zkapp_uri,
                    mina_p2p_messages::string::ZKAPP_URI_MAX_LENGTH,
                )?))
            } else {
                MinaBaseAccountUpdateUpdateStableV1ZkappUri::Keep
            },
            token_symbol: if let Some(token_symbol) = value.token_symbol {
                MinaBaseAccountUpdateUpdateStableV1TokenSymbol::Set(TokenSymbol::from(bounded(
                    &token_symbol,
                    mina_p2p_messages::string::TOKEN_SYMBOL_MAX_LENGTH,
                )?))
            } else {
                MinaBaseAccountUpdateUpdateStableV1TokenSymbol::Keep
            },
            timing: if let Some(timing) = value.timing {
                MinaBaseAccountUpdateUpdateStableV1Timing::Set(Box::new(
                    MinaBaseAccountUpdateUpdateTimingInfoStableV1::try_from(timing)?,
                ))
            } else {
                MinaBaseAccountUpdateUpdateStableV1Timing::Keep
            },
            voting_for: if let Some(voting_for) = value.voting_for {
                MinaBaseAccountUpdateUpdateStableV1VotingFor::Set(StateHash::from_str(bounded(
                    &voting_for,
                    limits::STATE_HASH,
                )?)?)
            } else {
                MinaBaseAccountUpdateUpdateStableV1VotingFor::Keep
            },
        })
    }
}

impl From<MinaBaseAccountUpdateFeePayerStableV1> for GraphQLFeePayer {
    fn from(value: MinaBaseAccountUpdateFeePayerStableV1) -> Self {
        Self {
            authorization: value.authorization.to_string(),
            body: GraphQLFeePayerBody {
                public_key: value.body.public_key.to_string(),
                fee: value.body.fee.as_u64().to_string(),
                valid_until: value.body.valid_until.map(|v| v.as_u32().to_string()),
                nonce: value.body.nonce.to_string(),
            },
        }
    }
}

impl TryFrom<InputGraphQLFeePayer> for MinaBaseAccountUpdateFeePayerStableV1 {
    type Error = ConversionError;

    fn try_from(value: InputGraphQLFeePayer) -> Result<Self, Self::Error> {
        Ok(Self {
            authorization: bounded(&value.authorization, limits::SIGNATURE)?.parse()?,
            body: MinaBaseAccountUpdateBodyFeePayerStableV1 {
                public_key: bounded(&value.body.public_key, limits::PUBLIC_KEY)?.parse()?,
                fee: CurrencyFeeStableV1(
                    bounded(&value.body.fee, limits::U64_DECIMAL)?
                        .parse::<u64>()?
                        .into(),
                ),
                valid_until: value
                    .body
                    .valid_until
                    .map(|v| -> Result<_, ConversionError> {
                        Ok(MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(
                            bounded(&v, limits::U32_DECIMAL)?.parse::<u32>()?.into(),
                        ))
                    })
                    .transpose()?,
                nonce: bounded(&value.body.nonce, limits::U32_DECIMAL)?
                    .parse::<u32>()?
                    .into(),
            },
        })
    }
}

impl TryFrom<MinaBaseAccountUpdateTStableV1> for GraphQLAccountUpdate {
    type Error = ConversionError;

    fn try_from(value: MinaBaseAccountUpdateTStableV1) -> Result<Self, Self::Error> {
        Ok(Self {
            body: GraphQLAccountUpdateBody {
                public_key: value.body.public_key.to_string(),
                token_id: value.body.token_id.to_string(),
                use_full_commitment: value.body.use_full_commitment,
                increment_nonce: value.body.increment_nonce,
                update: GraphQLAccountUpdateUpdate::try_from(value.body.update)?,
                balance_change: GraphQLBalanceChange::from(value.body.balance_change),
                events: value
                    .body
                    .events
                    .0
                    .into_iter()
                    .map(|v| v.into_iter().map(|i| i.to_decimal()).collect())
                    .collect(),
                actions: value
                    .body
                    .actions
                    .0
                    .into_iter()
                    .map(|v| v.into_iter().map(|i| i.to_decimal()).collect())
                    .collect(),
                call_data: value.body.call_data.to_decimal(),
                // Set by `flatten_account_updates`, which knows the depth.
                call_depth: 0,
                preconditions: GraphQLPreconditions::from(value.body.preconditions),
                may_use_token: GraphQLMayUseToken::from(value.body.may_use_token),
                authorization_kind: GraphQLAuthorizationKind::from(value.body.authorization_kind),
                implicit_account_creation_fee: value.body.implicit_account_creation_fee,
            },
            authorization: GraphQLAuthorization::try_from(value.authorization)?,
        })
    }
}

impl TryFrom<InputGraphQLAccountUpdate> for MinaBaseAccountUpdateTStableV1 {
    type Error = ConversionError;

    fn try_from(value: InputGraphQLAccountUpdate) -> Result<Self, Self::Error> {
        Ok(Self {
            body: MinaBaseAccountUpdateBodyStableV1 {
                public_key: bounded(&value.body.public_key, limits::PUBLIC_KEY)?.parse()?,
                token_id: bounded(&value.body.token_id, limits::TOKEN_ID)?.parse()?,
                update: value.body.update.try_into()?,
                balance_change: value.body.balance_change.try_into()?,
                increment_nonce: value.body.increment_nonce,
                events: MinaBaseAccountUpdateBodyEventsStableV1(
                    value
                        .body
                        .events
                        .into_iter()
                        .map(|v| {
                            v.into_iter()
                                .map(|i| decimal_field(&i).map_err(ConversionError::from))
                                .collect::<Result<_, _>>()
                        })
                        .collect::<Result<_, _>>()?,
                ),
                actions: MinaBaseAccountUpdateBodyEventsStableV1(
                    value
                        .body
                        .actions
                        .into_iter()
                        .map(|v| {
                            v.into_iter()
                                .map(|i| decimal_field(&i).map_err(ConversionError::from))
                                .collect::<Result<_, _>>()
                        })
                        .collect::<Result<_, _>>()?,
                ),
                call_data: decimal_field(&value.body.call_data)?,
                preconditions: value.body.preconditions.try_into()?,
                use_full_commitment: value.body.use_full_commitment,
                implicit_account_creation_fee: value.body.implicit_account_creation_fee,
                may_use_token: value.body.may_use_token.into(),
                authorization_kind: value.body.authorization_kind.try_into()?,
            },
            authorization: value.authorization.try_into()?,
        })
    }
}

impl TryFrom<InputGraphQLTiming> for MinaBaseAccountUpdateUpdateTimingInfoStableV1 {
    type Error = ConversionError;

    fn try_from(value: InputGraphQLTiming) -> Result<Self, Self::Error> {
        let cliff_time: u32 = value.cliff_time.try_into()?;
        let vesting_period: u32 = value.vesting_period.try_into()?;
        Ok(Self {
            initial_minimum_balance: CurrencyBalanceStableV1(CurrencyAmountStableV1(
                bounded(&value.initial_minimum_balance, limits::U64_DECIMAL)?
                    .parse::<u64>()?
                    .into(),
            )),
            cliff_time: MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis(cliff_time.into()),
            cliff_amount: CurrencyAmountStableV1(
                bounded(&value.cliff_amount, limits::U64_DECIMAL)?
                    .parse::<u64>()?
                    .into(),
            ),
            vesting_period: MinaNumbersGlobalSlotSpanStableV1::GlobalSlotSpan(
                vesting_period.into(),
            ),
            vesting_increment: CurrencyAmountStableV1(
                bounded(&value.vesting_increment, limits::U64_DECIMAL)?
                    .parse::<u64>()?
                    .into(),
            ),
        })
    }
}

#[cfg(test)]
mod test {
    use std::str::FromStr;

    use mina_p2p_messages::{
        binprot::BinProtRead,
        v2::{
            MinaBaseSignedCommandMemoStableV1, MinaBaseUserCommandStableV2,
            MinaBaseZkappCommandTStableV1WireStableV1,
        },
    };

    use super::*;

    #[test]
    fn verification_key_and_authorization_input_caps() {
        // Existing OCaml deployment fixture: fixed 1796-byte wire key.
        let command = create_input_graphql_zkapp().zkapp_command;
        let key = command
            .account_updates
            .into_iter()
            .find_map(|a| a.body.update.verification_key)
            .unwrap();
        assert_eq!(key.data.len(), limits::VERIFICATION_KEY);
        MinaBaseVerificationKeyWireStableV1::try_from(key).unwrap();
        for len in [limits::VERIFICATION_KEY + 1, 65_000] {
            let key = InputGraphQLVerificationKey {
                data: "z".repeat(len),
                hash: "0".into(),
            };
            assert!(matches!(
                MinaBaseVerificationKeyWireStableV1::try_from(key),
                Err(ConversionError::InputTooLong(_))
            ));
        }
        for len in [limits::FIELD_DECIMAL + 1, 65_000] {
            let auth = InputGraphQLAuthorizationKind {
                is_signed: false,
                is_proved: true,
                verification_key_hash: Some("9".repeat(len)),
            };
            assert!(matches!(
                MinaBaseAccountUpdateAuthorizationKindStableV1::try_from(auth),
                Err(ConversionError::InputTooLong(_))
            ));
        }
        let auth = InputGraphQLAuthorization {
            signature: None,
            proof: Some("z".repeat(limits::OPAQUE_BASE64 + 1)),
        };
        assert!(matches!(
            MinaBaseControlStableV2::try_from(auth),
            Err(ConversionError::InputTooLong(_))
        ));
    }

    #[test]
    fn test_empty_memo() {
        use ledger::scan_state::transaction_logic::Memo;

        let expected = "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH";
        let empty_memo = Memo::from_str("").unwrap();
        let mina_empty_memo = MinaBaseSignedCommandMemoStableV1::from(&empty_memo);
        assert_eq!(mina_empty_memo.to_base58check(), expected);
        let empty_memo = Memo::empty();
        let mina_empty_memo = MinaBaseSignedCommandMemoStableV1::from(&empty_memo);
        assert_eq!(mina_empty_memo.to_base58check(), expected);
    }

    #[test]
    fn test_parse_zkapp_bug_1464_no_proof_nor_signature() {
        let input = create_input_graphql_zkapp_bug_1464();
        let converted: MinaBaseUserCommandStableV2 = input
            .zkapp_command
            .try_into()
            .expect("Failed to parse input");
        let MinaBaseUserCommandStableV2::ZkappCommand(zkapp) = converted else {
            panic!("expected a zkApp command");
        };
        assert_eq!(zkapp.account_updates.len(), 1);
        let update = &zkapp
            .account_updates
            .iter()
            .next()
            .unwrap()
            .elt
            .account_update;
        assert_eq!(update.authorization, MinaBaseControlStableV2::NoneGiven);
        assert_eq!(
            update.body.authorization_kind,
            MinaBaseAccountUpdateAuthorizationKindStableV1::NoneGiven
        );
    }

    #[test]
    fn test_zkapp_from_input() {
        use juniper::FromInputValue;

        // Keep the deployment fixture's verification-key and permissions coverage.
        // Only its four vectors were widened; this is a parsing fixture, not a
        // claim that the historical signatures/proof key are valid on Mesa.
        // See private implementation notes for the independent migration.
        let deployment = include_bytes!("../../../../../tests/files/zkapps/valid_zkapp.bin");
        // Unmodified OCaml 4.0.2 GraphQL input and transaction-id bytes from
        // mainnet block 564394. This also covers signed/unsigned nested calls.
        let mesa = include_bytes!("../../../../../tests/files/zkapps/with_sig_auth.bin");
        let mesa_input: juniper::InputValue = serde_json::from_str(include_str!(
            "../../../../../tests/files/zkapps/with_sig_auth_graphql.json"
        ))
        .unwrap();
        let mesa_input = InputGraphQLZkappCommand::from_input_value(&mesa_input).unwrap();

        for (bytes, input) in [
            (
                deployment.as_slice(),
                create_input_graphql_zkapp().zkapp_command,
            ),
            (mesa.as_slice(), mesa_input),
        ] {
            let mut remaining = bytes;
            let expected =
                MinaBaseZkappCommandTStableV1WireStableV1::binprot_read(&mut remaining).unwrap();
            assert!(remaining.is_empty(), "fixture must decode completely");
            let converted: MinaBaseUserCommandStableV2 = input.try_into().unwrap();
            let MinaBaseUserCommandStableV2::ZkappCommand(converted) = converted else {
                panic!("expected a zkApp command");
            };
            assert_eq!(expected, converted);
        }
    }

    #[test]
    fn test_authorization_kind() {
        let kind = InputGraphQLAuthorizationKind {
            is_signed: false,
            is_proved: true,
            verification_key_hash: Some(
                "19951435866906059835892103359374709356309230417850637795098911039647240505427"
                    .to_string(),
            ),
        };
        let converted: Result<MinaBaseAccountUpdateAuthorizationKindStableV1, ConversionError> =
            kind.try_into();

        assert!(converted.is_ok());
    }

    #[test]
    fn test_authorization_proof() {
        let proof = InputGraphQLAuthorization {
            signature: None,
            proof: Some(
                include_str!("../../../../../tests/files/zkapps/proof_string.txt").to_string(),
            ),
        };
        let converted: Result<MinaBaseControlStableV2, _> = proof.try_into();
        assert!(converted.is_ok());

        let proof = InputGraphQLAuthorization {
            signature: None,
            proof: None,
        };
        let converted: MinaBaseControlStableV2 =
            proof.try_into().expect("Error parsing None given auth");

        assert_eq!(converted, MinaBaseControlStableV2::NoneGiven);
    }

    // Mesa uses 32 app-state and account-state precondition fields.
    // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_state.ml#L1-L7>
    fn create_input_graphql_zkapp() -> InputGraphQLZkapp {
        InputGraphQLZkapp {
            zkapp_command: InputGraphQLZkappCommand {
                memo: Some("E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH".to_string()),
                fee_payer: InputGraphQLFeePayer {
                    body: InputGraphQLFeePayerBody {
                        public_key: "B62qpD75xH5R19wxZG2uz8whNsHPTioVoYcPV3zfjjSbzTmaHQHKKEV".to_string(),
                        fee: "117000000".to_string(),
                        valid_until: None,
                        nonce: "1128".to_string(),
                    },
                    authorization: "7mX5Lwu2bdnJPc4DJu7CkwTSR5behoKH8yZh7myCGgYfib5Sq3dfgPQY6LcXdrpQma1NvoLC5i7HLFEQZTnkBFcn96TP57JF".to_string(),
                },
                account_updates: vec![
                    InputGraphQLAccountUpdate {
                        body: InputGraphQLAccountUpdateBody {
                            call_depth: 0,
                            public_key: "B62qpD75xH5R19wxZG2uz8whNsHPTioVoYcPV3zfjjSbzTmaHQHKKEV".to_string(),
                            token_id: "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf".to_string(),
                            update: InputGraphQLAccountUpdateUpdate {
                                app_state: vec![None; 32],
                                delegate: None,
                                verification_key: None,
                                permissions: None,
                                zkapp_uri: None,
                                token_symbol: None,
                                timing: None,
                                voting_for: None,
                            },
                            balance_change: InputGraphQLBalanceChange {
                                magnitude: "1000000000".to_string(),
                                sgn: "Negative".to_string(),
                            },
                            increment_nonce: false,
                            events: vec![],
                            actions: vec![],
                            call_data: "0".to_string(),
                            preconditions: InputGraphQLPreconditions {
                                network: InputGraphQLPreconditionsNetwork {
                                    snarked_ledger_hash: None,
                                    blockchain_length: None,
                                    min_window_density: None,
                                    total_currency: None,
                                    global_slot_since_genesis: None,
                                    staking_epoch_data: InputGraphQLPreconditionsNetworkEpochData {
                                        ledger: InputGraphQLPreconditionsNetworkLedger {
                                            hash: None,
                                            total_currency: None,
                                        },
                                        seed: None,
                                        start_checkpoint: None,
                                        lock_checkpoint: None,
                                        epoch_length: None,
                                    },
                                    next_epoch_data: InputGraphQLPreconditionsNetworkEpochData {
                                        ledger: InputGraphQLPreconditionsNetworkLedger {
                                            hash: None,
                                            total_currency: None,
                                        },
                                        seed: None,
                                        start_checkpoint: None,
                                        lock_checkpoint: None,
                                        epoch_length: None,
                                    },
                                },
                                account: InputGraphQLPreconditionsAccount {
                                    balance: None,
                                    nonce: None,
                                    receipt_chain_hash: None,
                                    delegate: None,
                                    state: vec![None; 32],
                                    action_state: None,
                                    proved_state: None,
                                    is_new: None,
                                },
                                valid_while: None,
                            },
                            use_full_commitment: true,
                            implicit_account_creation_fee: false,
                            may_use_token: InputGraphQLMayUseToken {
                                parents_own_token: false,
                                inherit_from_parent: false,
                            },
                            authorization_kind: InputGraphQLAuthorizationKind {
                                is_signed: true,
                                is_proved: false,
                                verification_key_hash: None,
                            },
                        },
                        authorization: InputGraphQLAuthorization {
                            proof: None,
                            signature: Some("7mX5Lwu2bdnJPc4DJu7CkwTSR5behoKH8yZh7myCGgYfib5Sq3dfgPQY6LcXdrpQma1NvoLC5i7HLFEQZTnkBFcn96TP57JF".to_string()),
                        },
                    },
                    InputGraphQLAccountUpdate {
                        body: InputGraphQLAccountUpdateBody {
                            call_depth: 0,
                            public_key: "B62qqKAQh8M61uvuw3tjJsmRgsEvzRm84Nc9MwXTF3zoqFRZ86rV8qk".to_string(),
                            token_id: "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf".to_string(),
                            update: InputGraphQLAccountUpdateUpdate {
                                app_state: vec![
                                    Some("1".to_string()),
                                    Some("0".to_string()),
                                    Some("0".to_string()),
                                    Some("0".to_string()),
                                    Some("0".to_string()),
                                    Some("0".to_string()),
                                    Some("0".to_string()),
                                    Some("0".to_string()),
                                ].into_iter().chain(vec![None; 24]).collect(),
                                delegate: None,
                                verification_key: Some(InputGraphQLVerificationKey {
                                    data: "AACcenc1yLdGBm4xtUN1dpModROI0zovuy5rz2a94vfdBgG1C75BqviU4vw6JUYqODF8n9ivtfeU5s9PcpEGIP0htil2mfx8v2DB5RuNQ7VxJWkha0TSnJJsOl0FxhjldBbOY3tUZzZxHpPhHOKHz/ZAXRYFIsf2x+7boXC0iPurEX9VcnaJIq+YxxmnSfeYYxHkjxO9lrDBqjXzd5AHMnYyjTPC69B+5In7AOGS6R+A/g3/aR/MKDa4eDVrnsF9Oy/Ay8ahic2sSAZvtn08MdRyk/jm2cLlJbeAAad6Xyz/H9l7JrkbVwDMMPxvHVHs27tNoJCzIlrRzB7pg3ju9aQOu4h3thDr+WSgFQWKvcRPeL7f3TFjIr8WZ2457RgMcTwXwORKbqJCcyKVNOE+FlNwVkOKER+WIpC0OlgGuayPFwQQkbb91jaRlJvahfwkbF2+AJmDnavmNpop9T+/Xak1adXIrsRPeOjC+qIKxIbGimoMOoYzYlevKA80LnJ7HC0IxR+yNLvoSYxDDPNRD+OCCxk5lM2h8IDUiCNWH4FZNJ+doiigKjyZlu/xZ7jHcX7qibu/32KFTX85DPSkQM8dAEkH+vlkHmyXGLF4+xOVKeM0ihV5OEQrOABcgfTkbRsyxNInUBh0WiQyALE2ctjvkRCiE2P24bjFA8SgFmTM7gAKR89XcqLS/NP7lwCEej/L8q8R7sKGMCXmgFYluWH4JBSPDgvMxScfjFS33oBNb7po8cLnAORzohXoYTSgztklD0mKn6EegLbkLtwwr9ObsLz3m7fp/3wkNWFRkY5xzSZN1VybbQbmpyQNCpxd/kdDsvlszqlowkyC8HnKbhnvE0Mrz3ZIk4vSs/UGBSXAoESFCFCPcTq11TCOhE5rumMJErv5LusDHJgrBtQUMibLU9A1YbF7SPDAR2QZd0yx3waAC2F3xF+U682SOKF7oCZl2OICysRHqH+rZ604UfdGG0zWRuP2yg6kfGwcGQbO1ql40WrWTiFhbxxdKC7Gbz4y9Sb7q5EsPt6Z1AIn34/nXB/IWfC0gg/OgfPQTR7uxiTo2OOwjHni1f4KhT4rEmDAQn6ty6/ZRKHPWjUaAREbEw3tC36fI09hCYjjVTEmMAFTApk/tMUu0tC9Dt/vfDgXAlDJBwN5Y2Pt60qWY92skizVcWyWBxp5A8e4cVu3iToxOGUbSHzawovjubcH7qWjIZoghZJ16QB1c0ryiAfHB48OHhs2p/JZWz8Dp7kfcPkeg2Of2NbupJlNVMLIH4IGWaPAscBRkZ+F4oLqOhJ5as7fAzzU8PQdeZi0YgssGDJVmNEHP61I16KZNcxQqR0EUVwhyMmYmpVjvtfhHi/6I3TgYCmfnm6GL2sN144vMWg/gJ+p9a4GcEA0+gK3oCcKcwkq5rm+1Oxo9LWLp92Bdxq3iqfoIFmJ/ANGSbHF8StVmlVsP8zA+xuHylyiww/Lercce7cq0YA5PtYS3ge9IDYwXckBUXb5ikD3alrrv5mvMu6itB7ix2f8lbiF9Fkmc4Bk2ycIWXJDCuBN+2sTFqzUeoT6xY8XWaOcnDvqOgSm/CCSv38umiOE2jEpsKYxhRc6W70UJkrzd3hr2DiSF1I2B+krpUVK1GeOdCLC5sl7YPzk+pF8183uI9wse6UTlqIiroKqsggzLBy/IjAfxS0BxFy5zywXqp+NogFkoTEJmR5MaqOkPfap+OsD1lGScY6+X4WW/HqCWrmA3ZTqDGngQMTGXLCtl6IS/cQpihS1NRbNqOtKTaCB9COQu0oz6RivBlywuaj3MKUdmbQ2gVDj+SGQItCNaXawyPSBjB9VT+68SoJVySQsYPCuEZCb0V/40n/a7RAbyrnNjP+2HwD7p27Pl1RSzqq35xiPdnycD1UeEPLpx/ON65mYCkn+KLQZmkqPio+vA2KmJngWTx+ol4rVFimGm76VT0xCFDsu2K0YX0yoLNH4u2XfmT9NR8gGfkVRCnnNjlbgHQmEwC75+GmEJ5DjD3d+s6IXTQ60MHvxbTHHlnfmPbgKn2SAI0uVoewKC9GyK6dSaboLw3C48jl0E2kyc+7umhCk3kEeWmt//GSjRNhoq+B+mynXiOtgFs/Am2v1TBjSb+6tcijsf5tFJmeGxlCjJnTdNWBkSHpMoo6OFkkpA6/FBAUHLSM7Yv8oYyd0GtwF5cCwQ6aRTbl9oG/mUn5Q92OnDMQcUjpgEho0Dcp2OqZyyxqQSPrbIIZZQrS2HkxBgjcfcSTuSHo7ONqlRjLUpO5yS95VLGXBLLHuCiIMGT+DW6DoJRtRIS+JieVWBoX0YsWgYInXrVlWUv6gDng5AyVFkUIFwZk7/3mVAgvXO83ArVKA4S747jT60w5bgV4Jy55slDM=".to_string(),
                                    hash: "11640126627177324946637007967436400725357874055180801746732941023691529117236".to_string(),
                                }),
                                permissions: Some(InputGraphQLAccountUpdateUpdatePermissions {
                                    edit_state: "Proof".to_string(),
                                    access: "Proof".to_string(),
                                    send: "Proof".to_string(),
                                    receive: "Proof".to_string(),
                                    set_delegate: "Proof".to_string(),
                                    set_permissions: "Proof".to_string(),
                                    set_verification_key: InputGraphQLSetVerificationKeyPermissions {
                                        auth: "Proof".to_string(),
                                        txn_version: "3".to_string(),
                                    },
                                    set_zkapp_uri: "Proof".to_string(),
                                    edit_action_state: "Proof".to_string(),
                                    set_token_symbol: "Proof".to_string(),
                                    set_timing: "Proof".to_string(),
                                    set_voting_for: "Proof".to_string(),
                                    increment_nonce: "Proof".to_string(),
                                }),
                                zkapp_uri: None,
                                token_symbol: None,
                                timing: None,
                                voting_for: None,
                            },
                            balance_change: InputGraphQLBalanceChange {
                                magnitude: "0".to_string(),
                                sgn: "Positive".to_string(),
                            },
                            increment_nonce: true,
                            events: vec![],
                            actions: vec![],
                            call_data: "0".to_string(),
                            preconditions: InputGraphQLPreconditions {
                                network: InputGraphQLPreconditionsNetwork {
                                    snarked_ledger_hash: None,
                                    blockchain_length: None,
                                    min_window_density: None,
                                    total_currency: None,
                                    global_slot_since_genesis: None,
                                    staking_epoch_data: InputGraphQLPreconditionsNetworkEpochData {
                                        ledger: InputGraphQLPreconditionsNetworkLedger {
                                            hash: None,
                                            total_currency: None,
                                        },
                                        seed: None,
                                        start_checkpoint: None,
                                        lock_checkpoint: None,
                                        epoch_length: None,
                                    },
                                    next_epoch_data: InputGraphQLPreconditionsNetworkEpochData {
                                        ledger: InputGraphQLPreconditionsNetworkLedger {
                                            hash: None,
                                            total_currency: None,
                                        },
                                        seed: None,
                                        start_checkpoint: None,
                                        lock_checkpoint: None,
                                        epoch_length: None,
                                    },
                                },
                                account: InputGraphQLPreconditionsAccount {
                                    balance: None,
                                    nonce: Some(InputGraphQLPreconditionsNetworkBounds {
                                        upper: "0".to_string(),
                                        lower: "0".to_string(),
                                    }),
                                    receipt_chain_hash: None,
                                    delegate: None,
                                    state: vec![None; 32],
                                    action_state: None,
                                    proved_state: Some(false),
                                    is_new: None,
                                },
                                valid_while: None,
                            },
                            use_full_commitment: false,
                            implicit_account_creation_fee: false,
                            may_use_token: InputGraphQLMayUseToken {
                                parents_own_token: false,
                                inherit_from_parent: false,
                            },
                            authorization_kind: InputGraphQLAuthorizationKind {
                                is_signed: true,
                                is_proved: false,
                                verification_key_hash: None,
                            },
                        },
                        authorization: InputGraphQLAuthorization {
                            proof: None,
                            signature: Some("7mXFnDxZBE5iXBfw9LRPXST3sSodXAdTJSWFqX3hBoDA3wv5s2s9TLMDCXgatMvMH4bDttAFyJuezWmbSA81FXeMFZgqcxtt".to_string()),
                        },
                    },
                ],
            },
        }
    }

    fn create_input_graphql_zkapp_bug_1464() -> InputGraphQLZkapp {
        InputGraphQLZkapp {
            zkapp_command: InputGraphQLZkappCommand {
                memo: Some("E4YdMZb5VHHE51HPFPs1zUHnyus1q7RW61tWq1F4FNYrp5FKtKZMh".to_string()),
                fee_payer: InputGraphQLFeePayer {
                    body: InputGraphQLFeePayerBody {
                        public_key: "B62qmGcEhZsZ2EMh2EzRXDJEmaD9PUJcW5xdNq2im2xNVuX2bsEPjoj".to_string(),
                        fee: "200000000".to_string(),
                        valid_until: None,
                        nonce: "250".to_string(),
                    },
                    authorization: "7mXMa44TWfDGtQmsfGtkRogZVzGVYqosP6qkXCBApsjoZ2ye2Y5AL2VWzp7m5CxkTjZhcoqtDVJkiQRm38tAmQdhpTTsSEr6".to_string(),
                },
                account_updates: vec![
                    InputGraphQLAccountUpdate {
                        body: InputGraphQLAccountUpdateBody {
                            call_depth: 0,
                            public_key: "B62qmGcEhZsZ2EMh2EzRXDJEmaD9PUJcW5xdNq2im2xNVuX2bsEPjoj".to_string(),
                            token_id: "wSHV2S4qX9jFsLjQo8r1BsMLH2ZRKsZx6EJd1sbozGPieEC4Jf".to_string(),
                            update: InputGraphQLAccountUpdateUpdate {
                                app_state: vec![None; 32],
                                delegate: None,
                                verification_key: None,
                                permissions: None,
                                zkapp_uri: None,
                                token_symbol: None,
                                timing: None,
                                voting_for: None,
                            },
                            balance_change: InputGraphQLBalanceChange {
                                magnitude: "0".to_string(),
                                sgn: "Positive".to_string(),
                            },
                            increment_nonce: false,
                            events: vec![vec![
                        "852".to_owned(),
                        "993".to_owned(),
                        "540".to_owned(),
                        "951".to_owned(),
                        "340".to_owned()
                            ]],
                            actions: vec![],
                            call_data: "0".to_string(),
                            preconditions: InputGraphQLPreconditions {
                                network: InputGraphQLPreconditionsNetwork {
                                    snarked_ledger_hash: None,
                                    blockchain_length: None,
                                    min_window_density: None,
                                    total_currency: None,
                                    global_slot_since_genesis: None,
                                    staking_epoch_data: InputGraphQLPreconditionsNetworkEpochData {
                                        ledger: InputGraphQLPreconditionsNetworkLedger {
                                            hash: None,
                                            total_currency: None,
                                        },
                                        seed: None,
                                        start_checkpoint: None,
                                        lock_checkpoint: None,
                                        epoch_length: None,
                                    },
                                    next_epoch_data: InputGraphQLPreconditionsNetworkEpochData {
                                        ledger: InputGraphQLPreconditionsNetworkLedger {
                                            hash: None,
                                            total_currency: None,
                                        },
                                        seed: None,
                                        start_checkpoint: None,
                                        lock_checkpoint: None,
                                        epoch_length: None,
                                    },
                                },
                                account: InputGraphQLPreconditionsAccount {
                                    balance: None,
                                    nonce: None,
                                    receipt_chain_hash: None,
                                    delegate: None,
                                    state: vec![None; 32],
                                    action_state: None,
                                    proved_state: None,
                                    is_new: None,
                                },
                                valid_while: None,
                            },
                            use_full_commitment: false,
                            implicit_account_creation_fee: false,
                            may_use_token: InputGraphQLMayUseToken {
                                parents_own_token: false,
                                inherit_from_parent: false,
                            },
                            authorization_kind: InputGraphQLAuthorizationKind {
                                is_signed: false,
                                is_proved: false,
                                verification_key_hash: Some("3392518251768960475377392625298437850623664973002200885669375116181514017494".parse().unwrap()),
                            },
                        },
                        authorization: InputGraphQLAuthorization {
                            proof: None,
                            signature: None
                        },
                    },
                ],
            },
        }
    }

    #[test]
    pub fn test_bigint_to_decimal() {
        let bigint = decimal_field("1").unwrap();
        let decimal = serde_json::to_string(&bigint).unwrap();
        assert_eq!(
            decimal,
            "\"0x0000000000000000000000000000000000000000000000000000000000000001\"".to_string()
        );
    }
}
