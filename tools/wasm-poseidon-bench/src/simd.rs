//! WASM SIMD128 versions of the `Fp9` arithmetic: two field elements per
//! vector, limb i of each in i32x4 lanes 0 and 1, multiplied with
//! `i64x2.extmul_low_i32x4_u` (two 32x32->64 products per instruction).
//!
//! No conditional subtractions inside the permutation: with R = 2^261 and
//! p < 2^255, Montgomery outputs stay below 1.04p for inputs below 2.1p, and
//! the lazy MDS sums stay below 2.03p, so values are canonicalised only at the
//! end (two conditional subtractions).

use core::arch::wasm32::*;

use crate::{
    fp9::{Fp9, MASK, P},
    perm9::{Params9, ROUNDS},
};

/// 9 limbs of two elements: `[a_i, b_i, 0, 0]` as u32x4.
type V9 = [v128; 9];
/// Uncarried product columns, two lanes of u64.
type W = [v128; 18];

const MASK64: u64 = MASK as u64;

/// `i64x2.extmul_low_i32x4_u`. Rust implements the intrinsic as
/// zero-extend + multiply; once LLVM shares an operand's extension between
/// several products (each limb is used in nine), it emits `i64x2.mul`, which
/// V8 emulates with three `pmuludq`. Volatile loads give every product its
/// own operands, so the `extmul` pattern survives instruction selection.
#[inline(always)]
fn extmul(a: &v128, b: &v128) -> v128 {
    unsafe { u64x2_extmul_low_u32x4(core::ptr::read_volatile(a), core::ptr::read_volatile(b)) }
}

#[inline(always)]
fn pack(a: &Fp9, b: &Fp9) -> V9 {
    core::array::from_fn(|i| u32x4(a.0[i], b.0[i], 0, 0))
}

#[inline(always)]
fn unpack(v: &V9) -> [Fp9; 2] {
    [
        Fp9(core::array::from_fn(|i| u32x4_extract_lane::<0>(v[i]))),
        Fp9(core::array::from_fn(|i| u32x4_extract_lane::<1>(v[i]))),
    ]
}

/// Both lanes set to lane `L` of `v`.
#[inline(always)]
fn splat_lane<const L: usize>(v: &V9) -> V9 {
    core::array::from_fn(|i| match L {
        0 => i32x4_shuffle::<0, 0, 0, 0>(v[i], v[i]),
        _ => i32x4_shuffle::<1, 1, 1, 1>(v[i], v[i]),
    })
}

#[inline(always)]
fn zero_w() -> W {
    [u64x2_splat(0); 18]
}

#[inline(always)]
fn mul_wide(x: &V9, y: &V9, t: &mut W) {
    for i in 0..9 {
        for j in 0..9 {
            t[i + j] = i64x2_add(t[i + j], extmul(&x[i], &y[j]));
        }
    }
}

#[inline(always)]
fn square_wide(x: &V9, t: &mut W) {
    for i in 0..9 {
        t[2 * i] = i64x2_add(t[2 * i], extmul(&x[i], &x[i]));
        // limbs are below 2^29, so doubling fits in 32 bits
        let xi2 = i32x4_shl(x[i], 1);
        for j in (i + 1)..9 {
            t[i + j] = i64x2_add(t[i + j], extmul(&xi2, &x[j]));
        }
    }
}

/// Montgomery reduction of both lanes; result limbs back in u32x4 lanes 0, 1.
#[inline(always)]
fn reduce(t: &mut W) -> V9 {
    let mask = u64x2_splat(MASK64);
    for i in 0..9 {
        let q = v128_and(i64x2_neg(t[i]), mask);
        // p[0] = 1
        t[i + 1] = i64x2_add(t[i + 1], u64x2_shr(i64x2_add(t[i], q), 29));
        let q32 = i32x4_shuffle::<0, 2, 0, 2>(q, q);
        for j in 1..5 {
            let pj = u32x4_splat(P[j]);
            t[i + j] = i64x2_add(t[i + j], extmul(&q32, &pj));
        }
        // p[5..8] = 0, p[8] = 2^22
        t[i + 8] = i64x2_add(t[i + 8], i64x2_shl(q, 22));
    }
    let mut r = [u64x2_splat(0); 9];
    for k in 9..17 {
        r[k - 9] = v128_and(t[k], mask);
        t[k + 1] = i64x2_add(t[k + 1], u64x2_shr(t[k], 29));
    }
    r[8] = t[17];
    r.map(|x| i32x4_shuffle::<0, 2, 0, 2>(x, x))
}

#[inline(always)]
fn mul(x: &V9, y: &V9) -> V9 {
    let mut t = zero_w();
    mul_wide(x, y, &mut t);
    reduce(&mut t)
}

#[inline(always)]
fn square(x: &V9) -> V9 {
    let mut t = zero_w();
    square_wide(x, &mut t);
    reduce(&mut t)
}

#[inline(always)]
fn sbox(x: &V9) -> V9 {
    let x2 = square(x);
    let x3 = mul(&x2, x);
    let x6 = square(&x3);
    mul(&x6, x)
}

/// Lane-wise Montgomery products, canonical.
pub fn mul2(a: &[Fp9; 2], b: &[Fp9; 2]) -> [Fp9; 2] {
    let r = mul(&pack(&a[0], &a[1]), &pack(&b[0], &b[1]));
    unpack(&r).map(|x| x.canonical().canonical())
}

struct SimdParams {
    /// Round constants of rows (0, 1) and (2, 2).
    rc01: Vec<V9>,
    rc22: Vec<V9>,
    /// MDS column j, rows (0, 1) and (2, 2).
    m01: [V9; 3],
    m22: [V9; 3],
    /// Two-state version: each constant in both lanes.
    rc_both: Vec<[V9; 3]>,
    m_both: [[V9; 3]; 3],
}

fn simd_params(params: &Params9) -> &'static SimdParams {
    use std::sync::OnceLock;
    static S: OnceLock<SimdParams> = OnceLock::new();
    S.get_or_init(|| {
        let m = &params.mds;
        SimdParams {
            rc01: params
                .round_constants
                .iter()
                .map(|rc| pack(&rc[0], &rc[1]))
                .collect(),
            rc22: params
                .round_constants
                .iter()
                .map(|rc| pack(&rc[2], &rc[2]))
                .collect(),
            m01: core::array::from_fn(|j| pack(&m[0][j], &m[1][j])),
            m22: core::array::from_fn(|j| pack(&m[2][j], &m[2][j])),
            rc_both: params
                .round_constants
                .iter()
                .map(|rc| rc.map(|c| pack(&c, &c)))
                .collect(),
            m_both: m.map(|row| row.map(|c| pack(&c, &c))),
        }
    })
}

#[inline(always)]
fn add_rc_times_r(t: &mut W, rc: &V9) {
    // + rc * R: limb j of rc goes to column 9 + j. Widen lanes 0, 1.
    for j in 0..9 {
        t[9 + j] = i64x2_add(t[9 + j], u64x2_extend_low_u32x4(rc[j]));
    }
}

/// One permutation, with the three S-boxes as two vector chains, (s0, s1)
/// and (s2, s2), and the MDS rows as (0, 1) and (2, 2).
pub fn permute(params: &Params9, state: &mut [Fp9; 3]) {
    let sp = simd_params(params);
    let mut a = pack(&state[0], &state[1]);
    let mut b = pack(&state[2], &state[2]);
    for r in 0..ROUNDS {
        let sa = sbox(&a);
        let sb = sbox(&b);
        let s0 = splat_lane::<0>(&sa);
        let s1 = splat_lane::<1>(&sa);
        let s = [&s0, &s1, &sb];
        let mut t01 = zero_w();
        let mut t22 = zero_w();
        for j in 0..3 {
            mul_wide(&sp.m01[j], s[j], &mut t01);
            mul_wide(&sp.m22[j], s[j], &mut t22);
        }
        add_rc_times_r(&mut t01, &sp.rc01[r]);
        add_rc_times_r(&mut t22, &sp.rc22[r]);
        a = reduce(&mut t01);
        b = reduce(&mut t22);
    }
    let [s0, s1] = unpack(&a);
    let [s2, _] = unpack(&b);
    *state = [s0, s1, s2].map(|x| x.canonical().canonical());
}

/// Two independent permutations, one per lane.
pub fn permute2(params: &Params9, states: &mut [[Fp9; 3]; 2]) {
    let sp = simd_params(params);
    let mut v: [V9; 3] = core::array::from_fn(|i| pack(&states[0][i], &states[1][i]));
    for r in 0..ROUNDS {
        let s = [sbox(&v[0]), sbox(&v[1]), sbox(&v[2])];
        for i in 0..3 {
            let mut t = zero_w();
            for j in 0..3 {
                mul_wide(&sp.m_both[i][j], &s[j], &mut t);
            }
            add_rc_times_r(&mut t, &sp.rc_both[r][i]);
            v[i] = reduce(&mut t);
        }
    }
    for i in 0..3 {
        let [x, y] = unpack(&v[i]);
        states[0][i] = x.canonical().canonical();
        states[1][i] = y.canonical().canonical();
    }
}

pub fn bench_mul2(n: u32) -> u32 {
    let mut rng = crate::Rng::new(1);
    let a = Fp9::from_ark(&rng.fp());
    let b = Fp9::from_ark(&rng.fp());
    let mut x = pack(&a, &b);
    let y = pack(&b, &a);
    for _ in 0..n {
        x = mul(&x, &y);
    }
    u32x4_extract_lane::<0>(x[0]) ^ u32x4_extract_lane::<1>(x[0])
}

pub fn bench_permutation(n: u32) -> u32 {
    let params = crate::perm9::params();
    let mut rng = crate::Rng::new(4);
    let mut state = [rng.fp(), rng.fp(), rng.fp()].map(|x| Fp9::from_ark(&x));
    for _ in 0..n {
        permute(params, &mut state);
    }
    state[0].0[0]
}

/// n permutations, done two at a time.
pub fn bench_permutation_pairs(n: u32) -> u32 {
    let params = crate::perm9::params();
    let mut rng = crate::Rng::new(4);
    let mut states: [[Fp9; 3]; 2] =
        core::array::from_fn(|_| [rng.fp(), rng.fp(), rng.fp()].map(|x| Fp9::from_ark(&x)));
    for _ in 0..n.div_ceil(2) {
        permute2(params, &mut states);
    }
    states[0][0].0[0] ^ states[1][0].0[0]
}
