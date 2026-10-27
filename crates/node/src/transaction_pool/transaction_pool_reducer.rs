use crate::p2p::{
    channels::transaction::P2pChannelsTransactionAction, BroadcastMessageId, P2pNetworkPubsubAction,
};
use ledger::{
    scan_state::transaction_logic::{valid, GenericCommand, UserCommand},
    transaction_pool::{
        diff::{self, DiffVerified},
        transaction_hash, ApplyDecision, TransactionPoolErrors,
    },
    Account, AccountId,
};
use mina_core::{
    bug_condition,
    constants::constraint_constants,
    transaction::{Transaction, TransactionPoolMessageSource, TransactionWithHash},
};
use mina_snark::user_command_verify::{SnarkUserCommandVerifyAction, SnarkUserCommandVerifyId};
use redux::callback;
use std::collections::{BTreeMap, BTreeSet};

use crate::{rpc::TRANSACTION_INJECT_BOOTSTRAPPING, BlockProducerAction, RpcAction};

use super::{
    PendingId, TransactionPoolAction, TransactionPoolActionWithMetaRef,
    TransactionPoolEffectfulAction, TransactionPoolState, TransactionState,
};

impl TransactionPoolState {
    pub fn reducer(mut state: crate::Substate<Self>, action: TransactionPoolActionWithMetaRef<'_>) {
        // Uncoment following line to save actions to `/tmp/pool.bin`
        // Self::save_actions(&mut state);

        let substate = state.get_substate_mut().unwrap();
        if let Some(file) = substate.file.as_mut() {
            postcard::to_io(&action, file).unwrap();
        };

        Self::handle_action(state, action)
    }

    pub(super) fn handle_action(
        mut state: crate::Substate<Self>,
        action: TransactionPoolActionWithMetaRef<'_>,
    ) {
        let (action, meta) = action.split();
        // OCaml counts a gossip diff on receipt, before validation.
        // OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L1518-L1523
        if matches!(
            action,
            TransactionPoolAction::Candidate(
                super::candidate::TransactionPoolCandidateAction::Libp2pTransactionsReceived { .. }
            )
        ) {
            if let Ok(pool) = state.get_substate_mut() {
                pool.telemetry.diffs_received = pool.telemetry.diffs_received.saturating_add(1);
            }
        }
        let Some((global_slot, global_slot_from_genesis)) =
            // TODO: remove usage of `unsafe_get_state`
            Self::global_slots(state.unsafe_get_state())
        else {
            // No genesis block yet. A submission still gets its answer.
            if let TransactionPoolAction::StartVerify { from_source, .. } = action {
                answer_refusal(
                    state.into_dispatcher(),
                    from_source,
                    vec![TRANSACTION_INJECT_BOOTSTRAPPING.to_owned()],
                );
            }
            return;
        };
        let Ok(substate) = state.get_substate_mut() else {
            bug_condition!("transaction pool substate missing");
            return;
        };

        match action {
            TransactionPoolAction::Candidate(a) => {
                super::candidate::TransactionPoolCandidatesState::reducer(
                    mina_core::Substate::from_compatible_substate(state),
                    meta.with_action(a),
                );
            }
            TransactionPoolAction::StartVerify {
                commands,
                from_source,
            } => {
                let commands = match commands
                    .iter()
                    .map(TransactionWithHash::body)
                    .map(UserCommand::try_from)
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(commands) => commands,
                    Err(e) => {
                        // Refuse all commands if one is invalid. A field
                        // element out of range is a decode failure in OCaml.
                        answer_refusal(
                            state.into_dispatcher(),
                            from_source,
                            vec![format!("invalid transaction: {e}")],
                        );
                        return;
                    }
                };

                let account_ids = commands
                    .iter()
                    .flat_map(UserCommand::accounts_referenced)
                    .collect::<BTreeSet<_>>();
                let Some(best_tip_hash) = substate.best_tip_hash.clone() else {
                    answer_refusal(
                        state.into_dispatcher(),
                        from_source,
                        vec![TRANSACTION_INJECT_BOOTSTRAPPING.to_owned()],
                    );
                    return;
                };
                let pending_id = substate.make_action_pending(action);

                let dispatcher = state.into_dispatcher();
                dispatcher.push(TransactionPoolEffectfulAction::FetchAccounts {
                    account_ids,
                    ledger_hash: best_tip_hash.clone(),
                    on_result: callback!(fetch_to_verify((accounts: Result<BTreeMap<AccountId, Account>, String>, id: Option<PendingId>, from_source: TransactionPoolMessageSource)) -> crate::Action {
                        match (accounts, id) {
                            (Ok(accounts), Some(pending_id)) => TransactionPoolAction::StartVerifyWithAccounts { accounts, pending_id, from_source },
                            (Err(error), pending_id) => TransactionPoolAction::FetchAccountsFailed { pending_id, from_source, error },
                            (Ok(_), None) => TransactionPoolAction::FetchAccountsFailed { pending_id: None, from_source, error: "no pending action".to_owned() },
                        }
                    }),
                    pending_id: Some(pending_id),
                    from_source: *from_source,
                });
            }
            TransactionPoolAction::StartVerifyWithAccounts {
                accounts,
                pending_id,
                from_source,
            } => {
                let Some(TransactionPoolAction::StartVerify { commands, .. }) =
                    substate.pending_actions.remove(pending_id)
                else {
                    bug_condition!("StartVerifyWithAccounts: no pending StartVerify");
                    answer_refusal(
                        state.into_dispatcher(),
                        from_source,
                        vec!["internal error: no pending verification".to_owned()],
                    );
                    return;
                };

                // TODO: Convert those commands only once
                let commands = match commands
                    .iter()
                    .map(TransactionWithHash::body)
                    .map(UserCommand::try_from)
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(commands) => commands,
                    Err(e) => {
                        answer_refusal(
                            state.into_dispatcher(),
                            from_source,
                            vec![format!("invalid transaction: {e}")],
                        );
                        return;
                    }
                };
                let diff = diff::Diff { list: commands };

                match substate
                    .pool
                    .prevalidate(diff)
                    .and_then(|diff| substate.pool.convert_diff_to_verifiable(diff, accounts))
                {
                    Ok(verifiable) => {
                        let (dispatcher, global_state) = state.into_dispatcher_and_state();
                        let req_id = global_state.snark.user_command_verify.next_req_id();

                        dispatcher.push(SnarkUserCommandVerifyAction::Init {
                            req_id,
                            commands: verifiable,
                            from_source: *from_source,
                            on_success: callback!(
                                on_snark_user_command_verify_success(
                                    (req_id: SnarkUserCommandVerifyId, valids: Vec<valid::UserCommand>, from_source: TransactionPoolMessageSource)
                                ) -> crate::Action {
                                    TransactionPoolAction::VerifySuccess {
                                        valids,
                                        from_source,
                                    }
                                }
                            ),
                            on_error: callback!(
                                on_snark_user_command_verify_error(
                                    (req_id: SnarkUserCommandVerifyId, errors: Vec<String>, from_source: TransactionPoolMessageSource)
                                ) -> crate::Action {
                                    TransactionPoolAction::VerifyError { errors, from_source }
                                }
                            )
                        });
                    }
                    Err(e) => {
                        // `VerifyError` answers the source.
                        let errors = match e {
                            TransactionPoolErrors::BatchedErrors(errors) => {
                                errors.into_iter().map(|e| e.to_string()).collect()
                            }
                            TransactionPoolErrors::LoadingVK(error) => vec![error],
                            TransactionPoolErrors::Unexpected(es) => {
                                bug_condition!("transaction pool: unexpected error: {es}");
                                vec![es]
                            }
                        };
                        state
                            .into_dispatcher()
                            .push(TransactionPoolAction::VerifyError {
                                errors,
                                from_source: *from_source,
                            });
                    }
                }
            }
            TransactionPoolAction::VerifySuccess {
                valids,
                from_source,
            } => {
                let valids = valids
                    .iter()
                    .cloned()
                    .map(transaction_hash::hash_command)
                    .collect::<Vec<_>>();
                let Some(best_tip_hash) = substate.best_tip_hash.clone() else {
                    answer_refusal(
                        state.into_dispatcher(),
                        from_source,
                        vec![TRANSACTION_INJECT_BOOTSTRAPPING.to_owned()],
                    );
                    return;
                };
                let diff = DiffVerified { list: valids };

                let dispatcher = state.into_dispatcher();
                dispatcher.push(TransactionPoolAction::ApplyVerifiedDiff {
                    best_tip_hash,
                    diff,
                    from_source: *from_source,
                });
            }
            TransactionPoolAction::VerifyError {
                errors,
                from_source,
            } => {
                // A submission gets the errors. A gossiped diff is ignored,
                // not rejected: our rejection disconnects the peer (P6f), and
                // OCaml peers forward only what they verified, so a rejection
                // here can be ours to blame (a verification key we lack).
                answer_refusal(state.into_dispatcher(), from_source, errors.clone());
            }
            TransactionPoolAction::FetchAccountsFailed {
                pending_id,
                from_source,
                error,
            } => {
                if let Some(pending_id) = pending_id {
                    substate.pending_actions.remove(pending_id);
                }
                answer_refusal(state.into_dispatcher(), from_source, vec![error.clone()]);
            }
            TransactionPoolAction::BestTipChanged { best_tip_hash } => {
                let account_ids = substate.pool.get_accounts_to_revalidate_on_new_best_tip();
                substate.best_tip_hash = Some(best_tip_hash.clone());

                let dispatcher = state.into_dispatcher();
                dispatcher.push(TransactionPoolEffectfulAction::FetchAccounts {
                    account_ids,
                    ledger_hash: best_tip_hash.clone(),
                    on_result: callback!(fetch_for_best_tip((accounts: Result<BTreeMap<AccountId, Account>, String>, id: Option<PendingId>, from_source: TransactionPoolMessageSource)) -> crate::Action {
                        match accounts {
                            Ok(accounts) => TransactionPoolAction::BestTipChangedWithAccounts { accounts },
                            Err(error) => TransactionPoolAction::FetchAccountsFailed { pending_id: id, from_source, error },
                        }
                    }),
                    pending_id: None,
                    from_source: TransactionPoolMessageSource::None,
                });
            }
            TransactionPoolAction::BestTipChangedWithAccounts { accounts } => {
                match substate
                    .pool
                    .on_new_best_tip(global_slot_from_genesis, accounts)
                {
                    Err(e) => bug_condition!("transaction pool::on_new_best_tip failed: {:?}", e),
                    Ok(dropped) => {
                        for tx in dropped {
                            substate.dpool.remove(&tx.hash);
                        }
                    }
                }
            }
            TransactionPoolAction::ApplyVerifiedDiff {
                best_tip_hash,
                diff,
                from_source,
            } => {
                let account_ids = substate.pool.get_accounts_to_apply_diff(diff);
                let pending_id = substate.make_action_pending(action);

                let dispatcher = state.into_dispatcher();
                dispatcher.push(TransactionPoolEffectfulAction::FetchAccounts {
                    account_ids,
                    ledger_hash: best_tip_hash.clone(),
                    on_result: callback!(fetch_for_apply((accounts: Result<BTreeMap<AccountId, Account>, String>, id: Option<PendingId>, from_source: TransactionPoolMessageSource)) -> crate::Action {
                        match (accounts, id) {
                            (Ok(accounts), Some(pending_id)) => TransactionPoolAction::ApplyVerifiedDiffWithAccounts {
                                accounts,
                                pending_id,
                            },
                            (Err(error), pending_id) => TransactionPoolAction::FetchAccountsFailed { pending_id, from_source, error },
                            (Ok(_), None) => TransactionPoolAction::FetchAccountsFailed { pending_id: None, from_source, error: "no pending action".to_owned() },
                        }
                    }),
                    pending_id: Some(pending_id),
                    from_source: *from_source,
                });
            }
            TransactionPoolAction::ApplyVerifiedDiffWithAccounts {
                accounts,
                pending_id,
            } => {
                let Some(TransactionPoolAction::ApplyVerifiedDiff {
                    best_tip_hash: _,
                    diff,
                    from_source,
                }) = substate.pending_actions.remove(pending_id)
                else {
                    bug_condition!("ApplyVerifiedDiffWithAccounts: no pending ApplyVerifiedDiff");
                    return;
                };
                let is_sender_local = from_source.is_sender_local();
                let pool_size_before = substate.pool.size();

                // Note(adonagy): Action for rebroadcast, in his action we can use forget_check
                let (was_accepted, accepted, rejected) = match substate.pool.unsafe_apply(
                    meta.time(),
                    global_slot_from_genesis,
                    global_slot,
                    &diff,
                    accounts,
                    is_sender_local,
                ) {
                    Ok((ApplyDecision::Accept, accepted, rejected, dropped)) => {
                        for hash in dropped {
                            substate.dpool.remove(&hash);
                        }
                        for tx in &accepted {
                            substate.dpool.insert(TransactionState {
                                time: meta.time(),
                                hash: tx.hash.clone(),
                            });
                        }

                        (true, accepted, rejected)
                    }
                    Ok((ApplyDecision::Reject, accepted, rejected, _)) => {
                        (false, accepted, rejected)
                    }
                    Err(e) => {
                        crate::core::warn!(meta.time(); kind = "TransactionPoolUnsafeApplyError", summary = e.clone());
                        answer_refusal(state.into_dispatcher(), &from_source, vec![e]);
                        return;
                    }
                };

                // OCaml increments by the positive net pool-size change after
                // applying a diff, including local submissions.
                // OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/network_pool/transaction_pool.ml#L1451-L1458
                let added = substate.pool.size().saturating_sub(pool_size_before) as u64;
                substate.telemetry.transactions_added =
                    substate.telemetry.transactions_added.saturating_add(added);

                // Accepted incoming diffs are forwarded once by pubsub.
                // OCaml sends accepted pool diffs through the same broadcast
                // loop as local submissions.
                // OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_lib/mina_lib.ml#L2264-L2284
                if was_accepted
                    && matches!(from_source, TransactionPoolMessageSource::Pubsub { .. })
                {
                    substate.telemetry.diffs_broadcasted =
                        substate.telemetry.diffs_broadcasted.saturating_add(1);
                }

                let dispatcher = state.into_dispatcher();

                // TODO: use callbacks
                match (was_accepted, from_source) {
                    (true, TransactionPoolMessageSource::Rpc { id }) => {
                        // Note: even though the diff was labeled as accepted the specific tx could be rejected
                        //       (if it is not grounds for diff rejection)
                        if !rejected.is_empty() {
                            dispatcher.push(RpcAction::TransactionInjectRejected {
                                rpc_id: id,
                                response: rejected.clone(),
                            });
                        } else if !accepted.is_empty() {
                            dispatcher.push(RpcAction::TransactionInjectSuccess {
                                rpc_id: id,
                                response: accepted.clone(),
                            });
                        } else {
                            // Accepted but dropped at once (pool full of
                            // higher fees): neither list has it.
                            // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_commands/mina_commands.ml#L89-L109>
                            dispatcher.push(RpcAction::TransactionInjectFailure {
                                rpc_id: id,
                                errors: vec![
                                    "Internal error while scheduling a user command".to_owned()
                                ],
                            });
                        }
                    }
                    (true, TransactionPoolMessageSource::Pubsub { id }) => {
                        dispatcher.push(P2pNetworkPubsubAction::BroadcastValidatedMessage {
                            message_id: BroadcastMessageId::MessageId { message_id: id },
                        });
                    }
                    (false, TransactionPoolMessageSource::Rpc { id }) => {
                        dispatcher.push(RpcAction::TransactionInjectRejected {
                            rpc_id: id,
                            response: rejected.clone(),
                        });
                    }
                    (false, TransactionPoolMessageSource::Pubsub { id }) => {
                        dispatcher.push(P2pNetworkPubsubAction::RejectMessage {
                            message_id: Some(BroadcastMessageId::MessageId { message_id: id }),
                            peer_id: None,
                            reason: "Rejected transaction diff".to_owned(),
                        });
                    }
                    (_, TransactionPoolMessageSource::None) => {}
                }

                if was_accepted && !from_source.is_libp2p() {
                    dispatcher.push(TransactionPoolAction::Rebroadcast {
                        accepted,
                        rejected,
                        is_local: is_sender_local,
                    });
                }
            }
            TransactionPoolAction::ApplyTransitionFrontierDiff {
                best_tip_hash,
                diff,
            } => {
                if substate.best_tip_hash.as_ref() != Some(best_tip_hash) {
                    bug_condition!(
                        "ApplyTransitionFrontierDiff for {best_tip_hash}, but the pool's best tip is {:?}",
                        substate.best_tip_hash
                    );
                    return;
                }

                let (account_ids, uncommitted) =
                    substate.pool.get_accounts_to_handle_transition_diff(diff);
                let pending_id = substate.make_action_pending(action);

                let dispatcher = state.into_dispatcher();
                dispatcher.push(TransactionPoolEffectfulAction::FetchAccounts {
                    account_ids: account_ids.union(&uncommitted).cloned().collect(),
                    ledger_hash: best_tip_hash.clone(),
                    on_result: callback!(fetch_for_diff((accounts: Result<BTreeMap<AccountId, Account>, String>, id: Option<PendingId>, from_source: TransactionPoolMessageSource)) -> crate::Action {
                        match (accounts, id) {
                            (Ok(accounts), Some(pending_id)) => TransactionPoolAction::ApplyTransitionFrontierDiffWithAccounts {
                                accounts,
                                pending_id,
                            },
                            (Err(error), pending_id) => TransactionPoolAction::FetchAccountsFailed { pending_id, from_source, error },
                            (Ok(_), None) => TransactionPoolAction::FetchAccountsFailed { pending_id: None, from_source, error: "no pending action".to_owned() },
                        }
                    }),
                    pending_id: Some(pending_id),
                    from_source: TransactionPoolMessageSource::None,
                });
            }
            TransactionPoolAction::ApplyTransitionFrontierDiffWithAccounts {
                accounts,
                pending_id,
            } => {
                let Some(TransactionPoolAction::ApplyTransitionFrontierDiff {
                    best_tip_hash: _,
                    diff,
                }) = substate.pending_actions.remove(pending_id)
                else {
                    bug_condition!(
                        "ApplyTransitionFrontierDiffWithAccounts: no pending ApplyTransitionFrontierDiff"
                    );
                    return;
                };

                let collect = |set: &BTreeSet<AccountId>| {
                    set.iter()
                        .filter_map(|id| {
                            let account = accounts.get(id).cloned()?;
                            Some((id.clone(), account))
                        })
                        .collect::<BTreeMap<_, _>>()
                };

                let (account_ids, uncommitted) =
                    substate.pool.get_accounts_to_handle_transition_diff(&diff);

                let in_cmds = collect(&account_ids);
                let uncommitted = collect(&uncommitted);

                if let Err(e) = substate.pool.handle_transition_frontier_diff(
                    global_slot_from_genesis,
                    global_slot,
                    &diff,
                    &account_ids,
                    &in_cmds,
                    &uncommitted,
                ) {
                    bug_condition!(
                        "transaction pool::handle_transition_frontier_diff failed: {:?}",
                        e
                    );
                }
            }
            TransactionPoolAction::Rebroadcast {
                accepted,
                rejected,
                is_local,
            } => {
                // One action is one outgoing diff, even when it contains
                // several commands. OCaml counts the broadcast call.
                // OCaml: https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_networking/mina_networking.ml#L211-L223
                substate.telemetry.diffs_broadcasted =
                    substate.telemetry.diffs_broadcasted.saturating_add(1);
                let rejected = rejected.iter().map(|(cmd, _)| cmd.data.forget_check());

                let all_commands = accepted
                    .iter()
                    .map(|cmd| cmd.data.forget_check())
                    .chain(rejected)
                    .collect::<Vec<_>>();

                let dispatcher = state.into_dispatcher();

                for cmd in all_commands {
                    dispatcher.push(P2pChannelsTransactionAction::Libp2pBroadcast {
                        transaction: Box::new((&cmd).into()),
                        nonce: 0,
                        is_local: *is_local,
                    });
                }
            }
            TransactionPoolAction::CollectTransactionsByFee => {
                let transaction_capacity =
                    2u64.pow(constraint_constants().transaction_capacity_log_2 as u32);
                let transactions_by_fee = substate
                    .pool
                    .list_includable_transactions(transaction_capacity as usize)
                    .into_iter()
                    .map(|cmd| cmd.data)
                    .collect::<Vec<_>>();

                let dispatcher = state.into_dispatcher();

                dispatcher.push(BlockProducerAction::WonSlotTransactionsSuccess {
                    transactions_by_fee,
                });
            }
            TransactionPoolAction::P2pSendAll => {
                let (dispatcher, global_state) = state.into_dispatcher_and_state();
                for peer_id in global_state.p2p.ready_peers() {
                    dispatcher.push(TransactionPoolAction::P2pSend { peer_id });
                }
            }
            TransactionPoolAction::P2pSend { peer_id } => {
                let peer_id = *peer_id;
                let (dispatcher, global_state) = state.into_dispatcher_and_state();
                let Some(peer) = global_state.p2p.get_ready_peer(&peer_id) else {
                    return;
                };

                // Send commitments.
                let index_and_limit = peer.channels.transaction.next_send_index_and_limit();
                let (transactions, first_index, last_index) = global_state
                    .transaction_pool
                    .dpool
                    .next_messages_to_send(index_and_limit, |state| {
                        let tx = global_state.transaction_pool.get(&state.hash)?;
                        let tx = tx.clone().forget();
                        // TODO(binier): avoid conversion
                        Some((&Transaction::from(&tx)).into())
                    });

                dispatcher.push(P2pChannelsTransactionAction::ResponseSend {
                    peer_id,
                    transactions,
                    first_index,
                    last_index,
                });
            }
        }
    }
}

/// Answers a submission or a gossiped diff that the pool won't take, so that
/// every RPC request gets exactly one answer and no gossip waits for one.
///
/// A gossiped diff is ignored: it is never forwarded, and the peer is kept.
fn answer_refusal(
    dispatcher: &mut redux::Dispatcher<crate::Action, crate::State>,
    from_source: &TransactionPoolMessageSource,
    errors: Vec<String>,
) {
    match from_source {
        TransactionPoolMessageSource::Rpc { id } => {
            dispatcher.push(RpcAction::TransactionInjectFailure {
                rpc_id: *id,
                errors,
            });
        }
        TransactionPoolMessageSource::Pubsub { id } => {
            dispatcher.push(P2pNetworkPubsubAction::IgnoreMessage {
                message_id: Some(BroadcastMessageId::MessageId { message_id: *id }),
                reason: errors.join("; "),
            });
        }
        TransactionPoolMessageSource::None => {}
    }
}
