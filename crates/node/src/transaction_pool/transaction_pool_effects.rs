use std::collections::BTreeMap;

use crate::{ledger::LedgerService, snark::SnarkStore};

use super::TransactionPoolEffectfulAction;

impl TransactionPoolEffectfulAction {
    /// `synced`: whether the node is synced to a block after genesis. Before
    /// that, as at every start, the best tip's ledger may not be loaded yet,
    /// which is no error.
    pub fn effects<Store, S>(self, store: &mut Store, synced: bool)
    where
        Store: SnarkStore<S>,
        Store::Service: LedgerService,
    {
        match self {
            TransactionPoolEffectfulAction::FetchAccounts {
                account_ids,
                ledger_hash,
                on_result,
                pending_id,
                from_source,
            } => {
                mina_core::log::debug!(
                    mina_core::log::system_time();
                    kind = "TransactionPoolEffectfulFetchAccounts",
                    summary = "fetching accounts for tx pool");
                // FIXME: the ledger ctx `get_accounts` function doesn't ensure that every account we
                // asked for is included in the result.
                // TODO: should be asynchronous. Once asynchronous, watch out for race
                // conditions between tx pool and transition frontier. By the time the
                // accounts have been fetched the best tip may have changed already.
                let accounts = match store
                    .service()
                    .ledger_manager()
                    .get_accounts(&ledger_hash, account_ids.iter().cloned().collect())
                {
                    Ok(accounts) => Ok(accounts
                        .into_iter()
                        .map(|account| (account.id(), account))
                        .collect::<BTreeMap<_, _>>()),
                    Err(err) => {
                        if is_ledger_still_loading(synced, &err) {
                            // At every start, before the ledger is loaded
                            // (RC7-FIXES): `mina report` titled healthy
                            // nodes' reports with it at ERROR.
                            mina_core::log::debug!(
                                mina_core::log::system_time();
                                kind = "TransactionPoolLedgerLoading",
                                summary = "tx pool accounts not fetched: the ledger isn't loaded yet",
                                ledger = ledger_hash.to_string());
                        } else {
                            mina_core::log::error!(
                                mina_core::log::system_time();
                                kind = "Error",
                                summary = "failed to fetch accounts for tx pool",
                                error = format!("ledger {:?}, error: {:?}", ledger_hash, err));
                        }
                        // Still call back, so that the pending action is
                        // answered and removed.
                        Err(format!("failed to fetch accounts: {err:?}"))
                    }
                };

                store.dispatch_callback(on_result, (accounts, pending_id, from_source));
            }
        }
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
