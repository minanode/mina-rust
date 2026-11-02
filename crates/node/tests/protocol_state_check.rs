//! Captured OCaml mainnet states, with one mutation per rebuilt JSON leaf.
//! Expected states come from published blocks, never from the Rust builder.
use mina_core::consensus::ConsensusConstants;
use mina_node::transition_frontier::protocol_state::check_protocol_state;
use mina_p2p_messages::v2;
use serde::Deserialize;
use serde_json::Value;

type State = v2::MinaStateProtocolStateValueStableV2;
#[derive(Deserialize)]
struct Recorded {
    state_hash: v2::StateHash,
    protocol_state: State,
}
fn pairs() -> Vec<[Recorded; 2]> {
    [
        include_str!("files/protocol-state/mainnet-548149.json"),
        include_str!("files/protocol-state/mainnet-551056.json"),
        include_str!("files/protocol-state/mainnet-557000.json"),
        include_str!("files/protocol-state/mainnet-564336.json"),
    ]
    .into_iter()
    .map(|s| serde_json::from_str(s).unwrap())
    .collect()
}
fn check(pair: &[Recorded; 2], state: &State) -> bool {
    let parent = &pair[0].protocol_state;
    let original = &pair[1].protocol_state;
    let constants = ConsensusConstants::create(
        mina_core::constants::constraint_constants(),
        &parent.body.constants,
    );
    let stmt = &original.body.blockchain_state.ledger_proof_statement;
    check_protocol_state(
        &constants,
        parent,
        &pair[0].state_hash,
        state,
        &original.body.blockchain_state.staged_ledger_hash,
        &original.body.blockchain_state.body_reference,
        (stmt != &parent.body.blockchain_state.ledger_proof_statement).then_some(stmt),
    )
    .is_ok()
}
#[test]
fn captured_states_and_noncanonical_timestamp_pass() {
    let pairs = pairs();
    for pair in &pairs {
        for block in pair {
            assert_eq!(
                v2::StateHash::from(block.protocol_state.try_hash().unwrap()),
                block.state_hash
            );
        }
        assert!(check(pair, &pair[1].protocol_state));
        let mut late = pair[1].protocol_state.clone();
        late.body.blockchain_state.timestamp.0 =
            (late.body.blockchain_state.timestamp.as_u64() + 78_200).into();
        assert!(check(pair, &late), "timestamp is not enforced");
    }
    let state = &pairs[3][1].protocol_state;
    assert_eq!(
        (state.body.blockchain_state.timestamp.as_u64()
            - state.body.constants.genesis_state_timestamp.as_u64())
            % 90_000,
        78_208
    );
}

fn leaves(path: String, value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                leaves(format!("{path}/{key}"), value, out);
            }
        }
        Value::Array(array) if !array.is_empty() => {
            for (i, value) in array.iter().enumerate() {
                leaves(format!("{path}/{i}"), value, out);
            }
        }
        _ => out.push((path, value.clone())),
    }
}
fn producer_input(path: &str, state: &State) -> bool {
    let cs = "/body/consensus_state/";
    [
        "last_vrf_output",
        "block_stake_winner",
        "block_creator",
        "coinbase_receiver",
        "supercharge_coinbase",
    ]
    .iter()
    .any(|field| path.starts_with(&format!("{cs}{field}")))
        || path == "/body/blockchain_state/timestamp"
        || (path == format!("{cs}next_epoch_data/seed") && {
            let slot = state
                .body
                .consensus_state
                .curr_global_slot_since_hard_fork
                .slot_number
                .as_u32();
            let width = state.body.constants.slots_per_epoch.as_u32();
            slot % width < width / 3 * 2
        })
}
#[test]
fn every_rebuilt_leaf_rejects_and_producer_inputs_pass() {
    let pairs = pairs();
    let mut alternatives = Vec::new();
    for pair in &pairs {
        for block in pair {
            leaves(
                String::new(),
                &serde_json::to_value(&block.protocol_state).unwrap(),
                &mut alternatives,
            );
        }
    }
    let mut rejected = 0;
    let mut accepted = 0;
    for pair in &pairs {
        let state = &pair[1].protocol_state;
        let value = serde_json::to_value(state).unwrap();
        let mut fields = Vec::new();
        leaves(String::new(), &value, &mut fields);
        for (path, old) in fields {
            // Unit SOK digest is the only field with no alternate value.
            if old.is_null() {
                continue;
            }
            let new = match &old {
                Value::Array(array) if array.is_empty() => serde_json::json!([[]]),
                Value::Bool(b) => Value::Bool(!b),
                Value::Number(n) => Value::from(n.as_u64().unwrap() ^ 1),
                Value::String(s) => {
                    if let Ok(n) = s.parse::<u64>() {
                        Value::String((n ^ 1).to_string())
                    } else if s == "Pos" {
                        Value::String("Neg".to_owned())
                    } else if s == "Neg" {
                        Value::String("Pos".to_owned())
                    } else if path.ends_with("/token") {
                        serde_json::to_value(v2::TokenIdKeyHash::from(
                            v2::MinaBaseAccountIdDigestStableV1(
                                mina_curves::pasta::Fp::from(0).into(),
                            ),
                        ))
                        .unwrap()
                    } else if s.starts_with("0x") {
                        let mut bytes = s.as_bytes().to_vec();
                        let last = bytes.last_mut().unwrap();
                        *last = if *last == b'0' { b'1' } else { b'0' };
                        Value::String(String::from_utf8(bytes).unwrap())
                    } else {
                        alternatives
                            .iter()
                            .find(|(p, v)| p == &path && v != &old)
                            .or_else(|| {
                                alternatives.iter().find(|(_, v)| {
                                    v.as_str()
                                        .is_some_and(|v| v.len() == s.len() && v[..2] == s[..2])
                                        && v != &old
                                })
                            })
                            .unwrap_or_else(|| panic!("no alternative for {path}: {old}"))
                            .1
                            .clone()
                    }
                }
                _ => panic!("unhandled leaf {path}: {old}"),
            };
            let mut mutated = value.clone();
            *mutated.pointer_mut(&path).unwrap() = new;
            let candidate: State = serde_json::from_value(mutated)
                .unwrap_or_else(|e| panic!("invalid mutation {path}: {e}"));
            // The chosen slot is an input, but changing it alone also changes
            // derived fields, so this mutation must still be rejected.
            let expected_accept = producer_input(&path, state);
            assert_eq!(
                check(pair, &candidate),
                expected_accept,
                "height {} mutation {path}",
                state.body.consensus_state.blockchain_length.as_u32()
            );
            if expected_accept {
                accepted += 1;
            } else {
                rejected += 1;
            }
            println!(
                "height={} field={path} accepted={expected_accept}",
                state.body.consensus_state.blockchain_length.as_u32()
            );
        }
    }
    println!("rejected={rejected} accepted_producer_inputs={accepted}");
}

#[test]
fn genesis_ledger_hash_is_rejected_explicitly() {
    for pair in pairs() {
        let mut state = pair[1].protocol_state.clone();
        state.body.blockchain_state.genesis_ledger_hash = v2::LedgerHash::zero();
        assert!(!check(&pair, &state));
    }
}
