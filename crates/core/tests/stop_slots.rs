//! Expected rules: OCaml 4.0.2 validator.ml L42-L106, diff.ml L512-L524
//! and block_producer.ml L177-L237 (all at 84b8d969).
use mina_core::{block::prevalidate::*, constants::StopSlots};
use mina_p2p_messages::v2::{
    self, StagedLedgerDiffDiffPreDiffWithAtMostTwoCoinbaseStableV2Coinbase as Coinbase,
};

fn empty() -> v2::StagedLedgerDiffDiffDiffStableV2 {
    v2::StagedLedgerDiffDiffDiffStableV2(
        v2::StagedLedgerDiffDiffPreDiffWithAtMostTwoCoinbaseStableV2 {
            completed_works: std::iter::empty().collect(),
            commands: std::iter::empty().collect(),
            coinbase: Coinbase::Zero,
            internal_command_statuses: std::iter::empty().collect(),
        },
        None,
    )
}

#[test]
fn inclusive_stop_boundaries_and_production_policy() {
    let stops = StopSlots {
        slot_tx_end: Some(10),
        slot_chain_end: Some(20),
        ..Default::default()
    };
    let mut coinbase = empty();
    coinbase.0.coinbase = Coinbase::One(None);
    for (slot, tx, blocks) in [
        (9, true, true),
        (10, false, true),
        (11, false, true),
        (19, false, true),
        (20, false, false),
        (21, false, false),
    ] {
        assert_eq!(stops.transactions_allowed(slot), tx);
        assert_eq!(stops.blocks_allowed(slot), blocks);
        assert_eq!(
            validate_stop_slots_diff(slot, &empty(), stops).is_ok(),
            blocks
        );
        assert_eq!(
            validate_stop_slots_diff(slot, &coinbase, stops).is_ok(),
            tx && blocks
        );
    }
    for slot in [0, 10, 20, u32::MAX] {
        assert!(validate_stop_slots_diff(slot, &coinbase, StopSlots::default()).is_ok());
    }
    assert!(matches!(
        validate_stop_slots_diff(
            0,
            &empty(),
            StopSlots {
                slot_tx_end: None,
                slot_chain_end: Some(0),
                ..Default::default()
            }
        ),
        Err(BlockPrevalidationError::BlockAfterStopSlot)
    ));
    assert!(matches!(
        validate_stop_slots_diff(
            0,
            &coinbase,
            StopSlots {
                slot_tx_end: Some(0),
                slot_chain_end: None,
                ..Default::default()
            }
        ),
        Err(BlockPrevalidationError::NonEmptyStagedLedgerDiffAfterStopSlot)
    ));
}

#[test]
fn empty_means_every_component_absent() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../web-verifier/tests/fixtures/mainnet-560000.json"
    ))
    .unwrap();
    let diff: v2::StagedLedgerDiffDiffStableV2 =
        serde_json::from_value(fixture["data"]["staged_ledger_diff"].clone()).unwrap();
    let real = &diff.diff;
    assert!(!real.0.commands.is_empty());
    let work: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/files/ocaml/stop-slots-work.json"
    ))
    .unwrap();
    let work: v2::TransactionSnarkWorkTStableV2 =
        serde_json::from_value(work["work"].clone()).unwrap();
    let mut variants = Vec::new();
    let mut diff = empty();
    diff.0.commands = real.0.commands.clone();
    variants.push(diff);
    let mut diff = empty();
    diff.0.completed_works = std::iter::once(work).collect();
    variants.push(diff);
    let mut diff = empty();
    diff.0.coinbase = Coinbase::One(None);
    variants.push(diff);
    let mut diff = empty();
    diff.0.coinbase = Coinbase::Two(None);
    variants.push(diff);
    let mut diff = empty();
    diff.0.internal_command_statuses = vec![v2::MinaBaseTransactionStatusStableV2::Applied]
        .into_iter()
        .collect();
    variants.push(diff);
    let mut diff = empty();
    diff.1 = Some(
        v2::StagedLedgerDiffDiffPreDiffWithAtMostOneCoinbaseStableV2 {
            completed_works: std::iter::empty().collect(),
            commands: std::iter::empty().collect(),
            coinbase: v2::StagedLedgerDiffDiffPreDiffWithAtMostOneCoinbaseStableV2Coinbase::Zero,
            internal_command_statuses: std::iter::empty().collect(),
        },
    );
    variants.push(diff);
    let stops = StopSlots {
        slot_tx_end: Some(10),
        slot_chain_end: None,
        ..Default::default()
    };
    assert!(staged_ledger_diff_is_empty(&empty()));
    for diff in variants {
        assert!(!staged_ledger_diff_is_empty(&diff));
        assert!(validate_stop_slots_diff(9, &diff, stops).is_ok());
        assert!(matches!(
            validate_stop_slots_diff(10, &diff, stops),
            Err(BlockPrevalidationError::NonEmptyStagedLedgerDiffAfterStopSlot)
        ));
    }
}
