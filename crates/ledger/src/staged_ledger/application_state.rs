use std::collections::HashSet;

use crate::AccountId;

// Both public network profiles use 12 (devnet includes Mainnet).
// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_config/profiled/mainnet.ml#L53>
const ZKAPP_PRODUCTION_LIMIT: usize = 12;

/// Keep capped commands and their nonce dependants in the pool, without applying
/// them to the validating ledger or reporting them as invalid.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/staged_ledger/application_state.ml#L15-L78>
pub(super) struct CommandSelection {
    remaining: u64,
    zkapps_remaining: usize,
    skipped_fee_payers: HashSet<AccountId>,
}

impl CommandSelection {
    pub(super) fn new(remaining: u64) -> Self {
        Self {
            remaining,
            zkapps_remaining: ZKAPP_PRODUCTION_LIMIT,
            skipped_fee_payers: HashSet::new(),
        }
    }

    pub(super) fn is_full(&self) -> bool {
        self.remaining == 0
    }

    pub(super) fn should_apply(&mut self, fee_payer: AccountId, is_zkapp: bool) -> bool {
        if self.is_full() {
            return false;
        }
        if is_zkapp && self.zkapps_remaining == 0 {
            self.skipped_fee_payers.insert(fee_payer);
            return false;
        }
        !self.skipped_fee_payers.contains(&fee_payer)
    }

    /// Only an accepted first pass consumes space, including commands whose
    /// later execution has a Failed status. First-pass errors consume none.
    pub(super) fn applied(&mut self, is_zkapp: bool) {
        self.remaining -= 1;
        if is_zkapp {
            self.zkapps_remaining -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenId;
    use mina_signer::CompressedPubKey;

    fn payer(token: u64) -> AccountId {
        AccountId::new(
            CompressedPubKey::from_address(
                "B62qkiqPXFDayJV8JutYvjerERZ35EKrdmdcXh3j1rDUHRs1bJkFFcX",
            )
            .unwrap(),
            TokenId::from(token),
        )
    }

    #[test]
    fn mesa_limit_preserves_nonce_dependencies_and_unrelated_payments() {
        // Expected decisions follow OCaml application_state.ml:39-78;
        // the limit 12 is from the public-network profile, not Rust output.
        let mut state = CommandSelection::new(128);
        for _ in 0..12 {
            assert!(state.should_apply(payer(1), true));
            state.applied(true);
        }
        assert!(!state.should_apply(payer(1), true));
        assert!(!state.should_apply(payer(1), false));
        assert!(state.should_apply(payer(2), false));
        state.applied(false);
        assert!(!state.should_apply(payer(2), true));
        assert!(!state.should_apply(payer(2), false));
    }

    #[test]
    fn failed_first_pass_does_not_spend_capacity() {
        let mut state = CommandSelection::new(1);
        assert!(state.should_apply(payer(1), true));
        // A first-pass error does not call applied, as in OCaml's Error arm.
        assert!(state.should_apply(payer(1), true));
        state.applied(true);
        assert!(state.is_full());
        assert!(!state.should_apply(payer(2), false));
        assert!(!CommandSelection::new(0).should_apply(payer(1), true));
    }
}
