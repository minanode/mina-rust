//! Account-read lifecycle regressions. Commands come from an OCaml block;
//! the harness controls only account snapshots and completion order.
use std::collections::BTreeMap;

use ledger::{
    scan_state::{
        currency::Balance,
        transaction_logic::{valid::UserCommand, TransactionStatus, WithStatus},
    },
    transaction_pool::diff::BestTipDiff,
    Account, AccountId,
};
use mina_core::transaction::TransactionPoolMessageSource;
use mina_p2p_messages::v2::LedgerHash;

use super::{
    submission_tests::{fixture_block, run, synced_state},
    *,
};
use crate::{
    rpc::{RpcId, RpcRequest, RpcRequestState, RpcRequestStatus},
    rpc_effectful::RpcEffectfulAction,
    Action, RpcAction, State,
};

type Accounts = BTreeMap<AccountId, Account>;

pub(crate) fn command() -> UserCommand {
    let diff = fixture_block().body.staged_ledger_diff.diff;
    diff.0
        .commands
        .into_iter()
        .chain(diff.1.into_iter().flat_map(|d| d.commands))
        .find_map(|c| match c.data {
            c @ mina_p2p_messages::v2::MinaBaseUserCommandStableV2::SignedCommand(_) => {
                Some(UserCommand::try_from(&c).unwrap())
            }
            _ => None,
        })
        .unwrap()
}

pub(crate) fn accounts() -> Accounts {
    let cmd = command();
    let id = cmd.fee_payer();
    let mut account = Account::create_with(id.clone(), Balance::from_u64(1_000_000_000_000));
    account.nonce = cmd.nonce().unwrap();
    BTreeMap::from([(id, account)])
}

fn submit(state: &mut State, id: u32) -> Vec<Action> {
    let rpc_id = RpcId::new_unchecked(id as usize, 1);
    state.rpc.requests.insert(
        rpc_id,
        RpcRequestState {
            req: RpcRequest::TransactionInject(vec![]),
            status: RpcRequestStatus::Pending {
                time: super::submission_tests::now(),
            },
            data: Default::default(),
        },
    );
    run(
        state,
        TransactionPoolAction::VerifySuccess {
            valids: vec![command()],
            from_source: TransactionPoolMessageSource::rpc(rpc_id),
        },
    )
}

fn fetches(ran: &[Action]) -> Vec<u32> {
    ran.iter()
        .filter_map(|a| match a {
            Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts {
                pending_id,
                ..
            }) => *pending_id,
            _ => None,
        })
        .collect()
}

fn respond(state: &mut State, id: u32, result: Result<Accounts, String>) -> Vec<Action> {
    run(
        state,
        TransactionPoolAction::AccountsFetched {
            pending_id: id,
            accounts: result,
        },
    )
}

fn answers(ran: &[Action]) -> Vec<(RpcId, &'static str)> {
    ran.iter()
        .filter_map(|a| match a {
            Action::RpcEffectful(RpcEffectfulAction::TransactionInjectSuccess {
                rpc_id, ..
            }) => Some((*rpc_id, "success")),
            Action::RpcEffectful(RpcEffectfulAction::TransactionInjectRejected {
                rpc_id, ..
            }) => Some((*rpc_id, "rejected")),
            Action::RpcEffectful(RpcEffectfulAction::TransactionInjectFailure {
                rpc_id, ..
            }) => Some((*rpc_id, "failure")),
            _ => None,
        })
        .collect()
}

#[test]
fn responses_apply_in_submission_order_and_preserve_rebroadcast_and_nonce() {
    let mut state = synced_state();
    let first = fetches(&submit(&mut state, 1))[0];
    let second = fetches(&run(
        &mut state,
        TransactionPoolAction::VerifySuccess {
            valids: vec![command()],
            from_source: TransactionPoolMessageSource::pubsub(
                crate::p2p::P2pNetworkPubsubMessageCacheId {
                    source: crate::p2p::libp2p_identity::PeerId::random(),
                    seqno: 1,
                },
            ),
        },
    ))[0];
    assert!(answers(&respond(&mut state, second, Ok(accounts()))).is_empty());
    assert_eq!(state.transaction_pool.size(), 0);
    let ran = respond(&mut state, first, Ok(accounts()));
    assert_eq!(answers(&ran), vec![(RpcId::new_unchecked(1, 1), "success")]);
    let applied = ran
        .iter()
        .filter_map(|a| match a {
            Action::TransactionPool(TransactionPoolAction::ApplyVerifiedDiffWithAccounts {
                pending_id,
                ..
            }) => Some(*pending_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(applied, vec![first, second]);
    assert_eq!(
        ran.iter()
            .filter(|a| matches!(
                a,
                Action::TransactionPool(TransactionPoolAction::Rebroadcast { .. })
            ))
            .count(),
        1
    );
    assert_eq!(state.transaction_pool.size(), 1);
    assert_eq!(state.transaction_pool.telemetry.transactions_added, 1);
    assert_eq!(
        state
            .transaction_pool
            .inferred_nonce(&command().fee_payer()),
        Some(command().nonce().unwrap().succ())
    );
    assert!(state.transaction_pool.pending_actions.is_empty());
    assert!(state.transaction_pool.account_reads_in_flight.is_empty());
    assert!(answers(&respond(&mut state, first, Ok(accounts()))).is_empty());
}

#[test]
fn full_bound_waits_and_ready_responses_do_not_open_unbounded_speculation() {
    let mut state = synced_state();
    let mut issued = Vec::new();
    for i in 0..6 {
        issued.extend(fetches(&submit(&mut state, i)));
    }
    assert_eq!(issued.len(), MAX_ACCOUNT_READS);
    assert_eq!(state.transaction_pool.pending_actions.len(), 6);
    for id in issued.iter().skip(1).rev() {
        let ran = respond(&mut state, *id, Ok(accounts()));
        assert!(fetches(&ran).is_empty());
        assert!(answers(&ran).is_empty());
        assert_eq!(
            state.transaction_pool.account_read_slots(),
            MAX_ACCOUNT_READS
        );
    }
    let ran = respond(&mut state, issued[0], Ok(accounts()));
    assert_eq!(answers(&ran).len(), 4);
    let rest = fetches(&ran);
    assert_eq!(rest.len(), 2);
    for id in rest {
        respond(&mut state, id, Ok(accounts()));
    }
    assert!(state.transaction_pool.pending_actions.is_empty());
    assert!(state.transaction_pool.account_reads_in_flight.is_empty());
}

#[test]
fn best_tip_change_cancels_old_accounts_and_retries_against_the_new_nonce() {
    let mut state = synced_state();
    let old = fetches(&submit(&mut state, 1))[0];
    let changed = run(
        &mut state,
        TransactionPoolAction::BestTipChanged {
            best_tip_hash: LedgerHash::zero(),
            diff: None,
        },
    );
    let reconcile = fetches(&changed)[0];
    assert!(answers(&respond(&mut state, old, Ok(accounts()))).is_empty());
    assert_eq!(state.transaction_pool.size(), 0);
    let next = respond(&mut state, reconcile, Ok(BTreeMap::new()));
    let retry = fetches(&next)[0];
    assert_ne!(old, retry);
    assert!(next.iter().any(|a| matches!(a, Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts { ledger_hash, .. }) if ledger_hash == &LedgerHash::zero())));
    let mut new_accounts = accounts();
    for account in new_accounts.values_mut() {
        account.nonce = account.nonce.succ();
    }
    let ran = respond(&mut state, retry, Ok(new_accounts));
    assert_eq!(
        answers(&ran),
        vec![(RpcId::new_unchecked(1, 1), "rejected")]
    );
    assert!(ran.iter().any(|a| matches!(a, Action::Rpc(RpcAction::TransactionInjectRejected { response, .. }) if matches!(response[0].1, ledger::transaction_pool::diff::Error::InvalidNonce))));
    assert_eq!(state.transaction_pool.size(), 0);
    assert!(state.transaction_pool.pending_actions.is_empty());
}

#[test]
fn tip_churn_keeps_cancelled_reads_in_the_physical_bound() {
    let mut state = synced_state();
    let mut ids = Vec::new();
    for i in 0..MAX_ACCOUNT_READS {
        ids.extend(fetches(&submit(&mut state, i as u32)));
    }
    for _ in 0..3 {
        let ran = run(
            &mut state,
            TransactionPoolAction::BestTipChanged {
                best_tip_hash: LedgerHash::zero(),
                diff: None,
            },
        );
        assert!(fetches(&ran).is_empty());
        assert_eq!(
            state.transaction_pool.account_reads_in_flight.len(),
            MAX_ACCOUNT_READS
        );
    }
    let ran = respond(&mut state, ids[0], Err("cancelled".into()));
    assert_eq!(fetches(&ran).len(), 1);
    assert!(answers(&ran).is_empty());
    assert_eq!(
        state.transaction_pool.account_reads_in_flight.len(),
        MAX_ACCOUNT_READS
    );
}

#[test]
fn failed_read_answers_once_and_unblocks_the_next_submission() {
    let mut state = synced_state();
    let first = fetches(&submit(&mut state, 1))[0];
    let second = fetches(&submit(&mut state, 2))[0];
    respond(&mut state, second, Ok(accounts()));
    let ran = respond(&mut state, first, Err("injected read failure".into()));
    assert_eq!(
        answers(&ran),
        vec![
            (RpcId::new_unchecked(1, 1), "failure"),
            (RpcId::new_unchecked(2, 1), "success")
        ]
    );
    assert!(state.transaction_pool.pending_actions.is_empty());
    assert!(answers(&respond(&mut state, first, Err("duplicate failure".into()))).is_empty());
}

#[test]
fn consecutive_frontier_deltas_remain_ordered() {
    let mut state = synced_state();
    let backtrack = BestTipDiff {
        new_commands: vec![],
        removed_commands: vec![WithStatus {
            data: command(),
            status: TransactionStatus::Applied,
        }],
        reorg_best_tip: true,
    };
    let first = run(
        &mut state,
        TransactionPoolAction::BestTipChanged {
            best_tip_hash: LedgerHash::zero(),
            diff: Some(backtrack),
        },
    );
    let first = fetches(&first)[0];
    let next_tip = state
        .transition_frontier
        .best_tip()
        .unwrap()
        .merkle_root_hash()
        .clone();
    let second = run(
        &mut state,
        TransactionPoolAction::BestTipChanged {
            best_tip_hash: next_tip,
            diff: None,
        },
    );
    assert!(fetches(&second).is_empty());
    let second = fetches(&respond(&mut state, first, Ok(accounts())))[0];
    assert_eq!(
        state.transaction_pool.size(),
        1,
        "the first delta was retained"
    );
    let mut committed = accounts();
    for account in committed.values_mut() {
        account.nonce = account.nonce.succ();
    }
    respond(&mut state, second, Ok(committed));
    assert_eq!(
        state.transaction_pool.size(),
        0,
        "the second snapshot revalidated the backtrack"
    );
}

/// Isolates the event-thread account-read stall, using the production ledger
/// worker and reducers. The synchronous comparator is the unchanged pre-fix
/// LedgerManager::get_accounts path. Delay models competing ledger work; no
/// proof-verification or network throughput claim is made by this benchmark.
#[test]
#[ignore = "manual 3000-submission event-loop stall measurement"]
fn transaction_pool_burst_stall_measurement() {
    use ledger::{BaseLedger, Database, Mask};
    use redux::EnablingCondition;
    use std::time::{Duration, Instant};

    fn dispatch(state: &mut State, action: Action, time: redux::Timestamp, ran: &mut Vec<Action>) {
        if !action.is_enabled(state, time) {
            return;
        }
        ran.push(action.clone());
        let action = redux::ActionMeta::zero_custom(time).with_action(action);
        let mut dispatcher = redux::Dispatcher::new();
        crate::reducer(state, &action, &mut dispatcher);
        while let Some(action) = dispatcher.pop() {
            dispatch(state, action, time, ran);
        }
    }
    fn effects(
        state: &mut State,
        ran: Vec<Action>,
        manager: &crate::ledger::LedgerManager,
        asynchronous: bool,
        time: redux::Timestamp,
        waits: &mut Vec<f64>,
        delay: Duration,
    ) {
        for action in ran {
            if let Action::TransactionPoolEffect(TransactionPoolEffectfulAction::FetchAccounts {
                account_ids,
                ledger_hash,
                pending_id,
                on_result,
                from_source,
                generation,
                pinned,
            }) = action
            {
                manager.delay_for_tests(delay);
                let start = Instant::now();
                if asynchronous {
                    manager.transaction_pool_accounts(
                        pending_id.unwrap(),
                        ledger_hash,
                        account_ids.into_iter().collect(),
                        generation,
                        pinned,
                    );
                    waits.push(start.elapsed().as_secs_f64() * 1000.0);
                } else {
                    let accounts = manager
                        .get_accounts(&ledger_hash, account_ids.into_iter().collect())
                        .map(|accounts| accounts.into_iter().map(|a| (a.id(), a)).collect());
                    waits.push(start.elapsed().as_secs_f64() * 1000.0);
                    let mut next = Vec::new();
                    dispatch(
                        state,
                        on_result.call((accounts, pending_id, from_source)),
                        time,
                        &mut next,
                    );
                    effects(state, next, manager, asynchronous, time, waits, delay);
                }
            }
        }
    }
    fn stats(mut values: Vec<f64>) -> serde_json::Value {
        values.sort_by(f64::total_cmp);
        let n = values.len();
        serde_json::json!({"n":n,"p50_ms":values[n/2],"p95_ms":values[n*95/100],"p99_ms":values[n*99/100],"max_ms":values[n-1],"total_ms":values.iter().sum::<f64>()})
    }

    let delay = Duration::from_micros(
        std::env::var("MINA_TXPOOL_BENCH_DELAY_US")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000),
    );
    let count = 3000;
    let time = super::submission_tests::now();
    let cmd = command();
    for asynchronous in [false, true] {
        let mut state = synced_state();
        let mut mask = Mask::new_root(Database::create(crate::ledger::LEDGER_DEPTH as u8));
        for (id, account) in accounts() {
            mask.get_or_create_account(id, account).unwrap();
        }
        let hash = LedgerHash::from_fp(mask.merkle_root());
        state.transaction_pool.best_tip_hash = Some(hash.clone());
        let mut ctx = crate::ledger::LedgerCtx::default();
        ctx.insert_genesis_ledger(mask);
        let (sender, mut receiver) = mina_core::channels::mpsc::unbounded_channel();
        ctx.set_event_sender(sender);
        let manager = crate::ledger::LedgerManager::spawn(ctx);
        let mut submitted = 0;
        let mut slices = Vec::new();
        let mut waits = Vec::new();
        let mut peak = 0;
        let started = Instant::now();
        while submitted < count || !state.transaction_pool.pending_actions.is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(180),
                "benchmark made no progress"
            );
            let action = if submitted < count {
                submitted += 1;
                Some(TransactionPoolAction::VerifySuccess {
                    valids: vec![cmd.clone()],
                    from_source: TransactionPoolMessageSource::None,
                })
            } else {
                None
            };
            let event = receiver.try_recv().ok();
            if action.is_none() && event.is_none() {
                std::thread::sleep(Duration::from_micros(100));
                continue;
            }
            let start = Instant::now();
            let mut ran = Vec::new();
            if let Some(action) = action {
                dispatch(&mut state, action.into(), time, &mut ran);
            }
            if let Some(crate::event_source::Event::Ledger(
                crate::ledger::LedgerEvent::TransactionPoolAccounts { id, result },
            )) = event
            {
                assert!(result.is_ok());
                dispatch(
                    &mut state,
                    TransactionPoolAction::AccountsFetched {
                        pending_id: id,
                        accounts: result,
                    }
                    .into(),
                    time,
                    &mut ran,
                );
            }
            effects(
                &mut state,
                ran,
                &manager,
                asynchronous,
                time,
                &mut waits,
                delay,
            );
            slices.push(start.elapsed().as_secs_f64() * 1000.0);
            peak = peak.max(state.transaction_pool.account_read_slots());
            assert!(peak <= MAX_ACCOUNT_READS);
        }
        assert_eq!(waits.len(), count);
        assert_eq!(state.transaction_pool.size(), 1);
        assert_eq!(
            state.transaction_pool.inferred_nonce(&cmd.fee_payer()),
            Some(cmd.nonce().unwrap().succ())
        );
        println!(
            "TXPOOL_BURST {}",
            serde_json::json!({
                "mode":if asynchronous {"asynchronous"} else {"synchronous_baseline"},
                "submissions":count,"injected_ledger_delay_us":delay.as_micros(),
                "elapsed_ms":started.elapsed().as_secs_f64()*1000.0,
                "account_read_call":stats(waits),"event_thread_active_slice":stats(slices),
                "peak_read_slots":peak,"final_pool_size":state.transaction_pool.size(),
            })
        );
    }
}

fn run_at(state: &mut State, time: redux::Timestamp, action: impl Into<Action>) -> Vec<Action> {
    use redux::EnablingCondition;
    fn dispatch(state: &mut State, time: redux::Timestamp, action: Action, ran: &mut Vec<Action>) {
        if !action.is_enabled(state, time) {
            return;
        }
        ran.push(action.clone());
        let mut dispatcher = redux::Dispatcher::new();
        crate::reducer(
            state,
            &redux::ActionMeta::zero_custom(time).with_action(action),
            &mut dispatcher,
        );
        while let Some(action) = dispatcher.pop() {
            dispatch(state, time, action, ran);
        }
    }
    let mut ran = Vec::new();
    dispatch(state, time, action.into(), &mut ran);
    ran
}

fn backtrack() -> BestTipDiff {
    BestTipDiff {
        new_commands: vec![],
        removed_commands: vec![WithStatus {
            data: command(),
            status: TransactionStatus::Applied,
        }],
        reorg_best_tip: true,
    }
}

/// The review's two-tip reproduction, with zero, one and repeated failures.
/// A failed read must not consume the backtracked-command delta or reorder it.
#[test]
fn review_198_failed_reconciliation_retains_backtracked_command() {
    for failures in [0, 1, 3] {
        let mut state = synced_state();
        let first = run(
            &mut state,
            TransactionPoolAction::BestTipChanged {
                best_tip_hash: LedgerHash::zero(),
                diff: Some(backtrack()),
            },
        );
        let mut first = fetches(&first)[0];
        let second = run(
            &mut state,
            TransactionPoolAction::BestTipChanged {
                best_tip_hash: LedgerHash::zero(),
                diff: None,
            },
        );
        assert!(fetches(&second).is_empty());
        let mut time = super::submission_tests::now();
        for _ in 0..failures {
            let failed = run_at(
                &mut state,
                time,
                TransactionPoolAction::AccountsFetched {
                    pending_id: first,
                    accounts: Err("Ledger unavailable".into()),
                },
            );
            assert!(fetches(&failed).is_empty());
            assert_eq!(state.transaction_pool.pending_actions.len(), 2);
            assert_eq!(state.transaction_pool.account_read_slots(), 0);
            assert_eq!(
                state.transaction_pool.account_read_order.front(),
                Some(&first)
            );
            let early = time + std::time::Duration::from_millis(999);
            assert!(!state.transaction_pool.account_read_retry_due(early));
            assert!(fetches(&run_at(
                &mut state,
                early,
                TransactionPoolAction::ProcessAccountReads
            ))
            .is_empty());
            time = time + ACCOUNT_READ_RETRY_DELAY;
            assert!(state.transaction_pool.account_read_retry_due(time));
            let retry = run_at(&mut state, time, TransactionPoolAction::ProcessAccountReads);
            let fresh = fetches(&retry)[0];
            assert_ne!(fresh, first);
            assert!(retry.iter().any(|a| matches!(a, Action::TransactionPoolEffect(TransactionPoolEffectfulAction::PinLedger { pending_id, .. }) if *pending_id == fresh)));
            assert!(run_at(
                &mut state,
                time,
                TransactionPoolAction::AccountsFetched {
                    pending_id: first,
                    accounts: Ok(accounts()),
                }
            )
            .iter()
            .all(|a| !matches!(
                a,
                Action::TransactionPool(TransactionPoolAction::BestTipChangedWithAccounts { .. })
            )));
            assert_eq!(
                state.transaction_pool.size(),
                0,
                "stale attempt cannot apply the delta"
            );
            first = fresh;
        }
        let next = run_at(
            &mut state,
            time,
            TransactionPoolAction::AccountsFetched {
                pending_id: first,
                accounts: Ok(accounts()),
            },
        );
        assert_eq!(
            state.transaction_pool.size(),
            1,
            "backtracked command restored before next tip"
        );
        let second = fetches(&next)[0];
        run_at(
            &mut state,
            time,
            TransactionPoolAction::AccountsFetched {
                pending_id: second,
                accounts: Ok(accounts()),
            },
        );
        assert!(!state.transaction_pool.reconciling());
        assert!(state.transaction_pool.pending_actions.is_empty());
        assert!(state.transaction_pool.account_reads_ready.is_empty());
        assert_eq!(state.transaction_pool.size(), 1);
        assert_eq!(
            state
                .transaction_pool
                .inferred_nonce(&command().fee_payer()),
            Some(command().nonce().unwrap().succ())
        );
    }
}

#[test]
fn failed_reorg_retry_restores_local_rebroadcast_tracking() {
    let mut state = synced_state();
    let pending = fetches(&submit(&mut state, 1))[0];
    respond(&mut state, pending, Ok(accounts()));
    let empty = BestTipDiff {
        new_commands: vec![],
        removed_commands: vec![],
        reorg_best_tip: false,
    };
    assert!(state
        .transaction_pool
        .pool
        .get_accounts_to_handle_transition_diff(&empty)
        .1
        .contains(&command().fee_payer()));

    // Establish the OCaml committed-local state before the failed reorg. The
    // core transition API moves local tracking from uncommitted to committed.
    let committed_diff = BestTipDiff {
        new_commands: backtrack().removed_commands,
        removed_commands: vec![],
        reorg_best_tip: false,
    };
    let (ids, _) = state
        .transaction_pool
        .pool
        .get_accounts_to_handle_transition_diff(&committed_diff);
    let mut committed_accounts = accounts();
    for account in committed_accounts.values_mut() {
        account.nonce = account.nonce.succ();
    }
    let (slot, since_genesis) = TransactionPoolState::global_slots(&state).unwrap();
    state
        .transaction_pool
        .pool
        .handle_transition_frontier_diff(
            since_genesis,
            slot,
            &committed_diff,
            &ids,
            &committed_accounts,
            &committed_accounts,
        )
        .unwrap();
    assert_eq!(state.transaction_pool.size(), 0);
    assert!(state
        .transaction_pool
        .pool
        .get_accounts_to_handle_transition_diff(&empty)
        .1
        .is_empty());

    let pending = run(
        &mut state,
        TransactionPoolAction::BestTipChanged {
            best_tip_hash: LedgerHash::zero(),
            diff: Some(backtrack()),
        },
    );
    let pending = fetches(&pending)[0];
    respond(&mut state, pending, Err("Ledger unavailable".into()));
    assert_eq!(state.transaction_pool.size(), 0);
    let time = super::submission_tests::now() + ACCOUNT_READ_RETRY_DELAY;
    let retry = run_at(&mut state, time, TransactionPoolAction::ProcessAccountReads);
    let retry = fetches(&retry)[0];
    run_at(
        &mut state,
        time,
        TransactionPoolAction::AccountsFetched {
            pending_id: retry,
            accounts: Ok(accounts()),
        },
    );
    assert_eq!(state.transaction_pool.size(), 1);
    assert!(
        state
            .transaction_pool
            .pool
            .get_accounts_to_handle_transition_diff(&empty)
            .1
            .contains(&command().fee_payer()),
        "a backtracked local transaction must be eligible for local rebroadcast again"
    );
    assert!(state.transaction_pool.pending_actions.is_empty());
}

#[test]
fn failed_snapshot_without_delta_is_superseded_by_a_new_tip() {
    for new_tip_first in [false, true] {
        let mut state = synced_state();
        let pending = run(
            &mut state,
            TransactionPoolAction::BestTipChanged {
                best_tip_hash: LedgerHash::zero(),
                diff: None,
            },
        );
        let pending = fetches(&pending)[0];
        let change = TransactionPoolAction::BestTipChanged {
            best_tip_hash: ledger_hash_for_tip(&state),
            diff: None,
        };
        let next = if new_tip_first {
            run(&mut state, change);
            respond(&mut state, pending, Err("Ledger unavailable".into()))
        } else {
            respond(&mut state, pending, Err("Ledger unavailable".into()));
            run(&mut state, change)
        };
        let next = fetches(&next)[0];
        respond(&mut state, next, Ok(accounts()));
        assert!(state.transaction_pool.pending_actions.is_empty());
    }
    fn ledger_hash_for_tip(state: &State) -> LedgerHash {
        state
            .transition_frontier
            .best_tip()
            .unwrap()
            .merkle_root_hash()
            .clone()
    }
}

#[test]
fn tip_change_releases_buffered_response_slots() {
    let mut state = synced_state();
    let first = fetches(&submit(&mut state, 1))[0];
    let second = fetches(&submit(&mut state, 2))[0];
    respond(&mut state, second, Ok(accounts()));
    assert_eq!(state.transaction_pool.account_reads_ready.len(), 1);
    let changed = run(
        &mut state,
        TransactionPoolAction::BestTipChanged {
            best_tip_hash: LedgerHash::zero(),
            diff: None,
        },
    );
    assert!(state.transaction_pool.account_reads_ready.is_empty());
    assert_eq!(state.transaction_pool.account_read_slots(), 2);
    let barrier = fetches(&changed)[0];
    respond(&mut state, first, Err("cancelled".into()));
    let retried = respond(&mut state, barrier, Ok(accounts()));
    let retried = fetches(&retried);
    assert_eq!(retried.len(), 2);
    for id in retried {
        respond(&mut state, id, Ok(accounts()));
    }
    assert_eq!(state.transaction_pool.account_read_slots(), 0);
    assert!(state.transaction_pool.account_reads_ready.is_empty());
}
