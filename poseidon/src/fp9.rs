//! Pasta Fp as 9 limbs of 29 bits in `u32`s, in Montgomery form with
//! R = 2^261, for the WASM fast path of the Poseidon permutation
//! ([`crate::perm9`]).
//!
//! WASM has no 64x64->128-bit multiply, so arkworks' 4x64-bit Montgomery
//! multiplication splits every product into four 32-bit ones and detects
//! carries with compares. With 29-bit limbs a product fits in 58 bits, a whole
//! column of products sums in a `u64` without carries, and the only multiply
//! is WASM's native `i64.mul`. In Chrome a multiplication takes 35 ns instead
//! of 72.
//!
//! The representation follows the "wasm-friendly field" of
//! o1-labs/proof-systems#2638 and the `32x9` field of openmina's arkworks 0.3
//! fork, which mina-rust used until 2025-09, with Pasta Fp's modulus folded
//! in: p = 2^254 + c with c < 2^126 and p = 1 mod 2^29, so the Montgomery
//! quotient digit is `-t mod 2^29`, limbs 5 to 7 of p are zero and limb 8 is
//! 2^22. Unlike the proof-systems PR, `reduce` propagates every column's
//! carry in full.
//!
//! Values move between this form and arkworks' with one multiplication each
//! way: arkworks' `Fp` holds `x * 2^256 mod p` in `Fp.0` (its own Montgomery
//! form, pinned by a test below), and this form is `x * 2^261 mod p`.

// Limb loops index several arrays in step.
#![allow(clippy::needless_range_loop)]

use ark_ff::BigInt;
use mina_curves::pasta::Fp;

const SHIFT: u32 = 29;
const MASK: u32 = (1 << SHIFT) - 1;
const SHIFT64: u64 = SHIFT as u64;
const MASK64: u64 = MASK as u64;

/// p = 0x40000000000000000000000000000000224698fc094cf91b992d30ed00000001
const MODULUS_64X4: [u64; 4] = [
    0x992d30ed00000001,
    0x224698fc094cf91b,
    0,
    0x4000000000000000,
];
const P: [u32; 9] = from_64x4(MODULUS_64X4);
const P64: [u64; 9] = {
    let mut r = [0u64; 9];
    let mut i = 0;
    while i < 9 {
        r[i] = P[i] as u64;
        i += 1;
    }
    r
};

// The modulus is arkworks' Pasta Fp modulus.
const _: () = {
    let m = <Fp as ark_ff::PrimeField>::MODULUS.0;
    let mut i = 0;
    while i < 4 {
        assert!(m[i] == MODULUS_64X4[i]);
        i += 1;
    }
};
// `reduce` relies on these: q * p[0] = q, and the zero limbs and the shift in
// limb 8 are constant-folded.
const _: () = assert!(P[0] == 1);
const _: () = assert!(P[5] == 0 && P[6] == 0 && P[7] == 0);
const _: () = assert!(P[8] == 1 << 22);

const fn from_64x4(pa: [u64; 4]) -> [u32; 9] {
    let mut p = [0u32; 9];
    p[0] = (pa[0] & MASK64) as u32;
    p[1] = ((pa[0] >> 29) & MASK64) as u32;
    p[2] = (((pa[0] >> 58) | (pa[1] << 6)) & MASK64) as u32;
    p[3] = ((pa[1] >> 23) & MASK64) as u32;
    p[4] = (((pa[1] >> 52) | (pa[2] << 12)) & MASK64) as u32;
    p[5] = ((pa[2] >> 17) & MASK64) as u32;
    p[6] = (((pa[2] >> 46) | (pa[3] << 18)) & MASK64) as u32;
    p[7] = ((pa[3] >> 11) & MASK64) as u32;
    p[8] = (pa[3] >> 40) as u32;
    p
}

const fn to_64x4(pa: [u32; 9]) -> [u64; 4] {
    let mut p = [0u64; 4];
    p[0] = pa[0] as u64;
    p[0] |= (pa[1] as u64) << 29;
    p[0] |= (pa[2] as u64) << 58;
    p[1] = (pa[2] as u64) >> 6;
    p[1] |= (pa[3] as u64) << 23;
    p[1] |= (pa[4] as u64) << 52;
    p[2] = (pa[4] as u64) >> 12;
    p[2] |= (pa[5] as u64) << 17;
    p[2] |= (pa[6] as u64) << 46;
    p[3] = (pa[6] as u64) >> 18;
    p[3] |= (pa[7] as u64) << 11;
    p[3] |= (pa[8] as u64) << 40;
    p
}

const fn geq(a: &[u64; 4], b: &[u64; 4]) -> bool {
    let mut i = 3;
    loop {
        if a[i] != b[i] {
            return a[i] > b[i];
        }
        if i == 0 {
            return true;
        }
        i -= 1;
    }
}

const fn sub(a: [u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let mut r = [0u64; 4];
    let mut borrow = 0u64;
    let mut i = 0;
    while i < 4 {
        let (d, b1) = a[i].overflowing_sub(b[i]);
        let (d, b2) = d.overflowing_sub(borrow);
        r[i] = d;
        borrow = (b1 | b2) as u64;
        i += 1;
    }
    r
}

/// 2^k mod p, by doubling (for compile-time constants).
const fn pow2_mod(k: u32) -> [u64; 4] {
    let mut x = [1, 0, 0, 0];
    let mut n = 0;
    while n < k {
        // p < 2^255, so doubling a value below p doesn't overflow.
        x = [
            x[0] << 1,
            (x[1] << 1) | (x[0] >> 63),
            (x[2] << 1) | (x[1] >> 63),
            (x[3] << 1) | (x[2] >> 63),
        ];
        if geq(&x, &MODULUS_64X4) {
            x = sub(x, &MODULUS_64X4);
        }
        n += 1;
    }
    x
}

/// A field element in Montgomery form, `x * 2^261 mod p`. Limbs 0 to 7 hold 29
/// bits each, limb 8 the rest.
///
/// Results of [`Fp9::mul`] and [`Fp9::add`] are canonical (below p). Those of
/// the `_nr` operations and of [`reduce`] are only "almost reduced": callers
/// keep track of their bound, below 2.1p in the permutation. R = 2^261 is 128
/// times p, so a Montgomery product of values below 2.1p is below 1.04p.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fp9(pub(crate) [u32; 9]);

#[inline(always)]
fn gte_modulus(x: &[u32; 9]) -> bool {
    let mut i = 8;
    loop {
        if x[i] != P[i] {
            return x[i] > P[i];
        }
        if i == 0 {
            return true;
        }
        i -= 1;
    }
}

/// Subtracts p once if x >= p.
#[inline(always)]
fn conditional_reduce(x: &mut [u32; 9]) {
    if gte_modulus(x) {
        let mut borrow: i32 = 0;
        for i in 0..9 {
            let t = (x[i] as i32).wrapping_sub(P[i] as i32).wrapping_add(borrow);
            borrow = t >> SHIFT;
            x[i] = (t as u32) & MASK;
        }
        // x >= p, so the final borrow is zero.
    }
}

/// Adds the columns of the 9x9-limb product x * y to `t`, without carrying.
/// Each product is below 2^58, so a column grows by less than 9 * 2^58.
#[inline(always)]
pub(crate) fn mul_wide(x: &[u32; 9], y: &[u32; 9], t: &mut [u64; 18]) {
    for i in 0..9 {
        let xi = x[i] as u64;
        for j in 0..9 {
            t[i + j] += xi * (y[j] as u64);
        }
    }
}

/// The columns of x^2, with each cross product computed once and doubled.
#[inline(always)]
fn square_wide(x: &[u32; 9], t: &mut [u64; 18]) {
    for i in 0..9 {
        let xi = x[i] as u64;
        t[2 * i] += xi * xi;
        let xi2 = xi << 1;
        for j in (i + 1)..9 {
            t[i + j] += xi2 * (x[j] as u64);
        }
    }
}

/// Montgomery reduction of an uncarried column sum: returns t / R mod p, a
/// value below t / R + p.
///
/// It adds less than 5 * 2^58 plus a carry to each column (the nonzero limbs of
/// p other than limb 0), so every column of `t` must be below 2^64 - 2^61 on
/// entry. The permutation's largest is below 27 * 2^58 + 2^29.
#[inline(always)]
pub(crate) fn reduce(t: &mut [u64; 18]) -> [u32; 9] {
    for i in 0..9 {
        let q = t[i].wrapping_neg() & MASK64;
        // p[0] = 1: (t[i] + q) is a multiple of 2^29.
        t[i + 1] += (t[i] + q) >> SHIFT64;
        for j in 1..9 {
            // Constant-folded: p[5..8] are zero, p[8] is a shift.
            t[i + j] += q * P64[j];
        }
    }
    let mut r = [0u32; 9];
    for k in 9..17 {
        r[k - 9] = (t[k] & MASK64) as u32;
        t[k + 1] += t[k] >> SHIFT64;
    }
    r[8] = t[17] as u32;
    r
}

impl Fp9 {
    /// Montgomery product, canonical for inputs below 2.1p.
    #[inline(always)]
    pub(crate) fn mul(&self, other: &Fp9) -> Fp9 {
        let mut r = self.mul_nr(other).0;
        conditional_reduce(&mut r);
        Fp9(r)
    }

    /// Montgomery product without the final conditional subtraction: below
    /// 1.04p for inputs below 2.1p.
    #[inline(always)]
    pub(crate) fn mul_nr(&self, other: &Fp9) -> Fp9 {
        let mut t = [0u64; 18];
        mul_wide(&self.0, &other.0, &mut t);
        Fp9(reduce(&mut t))
    }

    /// Square without the final conditional subtraction, as `mul_nr`.
    #[inline(always)]
    pub(crate) fn square_nr(&self) -> Fp9 {
        let mut t = [0u64; 18];
        square_wide(&self.0, &mut t);
        Fp9(reduce(&mut t))
    }

    /// Sum of two canonical values, canonical.
    #[inline(always)]
    pub(crate) fn add(&self, other: &Fp9) -> Fp9 {
        let mut r = [0u32; 9];
        let mut carry = 0u32;
        for i in 0..9 {
            let t = self.0[i] + other.0[i] + carry;
            carry = t >> SHIFT;
            r[i] = t & MASK;
        }
        // Limb 8 has room: the sum is below 2p < 2^256.
        r[8] += carry << SHIFT;
        conditional_reduce(&mut r);
        Fp9(r)
    }

    /// From arkworks' Montgomery form A = x * 2^256 to this one with one
    /// product: A * 2^266 / 2^261 = x * 2^261. Canonical.
    #[inline(always)]
    pub(crate) fn from_ark(x: &Fp) -> Fp9 {
        const C: Fp9 = Fp9(from_64x4(pow2_mod(266)));
        Fp9(from_64x4(x.0 .0)).mul(&C)
    }

    /// Back to arkworks' Montgomery form, for values below 2.1p:
    /// B * 2^256 / 2^261 = x * 2^256, canonical as arkworks requires.
    #[inline(always)]
    pub(crate) fn to_ark(self) -> Fp {
        const C: Fp9 = Fp9(from_64x4(pow2_mod(256)));
        Fp::new_unchecked(BigInt(to_64x4(self.mul(&C).0)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ark_ff::{AdditiveGroup, Field, PrimeField};

    #[cfg(target_arch = "wasm32")]
    use wasm_bindgen_test::wasm_bindgen_test as test;

    /// Deterministic pseudo-random field elements (xorshift64*), the same on
    /// every platform, so a failure can be reproduced natively.
    pub(crate) struct Rng(pub(crate) u64);

    impl Rng {
        fn next_u64(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545F4914F6CDD1D)
        }

        pub(crate) fn fp(&mut self) -> Fp {
            let mut bytes = [0u8; 32];
            for chunk in bytes.chunks_mut(8) {
                chunk.copy_from_slice(&self.next_u64().to_le_bytes());
            }
            Fp::from_le_bytes_mod_order(&bytes)
        }
    }

    /// Edge values, values next to limb and word boundaries, then random ones.
    pub(crate) fn values(n: usize) -> Vec<Fp> {
        let minus_one = -Fp::ONE;
        let mut values = vec![Fp::ZERO, Fp::ONE, minus_one, minus_one.double()];
        for k in [29u64, 58, 64, 116, 128, 232, 253] {
            let two_k = Fp::from(2u64).pow([k]);
            values.extend([two_k, two_k - Fp::ONE, -two_k]);
        }
        let mut rng = Rng(42);
        while values.len() < n {
            values.push(rng.fp());
        }
        values
    }

    fn canonical(x: &Fp9) -> Fp9 {
        let mut r = x.0;
        conditional_reduce(&mut r);
        conditional_reduce(&mut r);
        Fp9(r)
    }

    #[test]
    fn constants() {
        assert_eq!(to_64x4(P), MODULUS_64X4);
        assert_eq!(to_64x4(from_64x4([u64::MAX; 4])), [u64::MAX; 4]);
        // arkworks' Montgomery form, which `from_ark` and `to_ark` assume:
        // `Fp.0` holds x * 2^256 mod p.
        assert_eq!(Fp::ONE.0 .0, pow2_mod(256));
        assert_eq!(Fp::from(2u64).0 .0, pow2_mod(257));
        // 2^254 < p < 2^255, so R = 2^261 is between 64p and 128p...
        assert!(!geq(&[0, 0, 0, 1 << 62], &MODULUS_64X4));
        assert!(!geq(&MODULUS_64X4, &[0, 0, 0, 1 << 63]));
        // ...and one in this form is 2^261 mod p.
        assert_eq!(to_64x4(Fp9::from_ark(&Fp::ONE).0), pow2_mod(261));
    }

    #[test]
    fn arithmetic_matches_arkworks() {
        let values = values(2_000);
        for w in values.windows(2) {
            let (a, b) = (w[0], w[1]);
            let (a9, b9) = (Fp9::from_ark(&a), Fp9::from_ark(&b));
            assert_eq!(a9.to_ark(), a);
            assert!(!gte_modulus(&a9.0), "from_ark is canonical");
            assert_eq!(a9.mul(&b9).to_ark(), a * b);
            assert_eq!(canonical(&a9.mul_nr(&b9)), a9.mul(&b9));
            assert_eq!(a9.square_nr().to_ark(), a.square());
            assert_eq!(a9.add(&b9).to_ark(), a + b);
            // Almost reduced inputs, as inside the permutation: a + p.
            let mut ap = [0u64; 18];
            for i in 0..9 {
                ap[i] = (a9.0[i] + P[i]) as u64;
            }
            let ap = Fp9(reduce_carry(&ap));
            assert_eq!(ap.mul_nr(&b9).to_ark(), a * b);
            assert_eq!(ap.square_nr().to_ark(), a.square());
            assert_eq!(ap.to_ark(), a);
        }
    }

    /// Carries a 9-column sum below 2^256 into limbs (no reduction).
    fn reduce_carry(t: &[u64; 18]) -> [u32; 9] {
        let mut r = [0u32; 9];
        let mut carry = 0u64;
        for i in 0..8 {
            let s = t[i] + carry;
            r[i] = (s & MASK64) as u32;
            carry = s >> SHIFT64;
        }
        r[8] = (t[8] + carry) as u32;
        r
    }
}
