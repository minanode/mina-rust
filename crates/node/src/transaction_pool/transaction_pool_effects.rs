use std::collections::BTreeMap;

use super::TransactionPoolEffectfulAction;
use crate::{ledger::LedgerService, snark::SnarkStore};

impl TransactionPoolEffectfulAction {
    pub fn effects<Store, S>(self, store: &mut Store, _synced: bool)
    where
        Store: SnarkStore<S>,
        Store::Service: LedgerService,
    {
        match self {
            Self::FetchAccounts {
                account_ids,
                ledger_hash,
                pending_id,
                generation,
                pinned,
                on_result,
                from_source,
            } => {
                if let Some(id) = pending_id {
                    store.service().ledger_manager().transaction_pool_accounts(
                        id,
                        ledger_hash,
                        account_ids.into_iter().collect(),
                        generation,
                        pinned,
                    );
                } else {
                    store.dispatch_callback(
                        on_result,
                        (Err("missing account read id".to_owned()), None, from_source),
                    );
                }
            }
            Self::PinLedger {
                pending_id,
                ledger_hash,
            } => {
                store
                    .service()
                    .ledger_manager()
                    .pin_transaction_pool_ledger(pending_id, ledger_hash);
            }
            Self::CancelReads { generation } => {
                store
                    .service()
                    .ledger_manager()
                    .cancel_transaction_pool_reads(generation);
            }
        }
    }
}

pub(crate) fn report_account_read_error(
    state: &crate::State,
    id: u32,
    result: &Result<BTreeMap<ledger::AccountId, ledger::Account>, String>,
) {
    // Cancelled responses release admission slots without reporting a failure.
    let Some(read) = state.transaction_pool.pending_actions.get(&id) else {
        return;
    };
    let Err(err) = result else { return };
    let frontier = &state.transition_frontier;
    let synced =
        frontier.sync.is_synced() && frontier.best_tip().is_some_and(|tip| !tip.is_genesis());
    if is_ledger_still_loading(synced, err) {
        mina_core::log::debug!(mina_core::log::system_time(); kind = "TransactionPoolLedgerLoading",
            summary = "tx pool accounts not fetched: the ledger isn't loaded yet", ledger = read.ledger_hash.to_string());
    } else {
        mina_core::log::error!(mina_core::log::system_time(); kind = "Error",
            summary = "failed to fetch accounts for tx pool", error = format!("ledger {:?}, error: {:?}", read.ledger_hash, err));
    }
}

/// The ledger manager's answer for a ledger it doesn't hold.
const LEDGER_UNAVAILABLE: &str = "Ledger unavailable";

/// Whether a failed account fetch only means the node hasn't loaded the
/// ledger yet: the ledger is missing while the node isn't synced.
fn is_ledger_still_loading(synced: bool, error: &str) -> bool {
    !synced && error == LEDGER_UNAVAILABLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc7_a_missing_ledger_before_sync_is_not_an_error() {
        assert!(is_ledger_still_loading(false, "Ledger unavailable"));
        // Once synced, a missing best tip ledger is a real failure.
        assert!(!is_ledger_still_loading(true, "Ledger unavailable"));
        assert!(!is_ledger_still_loading(false, "account lookup failed"));
    }
}
