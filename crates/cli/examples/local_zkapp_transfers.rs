//! P5.2e: signature-only zkApp transfers for a private test network.
//!
//! Builds `COUNT` zkApp commands from the account of the encrypted key at
//! `KEY_PATH` (password in `PASSWORD_PATH`, never printed), nonces `NONCE`
//! on: a fee payer and a signed debit of 1 MINA, both signing the full
//! commitment, and an unauthorised credit of 1 MINA to `RECIPIENT`, with an
//! empty memo; P4.15's command shape (`SS S`: two zkApp segments and a
//! merge). Each signature is checked with the ledger's verifier. Writes one
//! GraphQL `sendZkapp` request per nonce to `OUT_DIR/zkapp-<nonce>.json` and
//! prints each command's nonce and hash. Never submits anything.
//!
//! Usage: `local_zkapp_transfers NETWORK KEY_PATH PASSWORD_PATH RECIPIENT
//! NONCE COUNT OUT_DIR [AMOUNT]`, with `NETWORK` the profile whose signature
//! kind the network uses (`devnet` signs for testnets) and `AMOUNT` the
//! transfer in nanomina (default 1 MINA). A transfer larger than the payer's
//! balance makes a command that is valid but fails when applied: the fee is
//! charged and the account updates are not applied.

use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use ledger::{
    scan_state::{
        currency::{Amount, Fee, Nonce, Sgn, Signed},
        transaction_logic::{
            verifiable,
            zkapp_command::{
                Account as UpdateAccount, AccountPreconditions, AccountUpdate, Actions,
                AuthorizationKind, Body, CallForest, Control, Events, FeePayer, FeePayerBody,
                MayUseToken, Numeric, Preconditions, Tree, Update, WithStackHash, ZkAppCommand,
                ZkAppPreconditions,
            },
            zkapp_statement::TransactionCommitment,
            Memo, WithStatus,
        },
    },
    verifier::common::{self, CheckResult},
    MutableFp, TokenId,
};
use mina_core::NetworkConfig;
use mina_node_account::{AccountPublicKey, AccountSecretKey};
use mina_p2p_messages::v2::{
    MinaBaseSignatureStableV1, MinaBaseZkappCommandTStableV1WireStableV1, TokenIdKeyHash,
};
use mina_signer::{BaseField, Keypair, NetworkId, NonceMode, PubKey, Signature, Signer};
use serde_json::{json, Value};

const TRANSFER: u64 = 1_000_000_000;
const FEE: u64 = 10_000_000;
/// OCaml's `Zkapp_account.dummy_vk_hash`, which OCaml's GraphQL requires
/// for an account update not authorised by a proof.
const DUMMY_VK_HASH: &str =
    "3392518251768960475377392625298437850623664973002200885669375116181514017494";

fn account_update(
    pk: mina_signer::CompressedPubKey,
    amount: Signed<Amount>,
    signed: bool,
) -> AccountUpdate {
    AccountUpdate {
        body: Body {
            public_key: pk,
            token_id: TokenId::default(),
            update: Update::noop(),
            balance_change: amount,
            increment_nonce: false,
            events: Events::empty(),
            actions: Actions::empty(),
            call_data: BaseField::from(0u64),
            preconditions: Preconditions {
                network: ZkAppPreconditions::accept(),
                account: AccountPreconditions(UpdateAccount::accept()),
                valid_while: Numeric::Ignore,
            },
            use_full_commitment: signed,
            implicit_account_creation_fee: false,
            may_use_token: MayUseToken::No,
            authorization_kind: if signed {
                AuthorizationKind::Signature
            } else {
                AuthorizationKind::NoneGiven
            },
        },
        authorization: if signed {
            Control::Signature(Signature::dummy())
        } else {
            Control::NoneGiven
        },
    }
}

fn tree(update: AccountUpdate) -> WithStackHash<AccountUpdate> {
    WithStackHash {
        elt: Tree {
            account_update: update,
            account_update_digest: MutableFp::empty(),
            calls: CallForest::new(),
        },
        stack_hash: MutableFp::empty(),
    }
}

/// An account update as OCaml's and our GraphQL read it, every optional
/// field given as null.
fn graphql_update(update: &AccountUpdate, signature: Option<String>) -> Value {
    let b = &update.body;
    let epoch = json!({"ledger": {"hash": null, "totalCurrency": null},
        "seed": null, "startCheckpoint": null, "lockCheckpoint": null, "epochLength": null});
    let state = vec![None::<String>; ledger::ZKAPP_STATE_SIZE];
    json!({
        "body": {
            "publicKey": AccountPublicKey::from(b.public_key.clone()).to_string(),
            "tokenId": TokenIdKeyHash::from(&b.token_id).to_string(),
            "update": {"appState": state, "delegate": null, "verificationKey": null,
                "permissions": null, "zkappUri": null, "tokenSymbol": null, "timing": null,
                "votingFor": null},
            "balanceChange": {"magnitude": b.balance_change.magnitude.as_u64().to_string(),
                "sgn": match b.balance_change.sgn { Sgn::Pos => "Positive", Sgn::Neg => "Negative" }},
            "incrementNonce": false,
            "events": [], "actions": [], "callData": "0", "callDepth": 0,
            "preconditions": {
                "network": {"snarkedLedgerHash": null, "blockchainLength": null,
                    "minWindowDensity": null, "totalCurrency": null,
                    "globalSlotSinceGenesis": null, "stakingEpochData": epoch,
                    "nextEpochData": epoch},
                "account": {"balance": null, "nonce": null, "receiptChainHash": null,
                    "delegate": null, "state": state, "actionState": null,
                    "provedState": null, "isNew": null},
                "validWhile": null
            },
            "useFullCommitment": b.use_full_commitment,
            "implicitAccountCreationFee": false,
            "mayUseToken": {"parentsOwnToken": false, "inheritFromParent": false},
            "authorizationKind": {"isSigned": signature.is_some(), "isProved": false,
                "verificationKeyHash": DUMMY_VK_HASH}
        },
        "authorization": {"proof": null, "signature": signature}
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let (network, key_path, password_path, recipient, nonce, count, out_dir, amount) = match &args[..]
    {
        [_, network, key, password, recipient, nonce, count, out_dir] => (
            network, key, password, recipient, nonce, count, out_dir, TRANSFER,
        ),
        [_, network, key, password, recipient, nonce, count, out_dir, amount] => (
            network,
            key,
            password,
            recipient,
            nonce,
            count,
            out_dir,
            amount.parse().context("AMOUNT")?,
        ),
        _ => bail!(
            "usage: local_zkapp_transfers NETWORK KEY_PATH PASSWORD_PATH RECIPIENT NONCE \
                 COUNT OUT_DIR [AMOUNT]"
        ),
    };
    NetworkConfig::init(network).map_err(anyhow::Error::msg)?;
    let network_id = match network.as_str() {
        "mainnet" => NetworkId::MAINNET,
        _ => NetworkId::TESTNET,
    };
    let first_nonce: u32 = nonce.parse().context("NONCE")?;
    let count: u32 = count.parse().context("COUNT")?;
    let password = fs::read_to_string(password_path)?;
    let key =
        AccountSecretKey::from_encrypted_file(key_path, password.trim_end_matches(['\r', '\n']))?;
    drop(password);
    let kp: Keypair = key.clone().into();
    let sender_pk = key.public_key_compressed();
    let sender = AccountPublicKey::from(sender_pk.clone()).to_string();
    let recipient_pk = PubKey::from_address(recipient)?.into_compressed();
    let out_dir = Path::new(out_dir);
    fs::create_dir_all(out_dir)?;

    for nonce in first_nonce..first_nonce + count {
        let debit = account_update(
            sender_pk.clone(),
            Signed {
                magnitude: Amount::from_u64(amount),
                sgn: Sgn::Neg,
            },
            true,
        );
        let credit = account_update(
            recipient_pk.clone(),
            Signed {
                magnitude: Amount::from_u64(amount),
                sgn: Sgn::Pos,
            },
            false,
        );
        let forest = CallForest(vec![tree(debit), tree(credit)]);
        forest.ensure_hashed();
        let mut command = ZkAppCommand {
            fee_payer: FeePayer {
                body: FeePayerBody {
                    public_key: sender_pk.clone(),
                    fee: Fee::from_u64(FEE),
                    valid_until: None,
                    nonce: Nonce::from_u32(nonce),
                },
                authorization: Signature::dummy(),
            },
            account_updates: forest,
            memo: Memo::empty(),
        };
        let commitment = TransactionCommitment::create(command.account_updates_hash());
        let full_commitment = commitment.create_complete(
            command.memo.hash(),
            AccountUpdate::of_fee_payer(command.fee_payer.clone()).digest(),
        );
        let mut signer = mina_signer::create_kimchi::<TransactionCommitment>(network_id.clone());
        let signature = signer.sign(&kp, &full_commitment, NonceMode::Chunked);
        if !signer.verify(&signature, &kp.public, &full_commitment) {
            bail!("nonce {nonce}: the signature does not verify");
        }
        command.fee_payer.authorization = signature.clone();
        command.account_updates.0[0]
            .elt
            .account_update
            .authorization = Control::Signature(signature.clone());
        let checked = ledger::scan_state::transaction_logic::zkapp_command::verifiable::create(
            &command,
            false,
            |_, _| Err("unexpected proof".into()),
        )
        .map_err(anyhow::Error::msg)?;
        match common::check(WithStatus::applied(verifiable::UserCommand::ZkAppCommand(
            Box::new(checked),
        ))) {
            CheckResult::Valid(_) => {}
            other => bail!("nonce {nonce}: the ledger's verifier refused it: {other:?}"),
        }

        let wire: MinaBaseZkappCommandTStableV1WireStableV1 = (&command).into();
        let hash = wire.hash()?;
        let signature =
            mina_p2p_messages::v2::Signature::from(MinaBaseSignatureStableV1::from(&signature))
                .to_string();
        let request = json!({
            "query": "mutation SendZkapp($input: SendZkappInput!) { sendZkapp(input: $input) { zkapp { hash id } } }",
            "variables": {"input": {"zkappCommand": {
                "memo": mina_p2p_messages::v2::MinaBaseSignedCommandMemoStableV1::from(&command.memo).to_base58check(),
                "feePayer": {"body": {"publicKey": sender, "fee": FEE.to_string(),
                    "validUntil": null, "nonce": nonce.to_string()},
                    "authorization": signature.clone()},
                "accountUpdates": [
                    graphql_update(&command.account_updates.0[0].elt.account_update, Some(signature)),
                    graphql_update(&command.account_updates.0[1].elt.account_update, None),
                ]
            }}}
        });
        let path = out_dir.join(format!("zkapp-{nonce}.json"));
        fs::write(&path, serde_json::to_vec_pretty(&request)?)?;
        println!("{nonce} {hash} {}", path.display());
    }
    Ok(())
}
