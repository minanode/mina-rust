use mina_core::constants::{ConstraintConstants, ForkConstants};
use serde::{Deserialize, Serialize};

/// Runtime overrides of the compiled network profile. Currency strings are MINA.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/runtime_config/runtime_config.ml#L406-L442>
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Proof {
    pub fork: Option<ForkConstants>,
    pub sub_windows_per_window: Option<u64>,
    pub ledger_depth: Option<u64>,
    pub work_delay: Option<u64>,
    pub block_window_duration_ms: Option<u64>,
    pub transaction_capacity: Option<TransactionCapacity>,
    pub coinbase_amount: Option<String>,
    pub supercharged_coinbase_factor: Option<u64>,
    pub account_creation_fee: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionCapacity {
    #[serde(rename = "2_to_the", alias = "two_to_the", alias = "log_2")]
    pub log_2: Option<u64>,
    pub txns_per_second_x10: Option<u64>,
}

impl Proof {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/genesis_ledger_helper/lib/genesis_ledger_helper_lib.ml#L416-L482>
    pub fn constraint_constants(
        &self,
        default: &ConstraintConstants,
    ) -> Result<ConstraintConstants, String> {
        let mut constants = default.clone();
        macro_rules! override_field {
            ($($field:ident),*) => { $(if let Some(value) = self.$field { constants.$field = value; })* };
        }
        override_field!(
            sub_windows_per_window,
            ledger_depth,
            work_delay,
            block_window_duration_ms,
            supercharged_coinbase_factor
        );
        if let Some(fork) = &self.fork {
            constants.fork = Some(fork.clone());
        }
        if let Some(amount) = &self.coinbase_amount {
            constants.coinbase_amount = currency(amount)?;
        }
        if let Some(fee) = &self.account_creation_fee {
            constants.account_creation_fee = currency(fee)?;
        }
        if let Some(capacity) = &self.transaction_capacity {
            constants.transaction_capacity_log_2 = match (
                capacity.log_2,
                capacity.txns_per_second_x10,
            ) {
                (Some(log_2), None) => log_2,
                (None, Some(tps)) => {
                    let commands = tps
                        .checked_mul(constants.block_window_duration_ms)
                        .ok_or("transaction capacity overflow")?
                        / 10_000;
                    1 + ceil_log2(
                        commands
                            .checked_add(2)
                            .ok_or("transaction capacity overflow")?,
                    )
                }
                _ => return Err(
                    "transaction_capacity requires exactly one of 2_to_the or txns_per_second_x10"
                        .into(),
                ),
            };
        }
        let pending = constants
            .transaction_capacity_log_2
            .checked_add(1)
            .and_then(|capacity| {
                constants
                    .work_delay
                    .checked_add(1)
                    .and_then(|delay| capacity.checked_mul(delay))
            })
            .and_then(|value| value.checked_add(1))
            .ok_or("pending coinbase depth overflow")?;
        constants.pending_coinbase_depth = ceil_log2(pending) as usize;
        if constants.block_window_duration_ms == 0
            || constants.sub_windows_per_window == 0
            || constants.ledger_depth == 0
            || constants.ledger_depth > 63
            || constants.transaction_capacity_log_2 >= 63
            || constants.pending_coinbase_depth >= 63
        {
            return Err(
                "invalid proof constants: zero duration/window/depth or unsupported depth/capacity"
                    .into(),
            );
        }
        Ok(constants)
    }
}

fn ceil_log2(value: u64) -> u64 {
    (u64::BITS - (value - 1).leading_zeros()).into()
}

// Parse decimal MINA without floating point or panics on malformed config.
fn currency(value: &str) -> Result<u64, String> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 9
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(format!("invalid MINA amount: {value}"));
    }
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u64>().map_err(|e| e.to_string())? * 10u64.pow(9 - fraction.len() as u32)
    };
    whole
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(1_000_000_000))
        .and_then(|n| n.checked_add(fraction))
        .ok_or_else(|| format!("MINA amount overflow: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mina_core::network::{devnet, mainnet};
    use serde_json::json;

    #[test]
    fn omitted_values_preserve_both_mesa_profiles() {
        for defaults in [
            &mainnet::CONSTRAINT_CONSTANTS,
            &devnet::CONSTRAINT_CONSTANTS,
        ] {
            assert_eq!(
                Proof::default().constraint_constants(defaults).unwrap(),
                *defaults
            );
        }
    }

    // Expected arithmetic is make_constraint_constants, OCaml 4.0.2 L424-L452:
    // 2 * 90000 / 10000 = 18 commands; 1 + ceil_log2(18 + 2) = 6;
    // ceil_log2((6 + 1) * (2 + 1) + 1) = 5.
    #[test]
    fn ocaml_transaction_capacity_and_derived_pending_depth() {
        let proof: Proof = serde_json::from_value(json!({
            "transaction_capacity": {"txns_per_second_x10": 2},
            "coinbase_amount": "360.000000001", "account_creation_fee": "1.5"
        }))
        .unwrap();
        let constants = proof
            .constraint_constants(&mainnet::CONSTRAINT_CONSTANTS)
            .unwrap();
        assert_eq!(constants.transaction_capacity_log_2, 6);
        assert_eq!(constants.pending_coinbase_depth, 5);
        assert_eq!(constants.coinbase_amount, 360_000_000_001);
        assert_eq!(constants.account_creation_fee, 1_500_000_000);
        for key in ["2_to_the", "two_to_the", "log_2"] {
            let proof: Proof = serde_json::from_value(json!({
                "transaction_capacity": {key: 3}, "work_delay": 1,
                "block_window_duration_ms": 180000, "sub_windows_per_window": 12,
                "ledger_depth": 30, "supercharged_coinbase_factor": 2
            }))
            .unwrap();
            let c = proof
                .constraint_constants(&mainnet::CONSTRAINT_CONSTANTS)
                .unwrap();
            assert_eq!(c.pending_coinbase_depth, 4); // ceil_log2(4 * 2 + 1)
            assert_eq!((c.transaction_capacity_log_2, c.work_delay), (3, 1));
            assert_eq!(
                (c.block_window_duration_ms, c.sub_windows_per_window),
                (180000, 12)
            );
            assert_eq!((c.ledger_depth, c.supercharged_coinbase_factor), (30, 2));
        }
    }

    #[test]
    fn malformed_overrides_are_errors() {
        for value in [
            json!({"transaction_capacity": {}}),
            json!({"transaction_capacity": {"2_to_the": 7, "txns_per_second_x10": 2}}),
            json!({"block_window_duration_ms": 0}),
            json!({"work_delay": u64::MAX}),
            json!({"coinbase_amount": "invalid"}),
            json!({"coinbase_amount": "1.0000000001"}),
            json!({"account_creation_fee": "18446744073709551615"}),
        ] {
            let proof: Proof = serde_json::from_value(value).unwrap();
            assert!(proof
                .constraint_constants(&mainnet::CONSTRAINT_CONSTANTS)
                .is_err());
        }
    }
}
