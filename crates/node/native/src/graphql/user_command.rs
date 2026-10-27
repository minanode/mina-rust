use crate::input_limits::{self as limits, bounded};
use std::str::FromStr;

use juniper::{GraphQLInputObject, GraphQLObject};
use ledger::scan_state::{
    currency::{Amount, Fee, Nonce, Slot},
    transaction_logic::{signed_command, Memo},
};
use mina_node::account::AccountPublicKey;
use mina_p2p_messages::{bigint::BigInt, v2};
use mina_signer::CompressedPubKey;
use o1_utils::field_helpers::FieldHelpers;

use super::{
    block::GraphQLUserCommands,
    scalars::{PublicKeyScalar, UInt32Scalar, UInt64Scalar},
};

/// OCaml's `SendPaymentInput`, with its names and scalars, so that clients'
/// operations validate.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2992-L3022>
#[derive(GraphQLInputObject, Debug)]
#[graphql(name = "SendPaymentInput")]
pub struct InputGraphQLPayment {
    pub from: PublicKeyScalar,
    pub to: PublicKeyScalar,
    pub amount: UInt64Scalar,
    pub valid_until: Option<UInt32Scalar>,
    pub fee: UInt64Scalar,
    pub memo: Option<String>,
    pub nonce: Option<UInt32Scalar>,
}

/// OCaml's `SendDelegationInput`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L3055-L3083>
#[derive(GraphQLInputObject, Debug)]
#[graphql(name = "SendDelegationInput")]
pub struct InputGraphQLDelegation {
    pub from: PublicKeyScalar,
    pub to: PublicKeyScalar,
    pub valid_until: Option<UInt32Scalar>,
    pub fee: UInt64Scalar,
    pub memo: Option<String>,
    pub nonce: Option<UInt32Scalar>,
}

/// OCaml's `SignatureInput`: `field` and `scalar`, or `rawSignature`.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2799-L2838>
#[derive(GraphQLInputObject, Debug, Clone)]
#[graphql(name = "SignatureInput")]
pub struct UserCommandSignature {
    pub field: Option<String>,
    pub scalar: Option<String>,
    // Note: either raw_signature or scalar + field must be provided
    pub raw_signature: Option<String>,
}

impl TryFrom<UserCommandSignature> for mina_signer::Signature {
    type Error = super::ConversionError;

    fn try_from(value: UserCommandSignature) -> Result<Self, Self::Error> {
        let UserCommandSignature {
            field,
            scalar,
            raw_signature,
        } = value;

        if let Some(raw_signature) = raw_signature {
            bounded(&raw_signature, limits::SIGNATURE_HEX)?;
            if raw_signature.len() != limits::SIGNATURE_HEX || !raw_signature.is_ascii() {
                return Err(super::ConversionError::InvalidLength);
            }
            let sig_parts_len = raw_signature
                .len()
                .checked_div(2)
                .ok_or(super::ConversionError::InvalidLength)?;
            let (rx_hex, s_hex) = raw_signature.split_at(sig_parts_len);

            let rx_bytes = hex::decode(rx_hex).map_err(|_| super::ConversionError::InvalidHex)?;
            let s_bytes = hex::decode(s_hex).map_err(|_| super::ConversionError::InvalidHex)?;

            let rx = mina_signer::BaseField::from_bytes(&rx_bytes)?;
            let s = mina_signer::ScalarField::from_bytes(&s_bytes)?;

            Ok(Self { rx, s })
        } else if let (Some(field), Some(scalar)) = (field, scalar) {
            let sig = Self {
                rx: BigInt::from_decimal(bounded(&field, limits::FIELD_DECIMAL)?)?
                    .try_into()
                    .map_err(|_| super::ConversionError::InvalidBigInt)?,
                s: BigInt::from_decimal(bounded(&scalar, limits::FIELD_DECIMAL)?)?
                    .try_into()
                    .map_err(|_| super::ConversionError::InvalidBigInt)?,
            };

            Ok(sig)
        } else {
            Err(super::ConversionError::MissingField(
                "Either raw_signature or scalar + field must be provided".to_string(),
            ))
        }
    }
}

impl TryFrom<&UserCommandSignature> for mina_signer::Signature {
    type Error = super::ConversionError;

    fn try_from(value: &UserCommandSignature) -> Result<Self, Self::Error> {
        value.clone().try_into()
    }
}

impl UserCommandSignature {
    pub fn validate(&self) -> Result<(), super::Error> {
        if self.raw_signature.is_some() || (self.scalar.is_some() && self.field.is_some()) {
            Ok(())
        } else {
            Err(super::Error::Custom(
                "Either raw_signature or scalar + field must be provided".to_string(),
            ))
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L2339-L2355>
#[derive(GraphQLObject, Debug)]
#[graphql(name = "SendPaymentPayload", context = super::Context)]
pub(crate) struct GraphQLSendPaymentResponse {
    pub payment: GraphQLUserCommands,
}

#[derive(GraphQLObject, Debug)]
#[graphql(name = "SendDelegationPayload", context = super::Context)]
pub(crate) struct GraphQLSendDelegationResponse {
    pub delegation: GraphQLUserCommands,
}

impl TryFrom<v2::MinaBaseUserCommandStableV2> for GraphQLSendPaymentResponse {
    type Error = super::ConversionError;
    fn try_from(value: v2::MinaBaseUserCommandStableV2) -> Result<Self, Self::Error> {
        match value {
            v2::MinaBaseUserCommandStableV2::SignedCommand(command)
                if matches!(
                    command.payload.body,
                    v2::MinaBaseSignedCommandPayloadBodyStableV2::Payment(_)
                ) =>
            {
                Ok(Self {
                    payment: GraphQLUserCommands::new(&command, None)?,
                })
            }
            _ => Err(super::ConversionError::WrongVariant),
        }
    }
}

impl TryFrom<v2::MinaBaseUserCommandStableV2> for GraphQLSendDelegationResponse {
    type Error = super::ConversionError;
    fn try_from(value: v2::MinaBaseUserCommandStableV2) -> Result<Self, Self::Error> {
        match value {
            v2::MinaBaseUserCommandStableV2::SignedCommand(command)
                if matches!(
                    command.payload.body,
                    v2::MinaBaseSignedCommandPayloadBodyStableV2::StakeDelegation(_)
                ) =>
            {
                Ok(Self {
                    delegation: GraphQLUserCommands::new(&command, None)?,
                })
            }
            _ => Err(super::ConversionError::WrongVariant),
        }
    }
}

/// The minimum user command fee on mainnet and devnet: 0.001 MINA. P6h moves
/// it into the constants.
///
/// OCaml: `genesis_constants.minimum_user_command_fee`, from
/// <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml>
pub const MINIMUM_USER_COMMAND_FEE: u64 = 1_000_000;

/// An amount in nanomina as OCaml prints it: whole MINA, then the decimals
/// without trailing zeros.
///
/// OCaml: `Currency.to_mina_string`, <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L82-L96>
pub fn to_mina_string(amount: u64) -> String {
    const PRECISION: usize = 9;
    const EXP: u64 = 1_000_000_000;
    let whole = amount / EXP;
    let mut remainder = amount % EXP;
    if remainder == 0 {
        return whole.to_string();
    }
    let mut stripped = 0;
    while remainder % 10 == 0 {
        remainder /= 10;
        stripped += 1;
    }
    format!("{whole}.{remainder:0width$}", width = PRECISION - stripped)
}

/// How OCaml words an error from building a command (nonce, signature): its
/// `to_user_command` and `send_user_command` wrap the reason twice. OCaml also
/// puts the input as JSON before `Error:`; we don't.
pub fn creation_error(reason: &str) -> String {
    format!("Couldn't send user command: Error creating user command: Error: {reason}")
}

/// OCaml's message when neither the pool nor the ledger knows the sender.
pub const CANNOT_INFER_NONCE: &str = "Couldn't infer nonce for transaction from specified \
     `sender` since `sender` is not in the ledger or sent a transaction in transaction pool.";

/// The nonce OCaml would use: the given one if `min <= nonce <= inferred`, the
/// inferred one if none is given. `min` is the ledger nonce, `inferred` the
/// nonce after the sender's last pooled command, else the ledger nonce.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/user_command_input/user_command_input.ml#L28-L55>
pub fn choose_nonce(input: Option<Nonce>, min: Nonce, inferred: Nonce) -> Result<Nonce, String> {
    match input {
        None => Ok(inferred),
        Some(nonce) if min <= nonce && nonce <= inferred => Ok(nonce),
        Some(nonce) => Err(format!(
            "Input nonce {} either different from inferred nonce {} or below minimum_nonce {}",
            nonce.as_u32(),
            inferred.as_u32(),
            min.as_u32()
        )),
    }
}

/// What `sendPayment` and `sendDelegation` share, checked in OCaml's order:
/// the fee, then the memo.
///
/// OCaml: `create_user_command_input`, <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/mina_graphql.ml#L507-L538>
#[derive(Debug, Clone)]
pub struct SignedCommandInput {
    pub from: AccountPublicKey,
    pub fee: Fee,
    pub memo: Memo,
    pub valid_until: Option<Slot>,
    pub nonce: Option<Nonce>,
}

fn parse_u32(value: &str, name: &str) -> Result<u32, String> {
    bounded(value, limits::U32_DECIMAL)
        .map_err(|_| format!("Invalid `{name}` provided."))?
        .parse::<u32>()
        .map_err(|_| format!("Invalid `{name}` provided."))
}

fn parse_public_key(value: &str, name: &str) -> Result<AccountPublicKey, String> {
    AccountPublicKey::from_str(
        bounded(value, limits::PUBLIC_KEY).map_err(|_| format!("Invalid `{name}` provided."))?,
    )
    .map_err(|_| format!("Invalid `{name}` provided."))
}

pub fn compressed(key: &AccountPublicKey) -> Result<CompressedPubKey, String> {
    key.clone()
        .try_into()
        .map_err(|_| "Invalid public key".to_owned())
}

impl SignedCommandInput {
    pub fn parse(
        from: &str,
        fee: &str,
        memo: Option<&str>,
        valid_until: Option<&str>,
        nonce: Option<&str>,
    ) -> Result<Self, String> {
        let fee = bounded(fee, limits::U64_DECIMAL)
            .map_err(|_| "Invalid `fee` provided.".to_owned())?
            .parse::<u64>()
            .map_err(|_| "Invalid `fee` provided.".to_owned())?;
        if fee < MINIMUM_USER_COMMAND_FEE {
            // OCaml: "Do not change the content of this error without updating
            // Rosetta's construction API".
            return Err(format!(
                "Invalid user command. Fee {} is less than the minimum fee, {}.",
                to_mina_string(fee),
                to_mina_string(MINIMUM_USER_COMMAND_FEE)
            ));
        }
        let memo = match memo {
            Some(memo) => Memo::create_from_str_checked(memo)
                .map_err(|_| "Invalid `memo` provided.".to_owned())?,
            None => Memo::empty(),
        };
        Ok(Self {
            from: parse_public_key(from, "from")?,
            fee: Fee::from_u64(fee),
            memo,
            valid_until: valid_until
                .map(|v| parse_u32(v, "validUntil").map(Slot::from_u32))
                .transpose()?,
            nonce: nonce
                .map(|n| parse_u32(n, "nonce").map(Nonce::from_u32))
                .transpose()?,
        })
    }

    /// The signed command for `nonce`, unchecked.
    pub fn command(
        &self,
        nonce: Nonce,
        body: signed_command::Body,
        signature: mina_signer::Signature,
    ) -> Result<signed_command::SignedCommand, String> {
        let from = compressed(&self.from)?;
        Ok(signed_command::SignedCommand {
            payload: signed_command::SignedCommandPayload::create(
                self.fee,
                from.clone(),
                nonce,
                self.valid_until,
                self.memo.clone(),
                body,
            ),
            signer: from,
            signature,
        })
    }
}

/// Checks the signature for this node's network, as OCaml does before the
/// pool: `Invalid_signature` otherwise.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/user_command_input/user_command_input.ml#L136-L151>
pub fn check_signature(command: &signed_command::SignedCommand) -> Result<(), String> {
    use ledger::scan_state::transaction_logic::verifiable;
    if command.check_valid_keys()
        && verifiable::check_only_for_signature(Box::new(command.clone())).is_ok()
    {
        Ok(())
    } else {
        Err("Invalid_signature".to_owned())
    }
}

impl InputGraphQLPayment {
    pub fn parse(&self) -> Result<(SignedCommandInput, signed_command::Body), String> {
        let common = SignedCommandInput::parse(
            &self.from.0,
            &self.fee.0,
            self.memo.as_deref(),
            self.valid_until.as_ref().map(|v| v.0.as_str()),
            self.nonce.as_ref().map(|n| n.0.as_str()),
        )?;
        let receiver_pk = compressed(&parse_public_key(&self.to.0, "to")?)?;
        let amount = bounded(&self.amount.0, limits::U64_DECIMAL)
            .map_err(|_| "Invalid `amount` provided.".to_owned())?
            .parse::<u64>()
            .map_err(|_| "Invalid `amount` provided.".to_owned())?;
        let body = signed_command::Body::Payment(signed_command::PaymentPayload {
            receiver_pk,
            amount: Amount::from_u64(amount),
        });
        Ok((common, body))
    }
}

impl InputGraphQLDelegation {
    pub fn parse(&self) -> Result<(SignedCommandInput, signed_command::Body), String> {
        let common = SignedCommandInput::parse(
            &self.from.0,
            &self.fee.0,
            self.memo.as_deref(),
            self.valid_until.as_ref().map(|v| v.0.as_str()),
            self.nonce.as_ref().map(|n| n.0.as_str()),
        )?;
        let new_delegate = compressed(&parse_public_key(&self.to.0, "to")?)?;
        let body = signed_command::Body::StakeDelegation(
            signed_command::StakeDelegationPayload::SetDelegate { new_delegate },
        );
        Ok((common, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mina_strings_as_ocaml() {
        assert_eq!(to_mina_string(1_000_000), "0.001");
        assert_eq!(to_mina_string(100_000), "0.0001");
        assert_eq!(to_mina_string(1_000_000_000), "1");
        assert_eq!(to_mina_string(1_500_000_000), "1.5");
        assert_eq!(to_mina_string(0), "0");
        assert_eq!(to_mina_string(1), "0.000000001");
        assert_eq!(to_mina_string(12_345_678_900), "12.3456789");
    }

    #[test]
    fn nonces_as_ocaml() {
        let n = Nonce::from_u32;
        // Omitted: the inferred one, even for a fresh account (0).
        assert_eq!(choose_nonce(None, n(0), n(0)), Ok(n(0)));
        assert_eq!(choose_nonce(None, n(3), n(5)), Ok(n(5)));
        // Explicit: from the ledger nonce to the inferred one.
        assert_eq!(choose_nonce(Some(n(0)), n(0), n(0)), Ok(n(0)));
        assert_eq!(choose_nonce(Some(n(3)), n(3), n(5)), Ok(n(3)));
        assert_eq!(choose_nonce(Some(n(5)), n(3), n(5)), Ok(n(5)));
        assert_eq!(
            choose_nonce(Some(n(6)), n(3), n(5)),
            Err(
                "Input nonce 6 either different from inferred nonce 5 or below minimum_nonce 3"
                    .to_owned()
            )
        );
        assert!(choose_nonce(Some(n(2)), n(3), n(5)).is_err());
    }

    #[test]
    fn fee_and_memo_checked_first() {
        let pk = "B62qrPN5Y5yq8kGE3FbVKbGTdTAJNdtNtB5sNVpxyRwWGcDEhpMzc8g";
        assert_eq!(
            SignedCommandInput::parse(pk, "100000", None, None, None).unwrap_err(),
            "Invalid user command. Fee 0.0001 is less than the minimum fee, 0.001."
        );
        assert_eq!(
            SignedCommandInput::parse(pk, "1000000", Some(&"x".repeat(33)), None, None)
                .unwrap_err(),
            "Invalid `memo` provided."
        );
        assert!(
            SignedCommandInput::parse(pk, "1000000", Some(&"x".repeat(32)), None, None).is_ok()
        );
    }
}
