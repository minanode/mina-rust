use std::mem;

use mina_core::{bug_condition, Substate, SubstateAccess};
use redux::ActionWithMeta;

use crate::{
    bootstrap::{
        P2pNetworkKadBootstrapFailedRequest, P2pNetworkKadBootstrapOngoingRequest,
        P2pNetworkKadBootstrapRequestStat, P2pNetworkKadBootstrapSuccessfulRequest,
    },
    connection::outgoing::P2pConnectionOutgoingInitOpts,
    P2pNetworkKadEffectfulAction, P2pNetworkKadRequestAction, P2pNetworkKadState,
    P2pNetworkKademliaAction, P2pState,
};

use super::{
    P2pNetworkKadBootstrapAction, P2pNetworkKadBootstrapNextStep, P2pNetworkKadBootstrapState,
};

impl P2pNetworkKadBootstrapState {
    pub fn reducer<Action, State>(
        mut state_context: Substate<Action, State, P2pState>,
        action: ActionWithMeta<P2pNetworkKadBootstrapAction>,
        filter_addrs: bool,
    ) -> Result<(), String>
    where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        let (action, meta) = action.split();

        match action {
            P2pNetworkKadBootstrapAction::CreateRequests => {
                let discovery_state: &P2pNetworkKadState =
                    state_context.get_substate()?.substate()?;
                let routing_table = &discovery_state.routing_table;
                let bootstrap_state: &Self = state_context.get_substate()?.substate()?;

                let to_request = routing_table
                    .closest_peers(&bootstrap_state.kademlia_key) // for the next request we take closest peer
                    .filter(|entry| !bootstrap_state.processed_peers.contains(&entry.peer_id))
                    .cloned()
                    .collect::<Vec<_>>();

                let bootstrap_state: &mut Self =
                    state_context.get_substate_mut()?.substate_mut()?;
                bootstrap_state.requests_number = to_request.len();
                let empty = to_request.is_empty();

                let dispatcher = state_context.into_dispatcher();
                for entry in to_request {
                    dispatcher.push(P2pNetworkKadEffectfulAction::MakeRequest {
                        multiaddr: entry.addresses().clone(),
                        filter_local: filter_addrs,
                        peer_id: entry.peer_id,
                    });
                }
                if empty {
                    dispatcher.push(P2pNetworkKadBootstrapAction::FinalizeRequests);
                }

                Ok(())
            }
            P2pNetworkKadBootstrapAction::AppendRequest { request, peer_id } => {
                let state: &mut Self = state_context.get_substate_mut()?.substate_mut()?;
                // Saturating: a batch started while another was being
                // assembled used to drain the counter first, and the outer
                // batch's appends then took it below zero (a panic with
                // overflow checks, a wrap without, after which the batch was
                // finalized only in threes). `next_step` no longer starts
                // such a batch; this keeps the counter sane regardless.
                state.requests_number = state.requests_number.saturating_sub(1);
                // A peer this lookup has asked already, or that this batch
                // holds, is not asked again.
                if let Some(request) = request.filter(|_| !state.is_known(&peer_id)) {
                    state.peer_id_req_vec.push((peer_id, request));
                }
                if state.peer_id_req_vec.len() == 3 || state.requests_number == 0 {
                    let dispatcher = state_context.into_dispatcher();
                    dispatcher.push(P2pNetworkKadBootstrapAction::FinalizeRequests);
                }
                Ok(())
            }
            P2pNetworkKadBootstrapAction::FinalizeRequests => {
                let state: &mut Self = state_context.get_substate_mut()?.substate_mut()?;
                for (peer_id, request) in mem::take(&mut state.peer_id_req_vec) {
                    if !state.processed_peers.insert(peer_id) {
                        // Asked already in this lookup.
                        continue;
                    }
                    let address =
                        P2pConnectionOutgoingInitOpts::LibP2P((peer_id, request.addr).into());
                    state
                        .stats
                        .requests
                        .push(P2pNetworkKadBootstrapRequestStat::Ongoing(
                            P2pNetworkKadBootstrapOngoingRequest {
                                peer_id,
                                address,
                                start: meta.time(),
                            },
                        ));
                    state.requests.insert(peer_id, request);
                }

                let (dispatcher, state) = state_context.into_dispatcher_and_state();
                let bootstrap_state: &P2pNetworkKadBootstrapState = state.substate()?;
                let discovery_state: &P2pNetworkKadState = state.substate()?;
                let key = bootstrap_state.key;

                if bootstrap_state.requests.is_empty() {
                    dispatcher.push(P2pNetworkKademliaAction::BootstrapFinished {});
                } else {
                    bootstrap_state
                        .requests
                        .iter()
                        .filter_map(|(peer_id, req)| {
                            (!discovery_state.requests.contains_key(peer_id)).then_some(
                                P2pNetworkKadRequestAction::New {
                                    peer_id: *peer_id,
                                    addr: req.addr,
                                    key,
                                },
                            )
                        })
                        .for_each(|action| dispatcher.push(action));
                }

                Ok(())
            }
            P2pNetworkKadBootstrapAction::RequestDone {
                peer_id,
                closest_peers,
            } => {
                let state: &mut P2pNetworkKadBootstrapState =
                    state_context.get_substate_mut()?.substate_mut()?;
                let Some(req) = state.requests.remove(&peer_id) else {
                    bug_condition!("cannot find request for peer {peer_id}");
                    return Ok(());
                };
                state.successful_requests += 1;
                let address = P2pConnectionOutgoingInitOpts::LibP2P((peer_id, req.addr).into());

                if let Some(request_stats) =
                    state.stats.requests.iter_mut().rev().find(|req_stat| {
                        matches!(
                            req_stat,
                            P2pNetworkKadBootstrapRequestStat::Ongoing(
                                P2pNetworkKadBootstrapOngoingRequest {
                                    address: a,
                                    ..
                                },
                            ) if a == &address
                        )
                    })
                {
                    *request_stats = P2pNetworkKadBootstrapRequestStat::Successful(
                        P2pNetworkKadBootstrapSuccessfulRequest {
                            peer_id,
                            address,
                            start: req.time,
                            finish: meta.time(),
                            closest_peers,
                        },
                    );
                } else {
                    bug_condition!("cannot find stats for request {req:?}");
                };

                let step = state.next_step();
                Self::dispatch_next_step(state_context.into_dispatcher(), step);
                Ok(())
            }
            P2pNetworkKadBootstrapAction::RequestError { peer_id, error } => {
                let bootstrap_state: &mut P2pNetworkKadBootstrapState =
                    state_context.get_substate_mut()?.substate_mut()?;

                let Some(req) = bootstrap_state.requests.remove(&peer_id) else {
                    bug_condition!("cannot find request for peer {peer_id}");
                    return Ok(());
                };

                let address = P2pConnectionOutgoingInitOpts::LibP2P((peer_id, req.addr).into());
                if let Some(request_stats) =
                    bootstrap_state
                        .stats
                        .requests
                        .iter_mut()
                        .rev()
                        .find(|req_stat| {
                            matches!(
                                req_stat,
                                P2pNetworkKadBootstrapRequestStat::Ongoing(
                                    P2pNetworkKadBootstrapOngoingRequest {
                                        address: a,
                                        ..
                                    },
                                ) if a == &address
                            )
                        })
                {
                    *request_stats = P2pNetworkKadBootstrapRequestStat::Failed(
                        P2pNetworkKadBootstrapFailedRequest {
                            peer_id,
                            address,
                            start: req.time,
                            finish: meta.time(),
                            error,
                        },
                    );
                } else {
                    bug_condition!("cannot find stats for request {req:?}");
                };

                let step = bootstrap_state.next_step();
                Self::dispatch_next_step(state_context.into_dispatcher(), step);
                Ok(())
            }
        }
    }

    /// After a request ended: ask more peers while the lookup has had fewer
    /// than [`Self::SUCCESSFUL_REQUESTS`] successful ones, and otherwise end
    /// it once the last request in flight has ended. Before I18b nothing ended
    /// a lookup that reached 20 successful requests, so Kademlia stayed
    /// `Bootstrapping`, never looked up again, and the node discovered no
    /// peer after its first 16 seconds on mainnet.
    ///
    /// Nothing starts while a batch is being assembled
    /// ([`Self::next_step`]). Dispatch is depth first, so a request refused
    /// at once (the backoff, the target, the seed policy) ends inside the
    /// batch that made it; a new batch there asked again the peers the outer
    /// batch had still to append, and drained `requests_number` under it.
    /// The batch in progress already covers every peer not yet asked, and
    /// `FinalizeRequests` ends the lookup if it leaves nothing in flight.
    fn dispatch_next_step<Action, State>(
        dispatcher: &mut redux::Dispatcher<Action, State>,
        step: P2pNetworkKadBootstrapNextStep,
    ) where
        State: crate::P2pStateTrait,
        Action: crate::P2pActionTrait<State>,
    {
        match step {
            P2pNetworkKadBootstrapNextStep::AskMore => {
                dispatcher.push(P2pNetworkKadBootstrapAction::CreateRequests);
            }
            P2pNetworkKadBootstrapNextStep::Finish => {
                dispatcher.push(P2pNetworkKademliaAction::BootstrapFinished {});
            }
            P2pNetworkKadBootstrapNextStep::Wait => {}
        }
    }
}
