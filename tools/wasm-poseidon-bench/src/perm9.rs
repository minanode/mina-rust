//! The Kimchi Poseidon permutation (55 full rounds, width 3, x^7) over `Fp9`,
//! and the sponge of `poseidon::hash::hash_with_kimchi` on top of it.

use std::sync::OnceLock;

use mina_curves::pasta::Fp;
use poseidon::{PlonkSpongeConstantsKimchi, SpongeConstants};

use crate::fp9::{self, Fp9};

pub const ROUNDS: usize = PlonkSpongeConstantsKimchi::PERM_ROUNDS_FULL;

pub struct Params9 {
    pub round_constants: Vec<[Fp9; 3]>,
    pub mds: [[Fp9; 3]; 3],
}

pub fn params() -> &'static Params9 {
    static P: OnceLock<Params9> = OnceLock::new();
    P.get_or_init(|| {
        let p = poseidon::fp::params();
        Params9 {
            round_constants: p
                .round_constants
                .iter()
                .map(|rc| rc.map(|x| Fp9::from_ark(&x)))
                .collect(),
            mds: p.mds.map(|row| row.map(|x| Fp9::from_ark(&x))),
        }
    })
}

#[inline(always)]
fn sbox(x: Fp9) -> Fp9 {
    // x^7 the way `poseidon::sbox` does it: ((x^2 * x)^2) * x
    let x2 = x.square();
    let x3 = x2.mul(&x);
    let x6 = x3.square();
    x6.mul(&x)
}

#[inline(always)]
fn sbox_cios(x: Fp9) -> Fp9 {
    let x2 = x.square();
    let x3 = x2.mul_cios(&x);
    let x6 = x3.square();
    x6.mul_cios(&x)
}

/// Same structure as `poseidon::poseidon_block_cipher`: per round, S-box on
/// each element, MDS with 9 multiplications, then the round constants.
pub fn permute(params: &Params9, state: &mut [Fp9; 3]) {
    for r in 0..ROUNDS {
        let s = [sbox(state[0]), sbox(state[1]), sbox(state[2])];
        let rc = &params.round_constants[r];
        for i in 0..3 {
            let m = &params.mds[i];
            let acc = m[0].mul(&s[0]).add(&m[1].mul(&s[1])).add(&m[2].mul(&s[2]));
            state[i] = acc.add(&rc[i]);
        }
    }
}

/// As `permute`, with the interleaved multiplication.
pub fn permute_cios(params: &Params9, state: &mut [Fp9; 3]) {
    for r in 0..ROUNDS {
        let s = [
            sbox_cios(state[0]),
            sbox_cios(state[1]),
            sbox_cios(state[2]),
        ];
        let rc = &params.round_constants[r];
        for i in 0..3 {
            let m = &params.mds[i];
            let acc = m[0]
                .mul_cios(&s[0])
                .add(&m[1].mul_cios(&s[1]))
                .add(&m[2].mul_cios(&s[2]));
            state[i] = acc.add(&rc[i]);
        }
    }
}

/// Lazy reduction: each MDS row sums its three products as uncarried
/// columns, adds the round constant times R, and does one Montgomery
/// reduction. Three reductions per round instead of nine multiplications,
/// three additions and three constant additions.
///
/// Bounds: state below 2p, constants below p, so each column is below
/// 27 * 2^58 + 5 * 2^58 < 2^63, and the reduced value is below
/// (6p^2 + 2pR)/R < 2.05p, below 2p after one conditional subtraction.
pub fn permute_lazy(params: &Params9, state: &mut [Fp9; 3]) {
    for r in 0..ROUNDS {
        let s = [sbox(state[0]), sbox(state[1]), sbox(state[2])];
        let rc = &params.round_constants[r];
        for i in 0..3 {
            let m = &params.mds[i];
            let mut t = [0u64; 18];
            fp9::mul_wide(&m[0].0, &s[0].0, &mut t);
            fp9::mul_wide(&m[1].0, &s[1].0, &mut t);
            fp9::mul_wide(&m[2].0, &s[2].0, &mut t);
            for j in 0..9 {
                t[9 + j] += rc[i].0[j] as u64;
            }
            state[i] = Fp9(fp9::reduce(&mut t)).canonical();
        }
    }
    for x in state.iter_mut() {
        *x = x.canonical();
    }
}

#[inline(always)]
fn sbox_nr(x: Fp9) -> Fp9 {
    let x2 = x.square_nr();
    let x3 = x2.mul_nr(&x);
    let x6 = x3.square_nr();
    x6.mul_nr(&x)
}

/// `permute_lazy` without conditional subtractions inside the rounds: state
/// values stay below 2.03p (S-box outputs below 1.04p, MDS sums below
/// (3 * 1.04p * p + 2pR)/R), and are canonicalised once at the end.
pub fn permute_lazy_nr(params: &Params9, state: &mut [Fp9; 3]) {
    for r in 0..ROUNDS {
        let s = [sbox_nr(state[0]), sbox_nr(state[1]), sbox_nr(state[2])];
        let rc = &params.round_constants[r];
        for i in 0..3 {
            let m = &params.mds[i];
            let mut t = [0u64; 18];
            fp9::mul_wide(&m[0].0, &s[0].0, &mut t);
            fp9::mul_wide(&m[1].0, &s[1].0, &mut t);
            fp9::mul_wide(&m[2].0, &s[2].0, &mut t);
            for j in 0..9 {
                t[9 + j] += rc[i].0[j] as u64;
            }
            state[i] = Fp9(fp9::reduce(&mut t));
        }
    }
    for x in state.iter_mut() {
        *x = x.canonical().canonical();
    }
}

/// `hash_with_kimchi` for a `LazyParam`: those all come from absorbing one
/// field and squeezing, so the sponge starts in state `Squeezed(1)`.
pub fn hash_with_kimchi9(
    perm: fn(&Params9, &mut [Fp9; 3]),
    init: &[Fp9; 3],
    fields: &[Fp9],
) -> Fp9 {
    let params = params();
    let mut state = *init;
    // Squeezed(_): the first field goes to state[0], then Absorbed(1).
    let mut absorbed = 0usize;
    for x in fields {
        if absorbed == 2 {
            perm(params, &mut state);
            absorbed = 0;
        }
        state[absorbed] = state[absorbed].add(x);
        absorbed += 1;
    }
    perm(params, &mut state);
    state[0]
}

pub fn lazy_param_state9(param: &poseidon::hash::LazyParam) -> [Fp9; 3] {
    param.state().map(|x: Fp| Fp9::from_ark(&x))
}
