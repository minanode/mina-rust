use super::*;
use mina_core::block::BlockWithHash;
use mina_p2p_messages::{binprot::BinProtRead, v2::MinaBlockBlockStableV2};
use std::sync::Arc;

fn live_block() -> ArcBlockWithHash {
    // OCaml mainnet gossip fixture, also used by P14's body/delta checks.
    let bytes = include_bytes!("../../../web-verifier/tests/fixtures/mainnet-563978.gossipbin");
    BlockWithHash::try_new(Arc::new(
        MinaBlockBlockStableV2::binprot_read(&mut &bytes[9..]).unwrap(),
    ))
    .unwrap()
}
fn time() -> Timestamp {
    Timestamp::new(1_000_000_000)
}
fn proof(block: BlockIdentity, status: CheckStatus, ms: f64) -> VerificationUpdate {
    VerificationUpdate::Proof {
        block,
        status,
        elapsed_ms: Some(ms),
        verifier_key: None,
    }
}

#[test]
fn applied_bootstrap_root_has_no_individual_checks() {
    let mut state = VerificationState::default();
    state.applied(&live_block(), time(), false);
    let snapshot = state.snapshot(VerificationQuery::default());
    let receipt = &snapshot.receipts[0];
    assert!(receipt.applied);
    assert_eq!(receipt.arrival, Arrival::Bootstrap);
    assert_eq!(receipt.block_proof.status, CheckStatus::NotRun);
    assert_eq!(receipt.delta_chain.status, CheckStatus::NotRun);
    assert_eq!(receipt.body_reference.status, CheckStatus::NotRun);
    assert_eq!(snapshot.verified_since_start, 0);
    assert_eq!(snapshot.applied_since_start, 0);
    assert!(snapshot.median_proof_time_ms.is_none());
}

#[test]
fn fetched_and_applied_block_is_not_proof_verified() {
    let mut state = VerificationState::default();
    let block = live_block();
    let result = mina_core::block::prevalidate::validate_delta_block_chain(block.header())
        .and_then(|()| {
            crate::transition_frontier::body_reference::validate_body_reference(&block.block)
        });
    state.record(
        &VerificationUpdate::headers(&block, Arrival::Bootstrap, &result),
        time(),
    );
    state.applied(&block, time(), true);
    state.applied(&block, time(), true);
    assert_eq!(state.summary().applied_since_start, 1);
    let receipt = &state.snapshot(VerificationQuery::default()).receipts[0];
    assert_eq!(receipt.block_proof.status, CheckStatus::NotRun);
    assert_eq!(receipt.delta_chain.status, CheckStatus::Passed);
    assert_eq!(receipt.body_reference.status, CheckStatus::Passed);
}

#[test]
fn short_circuit_does_not_invent_checks_or_keep_raw_errors() {
    let block = live_block();
    for (error, delta, body) in [
        (
            BlockPrevalidationError::GenesisNotReady,
            CheckStatus::NotRun,
            CheckStatus::NotRun,
        ),
        (
            BlockPrevalidationError::InvalidDeltaBlockChainProof,
            CheckStatus::Failed,
            CheckStatus::NotRun,
        ),
        (
            BlockPrevalidationError::InvalidProtocolVersion,
            CheckStatus::Passed,
            CheckStatus::NotRun,
        ),
        (
            BlockPrevalidationError::InvalidBodyReference {
                expected: "raw-private-data".into(),
                got: "raw-private-data".into(),
            },
            CheckStatus::Passed,
            CheckStatus::Failed,
        ),
    ] {
        let mut state = VerificationState::default();
        state.record(
            &VerificationUpdate::headers(&block, Arrival::Gossip, &Err(error)),
            time(),
        );
        let snapshot = state.snapshot(VerificationQuery::default());
        assert_eq!(snapshot.receipts[0].delta_chain.status, delta);
        assert_eq!(snapshot.receipts[0].body_reference.status, body);
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("raw-private-data"));
    }
}

#[test]
fn skipped_or_failed_proofs_never_increase_verified_count() {
    for status in [
        CheckStatus::Pending,
        CheckStatus::Skipped,
        CheckStatus::Failed,
    ] {
        let mut state = VerificationState::default();
        state.record(
            &proof(BlockIdentity::new(&live_block()), status, 4.0),
            time(),
        );
        let snapshot = state.snapshot(VerificationQuery::default());
        assert_eq!(snapshot.verified_since_start, 0);
        assert!(snapshot.last_verified.is_none());
        assert!(snapshot.median_proof_time_ms.is_none());
        assert_eq!(
            snapshot.failures_since_start,
            u64::from(status == CheckStatus::Failed)
        );
    }
}

#[test]
fn records_failures_and_timing_samples_are_bounded_counters_survive_eviction() {
    let mut state = VerificationState::default();
    // Synthetic identities exercise retention, not consensus hash expectations.
    for i in 0..RECEIPT_CAPACITY + 5 {
        let block = BlockIdentity {
            state_hash: StateHash::from_fp((i as u64).into()),
            height: i as u32,
        };
        state.record(&proof(block.clone(), CheckStatus::Failed, 0.0), time());
        state.record(&proof(block, CheckStatus::Passed, i as f64), time());
    }
    let snapshot = state.snapshot(VerificationQuery::default());
    assert_eq!(snapshot.receipts.len(), DEFAULT_RECEIPT_PAGE_SIZE);
    assert_eq!(state.order.len(), RECEIPT_CAPACITY);
    assert_eq!(state.proof_times.len(), RECEIPT_CAPACITY);
    assert_eq!(snapshot.recent_failure_count, FAILURE_CAPACITY);
    assert_eq!(state.sorted_proof_times.len(), RECEIPT_CAPACITY);
    assert_eq!(snapshot.verified_since_start, 1029);
    assert_eq!(snapshot.failures_since_start, 1029);
    assert_eq!(snapshot.proof_failures_since_start, 1029);
    assert_eq!(snapshot.proof_time_count, 1029);
    assert_eq!(snapshot.proof_time_buckets[0], 51);
    // Retained samples 5..=1028: arithmetic median = (516 + 517) / 2.
    assert_eq!(snapshot.median_proof_time_ms, Some(516.5));
    assert!(!state.records.contains_key(&StateHash::from_fp(0u64.into())));
}

#[test]
fn rejected_payload_cannot_erase_valid_evidence_or_double_count() {
    let mut state = VerificationState::default();
    let block = live_block();
    state.record(
        &VerificationUpdate::headers(&block, Arrival::Gossip, &Ok(())),
        time(),
    );
    state.record(
        &proof(BlockIdentity::new(&block), CheckStatus::Passed, 142.0),
        time(),
    );
    state.record(
        &proof(BlockIdentity::new(&block), CheckStatus::Passed, 142.0),
        time(),
    );
    state.record(
        &VerificationUpdate::headers(
            &block,
            Arrival::Gossip,
            &Err(BlockPrevalidationError::InvalidBodyReference {
                expected: String::new(),
                got: String::new(),
            }),
        ),
        time(),
    );
    state.applied(&block, time(), true);
    let snapshot = state.snapshot(VerificationQuery::default());
    assert_eq!(snapshot.receipts[0].block_proof.status, CheckStatus::Passed);
    assert_eq!(
        snapshot.receipts[0].body_reference.status,
        CheckStatus::Passed
    );
    assert_eq!(snapshot.receipts[0].arrival, Arrival::Gossip);
    assert_eq!(snapshot.verified_since_start, 1);
    assert_eq!(snapshot.failures_since_start, 1);
}

#[test]
fn last_verified_tracks_later_application() {
    let block = live_block();
    let mut state = VerificationState::default();
    state.record(
        &proof(BlockIdentity::new(&block), CheckStatus::Passed, 12.0),
        time(),
    );
    assert!(!state.summary().last_verified.unwrap().applied);
    state.applied(&block, time(), true);
    let summary = state.summary();
    let last = summary.last_verified.unwrap();
    assert!(last.applied);
    assert_eq!(last.body_reference.status, CheckStatus::Passed);
    assert_eq!(
        serde_json::to_value(last).unwrap(),
        serde_json::to_value(&state.snapshot(VerificationQuery::default()).receipts[0]).unwrap()
    );
}

#[test]
fn summary_is_independent_of_receipt_and_sample_queues() {
    let mut state = VerificationState::default();
    state.record(
        &proof(BlockIdentity::new(&live_block()), CheckStatus::Passed, 12.0),
        time(),
    );
    state.record(
        &proof(BlockIdentity::new(&live_block()), CheckStatus::Failed, 0.0),
        time(),
    );
    let before = serde_json::to_value(state.summary()).unwrap();
    // Remove the storage a full snapshot used to scan. Summary must still use
    // the incrementally maintained aggregates and last-proof copy exclusively.
    state.records.clear();
    state.order.clear();
    state.proof_times.clear();
    let summary = state.snapshot(VerificationQuery::summary());
    assert_eq!(serde_json::to_value(&summary).unwrap(), before);
    assert_eq!(summary.recent_failure_count, 1);
    assert!(summary.receipts.is_empty());
    assert!(summary.recent_failures.is_empty());
    assert_eq!(summary.median_proof_time_ms, Some(12.0));
}

#[test]
fn pages_clamp_limits_and_offsets_without_cloning_the_rest() {
    let mut state = VerificationState::default();
    for i in 0..RECEIPT_CAPACITY {
        state.record(
            &proof(
                BlockIdentity {
                    state_hash: StateHash::from_fp((i as u64).into()),
                    height: i as u32,
                },
                CheckStatus::Pending,
                0.0,
            ),
            time(),
        );
    }
    let page = |limit, offset| {
        state
            .snapshot(VerificationQuery::page(limit, offset))
            .receipts
    };
    assert_eq!(page(None, 0).len(), DEFAULT_RECEIPT_PAGE_SIZE);
    assert_eq!(page(Some(usize::MAX), 0).len(), MAX_RECEIPT_PAGE_SIZE);
    assert!(page(Some(0), 0).is_empty());
    assert!(page(None, usize::MAX).is_empty());
    let second = page(Some(2), 2);
    assert_eq!(
        second.iter().map(|r| r.block.height).collect::<Vec<_>>(),
        vec![1021, 1020]
    );
    assert_eq!(page(None, RECEIPT_CAPACITY - 1).len(), 1);
}

fn identity(i: usize) -> BlockIdentity {
    BlockIdentity {
        state_hash: StateHash::from_fp((i as u64).into()),
        height: i as u32,
    }
}

/// R26a: a fresh node verifies the best tip first, then bootstrap applies
/// 290 ancestors, which push the tip out of the default 64-receipt page. A
/// lookup by hash still finds both kinds of receipt.
#[test]
fn lookup_by_hash_finds_a_tip_pushed_off_the_default_page() {
    let mut state = VerificationState::default();
    let tip = identity(10_000);
    state.record(&proof(tip.clone(), CheckStatus::Passed, 174.0), time());
    for i in 0..290 {
        state.record(
            &VerificationUpdate::Headers {
                block: identity(i),
                arrival: Arrival::Bootstrap,
                delta: CheckStatus::Passed,
                body: CheckStatus::Passed,
            },
            time(),
        );
    }
    let default_page = state.snapshot(VerificationQuery::default());
    assert!(default_page.lookup.is_none());
    assert!(!default_page
        .receipts
        .iter()
        .any(|r| r.block.state_hash == tip.state_hash));

    let unknown = identity(20_000).state_hash;
    let snapshot = state.snapshot(VerificationQuery {
        // Ignored by a lookup.
        limit: Some(1),
        offset: 500,
        hashes: vec![
            tip.state_hash.clone(),
            identity(3).state_hash,
            unknown,
            tip.state_hash.clone(),
        ],
    });
    assert_eq!(
        snapshot.lookup,
        Some(ReceiptLookup {
            requested: 3,
            found: 2
        })
    );
    let heights = snapshot
        .receipts
        .iter()
        .map(|r| r.block.height)
        .collect::<Vec<_>>();
    assert_eq!(heights, vec![10_000, 3]);
    assert_eq!(snapshot.receipts[0].block_proof.status, CheckStatus::Passed);
    assert_eq!(snapshot.receipts[1].block_proof.status, CheckStatus::NotRun);
    assert_eq!(snapshot.receipts[1].arrival, Arrival::Bootstrap);
    // The summary still comes with a lookup.
    assert_eq!(snapshot.verified_since_start, 1);
    assert!(snapshot.recent_failures.is_empty());
}

#[test]
fn lookup_by_hash_is_bounded_and_misses_evicted_blocks() {
    let mut state = VerificationState::default();
    for i in 0..RECEIPT_CAPACITY + 1 {
        state.record(&proof(identity(i), CheckStatus::Pending, 0.0), time());
    }
    // Block 0 was evicted by the 1,025th.
    let evicted = state.snapshot(VerificationQuery::by_hash(vec![identity(0).state_hash]));
    assert!(evicted.receipts.is_empty());
    assert_eq!(
        evicted.lookup,
        Some(ReceiptLookup {
            requested: 1,
            found: 0
        })
    );
    let many = (1..=MAX_RECEIPT_PAGE_SIZE + 10)
        .map(|i| identity(i).state_hash)
        .collect();
    let bounded = state.snapshot(VerificationQuery::by_hash(many));
    assert_eq!(bounded.receipts.len(), MAX_RECEIPT_PAGE_SIZE);
    assert_eq!(bounded.receipts[0].block.height, 1);
}

#[test]
fn lookup_wire_format_is_backward_compatible() {
    // An old client's query, without hashes.
    let query: VerificationQuery = serde_json::from_str(r#"{"limit":2}"#).unwrap();
    assert!(query.hashes.is_empty());
    let mut state = VerificationState::default();
    state.record(&proof(identity(1), CheckStatus::Passed, 1.0), time());
    let page = serde_json::to_value(state.snapshot(query)).unwrap();
    assert!(page.get("lookup").is_none());
    let lookup = serde_json::to_value(
        state.snapshot(VerificationQuery::by_hash(vec![identity(1).state_hash])),
    )
    .unwrap();
    assert_eq!(
        lookup["lookup"],
        serde_json::json!({"requested": 1, "found": 1})
    );
    assert_eq!(lookup["receipts"][0]["block"]["height"], 1);
}

#[test]
fn query_round_trips_through_postcard() {
    // The recorder writes RPC requests, this query among them, with postcard,
    // which has no field names: every field must be written, even when empty.
    for query in [
        VerificationQuery::page(Some(2), 3),
        VerificationQuery::by_hash(vec![identity(1).state_hash, identity(2).state_hash]),
    ] {
        let bytes = postcard::to_allocvec(&query).unwrap();
        let decoded: VerificationQuery = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.limit, query.limit);
        assert_eq!(decoded.offset, query.offset);
        assert_eq!(decoded.hashes, query.hashes);
    }
    // Followed by more data, as in a recorded event stream.
    let mut bytes = postcard::to_allocvec(&VerificationQuery::page(None, 0)).unwrap();
    bytes.extend(postcard::to_allocvec(&7u32).unwrap());
    let (decoded, rest): (VerificationQuery, _) = postcard::take_from_bytes(&bytes).unwrap();
    assert!(decoded.hashes.is_empty());
    assert_eq!(postcard::from_bytes::<u32>(rest).unwrap(), 7);
}

#[test]
fn incremental_median_handles_duplicates_and_eviction() {
    let mut state = VerificationState::default();
    // Independent arithmetic expectations, not consensus test vectors.
    for (i, ms) in [9.0, 1.0, 5.0, 5.0].into_iter().enumerate() {
        state.record(
            &proof(
                BlockIdentity {
                    state_hash: StateHash::from_fp((i as u64).into()),
                    height: i as u32,
                },
                CheckStatus::Passed,
                ms,
            ),
            time(),
        );
        assert_eq!(
            state.summary().median_proof_time_ms,
            Some([9.0, 5.0, 5.0, 5.0][i])
        );
    }
    for i in 4..RECEIPT_CAPACITY + 4 {
        state.record(
            &proof(
                BlockIdentity {
                    state_hash: StateHash::from_fp((i as u64).into()),
                    height: i as u32,
                },
                CheckStatus::Passed,
                7.0,
            ),
            time(),
        );
    }
    assert_eq!(state.summary().median_proof_time_ms, Some(7.0));
    assert_eq!(state.sorted_proof_times.len(), RECEIPT_CAPACITY);
}
