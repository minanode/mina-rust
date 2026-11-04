//! Staged ledger parts a peer sends can make the staged ledger hash and still
//! fail to rebuild the staged ledger: the scan state hash leaves out the scan
//! state's done jobs, whose transactions the rebuild applies again. The node
//! rejects that peer's answer and asks another peer, as OCaml's bootstrap
//! starts over, instead of panicking. Parts whose scan state trees no scan
//! state has are invalid before the rebuild.
use std::time::Duration;

use ledger::{
    scan_state::{currency::Balance, pending_coinbase::PendingCoinbase, scan_state::ScanState},
    staged_ledger::hash::StagedLedgerHash,
    Account, AccountId, BaseLedger, Mask, TokenId,
};
use mina_core::{constants::constraint_constants, log::inner as tracing};
use mina_p2p_messages::v2::{self, TransactionSnarkScanStateStableV2ScanStateTreesA as Tree};
use redux::EnablingCondition;

use super::{
    nearest_peer_sync_tests::{add_peer, initialized_node},
    no_outcome_sync_tests::reduce,
    *,
};
use crate::{
    ledger::{LedgerCtx, LEDGER_DEPTH},
    p2p::{
        channels::{
            rpc::{P2pChannelsRpcAction, StagedLedgerAuxAndPendingCoinbases},
            P2pChannelsAction,
        },
        P2pAction, PeerId,
    },
    persist::test_fixtures::{done_fee_transfer, hash_of, leaves_mut, mainnet_blocks},
    transition_frontier::sync::{
        ledger::staged::{
            PeerStagedLedgerPartsFetchState as PartsRpc, StagedLedgerAuxAndPendingCoinbasesValid,
            StagedLedgerAuxAndPendingCoinbasesValidated as Validated,
            TransitionFrontierSyncLedgerStagedAction as StagedAction,
        },
        TransitionFrontierSyncAction,
    },
};

const BALANCE: u64 = 1_000_000_000;
pub(super) const FEE: u64 = 5_000_000;

/// A snarked ledger of one account, and the block a scan state's transfer
/// to it is in.
pub(super) struct Fixture {
    snarked: Mask,
    receiver: Account,
    state: v2::MinaStateProtocolStateValueStableV2,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let id = AccountId::new(Account::rand().public_key, TokenId::default());
        let receiver = Account::create_with(id, Balance::from_u64(BALANCE));
        let mut snarked = Mask::create(LEDGER_DEPTH);
        snarked
            .get_or_create_account(receiver.id(), receiver.clone())
            .unwrap();
        Self {
            snarked,
            receiver,
            state: mainnet_blocks()[0].header().protocol_state.clone(),
        }
    }

    /// The parts of the staged ledger over the snarked ledger after a fee
    /// transfer of `FEE` to its account, which its scan state holds done,
    /// as a peer sends them with that transfer's fee set to `fee`, and the
    /// staged ledger hash they make.
    fn parts(
        &self,
        fee: u64,
    ) -> (
        Arc<StagedLedgerAuxAndPendingCoinbases>,
        v2::MinaBaseStagedLedgerHashStableV1,
    ) {
        self.parts_in_trees(fee, 1)
    }

    /// The same with `trees` scan state trees: the transfer in the first,
    /// the others empty.
    pub(super) fn parts_in_trees(
        &self,
        fee: u64,
        trees: usize,
    ) -> (
        Arc<StagedLedgerAuxAndPendingCoinbases>,
        v2::MinaBaseStagedLedgerHashStableV1,
    ) {
        let constants = constraint_constants();
        let mut staged = Mask::create(LEDGER_DEPTH);
        let paid = Account::create_with(self.receiver.id(), Balance::from_u64(BALANCE + FEE));
        staged.get_or_create_account(paid.id(), paid).unwrap();
        let mut scan_state =
            v2::TransactionSnarkScanStateStableV2::from(&ScanState::empty(constants));
        let empty = scan_state.scan_state.trees.0.clone();
        for _ in 1..trees {
            scan_state.scan_state.trees.1.push_back(empty.clone());
        }
        leaves_mut(&mut scan_state.scan_state.trees.0)[0] =
            done_fee_transfer(&self.receiver, fee, &self.state);
        let mut pending_coinbase = PendingCoinbase::create(constants.pending_coinbase_depth);
        let staged_ledger_hash = hash_of(&staged);
        let hash = StagedLedgerHash::of_aux_ledger_and_coinbase_hash(
            ScanState::try_from(&scan_state).unwrap().hash(),
            staged_ledger_hash.to_field().unwrap(),
            &mut pending_coinbase,
        );
        let mut needed_blocks = List::new();
        needed_blocks.push_back(self.state.clone());
        let parts = StagedLedgerAuxAndPendingCoinbases {
            scan_state,
            staged_ledger_hash,
            pending_coinbase: (&pending_coinbase).into(),
            needed_blocks,
        };
        (Arc::new(parts), (&hash).into())
    }

    /// Rebuilds the staged ledger from `parts` as the ledger thread does for
    /// `StagedLedgerReconstruct`, over a sync copy of the snarked ledger, and
    /// says whether that copy has a child mask after it.
    fn rebuild_over(
        &self,
        parts: Arc<StagedLedgerAuxAndPendingCoinbasesValid>,
    ) -> (Result<(), String>, bool) {
        let hash = hash_of(&self.snarked);
        let mut ctx = LedgerCtx::default();
        ctx.insert_genesis_ledger(self.snarked.copy());
        ctx.copy_snarked_ledger_contents_for_sync(&[], hash.clone(), false)
            .unwrap();
        let (_, result) = ctx
            .staged_ledger_reconstruct_sync(hash.clone(), Some(parts))
            .unwrap();
        let (synced, _) = ctx.sync_ledger_for_tests(&hash).unwrap();
        (result, synced.has_children())
    }

    pub(super) fn rebuild(
        &self,
        parts: Arc<StagedLedgerAuxAndPendingCoinbasesValid>,
    ) -> Result<(), String> {
        self.rebuild_over(parts).0
    }

    pub(super) fn snarked_ledger_hash(&self) -> v2::LedgerHash {
        hash_of(&self.snarked)
    }

    /// A root ledger sync fetching the parts that make `hash` from two
    /// peers, none asked yet.
    fn syncing(&self, hash: &v2::MinaBaseStagedLedgerHashStableV1) -> (crate::State, [PeerId; 2]) {
        let (mut state, root) = initialized_node();
        let peers = [PeerId::from_bytes([21; 32]), PeerId::from_bytes([22; 32])];
        for peer in peers {
            add_peer(&mut state, peer, true, &root, None);
        }
        let mut target = SyncLedgerTarget::root(&root).with_staged().unwrap();
        target.snarked_ledger_hash = hash_of(&self.snarked);
        target.staged.hashes = hash.clone();
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
        (state, peers)
    }
}

fn valid(
    parts: &Arc<StagedLedgerAuxAndPendingCoinbases>,
    hash: &v2::MinaBaseStagedLedgerHashStableV1,
) -> Arc<StagedLedgerAuxAndPendingCoinbasesValid> {
    match Validated::validate(parts, hash) {
        Validated::Valid(valid) => valid,
        Validated::Invalid(_) => panic!("parts that make the hash are invalid"),
    }
}

pub(super) fn staged(state: &crate::State) -> &TransitionFrontierSyncLedgerStagedState {
    state
        .transition_frontier
        .sync
        .ledger()
        .unwrap()
        .staged()
        .unwrap()
}

/// `sender`'s parts received, checked and taken: the staged ledger rebuild
/// pending.
fn received(
    state: &mut crate::State,
    sender: PeerId,
    parts: &Arc<StagedLedgerAuxAndPendingCoinbases>,
    now: Timestamp,
) {
    reduce(
        state,
        StagedAction::PartsPeerFetchPending {
            peer_id: sender,
            rpc_id: 1,
        },
        now,
    );
    let followups = reduce(
        state,
        StagedAction::PartsPeerFetchSuccess {
            peer_id: sender,
            rpc_id: 1,
            parts: parts.clone(),
        },
        now,
    );
    assert_eq!(followups.len(), 1);
    reduce(state, StagedAction::PartsPeerValid { sender }, now);
    reduce(state, StagedAction::PartsFetchSuccess { sender }, now);
    reduce(state, StagedAction::ReconstructPending, now);
}

/// The parts the rebuild is pending for.
pub(super) fn rebuilding(state: &crate::State) -> Arc<StagedLedgerAuxAndPendingCoinbasesValid> {
    let (_, parts) = staged(state).target_with_parts().unwrap();
    parts.unwrap().clone()
}

#[test]
fn honest_parts_rebuild_the_staged_ledger() {
    let fixture = Fixture::new();
    let (parts, hash) = fixture.parts(FEE);
    let (mut state, [sender, _]) = fixture.syncing(&hash);
    let now = Timestamp::ZERO + Duration::from_secs(1000);
    received(&mut state, sender, &parts, now);
    assert_eq!(fixture.rebuild(rebuilding(&state)), Ok(()));
    reduce(
        &mut state,
        StagedAction::ReconstructSuccess {
            ledger_hash: parts.staged_ledger_hash.clone(),
        },
        now,
    );
    reduce(&mut state, StagedAction::Success, now);
    assert!(staged(&state).is_success());
}

#[test]
fn parts_that_make_the_hash_but_do_not_rebuild_are_rejected_and_asked_of_another_peer() {
    let fixture = Fixture::new();
    let (_, hash) = fixture.parts(FEE);
    // A peer changes the done transfer's fee: the staged ledger hash is the
    // same, and the parts are valid.
    let (tampered, tampered_hash) = fixture.parts(FEE + 1);
    assert_eq!(tampered_hash, hash);
    valid(&tampered, &hash);

    let (mut state, [sender, other]) = fixture.syncing(&hash);
    let now = Timestamp::ZERO + Duration::from_secs(1000);
    received(&mut state, sender, &tampered, now);
    let error = fixture.rebuild(rebuilding(&state)).unwrap_err();
    assert!(error.contains("Mismatching merkle root"), "{error}");

    let followups = reduce(&mut state, StagedAction::ReconstructError { error }, now);
    assert!(
        matches!(
            &followups[..],
            [
                crate::Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
                    P2pChannelsRpcAction::SyncAnswerRejected { peer_id }
                ))),
                crate::Action::TransitionFrontier(crate::TransitionFrontierAction::Sync(
                    TransitionFrontierSyncAction::Ledger(
                        crate::transition_frontier::sync::ledger::TransitionFrontierSyncLedgerAction::Staged(
                            StagedAction::PartsPeerFetchInit
                        )
                    )
                )),
            ] if *peer_id == sender
        ),
        "{followups:?}"
    );
    let attempts = staged(&state).fetch_attempts().unwrap();
    assert!(matches!(
        attempts.get(&sender),
        Some(PartsRpc::Invalid { .. })
    ));
    assert_eq!(attempts.len(), 1);

    // The parts are asked of the other peer.
    let mut followups = followups.into_iter();
    let rejected = followups.next().unwrap();
    if rejected.is_enabled(&state, now) {
        reduce(&mut state, rejected, now);
    }
    let asked = reduce(&mut state, followups.next().unwrap(), now);
    assert!(
        matches!(
            &asked[..],
            [crate::Action::P2p(P2pAction::Channels(P2pChannelsAction::Rpc(
                P2pChannelsRpcAction::RequestSend { peer_id, .. }
            )))] if *peer_id == other
        ),
        "{asked:?}"
    );
}

/// The sync then asks another peer for the parts, over the same snarked
/// ledger: a failed rebuild leaves no mask over it, a successful one its
/// staged ledger's.
#[test]
fn a_failed_rebuild_leaves_no_mask_over_the_snarked_ledger() {
    let fixture = Fixture::new();
    let (parts, hash) = fixture.parts(FEE);
    let (tampered, _) = fixture.parts(FEE + 1);
    let (result, child) = fixture.rebuild_over(valid(&parts, &hash));
    assert_eq!((result, child), (Ok(()), true));
    let (result, child) = fixture.rebuild_over(valid(&tampered, &hash));
    assert!(result.is_err());
    assert!(!child, "the failed rebuild's mask is still a child");
}

/// `parts` with trees of a done transfer of nothing after their own tree:
/// the scan state hash leaves them out and the rebuilt ledger is the same,
/// but no update leaves that many trees, and the next one panics.
fn with_more_trees(
    fixture: &Fixture,
    parts: &StagedLedgerAuxAndPendingCoinbases,
) -> Arc<StagedLedgerAuxAndPendingCoinbases> {
    let constants = constraint_constants();
    let max_trees = (constants.transaction_capacity_log_2 + 1) * (constants.work_delay + 1) + 1;
    let mut tampered = parts.clone();
    for _ in 0..max_trees {
        tampered
            .scan_state
            .scan_state
            .trees
            .1
            .push_back(Tree::Leaf(vec![done_fee_transfer(
                &fixture.receiver,
                0,
                &fixture.state,
            )]));
    }
    Arc::new(tampered)
}

#[test]
fn parts_with_more_scan_state_trees_than_a_scan_state_has_are_invalid() {
    let fixture = Fixture::new();
    let (parts, hash) = fixture.parts(FEE);
    let tampered = with_more_trees(&fixture, &parts);
    let scan_state = ScanState::try_from(&tampered.scan_state).unwrap();
    assert_eq!(
        scan_state.hash(),
        ScanState::try_from(&parts.scan_state).unwrap().hash()
    );
    assert!(scan_state.check_shape().is_err());

    assert!(!Validated::validate(&tampered, &hash).is_valid());
    assert!(Validated::validate(&parts, &hash).is_valid());
}

/// The events logged while `f` runs, with their levels and fields.
fn logged(f: impl FnOnce()) -> Vec<(tracing::Level, String)> {
    use std::sync::Mutex;
    use tracing::{
        field::{Field, Visit},
        span, Event, Metadata, Subscriber,
    };

    struct Fields(String);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.push_str(&format!("{}={value:?} ", field.name()));
        }
    }
    struct Capture(Arc<Mutex<Vec<(tracing::Level, String)>>>);
    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &span::Attributes<'_>) -> span::Id {
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap()
                .push((*event.metadata().level(), fields.0));
        }
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    tracing::subscriber::with_default(Capture(events.clone()), f);
    let events = events.lock().unwrap().clone();
    events
}

struct LogContext;
static ZERO: u64 = 0;
impl mina_core::log::EventContext for LogContext {
    fn timestamp(&self) -> Timestamp {
        Timestamp::ZERO
    }
    fn time(&self) -> &'_ dyn tracing::Value {
        &ZERO
    }
    fn node_id(&self) -> &'_ dyn tracing::Value {
        &ZERO
    }
    fn log_node_id(&self) -> bool {
        false
    }
}

/// A refusal of parts that make the hash is logged at warn, naming the peer
/// and why in this node's words: were an honest peer's parts ever refused,
/// the logs would show it.
#[test]
fn a_refusal_is_logged_with_the_peer_and_the_reason() {
    use mina_core::ActionEvent;

    let fixture = Fixture::new();
    let (parts, hash) = fixture.parts(FEE);
    let tampered = with_more_trees(&fixture, &parts);
    let (mut state, [sender, _]) = fixture.syncing(&hash);
    let now = Timestamp::ZERO + Duration::from_secs(1000);
    reduce(
        &mut state,
        StagedAction::PartsPeerFetchPending {
            peer_id: sender,
            rpc_id: 1,
        },
        now,
    );
    let followups = reduce(
        &mut state,
        StagedAction::PartsPeerFetchSuccess {
            peer_id: sender,
            rpc_id: 1,
            parts: tampered,
        },
        now,
    );
    let [crate::Action::TransitionFrontier(crate::TransitionFrontierAction::Sync(
        TransitionFrontierSyncAction::Ledger(
            crate::transition_frontier::sync::ledger::TransitionFrontierSyncLedgerAction::Staged(
                refused @ StagedAction::PartsPeerInvalid { .. },
            ),
        ),
    ))] = &followups[..]
    else {
        panic!("no refusal: {followups:?}");
    };
    let StagedAction::PartsPeerInvalid { reason, .. } = refused else {
        unreachable!()
    };
    assert_eq!(reason, "scan state: 26 trees, not 1 to 25");

    let events = logged(|| refused.action_event(&LogContext));
    assert_eq!(events.len(), 1, "{events:?}");
    let (level, fields) = &events[0];
    assert_eq!(*level, tracing::Level::WARN);
    assert!(fields.contains(&sender.to_string()), "{fields}");
    assert!(fields.contains(reason.as_str()), "{fields}");
    reduce(&mut state, refused.clone(), now);
    let attempts = staged(&state).fetch_attempts().unwrap();
    assert!(matches!(
        attempts.get(&sender),
        Some(PartsRpc::Invalid { .. })
    ));
}
