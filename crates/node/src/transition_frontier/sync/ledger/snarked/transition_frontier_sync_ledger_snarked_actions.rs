use mina_core::ActionEvent;
use mina_p2p_messages::v2::{LedgerHash, MinaBaseAccountBinableArgStableV2};
use serde::{Deserialize, Serialize};

use crate::{
    ledger::{LedgerAddress, LEDGER_DEPTH},
    p2p::{channels::rpc::P2pRpcId, PeerId},
    transition_frontier::sync::ledger::TransitionFrontierSyncLedgerState,
};

use super::{
    PeerLedgerQueryError, PeerLedgerQueryResponse, PeerRpcState,
    TransitionFrontierSyncLedgerSnarkedState,
};

/// Once we reach subtrees of this height, we begin performing
/// queries to fetch all the accounts in the subtree at once
/// instead of fetching intermediary hashes.
pub const ACCOUNT_SUBTREE_HEIGHT: usize = 6;

/// How many levels of hashes to ask for at once above the account subtrees:
/// at most 2^7 = 128 hashes per query. OCaml mainnet nodes ask for 6 but
/// serve up to 8 (`sync_ledger_max_subtree_depth`). With mainnet's 290,000
/// or so accounts, the content root is 13 levels above the account
/// subtrees: 7 + 6 takes two rounds of hash queries, where 6 + 6 + 1 took
/// three, the last of them returning only 2 hashes each.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml#L55-L57>
pub const SYNC_LEDGER_QUERY_DEPTH: usize = 7;

/// How many levels of child hashes to ask for at an address this deep, and
/// the most an answer may hold: [`SYNC_LEDGER_QUERY_DEPTH`], but not below
/// the account subtrees. `None` at and below them: they are fetched whole.
///
/// The request and the answer check must both use this. An answer deeper
/// than the check allows is rejected, and its address is then never asked
/// again, so the sync stalls.
pub fn child_hashes_query_depth(address_depth: usize) -> Option<usize> {
    let above_accounts = (LEDGER_DEPTH - ACCOUNT_SUBTREE_HEIGHT).checked_sub(address_depth)?;
    Some(SYNC_LEDGER_QUERY_DEPTH.min(above_accounts)).filter(|depth| *depth > 0)
}

pub type TransitionFrontierSyncLedgerSnarkedActionWithMeta =
    redux::ActionWithMeta<TransitionFrontierSyncLedgerSnarkedAction>;
pub type TransitionFrontierSyncLedgerSnarkedActionWithMetaRef<'a> =
    redux::ActionWithMeta<&'a TransitionFrontierSyncLedgerSnarkedAction>;

#[derive(Serialize, Deserialize, Debug, Clone, ActionEvent)]
#[action_event(level = trace)]
pub enum TransitionFrontierSyncLedgerSnarkedAction {
    Pending,
    PeersQuery,

    // For NumAccounts query
    PeerQueryNumAccountsInit {
        peer_id: PeerId,
    },
    PeerQueryNumAccountsPending {
        peer_id: PeerId,
        rpc_id: P2pRpcId,
    },
    PeerQueryNumAccountsRetry {
        peer_id: PeerId,
    },
    PeerQueryNumAccountsError {
        peer_id: PeerId,
        rpc_id: P2pRpcId,
        error: PeerLedgerQueryError,
    },
    PeerQueryNumAccountsSuccess {
        peer_id: PeerId,
        rpc_id: P2pRpcId,
        response: PeerLedgerQueryResponse,
    },
    NumAccountsReceived {
        num_accounts: u64,
        contents_hash: LedgerHash,
        sender: PeerId,
    },
    NumAccountsAccepted {
        num_accounts: u64,
        contents_hash: LedgerHash,
        sender: PeerId,
    },
    NumAccountsRejected {
        num_accounts: u64,
        sender: PeerId,
    },
    NumAccountsSuccess {
        num_accounts: u64,
        contents_hash: LedgerHash,
    },

    MerkleTreeSyncPending,

    // For child hashes and content queries
    PeerQueryAddressInit {
        address: LedgerAddress,
        expected_hash: LedgerHash,
        peer_id: PeerId,
    },
    PeerQueryAddressPending {
        address: LedgerAddress,
        peer_id: PeerId,
        rpc_id: P2pRpcId,
    },
    PeerQueryAddressRetry {
        address: LedgerAddress,
        peer_id: PeerId,
    },
    PeerQueryAddressError {
        peer_id: PeerId,
        rpc_id: P2pRpcId,
        error: PeerLedgerQueryError,
    },
    PeerQueryAddressSuccess {
        peer_id: PeerId,
        rpc_id: P2pRpcId,
        response: PeerLedgerQueryResponse,
    },
    /// `hashes` are the descendants of `address` some levels below it, left
    /// to right.
    ChildHashesReceived {
        address: LedgerAddress,
        hashes: Vec<LedgerHash>,
        sender: PeerId,
    },
    ChildHashesAccepted {
        address: LedgerAddress,
        hashes: Vec<LedgerHash>,
        previous_hashes: Vec<LedgerHash>,
        sender: PeerId,
    },
    ChildHashesRejected {
        address: LedgerAddress,
        hashes: Vec<LedgerHash>,
        sender: PeerId,
    },
    ChildAccountsReceived {
        address: LedgerAddress,
        accounts: Vec<MinaBaseAccountBinableArgStableV2>,
        sender: PeerId,
    },
    ChildAccountsAccepted {
        address: LedgerAddress,
        count: u64,
        sender: PeerId,
    },
    ChildAccountsRejected {
        address: LedgerAddress,
        sender: PeerId,
    },

    #[action_event(level = debug)]
    MerkleTreeSyncSuccess,
    #[action_event(level = debug)]
    Success,
}

impl redux::EnablingCondition<crate::State> for TransitionFrontierSyncLedgerSnarkedAction {
    fn is_enabled(&self, state: &crate::State, time: redux::Timestamp) -> bool {
        match self {
            TransitionFrontierSyncLedgerSnarkedAction::Pending => state
                .transition_frontier
                .sync
                .ledger()
                .is_some_and(|s| matches!(s, TransitionFrontierSyncLedgerState::Init { .. })),
            TransitionFrontierSyncLedgerSnarkedAction::PeersQuery => {
                // This condition passes if:
                // - there are available peers to query
                // - there is a snarked ledger to sync
                // - there are either queued num_accounts or address queries
                //   or queries to retry
                let peers_available = state
                    .p2p
                    .ready_peers_iter()
                    .any(|(_, p)| p.channels.rpc.can_send_request());
                let sync_next_available = state
                    .transition_frontier
                    .sync
                    .ledger()
                    .and_then(|s| s.snarked())
                    .is_some_and(|s| {
                        s.is_num_accounts_query_next() || s.contains_pending_address_queries()
                    });
                peers_available && sync_next_available
            }

            // num accounts
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsInit { peer_id } => None
                .or_else(|| {
                    let target_best_tip = state.transition_frontier.sync.best_tip()?;
                    let ledger = state.transition_frontier.sync.ledger()?.snarked()?;
                    let target = ledger.target();

                    let check_num_accounts = matches!(
                        ledger,
                        TransitionFrontierSyncLedgerSnarkedState::NumAccountsPending { .. }
                    );

                    let peer = state.p2p.get_ready_peer(peer_id)?;
                    let peer_available = check_peer_available(
                        peer,
                        target,
                        target_best_tip,
                        state.transition_frontier.sync.root_block(),
                    );

                    let not_attempted = ledger
                        .num_accounts_pending()
                        .is_some_and(|s| !s.attempts.contains_key(peer_id));

                    Some(check_num_accounts && peer_available && not_attempted)
                })
                .unwrap_or(false),
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsPending {
                peer_id,
                ..
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked()?.num_accounts_pending())
                .is_some_and(|pending| {
                    pending
                        .attempts
                        .get(peer_id)
                        .map(|peer_rpc_state| matches!(peer_rpc_state, PeerRpcState::Init { .. }))
                        .unwrap_or(false)
                }),
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsRetry { peer_id } => {
                None.or_else(|| {
                    let target_best_tip = state.transition_frontier.sync.best_tip()?;
                    let ledger = state.transition_frontier.sync.ledger()?.snarked()?;
                    let target = ledger.target();

                    let check_num_accounts = matches!(
                        ledger,
                        TransitionFrontierSyncLedgerSnarkedState::NumAccountsPending { .. }
                    );

                    let peer = state.p2p.get_ready_peer(peer_id)?;
                    let peer_available = check_peer_available(
                        peer,
                        target,
                        target_best_tip,
                        state.transition_frontier.sync.root_block(),
                    );

                    let unavailable_answer = ledger
                        .num_accounts_pending()
                        .and_then(|s| s.attempts.get(peer_id))
                        .is_some_and(|attempt| {
                            matches!(
                                attempt,
                                PeerRpcState::Error {
                                    error: PeerLedgerQueryError::LedgerUnavailable
                                        | PeerLedgerQueryError::DataUnavailable,
                                    ..
                                }
                            )
                        });
                    let other_untried = ledger.num_accounts_pending().is_some_and(|pending| {
                        state.p2p.ready_peers_iter().any(|(other_id, other)| {
                            other_id != peer_id
                                && other.channels.rpc.can_send_request()
                                && check_peer_available(
                                    other,
                                    target,
                                    target_best_tip,
                                    state.transition_frontier.sync.root_block(),
                                )
                                && !pending.attempts.contains_key(other_id)
                        })
                    });
                    // After a full pass, retry a connected peer: a snapshot
                    // may become available as its root advances.
                    let retry_allowed = (!unavailable_answer || !other_untried)
                        && ledger
                            .num_accounts_pending()
                            .and_then(|pending| pending.attempts.get(peer_id))
                            .is_some_and(|attempt| attempt.retry_ready(time));

                    Some(check_num_accounts && peer_available && retry_allowed)
                })
                .unwrap_or(false)
            }
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsError {
                peer_id,
                rpc_id,
                ..
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked())
                .is_some_and(|s| {
                    s.peer_num_account_query_get(peer_id, *rpc_id)
                        .and_then(|s| s.attempts.get(peer_id))
                        .is_some_and(|s| matches!(s, PeerRpcState::Pending { .. }))
                }),
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsSuccess {
                peer_id,
                rpc_id,
                ..
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked())
                .is_some_and(|s| {
                    // TODO(tizoc): check if expected response kind is correct.
                    s.peer_num_account_query_get(peer_id, *rpc_id)
                        .and_then(|s| s.attempts.get(peer_id))
                        .is_some_and(|s| matches!(s, PeerRpcState::Pending { .. }))
                }),
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsReceived { sender, .. }
            | TransitionFrontierSyncLedgerSnarkedAction::NumAccountsAccepted { sender, .. }
            | TransitionFrontierSyncLedgerSnarkedAction::NumAccountsRejected { sender, .. } => {
                state
                    .transition_frontier
                    .sync
                    .ledger()
                    .and_then(|s| s.snarked()?.num_accounts_pending())
                    .and_then(|s| s.attempts.get(sender))
                    .is_some_and(|s| s.is_success())
            }
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsSuccess { .. } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked()?.num_accounts_pending())
                .is_some(),

            TransitionFrontierSyncLedgerSnarkedAction::MerkleTreeSyncPending => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked())
                .is_some_and(|s| {
                    matches!(
                        s,
                        TransitionFrontierSyncLedgerSnarkedState::NumAccountsSuccess { .. }
                    )
                }),
            TransitionFrontierSyncLedgerSnarkedAction::MerkleTreeSyncSuccess => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked())
                .is_some_and(|s| match s {
                    TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
                        queue,
                        pending_addresses: pending,
                        ..
                    } => queue.is_empty() && pending.is_empty(),
                    _ => false,
                }),

            // hashes and contents
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressInit {
                address,
                peer_id,
                expected_hash: _,
            } => {
                None.or_else(|| {
                    let target_best_tip = state.transition_frontier.sync.best_tip()?;
                    let ledger = state.transition_frontier.sync.ledger()?.snarked()?;
                    let target = ledger.target();

                    // This is true if the address being requested is in the queue
                    let check_next_addr = match ledger {
                        TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncPending {
                            queue,
                            ..
                        } => queue.contains_key(address),
                        _ => false,
                    };

                    let peer = state.p2p.get_ready_peer(peer_id)?;
                    let check_peer_available = check_peer_available(
                        peer,
                        target,
                        target_best_tip,
                        state.transition_frontier.sync.root_block(),
                    );

                    Some(
                        check_next_addr
                            && check_peer_available
                            && !ledger.peer_unavailable(peer_id)
                            && ledger.peer_query_retry_ready(peer_id, time),
                    )
                })
                .unwrap_or(false)
            }
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressRetry {
                address,
                peer_id,
            } => {
                None.or_else(|| {
                    let target_best_tip = state.transition_frontier.sync.best_tip()?;
                    let ledger = state.transition_frontier.sync.ledger()?.snarked()?;
                    let target = ledger.target();

                    // This is true if there there is a retry address that
                    // matches the one requested in this action.
                    let check_next_addr = state
                        .transition_frontier
                        .sync
                        .ledger()
                        .and_then(|s| {
                            let found = s
                                .snarked()?
                                .sync_address_retry_iter(time)
                                .any(|addr| &addr == address);
                            Some(found)
                        })
                        .unwrap_or(false);

                    let peer = state.p2p.get_ready_peer(peer_id)?;
                    let check_peer_available = check_peer_available(
                        peer,
                        target,
                        target_best_tip,
                        state.transition_frontier.sync.root_block(),
                    );
                    Some(
                        check_next_addr
                            && check_peer_available
                            && !ledger.peer_unavailable(peer_id)
                            && ledger.peer_query_retry_ready(peer_id, time),
                    )
                })
                .unwrap_or(false)
            }
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressPending {
                peer_id, ..
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked()?.fetch_pending())
                .is_some_and(|pending| {
                    pending
                        .iter()
                        .filter_map(|(_, query_state)| query_state.attempts.get(peer_id))
                        .any(|peer_rpc_state| matches!(peer_rpc_state, PeerRpcState::Init { .. }))
                }),
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError {
                peer_id,
                rpc_id,
                ..
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked())
                .is_some_and(|s| {
                    s.peer_address_query_get(peer_id, *rpc_id)
                        .and_then(|(_, s)| s.attempts.get(peer_id))
                        .is_some_and(|s| matches!(s, PeerRpcState::Pending { .. }))
                }),
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressSuccess {
                peer_id,
                rpc_id,
                ..
            } => {
                state
                    .transition_frontier
                    .sync
                    .ledger()
                    .and_then(|s| s.snarked())
                    .is_some_and(|s| {
                        // TODO(binier): check if expected response kind is correct.
                        s.peer_address_query_get(peer_id, *rpc_id)
                            .and_then(|(_, s)| s.attempts.get(peer_id))
                            .is_some_and(|s| matches!(s, PeerRpcState::Pending { .. }))
                    })
            }
            TransitionFrontierSyncLedgerSnarkedAction::ChildHashesReceived {
                address,
                sender,
                ..
            }
            | TransitionFrontierSyncLedgerSnarkedAction::ChildHashesAccepted {
                address,
                sender,
                ..
            }
            | TransitionFrontierSyncLedgerSnarkedAction::ChildHashesRejected {
                address,
                sender,
                ..
            } => {
                address.length() < LEDGER_DEPTH - ACCOUNT_SUBTREE_HEIGHT
                    && state
                        .transition_frontier
                        .sync
                        .ledger()
                        .and_then(|s| s.snarked()?.fetch_pending()?.get(address))
                        .and_then(|s| s.attempts.get(sender))
                        .is_some_and(|s| s.is_success())
            }
            TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsReceived {
                address,
                sender,
                ..
            }
            | TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsRejected {
                address,
                sender,
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked()?.fetch_pending()?.get(address))
                .and_then(|s| s.attempts.get(sender))
                .is_some_and(|s| s.is_success()),
            // An empty range is accepted when its hash is the expected one;
            // refusing it left the attempt `Success` with nothing to follow.
            TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsAccepted {
                address,
                count: _,
                sender,
            } => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked()?.fetch_pending()?.get(address))
                .and_then(|s| s.attempts.get(sender))
                .is_some_and(|s| s.is_success()),
            TransitionFrontierSyncLedgerSnarkedAction::Success => state
                .transition_frontier
                .sync
                .ledger()
                .and_then(|s| s.snarked())
                .is_some_and(|s| {
                    matches!(
                        s,
                        TransitionFrontierSyncLedgerSnarkedState::MerkleTreeSyncSuccess { .. }
                    )
                }),
        }
    }
}

pub(super) fn check_peer_available(
    peer: &crate::p2p::P2pPeerStatusReady,
    target: &crate::transition_frontier::sync::ledger::SyncLedgerTarget,
    target_best_tip: &mina_core::block::BlockWithHash<
        std::sync::Arc<mina_p2p_messages::v2::MinaBlockBlockStableV2>,
    >,
    root_block: Option<&mina_core::block::ArcBlockWithHash>,
) -> bool {
    None.or_else(|| {
        let peer_best_tip = peer.best_tip.as_ref()?;
        let available = if !peer.channels.rpc.can_send_request() {
            false
        } else if target.staged.is_some() {
            // if peer has same best tip, then he has same root
            // so we can sync root snarked+staged ledger from that peer.
            target_best_tip.hash() == peer_best_tip.hash()
        } else {
            &target.snarked_ledger_hash == peer_best_tip.snarked_ledger_hash()
                || &target.snarked_ledger_hash == peer_best_tip.staking_epoch_ledger_hash()
                || &target.snarked_ledger_hash == peer_best_tip.next_epoch_ledger_hash()
                || unadvertised_staking_ledger(peer, target, root_block)
        };

        Some(available)
    })
    .unwrap_or(false)
}

/// Before the root enters the new epoch, OCaml keeps the old staking
/// snapshot and answers ledger queries for its hash (sync_handler.ml,
/// get_ledger_by_hash). The peer's newer best tip need not advertise it.
pub(super) fn unadvertised_staking_ledger(
    peer: &crate::p2p::P2pPeerStatusReady,
    target: &crate::transition_frontier::sync::ledger::SyncLedgerTarget,
    root_block: Option<&mina_core::block::ArcBlockWithHash>,
) -> bool {
    matches!(
        target.kind,
        crate::transition_frontier::sync::ledger::SyncLedgerTargetKind::StakingEpoch
    ) && root_block
        .is_some_and(|root| root.staking_epoch_ledger_hash() == &target.snarked_ledger_hash)
        && peer.best_tip.as_ref().is_some_and(|tip| {
            tip.staking_epoch_ledger_hash() != &target.snarked_ledger_hash
                && tip.next_epoch_ledger_hash() != &target.snarked_ledger_hash
                && tip.snarked_ledger_hash() != &target.snarked_ledger_hash
        })
}

use crate::transition_frontier::{
    sync::{ledger::TransitionFrontierSyncLedgerAction, TransitionFrontierSyncAction},
    TransitionFrontierAction,
};

impl From<TransitionFrontierSyncLedgerSnarkedAction> for crate::Action {
    fn from(value: TransitionFrontierSyncLedgerSnarkedAction) -> Self {
        Self::TransitionFrontier(TransitionFrontierAction::Sync(
            TransitionFrontierSyncAction::Ledger(TransitionFrontierSyncLedgerAction::Snarked(
                value,
            )),
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::ledger::tree_height_for_num_accounts;

    use super::*;

    /// Queries to sync a ledger of `num_accounts` from empty, where every
    /// subtree holding an account differs and is asked for: hash queries down
    /// to the account subtrees, then one query per account subtree.
    fn full_ledger_queries(num_accounts: u64) -> u64 {
        let mut depth = LEDGER_DEPTH - tree_height_for_num_accounts(num_accounts);
        let mut queries = 0;
        loop {
            let subtree_accounts = 1u64 << (LEDGER_DEPTH - depth);
            queries += num_accounts.div_ceil(subtree_accounts);
            match child_hashes_query_depth(depth) {
                Some(levels) => depth += levels,
                None => return queries,
            }
        }
    }

    #[test]
    fn child_hashes_query_depth_stops_at_the_account_subtrees() {
        let accounts_depth = LEDGER_DEPTH - ACCOUNT_SUBTREE_HEIGHT;
        assert_eq!(child_hashes_query_depth(0), Some(SYNC_LEDGER_QUERY_DEPTH));
        assert_eq!(child_hashes_query_depth(accounts_depth - 7), Some(7));
        assert_eq!(child_hashes_query_depth(accounts_depth - 1), Some(1));
        assert_eq!(child_hashes_query_depth(accounts_depth), None);
        assert_eq!(child_hashes_query_depth(LEDGER_DEPTH), None);
    }

    /// Mainnet's ledgers hold about 290,450 accounts, so the sync starts 13
    /// levels above the account subtrees. Asking for 6 levels at a time took
    /// 16 -> 22 -> 28, then a 2-hash query at depth 28 for every pair of
    /// account subtrees: 6,846 queries in R4's runs. 7 levels take 16 -> 23 ->
    /// 29, and about 4,600 queries.
    #[test]
    fn a_mainnet_size_ledger_takes_two_rounds_of_hash_queries() {
        let num_accounts = 290_450;
        let content_root_depth = LEDGER_DEPTH - tree_height_for_num_accounts(num_accounts);
        assert_eq!(content_root_depth, 16);
        assert_eq!(child_hashes_query_depth(16), Some(7));
        assert_eq!(child_hashes_query_depth(23), Some(6));
        assert_eq!(child_hashes_query_depth(29), None);
        // 1 + 71 hash queries, 4,539 account subtrees.
        assert_eq!(full_ledger_queries(num_accounts), 4_611);
    }

    use ledger::{
        scan_state::currency::{Amount, Balance, Nonce, Slot, SlotSpan, TxnVersion},
        Account, Timing, TokenSymbol, VerificationKeyWire, ZkAppAccount, ZkAppUri,
    };
    use mina_p2p_messages::{binprot::BinProtWrite, v2};

    use crate::p2p::P2pLimits;

    /// The largest account in bin_prot: every number at its largest, a token
    /// symbol of the 6 characters OCaml allows, a delegate, a vesting
    /// schedule, and a zkApp with a verification key and a URI of the 255
    /// bytes OCaml allows. Field elements are 32 bytes whatever their value.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/account.ml#L92>
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_base/zkapp_account.ml#L165>
    fn largest_account() -> Account {
        let mut account = Account::empty();
        account.delegate = Some(account.public_key.clone());
        account.token_symbol = TokenSymbol(b"ABCDEF".to_vec());
        account.balance = Balance::max();
        account.nonce = Nonce::max();
        account.timing = Timing::Timed {
            initial_minimum_balance: Balance::max(),
            cliff_time: Slot::max(),
            cliff_amount: Amount::max(),
            vesting_period: SlotSpan::max(),
            vesting_increment: Amount::max(),
        };
        account.permissions.set_verification_key.txn_version = TxnVersion::max();
        account.zkapp = Some(Box::new(ZkAppAccount {
            verification_key: Some(VerificationKeyWire::dummy()),
            zkapp_version: u32::MAX,
            last_action_slot: Slot::max(),
            proved_state: true,
            zkapp_uri: ZkAppUri::from(vec![b'x'; 255]),
            ..ZkAppAccount::default()
        }));
        account
    }

    /// A peer answers an account query with the accounts of a subtree of
    /// height [`ACCOUNT_SUBTREE_HEIGHT`]: the answer must fit the RPC size
    /// limit even if they are all as large as accounts can be. With the
    /// former 200 KB limit, a devnet node never synced the 5 subtrees of the
    /// devnet staking ledger over 200 KB (P1c).
    #[test]
    fn largest_account_query_answer_fits_the_rpc_limit() {
        let account = v2::MinaBaseAccountBinableArgStableV2::from(&largest_account());
        let mut account_bytes = Vec::new();
        account.binprot_write(&mut account_bytes).unwrap();

        let answer = v2::MinaLedgerSyncLedgerAnswerStableV2::ContentsAre(
            std::iter::repeat_n(account, 1 << ACCOUNT_SUBTREE_HEIGHT).collect(),
        );
        let mut answer_bytes = Vec::new();
        answer.binprot_write(&mut answer_bytes).unwrap();
        // The RPC response around it, its header, query id, result tag and
        // length, takes a few bytes more.
        let message_len = answer_bytes.len() + 64;

        let limit = P2pLimits::default().rpc_answer_sync_ledger_query();
        println!(
            "{} accounts of {} bytes: {message_len} bytes, limit {limit:?}",
            1 << ACCOUNT_SUBTREE_HEIGHT,
            account_bytes.len()
        );
        assert!(
            message_len > 200_000,
            "{} accounts of {} bytes: {message_len} bytes",
            1 << ACCOUNT_SUBTREE_HEIGHT,
            account_bytes.len()
        );
        assert!(
            limit >= message_len,
            "{} accounts of {} bytes: {message_len} bytes, over the limit {limit:?}",
            1 << ACCOUNT_SUBTREE_HEIGHT,
            account_bytes.len()
        );
    }
}
