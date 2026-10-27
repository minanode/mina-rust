//! LN2: the light RPCs on a real `State`, with the reducers only (see
//! `transaction_pool::submission_tests` for the harness).

use std::sync::Arc;

use ledger::AccountId;
use mina_core::block::{AppliedBlock, ArcBlockWithHash, BlockWithHash};
use mina_p2p_messages::{
    binprot::BinProtRead,
    rpc::GetBestTipV2Response,
    rpc_kernel::ResponsePayload,
    v2::{MinaBlockBlockStableV2, StateHash},
};
use mina_signer::CompressedPubKey;

use crate::{
    ledger::{
        read::{LedgerReadAction, LedgerReadRequest, LedgerReadResponse},
        LedgerAddress,
    },
    ledger_effectful::LedgerEffectfulAction,
    rpc::{
        LightLedgerKind, RpcAction, RpcId, RpcLightAccountQuery, RpcLightAccountResponse,
        RpcLightBestTipResponse, RpcLightError,
    },
    rpc_effectful::RpcEffectfulAction,
    transaction_pool::submission_tests::{block, fresh_state, run, synced_state},
    Action, State,
};

fn rpc_id() -> RpcId {
    RpcId::new_unchecked(11, 1)
}

/// The fixture's root block: the `get_best_tip` proof's root.
fn root_block() -> ArcBlockWithHash {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
    );
    let bytes = std::fs::read(path).unwrap();
    let payload = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
    let root: MinaBlockBlockStableV2 = payload.0.unwrap().0.unwrap().proof.1;
    BlockWithHash::try_new(Arc::new(root)).unwrap()
}

/// Synced, with the fixture's root and tip as the best chain.
fn synced_with_root() -> State {
    let mut state = synced_state();
    state.transition_frontier.best_chain.insert(
        0,
        AppliedBlock {
            block: root_block(),
            just_emitted_a_proof: false,
        },
    );
    state
}

fn query(ledger: LightLedgerKind, block: Option<StateHash>) -> RpcLightAccountQuery {
    let key =
        CompressedPubKey::from_address("B62qoXucNXRjjG1SmWvogDQTeC17GRNsdhvhsBUEcsMy6CAXhWSgmTV")
            .unwrap();
    RpcLightAccountQuery {
        ledger,
        account_ids: vec![AccountId::new_with_default_token(key)],
        block,
    }
}

fn account_answers(ran: &[Action]) -> Vec<RpcLightAccountResponse> {
    ran.iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::LightAccountSuccess {
                rpc_id: id,
                response,
            }) if *id == rpc_id() => Some(response.clone()),
            _ => None,
        })
        .collect()
}

fn best_tip_answers(ran: &[Action]) -> Vec<RpcLightBestTipResponse> {
    ran.iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::LightBestTip { response, .. }) => {
                Some(response.clone())
            }
            _ => None,
        })
        .collect()
}

fn ledger_reads(ran: &[Action]) -> Vec<LedgerReadRequest> {
    ran.iter()
        .filter_map(|action| match action {
            Action::LedgerEffects(LedgerEffectfulAction::ReadInit { request, .. }) => {
                Some(request.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn not_synced_is_answered_at_once() {
    let mut state = fresh_state();
    let ran = run(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: rpc_id(),
            query: query(LightLedgerKind::Staged, None),
        },
    );
    assert_eq!(account_answers(&ran), vec![Err(RpcLightError::NotSynced)]);
    assert!(ledger_reads(&ran).is_empty());
    // The effect answers and finishes it; nothing is left to retry.
    run(&mut state, RpcAction::Finish { rpc_id: rpc_id() });
    assert!(state.rpc.requests.is_empty());

    let ran = run(&mut state, RpcAction::LightBestTipGet { rpc_id: rpc_id() });
    assert!(matches!(
        best_tip_answers(&ran).as_slice(),
        [Err(RpcLightError::NotSynced)]
    ));
}

#[test]
fn bootstrapping_with_a_stale_chain_is_not_synced() {
    // A re-bootstrap keeps the old best chain until it commits.
    let mut state = synced_state();
    state.transition_frontier.genesis_ledgers =
        crate::transition_frontier::GenesisLedgersState::Deferred;
    let ran = run(&mut state, RpcAction::LightBestTipGet { rpc_id: rpc_id() });
    assert!(matches!(
        best_tip_answers(&ran).as_slice(),
        [Err(RpcLightError::NotSynced)]
    ));
}

#[test]
fn a_block_off_the_best_chain_is_refused() {
    let mut state = synced_with_root();
    let ran = run(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: rpc_id(),
            query: query(LightLedgerKind::Snarked, Some(StateHash::zero())),
        },
    );
    assert_eq!(
        account_answers(&ran),
        vec![Err(RpcLightError::BlockNotInBestChain(StateHash::zero()))]
    );
    assert!(ledger_reads(&ran).is_empty());
}

#[test]
fn best_tip_is_the_best_chain_root_first() {
    let mut state = synced_with_root();
    let ran = run(&mut state, RpcAction::LightBestTipGet { rpc_id: rpc_id() });
    let answers = best_tip_answers(&ran);
    let [Ok(chain)] = answers.as_slice() else {
        panic!("{answers:?}");
    };
    let hashes: Vec<_> = chain.iter().map(|block| block.hash().clone()).collect();
    assert_eq!(
        hashes,
        vec![root_block().hash().clone(), block().hash().clone()]
    );
    // A read-only answer: no request is kept for it.
    assert!(state.rpc.requests.is_empty());
}

/// Each kind reads the ledger hash at its field of the named block (the tip
/// by default), then the ledger thread's answer reaches the RPC and the
/// request is removed.
#[test]
fn each_kind_reads_its_ledger_at_the_named_block() {
    let tip = block();
    let root = root_block();
    for (kind, named, expected) in [
        (LightLedgerKind::Staged, None, tip.merkle_root_hash()),
        (LightLedgerKind::Snarked, None, tip.snarked_ledger_hash()),
        (
            LightLedgerKind::Staking,
            None,
            tip.staking_epoch_ledger_hash(),
        ),
        (LightLedgerKind::Next, None, tip.next_epoch_ledger_hash()),
        (
            LightLedgerKind::Snarked,
            Some(&root),
            root.snarked_ledger_hash(),
        ),
        (
            LightLedgerKind::Staged,
            Some(&root),
            root.merkle_root_hash(),
        ),
    ] {
        let mut state = synced_with_root();
        let ran = run(
            &mut state,
            RpcAction::LightAccountInit {
                rpc_id: rpc_id(),
                query: query(kind, named.map(|block| block.hash().clone())),
            },
        );
        let reads = ledger_reads(&ran);
        let [LedgerReadRequest::LightAccount(id, read)] = reads.as_slice() else {
            panic!("{kind}: {reads:?}");
        };
        assert_eq!(*id, rpc_id());
        assert_eq!(read.ledger, kind);
        assert_eq!(&read.ledger_hash, expected, "{kind}");
        let block = named.unwrap_or(&tip);
        assert_eq!(&read.block.state_hash, block.hash());
        assert_eq!(read.block.height, block.height());
        assert!(account_answers(&ran).is_empty());

        // What the ledger effect and the ledger thread would do.
        let ledger_id = state.ledger.read.next_req_id();
        let request = LedgerReadRequest::LightAccount(rpc_id(), read.clone());
        run(
            &mut state,
            LedgerReadAction::Pending {
                id: ledger_id,
                request,
            },
        );
        run(
            &mut state,
            RpcAction::LightAccountPending { rpc_id: rpc_id() },
        );
        let ran = run(
            &mut state,
            LedgerReadAction::Success {
                id: ledger_id,
                response: LedgerReadResponse::LightAccount(
                    rpc_id(),
                    Ok(vec![Err(RpcLightError::AccountNotFound(kind))]),
                ),
            },
        );
        assert_eq!(
            account_answers(&ran),
            vec![Ok(vec![Err(RpcLightError::AccountNotFound(kind))])]
        );
        run(&mut state, RpcAction::Finish { rpc_id: rpc_id() });
        assert!(state.rpc.requests.is_empty(), "{kind}");
        assert!(state.rpc.light_pending.is_empty(), "{kind}");
    }
}

/// A request the ledger read quota held back stays in Init and is retried
/// when reads finish (`FindTodos`), resolving against the chain as it is then.
#[test]
fn a_held_back_request_is_retried() {
    let mut state = synced_with_root();
    let filler = LedgerReadRequest::GetChildAccountsAtAddr(
        block().merkle_root_hash().clone(),
        LedgerAddress::first(crate::ledger::LEDGER_DEPTH - 6),
    );
    let mut fillers = Vec::new();
    while state.ledger.read.is_total_cost_under_limit() {
        fillers.push(state.ledger.read.add(
            crate::transaction_pool::submission_tests::now(),
            filler.clone(),
        ));
        assert!(fillers.len() < 1_000, "the quota never filled");
    }
    let ran = run(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: rpc_id(),
            query: query(LightLedgerKind::Staged, None),
        },
    );
    assert!(ledger_reads(&ran).is_empty());
    assert!(state.rpc.requests[&rpc_id()].status.is_init());

    for id in fillers {
        state.ledger.read.remove(id);
    }
    let ran = run(&mut state, LedgerReadAction::FindTodos);
    let reads = ledger_reads(&ran);
    assert!(
        matches!(reads.as_slice(), [LedgerReadRequest::LightAccount(id, _)] if *id == rpc_id()),
        "{reads:?}"
    );
}

/// Runs `action` at `time`, with the reducers and enabling conditions only,
/// as `submission_tests::run` does at its fixed time.
fn run_at(state: &mut State, action: impl Into<Action>, time: redux::Timestamp) -> Vec<Action> {
    use redux::EnablingCondition;
    let mut ran = Vec::new();
    let mut queue = vec![action.into()];
    while let Some(action) = queue.pop() {
        if !action.is_enabled(state, time) {
            continue;
        }
        ran.push(action.clone());
        let action = redux::ActionMeta::zero_custom(time).with_action(action);
        let mut dispatcher = redux::Dispatcher::new();
        crate::reducer(state, &action, &mut dispatcher);
        let mut next = Vec::new();
        while let Some(action) = dispatcher.pop() {
            next.push(action);
        }
        queue.extend(next.into_iter().rev());
    }
    ran
}

fn nth_rpc_id(n: u32) -> RpcId {
    RpcId::new_unchecked(100 + n as usize, 1)
}

fn answers_by_id(ran: &[Action]) -> Vec<(RpcId, RpcLightAccountResponse)> {
    ran.iter()
        .filter_map(|action| match action {
            Action::RpcEffectful(RpcEffectfulAction::LightAccountSuccess { rpc_id, response }) => {
                Some((*rpc_id, response.clone()))
            }
            _ => None,
        })
        .collect()
}

/// With the ledger thread stalled (its read quota full), clients that keep
/// sending, or that give up and send again, never get more than
/// MAX_PENDING_LIGHT_REQUESTS light requests into the node: the rest are
/// refused at once. Those still waiting at the deadline are answered and
/// dropped, so nothing is left to retry ahead of block work.
#[test]
fn pending_light_requests_stay_bounded_while_the_ledger_stalls() {
    use crate::rpc::{LIGHT_REQUEST_TIMEOUT, MAX_PENDING_LIGHT_REQUESTS};
    let mut state = synced_with_root();
    let start = crate::transaction_pool::submission_tests::now();
    let filler = LedgerReadRequest::GetChildAccountsAtAddr(
        block().merkle_root_hash().clone(),
        LedgerAddress::first(crate::ledger::LEDGER_DEPTH - 6),
    );
    while state.ledger.read.is_total_cost_under_limit() {
        state.ledger.read.add(start, filler.clone());
    }

    // Three waves of 20 requests, 4 s apart: an HTTP client would have
    // timed out and retried meanwhile.
    let mut busy = 0;
    let mut n = 0;
    for wave in 0..3u64 {
        let time = start + std::time::Duration::from_secs(4 * wave);
        for _ in 0..20 {
            let ran = run_at(
                &mut state,
                RpcAction::LightAccountInit {
                    rpc_id: nth_rpc_id(n),
                    query: query(LightLedgerKind::Staged, None),
                },
                time,
            );
            n += 1;
            busy += answers_by_id(&ran)
                .iter()
                .filter(|(_, response)| *response == Err(RpcLightError::Busy))
                .count();
            for (id, _) in answers_by_id(&ran) {
                run_at(&mut state, RpcAction::Finish { rpc_id: id }, time);
            }
            assert!(state.rpc.light_pending.len() <= MAX_PENDING_LIGHT_REQUESTS);
            let open = state
                .rpc
                .requests
                .values()
                .filter(|r| matches!(r.req, crate::rpc::RpcRequest::LightAccount(_)))
                .count();
            assert!(
                open <= MAX_PENDING_LIGHT_REQUESTS,
                "{open} light requests in state"
            );
        }
    }
    assert_eq!(state.rpc.light_pending.len(), MAX_PENDING_LIGHT_REQUESTS);
    assert_eq!(busy, 60 - MAX_PENDING_LIGHT_REQUESTS);

    // Not yet due: nothing expires.
    let before = start + (LIGHT_REQUEST_TIMEOUT - std::time::Duration::from_secs(1));
    assert!(run_at(&mut state, RpcAction::LightAccountsExpire, before).is_empty());
    // Due (the expiry runs on CheckTimeouts): each is answered and dropped.
    let after = start + LIGHT_REQUEST_TIMEOUT;
    let ran = run_at(
        &mut state,
        Action::CheckTimeouts(crate::CheckTimeoutsAction {}),
        after,
    );
    let expired = answers_by_id(&ran);
    assert_eq!(expired.len(), MAX_PENDING_LIGHT_REQUESTS);
    assert!(expired
        .iter()
        .all(|(_, response)| *response == Err(RpcLightError::Timeout)));
    for (id, _) in expired {
        run_at(&mut state, RpcAction::Finish { rpc_id: id }, after);
    }
    assert!(state.rpc.light_pending.is_empty());
    assert!(state.rpc.requests.is_empty());
    // And the retry finds nothing to put ahead of other reads.
    assert!(ledger_reads(&run_at(&mut state, LedgerReadAction::FindTodos, after)).is_empty());
}

/// A read the ledger thread has taken is not expired: its answer comes.
#[test]
fn a_taken_read_is_answered_not_expired() {
    use crate::rpc::LIGHT_REQUEST_TIMEOUT;
    let mut state = synced_with_root();
    let start = crate::transaction_pool::submission_tests::now();
    run_at(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: rpc_id(),
            query: query(LightLedgerKind::Staged, None),
        },
        start,
    );
    run_at(
        &mut state,
        RpcAction::LightAccountPending { rpc_id: rpc_id() },
        start,
    );
    let late = start + LIGHT_REQUEST_TIMEOUT * 3;
    assert!(run_at(&mut state, RpcAction::LightAccountsExpire, late).is_empty());
    assert_eq!(state.rpc.light_pending.len(), 1);
}

#[test]
fn a_request_for_too_many_or_no_accounts_is_refused() {
    use crate::rpc::MAX_LIGHT_ACCOUNTS_PER_REQUEST;
    for count in [0, MAX_LIGHT_ACCOUNTS_PER_REQUEST + 1] {
        let mut state = synced_with_root();
        let mut query = query(LightLedgerKind::Snarked, None);
        query.account_ids = std::iter::repeat_n(query.account_ids[0].clone(), count).collect();
        let ran = run(
            &mut state,
            RpcAction::LightAccountInit {
                rpc_id: rpc_id(),
                query,
            },
        );
        assert_eq!(
            account_answers(&ran),
            vec![Err(RpcLightError::TooManyAccounts(
                MAX_LIGHT_ACCOUNTS_PER_REQUEST
            ))]
        );
        assert!(ledger_reads(&ran).is_empty());
        assert!(state.rpc.light_pending.is_empty());
    }
    // The cap itself is allowed, as one ledger read.
    let mut state = synced_with_root();
    let mut query = query(LightLedgerKind::Snarked, None);
    query.account_ids =
        std::iter::repeat_n(query.account_ids[0].clone(), MAX_LIGHT_ACCOUNTS_PER_REQUEST).collect();
    let ran = run(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: rpc_id(),
            query,
        },
    );
    assert_eq!(ledger_reads(&ran).len(), 1);
}

/// The accounts all pending light requests read together stay within
/// MAX_PENDING_LIGHT_ACCOUNTS: a full 32-account request takes the whole
/// budget, and is refused while any other light read is pending.
#[test]
fn pending_light_accounts_stay_within_the_budget() {
    use crate::rpc::{MAX_LIGHT_ACCOUNTS_PER_REQUEST, MAX_PENDING_LIGHT_ACCOUNTS};
    let mut state = synced_with_root();
    let start = crate::transaction_pool::submission_tests::now();
    let filler = LedgerReadRequest::GetChildAccountsAtAddr(
        block().merkle_root_hash().clone(),
        LedgerAddress::first(crate::ledger::LEDGER_DEPTH - 6),
    );
    while state.ledger.read.is_total_cost_under_limit() {
        state.ledger.read.add(start, filler.clone());
    }
    let full = |q: RpcLightAccountQuery| RpcLightAccountQuery {
        account_ids: std::iter::repeat_n(q.account_ids[0].clone(), MAX_LIGHT_ACCOUNTS_PER_REQUEST)
            .collect(),
        ..q
    };
    // One single-account read pending: a full request no longer fits.
    run_at(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: nth_rpc_id(0),
            query: query(LightLedgerKind::Staged, None),
        },
        start,
    );
    let ran = run_at(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: nth_rpc_id(1),
            query: full(query(LightLedgerKind::Snarked, None)),
        },
        start,
    );
    assert_eq!(
        answers_by_id(&ran),
        vec![(nth_rpc_id(1), Err(RpcLightError::Busy))]
    );
    assert_eq!(state.rpc.light_pending_accounts(), 1);
    // Once it is answered, the full request fits, and then nothing else.
    run_at(
        &mut state,
        RpcAction::LightAccountSuccess {
            rpc_id: nth_rpc_id(0),
            response: Err(RpcLightError::NotSynced),
        },
        start,
    );
    for id in [nth_rpc_id(0), nth_rpc_id(1)] {
        run_at(&mut state, RpcAction::Finish { rpc_id: id }, start);
    }
    let ran = run_at(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: nth_rpc_id(2),
            query: full(query(LightLedgerKind::Snarked, None)),
        },
        start,
    );
    assert!(answers_by_id(&ran).is_empty());
    assert_eq!(
        state.rpc.light_pending_accounts(),
        MAX_PENDING_LIGHT_ACCOUNTS
    );
    let ran = run_at(
        &mut state,
        RpcAction::LightAccountInit {
            rpc_id: nth_rpc_id(3),
            query: query(LightLedgerKind::Staged, None),
        },
        start,
    );
    assert_eq!(
        answers_by_id(&ran),
        vec![(nth_rpc_id(3), Err(RpcLightError::Busy))]
    );
    assert!(state.rpc.light_pending_accounts() <= MAX_PENDING_LIGHT_ACCOUNTS);
}
