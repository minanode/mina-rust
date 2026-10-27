use anyhow::{Context, Result};
use mina_node::daemon_json::to_mina_string;
use serde_json::Value;

/// A GraphQL amount or fee, which comes in nanomina, as MINA: the unit the
/// wallet commands take. A value that isn't a nanomina count is shown as it
/// came.
fn mina(nanomina: &str) -> String {
    match nanomina.parse::<u64>() {
        Ok(nanomina) => format!("{} MINA", to_mina_string(nanomina)),
        Err(_) => nanomina.to_owned(),
    }
}

#[derive(Debug, clap::Args)]
pub struct Status {
    /// The transaction hash.
    #[arg(long, value_name = "HASH")]
    pub hash: String,

    /// The node's HTTP base URL (its `--port`).
    #[arg(
        long,
        default_value = "http://localhost:3000",
        env = "MINA_NODE_URL",
        value_name = "URL"
    )]
    pub node: String,

    /// Leave out the hint to use this option when the status is UNKNOWN.
    ///
    /// It doesn't change the lookup: the transaction pool is searched only
    /// when the node answers the status query with an error.
    #[arg(long)]
    pub check_mempool: bool,
}

impl Status {
    pub fn run(self) -> Result<()> {
        println!("Checking transaction status...");
        println!("Transaction hash: {}", self.hash);

        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to create HTTP client")?;

        let url = crate::commands::node_api::graphql_url(&self.node);

        // First, try to find the transaction in the best chain
        let query = serde_json::json!({
            "query": format!(
                r#"query {{
                    transactionStatus(payment: "{}")
                }}"#,
                self.hash
            )
        });

        let response = client
            .post(&url)
            .json(&query)
            .send()
            .context("Failed to query transaction status")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to query node: HTTP {}", response.status());
        }

        let response_json: Value = response
            .json()
            .context("Failed to parse GraphQL response")?;

        // Check for GraphQL errors
        if let Some(_errors) = response_json.get("errors") {
            // Transaction might not be found in the chain yet
            // Automatically check mempool as fallback
            println!("\nTransaction not found in blockchain, checking mempool...");
            return self.check_pooled_transactions(&client, &url);
        }

        // Parse transaction status
        if let Some(status) = response_json["data"]["transactionStatus"].as_str() {
            println!("\nTransaction Status: {}", status);

            match status {
                "INCLUDED" => {
                    println!("✓ Transaction has been included in a block");
                }
                "PENDING" => {
                    println!("⏳ Transaction is pending inclusion");
                }
                "UNKNOWN" => {
                    println!("? Transaction status is unknown");
                    if !self.check_mempool {
                        println!(
                            "\nTry using --check-mempool to check if it's in the transaction pool"
                        );
                    }
                }
                _ => {
                    println!("Status: {}", status);
                }
            }
        } else {
            println!("Unable to determine transaction status");
        }

        Ok(())
    }

    fn check_pooled_transactions(
        &self,
        client: &reqwest::blocking::Client,
        url: &str,
    ) -> Result<()> {
        let query = serde_json::json!({
            "query": r#"query {
                pooledUserCommands {
                    hash
                    from
                    to
                    amount
                    fee
                    nonce
                }
            }"#
        });

        let response = client
            .post(url)
            .json(&query)
            .send()
            .context("Failed to query pooled transactions")?;

        if !response.status().is_success() {
            anyhow::bail!("Failed to query mempool: HTTP {}", response.status());
        }

        let response_json: Value = response
            .json()
            .context("Failed to parse GraphQL response")?;

        if let Some(pooled) = response_json["data"]["pooledUserCommands"].as_array() {
            // Look for our transaction in the pool
            for tx in pooled {
                if let Some(hash) = tx["hash"].as_str() {
                    if hash == self.hash {
                        println!("\n✓ Transaction found in mempool!");
                        println!("\nTransaction Details:");
                        println!("  Hash:   {}", hash);
                        if let Some(from) = tx["from"].as_str() {
                            println!("  From:   {}", from);
                        }
                        if let Some(to) = tx["to"].as_str() {
                            println!("  To:     {}", to);
                        }
                        if let Some(amount) = tx["amount"].as_str() {
                            println!("  Amount: {}", mina(amount));
                        }
                        if let Some(fee) = tx["fee"].as_str() {
                            println!("  Fee:    {}", mina(fee));
                        }
                        // Nonce can be either string or number depending on the node
                        if let Some(nonce) = tx["nonce"].as_u64() {
                            println!("  Nonce:  {}", nonce);
                        } else if let Some(nonce) = tx["nonce"].as_str() {
                            println!("  Nonce:  {}", nonce);
                        }

                        println!("\nStatus: PENDING (waiting to be included in a block)");
                        return Ok(());
                    }
                }
            }

            println!("\n✗ Transaction not found in mempool");
            println!("\nThe transaction may have:");
            println!("  - Already been included in a block");
            println!("  - Been rejected by the network");
            println!("  - Not yet propagated to this node");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::mina;

    // CLI-REF fix round 1: the wallet commands agree on MINA.
    #[test]
    fn pooled_amounts_and_fees_are_shown_in_mina() {
        for (nanomina, shown) in [
            ("0", "0 MINA"),
            ("1", "0.000000001 MINA"),
            ("1000000", "0.001 MINA"),
            ("100000000", "0.1 MINA"),
            ("2500000000", "2.5 MINA"),
            ("18446744073709551615", "18446744073.709551615 MINA"),
            ("not-a-number", "not-a-number"),
        ] {
            assert_eq!(mina(nanomina), shown, "{nanomina}");
        }
    }
}
