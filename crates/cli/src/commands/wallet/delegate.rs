//! `mina wallet delegate`: OCaml's `mina client delegate-stake`, signed here
//! and sent through a node's GraphQL `sendDelegation`.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/app/cli/src/init/client.ml#L567-L602>

use std::{path::PathBuf, str::FromStr};

use anyhow::{bail, Context, Result};
use ledger::scan_state::{
    currency::{Fee, Nonce, Slot},
    transaction_logic::{
        signed_command::{
            Body, Common, SignedCommand, SignedCommandPayload, StakeDelegationPayload,
        },
        transaction_union_payload::TransactionUnionPayload,
        Memo,
    },
};
use mina_node::daemon_json::to_mina_string;
use mina_node_account::{AccountPublicKey, AccountSecretKey};
use mina_p2p_messages::bigint::BigInt;
use mina_signer::{CompressedPubKey, Keypair, NetworkId, Signer};
use serde_json::{json, Value};

use super::super::{node_api, Network};
use crate::commands::wallet::balance::OutputFormat;

/// The longest memo, in bytes.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/signed_command_memo.ml#L128-L133>
const MAX_MEMO_BYTES: usize = 32;

#[derive(Debug, clap::Args)]
pub struct Delegate {
    /// The encrypted key file of the account delegating its stake. Its
    /// password is read from `MINA_PRIVKEY_PASS`, or asked for on a terminal.
    #[arg(long, value_name = "FILE")]
    pub from: PathBuf,

    /// The public key to delegate the stake to, as OCaml's `--receiver`.
    #[arg(long, alias = "receiver", value_name = "PUBLIC_KEY")]
    pub to: AccountPublicKey,

    /// The transaction fee in MINA, like `0.1`, with at most 9 decimal
    /// places, as OCaml's `--fee`.
    #[arg(
        long,
        value_name = "MINA",
        allow_negative_numbers = true,
        value_parser = super::parse_mina
    )]
    pub fee: u64,

    /// The nonce. Defaults to the node's inferred nonce for the sender, which
    /// counts its pending transactions.
    #[arg(long, value_name = "NONCE")]
    pub nonce: Option<u32>,

    /// The memo, at most 32 bytes.
    #[arg(
        long,
        default_value = "",
        hide_default_value = true,
        value_name = "TEXT"
    )]
    pub memo: String,

    /// The last global slot since genesis in which the delegation may be
    /// included. By default it never expires.
    #[arg(long, value_name = "SLOT")]
    pub valid_until: Option<u32>,

    /// The node's HTTP address (its `--port`). The node must run with
    /// `--enable-tx-submission`.
    #[arg(
        long,
        default_value = "http://localhost:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: String,

    /// The output format.
    #[arg(long, value_enum, default_value = "text")]
    pub format: OutputFormat,
}

impl Delegate {
    pub fn run(self, network: Network) -> Result<()> {
        let result = self.send(&network);
        if let Err(err) = result {
            node_api::exit_with(err);
        }
        Ok(())
    }

    fn send(&self, network: &Network) -> Result<()> {
        if self.memo.len() > MAX_MEMO_BYTES {
            bail!(
                "the memo is {} bytes; at most {MAX_MEMO_BYTES} fit",
                self.memo.len()
            );
        }
        let new_delegate: CompressedPubKey = self
            .to
            .clone()
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid public key to delegate to"))?;

        node_api::check_node(&self.node, network, true)?;

        let password = password()?;
        let key = AccountSecretKey::from_encrypted_file(&self.from, &password)
            .with_context(|| format!("failed to decrypt the key file {}", self.from.display()))?;
        let sender = key.public_key_compressed();
        let sender_address = AccountPublicKey::from(sender.clone()).to_string();

        let nonce = match self.nonce {
            Some(nonce) => nonce,
            None => inferred_nonce(&self.node, &sender_address)?,
        };
        let payload = delegation_payload(
            sender,
            new_delegate,
            self.fee,
            nonce,
            self.valid_until,
            &self.memo,
        )?;
        let command = sign(payload, &key, network_id(network));
        let (input, signature) = graphql_variables(&command, &self.memo);
        let data = node_api::graphql(
            &self.node,
            SEND_DELEGATION,
            json!({ "input": input, "signature": signature }),
        )?;
        let delegation = &data["sendDelegation"]["delegation"];
        let hash = delegation["hash"]
            .as_str()
            .context("the node's answer has no transaction hash")?;

        let summary = json!({
            "status": "pending",
            "hash": hash,
            "id": delegation["id"],
            "from": sender_address,
            "to": self.to.to_string(),
            "fee_nanomina": self.fee.to_string(),
            "nonce": nonce,
            "valid_until": command.payload.common.valid_until.as_u32(),
            "memo": self.memo,
            "node": self.node,
        });
        match self.format {
            OutputFormat::Json => println!("{summary}"),
            OutputFormat::Text => {
                println!("Dispatched stake delegation with hash {hash}");
                println!(
                    "From {sender_address} to {}, fee {} MINA, nonce {nonce}",
                    self.to,
                    to_mina_string(self.fee)
                );
                println!("Check it with: mina wallet status --hash {hash}");
            }
        }
        Ok(())
    }
}

const SEND_DELEGATION: &str =
    "mutation ($input: SendDelegationInput!, $signature: SignatureInput!) {
  sendDelegation(input: $input, signature: $signature) {
    delegation { hash id nonce }
  }
}";

fn network_id(network: &Network) -> NetworkId {
    match network {
        Network::Mainnet => NetworkId::MAINNET,
        Network::Devnet => NetworkId::TESTNET,
    }
}

/// The key file's password: MINA_PRIVKEY_PASS, as OCaml reads it, or a
/// prompt on a terminal. Never on the command line.
fn password() -> Result<String> {
    if let Ok(password) = std::env::var("MINA_PRIVKEY_PASS") {
        return Ok(password);
    }
    if console::Term::stderr().is_term() {
        return dialoguer::Password::new()
            .with_prompt("Password for the key file")
            .allow_empty_password(true)
            .interact()
            .context("failed to read the password");
    }
    bail!("set MINA_PRIVKEY_PASS to the key file's password")
}

/// The sender's next nonce as the node infers it: its pool's, else its
/// ledger's.
fn inferred_nonce(node: &str, sender: &str) -> Result<u32> {
    let data = node_api::graphql(
        node,
        "query ($pk: PublicKey!) { account(publicKey: $pk) { nonce inferredNonce } }",
        json!({ "pk": sender }),
    )?;
    let account = &data["account"];
    if account.is_null() {
        bail!("{sender} has no account in the node's ledger");
    }
    let nonce = account["inferredNonce"]
        .as_str()
        .or_else(|| account["nonce"].as_str())
        .context("the node did not say the sender's nonce")?;
    nonce.parse().context("the node's nonce is not a number")
}

/// The signed command's payload: a stake delegation.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/signed_command_payload.ml#L274-L284>
fn delegation_payload(
    sender: CompressedPubKey,
    new_delegate: CompressedPubKey,
    fee: u64,
    nonce: u32,
    valid_until: Option<u32>,
    memo: &str,
) -> Result<SignedCommandPayload> {
    let memo = Memo::from_str(memo).map_err(|()| anyhow::anyhow!("invalid memo"))?;
    Ok(SignedCommandPayload {
        common: Common {
            fee: Fee::from_u64(fee),
            fee_payer_pk: sender,
            nonce: Nonce::from_u32(nonce),
            valid_until: valid_until.map(Slot::from_u32).unwrap_or_else(Slot::max),
            memo,
        },
        body: Body::StakeDelegation(StakeDelegationPayload::SetDelegate { new_delegate }),
    })
}

/// Signs as OCaml does: the legacy transaction hash with legacy nonce
/// derivation, so the signature is OCaml's for the same key and payload.
fn sign(
    payload: SignedCommandPayload,
    key: &AccountSecretKey,
    network: NetworkId,
) -> SignedCommand {
    let message = TransactionUnionPayload::of_user_command_payload(&payload);
    let keypair: Keypair = key.clone().into();
    let signature = mina_signer::create_legacy(network).sign(
        &keypair,
        &message,
        mina_signer::NonceMode::Legacy,
    );
    SignedCommand {
        payload,
        signer: key.public_key_compressed(),
        signature,
    }
}

/// `SendDelegationInput` and `SignatureInput` for the signed command.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/graphql/mina_graphql/types.ml#L3055-L3083>
fn graphql_variables(command: &SignedCommand, memo: &str) -> (Value, Value) {
    let common = &command.payload.common;
    let Body::StakeDelegation(StakeDelegationPayload::SetDelegate { new_delegate }) =
        &command.payload.body
    else {
        unreachable!("a delegation payload")
    };
    let mut input = json!({
        "from": AccountPublicKey::from(common.fee_payer_pk.clone()).to_string(),
        "to": AccountPublicKey::from(new_delegate.clone()).to_string(),
        "fee": common.fee.as_u64().to_string(),
        "nonce": common.nonce.as_u32().to_string(),
        "validUntil": common.valid_until.as_u32().to_string(),
    });
    if !memo.is_empty() {
        input["memo"] = json!(memo);
    }
    let signature = json!({
        "field": BigInt::from(command.signature.rx).to_decimal(),
        "scalar": BigInt::from(command.signature.s).to_decimal(),
    });
    (input, signature)
}

#[cfg(test)]
mod tests {
    use super::*;

    use mina_p2p_messages::v2::{self, MinaBaseSignedCommandMemoStableV1};
    use mina_signer::{PubKey, Signature};

    /// A stake delegation OCaml signed on mainnet, from precomputed block
    /// 552000 (`3NLVF6XLsDcKHqHvnQ4g4igXasxxSvBXRDKNBEWA3WujEfp8DWJC`),
    /// staged ledger diff command 2.
    const OCAML_SIGNER: &str = "B62qiVMNUUZgAzkEf2gNk3LmPYrDMCwP9MRPD6mvkp18p3NgHBpQKoz";
    const OCAML_DELEGATE: &str = "B62qq3TQ8AP7MFYPVtMx5tZGF3kWLJukfwG1A1RGvaBW1jfTPTkDBW6";
    const OCAML_FEE: u64 = 10_100_000; // "0.0101" MINA
    const OCAML_MEMO: &str = "E4YM2vTHhWEg66xpj52JErHUBU4pZ1yageL4TVDDpTTSsv8mK6YaH";
    const OCAML_SIGNATURE: &str =
        "7mX5ohESkLL1sbcURVY29iMgcNS6LZWY2DzhDgN8KG1Y5sUPDQpKGmS9gviHMu199xGwLcAtJ1wSAopnzqpRJnGBtwqnzwDB";

    fn pk(address: &str) -> CompressedPubKey {
        AccountPublicKey::from_str(address)
            .unwrap()
            .try_into()
            .unwrap()
    }

    /// Our payload for OCaml's fields is the message OCaml signed: OCaml's
    /// signature verifies against it on mainnet.
    #[test]
    fn ocaml_signature_verifies_against_our_payload() {
        let payload =
            delegation_payload(pk(OCAML_SIGNER), pk(OCAML_DELEGATE), OCAML_FEE, 0, None, "")
                .unwrap();
        assert_eq!(
            MinaBaseSignedCommandMemoStableV1::from(&payload.common.memo).to_base58check(),
            OCAML_MEMO
        );
        assert_eq!(payload.common.valid_until.as_u32(), 4294967295);

        let signature: v2::Signature = OCAML_SIGNATURE.parse().unwrap();
        let signature = Signature::try_from(&*signature).unwrap();
        let signer = PubKey::from_address(OCAML_SIGNER).unwrap();
        let message = TransactionUnionPayload::of_user_command_payload(&payload);
        assert!(
            mina_signer::create_legacy(NetworkId::MAINNET).verify(&signature, &signer, &message)
        );
        // Not on devnet: the network is part of the message.
        assert!(
            !mina_signer::create_legacy(NetworkId::TESTNET).verify(&signature, &signer, &message)
        );
        // Nor for another fee.
        let other = delegation_payload(
            pk(OCAML_SIGNER),
            pk(OCAML_DELEGATE),
            OCAML_FEE + 1,
            0,
            None,
            "",
        )
        .unwrap();
        assert!(!mina_signer::create_legacy(NetworkId::MAINNET).verify(
            &signature,
            &signer,
            &TransactionUnionPayload::of_user_command_payload(&other)
        ));
    }

    /// What we sign verifies, and what we send holds no secret.
    #[test]
    fn signed_delegation_verifies_and_variables_hold_no_secret() {
        let key = AccountSecretKey::rand();
        let payload = delegation_payload(
            key.public_key_compressed(),
            pk(OCAML_DELEGATE),
            100_000_000,
            7,
            Some(600_000),
            "mina-rust R43c",
        )
        .unwrap();
        let command = sign(payload, &key, NetworkId::TESTNET);
        let message = TransactionUnionPayload::of_user_command_payload(&command.payload);
        let public =
            PubKey::from_address(&AccountPublicKey::from(key.public_key_compressed()).to_string())
                .unwrap();
        assert!(mina_signer::create_legacy(NetworkId::TESTNET).verify(
            &command.signature,
            &public,
            &message
        ));

        let (input, signature) = graphql_variables(&command, "mina-rust R43c");
        assert_eq!(input["fee"], "100000000");
        assert_eq!(input["nonce"], "7");
        assert_eq!(input["validUntil"], "600000");
        assert_eq!(input["memo"], "mina-rust R43c");
        assert_eq!(input["to"], OCAML_DELEGATE);
        let sent = format!("{input}{signature}");
        assert!(!sent.contains(&key.to_string()));
    }

    #[test]
    fn a_long_memo_is_refused() {
        let delegate = Delegate {
            from: "/nonexistent".into(),
            to: AccountPublicKey::from_str(OCAML_DELEGATE).unwrap(),
            fee: 1,
            nonce: None,
            memo: "x".repeat(33),
            valid_until: None,
            node: "http://127.0.0.1:9".into(),
            format: OutputFormat::Json,
        };
        let err = delegate.send(&Network::Devnet).unwrap_err();
        assert!(err.to_string().contains("33 bytes"), "{err}");
    }
}
