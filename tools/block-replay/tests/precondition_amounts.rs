//! The amounts in zkApp preconditions come in nanomina in bucket JSON, unlike
//! balance changes, which come in decimal MINA. Two Mesa devnet blocks from
//! the public bucket, trimmed to their
//! protocol state and staged ledger diff without completed works:
//!
//! - 556601 `3NKn36tfGQKDTfdzY6hK96SJsrpFi6emB6TGEWAPpNpxYLuYdGW9`: an account
//!   balance check, `50000000`;
//! - 561628 `3NLqMKsmsDBqw31xuqbMGvJBxfYmM2sJLXrp7JCqQy3REBT9W2j7`: a staking
//!   epoch total currency check, `1592921154000001000`, which as MINA would
//!   not fit in 64 bits.
//!
//! Reading a block checks every account update against the digest OCaml wrote
//! next to it, so a block reads only if each amount was read as OCaml meant.

use mina_block_replay::parse_block;

const BALANCE: &str = include_str!("fixtures/devnet-556601-account-balance.json");
const TOTAL_CURRENCY: &str = include_str!("fixtures/devnet-561628-staking-total-currency.json");

/// The account update digests depend on the network: devnet's zkApp body
/// prefix. Without an explicit init, the global config is devnet's too.
fn devnet() {
    let _ = mina_core::NetworkConfig::init("devnet");
    assert_eq!(mina_core::NetworkConfig::global().name, "devnet");
}

#[test]
fn precondition_amounts_read_as_nanomina() {
    devnet();
    for (name, block) in [("556601", BALANCE), ("561628", TOTAL_CURRENCY)] {
        if let Err(error) = parse_block(block.as_bytes()) {
            panic!("block {name}: {error:#}");
        }
    }
}

#[test]
fn a_changed_precondition_amount_fails_the_digest_check() {
    devnet();
    for (block, from, to) in [
        (
            BALANCE,
            r#""balance":["Check",{"lower":"50000000""#,
            r#""balance":["Check",{"lower":"50000001""#,
        ),
        (
            TOTAL_CURRENCY,
            r#""lower":"1592921154000001000""#,
            r#""lower":"1592921154000001001""#,
        ),
    ] {
        let changed = block.replacen(from, to, 1);
        assert_ne!(changed, block, "{from} is not in the fixture");
        let error = parse_block(changed.as_bytes())
            .err()
            .unwrap_or_else(|| panic!("{to} instead of {from} still read"));
        assert!(
            format!("{error:#}").contains("reads differently"),
            "{error:#}"
        );
    }
}
