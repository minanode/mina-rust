//! The kimchi prover this node uses, at the pinned proof-systems revision,
//! on range-check and foreign-field addition edge values: a value outside a
//! 12-bit limb of a multi-range check fails to prove, and subtractions that
//! borrow prove. The node's own circuits use neither gate; zkApp proofs that
//! do are made by their authors and only verified here.
//!
//! Run with: cargo test -p mina-tree --test test_kimchi_gate_edges

use kimchi::{
    circuits::{
        gate::{CircuitGate, Connect},
        polynomial::COLUMNS,
        polynomials::{
            foreign_field_add::witness::{self as ffadd, FFOps},
            generic::GenericGateSpec,
            range_check,
        },
        wires::Wire,
    },
    groupmap::GroupMap,
    proof::ProverProof,
    prover_index::{testing::new_index_for_test_with_lookups, ProverIndex},
    verifier::verify,
};
use mina_curves::pasta::{Fp, Vesta, VestaParameters};
use mina_poseidon::{
    constants::PlonkSpongeConstantsKimchi,
    pasta::FULL_ROUNDS,
    sponge::{DefaultFqSponge, DefaultFrSponge},
};
use num_bigint::BigUint;
use poly_commitment::{
    commitment::CommitmentCurve,
    ipa::{OpeningProof, SRS},
};

type BaseSponge = DefaultFqSponge<VestaParameters, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
type ScalarSponge = DefaultFrSponge<Fp, PlonkSpongeConstantsKimchi, FULL_ROUNDS>;
type Index = ProverIndex<FULL_ROUNDS, Vesta, SRS<Vesta>>;
type Witness = [Vec<Fp>; COLUMNS];

/// With every gate checked: kimchi's `disable_gates_checks` zeroes the gate
/// selectors in debug builds, so the gates would constrain nothing.
fn index(gates: Vec<CircuitGate<Fp>>, public: usize) -> Index {
    new_index_for_test_with_lookups::<FULL_ROUNDS, Vesta>(
        gates,
        public,
        0,
        vec![],
        None,
        false,
        None,
        false,
    )
}

/// Proves and verifies; the prover's or the verifier's error otherwise.
fn prove_and_verify(index: &Index, witness: Witness) -> Result<(), String> {
    let group_map = <Vesta as CommitmentCurve>::Map::setup();
    let public_input = witness[0][0..index.cs.public].to_vec();
    let proof = ProverProof::create::<BaseSponge, ScalarSponge, _>(
        &group_map,
        witness,
        &[],
        index,
        &mut rand::rngs::OsRng,
    )
    .map_err(|error| format!("prover: {error}"))?;
    verify::<FULL_ROUNDS, Vesta, BaseSponge, ScalarSponge, OpeningProof<Vesta, FULL_ROUNDS>>(
        &group_map,
        &index.verifier_index(),
        &proof,
        &public_input,
    )
    .map_err(|error| format!("verifier: {error}"))
}

/// The reason no proof verifies: the prover's or the verifier's error, or
/// the prover's panic on a witness that breaks a constraint, which it checks
/// first in builds with debug assertions.
fn fails_to_prove(index: &Index, witness: Witness) -> String {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prove_and_verify(index, witness)
    }));
    match result {
        Ok(Ok(())) => panic!("a proof verifies"),
        Ok(Err(error)) => error,
        Err(panic) => panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
            .map(|panic| format!("prover panic: {panic}"))
            .unwrap_or_else(|| "prover panic".to_string()),
    }
}

fn two_to(bits: u64) -> Fp {
    ark_ff::Field::pow(&Fp::from(2u64), [bits])
}

fn multi_range_check_index() -> Index {
    let (_, gates) = CircuitGate::<Fp>::create_multi_range_check(0);
    index(gates, 0)
}

/// The largest values a multi-range check takes, 88 bits each, prove.
#[test]
fn a_multi_range_check_proves_88_bit_values() {
    let max = two_to(88) - Fp::from(1u64);
    let witness = range_check::witness::create_multi(max, max, max);
    let index = multi_range_check_index();
    index.verify(&witness, &[]).unwrap();
    prove_and_verify(&index, witness).unwrap();
}

/// Moves 2^12 from limb `(row, col)` of a 12-bit lookup into the limb
/// `(row, col_below)` 12 bits lower: the value is the same, so every
/// constraint holds, but the lower limb no longer fits in 12 bits. `copies`
/// are the cells that copy the two limbs, in the same order.
fn overfill(
    witness: &mut Witness,
    (row, col): (usize, usize),
    col_below: usize,
    copies: Option<((usize, usize), (usize, usize))>,
) {
    let carry = two_to(12);
    witness[col][row] -= Fp::from(1u64);
    witness[col_below][row] += carry;
    if let Some(((row_a, col_a), (row_b, col_b))) = copies {
        witness[col_a][row_a] -= Fp::from(1u64);
        witness[col_b][row_b] += carry;
    }
}

/// A limb of 13 bits or more in the top 12-bit slices of a value fails to
/// prove, with every constraint holding: only the lookup catches it. The
/// top two 12-bit limbs of `v0` and `v1` (columns 1 and 2 of their rows)
/// are copied into row 3, where they are looked up; the top 12-bit limb of
/// `v2` (row 2, column 3) is looked up in its own row.
#[test]
fn a_top_limb_past_12_bits_fails_to_prove() {
    let max = two_to(88) - Fp::from(1u64);
    let honest = range_check::witness::create_multi(max, max, max);
    let index = multi_range_check_index();
    // (row, limb), the limb 12 bits below it, and their copies in row 3.
    let cases = [
        ((0, 1), 2, Some(((3, 3), (3, 4)))),
        ((1, 1), 2, Some(((3, 5), (3, 6)))),
        // v2: the 2-bit crumb of bits 86..88 into the 12-bit limb below.
        ((2, 2), 3, None),
    ];
    for (limb, below, copies) in cases {
        let mut witness = honest.clone();
        overfill(&mut witness, limb, below, copies);
        index
            .verify(&witness, &[])
            .unwrap_or_else(|error| panic!("{limb:?}: a constraint fails: {error:?}"));
        let error = fails_to_prove(&index, witness);
        assert!(error.contains("lookup"), "{limb:?}: {error}");
    }
}

/// A value of 88 bits or more doesn't fit the decomposition: a constraint
/// fails and no proof verifies.
#[test]
fn a_value_past_88_bits_fails_to_prove() {
    let index = multi_range_check_index();
    for value in [two_to(88), two_to(88) + Fp::from(1u64), two_to(200)] {
        let zero = Fp::from(0u64);
        for witness in [
            range_check::witness::create_multi(value, zero, zero),
            range_check::witness::create_multi(zero, value, zero),
            range_check::witness::create_multi(zero, zero, value),
        ] {
            assert!(index.verify(&witness, &[]).is_err());
            fails_to_prove(&index, witness);
        }
    }
}

fn secp256k1_modulus() -> BigUint {
    BigUint::parse_bytes(
        b"fffffffffffffffffffffffffffffffffffffffffffffffffffffffefffffc2f",
        16,
    )
    .unwrap()
}

/// One foreign-field operation with every input, its result and the bound
/// range-checked, as kimchi's own `full_circuit` lays it out: a public row
/// holding 1 (row 0), the operation (1), the bound (2, then a zero row),
/// then four multi-range checks from row 4.
fn ffadd_circuit(op: FFOps, modulus: &BigUint) -> Vec<CircuitGate<Fp>> {
    let mut gates = vec![CircuitGate::<Fp>::create_generic_gadget(
        Wire::for_row(0),
        GenericGateSpec::Pub,
        None,
    )];
    let mut row = 1;
    CircuitGate::<Fp>::extend_chain_ffadd(&mut gates, 0, &mut row, &[op], modulus);
    for _ in 0..4 {
        CircuitGate::extend_multi_range_check(&mut gates, &mut row);
    }
    // Left input, right input and result of the operation; the bound.
    gates.connect_ffadd_range_checks(1, Some(4), Some(8), 12);
    gates.connect_ffadd_range_checks(2, None, None, 16);
    gates
}

fn ffadd_witness(inputs: &[BigUint], op: FFOps, modulus: &BigUint) -> Witness {
    let mut witness: Witness = std::array::from_fn(|_| vec![Fp::from(0u64)]);
    witness[0][0] = Fp::from(1u64);
    let chain = ffadd::create_chain::<Fp>(inputs, &[op], modulus.clone());
    for col in 0..COLUMNS {
        witness[col].extend(chain[col].iter());
    }
    let limbs = |witness: &Witness, row: usize, first: usize| {
        (
            witness[first][row],
            witness[first + 1][row],
            witness[first + 2][row],
        )
    };
    for (row, first) in [(1, 0), (1, 3), (2, 0), (3, 0)] {
        let (lo, mi, hi) = limbs(&witness, row, first);
        range_check::witness::extend_multi(&mut witness, lo, mi, hi);
    }
    witness
}

/// Subtractions whose result is negative before reduction, so that need
/// a borrow, at the bottom, across limbs and at the top of the field,
/// prove and verify; so do additions that carry.
#[test]
fn foreign_field_subtractions_that_borrow_prove() {
    let modulus = secp256k1_modulus();
    let one = BigUint::from(1u32);
    let limb: BigUint = BigUint::from(1u32) << 88u32;
    let top = &modulus - &one;
    let cases = [
        // 0 - 1 = p - 1.
        (BigUint::from(0u32), one.clone(), FFOps::Sub),
        (one.clone(), BigUint::from(2u32), FFOps::Sub),
        // A borrow out of the low limb and out of the middle limb.
        (limb.clone(), &limb + &one, FFOps::Sub),
        (&limb * &limb, &limb * &limb + &one, FFOps::Sub),
        // From the bottom of the field to its top, and back.
        (BigUint::from(0u32), top.clone(), FFOps::Sub),
        (top.clone(), top.clone(), FFOps::Sub),
        (top.clone(), one.clone(), FFOps::Add),
        (top.clone(), top.clone(), FFOps::Add),
    ];
    for (left, right, op) in cases {
        let gates = ffadd_circuit(op, &modulus);
        let index = index(gates, 1);
        let witness = ffadd_witness(&[left.clone(), right.clone()], op, &modulus);
        index
            .verify(&witness, &witness[0][0..1])
            .unwrap_or_else(|error| panic!("{left} {op:?} {right}: {error:?}"));
        prove_and_verify(&index, witness)
            .unwrap_or_else(|error| panic!("{left} {op:?} {right}: {error}"));
    }
}
