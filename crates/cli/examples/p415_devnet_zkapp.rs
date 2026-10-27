//! Build and validate P4.15's signature-only devnet zkApp transfer.
//!
//! Run with the encrypted key path, password file path, and an output path
//! under `.work/`. This writes a GraphQL request but never submits it.

use std::{fs, fs::OpenOptions, os::unix::fs::OpenOptionsExt, path::Path};

use anyhow::{bail, Context, Result};
use ledger::{
    scan_state::{
        currency::{Amount, Balance, Fee, Length, Nonce, Sgn, Signed, Slot},
        transaction_logic::{
            protocol_state::{EpochData, EpochLedger, ProtocolStateView},
            transaction_partially_applied::{
                apply_transaction_first_pass, apply_transaction_second_pass,
            },
            verifiable,
            zkapp_command::{
                Account as UpdateAccount, AccountPreconditions, AccountUpdate, Actions,
                AuthorizationKind, Body, CallForest, Control, Events, FeePayer, FeePayerBody,
                MayUseToken, Numeric, Preconditions, Tree, Update, WithStackHash, ZkAppCommand,
                ZkAppPreconditions,
            },
            zkapp_statement::TransactionCommitment,
            Memo, Transaction, TransactionStatus, UserCommand, WithStatus,
        },
    },
    verifier::common::{self, CheckResult},
    Account as LedgerAccount, AccountId, BaseLedger, Database, Mask, MutableFp, TokenId,
};
use mina_core::NetworkConfig;
use mina_node_account::{AccountPublicKey, AccountSecretKey};
use mina_p2p_messages::{
    bigint::BigInt,
    v2::{MinaBaseSignatureStableV1, MinaBaseZkappCommandTStableV1WireStableV1, TokenIdKeyHash},
};
use mina_signer::{BaseField, Keypair, NetworkId, NonceMode, PubKey, Signature, Signer};
use serde_json::{json, Value};

const RELAY: &str = "http://127.0.0.1:3540/graphql";
const SENDER: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
const RECIPIENT: &str = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";
const TRANSFER: u64 = 1_000_000_000;
const FEE: u64 = 100_000_000;
const MEMO: &str = "mina-rust P4.15";

fn account_state(address: &str) -> Result<(u32, u64)> {
    let reply: Value = reqwest::blocking::Client::new()
        .post(RELAY)
        .json(&json!({"query": format!(
            "query {{ account(publicKey: \"{address}\") {{ nonce balance {{ total }} }} }}"
        )}))
        .send()?
        .error_for_status()?
        .json()?;
    if reply.get("errors").is_some() {
        bail!("relay account query failed: {}", reply["errors"]);
    }
    let account = &reply["data"]["account"];
    Ok((
        account["nonce"]
            .as_str()
            .context("missing nonce")?
            .parse()?,
        account["balance"]["total"]
            .as_str()
            .context("missing balance")?
            .parse()?,
    ))
}

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

fn epoch() -> EpochData<BaseField> {
    EpochData {
        ledger: EpochLedger {
            hash: BaseField::from(0u64),
            total_currency: Amount::from_u64(0),
        },
        seed: BaseField::from(0u64),
        start_checkpoint: BaseField::from(0u64),
        lock_checkpoint: BaseField::from(0u64),
        epoch_length: Length::from_u32(0),
    }
}

fn graphql_update(update: &AccountUpdate, signature: Option<String>) -> Value {
    let b = &update.body;
    let empty_epoch = json!({"ledger": {"hash": null, "totalCurrency": null},
        "seed": null, "startCheckpoint": null, "lockCheckpoint": null, "epochLength": null});
    json!({
        "body": {
            "publicKey": AccountPublicKey::from(b.public_key.clone()).to_string(),
            "tokenId": TokenIdKeyHash::from(&b.token_id).to_string(),
            "useFullCommitment": b.use_full_commitment, "incrementNonce": false,
            "update": {"appState": vec![None::<String>; ledger::ZKAPP_STATE_SIZE]},
            "balanceChange": {"magnitude": b.balance_change.magnitude.as_u64().to_string(),
                "sgn": match b.balance_change.sgn { Sgn::Pos => "Positive", Sgn::Neg => "Negative" }},
            "events": [], "actions": [], "callData": "0", "callDepth": 0,
            "preconditions": {
                "network": {"stakingEpochData": empty_epoch, "nextEpochData": empty_epoch},
                "account": {"state": vec![None::<String>; ledger::ZKAPP_STATE_SIZE]}
            },
            "mayUseToken": {"parentsOwnToken": false, "inheritFromParent": false},
            "authorizationKind": {"isSigned": signature.is_some(), "isProved": false},
            "implicitAccountCreationFee": false
        },
        "authorization": {"signature": signature}
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        bail!("usage: p415_devnet_zkapp KEY_PATH PASS_PATH OUTPUT_PATH");
    }
    NetworkConfig::init("devnet").map_err(anyhow::Error::msg)?;
    let password = fs::read_to_string(&args[2])?;
    let key =
        AccountSecretKey::from_encrypted_file(&args[1], password.trim_end_matches(['\r', '\n']))?;
    let kp: Keypair = key.clone().into();
    let sender_pk = key.public_key_compressed();
    if sender_pk != PubKey::from_address(SENDER)?.into_compressed() {
        bail!("the key does not match P4.15's throwaway sender");
    }
    let recipient_pk = PubKey::from_address(RECIPIENT)?.into_compressed();
    let (sender_nonce, sender_balance) = account_state(SENDER)?;
    let (recipient_nonce, recipient_balance) = account_state(RECIPIENT)?;

    let debit = account_update(
        sender_pk.clone(),
        Signed {
            magnitude: Amount::from_u64(TRANSFER),
            sgn: Sgn::Neg,
        },
        true,
    );
    let credit = account_update(
        recipient_pk.clone(),
        Signed {
            magnitude: Amount::from_u64(TRANSFER),
            sgn: Sgn::Pos,
        },
        false,
    );
    let mut forest = CallForest(vec![tree(debit), tree(credit)]);
    forest.ensure_hashed();
    let mut command = ZkAppCommand {
        fee_payer: FeePayer {
            body: FeePayerBody {
                public_key: sender_pk.clone(),
                fee: Fee::from_u64(FEE),
                valid_until: None,
                nonce: Nonce::from_u32(sender_nonce),
            },
            authorization: Signature::dummy(),
        },
        account_updates: forest,
        memo: Memo::create_from_str_checked(MEMO)?,
    };
    let commitment = TransactionCommitment::create(command.account_updates_hash());
    let full_commitment = commitment.create_complete(
        command.memo.hash(),
        AccountUpdate::of_fee_payer(command.fee_payer.clone()).digest(),
    );
    let mut signer = mina_signer::create_kimchi::<TransactionCommitment>(NetworkId::TESTNET);
    let fee_signature = signer.sign(&kp, &full_commitment, NonceMode::Chunked);
    if !signer.verify(&fee_signature, &kp.public, &full_commitment) {
        bail!("fee payer signature failed signer verification");
    }
    command.fee_payer.authorization = fee_signature.clone();
    command.account_updates.0[0]
        .elt
        .account_update
        .authorization = Control::Signature(fee_signature.clone());
    let verifiable = ledger::scan_state::transaction_logic::zkapp_command::verifiable::create(
        &command,
        false,
        |_, _| Err("unexpected proof".into()),
    )
    .map_err(anyhow::Error::msg)?;
    match common::check(WithStatus::applied(verifiable::UserCommand::ZkAppCommand(
        Box::new(verifiable),
    ))) {
        CheckResult::Valid(_) => {}
        other => bail!("independent ledger verifier rejected signatures: {other:?}"),
    }

    // Local two-pass execution uses relay balances/nonces and the standard
    // user-account permissions. No zkApp precondition depends on other state.
    let mut ledger = Mask::new_root(Database::create(35));
    let sender_id = AccountId::new(sender_pk.clone(), TokenId::default());
    let recipient_id = AccountId::new(recipient_pk.clone(), TokenId::default());
    let mut sender_account =
        LedgerAccount::create_with(sender_id.clone(), Balance::from_u64(sender_balance));
    sender_account.nonce = Nonce::from_u32(sender_nonce);
    let mut recipient_account =
        LedgerAccount::create_with(recipient_id.clone(), Balance::from_u64(recipient_balance));
    recipient_account.nonce = Nonce::from_u32(recipient_nonce);
    ledger
        .get_or_create_account(sender_id.clone(), sender_account)
        .map_err(|e| anyhow::anyhow!("insert sender: {e:?}"))?;
    ledger
        .get_or_create_account(recipient_id.clone(), recipient_account)
        .map_err(|e| anyhow::anyhow!("insert recipient: {e:?}"))?;
    let zero = BaseField::from(0u64);
    let view = ProtocolStateView {
        snarked_ledger_hash: zero,
        blockchain_length: Length::from_u32(0),
        min_window_density: Length::from_u32(0),
        total_currency: Amount::from_u64(0),
        global_slot_since_genesis: Slot::from_u32(0),
        staking_epoch_data: epoch(),
        next_epoch_data: epoch(),
    };
    let tx = Transaction::Command(UserCommand::ZkAppCommand(Box::new(command.clone())));
    let partial = apply_transaction_first_pass(
        mina_core::constants::constraint_constants(),
        Slot::from_u32(0),
        &view,
        &mut ledger,
        &tx,
    )
    .map_err(anyhow::Error::msg)?;
    let applied = apply_transaction_second_pass(
        mina_core::constants::constraint_constants(),
        &mut ledger,
        partial,
    )
    .map_err(anyhow::Error::msg)?;
    if applied.transaction().status != TransactionStatus::Applied {
        bail!(
            "local ledger application failed: {:?}",
            applied.transaction().status
        );
    }
    let sender_after = ledger
        .get(
            ledger
                .location_of_account(&sender_id)
                .context("sender missing")?,
        )
        .context("sender missing")?;
    let recipient_after = ledger
        .get(
            ledger
                .location_of_account(&recipient_id)
                .context("recipient missing")?,
        )
        .context("recipient missing")?;
    if sender_after.balance.as_u64() != sender_balance - TRANSFER - FEE
        || sender_after.nonce != Nonce::from_u32(sender_nonce + 1)
        || recipient_after.balance.as_u64() != recipient_balance + TRANSFER
    {
        bail!("local ledger post-state did not match the expected transfer");
    }

    let wire: MinaBaseZkappCommandTStableV1WireStableV1 = (&command).into();
    let hash = wire.hash()?;
    let wire_signature: mina_p2p_messages::v2::Signature =
        MinaBaseSignatureStableV1::from(&fee_signature).into();
    let wire_signature = wire_signature.to_string();
    let request = json!({
        "query": "mutation SendZkapp($input: SendZkappInput!) { sendZkapp(input: $input) { zkapp { hash id failureReason { index failures } } } }",
        "variables": {"input": {"zkappCommand": {
            "memo": mina_p2p_messages::v2::MinaBaseSignedCommandMemoStableV1::from(&command.memo).to_base58check(),
            "feePayer": {"body": {"publicKey": SENDER, "fee": FEE.to_string(),
                "nonce": sender_nonce.to_string()},
                "authorization": wire_signature.clone()},
            "accountUpdates": [
                graphql_update(&command.account_updates.0[0].elt.account_update, Some(wire_signature.clone())),
                graphql_update(&command.account_updates.0[1].elt.account_update, None),
            ]
        }}}
    });
    let output = Path::new(&args[3]);
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(output)?;
    serde_json::to_writer_pretty(file, &request)?;
    println!("hash={hash}");
    println!("commitment={}", BigInt::from(commitment.0).to_decimal());
    println!(
        "full_commitment={}",
        BigInt::from(full_commitment.0).to_decimal()
    );
    println!("sender_before={sender_balance} nonce={sender_nonce}");
    println!("recipient_before={recipient_balance} nonce={recipient_nonce}");
    println!(
        "local_result=Applied sender_after={} nonce={} recipient_after={}",
        sender_after.balance.as_u64(),
        sender_after.nonce.as_u32(),
        recipient_after.balance.as_u64()
    );
    println!("request_path={}", output.display());
    Ok(())
}
