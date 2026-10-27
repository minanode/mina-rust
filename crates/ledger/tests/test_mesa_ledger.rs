//! Ledger roots under the Mesa account model (32-field zkApp state, transaction
//! version 4), checked against roots computed by the OCaml node 4.0.2.

use mina_p2p_messages::v2::LedgerHash;
use mina_tree::{BaseLedger, Mask};

/// `mina ledger hash` of an empty ledger, from the OCaml 4.0.2 mainnet binary.
const MESA_EMPTY_LEDGER_ROOT: &str = "jwkaDMeSMeL94hwJg6EYtSRD6gxB1BHpXnGKnZ1VW6jthKzgcef";

#[test]
fn empty_ledger_root_matches_ocaml() {
    let mut ledger = Mask::create(35);
    let root = LedgerHash::from_fp(ledger.merkle_root());
    assert_eq!(root.to_string(), MESA_EMPTY_LEDGER_ROOT);
}
