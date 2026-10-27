/// Copy-pasted from https://github.com/MinaProtocol/mina/blob/cf2a732ae39f4e784707e1fc32832da805bb7d09/src/lib/crypto/kimchi_bindings/stubs/src/urs_utils.rs
use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::{One, UniformRand, Zero};
use poly_commitment::{
    commitment::{b_poly_coefficients, CommitmentCurve},
    ipa::SRS,
};
use rayon::prelude::*;

// TODO: Not compatible with variable rounds
pub fn batch_dlog_accumulator_check<G>(urs: &SRS<G>, comms: &[G], chals: &[G::ScalarField]) -> bool
where
    G: CommitmentCurve + AffineRepr,
{
    let k = comms.len();

    if k == 0 {
        assert_eq!(chals.len(), 0);
        return true;
    }

    let rounds = chals.len() / k;
    assert_eq!(chals.len() % rounds, 0);

    let rs = {
        let r = G::ScalarField::rand(&mut rand::rngs::OsRng);
        let mut rs = vec![G::ScalarField::one(); k];
        for i in 1..k {
            rs[i] = r * rs[i - 1];
        }
        rs
    };

    let mut points = urs.g.clone();
    let n = points.len();
    points.extend(comms);

    let mut scalars = vec![G::ScalarField::zero(); n];
    scalars.extend(&rs[..]);

    // OCaml's batch accumulator check sums r^i * b_poly(challenges_i).
    // Keep one partial sum per worker instead of all k full polynomials:
    // for 128 proofs the old collection held 128 * SRS_DEPTH field elements.
    // Field addition and multiplication are exact, so grouping the sum does
    // not change the MSM inputs or verification result.
    let workers = rayon::current_num_threads().min(k);
    let group_size = k.div_ceil(workers);
    let partials: Vec<_> = rs
        .par_chunks(group_size)
        .enumerate()
        .map(|(group, weights)| {
            let mut partial = vec![G::ScalarField::zero(); n];
            for (offset, weight) in weights.iter().enumerate() {
                let proof = group * group_size + offset;
                let challenges = &chals[proof * rounds..(proof + 1) * rounds];
                let terms = b_poly_coefficients(challenges);
                assert_eq!(terms.len(), n);
                for (sum, term) in partial.iter_mut().zip(terms) {
                    *sum += term * weight;
                }
            }
            partial
        })
        .collect();

    for partial in partials {
        for (scalar, sum) in scalars.iter_mut().zip(partial) {
            *scalar -= sum;
        }
    }

    <G::Group as VariableBaseMSM>::msm(&points, &scalars).unwrap() == G::Group::zero()
}

#[allow(unused)]
pub fn batch_dlog_accumulator_generate<G: CommitmentCurve>(
    urs: &SRS<G>,
    num_comms: usize,
    chals: &Vec<G::ScalarField>,
) -> Vec<G> {
    let k = num_comms;

    if k == 0 {
        assert_eq!(chals.len(), 0);
        return vec![];
    }

    let rounds = chals.len() / k;
    assert_eq!(chals.len() % rounds, 0);

    let comms: Vec<_> = chals
        .into_par_iter()
        .chunks(rounds)
        .map(|chals| {
            let chals: Vec<G::ScalarField> = chals.into_iter().copied().collect();
            let scalars: Vec<_> = b_poly_coefficients(&chals);
            let points: Vec<_> = urs.g.clone();
            <G::Group as VariableBaseMSM>::msm(&points, &scalars)
                .unwrap() // Not sure have safe this unwrap is but function is unused
                .into_affine()
        })
        .collect();

    comms
}
