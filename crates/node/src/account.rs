pub use mina_node_account::*;

/// Validate an optional web-node coinbase receiver before starting the node.
pub fn parse_coinbase_receiver(
    receiver: Option<&str>,
    has_block_producer: bool,
) -> Result<Option<AccountPublicKey>, &'static str> {
    let Some(receiver) = receiver else {
        return Ok(None);
    };
    if !has_block_producer {
        return Err("coinbase receiver requires a block producer key");
    }
    receiver
        .parse()
        .map(Some)
        .map_err(|_| "invalid coinbase receiver")
}

/// OCaml's `default_snark_worker_fee` on the mainnet and devnet profiles.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml#L33>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/devnet.ml#L33>
pub const DEFAULT_SNARKER_FEE: &str = "0.1";

/// A fee in MINA, as OCaml's `Currency.Fee.of_mina_string_exn` reads it: whole
/// and decimal parts, at most 9 decimals kept (more are cut, not rounded), the
/// result in nanomina. Stricter than OCaml where OCaml is lenient: a value
/// with no digit (OCaml reads `""` and `"."` as 0), a sign or a space is
/// refused. The SNARK worker's `--snarker-fee`, and the web node's (P5.3).
/// No message holds the value: an operator may paste a secret into it (P5.2's
/// review).
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L98-L113>
pub fn parse_snarker_fee(value: &str) -> Result<u64, &'static str> {
    const PRECISION: usize = 9;
    const INVALID_FEE: &str = "invalid fee: expected MINA, like 0.1";
    let digits = match value.split('.').collect::<Vec<_>>()[..] {
        [whole] => format!("{whole}{}", "0".repeat(PRECISION)),
        [whole, decimal] => {
            let decimal = decimal.get(..PRECISION).unwrap_or(decimal);
            let padding = PRECISION.checked_sub(decimal.len()).ok_or(INVALID_FEE)?;
            format!("{whole}{decimal}{}", "0".repeat(padding))
        }
        _ => return Err(INVALID_FEE),
    };
    let has_digit = value.bytes().any(|b| b.is_ascii_digit());
    if !has_digit || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(INVALID_FEE);
    }
    digits
        .parse()
        .map_err(|_| "fee does not fit in 64 bits of nanomina")
}

/// A SNARK worker's public key, as OCaml's `public_key_compressed` argument:
/// base58check and a point on the curve. A private key gets a hint, not a
/// key error. No message holds the value: before P5.2 the native node's
/// `--run-snarker` took the worker's private key, so a value given out of
/// habit may be one, and it must not be repeated in an error, a log or a
/// page (R35, P5.2's review).
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/cli_lib/arg_type.ml#L31-L45>
pub fn parse_snarker_public_key(value: &str) -> Result<AccountPublicKey, &'static str> {
    if value.starts_with("EK") {
        return Err(
            "this looks like a private key: the SNARK worker takes the public key (B62...) \
             that receives the fees, as OCaml's --run-snark-worker does",
        );
    }
    mina_signer::PubKey::from_address(value)
        .map_err(|_| "not a valid public key: expected B62...")?;
    value
        .parse()
        .map_err(|_| "not a valid public key: expected B62...")
}

/// The web node's SNARK worker settings (P5.3): its public key, and its fee
/// in MINA (default [`DEFAULT_SNARKER_FEE`]), as nanomina. No key, no
/// worker; a fee without a key is refused. Only a public key is taken: no
/// message holds the key given ([`parse_snarker_public_key`]).
pub fn parse_snarker(
    public_key: Option<&str>,
    fee: Option<&str>,
) -> Result<Option<(AccountPublicKey, u64)>, String> {
    let Some(public_key) = public_key.filter(|key| !key.is_empty()) else {
        return match fee {
            Some(_) => Err("a SNARK worker fee needs a SNARK worker public key".to_owned()),
            None => Ok(None),
        };
    };
    let public_key = parse_snarker_public_key(public_key)
        .map_err(|error| format!("SNARK worker key: {error}"))?;
    let fee = parse_snarker_fee(fee.unwrap_or(DEFAULT_SNARKER_FEE))?;
    Ok(Some((public_key, fee)))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_coinbase_receiver, parse_snarker, parse_snarker_fee, parse_snarker_public_key,
    };

    // Public devnet receiver used by P4.10 and P4.14 (not a generated fixture).
    const DEVNET_RECEIVER: &str = "B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV";

    #[test]
    fn coinbase_receiver_valid_devnet_key() {
        let receiver = parse_coinbase_receiver(Some(DEVNET_RECEIVER), true)
            .unwrap()
            .unwrap();
        assert_eq!(receiver.to_string(), DEVNET_RECEIVER);
    }

    #[test]
    fn coinbase_receiver_malformed() {
        assert_eq!(
            parse_coinbase_receiver(Some("not-a-public-key"), true),
            Err("invalid coinbase receiver")
        );
    }

    #[test]
    fn coinbase_receiver_empty() {
        assert_eq!(
            parse_coinbase_receiver(Some(""), true),
            Err("invalid coinbase receiver")
        );
    }

    #[test]
    fn coinbase_receiver_omitted() {
        assert_eq!(parse_coinbase_receiver(None, false), Ok(None));
        assert_eq!(parse_coinbase_receiver(None, true), Ok(None));
    }

    #[test]
    fn coinbase_receiver_requires_producer() {
        assert_eq!(
            parse_coinbase_receiver(Some(DEVNET_RECEIVER), false),
            Err("coinbase receiver requires a block producer key")
        );
    }

    #[test]
    fn snarker_fee_is_read_as_ocaml_reads_mina() {
        // Expected values from `Currency.Fee.of_mina_string_exn`.
        for (input, nanomina) in [
            ("0.1", 100_000_000),
            ("1", 1_000_000_000),
            ("1.", 1_000_000_000),
            (".5", 500_000_000),
            ("0.000000001", 1),
            ("0.0000000019", 1),
            ("2.25", 2_250_000_000),
            ("18446744073.709551615", u64::MAX),
        ] {
            assert_eq!(parse_snarker_fee(input), Ok(nanomina), "{input}");
        }
        for input in [
            "",
            ".",
            "1.2.3",
            "-1",
            "0x10",
            "1e9",
            " 1",
            "0.12345678é",
            "18446744073.709551616",
        ] {
            assert!(parse_snarker_fee(input).is_err(), "{input:?} accepted");
        }
    }

    #[test]
    fn snarker_public_key_is_a_public_key_and_never_echoed() {
        // The public genesis winner of Mina's ledgers, a well-known key.
        const PUBLIC_KEY: &str = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
        assert_eq!(
            parse_snarker_public_key(PUBLIC_KEY).unwrap().to_string(),
            PUBLIC_KEY
        );
        // The well-known private key 1, never a funded key; and a private
        // key that is not even valid, which must not be echoed either.
        let secret = mina_node_account::AccountSecretKey::deterministic(1).to_string();
        let mut broken = secret.clone();
        broken.replace_range(10..11, "x");
        for value in [secret.as_str(), broken.as_str()] {
            let error = parse_snarker_public_key(value).unwrap_err();
            assert!(error.contains("looks like a private key"), "{error}");
            assert!(!error.contains(&value[2..12]), "the value was echoed");
        }
        let mut corrupted = PUBLIC_KEY.to_owned();
        corrupted.replace_range(10..11, "x");
        let error = parse_snarker_public_key(&corrupted).unwrap_err();
        assert!(!error.contains(&corrupted[2..12]), "the value was echoed");
        assert!(parse_snarker_public_key("").is_err());
    }

    #[test]
    fn web_snarker_takes_only_a_public_key_and_a_fee_in_mina() {
        const PUBLIC_KEY: &str = "B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg";
        let (key, fee) = parse_snarker(Some(PUBLIC_KEY), None).unwrap().unwrap();
        assert_eq!(key.to_string(), PUBLIC_KEY);
        assert_eq!(fee, 100_000_000, "OCaml's default, 0.1 MINA");
        let (_, fee) = parse_snarker(Some(PUBLIC_KEY), Some("1.5"))
            .unwrap()
            .unwrap();
        assert_eq!(fee, 1_500_000_000);
        assert_eq!(parse_snarker(None, None), Ok(None));
        assert_eq!(parse_snarker(Some(""), None), Ok(None));
        assert!(parse_snarker(None, Some("0.1")).is_err());
        assert!(parse_snarker(Some(PUBLIC_KEY), Some("-1")).is_err());

        let secret = mina_node_account::AccountSecretKey::deterministic(2).to_string();
        let error = parse_snarker(Some(&secret), Some("0.1")).unwrap_err();
        assert!(error.contains("looks like a private key"), "{error}");
        assert!(
            !error.contains(&secret[2..12]),
            "the private key was echoed"
        );
    }
}
