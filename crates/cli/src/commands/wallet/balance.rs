use anyhow::Context;
use mina_node_account::AccountSecretKey;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub struct Balance {
    /// The account's public key.
    #[arg(long, conflicts_with = "from", value_name = "PUBLIC_KEY")]
    pub address: Option<String>,

    /// An encrypted key file whose account to query, in place of
    /// `--address`. Its password is read from the `MINA_PRIVKEY_PASS`
    /// environment variable.
    #[arg(long, conflicts_with = "address", value_name = "FILE")]
    pub from: Option<PathBuf>,

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

    /// The node's HTTP base URL (its `--port`). Requests go to
    /// `<URL>/graphql`.
    #[arg(
        long,
        alias = "endpoint",
        default_value = "http://localhost:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: String,

    /// The output format.
    #[arg(long, default_value = "text")]
    pub format: OutputFormat,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Serialize)]
struct GraphQLRequest {
    query: String,
    variables: serde_json::Value,
}

#[derive(Deserialize, Debug)]
struct GraphQLResponse {
    data: Option<DataResponse>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Deserialize, Debug)]
struct DataResponse {
    account: Option<AccountResponse>,
}

#[derive(Deserialize, Debug)]
struct AccountResponse {
    balance: BalanceResponse,
    nonce: String,
    #[serde(rename = "delegateAccount")]
    delegate_account: Option<DelegateAccount>,
}

#[derive(Deserialize, Debug)]
struct BalanceResponse {
    total: String,
    liquid: Option<String>,
    locked: Option<String>,
}

#[derive(Deserialize, Debug)]
struct DelegateAccount {
    #[serde(rename = "publicKey")]
    public_key: String,
}

#[derive(Deserialize, Debug)]
struct GraphQLError {
    message: String,
}

#[derive(Serialize, Debug)]
struct BalanceOutput {
    account: String,
    balance: BalanceOutputData,
    nonce: String,
    delegate: Option<String>,
}

#[derive(Serialize, Debug)]
struct BalanceOutputData {
    total: String,
    total_mina: String,
    liquid: Option<String>,
    liquid_mina: Option<String>,
    locked: Option<String>,
    locked_mina: Option<String>,
}

impl Balance {
    pub fn run(self) -> anyhow::Result<()> {
        debug_assert!(self.password.is_none() && self.password_flag.is_none());
        let password = crate::commands::secret_value::password_from_env("MINA_PRIVKEY_PASS")?;
        // Get the public key either from address or from key file
        let public_key = if let Some(address) = self.address {
            address
        } else if let Some(from) = self.from {
            if password.is_empty() {
                anyhow::bail!(
                    "Password is required when using --from. Provide it via the MINA_PRIVKEY_PASS environment variable"
                );
            }
            let secret_key = AccountSecretKey::from_encrypted_file(&from, &password)
                .with_context(|| format!("Failed to decrypt key file: {}", from.display()))?;
            secret_key.public_key().to_string()
        } else {
            anyhow::bail!("Either --address or --from must be provided to specify the account");
        };

        // GraphQL query
        let query = r#"
            query GetBalance($publicKey: PublicKey!) {
                account(publicKey: $publicKey) {
                    balance {
                        total
                        liquid
                        locked
                    }
                    nonce
                    delegateAccount {
                        publicKey
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "publicKey": public_key
        });

        let request = GraphQLRequest {
            query: query.to_string(),
            variables,
        };

        // Make the GraphQL request
        let client = reqwest::blocking::Client::new();
        let response = client
            .post(crate::commands::node_api::graphql_url(&self.node))
            .json(&request)
            .send()
            .with_context(|| format!("Failed to connect to the node at {}", self.node))?;

        if !response.status().is_success() {
            anyhow::bail!("GraphQL request failed with status: {}", response.status());
        }

        let graphql_response: GraphQLResponse = response
            .json()
            .context("Failed to parse GraphQL response")?;

        // Check for GraphQL errors
        if let Some(errors) = graphql_response.errors {
            let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
            anyhow::bail!("GraphQL errors: {}", error_messages.join(", "));
        }

        // Extract account data
        let account = graphql_response
            .data
            .and_then(|d| d.account)
            .with_context(|| format!("Account not found: {}", public_key))?;

        // Create output structure
        let output = BalanceOutput {
            account: public_key.clone(),
            balance: BalanceOutputData {
                total: account.balance.total.clone(),
                total_mina: format_balance(&account.balance.total),
                liquid: account.balance.liquid.clone(),
                liquid_mina: account.balance.liquid.as_ref().map(|l| format_balance(l)),
                locked: account.balance.locked.clone(),
                locked_mina: account.balance.locked.as_ref().map(|l| format_balance(l)),
            },
            nonce: account.nonce.clone(),
            delegate: account
                .delegate_account
                .as_ref()
                .map(|d| d.public_key.clone()),
        };

        // Display the balance information based on format
        match self.format {
            OutputFormat::Json => {
                let json = serde_json::to_string_pretty(&output)
                    .context("Failed to serialize output to JSON")?;
                println!("{}", json);
            }
            OutputFormat::Text => {
                println!("Account: {}", output.account);
                println!();
                println!("Balance:");
                println!("  Total:  {} MINA", output.balance.total_mina);

                if let Some(liquid_mina) = &output.balance.liquid_mina {
                    println!("  Liquid: {} MINA", liquid_mina);
                }

                if let Some(locked_mina) = &output.balance.locked_mina {
                    println!("  Locked: {} MINA", locked_mina);
                }

                println!();
                println!("Nonce: {}", output.nonce);

                if let Some(delegate) = &output.delegate {
                    println!();
                    println!("Delegate: {}", delegate);
                }
            }
        }

        Ok(())
    }
}

fn format_balance(nanomina: &str) -> String {
    // Convert nanomina to MINA (1 MINA = 1,000,000,000 nanomina)
    let nano = nanomina.parse::<u64>().unwrap_or(0);
    let mina = nano as f64 / 1_000_000_000.0;
    format!("{:.9}", mina)
}
