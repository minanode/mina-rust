use redux::ActionMeta;

use crate::{ledger::hash_subtree_at_depth, Store};

use super::{
    child_hashes_query_depth, TransitionFrontierSyncLedgerSnarkedAction,
    TransitionFrontierSyncLedgerSnarkedService,
};

impl TransitionFrontierSyncLedgerSnarkedAction {
    pub fn effects<S>(&self, _: &ActionMeta, store: &mut Store<S>)
    where
        S: redux::Service + TransitionFrontierSyncLedgerSnarkedService,
    {
        match self {
            TransitionFrontierSyncLedgerSnarkedAction::Pending => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeersQuery => {}

            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsInit { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsPending { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsRetry { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsError { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryNumAccountsSuccess { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsReceived { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsAccepted { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsRejected { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsSuccess { .. } => {}

            TransitionFrontierSyncLedgerSnarkedAction::MerkleTreeSyncPending => {}
            TransitionFrontierSyncLedgerSnarkedAction::MerkleTreeSyncSuccess => {}

            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressInit { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressRetry { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressError { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressSuccess { .. } => {}

            TransitionFrontierSyncLedgerSnarkedAction::ChildHashesReceived {
                address,
                hashes,
                sender,
                ..
            } => {
                // TODO(refactor): service call must be split
                let Some((snarked_ledger_hash, parent_hash)) = None.or_else(|| {
                    let snarked_ledger =
                        store.state().transition_frontier.sync.ledger()?.snarked()?;
                    let parent_hash = snarked_ledger
                        .fetch_pending()?
                        .get(address)?
                        .expected_hash
                        .clone();
                    Some((snarked_ledger.ledger_hash().clone(), parent_hash))
                }) else {
                    return;
                };

                let reject = |store: &mut Store<S>| {
                    store.dispatch(
                        TransitionFrontierSyncLedgerSnarkedAction::ChildHashesRejected {
                            address: address.clone(),
                            hashes: hashes.clone(),
                            sender: *sender,
                        },
                    );
                };

                // At most as many levels as asked for, and not below the
                // account subtrees.
                let levels = hashes.len().trailing_zeros() as usize;
                let max_levels = child_hashes_query_depth(address.length()).unwrap_or(0);
                if !hashes.len().is_power_of_two() || levels == 0 || levels > max_levels {
                    reject(store);
                    return;
                }

                let (Ok(hashes_fp), Ok(parent_hash_fp)) = (
                    hashes
                        .iter()
                        .map(|hash| hash.to_field())
                        .collect::<Result<Vec<_>, _>>(),
                    parent_hash.to_field(),
                ) else {
                    // Reject in case of invalid fields
                    reject(store);
                    return;
                };

                if hash_subtree_at_depth(address.length(), hashes_fp) != Some(parent_hash_fp) {
                    reject(store);
                    return;
                }

                // TODO: for async ledger this needs an intermediary action
                let previous_hashes = store
                    .service()
                    .child_hashes_get(snarked_ledger_hash, address, levels)
                    .unwrap();

                store.dispatch(
                    TransitionFrontierSyncLedgerSnarkedAction::ChildHashesAccepted {
                        address: address.clone(),
                        hashes: hashes.clone(),
                        previous_hashes,
                        sender: *sender,
                    },
                );
            }
            TransitionFrontierSyncLedgerSnarkedAction::ChildHashesAccepted { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::ChildHashesRejected { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsReceived {
                address,
                accounts,
                sender,
            } => {
                // TODO(refactor): service call must be split
                let Some((snarked_ledger_hash, parent_hash)) = None.or_else(|| {
                    let snarked_ledger =
                        store.state().transition_frontier.sync.ledger()?.snarked()?;
                    Some((
                        snarked_ledger.ledger_hash().clone(),
                        snarked_ledger
                            .fetch_pending()?
                            .get(address)?
                            .expected_hash
                            .clone(),
                    ))
                }) else {
                    return;
                };

                // After setting the accounts, we get the new computed hash.
                // It must be equal to the parent node hash, otherwise we got
                // bad data from the peer. Accounts the ledger can't take are
                // bad data too.
                let computed_hash = store.service.accounts_set(
                    snarked_ledger_hash.clone(),
                    address,
                    &parent_hash,
                    accounts.clone(),
                );

                if computed_hash.as_ref() != Ok(&parent_hash) {
                    store.dispatch(
                        TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsRejected {
                            address: address.clone(),
                            sender: *sender,
                        },
                    );
                    return;
                }

                // Setting accounts doesn't immediately compute the hashes in the merkle tree,
                // so we force that here before continuing.
                let compute_hashes_result = store
                    .service
                    .compute_snarked_ledger_hashes(&snarked_ledger_hash);

                if let Err(_error) = compute_hashes_result {
                    // TODO(tizoc): log this error
                }

                store.dispatch(
                    TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsAccepted {
                        address: address.clone(),
                        count: accounts.len() as u64,
                        sender: *sender,
                    },
                );
            }
            TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsAccepted { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::ChildAccountsRejected { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::PeerQueryAddressPending { .. } => {}
            TransitionFrontierSyncLedgerSnarkedAction::Success => {}
        }
    }
}
