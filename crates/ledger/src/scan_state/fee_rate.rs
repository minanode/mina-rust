use crate::scan_state::currency::Magnitude;

use super::currency::Fee;
use serde::{Deserialize, Serialize};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Clone, Hash, Serialize, Deserialize)]
pub struct FeeRate {
    q: fraction::Fraction,
}

impl std::fmt::Display for FeeRate {
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/graphql/mina_graphql/types.ml#L878-L890>
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.q)
    }
}

impl FeeRate {
    pub fn make_exn(fee: Fee, weight: u64) -> Self {
        if weight == 0 {
            assert!(fee.is_zero());
        }

        Self {
            q: fraction::Fraction::new(fee.as_u64(), weight),
        }
    }
}

#[cfg(test)]
mod status_404_tests {
    use super::*;
    #[test]
    fn ocaml_q_string_is_an_exact_reduced_fraction() {
        // Q.to_string in OCaml: integral values omit /1; gcd reduces ratios.
        for (fee, weight, expected) in [
            (0, 1, "0"),
            (1_000_000, 1, "1000000"),
            (10, 4, "5/2"),
            (1_000_000_001, 3, "1000000001/3"),
        ] {
            assert_eq!(
                FeeRate::make_exn(Fee::from_u64(fee), weight).to_string(),
                expected
            );
        }
    }
}
