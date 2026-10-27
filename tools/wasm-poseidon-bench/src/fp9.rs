//! Pasta Fp (the field `poseidon::hash` works in) as 9 limbs of 29 bits in
//! `u32`s, in Montgomery form with R = 2^261.
//!
//! The point of the representation: a 29x29-bit product fits in 58 bits, so a
//! whole column of products can be summed in a `u64` without carries, and the
//! only multiply needed is WASM's native `i64.mul`. The 4x64-bit
//! representation of arkworks needs 64x64->128-bit products, which WASM
//! doesn't have.
//!
//! This follows the "wasm-friendly field" work for o1js/proof-systems
//! (o1-labs/proof-systems#2634, #2638) and the `32x9` field of the old
//! openmina arkworks 0.3 fork, adapted to Pasta Fp with its constant modulus
//! folded in: p = 2^254 + c with c < 2^126, so limbs 5, 6 and 7 of p are zero
//! and limb 8 is 2^22.

use ark_ff::{BigInteger256, PrimeField};
use mina_curves::pasta::Fp;

pub const SHIFT: u32 = 29;
pub const MASK: u32 = (1 << SHIFT) - 1;
const SHIFT64: u64 = SHIFT as u64;
const MASK64: u64 = MASK as u64;

/// p = 0x40000000000000000000000000000000224698fc094cf91b992d30ed00000001
pub const MODULUS_64X4: [u64; 4] = [
    0x992d30ed00000001,
    0x224698fc094cf91b,
    0,
    0x4000000000000000,
];
pub const P: [u32; 9] = from_64x4(MODULUS_64X4);
const P64: [u64; 9] = {
    let mut r = [0u64; 9];
    let mut i = 0;
    while i < 9 {
        r[i] = P[i] as u64;
        i += 1;
    }
    r
};

// The tricks below rely on these.
const _: () = assert!(P[0] == 1); // so -p^-1 mod 2^29 = -1, and q*p[0] = q
const _: () = assert!(P[5] == 0 && P[6] == 0 && P[7] == 0);
const _: () = assert!(P[8] == 1 << 22);

pub const fn from_64x4(pa: [u64; 4]) -> [u32; 9] {
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

pub const fn to_64x4(pa: [u32; 9]) -> [u64; 4] {
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

pub const fn geq(a: &[u64; 4], b: &[u64; 4]) -> bool {
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

pub const fn sub(a: [u64; 4], b: &[u64; 4]) -> [u64; 4] {
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

/// x * 2^k mod p, for canonical x, by doubling (compile-time constants).
pub const fn mul_pow2_mod(mut x: [u64; 4], k: u32) -> [u64; 4] {
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

/// A field element in Montgomery form. Limbs 0..8 hold 29 bits each, limb 8
/// the rest. Values may be "almost reduced" (below 2p) between operations;
/// `canonical` brings them below p.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct Fp9(pub [u32; 9]);

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

/// Subtract p once if x >= p.
#[inline(always)]
pub fn conditional_reduce(x: &mut [u32; 9]) {
    if gte_modulus(x) {
        let mut borrow: i32 = 0;
        for i in 0..9 {
            let t = (x[i] as i32).wrapping_sub(P[i] as i32).wrapping_add(borrow);
            borrow = t >> SHIFT;
            x[i] = (t as u32) & MASK;
        }
        // x >= p, so the final borrow is zero and limb 8 needs no mask beyond
        // its 29 bits.
    }
}

/// Columns of a 9x9-limb product, not yet carried. Column k < 9 * 2^58.
#[inline(always)]
pub fn mul_wide(x: &[u32; 9], y: &[u32; 9], t: &mut [u64; 18]) {
    for i in 0..9 {
        let xi = x[i] as u64;
        for j in 0..9 {
            t[i + j] += xi * (y[j] as u64);
        }
    }
}

/// Columns of a square, with each cross product computed once and doubled.
#[inline(always)]
pub fn square_wide(x: &[u32; 9], t: &mut [u64; 18]) {
    for i in 0..9 {
        let xi = x[i] as u64;
        t[2 * i] += xi * xi;
        let xi2 = xi << 1;
        for j in (i + 1)..9 {
            t[i + j] += xi2 * (x[j] as u64);
        }
    }
}

/// Montgomery reduction of an uncarried column sum: returns t / R mod p,
/// below 2p when t < 4 p^2 + p R (see `reduce` callers). Columns must stay
/// below about 2^63 including what this adds (at most 5 * 2^58 per column).
#[inline(always)]
pub fn reduce(t: &mut [u64; 18]) -> [u32; 9] {
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

/// Interleaved (CIOS-style) Montgomery multiplication, as in
/// o1-labs/proof-systems#2638 and openmina's `32x9`, with full carries.
#[inline(always)]
pub fn mul_cios(x: &[u32; 9], y: &[u32; 9]) -> [u32; 9] {
    let mut y64 = [0u64; 9];
    for j in 0..9 {
        y64[j] = y[j] as u64;
    }
    let mut z = [0u64; 9];
    for i in 0..9 {
        let xi = x[i] as u64;
        let tmp = xi * y64[0] + z[0];
        let qi = tmp.wrapping_neg() & MASK64;
        let carry = (tmp + qi) >> SHIFT64;
        for j in 1..8 {
            let mut zj = z[j];
            if j == 1 {
                zj += carry;
            }
            z[j - 1] = zj + xi * y64[j] + qi * P64[j];
        }
        z[7] = xi * y64[8] + qi * P64[8];
    }
    let mut r = [0u32; 9];
    for j in 1..9 {
        r[j - 1] = (z[j - 1] as u32) & MASK;
        z[j] += z[j - 1] >> SHIFT64;
    }
    r[8] = z[8] as u32;
    r
}

impl Fp9 {
    pub const ZERO: Fp9 = Fp9([0; 9]);

    #[inline(always)]
    pub fn mul(&self, other: &Fp9) -> Fp9 {
        let mut t = [0u64; 18];
        mul_wide(&self.0, &other.0, &mut t);
        let mut r = reduce(&mut t);
        conditional_reduce(&mut r);
        Fp9(r)
    }

    /// Montgomery product without the final conditional subtraction: below
    /// 1.04p for inputs below 2.1p (R = 2^261 is 128 times p).
    #[inline(always)]
    pub fn mul_nr(&self, other: &Fp9) -> Fp9 {
        let mut t = [0u64; 18];
        mul_wide(&self.0, &other.0, &mut t);
        Fp9(reduce(&mut t))
    }

    #[inline(always)]
    pub fn square_nr(&self) -> Fp9 {
        let mut t = [0u64; 18];
        square_wide(&self.0, &mut t);
        Fp9(reduce(&mut t))
    }

    #[inline(always)]
    pub fn mul_cios(&self, other: &Fp9) -> Fp9 {
        let mut r = mul_cios(&self.0, &other.0);
        conditional_reduce(&mut r);
        Fp9(r)
    }

    #[inline(always)]
    pub fn square(&self) -> Fp9 {
        let mut t = [0u64; 18];
        square_wide(&self.0, &mut t);
        let mut r = reduce(&mut t);
        conditional_reduce(&mut r);
        Fp9(r)
    }

    /// Addition with full carry and one conditional subtraction.
    #[inline(always)]
    pub fn add(&self, other: &Fp9) -> Fp9 {
        let mut r = [0u32; 9];
        let mut carry = 0u32;
        for i in 0..9 {
            let t = self.0[i] + other.0[i] + carry;
            carry = t >> SHIFT;
            r[i] = t & MASK;
        }
        // limb 8 has room: values stay below 2^256.
        r[8] += carry << SHIFT;
        conditional_reduce(&mut r);
        Fp9(r)
    }

    #[inline(always)]
    pub fn canonical(&self) -> Fp9 {
        let mut r = self.0;
        conditional_reduce(&mut r);
        Fp9(r)
    }

    /// R^2 mod p, canonical limbs, to move into Montgomery form.
    fn r2() -> [u32; 9] {
        use std::sync::OnceLock;
        static R2: OnceLock<[u32; 9]> = OnceLock::new();
        *R2.get_or_init(|| {
            let r2 = ark_ff::Field::pow(&Fp::from(2u64), [2 * 261]);
            from_64x4(r2.into_bigint().0)
        })
    }

    pub fn from_ark(x: &Fp) -> Fp9 {
        let canon = from_64x4(x.into_bigint().0);
        Fp9(canon).mul(&Fp9(Self::r2()))
    }

    /// From arkworks' internal Montgomery form (x * 2^256) to this one
    /// (x * 2^261) with one product: A * 2^266 / 2^261 = A * 2^5.
    #[inline(always)]
    pub fn from_ark_mont(x: &Fp) -> Fp9 {
        const C: [u32; 9] = from_64x4(mul_pow2_mod([1, 0, 0, 0], 266));
        Fp9(from_64x4(x.0 .0)).mul(&Fp9(C))
    }

    /// Back to arkworks' Montgomery form: B * 2^256 / 2^261.
    #[inline(always)]
    pub fn to_ark_mont(&self) -> Fp {
        const C: [u32; 9] = from_64x4(mul_pow2_mod([1, 0, 0, 0], 256));
        let r = self.mul(&Fp9(C)).canonical();
        ark_ff::Fp(ark_ff::BigInt(to_64x4(r.0)), core::marker::PhantomData)
    }

    pub fn to_ark(&self) -> Fp {
        let mut one = [0u32; 9];
        one[0] = 1;
        let canon = self.mul(&Fp9(one)).canonical();
        Fp::from_bigint(BigInteger256::new(to_64x4(canon.0))).expect("below p")
    }
}
