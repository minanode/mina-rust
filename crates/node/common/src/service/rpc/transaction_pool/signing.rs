//! Signing shared by the native tests and the page's WASM adapter. No secret
//! enters an RPC request, Redux action, log, or serialized result.

use ledger::{
    scan_state::{
        currency::{Amount, Fee, Nonce, Sgn, Signed, Slot},
        transaction_logic::{
            signed_command::{self, SignedCommand, SignedCommandPayload},
            transaction_union_payload::TransactionUnionPayload,
            zkapp_command::{
                Account, AccountPreconditions, AccountUpdate, Actions, AuthorizationKind, Body,
                CallForest, Control, Events, FeePayer, FeePayerBody, MayUseToken, Numeric,
                Preconditions, Tree, Update, WithStackHash, ZkAppCommand, ZkAppPreconditions,
            },
            zkapp_statement::TransactionCommitment,
            Memo,
        },
    },
    MutableFp, TokenId,
};
use mina_core::{network::NetworkId as NodeNetworkId, NetworkConfig};
use mina_node::account::{AccountPublicKey, AccountSecretKey};
use mina_p2p_messages::v2;
use mina_signer::{BaseField, CompressedPubKey, Keypair, NetworkId, NonceMode, Signature, Signer};
use serde::Deserialize;

/// Nanomina amounts are decimal strings so JavaScript never rounds a u64.
/// The network is deliberately not a caller-supplied field.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transfer {
    pub from: AccountPublicKey,
    pub to: AccountPublicKey,
    pub amount: String,
    pub fee: String,
    pub nonce: u32,
    pub memo: String,
    pub valid_until: Option<u32>,
}

fn network_id() -> NetworkId {
    match NetworkConfig::global().network_id {
        NodeNetworkId::MAINNET => NetworkId::MAINNET,
        NodeNetworkId::TESTNET => NetworkId::TESTNET,
    }
}

impl Transfer {
    fn values(
        &self,
        key: &AccountSecretKey,
    ) -> Result<(CompressedPubKey, CompressedPubKey, Amount, Fee, Memo), String> {
        if key.public_key() != self.from {
            return Err("signing key does not match from".into());
        }
        let recipient = self
            .to
            .clone()
            .try_into()
            .map_err(|_| "invalid recipient")?;
        let amount = self.amount.parse::<u64>().map_err(|_| "invalid amount")?;
        let fee = self.fee.parse::<u64>().map_err(|_| "invalid fee")?;
        amount.checked_add(fee).ok_or("amount plus fee overflows")?;
        let memo = Memo::create_from_str_checked(&self.memo).map_err(|e| e.to_string())?;
        Ok((
            key.public_key_compressed(),
            recipient,
            Amount::from_u64(amount),
            Fee::from_u64(fee),
            memo,
        ))
    }
}

/// Same legacy payload, signer and nonce derivation as `mina wallet send`.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/signed_command.ml>
pub fn payment(
    key: &AccountSecretKey,
    transfer: &Transfer,
) -> Result<v2::MinaBaseUserCommandStableV2, String> {
    let (sender, recipient, amount, fee, memo) = transfer.values(key)?;
    let payload = SignedCommandPayload::create(
        fee,
        sender.clone(),
        Nonce::from_u32(transfer.nonce),
        transfer.valid_until.map(Slot::from_u32),
        memo,
        signed_command::Body::Payment(signed_command::PaymentPayload {
            receiver_pk: recipient,
            amount,
        }),
    );
    let message = TransactionUnionPayload::of_user_command_payload(&payload);
    let kp: Keypair = key.clone().into();
    let signature = mina_signer::create_legacy(network_id()).sign(&kp, &message, NonceMode::Legacy);
    Ok(v2::MinaBaseUserCommandStableV2::SignedCommand(
        SignedCommand {
            payload,
            signer: sender,
            signature,
        }
        .into(),
    ))
}

fn account_update(
    public_key: CompressedPubKey,
    amount: Amount,
    debit: bool,
) -> WithStackHash<AccountUpdate> {
    WithStackHash {
        elt: Tree {
            account_update: AccountUpdate {
                body: Body {
                    public_key,
                    token_id: TokenId::default(),
                    update: Update::noop(),
                    balance_change: Signed {
                        magnitude: amount,
                        sgn: if debit { Sgn::Neg } else { Sgn::Pos },
                    },
                    increment_nonce: false,
                    events: Events::empty(),
                    actions: Actions::empty(),
                    call_data: BaseField::from(0u64),
                    preconditions: Preconditions {
                        network: ZkAppPreconditions::accept(),
                        account: AccountPreconditions(Account::accept()),
                        valid_while: Numeric::Ignore,
                    },
                    use_full_commitment: debit,
                    implicit_account_creation_fee: false,
                    may_use_token: MayUseToken::No,
                    authorization_kind: if debit {
                        AuthorizationKind::Signature
                    } else {
                        AuthorizationKind::NoneGiven
                    },
                },
                authorization: if debit {
                    Control::Signature(Signature::dummy())
                } else {
                    Control::NoneGiven
                },
            },
            // Empty, not zero: these caches must be computed from the bodies.
            account_update_digest: MutableFp::empty(),
            calls: CallForest::new(),
        },
        stack_hash: MutableFp::empty(),
    }
}

/// P4.15's two-root transfer: the fee payer and debit sign the full commitment;
/// the credit requires an existing receiver whose receive permission is None.
/// No proofs, account creation, custom tokens or state updates are constructed.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_command.ml>
pub fn zkapp_transfer(
    key: &AccountSecretKey,
    transfer: &Transfer,
) -> Result<v2::MinaBaseUserCommandStableV2, String> {
    let (sender, recipient, amount, fee, memo) = transfer.values(key)?;
    let mut forest = CallForest(vec![
        account_update(sender.clone(), amount, true),
        account_update(recipient, amount, false),
    ]);
    forest.ensure_hashed();
    let mut command = ZkAppCommand {
        fee_payer: FeePayer {
            body: FeePayerBody {
                public_key: sender,
                fee,
                valid_until: transfer.valid_until.map(Slot::from_u32),
                nonce: Nonce::from_u32(transfer.nonce),
            },
            authorization: Signature::dummy(),
        },
        account_updates: forest,
        memo,
    };
    let commitment = TransactionCommitment::create(command.account_updates_hash());
    let full_commitment = commitment.create_complete(
        command.memo.hash(),
        AccountUpdate::of_fee_payer(command.fee_payer.clone()).digest(),
    );
    let kp: Keypair = key.clone().into();
    let signature =
        mina_signer::create_kimchi(network_id()).sign(&kp, &full_commitment, NonceMode::Chunked);
    command.fee_payer.authorization = signature.clone();
    command.account_updates.0[0]
        .elt
        .account_update
        .authorization = Control::Signature(signature);
    Ok(v2::MinaBaseUserCommandStableV2::ZkappCommand(
        (&command).into(),
    ))
}
