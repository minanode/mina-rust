//! Snark work ids, as OCaml shows them in GraphQL: the `ppx_hash` hash of a
//! transaction snark statement (`Transaction_snark.Statement.hash`), a 30-bit
//! integer.
//!
//! `[@@deriving hash]` folds a value into Base's internal hash state field by
//! field: records in declaration order, a variant's constructor index (when
//! it has more than one constructor) before its arguments, a list's length
//! before its elements. Base's state is OCaml's `Hashtbl` MurmurHash3 mixing,
//! started at 0. Field elements fold Zarith's `Z.hash` of their integer value;
//! amounts and other unsigned numbers fold as an `int64`.
//!
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_snark_work/transaction_snark_work.ml#L46-L47>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_state/snarked_ledger_state.ml#L103-L137>
//! OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/kimchi_pasta_snarky_backend/field.ml#L221>

use mina_p2p_messages::{
    bigint::BigInt,
    binprot::BinProtWrite,
    v2::{
        CurrencyFeeStableV1, MinaBaseFeeExcessStableV1,
        MinaBasePendingCoinbaseStackVersionedStableV1,
        MinaBaseTransactionStatusFailureCollectionStableV1,
        MinaBaseTransactionStatusFailureStableV2,
        MinaStateBlockchainStateValueStableV2LedgerProofStatement,
        MinaStateBlockchainStateValueStableV2LedgerProofStatementSource,
        MinaStateBlockchainStateValueStableV2SignedAmount,
        MinaStateSnarkedLedgerStateWithSokStableV2,
        MinaTransactionLogicZkappCommandLogicLocalStateValueStableV1, SgnStableV1, SignedAmount,
    },
};

/// Base's `Internalhash` state, a 32-bit MurmurHash3 accumulator.
///
/// OCaml: <https://github.com/janestreet/base/blob/v0.14.3/hash_types/src/internalhash_stubs.c>
#[derive(Clone, Copy, Debug, Default)]
struct HashState(u32);

impl HashState {
    /// `MIX` from OCaml's `runtime/hash.c`.
    fn mix(&mut self, d: u32) {
        let d = d
            .wrapping_mul(0xcc9e_2d51)
            .rotate_left(15)
            .wrapping_mul(0x1b87_3593);
        self.0 = (self.0 ^ d)
            .rotate_left(13)
            .wrapping_mul(5)
            .wrapping_add(0xe654_6b64);
    }

    /// `Hash.fold_int`: OCaml's `caml_hash_mix_intnat`, which mixes a 64-bit
    /// int's two halves into one word.
    fn fold_int(&mut self, i: i64) {
        self.mix(((i >> 32) ^ (i >> 63) ^ i) as u32);
    }

    /// `Hash.fold_int64`: the low word, then the high word.
    fn fold_int64(&mut self, i: u64) {
        self.mix(i as u32);
        self.mix((i >> 32) as u32);
    }

    fn fold_bool(&mut self, b: bool) {
        self.fold_int(b.into());
    }

    /// A field element: `Bignum_bigint.hash_fold_t` of its integer value,
    /// that is `fold_int` of Zarith's `Z.hash`, which mixes each 64-bit limb
    /// up to the highest non-zero one, low word first, into a state of 0.
    ///
    /// OCaml: <https://github.com/ocaml/Zarith/blob/release-1.11/caml_z.c#L3187-L3207>
    fn fold_field(&mut self, value: &BigInt) {
        let limbs = value.as_ref().0;
        let used = limbs
            .iter()
            .rposition(|limb| *limb != 0)
            .map_or(0, |i| i + 1);
        let mut z = HashState::default();
        for limb in &limbs[..used] {
            z.fold_int64(*limb);
        }
        self.fold_int(z.0.into());
    }

    /// `Hash.get_hash_value`: OCaml's final mix, kept to 30 bits.
    fn value(self) -> i32 {
        let mut h = self.0;
        h ^= h >> 16;
        h = h.wrapping_mul(0x85eb_ca6b);
        h ^= h >> 13;
        h = h.wrapping_mul(0xc2b2_ae35);
        h ^= h >> 16;
        (h & 0x3fff_ffff) as i32
    }

    /// `Sgn.t = Pos | Neg`.
    fn fold_sgn(&mut self, sgn: &SgnStableV1) {
        self.fold_int(match sgn {
            SgnStableV1::Pos => 0,
            SgnStableV1::Neg => 1,
        });
    }

    /// `(Amount.t, Sgn.t) Signed_poly.t = { magnitude; sgn }`.
    fn fold_signed_amount(&mut self, amount: &MinaStateBlockchainStateValueStableV2SignedAmount) {
        self.fold_int64(amount.magnitude.0 .0.as_u64());
        self.fold_sgn(&amount.sgn);
    }

    fn fold_signed_fee(&mut self, fee: &SignedAmount) {
        self.fold_fee(&fee.magnitude);
        self.fold_sgn(&fee.sgn);
    }

    fn fold_fee(&mut self, fee: &CurrencyFeeStableV1) {
        self.fold_int64(fee.0 .0.as_u64());
    }

    /// `Fee_excess.t = { fee_token_l; fee_excess_l; fee_token_r; fee_excess_r }`.
    fn fold_fee_excess(&mut self, fee_excess: &MinaBaseFeeExcessStableV1) {
        for side in [&fee_excess.0, &fee_excess.1] {
            self.fold_field(&side.token);
            self.fold_signed_fee(&side.amount);
        }
    }

    /// `Pending_coinbase.Stack_versioned.t = { data; state = { init; curr } }`.
    fn fold_pending_coinbase(&mut self, stack: &MinaBasePendingCoinbaseStackVersionedStableV1) {
        self.fold_field(&stack.data);
        self.fold_field(&stack.state.init);
        self.fold_field(&stack.state.curr);
    }

    /// `Transaction_status.Failure.Collection.t`, a `Failure.t list list`.
    fn fold_failures(&mut self, table: &MinaBaseTransactionStatusFailureCollectionStableV1) {
        self.fold_int(table.0.len() as i64);
        for failures in table.0.iter() {
            self.fold_int(failures.len() as i64);
            for failure in failures.iter() {
                self.fold_int(failure_index(failure).into());
                if let MinaBaseTransactionStatusFailureStableV2::AccountAppStatePreconditionUnsatisfied(i) = failure {
                    self.fold_int(i.as_u64() as i64);
                }
            }
        }
    }

    /// `Zkapp_command_logic.Local_state.t`, in declaration order.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/transaction_logic/zkapp_command_logic.ml#L217-L229>
    fn fold_local_state(
        &mut self,
        state: &MinaTransactionLogicZkappCommandLogicLocalStateValueStableV1,
    ) {
        self.fold_field(&state.stack_frame.0);
        self.fold_field(&state.call_stack.0);
        self.fold_field(&state.transaction_commitment);
        self.fold_field(&state.full_transaction_commitment);
        self.fold_signed_fee(&state.excess);
        self.fold_signed_fee(&state.supply_increase);
        self.fold_field(&state.ledger);
        self.fold_bool(state.success);
        self.fold_int64(state.account_update_index.0.as_u32().into());
        self.fold_failures(&state.failure_status_tbl);
        self.fold_bool(state.will_succeed);
    }

    /// `Registers.t = { first_pass_ledger; second_pass_ledger;
    /// pending_coinbase_stack; local_state }`.
    ///
    /// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/mina_state/registers.ml#L8-L18>
    fn fold_registers(
        &mut self,
        registers: &MinaStateBlockchainStateValueStableV2LedgerProofStatementSource,
    ) {
        self.fold_field(&registers.first_pass_ledger);
        self.fold_field(&registers.second_pass_ledger);
        self.fold_pending_coinbase(&registers.pending_coinbase_stack);
        self.fold_local_state(&registers.local_state);
    }
}

/// The constructor's position in OCaml's `Transaction_status.Failure.t`,
/// which `ppx_hash` folds. bin_prot writes the same position as a variant's
/// first byte, and the generated type keeps OCaml's order.
fn failure_index(failure: &MinaBaseTransactionStatusFailureStableV2) -> u8 {
    let mut bytes = Vec::with_capacity(9);
    failure
        .binprot_write(&mut bytes)
        .expect("writing to a Vec doesn't fail");
    bytes[0]
}

/// The statement's parts that `Statement.hash` covers; `sok_digest` is
/// `unit` in `Statement.t`, which folds nothing.
struct StatementParts<'a> {
    source: &'a MinaStateBlockchainStateValueStableV2LedgerProofStatementSource,
    target: &'a MinaStateBlockchainStateValueStableV2LedgerProofStatementSource,
    connecting_ledger_left: &'a BigInt,
    connecting_ledger_right: &'a BigInt,
    supply_increase: &'a MinaStateBlockchainStateValueStableV2SignedAmount,
    fee_excess: &'a MinaBaseFeeExcessStableV1,
}

impl StatementParts<'_> {
    fn hash(&self) -> i32 {
        let mut state = HashState::default();
        state.fold_registers(self.source);
        state.fold_registers(self.target);
        state.fold_field(self.connecting_ledger_left);
        state.fold_field(self.connecting_ledger_right);
        state.fold_signed_amount(self.supply_increase);
        state.fold_fee_excess(self.fee_excess);
        state.value()
    }
}

/// The work id of a ledger proof's statement, without its sok digest.
pub(crate) fn work_id_with_sok(statement: &MinaStateSnarkedLedgerStateWithSokStableV2) -> i32 {
    StatementParts {
        source: &statement.source,
        target: &statement.target,
        connecting_ledger_left: &statement.connecting_ledger_left,
        connecting_ledger_right: &statement.connecting_ledger_right,
        supply_increase: &statement.supply_increase,
        fee_excess: &statement.fee_excess,
    }
    .hash()
}

/// The work id of a statement, as a pending snark work shows it.
pub(crate) fn work_id(
    statement: &MinaStateBlockchainStateValueStableV2LedgerProofStatement,
) -> i32 {
    StatementParts {
        source: &statement.source,
        target: &statement.target,
        connecting_ledger_left: &statement.connecting_ledger_left,
        connecting_ledger_right: &statement.connecting_ledger_right,
        supply_increase: &statement.supply_increase,
        fee_excess: &statement.fee_excess,
    }
    .hash()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OCaml's work ids for the statements of public devnet blocks, from an
    /// OCaml 4.0.2 node's GraphQL (`snarkJobs { workIds }`); see the
    /// fixture's `source`.
    #[test]
    fn work_ids_match_ocaml_devnet_node() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/files/graphql/ocaml/work-ids-devnet.json");
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let mut checked = 0;
        for block in fixture["blocks"].as_array().unwrap() {
            for work in block["works"].as_array().unwrap() {
                let expected: Vec<i32> = serde_json::from_value(work["workIds"].clone()).unwrap();
                let ours: Vec<i32> = work["statements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|statement| {
                        let statement: MinaStateBlockchainStateValueStableV2LedgerProofStatement =
                            serde_json::from_value(statement.clone()).unwrap();
                        work_id(&statement)
                    })
                    .collect();
                assert_eq!(
                    ours, expected,
                    "block {} work {}",
                    block["height"], work["index"]
                );
                checked += ours.len();
            }
        }
        assert_eq!(checked, 13);
    }

    #[test]
    fn failure_index_is_the_ocaml_constructor_position() {
        use MinaBaseTransactionStatusFailureStableV2 as F;
        assert_eq!(failure_index(&F::Predicate), 0);
        assert_eq!(failure_index(&F::Overflow), 9);
        assert_eq!(
            failure_index(&F::AccountAppStatePreconditionUnsatisfied(3u64.into())),
            36
        );
        assert_eq!(failure_index(&F::Cancelled), 44);
    }
}
