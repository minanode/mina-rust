use redux::ActionMeta;

use crate::Store;

use super::{
    snarked::TransitionFrontierSyncLedgerSnarkedAction,
    staged::TransitionFrontierSyncLedgerStagedAction, TransitionFrontierSyncLedgerAction,
};

// TODO(refactor): all this should be in the reducers?

pub fn transition_frontier_sync_ledger_init_effects<S: redux::Service>(
    _: &ActionMeta,
    store: &mut Store<S>,
) {
    store.dispatch(TransitionFrontierSyncLedgerSnarkedAction::Pending);
}

pub fn try_complete_snarked_ledger_locally<
    S: redux::Service + super::snarked::TransitionFrontierSyncLedgerSnarkedService,
>(
    store: &mut Store<S>,
    prepared_copy: bool,
) -> bool {
    let target = store
        .state()
        .transition_frontier
        .sync
        .ledger()
        .filter(|ledger| {
            matches!(
                ledger,
                super::TransitionFrontierSyncLedgerState::Init { .. }
            ) || ledger.snarked().is_some_and(|snarked| snarked.is_pending())
        })
        .map(|ledger| ledger.target().snarked_ledger_hash);
    let Some(hash) = target.filter(|hash| store.service().contains_snarked_ledger(hash)) else {
        return false;
    };
    // A previous sync attempt may have left an incomplete working mask under
    // this hash. Commit moves the working mask over the committed one, so
    // replace it unless the preparation just copied the committed target.
    if !prepared_copy {
        store
            .service()
            .copy_snarked_ledger_contents_for_sync(vec![], hash, true)
            .expect("copying a committed ledger for sync must succeed");
    }
    store.dispatch(TransitionFrontierSyncLedgerAction::LocalSnarkedSuccess)
}

pub fn transition_frontier_sync_ledger_snarked_success_effects<S: redux::Service>(
    _: &ActionMeta,
    store: &mut Store<S>,
) {
    if store.dispatch(TransitionFrontierSyncLedgerAction::Success) {
    } else if store.dispatch(TransitionFrontierSyncLedgerStagedAction::ReconstructEmpty) {
    } else if store.dispatch(TransitionFrontierSyncLedgerStagedAction::PartsFetchPending) {
    }
}

pub fn transition_frontier_sync_ledger_staged_success_effects<S: redux::Service>(
    _: &ActionMeta,
    store: &mut Store<S>,
) {
    store.dispatch(TransitionFrontierSyncLedgerAction::Success);
}
