//! The Poseidon permutation of [`crate::poseidon_block_cipher`] on 9x29-bit
//! limbs ([`crate::fp9`]), which that function uses in WASM for Pasta Fp with
//! this crate's own parameters: the Kimchi parameters (55 rounds, x^7) behind
//! `hash::hash_with_kimchi`, and the legacy ones (63 rounds, x^5, an initial
//! round of constants) behind `hash::legacy::hash_with_kimchi`.
//!
//! In Chrome a permutation takes 36 us this way instead of 94 us with arkworks
//!. Natively arkworks' 64-bit
//! limbs are faster, so this is only used in WASM.
//!
//! Beyond the faster multiplication, each round does its MDS product lazily:
//! a row sums its three products and the round constant times R as uncarried
//! columns and does one Montgomery reduction, so a round costs 3 reductions
//! instead of 9 multiplications and 6 additions. Nothing is canonicalised
//! inside the rounds: S-box outputs stay below 1.04p and state elements below
//! 2.03p (see [`permute`]), and the state is brought below p once, on the way
//! back to arkworks.

use std::any::Any;

use ark_ff::Field;
use mina_curves::pasta::Fp;
use once_cell::sync::Lazy;

use crate::{
    fp9::{self, Fp9},
    SpongeConstants, SpongeParams,
};

/// A parameter set converted to [`Fp9`], all canonical.
struct Params9 {
    round_constants: Box<[[Fp9; 3]]>,
    mds: [[Fp9; 3]; 3],
}

impl Params9 {
    fn new(params: &SpongeParams<Fp>) -> Self {
        Self {
            round_constants: params
                .round_constants
                .iter()
                .map(|rc| rc.map(|x| Fp9::from_ark(&x)))
                .collect(),
            mds: params.mds.map(|row| row.map(|x| Fp9::from_ark(&x))),
        }
    }

    /// The converted copy of `params` if it is one of this crate's own Fp
    /// parameter sets, whose addresses are fixed.
    fn of(params: &SpongeParams<Fp>) -> Option<&'static Self> {
        static KIMCHI: Lazy<Params9> = Lazy::new(|| Params9::new(crate::fp::params()));
        static LEGACY: Lazy<Params9> = Lazy::new(|| Params9::new(crate::fp_legacy::params()));

        if std::ptr::eq(params, crate::fp::params()) {
            Some(&KIMCHI)
        } else if std::ptr::eq(params, crate::fp_legacy::params()) {
            Some(&LEGACY)
        } else {
            None
        }
    }
}

/// Runs the permutation on 9x29-bit limbs if it can: `F` is Pasta Fp,
/// `params` is one of this crate's Fp parameter sets, and the S-box is x^7 or
/// x^5. Returns false, leaving `state` alone, otherwise. The result is the
/// same field element as arkworks' (`tests/fast_path.rs` compares them).
///
/// The type checks are on `'static` types (`Field: 'static`), so after
/// monomorphisation they are constants.
#[inline]
pub(crate) fn try_permute<F: Field, SC: SpongeConstants>(
    params: &SpongeParams<F>,
    state: &mut [F; 3],
) -> bool {
    let params: &dyn Any = params;
    let state: &mut dyn Any = state;
    let (Some(params), Some(state)) = (
        params.downcast_ref::<SpongeParams<Fp>>(),
        state.downcast_mut::<[Fp; 3]>(),
    ) else {
        return false;
    };
    let Some(params9) = Params9::of(params) else {
        return false;
    };
    let first = usize::from(SC::PERM_INITIAL_ARK);
    // The arkworks path panics on too few constants: leave that case to it.
    if params9.round_constants.len() < first + SC::PERM_ROUNDS_FULL {
        return false;
    }
    match SC::PERM_SBOX {
        7 => permute::<7>(params9, state, SC::PERM_INITIAL_ARK, SC::PERM_ROUNDS_FULL),
        5 => permute::<5>(params9, state, SC::PERM_INITIAL_ARK, SC::PERM_ROUNDS_FULL),
        _ => return false,
    }
    true
}

/// x^SBOX for x below 2.1p, below 1.04p.
#[inline(always)]
fn sbox<const SBOX: u32>(x: Fp9) -> Fp9 {
    match SBOX {
        // ((x^2 * x)^2) * x, as `crate::sbox`.
        7 => x.square_nr().mul_nr(&x).square_nr().mul_nr(&x),
        // (x^2)^2 * x.
        5 => x.square_nr().square_nr().mul_nr(&x),
        _ => unreachable!("try_permute only passes 5 and 7"),
    }
}

/// [`crate::poseidon_block_cipher`]: an optional initial round of constants
/// (round 0), then `rounds` full rounds of S-box, MDS and round constants.
///
/// Bounds, with R = 2^261 > 128p: inputs and constants are canonical, S-box
/// outputs below 1.04p, so an MDS row's columns are below 27 * 2^58 + 2^29 and
/// its value (3 * p * 1.04p + p * R) / R + p is below 2.03p.
fn permute<const SBOX: u32>(
    params: &Params9,
    state: &mut [Fp; 3],
    initial_ark: bool,
    rounds: usize,
) {
    let mut s = state.map(|x| Fp9::from_ark(&x));
    let mut first = 0;
    if initial_ark {
        let rc = &params.round_constants[0];
        s = [s[0].add(&rc[0]), s[1].add(&rc[1]), s[2].add(&rc[2])];
        first = 1;
    }
    for rc in &params.round_constants[first..first + rounds] {
        let x = [sbox::<SBOX>(s[0]), sbox::<SBOX>(s[1]), sbox::<SBOX>(s[2])];
        for i in 0..3 {
            let m = &params.mds[i];
            let mut t = [0u64; 18];
            fp9::mul_wide(&m[0].0, &x[0].0, &mut t);
            fp9::mul_wide(&m[1].0, &x[1].0, &mut t);
            fp9::mul_wide(&m[2].0, &x[2].0, &mut t);
            // + rc * R, which the reduction turns into + rc.
            for j in 0..9 {
                t[9 + j] += rc[i].0[j] as u64;
            }
            s[i] = Fp9(fp9::reduce(&mut t));
        }
    }
    *state = s.map(|x| x.to_ark());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fp9::tests::{values, Rng},
        PlonkSpongeConstantsKimchi, PlonkSpongeConstantsLegacy,
    };
    use mina_curves::pasta::Fq;

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// The arkworks permutation, `poseidon_block_cipher` without its fast
    /// path.
    fn ark_permute<F: Field, SC: SpongeConstants>(params: &SpongeParams<F>, state: &mut [F; 3]) {
        if SC::PERM_INITIAL_ARK {
            for (i, x) in params.round_constants[0].iter().enumerate() {
                state[i].add_assign(x);
            }
            for r in 0..SC::PERM_ROUNDS_FULL {
                crate::full_round::<F, SC>(params, state, r + 1);
            }
        } else {
            for r in 0..SC::PERM_ROUNDS_FULL {
                crate::full_round::<F, SC>(params, state, r);
            }
        }
    }

    fn check<SC: SpongeConstants>(params: &SpongeParams<Fp>, states: &[[Fp; 3]]) {
        for state in states {
            let mut expected = *state;
            ark_permute::<Fp, SC>(params, &mut expected);
            let mut fast = *state;
            assert!(try_permute::<Fp, SC>(params, &mut fast));
            assert_eq!(fast, expected, "from {state:?}");
        }
    }

    fn states(n: usize) -> Vec<[Fp; 3]> {
        let values = values(n);
        let mut rng = Rng(7);
        let mut states: Vec<[Fp; 3]> = values.iter().map(|&v| [v, v, v]).collect();
        states.extend((0..n).map(|_| [rng.fp(), rng.fp(), rng.fp()]));
        states
    }

    #[test]
    fn kimchi_permutation_matches_arkworks() {
        check::<PlonkSpongeConstantsKimchi>(crate::fp::params(), &states(1_000));
    }

    #[test]
    fn legacy_permutation_matches_arkworks() {
        check::<PlonkSpongeConstantsLegacy>(crate::fp_legacy::params(), &states(1_000));
    }

    #[test]
    fn other_parameters_are_left_to_arkworks() {
        let mut state = [Fp::from(1u64), Fp::from(2u64), Fp::from(3u64)];
        let before = state;

        // Not one of the crate's parameter sets, though equal to one.
        let copy = SpongeParams {
            round_constants: crate::fp::params().round_constants.clone(),
            mds: crate::fp::params().mds,
        };
        assert!(!try_permute::<Fp, PlonkSpongeConstantsKimchi>(
            &copy, &mut state
        ));

        // An S-box this path doesn't have.
        #[derive(Clone)]
        struct Cube;
        impl SpongeConstants for Cube {
            const PERM_ROUNDS_FULL: usize = 55;
            const PERM_ROUNDS_PARTIAL: usize = 0;
            const PERM_HALF_ROUNDS_FULL: usize = 0;
            const PERM_SBOX: u32 = 3;
            const PERM_FULL_MDS: bool = true;
            const PERM_INITIAL_ARK: bool = false;
        }
        assert!(!try_permute::<Fp, Cube>(crate::fp::params(), &mut state));

        // More rounds than constants: arkworks panics, and so must the caller.
        #[derive(Clone)]
        struct TooLong;
        impl SpongeConstants for TooLong {
            const PERM_ROUNDS_FULL: usize = 63;
            const PERM_ROUNDS_PARTIAL: usize = 0;
            const PERM_HALF_ROUNDS_FULL: usize = 0;
            const PERM_SBOX: u32 = 5;
            const PERM_FULL_MDS: bool = true;
            const PERM_INITIAL_ARK: bool = true;
        }
        assert!(!try_permute::<Fp, TooLong>(crate::fp::params(), &mut state));
        assert_eq!(state, before);

        // Fq.
        let mut fq = [Fq::from(1u64), Fq::from(2u64), Fq::from(3u64)];
        assert!(!try_permute::<Fq, PlonkSpongeConstantsKimchi>(
            crate::fq::params(),
            &mut fq
        ));
        assert_eq!(fq, [Fq::from(1u64), Fq::from(2u64), Fq::from(3u64)]);
    }

    /// The legacy constants with the Kimchi rounds, and the reverse: the
    /// fast path follows the constants of `SC`, not the parameter set.
    #[test]
    fn rounds_follow_the_sponge_constants() {
        let states = states(10);
        check::<PlonkSpongeConstantsKimchi>(crate::fp_legacy::params(), &states);
        #[derive(Clone)]
        struct LegacyShort;
        impl SpongeConstants for LegacyShort {
            const PERM_ROUNDS_FULL: usize = 54;
            const PERM_ROUNDS_PARTIAL: usize = 0;
            const PERM_HALF_ROUNDS_FULL: usize = 0;
            const PERM_SBOX: u32 = 5;
            const PERM_FULL_MDS: bool = true;
            const PERM_INITIAL_ARK: bool = true;
        }
        check::<LegacyShort>(crate::fp::params(), &states);
    }
}
