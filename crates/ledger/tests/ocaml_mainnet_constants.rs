//! P11d: mina-tree's hard-coded genesis constants against OCaml 4.0.2's
//! mainnet values, from `tests/files/ocaml/mainnet-4.0.2-constants.json`
//! (written by `scripts/ocaml-constants-fixture.py`). The rest of the fixture
//! is checked by `crates/core/tests/ocaml_mainnet_constants.rs`, whose
//! `CHECKED_IN_MINA_TREE` lists the names compared here.

use std::collections::BTreeMap;

use mina_tree::scan_state::GENESIS_CONSTANT;
use serde_json::{json, Value};

const FIXTURE: &str = include_str!("../../../tests/files/ocaml/mainnet-4.0.2-constants.json");

#[test]
fn genesis_constants_match_ocaml() {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("fixture should parse");
    assert_eq!(fixture["profile"], "mainnet");
    let constants: BTreeMap<String, Vec<Value>> =
        serde_json::from_value(fixture["constants"].clone()).unwrap();

    let ours = [
        ("txpool_max_size", json!(GENESIS_CONSTANT.txpool_max_size)),
        (
            "max_zkapp_segment_per_transaction",
            json!(GENESIS_CONSTANT.max_zkapp_segment_per_transaction),
        ),
        (
            "max_event_elements",
            json!(GENESIS_CONSTANT.max_event_elements),
        ),
        (
            "max_action_elements",
            json!(GENESIS_CONSTANT.max_action_elements),
        ),
        (
            "zkapp_cmd_limit_hardcap",
            json!(GENESIS_CONSTANT.zkapp_cmd_limit_hardcap),
        ),
        (
            "minimum_user_command_fee",
            json!(GENESIS_CONSTANT.minimum_user_command_fee.as_u64()),
        ),
    ];

    let mut mismatches = Vec::new();
    for (name, value) in ours {
        let observations = constants
            .get(name)
            .unwrap_or_else(|| panic!("{name} is missing from the fixture: regenerate it"));
        for observation in observations {
            let ok = observation["value"] == value;
            println!(
                "{} {name} = {value} (GENESIS_CONSTANT); OCaml {} from {}",
                if ok { "ok      " } else { "MISMATCH" },
                observation["value"],
                observation["how"].as_str().unwrap_or_default(),
            );
            if !ok {
                mismatches.push(format!(
                    "{name}: ours {value}, OCaml {} ({})",
                    observation["value"],
                    observation["how"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches with OCaml:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}
