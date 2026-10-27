use std::{path::PathBuf, str::FromStr};

use anyhow::{Context, Result};
use ledger::scan_state::{
    currency::{Amount, Fee, Nonce, Slot},
    transaction_logic::{
        signed_command::{Body, Common, PaymentPayload, SignedCommand, SignedCommandPayload},
        transaction_union_payload::TransactionUnionPayload,
        Memo,
    },
};
use mina_node_account::{AccountPublicKey, AccountSecretKey};
use mina_p2p_messages::v2::MinaBaseSignedCommandStableV2;
use mina_signer::{CompressedPubKey, Keypair, Signer};

use mina_node::daemon_json::to_mina_string;

use super::super::{node_api, Network};

/// Refuses a memo over 32 bytes: `Memo::from_str` cuts a longer memo to 32
/// bytes without saying so.
fn check_memo(memo: &str) -> Result<()> {
    anyhow::ensure!(
        memo.len() <= 32,
        "the memo is {} bytes; at most 32 fit",
        memo.len()
    );
    Ok(())
}

fn network_to_network_id(network: &Network) -> mina_signer::NetworkId {
    match network {
        Network::Mainnet => mina_signer::NetworkId::MAINNET,
        Network::Devnet => mina_signer::NetworkId::TESTNET,
    }
}

#[cfg(test)]
mod r39_tests {
    use super::*;
    use mina_node_common::rpc::transaction_pool::signing::{self, Transfer};
    use mina_p2p_messages::v2::{MinaBaseUserCommandStableV2, Signature};

    const SENDER: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";
    const RECIPIENT: &str = "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX";

    fn transfer() -> Transfer {
        Transfer {
            from: SENDER.parse().unwrap(),
            to: RECIPIENT.parse().unwrap(),
            amount: "1000000000".into(),
            fee: "100000000".into(),
            nonce: 0,
            memo: "mina-rust P4.13 #1".into(),
            valid_until: None,
        }
    }

    #[test]
    fn rejects_caller_network_and_numeric_amounts() {
        let mut input = serde_json::json!({"from": SENDER, "to": RECIPIENT,
            "amount": "1000000000", "fee": "100000000", "nonce": 0, "memo": "test"});
        assert!(serde_json::from_value::<Transfer>(input.clone()).is_ok());
        input["network"] = "mainnet".into();
        assert!(serde_json::from_value::<Transfer>(input.clone()).is_err());
        input.as_object_mut().unwrap().remove("network");
        input["amount"] = serde_json::json!(1000000000u64);
        assert!(serde_json::from_value::<Transfer>(input).is_err());
    }

    // Only the explicitly authorised throwaway account signs. The encrypted
    // file and password stay outside the repo. Run in separate processes for
    // R39_SIGNING_NETWORK=devnet and mainnet (NetworkConfig is once-only).
    #[test]
    #[ignore = "requires R39_KEY_FILE and R39_PASS_FILE for the throwaway account"]
    fn signing_matches_native_cli_and_p415() {
        let network = std::env::var("R39_SIGNING_NETWORK").unwrap_or("devnet".into());
        mina_core::NetworkConfig::init(&network).unwrap();
        let password = std::fs::read_to_string(std::env::var("R39_PASS_FILE").unwrap()).unwrap();
        let key = AccountSecretKey::from_encrypted_file(
            std::env::var("R39_KEY_FILE").unwrap(),
            password.trim_end_matches(['\r', '\n']),
        )
        .unwrap();
        assert_eq!(key.public_key().to_string(), SENDER);
        let mut transfer = transfer();
        let cli = Send {
            from: PathBuf::new(),
            password: None,
            password_flag: None,
            to: transfer.to.clone(),
            amount: 1_000_000_000,
            fee: 100_000_000,
            memo: transfer.memo.clone(),
            nonce: Some(0),
            valid_until: None,
            node: String::new(),
        };
        let payload = SignedCommandPayload {
            common: Common {
                fee: Fee::from_u64(cli.fee),
                fee_payer_pk: key.public_key_compressed(),
                nonce: Nonce::from_u32(0),
                valid_until: Slot::max(),
                memo: Memo::from_str(&cli.memo).unwrap(),
            },
            body: Body::Payment(PaymentPayload {
                receiver_pk: cli.to.clone().try_into().unwrap(),
                amount: Amount::from_u64(cli.amount),
            }),
        };
        let native_network = if network == "mainnet" {
            Network::Mainnet
        } else {
            Network::Devnet
        };
        let native = cli
            .sign_transaction(payload, &key, network_to_network_id(&native_network))
            .unwrap();
        let wasm_path = signing::payment(&key, &transfer).unwrap();
        let expected = MinaBaseUserCommandStableV2::SignedCommand((&native).into());
        // Includes the signature, which the transaction hash deliberately omits.
        assert_eq!(wasm_path, expected);
        assert_eq!(wasm_path.hash().unwrap(), expected.hash().unwrap());
        let wrong_domain = if network == "mainnet" {
            mina_signer::NetworkId::TESTNET
        } else {
            mina_signer::NetworkId::MAINNET
        };
        let kp: Keypair = key.clone().into();
        let msg = TransactionUnionPayload::of_user_command_payload(&native.payload);
        assert!(
            mina_signer::create_legacy(network_to_network_id(&native_network)).verify(
                &native.signature,
                &kp.public,
                &msg
            )
        );
        assert!(!mina_signer::create_legacy(wrong_domain).verify(
            &native.signature,
            &kp.public,
            &msg
        ));
        if network == "devnet" {
            // Live OCaml-applied P4.13 command, block 577957.
            assert_eq!(
                wasm_path.hash().unwrap().to_string(),
                "5Ju4i9buTnL4ej4YJoKLt2aBtpfK33nVMuVCXiC8H1P4pZdKBMWm"
            );
        }

        transfer.nonce = 4;
        transfer.memo = "mina-rust P4.15".into();
        let wire = signing::zkapp_transfer(&key, &transfer).unwrap();
        let MinaBaseUserCommandStableV2::ZkappCommand(ref zkapp) = wire else {
            panic!("expected zkApp")
        };
        let command: ledger::scan_state::transaction_logic::zkapp_command::ZkAppCommand =
            zkapp.try_into().unwrap();
        use ledger::scan_state::transaction_logic::{
            zkapp_command::AccountUpdate, zkapp_statement::TransactionCommitment,
        };
        let commitment = TransactionCommitment::create(command.account_updates_hash());
        let full = commitment.create_complete(
            command.memo.hash(),
            AccountUpdate::of_fee_payer(command.fee_payer.clone()).digest(),
        );
        assert!(
            mina_signer::create_kimchi(network_to_network_id(&native_network)).verify(
                &command.fee_payer.authorization,
                &kp.public,
                &full
            )
        );
        let wrong_domain = if network == "mainnet" {
            mina_signer::NetworkId::TESTNET
        } else {
            mina_signer::NetworkId::MAINNET
        };
        assert!(!mina_signer::create_kimchi(wrong_domain).verify(
            &command.fee_payer.authorization,
            &kp.public,
            &full
        ));
        if network == "devnet" {
            // P4.15 request3.json, independently accepted by OCaml in block
            // 577969. These expected values are historical public evidence.
            assert_eq!(
                wire.hash().unwrap().to_string(),
                "5JutrMV8zE62qQoANZzb7YoekYHPuV8BRDhTGoe7k9RnrEHZzkbT"
            );
            let signature: Signature = zkapp.fee_payer.authorization.clone().into();
            assert_eq!(signature.to_string(), "7mXQbw3BkS6vhDkTTffTUMuqEWpMyxZ3mu8r7LkL6Nw4H3eMQJ19QDt4Hgd9EV8Api4e2st8SbgctE49cx2yHXnwC1cCNWFN");
            assert_eq!(
                mina_p2p_messages::bigint::BigInt::from(commitment.0).to_decimal(),
                "7544711089621773277520552278650488216512637470710896640171130652119163773173"
            );
            assert_eq!(
                mina_p2p_messages::bigint::BigInt::from(full.0).to_decimal(),
                "27254074112025354018067691062675529431864978025499503035945909247801679607181"
            );
        }
        use ledger::scan_state::transaction_logic::{verifiable, WithStatus};
        let checked = ledger::scan_state::transaction_logic::zkapp_command::verifiable::create(
            &command,
            false,
            |_, _| Err("unexpected proof".into()),
        )
        .unwrap();
        assert!(matches!(
            ledger::verifier::common::check(WithStatus::applied(
                verifiable::UserCommand::ZkAppCommand(Box::new(checked))
            )),
            ledger::verifier::common::CheckResult::Valid(_)
        ));

        transfer.from = transfer.to.clone();
        assert!(signing::payment(&key, &transfer).is_err());
        assert!(signing::zkapp_transfer(&key, &transfer).is_err());
        transfer.from = key.public_key();
        transfer.memo = "x".repeat(33);
        assert!(signing::payment(&key, &transfer).is_err());
        assert!(signing::zkapp_transfer(&key, &transfer).is_err());
        transfer.memo.clear();
        transfer.amount = u64::MAX.to_string();
        assert!(signing::payment(&key, &transfer).is_err());
        assert!(signing::zkapp_transfer(&key, &transfer).is_err());
    }
}

#[derive(Debug, clap::Args)]
pub struct Send {
    /// The sender's encrypted key file. Its password is read from the
    /// `MINA_PRIVKEY_PASS` environment variable.
    #[arg(long, env = "MINA_FROM", value_name = "FILE")]
    pub from: PathBuf,

    /// Refused: a password on the command line would be visible to other
    /// processes; it is read from the MINA_PRIVKEY_PASS environment
    /// variable instead.
    #[arg(hide = true, value_parser = crate::commands::secret_value::refused_privkey_password())]
    pub password: Option<String>,

    /// Refused as the positional is: `--password` and the other names an
    /// operator may try (RC7-FIXES).
    #[arg(
        long = "password",
        hide = true,
        aliases = ["pass", "privkey-pass", "privkey-password"],
        num_args = 0..=1,
        default_missing_value = "",
        value_parser = crate::commands::secret_value::refused_privkey_password()
    )]
    pub password_flag: Option<String>,

    /// The receiver's public key.
    #[arg(long, value_name = "PUBLIC_KEY")]
    pub to: AccountPublicKey,

    /// The amount in MINA, like `2.5`, with at most 9 decimal places, as
    /// OCaml's `mina client send-payment --amount`.
    #[arg(
        long,
        value_name = "MINA",
        allow_negative_numbers = true,
        value_parser = super::parse_mina
    )]
    pub amount: u64,

    /// The transaction fee in MINA, like `0.1`, with at most 9 decimal
    /// places.
    #[arg(
        long,
        value_name = "MINA",
        allow_negative_numbers = true,
        value_parser = super::parse_mina
    )]
    pub fee: u64,

    /// The memo, at most 32 bytes.
    #[arg(
        long,
        default_value = "",
        hide_default_value = true,
        value_name = "TEXT"
    )]
    pub memo: String,

    /// The nonce. Defaults to the sender account's nonce in the node's
    /// ledger.
    #[arg(long, value_name = "NONCE")]
    pub nonce: Option<u32>,

    /// The last global slot since genesis in which the payment may be
    /// included. By default it never expires.
    #[arg(long, value_name = "SLOT")]
    pub valid_until: Option<u32>,

    /// The node's HTTP base URL (its `--port`). The node must run with
    /// `--enable-tx-submission`.
    #[arg(
        long,
        default_value = "http://localhost:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: String,
}

impl Send {
    pub fn run(self, network: Network) -> Result<()> {
        debug_assert!(self.password.is_none() && self.password_flag.is_none());
        let password = crate::commands::secret_value::password_from_env("MINA_PRIVKEY_PASS")?;
        check_memo(&self.memo)?;
        // Check node is synced and on the correct network
        println!("Checking node status...");
        self.check_node_status(&network)?;

        // Load the sender's secret key
        let sender_key = AccountSecretKey::from_encrypted_file(&self.from, &password)
            .with_context(|| {
                format!("Failed to decrypt sender key file: {}", self.from.display())
            })?;

        let sender_pk = sender_key.public_key_compressed();
        println!("Sender: {}", AccountPublicKey::from(sender_pk.clone()));

        // A payment's fee payer is its sender, which signs it.
        let fee_payer_pk = sender_pk.clone();
        println!(
            "Amount: {} MINA, fee: {} MINA",
            to_mina_string(self.amount),
            to_mina_string(self.fee)
        );

        // Convert receiver public key to CompressedPubKey
        let receiver_pk: CompressedPubKey = self
            .to
            .clone()
            .try_into()
            .map_err(|_| anyhow::anyhow!("Invalid receiver public key"))?;

        // Fetch nonce from node if not provided
        // Note: GraphQL API expects nonce to be account_nonce, but we need to sign
        // with account_nonce for the first transaction from a new account
        let nonce = if let Some(nonce) = self.nonce {
            nonce
        } else {
            println!("Fetching nonce from node...");
            self.fetch_nonce(&fee_payer_pk)?
        };

        println!("Using nonce: {}", nonce);

        // Create the payment payload
        let payload = SignedCommandPayload {
            common: Common {
                fee: Fee::from_u64(self.fee),
                fee_payer_pk,
                nonce: Nonce::from_u32(nonce),
                valid_until: self
                    .valid_until
                    .map(Slot::from_u32)
                    .unwrap_or_else(Slot::max),
                memo: Memo::from_str(&self.memo).unwrap_or_else(|_| Memo::empty()),
            },
            body: Body::Payment(PaymentPayload {
                receiver_pk,
                amount: Amount::from_u64(self.amount),
            }),
        };

        // Sign the transaction
        println!("Signing transaction...");
        let network_id = network_to_network_id(&network);
        let signed_command = self.sign_transaction(payload, &sender_key, network_id)?;

        // Submit to node
        println!("Submitting transaction to node...");
        let tx_hash = self.submit_transaction(signed_command)?;

        println!("\nTransaction submitted successfully!");
        println!("Transaction hash: {}", tx_hash);
        println!("Status: Pending");
        println!("\nYou can check the transaction status with:");
        println!("  mina wallet status --hash {}", tx_hash);

        Ok(())
    }

    fn check_node_status(&self, network: &Network) -> Result<()> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;
        let url = node_api::graphql_url(&self.node);

        // GraphQL query to check sync status and network ID
        let query = serde_json::json!({
            "query": r#"query {
                syncStatus
                networkID
            }"#
        });

        let response = client
            .post(&url)
            .json(&query)
            .send()
            .context("Failed to query node status")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to connect to node: HTTP {}", response.status());
        }

        let response_json: serde_json::Value = response
            .json()
            .context("Failed to parse GraphQL response")?;

        // Check for GraphQL errors
        if let Some(errors) = response_json.get("errors") {
            let error_msg = serde_json::to_string_pretty(errors)
                .unwrap_or_else(|_| "Unknown GraphQL error".to_string());
            anyhow::bail!("GraphQL error: {}", error_msg);
        }

        // Check sync status
        let sync_status = response_json["data"]["syncStatus"]
            .as_str()
            .context("Sync status not found in GraphQL response")?;

        if sync_status != "SYNCED" {
            anyhow::bail!(
                "Node is not synced (status: {}). Please wait for the node to sync before sending transactions.",
                sync_status
            );
        }

        println!("Node is synced: {}", sync_status);

        // Check network ID
        let network_id = response_json["data"]["networkID"]
            .as_str()
            .context("Network ID not found in GraphQL response")?;

        // Expected network ID based on selected network
        let expected_network = match network {
            Network::Mainnet => "mina:mainnet",
            Network::Devnet => "mina:devnet",
        };

        if !network_id.contains(expected_network) {
            anyhow::bail!(
                "Network mismatch: node is on '{}' but you selected {:?}. Use --network to specify the correct network.",
                network_id,
                network
            );
        }

        println!("Network verified: {}", network_id);

        Ok(())
    }

    fn fetch_nonce(&self, sender_pk: &CompressedPubKey) -> Result<u32> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;
        let url = node_api::graphql_url(&self.node);

        // GraphQL query to fetch account information
        let query = serde_json::json!({
            "query": format!(
                r#"query {{
                    account(publicKey: "{}") {{
                        nonce
                    }}
                }}"#,
                AccountPublicKey::from(sender_pk.clone()).to_string()
            )
        });

        let response = client
            .post(&url)
            .json(&query)
            .send()
            .context("Failed to query account from node")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to fetch account: HTTP {}", response.status());
        }

        let response_json: serde_json::Value = response
            .json()
            .context("Failed to parse GraphQL response")?;

        // Extract nonce from GraphQL response
        let nonce_str = response_json["data"]["account"]["nonce"]
            .as_str()
            .context("Nonce not found in GraphQL response")?;

        let nonce = nonce_str
            .parse::<u32>()
            .context("Failed to parse nonce as u32")?;

        Ok(nonce)
    }

    fn sign_transaction(
        &self,
        payload: SignedCommandPayload,
        sender_key: &AccountSecretKey,
        network_id: mina_signer::NetworkId,
    ) -> Result<SignedCommand> {
        // Create the transaction union payload for signing
        let payload_to_sign = TransactionUnionPayload::of_user_command_payload(&payload);

        // Create signer and sign the transaction
        let mut signer = mina_signer::create_legacy(network_id);
        let kp: Keypair = sender_key.clone().into();
        // Signed commands use OCaml's `Message.Legacy` nonce derivation, so the
        // signature is byte-identical to OCaml's for the same key and payload.
        let signature = signer.sign(&kp, &payload_to_sign, mina_signer::NonceMode::Legacy);

        Ok(SignedCommand {
            payload,
            signer: sender_key.public_key_compressed(),
            signature,
        })
    }

    fn submit_transaction(&self, signed_command: SignedCommand) -> Result<String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .context("Failed to create HTTP client")?;
        let url = node_api::graphql_url(&self.node);

        // Convert to v2 types for easier field extraction
        let signed_cmd_v2: MinaBaseSignedCommandStableV2 = (&signed_command).into();

        // Convert signature to GraphQL format (field and scalar as decimal strings)
        let sig_field =
            mina_p2p_messages::bigint::BigInt::from(signed_command.signature.rx).to_decimal();
        let sig_scalar =
            mina_p2p_messages::bigint::BigInt::from(signed_command.signature.s).to_decimal();

        // Extract payment details from signed command
        let (receiver_pk, amount) = match &signed_cmd_v2.payload.body {
            mina_p2p_messages::v2::MinaBaseSignedCommandPayloadBodyStableV2::Payment(payment) => {
                (payment.receiver_pk.to_string(), payment.amount.to_string())
            }
            _ => anyhow::bail!("Expected payment body in signed command"),
        };

        let fee_payer_pk = signed_cmd_v2.payload.common.fee_payer_pk.to_string();

        // Build memo field - omit if empty
        let memo_field = if self.memo.is_empty() {
            String::new()
        } else {
            format!(r#"memo: "{}""#, self.memo)
        };

        // Build GraphQL mutation
        let mutation = format!(
            r#"mutation {{
                sendPayment(
                    input: {{
                        from: "{}"
                        to: "{}"
                        amount: "{}"
                        fee: "{}"
                        {}
                        nonce: "{}"
                        validUntil: "{}"
                    }}
                    signature: {{
                        field: "{}"
                        scalar: "{}"
                    }}
                ) {{
                    payment {{
                        hash
                        id
                    }}
                }}
            }}"#,
            fee_payer_pk,
            receiver_pk,
            amount,
            ***signed_cmd_v2.payload.common.fee,
            memo_field,
            **signed_cmd_v2.payload.common.nonce,
            signed_cmd_v2.payload.common.valid_until.as_u32(),
            sig_field,
            sig_scalar,
        );

        let query = serde_json::json!({
            "query": mutation
        });

        let response = client
            .post(&url)
            .json(&query)
            .send()
            .context("Failed to submit transaction to node")?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            anyhow::bail!(
                "Failed to submit transaction: HTTP {} - {}",
                status,
                error_text
            );
        }

        let response_json: serde_json::Value = response
            .json()
            .context("Failed to parse GraphQL response")?;

        // Check for GraphQL errors
        if let Some(errors) = response_json.get("errors") {
            let error_msg = serde_json::to_string_pretty(errors)
                .unwrap_or_else(|_| "Unknown GraphQL error".to_string());
            anyhow::bail!("GraphQL error: {}", error_msg);
        }

        // Extract transaction hash from response
        let hash = response_json["data"]["sendPayment"]["payment"]["hash"]
            .as_str()
            .context("Transaction hash not found in GraphQL response")?
            .to_string();

        Ok(hash)
    }
}

#[cfg(test)]
mod cli_ref_tests {
    use super::*;

    // CLI-REF: Memo::from_str would cut a longer memo without saying so.
    #[test]
    fn memos_over_32_bytes_are_refused() {
        assert!(check_memo("").is_ok());
        assert!(check_memo(&"a".repeat(32)).is_ok());
        assert!(check_memo(&"a".repeat(33)).is_err());
        // Bytes, not characters: 16 two-byte characters fit, 17 don't.
        assert!(check_memo(&"é".repeat(16)).is_ok());
        assert!(check_memo(&"é".repeat(17)).is_err());
    }
}
