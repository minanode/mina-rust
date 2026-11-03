//! Montgomery multiplication on nine 29-bit limbs, for wasm32.
//!
//! WebAssembly has no 64x64->128-bit multiply, so the generic 64-bit limb code
//! splits every product into four 32-bit products and detects carries with
//! comparisons. Here the operands are split into nine limbs of 29 bits. A
//! product of two limbs fits in 58 bits, so each column of the product sums in a
//! `u64` with no carry handling, and the only multiply is `i64.mul`.
//!
//! The elements keep arkworks' representation: four 64-bit limbs holding
//! `a * 2^256 mod p`. Nine 29-bit digits make a Montgomery reduction by 2^261,
//! five bits more than arkworks' R = 2^256, so the first operand is shifted left
//! by five bits on the way in: for `A = a * 2^256` and `B = b * 2^256`,
//! `(32 * A) * B / 2^261 = A * B / 2^256`, the arkworks Montgomery product. The
//! result is reduced to below the modulus once, so it is the same `BigInt` as
//! the 64-bit code returns.
//!
//! This holds for a 4-limb modulus `p < 2^255` and operands below `p`, the
//! invariant of every `Fp`. Then `32 * A < 2^260` fits the nine limbs, every
//! column stays below `18 * 2^58 + 2^35 < 2^64`, and the reduced value is
//! below `(32 * p * p) / 2^261 + p < 1.5 * p`, so one conditional subtraction
//! makes it canonical.

/// Bits per limb.
const BITS: u32 = 29;
/// The low `BITS` bits.
pub const MASK: u64 = (1 << BITS) - 1;

/// Whether `modulus` can use this multiplication: four limbs, below 2^255.
pub const fn usable(modulus: &[u64]) -> bool {
    modulus.len() == 4 && modulus[3] >> 63 == 0
}

/// The 29-bit limbs of `modulus`, or zeros if it is not [`usable`].
pub const fn modulus_limbs(modulus: &[u64]) -> [u64; 9] {
    if !usable(modulus) {
        return [0; 9];
    }
    let m = [modulus[0], modulus[1], modulus[2], modulus[3]];
    unpack(&m, 0)
}

/// Bits `[29 k - shift, 29 k + 29 - shift)` of `a`, as limb `k` of `a << shift`.
#[inline(always)]
pub const fn unpack(a: &[u64; 4], shift: u32) -> [u64; 9] {
    let mut out = [0u64; 9];
    let mut k = 0;
    while k < 9 {
        // Bit position of this limb's lowest bit in `a`, as if `a` were
        // shifted left by `shift`.
        let start = (k as u32) * BITS;
        let limb = if start < shift {
            a[0] << (shift - start)
        } else {
            let bit = start - shift;
            let word = (bit / 64) as usize;
            let offset = bit % 64;
            let low = if word < 4 { a[word] >> offset } else { 0 };
            let high = if offset + BITS > 64 && word + 1 < 4 {
                a[word + 1] << (64 - offset)
            } else {
                0
            };
            low | high
        };
        out[k] = limb & MASK;
        k += 1;
    }
    out
}

/// Packs nine 29-bit limbs below 2^256 into four 64-bit limbs.
#[inline(always)]
pub fn pack(r: &[u64; 9]) -> [u64; 4] {
    let mut out = [0u64; 4];
    for k in 0..9 {
        let bit = k as u32 * BITS;
        let word = (bit / 64) as usize;
        let offset = bit % 64;
        out[word] |= r[k] << offset;
        if offset + BITS > 64 && word + 1 < 4 {
            out[word + 1] |= r[k] >> (64 - offset);
        }
    }
    out
}

/// Montgomery reduction of the product columns `t` by 2^261, then packing.
/// `p` holds the modulus's 29-bit limbs, `inv` is `-p^-1 mod 2^29`.
#[inline(always)]
fn reduce(mut t: [u64; 17], p: &[u64; 9], inv: u64) -> [u64; 4] {
    for i in 0..9 {
        let q = t[i].wrapping_mul(inv) & MASK;
        for j in 0..9 {
            t[i + j] += q * p[j];
        }
        // The low 29 bits of t[i] are now zero.
        t[i + 1] += t[i] >> BITS;
    }
    let mut r = [0u64; 9];
    let mut carry = 0;
    for k in 0..8 {
        let v = t[9 + k] + carry;
        r[k] = v & MASK;
        carry = v >> BITS;
    }
    // The value is below 1.5 p < 2^256, so this limb has at most 24 bits.
    r[8] = carry;
    pack(&r)
}

/// `a * b / 2^256 mod p`, below `2 p`: the caller subtracts `p` once if needed.
#[inline(always)]
pub fn mul(a: &[u64; 4], b: &[u64; 4], p: &[u64; 9], inv: u64) -> [u64; 4] {
    let x = unpack(a, 5);
    let y = unpack(b, 0);
    let mut t = [0u64; 17];
    for i in 0..9 {
        for j in 0..9 {
            t[i + j] += x[i] * y[j];
        }
    }
    reduce(t, p, inv)
}

/// `a * a / 2^256 mod p`, below `2 p`: the caller subtracts `p` once if needed.
///
/// The square of the unshifted limbs takes 45 products instead of 81. Its
/// columns are carried down to 29 bits before the five-bit shift, so that they
/// do not overflow.
#[inline(always)]
pub fn square(a: &[u64; 4], p: &[u64; 9], inv: u64) -> [u64; 4] {
    let x = unpack(a, 0);
    let mut t = [0u64; 17];
    for i in 0..9 {
        for j in (i + 1)..9 {
            t[i + j] += x[i] * x[j];
        }
    }
    for k in 0..17 {
        t[k] <<= 1;
    }
    for i in 0..9 {
        t[2 * i] += x[i] * x[i];
    }
    // Carry down to 29 bits, then shift the whole number left by five bits.
    // `a^2 < 2^510`, so `32 a^2 < 2^515` still fits 17 limbs plus the top
    // column's 29 + 5 bits.
    let mut carry = 0;
    for k in 0..17 {
        let v = t[k] + carry;
        t[k] = v & MASK;
        carry = v >> BITS;
    }
    t[16] += carry << BITS;
    let mut high = 0;
    for k in 0..17 {
        let v = (t[k] << 5) | high;
        high = v >> BITS;
        t[k] = v & MASK;
    }
    t[16] += high << BITS;
    reduce(t, p, inv)
}
