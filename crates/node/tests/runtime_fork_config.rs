//! Exercise non-default fork metadata through daemon JSON, prebuilt streaming
//! and compact JSON, plus the actual ledger producer/validator stop paths.
use ledger::{staged_ledger::staged_ledger::SkipVerification, BaseLedger};
use mina_core::{
    block::{AppliedBlock, BlockWithHash},
    constants::{constraint_constants, stop_slots},
    NetworkConfig,
};
use mina_node::{
    daemon_json::DaemonJson,
    ledger::LedgerCtx,
    transition_frontier::genesis::{
        GenesisConfig, PrebuiltGenesisConfig, TransitionFrontierGenesisState,
    },
};
use mina_p2p_messages::v2;

#[test]
fn configured_fork_survives_every_format_and_stops_production_and_catchup() {
    NetworkConfig::init("mainnet").unwrap();
    // Deliberately use OCaml 4.0.2 devnet's proof.fork against the mainnet
    // fallback. This cannot pass if the loader ignores runtime overrides.
    let daemon: DaemonJson = serde_json::from_value(serde_json::json!({
        "proof": {"fork": {
            "state_hash": "3NLYmfj4U9Fbbvruz3QJj2j8WJyjhGtq4LgYvo7oW1WZm16uEKib",
            "blockchain_length": 545433, "global_slot_since_genesis": 859560
        }},
        "daemon": {"slot_tx_end": 10, "slot_chain_end": 20},
        "ledger": {"accounts": [], "add_genesis_winner": true}
    }))
    .unwrap();
    let config = GenesisConfig::DaemonJson(Box::new(daemon));
    // `setup_node` calls this boundary before creating either verifier; its
    // error is sent to `run()` and becomes a rejected JavaScript promise.
    let web_error = config.prepare_web_startup().unwrap_err().to_string();
    assert!(
        web_error.contains("invalid runtime genesis config")
            && web_error.contains("web node")
            && web_error.contains("non-release"),
        "{web_error}"
    );
    let (masks, loaded) = config.load().unwrap();
    assert_eq!(
        constraint_constants()
            .fork
            .as_ref()
            .unwrap()
            .blockchain_length,
        545433
    );
    let prebuilt = PrebuiltGenesisConfig::from_loaded((masks, loaded)).unwrap();
    let mut bytes = vec![];
    prebuilt.store(&mut bytes).unwrap();
    let config = GenesisConfig::from_bytes(bytes.clone()).unwrap();
    assert_eq!(
        config
            .runtime_config()
            .unwrap()
            .daemon
            .unwrap()
            .stop_slots(),
        stop_slots()
    );
    let (masks, loaded) = config.load().unwrap(); // streaming loader
    let (_, read_loaded) = PrebuiltGenesisConfig::read(bytes.as_slice())
        .unwrap()
        .load()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        serde_json::to_value(read_loaded).unwrap()
    );
    // The tarball/dump constructor must preserve the same runtime overrides
    // as daemon JSON, including when the ledgers are supplied separately.
    let accounts = |hash: &v2::LedgerHash| {
        masks
            .iter()
            .find(|&mask| mask.clone().merkle_root() == hash.to_field().unwrap())
            .unwrap()
            .to_list()
    };
    let from_accounts = PrebuiltGenesisConfig::from_ledger_accounts(
        *loaded.runtime_config.clone(),
        loaded.constants.clone(),
        (
            accounts(&loaded.genesis_ledger_hash),
            loaded.genesis_ledger_hash.clone(),
        ),
        (
            accounts(&loaded.staking_epoch_ledger_hash),
            loaded.staking_epoch_ledger_hash.clone(),
            loaded.staking_epoch_seed.clone(),
        ),
        (
            accounts(&loaded.next_epoch_ledger_hash),
            loaded.next_epoch_ledger_hash.clone(),
            loaded.next_epoch_seed.clone(),
        ),
    )
    .unwrap();
    let mut bytes = vec![];
    from_accounts.store(&mut bytes).unwrap();
    let (_, from_accounts_loaded) = GenesisConfig::from_bytes(bytes).unwrap().load().unwrap();
    assert_eq!(
        serde_json::to_value(&loaded).unwrap(),
        serde_json::to_value(&from_accounts_loaded).unwrap()
    );
    let from_accounts_compact =
        GenesisConfig::from_bytes(serde_json::to_vec(&from_accounts_loaded).unwrap()).unwrap();
    assert_eq!(
        serde_json::to_value(from_accounts_compact.load().unwrap().1).unwrap(),
        serde_json::to_value(&loaded).unwrap()
    );
    let compact = GenesisConfig::from_bytes(serde_json::to_vec(&loaded).unwrap()).unwrap();
    let (empty, loaded) = compact.load().unwrap();
    assert!(empty.is_empty());
    let (_, genesis, hash) = loaded
        .protocol_states(
            loaded.genesis_total_currency.clone(),
            loaded.staking_epoch_total_currency.clone(),
            loaded.next_epoch_total_currency.clone(),
        )
        .unwrap();
    assert_eq!(
        genesis.previous_state_hash.to_string(),
        "3NLYmfj4U9Fbbvruz3QJj2j8WJyjhGtq4LgYvo7oW1WZm16uEKib"
    );
    assert_eq!(
        genesis.body.consensus_state.blockchain_length.as_u32(),
        545434
    );
    assert_eq!(
        genesis
            .body
            .consensus_state
            .global_slot_since_genesis
            .as_u32(),
        859560
    );
    let block =
        TransitionFrontierGenesisState::build_block_with_dummy_proof(&genesis, &hash).unwrap();
    let parent = AppliedBlock {
        block: block.clone(),
        just_emitted_a_proof: false,
    };
    let producer = genesis.body.consensus_state.block_creator.clone();
    let mut ctx = LedgerCtx::default();
    for mask in masks {
        ctx.insert_genesis_ledger(mask);
    }
    // OCaml block_producer.ml L177-L237: normal until 10, fully empty from
    // 10 through 19, no block from 20. Offset is since-genesis minus fork slot.
    for slot in [9, 10, 11, 19, 20, 21] {
        let output = ctx.staged_ledger_diff_create(
            parent.clone(),
            v2::MinaNumbersGlobalSlotSinceGenesisMStableV1::SinceGenesis((859560u32 + slot).into()),
            false,
            producer.clone(),
            producer.clone(),
            producer.clone(),
            Default::default(),
            false,
            vec![],
        );
        if slot >= 20 {
            assert_eq!(output.unwrap_err(), "reached slot_chain_end");
        } else {
            let output = output.unwrap();
            assert_eq!(
                mina_core::block::prevalidate::staged_ledger_diff_is_empty(&output.diff.diff),
                slot >= 10
            );
            if slot >= 10 {
                // Empty diffs leave account balances unchanged. Genesis uses
                // special zero auxiliary hashes, so the whole staged hash need
                // not match the first post-genesis empty block.
                assert_eq!(
                    output.staged_ledger_hash.non_snark.ledger_hash,
                    parent.staged_ledger_hashes().non_snark.ledger_hash
                );
            }
        }
    }
    // Catch-up's ledger entry rejects stopped blocks even when skipping proofs.
    let mut stopped = (*block.block).clone();
    stopped
        .header
        .protocol_state
        .body
        .consensus_state
        .curr_global_slot_since_hard_fork
        .slot_number = v2::MinaNumbersGlobalSlotSinceHardForkMStableV1::SinceHardFork(20u32.into());
    let stopped = BlockWithHash::try_new(stopped.into()).unwrap();
    let error = ctx
        .block_apply(stopped, parent, Some(SkipVerification::All))
        .unwrap_err();
    assert_eq!(error, "BlockAfterStopSlot");
}
