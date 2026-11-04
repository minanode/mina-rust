use mina_node::{transaction_pool::MAX_ACCOUNT_READS, ActionKind, ActionWithMeta, Store};
use strum::VariantArray;

use crate::{Invariant, InvariantResult};

/// Makes sure we don't have cycles in action chain.
///
/// Cycles in action chain can cause whole lot of issues:
/// 1. Stack overflow (as long as actions live on stack).
/// 2. Infinite loop.
/// 3. Harder to reason about and debug state machine.
///
/// A few actions dispatch themselves again on purpose, and their progress
/// bounds how often. A cycle through one of them is allowed, as long as the
/// action appears in the chain no more often than its limit
/// ([`self_requeue_limit`]).
#[derive(documented::Documented, Default, Clone, Copy)]
pub struct NoRecursion;

impl Invariant for NoRecursion {
    type InternalState = Vec<ActionKind>;
    fn triggers(&self) -> &[ActionKind] {
        ActionKind::VARIANTS
    }

    fn check<S: redux::Service>(
        self,
        action_stack: &mut Self::InternalState,
        _: &Store<S>,
        action: &ActionWithMeta,
    ) -> InvariantResult {
        check_action(
            action_stack,
            action.action().kind(),
            action.depth() as usize,
        )
    }
}

/// Checks the action of `action_kind` at `action_depth`, where 1 is a root
/// action, against the chain in `action_stack` and pushes it there.
fn check_action(
    action_stack: &mut Vec<ActionKind>,
    action_kind: ActionKind,
    action_depth: usize,
) -> InvariantResult {
    if action_stack.len() < action_depth {
        assert_eq!(action_stack.len() + 1, action_depth);
    } else {
        action_stack.truncate(action_depth.saturating_sub(1));
    }

    // let is_recursing = action_stack.iter().any(|kind| *kind == action_kind);
    let is_recursing = contains_cycle(action_stack);
    action_stack.push(action_kind);

    if is_recursing {
        return InvariantResult::Violation(format!("recursion detected: {action_stack:?}"));
    }
    if let Some(limit) = self_requeue_limit(action_kind) {
        let count = action_stack.iter().filter(|k| **k == action_kind).count();
        if count > limit {
            return InvariantResult::Violation(format!(
                "{action_kind:?} nested {count} times, limit {limit}: {action_stack:?}"
            ));
        }
    }
    InvariantResult::Updated
}

/// The most times an action that dispatches itself again on purpose may appear
/// in one action chain, or `None` for every other action.
fn self_requeue_limit(kind: ActionKind) -> Option<usize> {
    match kind {
        // Each call handles one step of the account read queue and dispatches
        // itself again only after a step that made progress: a ready read
        // handed to its completion, or a due retry. A ready read holds one of
        // `MAX_ACCOUNT_READS` read slots. Only the reconciliation at the head
        // of the queue retries, since `BestTipChanged` puts every other read
        // behind the reconciliations, and its retry leaves the head queued.
        // So a chain is either a retry or at most `MAX_ACCOUNT_READS` ready
        // steps, and then the call that stops.
        ActionKind::TransactionPoolProcessAccountReads => Some(MAX_ACCOUNT_READS + 1),
        // A Kademlia request refused at once (the backoff, the peer target,
        // the seed policy) ends inside the batch that made it, and its error
        // asks for the next batch. The lookup starts a batch only while fewer
        // than 3 requests are in flight and each refusal ends one of them, so
        // a chain holds the first batch and at most 3 more.
        ActionKind::P2pNetworkKadBootstrapCreateRequests => Some(4),
        // The WebRTC service wraps a plain event with its queue permit, and
        // the event source dispatches the unwrapped event once more.
        ActionKind::EventSourceNewEvent => Some(2),
        _ => None,
    }
}

fn contains_cycle(stack: &[ActionKind]) -> bool {
    let len = stack.len();
    if len < 2 {
        return false; // No cycle possible if the stack is too short
    }

    for cycle_length in 1..=(len / 2) {
        let slice1 = &stack[len - cycle_length..];
        let slice2 = &stack[len - 2 * cycle_length..len - cycle_length];
        // A cycle through a self-requeuing action is bounded by its limit.
        if slice1 == slice2 && !slice1.iter().any(|k| self_requeue_limit(*k).is_some()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const READS: ActionKind = ActionKind::TransactionPoolProcessAccountReads;
    const KAD_REQUESTS: ActionKind = ActionKind::P2pNetworkKadBootstrapCreateRequests;
    const NEW_EVENT: ActionKind = ActionKind::EventSourceNewEvent;
    const TIMEOUTS: ActionKind = ActionKind::CheckTimeouts;
    const FETCH: ActionKind = ActionKind::TransactionPoolEffectfulFetchAccounts;

    const BOUNDED: [(ActionKind, usize); 3] = [
        (READS, MAX_ACCOUNT_READS + 1),
        (KAD_REQUESTS, 4),
        (NEW_EVENT, 2),
    ];

    /// Dispatches `chain` as one chain under a root `CheckTimeouts`, each
    /// action the child of the one before, and returns the first violation.
    fn nest(stack: &mut Vec<ActionKind>, chain: &[ActionKind]) -> Option<String> {
        std::iter::once(TIMEOUTS)
            .chain(chain.iter().copied())
            .enumerate()
            .find_map(|(i, kind)| match check_action(stack, kind, i + 1) {
                InvariantResult::Violation(violation) => Some(violation),
                _ => None,
            })
    }

    #[test]
    fn repeated_action_is_a_cycle() {
        assert!(contains_cycle(&[TIMEOUTS, FETCH, FETCH]));
        assert!(contains_cycle(&[READS, TIMEOUTS, FETCH, TIMEOUTS, FETCH]));
        assert!(!contains_cycle(&[TIMEOUTS, FETCH]));
    }

    #[test]
    fn self_requeue_is_not_a_cycle() {
        assert!(!contains_cycle(&[TIMEOUTS, READS, READS]));
        assert!(!contains_cycle(&[READS, FETCH, READS, FETCH]));
        assert_eq!(self_requeue_limit(FETCH), None);
    }

    #[test]
    fn each_bounded_action_nests_up_to_its_limit() {
        for (kind, limit) in BOUNDED {
            assert_eq!(self_requeue_limit(kind), Some(limit));

            // The child after the last call checks the stack for a cycle.
            let mut chain = vec![kind; limit];
            chain.push(FETCH);
            assert_eq!(nest(&mut Vec::new(), &chain), None, "{kind:?}");

            let violation = nest(&mut Vec::new(), &vec![kind; limit + 1])
                .unwrap_or_else(|| panic!("{kind:?} nested {} times", limit + 1));
            assert!(
                violation.contains(&format!("nested {} times", limit + 1)),
                "{violation}"
            );
        }
    }

    #[test]
    fn a_cycle_through_a_bounded_action_counts_toward_its_limit() {
        let limit = MAX_ACCOUNT_READS + 1;
        let cycle = |calls| [READS, FETCH].repeat(calls);
        assert_eq!(nest(&mut Vec::new(), &cycle(limit)), None);

        let violation = nest(&mut Vec::new(), &cycle(limit + 1)).unwrap();
        assert!(violation.contains("limit"), "{violation}");
    }

    #[test]
    fn other_repeated_actions_still_fail() {
        let violation = nest(&mut Vec::new(), &[FETCH, FETCH, TIMEOUTS]).unwrap();
        assert!(violation.starts_with("recursion detected"), "{violation}");
    }

    #[test]
    fn siblings_and_new_roots_drop_the_rest_of_the_chain() {
        let limit = MAX_ACCOUNT_READS + 1;
        let mut stack = Vec::new();
        assert_eq!(nest(&mut stack, &vec![READS; limit]), None);

        // A sibling of the deepest call replaces it, so the count stays at
        // the limit.
        assert!(matches!(
            check_action(&mut stack, READS, limit + 1),
            InvariantResult::Updated
        ));
        // A sibling higher up drops everything below it.
        assert!(matches!(
            check_action(&mut stack, READS, 2),
            InvariantResult::Updated
        ));
        assert_eq!(stack, [TIMEOUTS, READS]);

        // The next root starts a new chain, which may nest to the limit
        // again.
        assert_eq!(nest(&mut stack, &vec![READS; limit]), None);
        assert_eq!(stack.len(), limit + 1);
    }
}
