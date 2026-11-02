use redux::ActionMeta;

use crate::{
    ledger::{
        hash_subtree_at_depth, ledger_empty_hash_at_depth, tree_height_for_num_accounts,
        LedgerAddress, LEDGER_DEPTH,
    },
    Store,
};

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
            TransitionFrontierSyncLedgerSnarkedAction::NumAccountsAccepted {
                num_accounts,
                contents_hash,
                ..
            } => {
                let Some(hash) = store
                    .state()
                    .transition_frontier
                    .sync
                    .ledger()
                    .and_then(|s| s.snarked())
                    .map(|s| s.ledger_hash().clone())
                else {
                    return;
                };
                // NumAccounts authenticated the content root and all the empty
                // right siblings above it. We skip querying those levels, so
                // clear their old contents before starting at the content root.
                // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/syncable_ledger/syncable_ledger.ml#L795-L842>
                let mut address = LedgerAddress::first(
                    LEDGER_DEPTH - tree_height_for_num_accounts(*num_accounts),
                );
                while let Some(parent) = address.parent() {
                    let right = parent.child_right();
                    let empty = ledger_empty_hash_at_depth(right.length());
                    if store
                        .service
                        .accounts_set(hash.clone(), &right, &empty, vec![])
                        .as_ref()
                        != Ok(&empty)
                    {
                        mina_core::error!(mina_core::log::system_time(); "Could not clear verified empty ledger suffix");
                        return;
                    }
                    address = parent;
                }
                store.dispatch(
                    TransitionFrontierSyncLedgerSnarkedAction::NumAccountsSuccess {
                        num_accounts: *num_accounts,
                        contents_hash: contents_hash.clone(),
                    },
                );
            }
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
                let Ok(mut previous_hashes) =
                    store
                        .service()
                        .child_hashes_get(snarked_ledger_hash.clone(), address, levels)
                else {
                    mina_core::error!(mina_core::log::system_time(); "Could not read sync ledger child hashes");
                    return;
                };

                // OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/syncable_ledger/syncable_ledger.ml#L795-L813>
                // Empty contents are not a valid RPC request. The verified
                // hash is all we need to remove this subtree locally.
                let depth = address.length() + levels;
                let empty = ledger_empty_hash_at_depth(depth);
                for ((child, expected), previous) in address
                    .iter_children(depth)
                    .zip(hashes)
                    .zip(&mut previous_hashes)
                {
                    if expected == &empty && previous != &empty {
                        if store
                            .service
                            .accounts_set(snarked_ledger_hash.clone(), &child, &empty, vec![])
                            .as_ref()
                            != Ok(&empty)
                        {
                            mina_core::error!(mina_core::log::system_time(); "Could not clear verified empty ledger subtree");
                            return;
                        }
                        *previous = empty.clone();
                    }
                }

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
