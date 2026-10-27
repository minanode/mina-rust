use std::{rc::Rc, sync::Arc};

use anyhow::Context;
use consensus::ConsensusState;
use mina_core::constants::{constraint_constants, ForkConstants};
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::{bigint::InvalidBigInt, v2};
use poseidon::hash::{
    params::{MINA_PROTO_STATE, MINA_PROTO_STATE_BODY},
    Inputs,
};

use crate::{
    dummy,
    proofs::{
        block::consensus::CheckedConsensusState,
        constants::{
            make_step_block_data, make_step_transaction_data, StepBlockProof, WrapBlockProof,
        },
        field::{Boolean, CircuitVar},
        numbers::{
            currency::CheckedSigned,
            nat::{CheckedNat, CheckedSlot},
        },
        step::extract_recursion_challenges,
        to_field_elements::ToFieldElementsDebug,
        util::sha256_sum,
        wrap::{wrap, WrapParams},
    },
    scan_state::{
        currency,
        fee_excess::{self, FeeExcess},
        pending_coinbase::{PendingCoinbase, PendingCoinbaseWitness, Stack},
        protocol_state::MinaHash,
        scan_state::transaction_snark::{
            validate_ledgers_at_merge_checked, Registers, SokDigest, Statement, StatementLedgers,
        },
        transaction_logic::{
            local_state::LocalStateSignedAmountsVar,
            protocol_state::{EpochLedger, ProtocolStateView},
        },
    },
    staged_ledger::hash::StagedLedgerHash,
    zkapps::interfaces::{SignedAmountBranchParam, SignedAmountInterface},
    ToInputs,
};

use super::{
    field::field,
    numbers::{
        currency::CheckedAmount,
        nat::{CheckedBlockTime, CheckedBlockTimeSpan, CheckedLength},
    },
    step::{step, InductiveRule, OptFlag, PreviousProofStatement, StepParams, StepProof},
    to_field_elements::ToFieldElements,
    transaction::{transaction_snark::checked_hash, Check, Prover},
    witness::Witness,
    wrap::WrapProof,
};

#[derive(Debug, Clone)]
pub struct BlockchainState {
    pub staged_ledger_hash: StagedLedgerHash<Fp>,
    pub genesis_ledger_hash: Fp,
    pub ledger_proof_statement: Statement<()>,
    pub timestamp: currency::BlockTime,
    pub body_reference: v2::ConsensusBodyReferenceStableV1,
}

#[derive(Debug, Clone)]
pub struct SnarkTransition {
    pub blockchain_state: BlockchainState,
    pub consensus_transition: currency::Slot,
    pub pending_coinbase_update: crate::scan_state::pending_coinbase::update::Update,
}

impl TryFrom<&v2::MinaStateBlockchainStateValueStableV2> for BlockchainState {
    type Error = InvalidBigInt;

    fn try_from(value: &v2::MinaStateBlockchainStateValueStableV2) -> Result<Self, Self::Error> {
        let v2::MinaStateBlockchainStateValueStableV2 {
            staged_ledger_hash,
            genesis_ledger_hash,
            ledger_proof_statement,
            timestamp,
            body_reference,
        } = value;

        Ok(Self {
            staged_ledger_hash: staged_ledger_hash.try_into()?,
            genesis_ledger_hash: genesis_ledger_hash.to_field()?,
            ledger_proof_statement: ledger_proof_statement.try_into()?,
            timestamp: (*timestamp).into(),
            body_reference: body_reference.clone(),
        })
    }
}

impl TryFrom<&v2::MinaStateSnarkTransitionValueStableV2> for SnarkTransition {
    type Error = InvalidBigInt;

    fn try_from(value: &v2::MinaStateSnarkTransitionValueStableV2) -> Result<Self, Self::Error> {
        let v2::MinaStateSnarkTransitionValueStableV2 {
            blockchain_state,
            consensus_transition,
            pending_coinbase_update,
        } = value;

        Ok(Self {
            blockchain_state: blockchain_state.try_into()?,
            consensus_transition: consensus_transition.into(),
            pending_coinbase_update: pending_coinbase_update.into(),
        })
    }
}

impl ToFieldElements<Fp> for SnarkTransition {
    fn to_field_elements(&self, fields: &mut Vec<Fp>) {
        let Self {
            blockchain_state,
            consensus_transition,
            pending_coinbase_update:
                crate::scan_state::pending_coinbase::update::Update {
                    action,
                    coinbase_amount,
                },
        } = self;

        blockchain_state.to_field_elements(fields);
        fields.push(Fp::from(consensus_transition.as_u32()));

        // <https://github.com/MinaProtocol/mina/blob/f6fb903bef974b191776f393a2f9a1e6360750fe/src/lib/mina_base/pending_coinbase.ml#L420>
        use crate::scan_state::pending_coinbase::update::Action::*;
        let bits = match action {
            None => [Boolean::False, Boolean::False],
            One => [Boolean::True, Boolean::False],
            TwoCoinbaseInFirst => [Boolean::False, Boolean::True],
            TwoCoinbaseInSecond => [Boolean::True, Boolean::True],
        };
        fields.extend(bits.into_iter().map(Boolean::to_field::<Fp>));
        fields.push(Fp::from(coinbase_amount.as_u64()));
    }
}

impl Check<Fp> for SnarkTransition {
    fn check(&self, w: &mut Witness<Fp>) {
        let Self {
            blockchain_state,
            consensus_transition,
            pending_coinbase_update:
                crate::scan_state::pending_coinbase::update::Update {
                    action: _,
                    coinbase_amount,
                },
        } = self;

        blockchain_state.check(w);
        consensus_transition.check(w);
        coinbase_amount.check(w);
    }
}

impl ToFieldElements<Fp> for EpochLedger<Fp> {
    fn to_field_elements(&self, fields: &mut Vec<Fp>) {
        let Self {
            hash,
            total_currency,
        } = self;
        hash.to_field_elements(fields);
        total_currency.to_field_elements(fields);
    }
}

impl ToFieldElements<Fp> for ProtocolState {
    fn to_field_elements(&self, fields: &mut Vec<Fp>) {
        let Self {
            previous_state_hash,
            body,
        } = self;

        previous_state_hash.to_field_elements(fields);
        body.to_field_elements(fields);
    }
}

impl Check<Fp> for ProtocolState {
    fn check(&self, w: &mut Witness<Fp>) {
        let Self {
            previous_state_hash: _,
            body,
        } = self;

        body.check(w);
    }
}

fn ledger_proof_opt(
    proof: Option<&v2::LedgerProofProdStableV2>,
    next_state: &v2::MinaStateProtocolStateValueStableV2,
) -> anyhow::Result<(Statement<SokDigest>, Arc<v2::TransactionSnarkProofStableV2>)> {
    match proof {
        Some(proof) => {
            let statement: Statement<SokDigest> = (&proof.0.statement).try_into()?;
            Ok((statement, proof.0.proof.get()?))
        }
        None => {
            let statement: Statement<()> =
                (&next_state.body.blockchain_state.ledger_proof_statement).try_into()?;
            let statement = statement.with_digest(SokDigest::default());
            let p = dummy::dummy_transaction_proof();
            Ok((statement, p))
        }
    }
}

fn checked_hash_protocol_state(
    state: &ProtocolState,
    w: &mut Witness<Fp>,
) -> anyhow::Result<(Fp, Fp)> {
    let ProtocolState {
        previous_state_hash,
        body,
    } = state;

    let mut inputs = Inputs::new();
    body.to_inputs(&mut inputs);
    let body_hash = checked_hash(&MINA_PROTO_STATE_BODY, &inputs.to_fields(), w);

    let mut inputs = Inputs::new();
    inputs.append_field(*previous_state_hash);
    inputs.append_field(body_hash);
    let hash = checked_hash(&MINA_PROTO_STATE, &inputs.to_fields(), w);

    Ok((hash, body_hash))
}

// Checked version
fn checked_hash_protocol_state2(
    state: &ProtocolState<CheckedConsensusState>,
    w: &mut Witness<Fp>,
) -> (Fp, Fp) {
    let ProtocolState {
        previous_state_hash,
        body,
    } = state;

    let mut inputs = Inputs::new();
    body.to_inputs(&mut inputs);
    let body_hash = checked_hash(&MINA_PROTO_STATE_BODY, &inputs.to_fields(), w);

    let mut inputs = Inputs::new();
    inputs.append_field(*previous_state_hash);
    inputs.append_field(body_hash);
    let hash = checked_hash(&MINA_PROTO_STATE, &inputs.to_fields(), w);

    (hash, body_hash)
}

/// A ledger statement in the block step circuit, with its five signed amounts
/// as variables: the `excess` and `supply_increase` of its source and target
/// local states, and its `supply_increase`.
///
/// OCaml's signed amount variables cache their field value, so comparing a
/// statement a second time pushes no value for its signed amounts
/// ([`LocalStateSignedAmountsVar`]). Since Mesa the block step compares the
/// previous ledger statement twice, so it makes one `LedgerStatementVar` of it
/// and passes it to both comparisons; a second conversion would push its five
/// values again (R15: 67 values for the second comparison instead of 62).
struct LedgerStatementVar<'a> {
    statement: &'a Statement<()>,
    source: LocalStateSignedAmountsVar,
    target: LocalStateSignedAmountsVar,
    supply_increase: CheckedSigned<Fp, CheckedAmount<Fp>>,
}

impl<'a> LedgerStatementVar<'a> {
    fn new(statement: &'a Statement<()>) -> Self {
        Self {
            statement,
            source: statement.source.local_state.signed_amounts_var(),
            target: statement.target.local_state.signed_amounts_var(),
            supply_increase: statement.supply_increase.to_checked(),
        }
    }
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/blockchain_snark/blockchain_snark_state.ml#L70-L83>
fn non_pc_registers_equal_var(
    (t1, a1): (&Registers, &LocalStateSignedAmountsVar),
    (t2, a2): (&Registers, &LocalStateSignedAmountsVar),
    w: &mut Witness<Fp>,
) -> Boolean {
    let alls = [
        // t1.pending_coinbase_stack.equal_var(&t2.pending_coinbase_stack, w),
        field::equal(t1.first_pass_ledger, t2.first_pass_ledger, w),
        field::equal(t1.second_pass_ledger, t2.second_pass_ledger, w),
    ]
    .into_iter()
    .chain(
        t1.local_state
            .checked_equal_prime(a1, &t2.local_state, a2, w),
    )
    .collect::<Vec<_>>();

    Boolean::all(&alls, w)
}

/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/blockchain_snark/blockchain_snark_state.ml#L85-L119>
fn txn_statement_ledger_hashes_equal(
    s1: &LedgerStatementVar,
    s2: &LedgerStatementVar,
    w: &mut Witness<Fp>,
) -> Boolean {
    let (t1, t2) = (s1.statement, s2.statement);
    let source_eq =
        non_pc_registers_equal_var((&t1.source, &s1.source), (&t2.source, &s2.source), w);
    let target_eq =
        non_pc_registers_equal_var((&t1.target, &s1.target), (&t2.target, &s2.target), w);
    let left_ledger_eq = field::equal(t1.connecting_ledger_left, t2.connecting_ledger_left, w);
    let right_ledger_eq = field::equal(t1.connecting_ledger_right, t2.connecting_ledger_right, w);
    let supply_increase_eq = s1.supply_increase.equal(&s2.supply_increase, w);

    Boolean::all(
        &[
            source_eq,
            target_eq,
            left_ledger_eq,
            right_ledger_eq,
            supply_increase_eq,
        ],
        w,
    )
}

mod floating_point {
    use std::cmp::Ordering;

    use ark_ff::BigInteger256;
    use num_bigint::BigUint;

    use crate::{
        proofs::{
            field::FieldWitness, to_field_elements::field_of_bits, transaction::field_to_bits2,
        },
        scan_state::currency::{Amount, Balance, Sgn},
    };

    use super::*;

    pub enum CoeffIntegerPart {
        Zero,
        One,
    }

    const COEFFICIENTS: [(Sgn, BigInteger256); 11] = [
        (Sgn::Pos, BigInteger256::new([405058, 0, 0, 0])),
        (Sgn::Neg, BigInteger256::new([1007582, 0, 0, 0])),
        (Sgn::Pos, BigInteger256::new([465602, 0, 0, 0])),
        (Sgn::Neg, BigInteger256::new([161365, 0, 0, 0])),
        (Sgn::Pos, BigInteger256::new([44739, 0, 0, 0])),
        (Sgn::Neg, BigInteger256::new([10337, 0, 0, 0])),
        (Sgn::Pos, BigInteger256::new([2047, 0, 0, 0])),
        (Sgn::Neg, BigInteger256::new([354, 0, 0, 0])),
        (Sgn::Pos, BigInteger256::new([54, 0, 0, 0])),
        (Sgn::Neg, BigInteger256::new([7, 0, 0, 0])),
        (Sgn::Pos, BigInteger256::new([0, 0, 0, 0])),
    ];

    pub struct Params {
        pub total_precision: usize,
        pub per_term_precision: usize,
        pub terms_needed: usize,
        pub coefficients: [(Sgn, BigInteger256); 11],
        pub linear_term_integer_part: CoeffIntegerPart,
    }

    pub const PARAMS: Params = Params {
        total_precision: 16,
        per_term_precision: 20,
        terms_needed: 11,
        coefficients: COEFFICIENTS,
        linear_term_integer_part: CoeffIntegerPart::One,
    };

    #[derive(Clone)]
    pub enum Interval {
        Constant(BigUint),
        LessThan(BigUint),
    }

    fn bits_needed(v: BigUint) -> u64 {
        v.bits().checked_sub(1).unwrap()
    }

    impl Interval {
        fn scale(&self, x: BigUint) -> Self {
            match self {
                Interval::Constant(v) => Self::Constant(v * x),
                Interval::LessThan(v) => Self::LessThan(v * x),
            }
        }

        fn bits_needed(&self) -> u64 {
            match self {
                Interval::Constant(x) => bits_needed(x + BigUint::from(1u64)),
                Interval::LessThan(x) => bits_needed(x.clone()),
            }
        }

        fn quotient(&self, b: &Self) -> Self {
            use Interval::*;

            match (self, b) {
                (Constant(a), Constant(b)) => Constant(a / b),
                (LessThan(a), Constant(b)) => LessThan((a / b) + 1u64),
                (Constant(a), LessThan(_)) => LessThan(a + 1u64),
                (LessThan(a), LessThan(_)) => LessThan(a.clone()),
            }
        }
    }

    pub struct SnarkyInteger<F: FieldWitness> {
        pub value: F,
        pub interval: Interval,
        pub bits: Option<Box<[bool]>>,
    }

    impl<F: FieldWitness> SnarkyInteger<F> {
        pub fn create(value: F, upper_bound: BigUint) -> Self {
            Self {
                value,
                interval: Interval::LessThan(upper_bound),
                bits: None,
            }
        }

        fn to_field(&self) -> F {
            self.value
        }

        fn shift_left(&self, k: usize) -> Self {
            let Self {
                value,
                interval,
                bits,
            } = self;

            let two_to_k = BigUint::new(vec![1, 0, 0, 0]) << k;

            Self {
                value: *value * F::from(two_to_k.clone()),
                interval: interval.scale(two_to_k),
                bits: bits.as_ref().map(|_| todo!()),
            }
        }

        fn div_mod(&self, b: &Self, w: &mut Witness<F>) -> (Self, Self) {
            let (q, r) = w.exists({
                let a: BigUint = self.value.into();
                let b: BigUint = b.value.into();
                (F::from(&a / &b), F::from(&a % &b))
            });

            let q_bit_length = self.interval.bits_needed();
            let b_bit_length = b.interval.bits_needed();

            let q_bits = w.exists(field_to_bits2(q, q_bit_length as usize));
            let r_bits = w.exists(field_to_bits2(r, b_bit_length as usize));

            field::assert_lt(b_bit_length, r, b.value, w);

            (
                Self {
                    value: q,
                    interval: self.interval.quotient(&b.interval),
                    bits: Some(q_bits),
                },
                Self {
                    value: r,
                    interval: b.interval.clone(),
                    bits: Some(r_bits),
                },
            )
        }
    }

    pub fn balance_upper_bound() -> BigUint {
        BigUint::new(vec![1, 0, 0, 0]) << Balance::NBITS as u32
    }

    pub fn amount_upper_bound() -> BigUint {
        BigUint::new(vec![1, 0, 0, 0]) << Amount::NBITS as u32
    }

    #[derive(Clone, Debug)]
    pub struct Point<F: FieldWitness> {
        pub value: F,
        pub precision: usize,
    }

    pub fn of_quotient<F: FieldWitness>(
        precision: usize,
        top: SnarkyInteger<F>,
        bottom: SnarkyInteger<F>,
        w: &mut Witness<F>,
    ) -> Point<F> {
        let (q, _r) = top.shift_left(precision).div_mod(&bottom, w);
        Point {
            value: q.to_field(),
            precision,
        }
    }

    impl<F: FieldWitness> Point<F> {
        pub fn mul(&self, y: &Self, w: &mut Witness<F>) -> Self {
            let new_precision = self.precision + y.precision;
            Self {
                value: field::mul(self.value, y.value, w),
                precision: new_precision,
            }
        }

        pub fn const_mul(&self, y: &Self) -> Self {
            let new_precision = self.precision + y.precision;
            Self {
                value: self.value * y.value,
                precision: new_precision,
            }
        }

        pub fn powers(&self, n: usize, w: &mut Witness<F>) -> Vec<Self> {
            let mut res = vec![self.clone(); n];
            for i in 1..n {
                res[i] = self.mul(&res[i - 1], w);
            }
            res
        }

        pub fn constant(value: &BigInteger256, precision: usize) -> anyhow::Result<Self> {
            Ok(Self {
                value: (*value).into(),
                precision,
            })
        }

        pub fn add_signed(&self, (sgn, t2): (Sgn, &Self)) -> Self {
            let precision = self.precision.max(t2.precision);
            let (t1, t2) = if self.precision < t2.precision {
                (self, t2)
            } else {
                (t2, self)
            };

            let powed2 = (0..(t2.precision - t1.precision)).fold(F::one(), |acc, _| acc.double());

            let value = match sgn {
                Sgn::Pos => (powed2 * t1.value) + t2.value,
                Sgn::Neg => (powed2 * t1.value) - t2.value,
            };

            Self { value, precision }
        }

        pub fn add(&self, t2: &Self) -> Self {
            self.add_signed((Sgn::Pos, t2))
        }

        pub fn of_bits<const N: usize>(bits: &[bool; N], precision: usize) -> Self {
            Self {
                value: field_of_bits(bits),
                precision,
            }
        }

        pub fn le(&self, t2: &Self, w: &mut Witness<F>) -> Boolean {
            let precision = self.precision.max(t2.precision);

            let padding = {
                let k = precision - self.precision.min(t2.precision);
                (0..k).fold(F::one(), |acc, _| acc.double())
            };

            let (x1, x2) = {
                let (x1, x2) = (self.value, t2.value);

                match self.precision.cmp(&t2.precision) {
                    Ordering::Less => (padding * x1, x2),
                    Ordering::Greater => (x1, padding * x2),
                    Ordering::Equal => (x1, x1),
                }
            };

            field::compare(precision as u64, x1, x2, w).1
        }
    }
}

mod snarky_taylor {
    use crate::{proofs::field::FieldWitness, scan_state::currency::Sgn};

    use super::*;
    use floating_point::*;

    fn taylor_sum<F: FieldWitness>(
        x_powers: Vec<Point<F>>,
        coefficients: impl Iterator<Item = (Sgn, floating_point::Point<F>)>,
        linear_term_integer_part: &CoeffIntegerPart,
    ) -> Point<F> {
        let acc = coefficients
            .zip(&x_powers)
            .fold(None, |sum, ((sgn, ci), xi)| {
                let term = ci.const_mul(xi);
                match sum {
                    None => Some(term),
                    Some(s) => Some(s.add_signed((sgn, &term))),
                }
            })
            .unwrap();

        match linear_term_integer_part {
            CoeffIntegerPart::Zero => acc,
            CoeffIntegerPart::One => acc.add(&x_powers[0]),
        }
    }

    pub fn one_minus_exp<F: FieldWitness>(
        params: &Params,
        x: Point<F>,
        w: &mut Witness<F>,
    ) -> Point<F> {
        let floating_point::Params {
            total_precision: _,
            per_term_precision,
            terms_needed,
            coefficients,
            linear_term_integer_part,
        } = params;

        let powers = x.powers(*terms_needed, w);
        let coefficients = coefficients
            .iter()
            .map(|(sgn, c)| (*sgn, Point::<F>::constant(c, *per_term_precision).unwrap())); // Never fail, coeffs are hardcoded
        taylor_sum(powers, coefficients, linear_term_integer_part)
    }
}

mod vrf {
    use std::ops::Neg;

    use mina_signer::{CompressedPubKey, PubKey};
    use poseidon::hash::params::{MINA_VRF_MESSAGE, MINA_VRF_OUTPUT};

    use crate::{
        checked_verify_merkle_path,
        proofs::{
            field::GroupAffine,
            transaction::{
                decompress_var, field_to_bits, legacy_input::to_bits, scale_known,
                scale_non_constant, InnerCurve,
            },
        },
        scan_state::currency::{Amount, Balance},
        sparse_ledger::SparseLedger,
        AccountIndex, Address, AppendToInputs,
    };

    use super::*;

    struct Message<'a> {
        global_slot: &'a CheckedSlot<Fp>,
        seed: Fp,
        delegator: AccountIndex,
        delegator_bits: [bool; 35],
    }

    impl ToInputs for Message<'_> {
        fn to_inputs(&self, inputs: &mut Inputs) {
            let Self {
                global_slot,
                seed,
                delegator: _,
                delegator_bits,
            } = self;

            inputs.append(seed);
            inputs.append_u32(global_slot.to_inner().as_u32());
            for b in delegator_bits {
                inputs.append_bool(*b);
            }
        }
    }

    fn hash_to_group(m: &Message, w: &mut Witness<Fp>) -> GroupAffine<Fp> {
        let inputs = m.to_inputs_owned().to_fields();
        let hash = checked_hash(&MINA_VRF_MESSAGE, &inputs, w);
        crate::proofs::group_map::to_group(hash, w)
    }

    fn scale_generator(
        s: &[bool; 255],
        init: &InnerCurve<Fp>,
        w: &mut Witness<Fp>,
    ) -> GroupAffine<Fp> {
        scale_known(InnerCurve::<Fp>::one().to_affine(), s, init, w)
    }

    fn eval(
        m: &InnerCurve<Fp>,
        private_key: &[bool; 255],
        message: &Message,
        w: &mut Witness<Fp>,
    ) -> Fp {
        let h = hash_to_group(message, w);

        let u = {
            let _ = w.exists_no_check(h);
            let u = scale_non_constant(h, private_key, m, w);

            // unshift_nonzero
            let neg = m.to_affine().neg();
            w.exists(neg.y);
            w.add_fast(neg, u)
        };

        let GroupAffine::<Fp> { x, y, .. } = u;

        let mut inputs = message.to_inputs_owned();
        inputs.append_field(x);
        inputs.append_field(y);

        checked_hash(&MINA_VRF_OUTPUT, &inputs.to_fields(), w)
    }

    fn eval_and_check_public_key(
        m: &InnerCurve<Fp>,
        private_key: &[bool; 255],
        public_key: &PubKey,
        message: Message,
        w: &mut Witness<Fp>,
    ) -> Fp {
        let _ = {
            let _public_key_shifted = w.add_fast(m.to_affine(), *public_key.point());
            scale_generator(private_key, m, w)
        };

        eval(m, private_key, &message, w)
    }

    fn get_vrf_evaluation(
        m: &InnerCurve<Fp>,
        message: Message,
        prover_state: &v2::ConsensusStakeProofStableV2,
        w: &mut Witness<Fp>,
    ) -> anyhow::Result<(Fp, Box<crate::Account>)> {
        let private_key = prover_state.producer_private_key.to_field::<Fq>()?;
        let private_key = w.exists(field_to_bits::<Fq, 255>(private_key));

        let account = {
            let mut ledger: SparseLedger = (&prover_state.ledger).try_into()?;

            let staker_addr = message.delegator;
            let staker_addr =
                Address::from_index(staker_addr, constraint_constants().ledger_depth as usize);

            let account = ledger.get_exn(&staker_addr);
            let path = ledger.path_exn(staker_addr.clone());

            let (account, path) = w.exists((account, path));
            checked_verify_merkle_path(&account, &path, w);

            account
        };

        let delegate = {
            let delegate = account.delegate.as_ref().unwrap();
            decompress_var(delegate, w)
        };

        let evaluation = eval_and_check_public_key(m, &private_key, &delegate, message, w);

        Ok((evaluation, account))
    }

    fn is_satisfied(
        my_stake: Balance,
        total_stake: Amount,
        vrf_output: &[bool; VRF_OUTPUT_NBITS],
        w: &mut Witness<Fp>,
    ) -> Boolean {
        use floating_point::*;

        let top = SnarkyInteger::create(my_stake.to_field::<Fp>(), balance_upper_bound());
        let bottom = SnarkyInteger::create(total_stake.to_field::<Fp>(), amount_upper_bound());
        let precision = PARAMS.per_term_precision;

        let point = floating_point::of_quotient(precision, top, bottom, w);
        let rhs = snarky_taylor::one_minus_exp(&PARAMS, point, w);

        let lhs = vrf_output;
        Point::of_bits(lhs, VRF_OUTPUT_NBITS).le(&rhs, w)
    }

    pub const VRF_OUTPUT_NBITS: usize = 253;

    fn truncate_vrf_output(output: Fp, w: &mut Witness<Fp>) -> Box<[bool; VRF_OUTPUT_NBITS]> {
        let output: [bool; 255] = w.exists(field_to_bits(output));
        Box::new(std::array::from_fn(|i| output[i])) // 2 last bits are ignored
    }

    pub fn check(
        m: &InnerCurve<Fp>,
        epoch_ledger: &EpochLedger<Fp>,
        global_slot: &CheckedSlot<Fp>,
        _block_stake_winner: &CompressedPubKey,
        _block_creator: &CompressedPubKey,
        seed: Fp,
        prover_state: &v2::ConsensusStakeProofStableV2,
        w: &mut Witness<Fp>,
    ) -> anyhow::Result<(
        Boolean,
        Fp,
        Box<[bool; VRF_OUTPUT_NBITS]>,
        Box<crate::Account>,
    )> {
        let (winner_addr, winner_addr_bits) = {
            const LEDGER_DEPTH: usize = 35;
            assert_eq!(constraint_constants().ledger_depth, LEDGER_DEPTH as u64);

            let account_index = prover_state.delegator.as_u64();
            let bits = w.exists(to_bits::<_, LEDGER_DEPTH>(account_index));
            (AccountIndex(account_index), bits)
        };

        let (result, winner_account) = get_vrf_evaluation(
            m,
            Message {
                global_slot,
                seed,
                delegator: winner_addr,
                delegator_bits: winner_addr_bits,
            },
            prover_state,
            w,
        )?;

        let my_stake = winner_account.balance;
        let truncated_result = truncate_vrf_output(result, w);
        let satisifed = is_satisfied(my_stake, epoch_ledger.total_currency, &truncated_result, w);

        Ok((satisifed, result, truncated_result, winner_account))
    }
}

pub mod consensus {
    use ark_ff::Zero;
    use mina_signer::CompressedPubKey;
    use poseidon::hash::params::MINA_EPOCH_SEED;

    use super::{vrf::VRF_OUTPUT_NBITS, *};
    use crate::{
        decompress_pk,
        proofs::{
            numbers::{
                currency::CheckedCurrency,
                nat::{CheckedN, CheckedN32, CheckedSlotSpan},
            },
            transaction::{compress_var, create_shifted_inner_curve, CompressedPubKeyVar},
        },
        scan_state::{
            currency::{Amount, Length},
            transaction_logic::protocol_state::EpochData,
        },
        Account,
    };

    pub struct ConsensusConstantsChecked {
        pub k: CheckedLength<Fp>,
        pub delta: CheckedLength<Fp>,
        pub block_window_duration_ms: CheckedBlockTimeSpan<Fp>,
        pub slots_per_sub_window: CheckedLength<Fp>,
        pub slots_per_window: CheckedLength<Fp>,
        pub sub_windows_per_window: CheckedLength<Fp>,
        pub slots_per_epoch: CheckedLength<Fp>,
        pub grace_period_slots: CheckedLength<Fp>,
        pub grace_period_end: CheckedLength<Fp>,
        pub slot_duration_ms: CheckedBlockTimeSpan<Fp>,
        pub epoch_duration: CheckedBlockTimeSpan<Fp>,
        pub checkpoint_window_slots_per_year: CheckedLength<Fp>,
        pub checkpoint_window_size_in_slots: CheckedLength<Fp>,
        pub delta_duration: CheckedBlockTimeSpan<Fp>,
        pub genesis_state_timestamp: CheckedBlockTime<Fp>,
    }

    /// Number of millisecond per day
    const N_MILLIS_PER_DAY: u64 = 86400000;

    // Keep this in sync with the implementation of ConsensusConstants::create
    // in ledger/src/transaction_pool.rs
    fn create_constant_prime(
        prev_state: &ProtocolState,
        w: &mut Witness<Fp>,
    ) -> ConsensusConstantsChecked {
        let v2::MinaBaseProtocolConstantsCheckedValueStableV1 {
            k,
            slots_per_epoch,
            slots_per_sub_window,
            grace_period_slots,
            delta,
            genesis_state_timestamp,
        } = &prev_state.body.constants;

        let constraint_constants = constraint_constants();

        let of_u64 = |n: u64| CheckedN::<Fp>::from_field(n.into());
        let of_u32 = |n: u32| CheckedN::<Fp>::from_field(n.into());

        let block_window_duration_ms = of_u64(constraint_constants.block_window_duration_ms);

        let k = of_u32(k.as_u32());
        let delta = of_u32(delta.as_u32());
        let slots_per_sub_window = of_u32(slots_per_sub_window.as_u32());
        let sub_windows_per_window = of_u64(constraint_constants.sub_windows_per_window);

        let slots_per_window = slots_per_sub_window.const_mul(&sub_windows_per_window, w);

        let slots_per_epoch = of_u32(slots_per_epoch.as_u32());

        let slot_duration_ms = block_window_duration_ms.clone();

        let epoch_duration = {
            let size = &slots_per_epoch;
            slot_duration_ms.const_mul(size, w)
        };

        let delta_duration = {
            let delta_plus_one = delta.add(&CheckedN::one(), w);
            slot_duration_ms.const_mul(&delta_plus_one, w)
        };

        let grace_period_slots = of_u32(grace_period_slots.as_u32());
        let grace_period_end = { grace_period_slots.add(&slots_per_window, w) };

        let to_length = |v: CheckedN<Fp>| CheckedLength::from_field(v.to_field());
        let to_timespan = |v: CheckedN<Fp>| CheckedBlockTimeSpan::from_field(v.to_field());

        ConsensusConstantsChecked {
            k: to_length(k),
            delta: to_length(delta),
            block_window_duration_ms: to_timespan(block_window_duration_ms),
            slots_per_sub_window: to_length(slots_per_sub_window),
            slots_per_window: to_length(slots_per_window),
            sub_windows_per_window: to_length(sub_windows_per_window),
            slots_per_epoch: to_length(slots_per_epoch),
            grace_period_slots: to_length(grace_period_slots),
            grace_period_end: to_length(grace_period_end),
            slot_duration_ms: to_timespan(slot_duration_ms),
            epoch_duration: to_timespan(epoch_duration),
            checkpoint_window_slots_per_year: CheckedLength::zero(),
            checkpoint_window_size_in_slots: CheckedLength::zero(),
            delta_duration: to_timespan(delta_duration),
            genesis_state_timestamp: {
                let v = of_u64(genesis_state_timestamp.as_u64());
                CheckedBlockTime::from_field(v.to_field())
            },
        }
    }

    fn check_invariants(constants: &ConsensusConstantsChecked) {
        let slots_per_epoch = constants.slots_per_epoch.to_inner().as_u32();
        let slots_per_window = constants.slots_per_window.to_inner().as_u32();
        let grace_period_end = constants.grace_period_end.to_inner().as_u32();

        let grace_period_effective_end = grace_period_end - slots_per_window;

        assert!(grace_period_effective_end < (slots_per_epoch / 3))
    }

    fn create_constant(
        prev_state: &ProtocolState,
        w: &mut Witness<Fp>,
    ) -> ConsensusConstantsChecked {
        let mut constants = create_constant_prime(prev_state, w);

        let (checkpoint_window_slots_per_year, checkpoint_window_size_in_slots) = {
            let of_u64 = |n: u64| CheckedN::<Fp>::from_field(n.into());
            let of_field = |f: Fp| CheckedN::<Fp>::from_field(f);

            let per_year = of_u64(12);
            let slot_duration_ms = of_field(constants.slot_duration_ms.to_field());

            let (slots_per_year, _) = {
                let one_year_ms = of_u64(365 * N_MILLIS_PER_DAY);
                one_year_ms.div_mod(&slot_duration_ms, w)
            };

            let size_in_slots = {
                let (size_in_slots, _rem) = slots_per_year.div_mod(&per_year, w);
                size_in_slots
            };

            let to_length = |v: CheckedN<Fp>| CheckedLength::from_field(v.to_field());
            (to_length(slots_per_year), to_length(size_in_slots))
        };

        constants.checkpoint_window_slots_per_year = checkpoint_window_slots_per_year;
        constants.checkpoint_window_size_in_slots = checkpoint_window_size_in_slots;

        check_invariants(&constants);

        constants
    }

    type CheckedEpoch = CheckedBlockTime<Fp>;

    #[derive(Clone, Debug)]
    pub struct GlobalSlot {
        pub slot_number: CheckedSlot<Fp>,
        pub slots_per_epoch: CheckedLength<Fp>,
    }

    impl From<&v2::ConsensusGlobalSlotStableV1> for GlobalSlot {
        fn from(value: &v2::ConsensusGlobalSlotStableV1) -> Self {
            let v2::ConsensusGlobalSlotStableV1 {
                slot_number,
                slots_per_epoch,
            } = value;

            Self {
                slot_number: CheckedSlot::from_field(slot_number.as_u32().into()),
                slots_per_epoch: CheckedLength::from_field(slots_per_epoch.as_u32().into()),
            }
        }
    }

    impl GlobalSlot {
        fn of_slot_number(
            constants: &ConsensusConstantsChecked,
            slot_number: CheckedSlot<Fp>,
        ) -> Self {
            Self {
                slot_number,
                slots_per_epoch: constants.slots_per_epoch,
            }
        }

        fn diff_slots(&self, other: &Self, w: &mut Witness<Fp>) -> CheckedSlotSpan<Fp> {
            self.slot_number.diff(&other.slot_number, w)
        }

        fn less_than(&self, other: &Self, w: &mut Witness<Fp>) -> Boolean {
            self.slot_number.less_than(&other.slot_number, w)
        }

        fn to_epoch_and_slot(&self, w: &mut Witness<Fp>) -> (CheckedEpoch, CheckedSlot<Fp>) {
            let (epoch, slot) = self
                .slot_number
                .div_mod(&CheckedSlot::from_field(self.slots_per_epoch.to_field()), w);

            (CheckedEpoch::from_field(epoch.to_field()), slot)
        }
    }

    fn compute_supercharge_coinbase(
        winner_account: &Account,
        global_slot: &CheckedSlot<Fp>,
        w: &mut Witness<Fp>,
    ) -> Boolean {
        let winner_locked = winner_account.has_locked_tokens_checked(global_slot, w);
        winner_locked.neg()
    }

    fn same_checkpoint_window(
        constants: &ConsensusConstantsChecked,
        prev: &GlobalSlot,
        next: &GlobalSlot,
        w: &mut Witness<Fp>,
    ) -> Boolean {
        let slot1 = prev;
        let slot2 = next;

        let slot1 = &slot1.slot_number;
        let checkpoint_window_size_in_slots = &constants.checkpoint_window_size_in_slots;

        let (_q1, r1) = slot1.div_mod(&checkpoint_window_size_in_slots.to_slot(), w);

        let next_window_start =
            { slot1.to_field() - r1.to_field() + checkpoint_window_size_in_slots.to_field() };

        slot2
            .slot_number
            .less_than(&CheckedSlot::from_field(next_window_start), w)
    }

    struct GlobalSubWindow {
        inner: CheckedN32<Fp>,
    }

    impl std::ops::Deref for GlobalSubWindow {
        type Target = CheckedN32<Fp>;

        fn deref(&self) -> &Self::Target {
            &self.inner
        }
    }

    impl GlobalSubWindow {
        fn of_global_slot(
            constants: &ConsensusConstantsChecked,
            s: &GlobalSlot,
            w: &mut Witness<Fp>,
        ) -> Self {
            let slot_as_field = s.slot_number.to_field();
            let slot_as_field = CheckedN32::from_field(slot_as_field);

            let (q, _) = slot_as_field.div_mod(
                &CheckedN32::from_field(constants.slots_per_sub_window.to_field()),
                w,
            );
            Self { inner: q }
        }

        fn sub_window(&self, constants: &ConsensusConstantsChecked, w: &mut Witness<Fp>) -> Self {
            let (_, shift) = self.inner.div_mod(
                &CheckedN32::from_field(constants.sub_windows_per_window.to_field()),
                w,
            );
            Self { inner: shift }
        }

        fn equal(&self, other: &Self, w: &mut Witness<Fp>) -> Boolean {
            self.inner.equal(&other.inner, w)
        }

        fn add(&self, b: &CheckedLength<Fp>, w: &mut Witness<Fp>) -> Self {
            let inner = self.inner.add(&CheckedN32::from_field(b.to_field()), w);
            Self { inner }
        }

        fn gte(&self, other: &Self, w: &mut Witness<Fp>) -> Boolean {
            self.inner.gte(&other.inner, w)
        }
    }

    type SubWindow = CheckedN32<Fp>;

    struct UpdateMinWindowDensityParams<'a> {
        constants: &'a ConsensusConstantsChecked,
        prev_global_slot: &'a GlobalSlot,
        next_global_slot: &'a GlobalSlot,
        prev_sub_window_densities: Vec<u32>,
        prev_min_window_density: u32,
    }

    fn update_min_window_density(
        params: UpdateMinWindowDensityParams,
        w: &mut Witness<Fp>,
    ) -> (CheckedLength<Fp>, Vec<CheckedLength<Fp>>) {
        let UpdateMinWindowDensityParams {
            constants: c,
            prev_global_slot,
            next_global_slot,
            prev_sub_window_densities,
            prev_min_window_density,
        } = params;

        let prev_global_sub_window = GlobalSubWindow::of_global_slot(c, prev_global_slot, w);
        let next_global_sub_window = GlobalSubWindow::of_global_slot(c, next_global_slot, w);

        let prev_relative_sub_window = prev_global_sub_window.sub_window(c, w);
        let next_relative_sub_window = next_global_sub_window.sub_window(c, w);

        let same_sub_window = prev_global_sub_window.equal(&next_global_sub_window, w);
        let overlapping_window = {
            let x = prev_global_sub_window.add(&c.sub_windows_per_window, w);
            x.gte(&next_global_sub_window, w)
        };

        let current_sub_window_densities = prev_sub_window_densities
            .iter()
            .enumerate()
            .map(|(i, density)| {
                let gt_prev_sub_window =
                    SubWindow::constant(i).greater_than(&prev_relative_sub_window, w);

                // :(
                // This will be removed once we have cvar
                let lt_next_sub_window = if i == 0 {
                    SubWindow::constant(i).const_less_than(&next_relative_sub_window, w)
                } else {
                    SubWindow::constant(i).less_than(&next_relative_sub_window, w)
                };

                let within_range = {
                    let cond = prev_relative_sub_window.less_than(&next_relative_sub_window, w);
                    let on_true = gt_prev_sub_window.and(&lt_next_sub_window, w);
                    let on_false = gt_prev_sub_window.or(&lt_next_sub_window, w);

                    w.exists_no_check(match cond {
                        Boolean::True => on_true,
                        Boolean::False => on_false,
                    })
                };

                let density = Length::from_u32(*density).to_checked::<Fp>();

                let on_true = density;
                let on_false = {
                    let cond = overlapping_window.and(&within_range.neg(), w);
                    w.exists_no_check(match cond {
                        Boolean::True => density,
                        Boolean::False => CheckedLength::zero(),
                    })
                };

                w.exists_no_check(match same_sub_window {
                    Boolean::True => on_true,
                    Boolean::False => on_false,
                })
            })
            .collect::<Vec<_>>();

        let current_window_density = current_sub_window_densities.iter().enumerate().fold(
            CheckedLength::zero(),
            |acc, (i, v)| {
                // :(
                // This will be removed once we have cvar
                if i == 0 {
                    acc.const_add(v, w)
                } else {
                    acc.add(v, w)
                }
            },
        );

        let min_window_density = {
            let in_grace_period = next_global_slot.less_than(
                &GlobalSlot::of_slot_number(c, c.grace_period_end.to_slot()),
                w,
            );
            let prev_min_window_density = Length::from_u32(prev_min_window_density).to_checked();

            let cond = same_sub_window.or(&in_grace_period, w);
            let on_true = prev_min_window_density;
            let on_false = current_window_density.min(&prev_min_window_density, w);

            w.exists_no_check(match cond {
                Boolean::True => on_true,
                Boolean::False => on_false,
            })
        };

        let next_sub_window_densities = current_sub_window_densities
            .iter()
            .enumerate()
            .map(|(i, density)| {
                let is_next_sub_window = SubWindow::constant(i).equal(&next_relative_sub_window, w);

                let on_true = {
                    w.exists_no_check(match same_sub_window {
                        Boolean::True => density.const_succ(),
                        Boolean::False => CheckedLength::zero().const_succ(),
                    })
                };

                w.exists_no_check(match is_next_sub_window {
                    Boolean::True => on_true,
                    Boolean::False => *density,
                })
            })
            .collect::<Vec<_>>();

        (min_window_density, next_sub_window_densities)
    }

    #[derive(Debug, Clone)]
    pub struct ConsensusState {
        pub blockchain_length: currency::Length,
        pub epoch_count: currency::Length,
        pub min_window_density: currency::Length,
        pub sub_window_densities: Vec<currency::Length>,
        pub last_vrf_output: Box<[bool; VRF_OUTPUT_NBITS]>,
        pub curr_global_slot_since_hard_fork: GlobalSlot,
        pub global_slot_since_genesis: currency::Slot,
        pub total_currency: currency::Amount,
        pub staking_epoch_data: EpochData<Fp>,
        pub next_epoch_data: EpochData<Fp>,
        pub has_ancestor_in_same_checkpoint_window: bool,
        pub block_stake_winner: CompressedPubKey,
        pub block_creator: CompressedPubKey,
        pub coinbase_receiver: CompressedPubKey,
        pub supercharge_coinbase: bool,
    }

    #[derive(Debug, Clone)]
    pub struct CheckedConsensusState {
        pub blockchain_length: CheckedLength<Fp>,
        pub epoch_count: CheckedLength<Fp>,
        pub min_window_density: CheckedLength<Fp>,
        pub sub_window_densities: Vec<CheckedLength<Fp>>,
        pub last_vrf_output: Box<[bool; VRF_OUTPUT_NBITS]>,
        pub curr_global_slot_since_hard_fork: GlobalSlot,
        pub global_slot_since_genesis: CheckedSlot<Fp>,
        pub total_currency: CheckedAmount<Fp>,
        pub staking_epoch_data: EpochData<Fp>,
        pub next_epoch_data: EpochData<Fp>,
        pub has_ancestor_in_same_checkpoint_window: Boolean,
        pub block_stake_winner: CompressedPubKey,
        pub block_creator: CompressedPubKey,
        pub coinbase_receiver: CompressedPubKey,
        pub supercharge_coinbase: Boolean,
    }

    pub fn next_state_checked(
        prev_state: &ProtocolState,
        prev_state_hash: Fp,
        transition: &SnarkTransition,
        supply_increase: CheckedSigned<Fp, CheckedAmount<Fp>>,
        prover_state: &v2::ConsensusStakeProofStableV2,
        w: &mut Witness<Fp>,
    ) -> anyhow::Result<(Boolean, CheckedConsensusState)> {
        let previous_blockchain_state_ledger_hash = prev_state
            .body
            .blockchain_state
            .ledger_proof_statement
            .target
            .first_pass_ledger;
        let genesis_ledger_hash = prev_state.body.blockchain_state.genesis_ledger_hash;
        let consensus_transition =
            CheckedSlot::<Fp>::from_field(transition.consensus_transition.as_u32().into());
        let previous_state = &prev_state.body.consensus_state;
        // transiti

        let constants = create_constant(prev_state, w);

        let ConsensusState {
            curr_global_slot_since_hard_fork: prev_global_slot,
            ..
        } = &prev_state.body.consensus_state;

        let next_global_slot = GlobalSlot::of_slot_number(&constants, consensus_transition);

        let slot_diff = next_global_slot.diff_slots(prev_global_slot, w);

        {
            let global_slot_increased = prev_global_slot.less_than(&next_global_slot, w);
            let is_genesis = field::equal(
                CheckedSlot::zero().to_field(),
                next_global_slot.slot_number.to_field(),
                w,
            );

            Boolean::assert_any(&[global_slot_increased, is_genesis], w)
        };

        let (next_epoch, next_slot) = next_global_slot.to_epoch_and_slot(w);
        let (prev_epoch, _prev_slot) = prev_global_slot.to_epoch_and_slot(w);

        let global_slot_since_genesis =
            CheckedSlot::<Fp>::from_field(previous_state.global_slot_since_genesis.as_u32().into())
                .add(&CheckedSlot::<Fp>::from_field(slot_diff.to_field()), w);

        let epoch_increased = prev_epoch.less_than(&next_epoch, w);

        let staking_epoch_data = {
            let next_epoch_data: &EpochData<Fp> = &previous_state.next_epoch_data;
            let staking_epoch_data: &EpochData<Fp> = &previous_state.staking_epoch_data;

            w.exists_no_check(match epoch_increased {
                Boolean::True => next_epoch_data,
                Boolean::False => staking_epoch_data,
            })
            .clone()
        };

        let next_slot_number = next_global_slot.slot_number;

        let block_stake_winner = {
            let delegator_pk: CompressedPubKey = (&prover_state.delegator_pk).try_into()?;
            w.exists(delegator_pk)
        };

        let block_creator = {
            // TODO: See why `prover_state.producer_public_key` is compressed
            //       In OCaml it's uncompressed
            let producer_public_key: CompressedPubKey =
                (&prover_state.producer_public_key).try_into()?;
            let pk = decompress_pk(&producer_public_key).unwrap();
            w.exists_no_check(&pk);
            // TODO: Remove this
            let CompressedPubKeyVar { x, is_odd } = compress_var(pk.point(), w);
            CompressedPubKey { x, is_odd }
        };

        let coinbase_receiver = {
            let pk: CompressedPubKey = (&prover_state.coinbase_receiver_pk).try_into()?;
            w.exists(pk)
        };

        let (threshold_satisfied, vrf_result, truncated_vrf_result, winner_account) = {
            let m = create_shifted_inner_curve::<Fp>(w);

            vrf::check(
                &m,
                &staking_epoch_data.ledger,
                &next_slot_number,
                &block_stake_winner,
                &block_creator,
                staking_epoch_data.seed,
                prover_state,
                w,
            )?
        };

        let supercharge_coinbase =
            compute_supercharge_coinbase(&winner_account, &global_slot_since_genesis, w);

        let (new_total_currency, _overflow) = {
            let total_currency: Amount = previous_state.total_currency;
            total_currency
                .to_checked()
                .add_signed_flagged(supply_increase, w)
        };

        let has_ancestor_in_same_checkpoint_window =
            same_checkpoint_window(&constants, prev_global_slot, &next_global_slot, w);

        let in_seed_update_range = next_slot.in_seed_update_range(&constants, w);

        let update_next_epoch_ledger = {
            let snarked_ledger_is_still_genesis = field::equal(
                genesis_ledger_hash,
                previous_blockchain_state_ledger_hash,
                w,
            );
            epoch_increased.and(&snarked_ledger_is_still_genesis.neg(), w)
        };

        fn epoch_seed_update_var(seed: Fp, vrf_result: Fp, w: &mut Witness<Fp>) -> Fp {
            checked_hash(&MINA_EPOCH_SEED, &[seed, vrf_result], w)
        }

        let next_epoch_data = {
            let seed = {
                let base: Fp = previous_state.next_epoch_data.seed;
                let updated = epoch_seed_update_var(base, vrf_result, w);
                w.exists_no_check(match in_seed_update_range {
                    Boolean::True => updated,
                    Boolean::False => base,
                })
            };

            let epoch_length = {
                let base = w.exists_no_check(match epoch_increased {
                    Boolean::True => CheckedLength::zero(),
                    Boolean::False => {
                        let length: &Length = &previous_state.next_epoch_data.epoch_length;
                        length.to_checked()
                    }
                });
                base.const_succ()
            };

            let ledger = w.exists_no_check(match update_next_epoch_ledger {
                Boolean::True => EpochLedger {
                    hash: previous_blockchain_state_ledger_hash,
                    total_currency: new_total_currency.to_inner(), // TODO: Might overflow ?
                },
                Boolean::False => {
                    let ledger = &previous_state.next_epoch_data.ledger;
                    EpochLedger {
                        hash: ledger.hash,
                        total_currency: ledger.total_currency,
                    }
                }
            });

            let start_checkpoint = w.exists_no_check(match epoch_increased {
                Boolean::True => prev_state_hash,
                Boolean::False => previous_state.next_epoch_data.start_checkpoint,
            });

            let lock_checkpoint = {
                let base = w.exists_no_check(match epoch_increased {
                    Boolean::True => Fp::zero(),
                    Boolean::False => previous_state.next_epoch_data.lock_checkpoint,
                });
                w.exists_no_check(match in_seed_update_range {
                    Boolean::True => prev_state_hash,
                    Boolean::False => base,
                })
            };

            EpochData {
                ledger,
                seed,
                start_checkpoint,
                lock_checkpoint,
                epoch_length: epoch_length.to_inner(), // TODO: Overflow ?
            }
        };

        let blockchain_length = {
            let blockchain_length: &Length = &previous_state.blockchain_length;
            blockchain_length.to_checked::<Fp>().const_succ()
        };

        let epoch_count = {
            let epoch_count: &Length = &previous_state.epoch_count;
            match epoch_increased {
                Boolean::True => epoch_count.to_checked::<Fp>().const_succ(),
                Boolean::False => epoch_count.to_checked(),
            }
        };

        let (min_window_density, sub_window_densities) = update_min_window_density(
            UpdateMinWindowDensityParams {
                constants: &constants,
                prev_global_slot,
                next_global_slot: &next_global_slot,
                prev_sub_window_densities: previous_state
                    .sub_window_densities
                    .iter()
                    .map(|v| v.as_u32())
                    .collect(),
                prev_min_window_density: previous_state.min_window_density.as_u32(),
            },
            w,
        );

        let state = CheckedConsensusState {
            blockchain_length,
            epoch_count,
            min_window_density,
            sub_window_densities,
            last_vrf_output: truncated_vrf_result,
            curr_global_slot_since_hard_fork: next_global_slot,
            global_slot_since_genesis,
            total_currency: new_total_currency,
            staking_epoch_data,
            next_epoch_data,
            has_ancestor_in_same_checkpoint_window,
            block_stake_winner,
            block_creator,
            coinbase_receiver,
            supercharge_coinbase,
        };

        Ok((threshold_satisfied, state))
    }
}

fn is_genesis_state_var(cs: &ConsensusState, w: &mut Witness<Fp>) -> Boolean {
    let consensus::GlobalSlot {
        slot_number,
        slots_per_epoch: _,
    } = cs.curr_global_slot_since_hard_fork;

    CheckedSlot::zero().equal(&slot_number, w)
}

fn is_genesis_state_var2(cs: &CheckedConsensusState, w: &mut Witness<Fp>) -> Boolean {
    let curr_global_slot = &cs.curr_global_slot_since_hard_fork;
    let slot_number = &curr_global_slot.slot_number;

    CheckedSlot::zero().equal(slot_number, w)
}

fn genesis_state_hash_checked(
    state_hash: Fp,
    state: &ProtocolState,
    w: &mut Witness<Fp>,
) -> anyhow::Result<Fp> {
    let is_genesis = is_genesis_state_var(&state.body.consensus_state, w);

    Ok(w.exists_no_check(match is_genesis {
        Boolean::True => state_hash,
        Boolean::False => state.body.genesis_state_hash,
    }))
}

#[derive(Debug, Clone)]
pub struct ProtocolStateBody<CS = ConsensusState> {
    pub genesis_state_hash: Fp,
    pub blockchain_state: BlockchainState,
    pub consensus_state: CS,
    pub constants: v2::MinaBaseProtocolConstantsCheckedValueStableV1,
}

#[derive(Debug, Clone)]
pub struct ProtocolState<CS = ConsensusState> {
    pub previous_state_hash: Fp,
    pub body: ProtocolStateBody<CS>,
}

impl ProtocolStateBody {
    pub fn view(&self) -> ProtocolStateView {
        let Self {
            genesis_state_hash: _,
            blockchain_state,
            consensus_state: cs,
            constants: _,
        } = self;

        let sed = &cs.staking_epoch_data;
        let ned = &cs.next_epoch_data;

        ProtocolStateView {
            // <https://github.com/MinaProtocol/mina/blob/436023ba41c43a50458a551b7ef7a9ae61670b25/src/lib/mina_state/blockchain_state.ml#L58>
            //
            snarked_ledger_hash: blockchain_state
                .ledger_proof_statement
                .target
                .first_pass_ledger,
            blockchain_length: cs.blockchain_length,
            min_window_density: cs.min_window_density,
            total_currency: cs.total_currency,
            global_slot_since_genesis: cs.global_slot_since_genesis,
            staking_epoch_data: sed.clone(),
            next_epoch_data: ned.clone(),
        }
    }
}

fn protocol_create_var(
    previous_state_hash: Fp,
    genesis_state_hash: Fp,
    blockchain_state: &BlockchainState,
    consensus_state: &CheckedConsensusState,
    constants: &v2::MinaBaseProtocolConstantsCheckedValueStableV1,
) -> ProtocolState<CheckedConsensusState> {
    ProtocolState {
        previous_state_hash,
        body: ProtocolStateBody {
            genesis_state_hash,
            blockchain_state: blockchain_state.clone(),
            consensus_state: consensus_state.clone(),
            constants: constants.clone(),
        },
    }
}

fn block_main<'a>(
    params: BlockMainParams<'a>,
    w: &mut Witness<Fp>,
) -> anyhow::Result<(Fp, [PreviousProofStatement<'a>; 2])> {
    let BlockMainParams {
        transition,
        prev_state,
        prev_state_proof,
        txn_snark,
        txn_snark_proof,
        next_state,
        prover_state,
        pending_coinbase,
    } = params;

    let next_state: ProtocolState = next_state.try_into()?;
    let new_state_hash = w.exists(MinaHash::hash(&next_state));
    let transition: SnarkTransition = w.exists(transition.try_into()?);
    w.exists(txn_snark);

    let prev_state: ProtocolState = w.exists(prev_state.try_into()?);

    let (
        previous_state,
        previous_state_hash,
        previous_blockchain_proof_input, // TODO: Use hash here
        previous_state_body_hash,
    ) = {
        let (previous_state_hash, body) = checked_hash_protocol_state(&prev_state, w)?;

        (&prev_state, previous_state_hash, &prev_state, body)
    };

    // The previous ledger statement's variables: compared here with the
    // transaction snark's statement, and again with the new state's below.
    let previous_ledger_statement =
        LedgerStatementVar::new(&previous_state.body.blockchain_state.ledger_proof_statement);
    let txn_snark_statement: Statement<()> = txn_snark.clone().without_digest();
    let txn_snark_statement = LedgerStatementVar::new(&txn_snark_statement);

    let txn_stmt_ledger_hashes_didn_t_change =
        txn_statement_ledger_hashes_equal(&previous_ledger_statement, &txn_snark_statement, w);

    let supply_increase = CheckedSigned::on_if(
        txn_stmt_ledger_hashes_didn_t_change.var(),
        SignedAmountBranchParam {
            on_true: &CheckedSigned::zero(),
            on_false: &txn_snark_statement.supply_increase,
        },
        w,
    );

    let (updated_consensus_state, consensus_state) = consensus::next_state_checked(
        previous_state,
        previous_state_hash,
        &transition,
        supply_increase,
        prover_state,
        w,
    )?;

    let CheckedConsensusState {
        global_slot_since_genesis,
        coinbase_receiver,
        supercharge_coinbase,
        ..
    } = &consensus_state;

    let prev_pending_coinbase_root = previous_state
        .body
        .blockchain_state
        .staged_ledger_hash
        .pending_coinbase_hash;

    let genesis_state_hash = genesis_state_hash_checked(previous_state_hash, previous_state, w)?;

    let (new_state, is_base_case) = {
        let mut t = protocol_create_var(
            previous_state_hash,
            genesis_state_hash,
            &transition.blockchain_state,
            &consensus_state,
            &previous_state.body.constants,
        );
        let is_base_case = CircuitVar::Var(is_genesis_state_var2(&t.body.consensus_state, w));

        let previous_state_hash = match constraint_constants().fork.as_ref() {
            Some(ForkConstants {
                state_hash: fork_prev,
                ..
            }) => w.exists_no_check(match is_base_case.value() {
                Boolean::True => *fork_prev,
                Boolean::False => t.previous_state_hash,
            }),
            None => t.previous_state_hash,
        };
        t.previous_state_hash = previous_state_hash;
        checked_hash_protocol_state2(&t, w);
        (t, is_base_case)
    };

    let (txn_snark_should_verify, success) = {
        let mut pending_coinbase = PendingCoinbaseWitness {
            is_new_stack: pending_coinbase.is_new_stack,
            pending_coinbase: (&pending_coinbase.pending_coinbases).try_into()?,
        };

        let global_slot = global_slot_since_genesis;

        let (new_pending_coinbase_hash, deleted_stack, no_coinbases_popped) = {
            let (root_after_delete, deleted_stack) = PendingCoinbase::pop_coinbases(
                txn_stmt_ledger_hashes_didn_t_change.neg(),
                &mut pending_coinbase,
                w,
            )?;

            let no_coinbases_popped =
                field::equal(root_after_delete, prev_pending_coinbase_root, w);

            let new_root = PendingCoinbase::add_coinbase_checked(
                &transition.pending_coinbase_update,
                coinbase_receiver,
                *supercharge_coinbase,
                previous_state_body_hash,
                global_slot,
                &mut pending_coinbase,
                w,
            )?;

            (new_root, deleted_stack, no_coinbases_popped)
        };

        let current_ledger_statement = &new_state.body.blockchain_state.ledger_proof_statement;
        let pending_coinbase_source_stack = Stack::var_create_with(&deleted_stack);

        let txn_snark_input_correct = {
            fee_excess::assert_equal_checked(&FeeExcess::empty(), &txn_snark.fee_excess, w);

            let s1 = previous_ledger_statement.statement;
            let s2 = current_ledger_statement;

            let ledger_statement_valid = validate_ledgers_at_merge_checked(
                &StatementLedgers::of_statement(s1),
                &StatementLedgers::of_statement(s2),
                w,
            );

            let a = txn_snark
                .source
                .pending_coinbase_stack
                .equal_var(&pending_coinbase_source_stack, w);
            let b = txn_snark
                .target
                .pending_coinbase_stack
                .equal_var(&deleted_stack, w);

            Boolean::all(&[ledger_statement_valid, a, b], w)
        };

        let nothing_changed = Boolean::all(
            &[txn_stmt_ledger_hashes_didn_t_change, no_coinbases_popped],
            w,
        );

        let correct_coinbase_status = {
            let new_root = transition
                .blockchain_state
                .staged_ledger_hash
                .pending_coinbase_hash;
            field::equal(new_pending_coinbase_hash, new_root, w)
        };

        Boolean::assert_any(&[txn_snark_input_correct, nothing_changed], w);

        // Mesa: when nothing changed (no ledger proof, no coinbase popped),
        // the new ledger statement must be the previous one. Before, the
        // block step left it unconstrained in that case.
        //
        // OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/blockchain_snark/blockchain_snark_state.ml#L315-L324>
        let ledger_statements_equal = txn_statement_ledger_hashes_equal(
            &previous_ledger_statement,
            &LedgerStatementVar::new(current_ledger_statement),
            w,
        );
        Boolean::assert_any(&[nothing_changed.neg(), ledger_statements_equal], w);

        let transaction_snark_should_verifiy = CircuitVar::Var(nothing_changed.neg());

        let result = Boolean::all(&[updated_consensus_state, correct_coinbase_status], w);

        (transaction_snark_should_verifiy, result)
    };

    let prev_should_verify = is_base_case.neg();

    Boolean::assert_any(&[*is_base_case.value(), success], w);

    let previous_proof_statements = [
        PreviousProofStatement {
            public_input: Rc::new(MinaHash::hash(previous_blockchain_proof_input)),
            proof: prev_state_proof,
            proof_must_verify: prev_should_verify,
        },
        PreviousProofStatement {
            public_input: Rc::new(txn_snark.clone()),
            proof: txn_snark_proof,
            proof_must_verify: txn_snark_should_verify,
        },
    ];

    Ok((new_state_hash, previous_proof_statements))
}

struct BlockMainParams<'a> {
    transition: &'a v2::MinaStateSnarkTransitionValueStableV2,
    prev_state: &'a v2::MinaStateProtocolStateValueStableV2,
    prev_state_proof: &'a v2::MinaBaseProofStableV2,
    txn_snark: &'a Statement<SokDigest>,
    txn_snark_proof: &'a v2::TransactionSnarkProofStableV2,
    next_state: &'a v2::MinaStateProtocolStateValueStableV2,
    prover_state: &'a v2::ConsensusStakeProofStableV2,
    pending_coinbase: &'a v2::MinaBasePendingCoinbaseWitnessStableV2,
}

pub struct BlockParams<'a> {
    pub input: &'a v2::ProverExtendBlockchainInputStableV2,
    pub block_step_prover: &'a Prover<Fp>,
    pub block_wrap_prover: &'a Prover<Fq>,
    pub tx_wrap_prover: &'a Prover<Fq>,
    /// When set to `true`, `generate_block_proof` will not create a proof, but only
    /// verify constraints in the step witnesses
    pub only_verify_constraints: bool,
    /// For debugging only
    pub expected_step_proof: Option<&'static str>,
    /// For debugging only
    pub ocaml_wrap_witness: Option<Vec<Fq>>,
}

const BLOCK_N_PREVIOUS_PROOFS: usize = 2;

pub(super) fn generate_block_proof(
    params: BlockParams,
    w: &mut Witness<Fp>,
) -> anyhow::Result<WrapProof> {
    let BlockParams {
        input:
            v2::ProverExtendBlockchainInputStableV2 {
                chain,
                next_state,
                block,
                ledger_proof,
                prover_state,
                pending_coinbase,
            },
        block_step_prover,
        block_wrap_prover,
        tx_wrap_prover,
        only_verify_constraints,
        expected_step_proof,
        ocaml_wrap_witness,
    } = params;

    let (txn_snark_statement, txn_snark_proof) =
        ledger_proof_opt(ledger_proof.as_deref(), next_state).context("ledger_proof_opt")?;
    let prev_state_proof = &chain.proof;

    let (new_state_hash, previous_proof_statements) = block_main(
        BlockMainParams {
            transition: block,
            prev_state: &chain.state,
            prev_state_proof,
            txn_snark: &txn_snark_statement,
            txn_snark_proof: &txn_snark_proof,
            next_state,
            prover_state,
            pending_coinbase,
        },
        w,
    )
    .context("block_main")?;

    let prev_challenge_polynomial_commitments =
        extract_recursion_challenges(&[prev_state_proof, &txn_snark_proof])
            .context("extract_recursion_challenges")?;

    let rule = InductiveRule {
        previous_proof_statements,
        public_output: (),
        auxiliary_output: (),
    };

    let dlog_plonk_index = super::merge::dlog_plonk_index(block_wrap_prover);
    let verifier_index = block_wrap_prover.index.verifier_index.as_ref().unwrap();

    let tx_dlog_plonk_index = super::merge::dlog_plonk_index(tx_wrap_prover);
    let tx_verifier_index = tx_wrap_prover.index.verifier_index.as_ref().unwrap();

    let dlog_plonk_index_cvar = dlog_plonk_index.to_cvar(CircuitVar::Var);
    let tx_dlog_plonk_index_cvar = tx_dlog_plonk_index.to_cvar(CircuitVar::Constant);

    let indexes = [
        (verifier_index, &dlog_plonk_index_cvar),
        (tx_verifier_index, &tx_dlog_plonk_index_cvar),
    ];

    let block_data = make_step_block_data(&dlog_plonk_index_cvar);
    let tx_data = make_step_transaction_data(&tx_dlog_plonk_index_cvar);
    let for_step_datas = [&block_data, &tx_data];

    let app_state: Rc<dyn ToFieldElementsDebug> = Rc::new(new_state_hash);

    let StepProof {
        statement,
        prev_evals,
        proof_with_public: proof,
    } = step::<StepBlockProof, BLOCK_N_PREVIOUS_PROOFS>(
        StepParams {
            app_state: Rc::clone(&app_state),
            rule,
            for_step_datas,
            indexes,
            prev_challenge_polynomial_commitments,
            hack_feature_flags: OptFlag::No,
            wrap_prover: block_wrap_prover,
            step_prover: block_step_prover,
            only_verify_constraints,
        },
        w,
    )
    .context("step")?;

    if let Some(expected) = expected_step_proof {
        let proof_json = serde_json::to_vec(&proof.proof).unwrap();
        assert_eq!(sha256_sum(&proof_json), expected);
    };

    let mut w = Witness::new::<WrapBlockProof>();

    if let Some(ocaml_aux) = ocaml_wrap_witness {
        w.ocaml_aux = ocaml_aux;
    };

    wrap::<WrapBlockProof>(
        WrapParams {
            app_state,
            proof_with_public: &proof,
            step_statement: statement,
            prev_evals: &prev_evals,
            dlog_plonk_index: &dlog_plonk_index,
            step_prover_index: &block_step_prover.index,
            wrap_prover: block_wrap_prover,
            wrap_domain_indices: None,
        },
        &mut w,
    )
    .context("wrap")
}

/// R15 (private implementation notes, item 4): witness cost of
/// the ledger-statement constraint Mesa adds to the block step.
///
/// OCaml 4.0.2 calls `txn_statement_ledger_hashes_equal` a second time, on the
/// previous and the current ledger statement, and asserts
/// `any [not nothing_changed; equal]`. `Currency.Amount.Signed.Checked.value`
/// caches the signed value on the variable, and the first call already
/// evaluated the five signed amounts of the previous statement, so OCaml's
/// second call pushes 61 values plus 1 for the assert: 62, the block step's
/// AUX_LEN delta left after the EndoMul and `ft_eval0` terms. Comparing new
/// variables of the previous statement instead ([`LedgerStatementVar::new`]
/// again) pushes those five values a second time.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/blockchain_snark/blockchain_snark_state.ml#L315-L324>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/currency/currency.ml#L550-L560>
#[cfg(test)]
mod r15_ledger_statement_witness {
    use super::*;
    use crate::scan_state::{
        currency::{Amount, Signed},
        transaction_logic::local_state::LocalState,
    };

    fn statement(seed: u64) -> Statement<()> {
        let registers = |n: u64| Registers {
            first_pass_ledger: Fp::from(seed + n),
            second_pass_ledger: Fp::from(seed + n + 1),
            pending_coinbase_stack: Stack::empty(),
            local_state: LocalState::dummy(),
        };
        Statement {
            source: registers(1),
            target: registers(3),
            connecting_ledger_left: Fp::from(seed + 5),
            connecting_ledger_right: Fp::from(seed + 6),
            supply_increase: Signed::<Amount>::zero(),
            fee_excess: FeeExcess::empty(),
            sok_digest: (),
        }
    }

    #[test]
    fn second_ledger_statement_check_witness_count() {
        let (previous, txn_snark, current) = (statement(0), statement(0), statement(10));
        let nothing_changed = Boolean::False;
        let previous_var = LedgerStatementVar::new(&previous);

        // The first call (txn_stmt_ledger_hashes_didn't_change): nothing is
        // cached yet.
        let mut w = Witness::<Fp>::empty();
        let eq = txn_statement_ledger_hashes_equal(
            &previous_var,
            &LedgerStatementVar::new(&txn_snark),
            &mut w,
        );
        assert_eq!(eq, Boolean::True);
        let first = w.aux().len();

        // Mesa's second call, as `block_main` makes it: the previous
        // statement's variables are the first call's.
        let mut cached = Witness::<Fp>::empty();
        let eq = txn_statement_ledger_hashes_equal(
            &previous_var,
            &LedgerStatementVar::new(&current),
            &mut cached,
        );
        assert_eq!(eq, Boolean::False);
        Boolean::assert_any(&[nothing_changed.neg(), eq], &mut cached);

        // With new variables for the previous statement.
        let mut naive = Witness::<Fp>::empty();
        let eq = txn_statement_ledger_hashes_equal(
            &LedgerStatementVar::new(&previous),
            &LedgerStatementVar::new(&current),
            &mut naive,
        );
        Boolean::assert_any(&[nothing_changed.neg(), eq], &mut naive);

        println!(
            "first call {first}, second call + assert {}, with new variables {}",
            cached.aux().len(),
            naive.aux().len()
        );
        assert_eq!(first, 66);
        assert_eq!(cached.aux().len(), 62);
        assert_eq!(naive.aux().len(), 67);
    }
}

/// P4.5: the Mesa block step and block wrap circuits on the base case, the
/// real genesis transition of Mesa mainnet.
#[cfg(test)]
mod mesa_tests {
    use mina_p2p_messages::binprot::BinProtRead;

    use super::*;
    use crate::{
        proofs::{
            constants::ProofConstants,
            provers::{init_devnet_with_mesa_blobs, init_mainnet_with_mesa_blobs, BlockProver},
            transaction::mesa_tests::{Outcome, PROVING},
            verification::verify_block_proof,
            verifiers::{block_verifier_index_of, BlockVerifier, TransactionVerifier},
        },
        scan_state::scan_state::transaction_snark::LedgerProof,
        sparse_ledger::SparseLedger,
    };

    /// The block prover's input for the Mesa mainnet genesis block, height
    /// 548147 (3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7), the
    /// parent of the first Mesa block, 548148
    /// (3NKtYZVHgyWrTzFE8j594Wy6HmFTb9rWw3sfPxZmZgRkrg572bAP), as the node
    /// builds it to prove its genesis block. Written by
    /// `tools/block-replay/examples/mesa_genesis_block_input.rs`.
    const GENESIS: &[u8] = include_bytes!("data/mesa_mainnet_genesis_block_input.bin");

    const GENESIS_HASH: &str = "3NLntCRR9RvjtzP24Q1dRvdhmpjiPvfKGaJmAZuJgYoqGpevqDR7";

    fn genesis_input() -> v2::ProverExtendBlockchainInputStableV2 {
        v2::ProverExtendBlockchainInputStableV2::binprot_read(&mut &*GENESIS).unwrap()
    }

    #[test]
    fn pc2_emitted_ledger_proof_read_cost_and_unavailable_error() {
        use mina_p2p_messages::{binprot::BinProtWrite, proof_cache::ProofStore};
        use std::time::Instant;
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-data/proof-cache/emitted-{}",
            std::process::id()
        ));
        let store = ProofStore::dir(&dir).unwrap();
        let bytes = include_bytes!("../verifier/fixtures/mainnet-551267-1.bin");
        let work = v2::TransactionSnarkWorkTStableV2::binprot_read(&mut bytes.as_slice()).unwrap();
        let proof = match &work.proofs {
            v2::TransactionSnarkWorkTStableV2Proofs::One(p) => p,
            v2::TransactionSnarkWorkTStableV2Proofs::Two((p, _)) => p,
        };
        let mut input = genesis_input();
        input.ledger_proof = Some(Arc::new(proof.clone()));
        let expected = ledger_proof_opt(Some(proof), &input.next_state).unwrap();
        let mut wire = Vec::new();
        input.binprot_write(&mut wire).unwrap();
        let rounds = 2000;
        let started = Instant::now();
        for _ in 0..rounds {
            std::hint::black_box(ledger_proof_opt(Some(proof), &input.next_state).unwrap());
        }
        let decoded = started.elapsed() / rounds;
        proof.0.proof.cache_in(&store).unwrap();
        let mut times = Vec::new();
        for _ in 0..rounds {
            let start = Instant::now();
            let got = ledger_proof_opt(Some(proof), &input.next_state).unwrap();
            times.push(start.elapsed());
            assert_eq!(got, expected);
        }
        times.sort();
        let mean = times.iter().sum::<std::time::Duration>() / rounds;
        let started = Instant::now();
        for _ in 0..rounds {
            let mut bytes = Vec::new();
            input.binprot_write(&mut bytes).unwrap();
            assert_eq!(bytes, wire);
        }
        println!("PC2 emitted ledger proof: decoded mean {decoded:?}; cached mean {mean:?}, p50 {:?}, p99 {:?}, max {:?}; whole prover input encode mean {:?} (2000 rounds)", times[1000], times[1980], times[1999], started.elapsed() / rounds);
        let file = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::remove_file(file).unwrap();
        assert!(ledger_proof_opt(Some(proof), &input.next_state).is_err());
        assert!(input.binprot_write(&mut Vec::new()).is_err());
        drop((input, work));
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn state_hash(state: &v2::MinaStateProtocolStateValueStableV2) -> Fp {
        MinaHash::hash(&ProtocolState::try_from(state).unwrap())
    }

    /// The fixture is the base case with Mesa mainnet's fork constants: the
    /// negative-one state to the genesis state, whose previous state hash is
    /// the fork's, at the fork's height plus one and its global slot, with
    /// the genesis winner's account in the staking epoch ledger.
    #[test]
    fn mesa_genesis_block_input_is_the_base_case() {
        let fork = mina_core::network::mainnet::CONSTRAINT_CONSTANTS
            .fork
            .unwrap();
        let input = genesis_input();
        let (negative_one, genesis) = (&input.chain.state, &input.next_state);

        let genesis_hash = v2::StateHash::from_fp(state_hash(genesis));
        assert_eq!(genesis_hash.to_string(), GENESIS_HASH);
        assert_eq!(
            genesis.previous_state_hash.to_field::<Fp>().unwrap(),
            fork.state_hash
        );
        assert_eq!(
            genesis.body.genesis_state_hash.to_field::<Fp>().unwrap(),
            state_hash(negative_one)
        );

        let consensus = &genesis.body.consensus_state;
        assert_eq!(
            consensus.blockchain_length.as_u32(),
            fork.blockchain_length + 1
        );
        assert_eq!(
            consensus.global_slot_since_genesis.as_u32(),
            fork.global_slot_since_genesis
        );
        assert_eq!(
            consensus
                .curr_global_slot_since_hard_fork
                .slot_number
                .as_u32(),
            0
        );
        assert_eq!(input.block.consensus_transition.as_u32(), 0);
        assert!(input.ledger_proof.is_none());
        assert_eq!(
            negative_one.body.blockchain_state.ledger_proof_statement,
            genesis.body.blockchain_state.ledger_proof_statement
        );

        let staking = &negative_one.body.consensus_state.staking_epoch_data;
        let mut stake_ledger = SparseLedger::try_from(&input.prover_state.ledger).unwrap();
        assert_eq!(
            stake_ledger.merkle_root(),
            staking.ledger.hash.to_field::<Fp>().unwrap()
        );
        assert_eq!(input.prover_state.delegator.0.as_u64(), 0);
        assert_eq!(
            input.prover_state.delegator_pk,
            consensus.block_stake_winner
        );
    }

    /// The genesis input with a ledger statement that changed while nothing
    /// else did: the transaction snark's statement is the previous one (with
    /// a dummy proof, which need not verify then), and the new state claims
    /// another connecting ledger. Before Mesa (OCaml 3.5.0) no constraint of
    /// the block step tied the new statement to the previous one in this
    /// case; 4.0.2's ledger-statement constraint rejects it.
    fn statement_changed_without_proof() -> v2::ProverExtendBlockchainInputStableV2 {
        let mut input = genesis_input();
        let previous: Statement<()> = (&input
            .chain
            .state
            .body
            .blockchain_state
            .ledger_proof_statement)
            .try_into()
            .unwrap();
        let proof = LedgerProof::create(
            previous,
            SokDigest::default(),
            dummy::dummy_transaction_proof(),
        );
        input.ledger_proof = Some(Arc::new((&proof).into()));

        let mut changed: Statement<()> = (&input
            .next_state
            .body
            .blockchain_state
            .ledger_proof_statement)
            .try_into()
            .unwrap();
        changed.connecting_ledger_right += Fp::from(1u64);
        let changed: v2::MinaStateBlockchainStateValueStableV2LedgerProofStatement =
            (&changed).into();
        input
            .next_state
            .body
            .blockchain_state
            .ledger_proof_statement = changed.clone();
        input.block.blockchain_state.ledger_proof_statement = changed;
        input
    }

    /// Generates the block step witness and checks every gate and copy
    /// constraint with kimchi, without proving. Returns the outcome and the
    /// witness length.
    fn check_constraints(
        input: &v2::ProverExtendBlockchainInputStableV2,
        provers: &BlockProver,
    ) -> (Outcome, usize) {
        let mut w = Witness::new::<StepBlockProof>();
        let result = generate_block_proof(
            BlockParams {
                input,
                block_step_prover: &provers.block_step_prover,
                block_wrap_prover: &provers.block_wrap_prover,
                tx_wrap_prover: &provers.tx_wrap_prover,
                only_verify_constraints: true,
                expected_step_proof: None,
                ocaml_wrap_witness: None,
            },
            &mut w,
        );
        (Outcome::of(result), w.aux().len())
    }

    fn provers() -> BlockProver {
        init_mainnet_with_mesa_blobs();
        BlockProver::make(
            Some(BlockVerifier::make()),
            Some(TransactionVerifier::make()),
        )
        .unwrap()
    }

    /// As [`provers`], with the Mesa devnet circuits (`4.0.2devnet`) and the
    /// devnet keys.
    fn devnet_provers() -> BlockProver {
        init_devnet_with_mesa_blobs();
        BlockProver::make(
            Some(BlockVerifier::make()),
            Some(TransactionVerifier::make()),
        )
        .unwrap()
    }

    /// Whether the blockchain verifier of the network the global config is
    /// set to, the live Mesa key, accepts `proof` for `state`, as the node
    /// verifies a block it receives.
    fn verifies(state: &v2::MinaStateProtocolStateValueStableV2, proof: &WrapProof) -> bool {
        let proof = v2::MinaBaseProofStableV2(proof.into());
        verify_block_proof(state, &proof, &BlockVerifier::make())
    }

    /// Whether the other network's embedded blockchain key, built without
    /// the global config, accepts `proof` for `state`. A block proof made for
    /// one network must not verify on the other.
    fn other_network_verifies(
        state: &v2::MinaStateProtocolStateValueStableV2,
        proof: &WrapProof,
    ) -> bool {
        let other = match mina_core::NetworkConfig::global().name {
            "devnet" => "mainnet",
            _ => "devnet",
        };
        let proof = v2::MinaBaseProofStableV2(proof.into());
        verify_block_proof(state, &proof, &block_verifier_index_of(other).unwrap())
    }

    /// P4.5's gate for the block step: the witness of the genesis transition
    /// has the Mesa block step's `AUX_LEN`, 343869, which counts the 62
    /// values of the ledger-statement constraint Mesa added, and satisfies
    /// the circuit (34901 rows). A ledger statement that changes without a
    /// ledger proof does not.
    #[test]
    #[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_genesis_block_step() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = provers();

        let (outcome, aux_len) = check_constraints(&genesis_input(), &provers);
        assert_eq!(aux_len, 343869);
        assert_eq!(aux_len, StepBlockProof::AUX_LEN);
        assert_eq!(outcome, Outcome::Satisfied);

        let (outcome, aux_len) = check_constraints(&statement_changed_without_proof(), &provers);
        assert_eq!(aux_len, StepBlockProof::AUX_LEN);
        eprintln!("ledger statement changed without a proof: {outcome:?}");
        assert!(matches!(outcome, Outcome::NotSatisfied(_)), "{outcome:?}");
    }

    /// P4.5's gate for the block wrap: the genesis block proves, step and
    /// wrap (the wrap witness must have the Mesa block wrap's `AUX_LEN`,
    /// 181742, asserted in `compute_witness`), and the proof verifies with the
    /// live mainnet blockchain key for the genesis state, and for no other
    /// state, and not with the devnet key (P4.8).
    #[test]
    #[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_genesis_block_proof_verifies() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = provers();
        let input = genesis_input();

        let mut w = Witness::new::<StepBlockProof>();
        let proof = generate_block_proof(
            BlockParams {
                input: &input,
                block_step_prover: &provers.block_step_prover,
                block_wrap_prover: &provers.block_wrap_prover,
                tx_wrap_prover: &provers.tx_wrap_prover,
                only_verify_constraints: false,
                expected_step_proof: None,
                ocaml_wrap_witness: None,
            },
            &mut w,
        )
        .unwrap();
        assert_eq!(WrapBlockProof::AUX_LEN, 181742);
        assert_eq!(WrapBlockProof::ROWS, 14664);

        assert!(verifies(&input.next_state, &proof));
        assert!(!other_network_verifies(&input.next_state, &proof));
        // The negative-one state fails, and so does the genesis state with
        // another genesis state hash, its parent's.
        assert!(!verifies(&input.chain.state, &proof));
        let mut other = input.next_state.clone();
        other.body.genesis_state_hash = other.previous_state_hash.clone();
        assert!(!verifies(&other, &proof));
    }

    /// Two blocks of P4.6a's offline Mesa chain, written by
    /// `tools/block-replay/examples/mesa_offline_chain.rs`: the block
    /// prover's inputs of its first two blocks after genesis, built by the
    /// node's block producer code. Its genesis is Mesa mainnet's but for the
    /// staking epoch ledger, where the genesis winner holds three times the
    /// total currency, so that it wins slots: a real Mesa block can't be
    /// proved again without its producer's private key. Each block carries the
    /// user commands of the real block at its height. The private key in them
    /// is the genesis winner's, which is public.
    const OFFLINE_548148: &[u8] = include_bytes!("data/mesa_offline_chain_548148_input.bin");
    const OFFLINE_548149: &[u8] = include_bytes!("data/mesa_offline_chain_548149_input.bin");

    const OFFLINE_GENESIS_HASH: &str = "3NLpUb3wT12N8XwgUiNx1LvtmAjmhd7jmMwPAt5ABCcEyQStgvVb";
    const OFFLINE_548148_HASH: &str = "3NLeqFbhhaaWxo8f3nkzRLQo76qNUBJui8MajdWuBMAPfUSFjtKh";
    const OFFLINE_548149_HASH: &str = "3NKLvHFnXhZw3mzUDz8ZdaicqVGMHEuoUYzB4YxjzKrkXLg2nYNr";

    fn offline_input(bytes: &[u8]) -> v2::ProverExtendBlockchainInputStableV2 {
        v2::ProverExtendBlockchainInputStableV2::binprot_read(&mut &*bytes).unwrap()
    }

    /// The offline chain's genesis is the Mesa mainnet genesis with another
    /// staking epoch ledger, and its two blocks are non-genesis transitions:
    /// each extends the previous state, at a later slot, without a ledger
    /// proof, with the genesis winner's account in the staking epoch ledger
    /// as the stake proof, delegating to the block's producer.
    #[test]
    fn mesa_offline_chain_inputs_extend_the_chain() {
        let first = offline_input(OFFLINE_548148);
        let second = offline_input(OFFLINE_548149);
        assert_eq!(second.chain.state, first.next_state);

        let genesis = &first.chain.state;
        let genesis_hash = state_hash(genesis);
        assert_eq!(
            v2::StateHash::from_fp(genesis_hash).to_string(),
            OFFLINE_GENESIS_HASH
        );
        // Mesa mainnet's negative-one and genesis states with the offline
        // staking epoch ledger are the offline chain's.
        let staking_ledger = &genesis.body.consensus_state.staking_epoch_data.ledger;
        let mesa = genesis_input();
        let (mut negative_one, mut mesa_genesis) = (mesa.chain.state, mesa.next_state);
        for state in [&mut negative_one, &mut mesa_genesis] {
            let staking = &mut state.body.consensus_state.staking_epoch_data;
            assert_ne!(&staking.ledger, staking_ledger);
            staking.ledger = staking_ledger.clone();
        }
        // So is the negative-one state's hash, which the genesis state
        // records twice.
        let negative_one_hash = v2::StateHash::from_fp(state_hash(&negative_one));
        mesa_genesis.body.genesis_state_hash = negative_one_hash.clone();
        let next_epoch = &mut mesa_genesis.body.consensus_state.next_epoch_data;
        next_epoch.lock_checkpoint = negative_one_hash;
        assert_eq!(&mesa_genesis, genesis);

        for (input, height, hash) in [
            (&first, 548148, OFFLINE_548148_HASH),
            (&second, 548149, OFFLINE_548149_HASH),
        ] {
            assert_offline_block(input, height, hash, genesis_hash);
        }
    }

    /// `input` is a non-genesis transition of an offline chain of genesis
    /// `genesis_hash` to the state of hash `hash` at `height`: it extends the
    /// previous state, at a later slot, without a ledger proof, with the
    /// genesis winner's account (index 0) in the staking epoch ledger as the
    /// stake proof, delegating to itself, the block's producer.
    fn assert_offline_block(
        input: &v2::ProverExtendBlockchainInputStableV2,
        height: u32,
        hash: &str,
        genesis_hash: Fp,
    ) {
        let (parent, state) = (&input.chain.state, &input.next_state);
        assert_eq!(v2::StateHash::from_fp(state_hash(state)).to_string(), hash);
        assert_eq!(
            state.previous_state_hash.to_field::<Fp>().unwrap(),
            state_hash(parent)
        );
        assert_eq!(
            state.body.genesis_state_hash.to_field::<Fp>().unwrap(),
            genesis_hash
        );

        let consensus = &state.body.consensus_state;
        let slot = &consensus.curr_global_slot_since_hard_fork.slot_number;
        assert_eq!(consensus.blockchain_length.as_u32(), height);
        assert!(
            slot.as_u32()
                > parent
                    .body
                    .consensus_state
                    .curr_global_slot_since_hard_fork
                    .slot_number
                    .as_u32()
        );
        assert_eq!(&input.block.consensus_transition, slot);
        assert!(input.ledger_proof.is_none());
        assert_eq!(
            state.body.blockchain_state.ledger_proof_statement,
            parent.body.blockchain_state.ledger_proof_statement
        );

        let mut stake_ledger = SparseLedger::try_from(&input.prover_state.ledger).unwrap();
        assert_eq!(
            stake_ledger.merkle_root(),
            parent
                .body
                .consensus_state
                .staking_epoch_data
                .ledger
                .hash
                .to_field::<Fp>()
                .unwrap()
        );
        assert_eq!(input.prover_state.delegator.0.as_u64(), 0);
        let account = stake_ledger.get_exn(&crate::Address::from_index(
            crate::AccountIndex(0),
            constraint_constants().ledger_depth as usize,
        ));
        assert_eq!(account.delegate.as_ref(), Some(&account.public_key));
        let producer = v2::NonZeroCurvePoint::from(&account.public_key);
        assert_eq!(input.prover_state.producer_public_key, producer);
        assert_eq!(consensus.block_creator, producer);
        assert_eq!(consensus.block_stake_winner, producer);
    }

    /// P4.6a: both offline blocks' step witnesses satisfy the Mesa block step,
    /// on the non-genesis branch. A state with another VRF output than the
    /// circuit computes, or another producer key than the stake proof's
    /// delegate, does not.
    #[test]
    #[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_offline_chain_block_step() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = provers();

        for bytes in [OFFLINE_548148, OFFLINE_548149] {
            let (outcome, aux_len) = check_constraints(&offline_input(bytes), &provers);
            assert_eq!(aux_len, StepBlockProof::AUX_LEN);
            assert_eq!(outcome, Outcome::Satisfied);
        }

        let mut other_vrf = offline_input(OFFLINE_548149);
        other_vrf
            .next_state
            .body
            .consensus_state
            .last_vrf_output
            .0
             .0[0] ^= 1;
        let (outcome, _) = check_constraints(&other_vrf, &provers);
        eprintln!("another VRF output: {outcome:?}");
        assert!(matches!(outcome, Outcome::NotSatisfied(_)), "{outcome:?}");

        let mut other_key = offline_input(OFFLINE_548149);
        other_key.prover_state.producer_private_key =
            v2::SignatureLibPrivateKeyStableV1(mina_p2p_messages::bigint::BigInt::one());
        let (outcome, _) = check_constraints(&other_key, &provers);
        eprintln!("another producer key: {outcome:?}");
        assert!(matches!(outcome, Outcome::NotSatisfied(_)), "{outcome:?}");
    }

    /// A real Mesa mainnet transition, 548414 to 548415, the first block that
    /// emits a ledger proof, as `mesa_real_transition_check` rebuilds the
    /// block prover's input: OCaml's proof of the parent, OCaml's ledger
    /// proof, the pending coinbase update and witness from a replay, and the
    /// stake winner's account in the staking epoch ledger. The producer's
    /// private key is unknown and is 1 in the fixture.
    const REAL_548415: &[u8] = include_bytes!("data/mesa_mainnet_548415_block_input.bin");

    const REAL_548415_HASH: &str = "3NKrjPJhwEjTw7dbvT4Yzjg7iuugAocLkQHW79Vuv82g3ngkdheZ";

    /// The failures `constraint_failures` lists after the first one.
    /// The failing rows, without the witness values each names after it.
    fn failures(outcome: &Outcome) -> Vec<String> {
        let Outcome::NotSatisfied(why) = outcome else {
            panic!("{outcome:?}");
        };
        let (_, list) = why.split_once(" failures: ").unwrap();
        list.split("; ")
            .map(|failure| match failure.split_once(" (aux ") {
                Some((row, _)) => row.to_owned(),
                None => failure.to_owned(),
            })
            .collect()
    }

    /// P4.6a: with the genesis winner's key in place of the real producer's,
    /// the real transition fails the block step exactly where the key
    /// matters: the key's public key against the winner's delegate (rows
    /// 4191 and 4640, both coordinates), the new state's hash, which commits
    /// to the VRF output the circuit computes with the key (rows 9418 and
    /// 34638), and the final assert (row 10390), as that output misses the
    /// winner's threshold. An offline block with a wrong key fails the same
    /// rows but the last: its winner's stake is large. Every other constraint
    /// holds for the real transition, the step's part of verifying OCaml's
    /// block proof and OCaml's ledger proof among them.
    #[test]
    #[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_real_transition_fails_only_where_the_key_matters() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = provers();

        let key_rows = [
            "row 4191 col 4 disconnected from row 4640 col 4",
            "row 4191 col 5 disconnected from row 4640 col 5",
            "row 4640 col 4 disconnected from row 4191 col 4",
            "row 4640 col 5 disconnected from row 4191 col 5",
            "row 9418 col 0 disconnected from row 34638 col 0",
            "row 34638 col 0 disconnected from row 9418 col 0",
        ];
        let mut real_rows = key_rows.to_vec();
        real_rows.insert(5, "row 10390 Generic gate: generic: incorrect gate");

        let mut real = offline_input(REAL_548415);
        assert_eq!(
            v2::StateHash::from_fp(state_hash(&real.next_state)).to_string(),
            REAL_548415_HASH
        );
        assert!(real.ledger_proof.is_some());
        real.prover_state.producer_private_key = genesis_input().prover_state.producer_private_key;
        let (outcome, aux_len) = check_constraints(&real, &provers);
        assert_eq!(aux_len, StepBlockProof::AUX_LEN);
        assert_eq!(failures(&outcome), real_rows);

        let mut offline = offline_input(OFFLINE_548149);
        offline.prover_state.producer_private_key =
            v2::SignatureLibPrivateKeyStableV1(mina_p2p_messages::bigint::BigInt::one());
        assert_eq!(failures(&check_constraints(&offline, &provers).0), key_rows);
    }

    /// P4.6a's gate: a non-genesis block, the offline chain's second, proves
    /// on its parent's proof (itself made by the Rust prover on the genesis
    /// proof), and the live mainnet blockchain key accepts the proof for the
    /// block's state, and for neither its parent's nor the state with another
    /// VRF output; the devnet key rejects it (P4.8). Proved on another
    /// previous proof, the genesis proof, the block's proof fails.
    #[test]
    #[ignore = "needs the Mesa circuit blobs: tools/mesa-circuits/generate.sh <dir>, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_offline_chain_block_proof_verifies() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = provers();
        let prove = |input: &v2::ProverExtendBlockchainInputStableV2| {
            let mut w = Witness::new::<StepBlockProof>();
            generate_block_proof(
                BlockParams {
                    input,
                    block_step_prover: &provers.block_step_prover,
                    block_wrap_prover: &provers.block_wrap_prover,
                    tx_wrap_prover: &provers.tx_wrap_prover,
                    only_verify_constraints: false,
                    expected_step_proof: None,
                    ocaml_wrap_witness: None,
                },
                &mut w,
            )
        };

        let input = offline_input(OFFLINE_548149);
        let proof = prove(&input).unwrap();
        assert!(verifies(&input.next_state, &proof));
        assert!(!other_network_verifies(&input.next_state, &proof));
        assert!(!verifies(&input.chain.state, &proof));
        let mut other = input.next_state.clone();
        other.body.consensus_state.last_vrf_output.0 .0[0] ^= 1;
        assert!(!verifies(&other, &proof));

        let mut other_parent_proof = input.clone();
        other_parent_proof.chain.proof = offline_input(OFFLINE_548148).chain.proof;
        match prove(&other_parent_proof) {
            Ok(proof) => assert!(!verifies(&input.next_state, &proof)),
            Err(error) => eprintln!("on the genesis proof: {error:#}"),
        }
    }

    /// P4.8: three inputs of an offline chain from the Mesa devnet genesis,
    /// written by `tools/block-replay/examples/mesa_offline_chain.rs
    /// --network devnet`: the genesis block's, and the first two blocks'
    /// after it, which carry the user commands of the real devnet blocks at
    /// their heights, none in 545435 and a payment in 545436. As on mainnet
    /// (P4.6a), the genesis is Mesa devnet's but for the staking epoch
    /// ledger, where the genesis winner holds three times the total currency,
    /// and the private key in them is the genesis winner's, which is public.
    const DEVNET_OFFLINE_GENESIS: &[u8] =
        include_bytes!("data/mesa_devnet_offline_chain_genesis_input.bin");
    const DEVNET_OFFLINE_545435: &[u8] =
        include_bytes!("data/mesa_devnet_offline_chain_545435_input.bin");
    const DEVNET_OFFLINE_545436: &[u8] =
        include_bytes!("data/mesa_devnet_offline_chain_545436_input.bin");

    const DEVNET_OFFLINE_GENESIS_HASH: &str =
        "3NLrbxZLyvkxGH8xQh5voNtuZ63attA8ThozNfZQJrdRgY3Zp7W3";
    const DEVNET_OFFLINE_545435_HASH: &str = "3NK4us8Y8DcRdf9NP2EiET3L5PEhw1KcX5rtQjQtTvk6M1qhCYzM";
    const DEVNET_OFFLINE_545436_HASH: &str = "3NK9hhj2PCqXGJhf5QrNNZv8PnirKHNtaLyY1KgGC4YzMuU5oT9k";

    /// The Mesa devnet genesis state hash, which every real Mesa devnet block
    /// records (OCaml's; R13).
    const DEVNET_GENESIS_HASH: &str = "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ";
    /// The Mesa devnet staking epoch ledger at genesis, from OCaml's genesis
    /// config (`genesis_ledgers/mesa-devnet-without-ledgers.json`).
    const DEVNET_STAKING_LEDGER_HASH: &str = "jwbFn9V1eyDa5AgLkHAwsVffHSjLLhfw8WpKYM6bJiKXb5Uhbti";
    const DEVNET_STAKING_TOTAL_CURRENCY: u64 = 1_582_881_647_000_001_000;

    /// A real Mesa devnet transition, 545435 to 545436, the first block with
    /// a user command (a payment), as `mesa_real_transition_check --network
    /// devnet` rebuilds the block prover's input: OCaml's devnet proof of the
    /// parent, the pending coinbase update and witness from a replay, and the
    /// stake winner's account in the staking epoch ledger. The producer's
    /// private key is unknown and is 1 in the fixture.
    const DEVNET_REAL_545436: &[u8] = include_bytes!("data/mesa_devnet_545436_block_input.bin");

    const DEVNET_REAL_545435_HASH: &str = "3NL1GQ579hqk3tsp16when7jKfjEw6NfA9Ud3ssnqPAwj3anXT3s";
    const DEVNET_REAL_545436_HASH: &str = "3NL1QfL3x2witWb5zqqgRGuCC5TcM58Fdp5eypWAwiCgKqYyYKdH";

    /// Proves `input`, step and wrap, as the block producer does.
    fn prove(
        input: &v2::ProverExtendBlockchainInputStableV2,
        provers: &BlockProver,
    ) -> anyhow::Result<WrapProof> {
        let mut w = Witness::new::<StepBlockProof>();
        generate_block_proof(
            BlockParams {
                input,
                block_step_prover: &provers.block_step_prover,
                block_wrap_prover: &provers.block_wrap_prover,
                tx_wrap_prover: &provers.tx_wrap_prover,
                only_verify_constraints: false,
                expected_step_proof: None,
                ocaml_wrap_witness: None,
            },
            &mut w,
        )
    }

    /// P4.8: the devnet offline chain's genesis is the Mesa devnet genesis
    /// with another staking epoch ledger: with Mesa devnet's staking epoch
    /// ledger put back, its genesis state hashes to the one every real devnet
    /// block records. It is the base case with devnet's fork constants, and
    /// its two blocks are non-genesis transitions, as on mainnet.
    #[test]
    fn mesa_devnet_offline_chain_inputs_extend_the_chain() {
        let fork = mina_core::network::devnet::CONSTRAINT_CONSTANTS
            .fork
            .unwrap();
        let genesis_input = offline_input(DEVNET_OFFLINE_GENESIS);
        let first = offline_input(DEVNET_OFFLINE_545435);
        let second = offline_input(DEVNET_OFFLINE_545436);
        assert_eq!(first.chain.state, genesis_input.next_state);
        assert_eq!(second.chain.state, first.next_state);

        let (negative_one, genesis) = (&genesis_input.chain.state, &genesis_input.next_state);
        let genesis_hash = state_hash(genesis);
        assert_eq!(
            v2::StateHash::from_fp(genesis_hash).to_string(),
            DEVNET_OFFLINE_GENESIS_HASH
        );
        assert_eq!(
            genesis.previous_state_hash.to_field::<Fp>().unwrap(),
            fork.state_hash
        );
        assert_eq!(
            genesis.body.genesis_state_hash.to_field::<Fp>().unwrap(),
            state_hash(negative_one)
        );
        let consensus = &genesis.body.consensus_state;
        assert_eq!(
            consensus.blockchain_length.as_u32(),
            fork.blockchain_length + 1
        );
        assert_eq!(
            consensus.global_slot_since_genesis.as_u32(),
            fork.global_slot_since_genesis
        );
        assert_eq!(genesis_input.block.consensus_transition.as_u32(), 0);
        assert!(genesis_input.ledger_proof.is_none());
        assert_eq!(
            genesis_input.prover_state.delegator_pk,
            consensus.block_stake_winner
        );

        // Mesa devnet's staking epoch ledger back in the negative-one and
        // genesis states, and the negative-one state's new hash where the
        // genesis state records it, give OCaml's devnet genesis.
        let real = offline_input(DEVNET_REAL_545436);
        let mesa_staking = v2::MinaBaseEpochLedgerValueStableV1 {
            hash: DEVNET_STAKING_LEDGER_HASH.parse().unwrap(),
            total_currency: v2::CurrencyAmountStableV1(DEVNET_STAKING_TOTAL_CURRENCY.into()),
        };
        let (mut negative_one, mut mesa_genesis) = (negative_one.clone(), genesis.clone());
        for state in [&mut negative_one, &mut mesa_genesis] {
            let staking = &mut state.body.consensus_state.staking_epoch_data;
            assert_ne!(staking.ledger, mesa_staking);
            staking.ledger = mesa_staking.clone();
        }
        let negative_one_hash = v2::StateHash::from_fp(state_hash(&negative_one));
        mesa_genesis.body.genesis_state_hash = negative_one_hash.clone();
        let next_epoch = &mut mesa_genesis.body.consensus_state.next_epoch_data;
        next_epoch.lock_checkpoint = negative_one_hash;
        let mesa_genesis_hash = v2::StateHash::from_fp(state_hash(&mesa_genesis));
        assert_eq!(mesa_genesis_hash.to_string(), DEVNET_GENESIS_HASH);
        assert_eq!(mesa_genesis_hash, real.next_state.body.genesis_state_hash);

        for (input, height, hash) in [
            (&first, 545435, DEVNET_OFFLINE_545435_HASH),
            (&second, 545436, DEVNET_OFFLINE_545436_HASH),
        ] {
            assert_offline_block(input, height, hash, genesis_hash);
        }
    }

    /// P4.8: the networks' block keys tell OCaml's block proofs apart, with
    /// no blobs: OCaml's proof of the real devnet block 545435 verifies with
    /// the embedded devnet blockchain key (the live one) and not with
    /// mainnet's, and OCaml's proof of the real mainnet block 548414 the
    /// other way round. Both keys are built without the global config.
    #[test]
    fn ocaml_block_proofs_verify_only_with_their_network_key() {
        let mainnet = block_verifier_index_of("mainnet").unwrap();
        let devnet = block_verifier_index_of("devnet").unwrap();

        let real = offline_input(DEVNET_REAL_545436);
        assert_eq!(
            v2::StateHash::from_fp(state_hash(&real.chain.state)).to_string(),
            DEVNET_REAL_545435_HASH
        );
        assert_eq!(
            v2::StateHash::from_fp(state_hash(&real.next_state)).to_string(),
            DEVNET_REAL_545436_HASH
        );
        assert_eq!(
            real.next_state.previous_state_hash.to_string(),
            DEVNET_REAL_545435_HASH
        );
        let mainnet_real = offline_input(REAL_548415);

        for (name, input, mainnet_ok) in [
            ("devnet 545435", &real, false),
            ("mainnet 548414", &mainnet_real, true),
        ] {
            let (state, proof) = (&input.chain.state, &*input.chain.proof);
            assert_eq!(
                verify_block_proof(state, proof, &mainnet),
                mainnet_ok,
                "{name}, mainnet key"
            );
            assert_eq!(
                verify_block_proof(state, proof, &devnet),
                !mainnet_ok,
                "{name}, devnet key"
            );
        }
    }

    /// P4.8: the devnet offline chain's genesis and both blocks' step
    /// witnesses satisfy the Mesa devnet block step (`4.0.2devnet`, devnet's
    /// fork constants). A state with another VRF output than the circuit
    /// computes does not.
    #[test]
    #[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
                tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_devnet_offline_chain_block_step() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = devnet_provers();

        for bytes in [
            DEVNET_OFFLINE_GENESIS,
            DEVNET_OFFLINE_545435,
            DEVNET_OFFLINE_545436,
        ] {
            let (outcome, aux_len) = check_constraints(&offline_input(bytes), &provers);
            assert_eq!(aux_len, StepBlockProof::AUX_LEN);
            assert_eq!(outcome, Outcome::Satisfied);
        }

        let mut other_vrf = offline_input(DEVNET_OFFLINE_545436);
        other_vrf
            .next_state
            .body
            .consensus_state
            .last_vrf_output
            .0
             .0[0] ^= 1;
        let (outcome, _) = check_constraints(&other_vrf, &provers);
        eprintln!("another VRF output: {outcome:?}");
        assert!(matches!(outcome, Outcome::NotSatisfied(_)), "{outcome:?}");
    }

    /// P4.8: the real devnet transition 545435 to 545436, with the genesis
    /// winner's key in place of the real producer's, fails the devnet block
    /// step at exactly the rows where the key matters, the rows mainnet's
    /// real transition fails (see
    /// `mesa_real_transition_fails_only_where_the_key_matters`). An offline
    /// devnet block with the key 1 fails the same seven rows: the threshold
    /// row too, as the VRF output of the key 1 for its slot misses even the
    /// offline winner's threshold (three quarters of the stake win about two
    /// slots in three; on mainnet's offline block it happened to win). Every
    /// other constraint holds for the real transition, the step's part of
    /// verifying OCaml's devnet proof of the parent among them.
    #[test]
    #[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
                tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_devnet_real_transition_fails_only_where_the_key_matters() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = devnet_provers();

        let key_rows = [
            "row 4191 col 4 disconnected from row 4640 col 4",
            "row 4191 col 5 disconnected from row 4640 col 5",
            "row 4640 col 4 disconnected from row 4191 col 4",
            "row 4640 col 5 disconnected from row 4191 col 5",
            "row 9418 col 0 disconnected from row 34638 col 0",
            "row 34638 col 0 disconnected from row 9418 col 0",
        ];
        let mut real_rows = key_rows.to_vec();
        real_rows.insert(5, "row 10390 Generic gate: generic: incorrect gate");

        let mut real = offline_input(DEVNET_REAL_545436);
        assert_eq!(
            v2::StateHash::from_fp(state_hash(&real.next_state)).to_string(),
            DEVNET_REAL_545436_HASH
        );
        real.prover_state.producer_private_key = offline_input(DEVNET_OFFLINE_GENESIS)
            .prover_state
            .producer_private_key;
        let (outcome, aux_len) = check_constraints(&real, &provers);
        assert_eq!(aux_len, StepBlockProof::AUX_LEN);
        assert_eq!(failures(&outcome), real_rows);

        let mut offline = offline_input(DEVNET_OFFLINE_545436);
        offline.prover_state.producer_private_key =
            v2::SignatureLibPrivateKeyStableV1(mina_p2p_messages::bigint::BigInt::one());
        assert_eq!(
            failures(&check_constraints(&offline, &provers).0),
            real_rows
        );
    }

    /// P4.8's gate for the devnet block circuits: the devnet offline chain's
    /// genesis block and its block 545436, which carries a real devnet
    /// payment, prove with the devnet circuits, the block on its parent's
    /// proof (made by the Rust prover on the genesis proof), and the live
    /// devnet blockchain key accepts each proof for its block's state, and
    /// not for its parent's or the state with another VRF output. The
    /// mainnet key rejects both.
    #[test]
    #[ignore = "needs the Mesa devnet circuit blobs: MINA_PROFILE=devnet \
                tools/mesa-circuits/generate.sh <dir> tools/mesa-circuits/mesa-devnet-fork.json, \
                then MINA_CIRCUIT_BLOBS_BASE_DIR=<dir>/circuit-blobs"]
    fn mesa_devnet_offline_chain_block_proof_verifies() {
        let _turn = PROVING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let provers = devnet_provers();
        assert_eq!(mina_core::NetworkConfig::global().name, "devnet");

        for bytes in [DEVNET_OFFLINE_GENESIS, DEVNET_OFFLINE_545436] {
            let input = offline_input(bytes);
            let proof = prove(&input, &provers).unwrap();
            assert!(verifies(&input.next_state, &proof));
            assert!(!other_network_verifies(&input.next_state, &proof));
            assert!(!verifies(&input.chain.state, &proof));
            let mut other = input.next_state.clone();
            other.body.consensus_state.last_vrf_output.0 .0[0] ^= 1;
            assert!(!verifies(&other, &proof));
        }
    }
}
