//! STAGED-SPLIT: a web node with two relays that serve a staged ledger from
//! its last part fetches it from both ends at once, puts the two halves
//! together where they meet, and cancels the other stream. Without the
//! advertisement, or with one stream set, it fetches from one relay as
//! before.
use super::{
    nearest_peer_sync_tests::{add_peer, at_ms, initialized_node, near, run},
    *,
};
use crate::{
    p2p::{
        channels::{
            rpc::{P2pSyncPeerStats, StagedLedgerAuxAndPendingCoinbases},
            streaming_rpc::{
                staged_ledger_parts::{from_first_part, from_last_part, StagedLedgerPartsResponse},
                P2pChannelsStreamingRpcAction, P2pChannelsStreamingRpcState, P2pStreamingRpcId,
                P2pStreamingRpcLocalState, P2pStreamingRpcResponse,
            },
        },
        PeerId,
    },
    transition_frontier::sync::ledger::staged::{
        PeerStagedLedgerPartsFetchState, TransitionFrontierSyncLedgerStagedAction as StagedAction,
        TransitionFrontierSyncLedgerStagedState,
    },
};
use mina_p2p_messages::{list::List, v2};
use redux::EnablingCondition;

fn other() -> PeerId {
    PeerId::from_bytes([14; 32])
}

fn stats(fastest_ms: u64) -> P2pSyncPeerStats {
    let mut stats = P2pSyncPeerStats::default();
    stats.record_answer(at_ms(0), at_ms(fastest_ms));
    stats
}

/// A node at the staged ledger with two WebRTC relays, `advertised` or not,
/// fetching with `streams` at once.
fn staged_node(streams: usize, advertised: bool) -> crate::State {
    let (mut state, root) = initialized_node();
    add_peer(&mut state, near(), false, &root, Some(stats(150)));
    add_peer(&mut state, other(), false, &root, Some(stats(170)));
    let p2p = state.p2p.ready_mut().unwrap();
    for peer in [near(), other()] {
        p2p.get_ready_peer_mut(&peer)
            .unwrap()
            .channels
            .staged_ledger_from_last = advertised;
    }
    p2p.config.limits = p2p
        .config
        .limits
        .clone()
        .with_staged_ledger_streams(crate::p2p::Limit::Some(streams));
    let target = SyncLedgerTarget::root(&root).with_staged().unwrap();
    state.transition_frontier.sync =
        TransitionFrontierSyncState::RootLedgerPending(TransitionFrontierSyncLedgerPending {
            time: Timestamp::ZERO,
            best_tip: root.clone(),
            root_block: root.clone(),
            blocks_inbetween: vec![],
            root_block_updates: vec![],
            ledger: TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
                time: Timestamp::ZERO,
                target,
                attempts: Default::default(),
                single_stream: false,
            }
            .into(),
        });
    state
}

fn requests(ran: &[String]) -> Vec<&String> {
    ran.iter()
        .filter(|a| a.contains("RequestSend") && a.contains("StagedLedgerParts"))
        .collect()
}

fn local_rpc(state: &crate::State, peer: &PeerId) -> Option<P2pStreamingRpcId> {
    state
        .p2p
        .get_ready_peer(peer)?
        .channels
        .streaming_rpc
        .pending_local_rpc_id()
}

fn local(state: &crate::State, peer: &PeerId) -> P2pStreamingRpcLocalState {
    match &state
        .p2p
        .get_ready_peer(peer)
        .unwrap()
        .channels
        .streaming_rpc
    {
        P2pChannelsStreamingRpcState::Ready { local, .. } => local.clone(),
        other => panic!("{other:?}"),
    }
}

/// A staged ledger of `trees` scan state trees.
fn staged_ledger(trees: u64) -> StagedLedgerAuxAndPendingCoinbases {
    let tree = |i: u64| {
        v2::TransactionSnarkScanStateStableV2ScanStateTreesA::Leaf(vec![(
            v2::ParallelScanWeightStableV1 {
                base: i.into(),
                merge: 0.into(),
            },
            v2::TransactionSnarkScanStateStableV2ScanStateTreesABaseT1::Empty,
        )])
    };
    StagedLedgerAuxAndPendingCoinbases {
        scan_state: v2::TransactionSnarkScanStateStableV2 {
            scan_state: v2::TransactionSnarkScanStateStableV2ScanState {
                trees: (tree(0), (1..trees).map(tree).collect()),
                acc: None,
                curr_job_seq_no: 0.into(),
                max_base_jobs: 1.into(),
                delay: 2.into(),
            },
            previous_incomplete_zkapp_updates: (
                List::new(),
                v2::TransactionSnarkScanStateStableV2PreviousIncompleteZkappUpdates1::Border_block_continued_in_the_next_tree(false),
            ),
        },
        staged_ledger_hash: v2::LedgerHash::zero(),
        pending_coinbase: v2::MinaBasePendingCoinbaseStableV2 {
            tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2 {
                indexes: List::new(),
                depth: 5.into(),
                tree: v2::MinaBasePendingCoinbaseMerkleTreeVersionedStableV2Tree::Hash(
                    v2::MinaBasePendingCoinbaseHashVersionedStableV1(
                        v2::MinaBasePendingCoinbaseHashBuilderStableV1(
                            mina_p2p_messages::bigint::BigInt::zero(),
                        ),
                    )
                    .into(),
                ),
            },
            pos_list: List::new(),
            new_pos: v2::MinaBasePendingCoinbaseStackIdStableV1(0.into()),
        },
        needed_blocks: List::new(),
    }
}

fn part(
    state: &mut crate::State,
    peer: PeerId,
    id: P2pStreamingRpcId,
    response: Option<StagedLedgerPartsResponse>,
) -> Vec<String> {
    run(
        state,
        P2pChannelsStreamingRpcAction::ResponsePartReceived {
            peer_id: peer,
            id,
            response: P2pStreamingRpcResponse::StagedLedgerParts(response.unwrap()),
        },
        at_ms(2_000),
    )
}

/// Both relays advertised it: the nearer sends the staged ledger from the
/// first part, the other from the last, at once.
#[test]
fn two_relays_send_the_staged_ledger_from_both_ends() {
    let mut state = staged_node(2, true);
    let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let asked = requests(&ran);
    assert_eq!(asked.len(), 2, "{ran:#?}");
    let from_first = asked
        .iter()
        .find(|a| !a.contains("FromLast"))
        .expect("from the first");
    let from_last = asked
        .iter()
        .find(|a| a.contains("FromLast"))
        .expect("from the last");
    assert!(
        from_first.contains(&format!("{:?}", near())),
        "{from_first}"
    );
    assert!(from_last.contains(&format!("{:?}", other())), "{from_last}");

    // Both live: no more on the next tick.
    assert!(!StagedAction::PartsPeerFetchInit.is_enabled(&state, at_ms(1_500)));
}

/// Not advertised, or one stream set: one request, as before.
#[test]
fn one_relay_sends_it_without_the_advertisement_or_with_one_stream() {
    for (streams, advertised) in [(1, true), (2, false)] {
        let mut state = staged_node(streams, advertised);
        let ran = run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
        assert_eq!(requests(&ran).len(), 1, "{ran:#?}");
        assert!(
            !StagedAction::PartsPeerFetchInit.is_enabled(&state, at_ms(1_500)),
            "a second stream with streams={streams} advertised={advertised}"
        );
    }
}

/// The two streams meet: the parts are put together as the second's
/// answer, the first is cancelled and its part on the way dropped. The
/// parts here don't hash to the target, so both peers are blamed and the
/// next fetch is from one at a time.
#[test]
fn the_two_halves_are_put_together_and_the_other_cancelled() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);

    // From the first: base, scan state base, zkapp updates, trees 0 and 1.
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    // From the last: scan state base, trees 4 and 3.
    for sent in 0..3 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    assert!(local_rpc(&state, &near()).is_some());
    // Tree 2 from the last completes it.
    let ran = part(&mut state, other(), last, from_last_part(&data, 3));
    assert!(
        ran.iter().any(
            |a| a.contains(&format!("Cancel({first})")) && a.contains(&format!("{:?}", near()))
        ),
        "{ran:#?}"
    );
    assert!(
        ran.iter().any(|a| a.contains("PartsPeerFetchSuccess")),
        "{ran:#?}"
    );
    assert!(
        matches!(local(&state, &near()), P2pStreamingRpcLocalState::Cancelled { id, .. } if id == first)
    );
    // The tree the first was sending comes, and is dropped.
    part(&mut state, near(), first, from_first_part(&data, 5));
    assert!(
        matches!(local(&state, &near()), P2pStreamingRpcLocalState::Responded { id, .. } if id == first)
    );

    // Fake parts: rejected, both blamed, one stream from now on.
    let staged = state
        .transition_frontier
        .sync
        .ledger()
        .and_then(|l| l.staged())
        .unwrap();
    let TransitionFrontierSyncLedgerStagedState::PartsFetchPending {
        attempts,
        single_stream,
        ..
    } = staged
    else {
        panic!("{staged:?}");
    };
    assert!(*single_stream);
    for peer in [near(), other()] {
        assert!(
            matches!(
                attempts.get(&peer),
                Some(PeerStagedLedgerPartsFetchState::Invalid { .. })
            ),
            "{attempts:?}"
        );
    }
}

/// The same, the first completing it: the last had asked for its next tree,
/// which comes after the cancel and is dropped, with no disconnection.
#[test]
fn the_first_completing_it_drops_the_lasts_part_on_its_way() {
    let mut state = staged_node(2, true);
    run(&mut state, StagedAction::PartsPeerFetchInit, at_ms(1_000));
    let first = local_rpc(&state, &near()).unwrap();
    let last = local_rpc(&state, &other()).unwrap();
    let data = staged_ledger(5);

    // From the last: scan state base, trees 4 and 3; tree 2 asked for.
    for sent in 0..3 {
        part(&mut state, other(), last, from_last_part(&data, sent));
    }
    // From the first: base, scan state base, zkapp updates, trees 0, 1, 2.
    for sent in 0..5 {
        part(&mut state, near(), first, from_first_part(&data, sent));
    }
    let ran = part(&mut state, near(), first, from_first_part(&data, 5));
    assert!(
        ran.iter().any(|a| a.contains("PartsPeerFetchSuccess")),
        "{ran:#?}"
    );
    assert!(
        matches!(local(&state, &other()), P2pStreamingRpcLocalState::Cancelled { id, .. } if id == last)
    );
    let ran = part(&mut state, other(), last, from_last_part(&data, 3));
    assert!(!ran.iter().any(|a| a.contains("Disconnection")), "{ran:#?}");
    assert!(
        matches!(local(&state, &other()), P2pStreamingRpcLocalState::Responded { id, .. } if id == last)
    );
}
