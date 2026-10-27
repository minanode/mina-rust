//! A drop-in arkworks backend: `FpConfig<4>` for Pasta Fp whose
//! multiplication runs on 9x29-bit limbs.
//!
//! `ark_ff::Fp<P, 4>` is generic over its config, so `Fp<Fp9Config, 4>` is a
//! normal arkworks field that the unchanged `poseidon` crate (or kimchi) can
//! use. Elements are stored in a `BigInt<4>` as x * 2^261 mod p, fully
//! reduced (so `Eq` and `Hash` work), and unpacked to 9 limbs for each
//! multiplication. Switching `mina_curves::pasta::Fp` to this type on wasm32
//! is the least invasive way to use the faster multiplication everywhere.
//!
//! Prototype: inverse goes through arkworks, and there is no square-root
//! precomputation (`sqrt` is needed to decompress public keys).

use core::marker::PhantomData;

use ark_ff::{BigInt, BigInteger, Fp, FpConfig, PrimeField, SqrtPrecomputation};

use crate::fp9::{self, from_64x4, geq, mul_pow2_mod, to_64x4, MODULUS_64X4};

pub struct Fp9Config;
pub type FpW = Fp<Fp9Config, 4>;

const fn form(canonical: [u64; 4]) -> FpW {
    Fp(BigInt(mul_pow2_mod(canonical, 261)), PhantomData)
}

/// The 2^32-th root of unity of mina-curves' Fp, canonical.
/// (Printed by the `print_two_adic_root` test, checked in `crate::check`.)
const TWO_ADIC_ROOT_CANONICAL: [u64; 4] = [
    0xbdad6fabd87ea32f,
    0xea322bf2b7bb7584,
    0x362120830561f81a,
    0x2bce74deac30ebda,
];

#[inline(always)]
fn mul_limbs(a: &[u64; 4], b: &[u64; 4]) -> [u64; 4] {
    let (x, y) = (from_64x4(*a), from_64x4(*b));
    let mut t = [0u64; 18];
    fp9::mul_wide(&x, &y, &mut t);
    let mut r = fp9::reduce(&mut t);
    fp9::conditional_reduce(&mut r);
    to_64x4(r)
}

#[inline(always)]
fn square_limbs(a: &[u64; 4]) -> [u64; 4] {
    let x = from_64x4(*a);
    let mut t = [0u64; 18];
    fp9::square_wide(&x, &mut t);
    let mut r = fp9::reduce(&mut t);
    fp9::conditional_reduce(&mut r);
    to_64x4(r)
}

impl FpConfig<4> for Fp9Config {
    const MODULUS: BigInt<4> = BigInt(MODULUS_64X4);
    const GENERATOR: FpW = form([5, 0, 0, 0]);
    const ZERO: FpW = Fp(BigInt([0; 4]), PhantomData);
    const ONE: FpW = form([1, 0, 0, 0]);
    const TWO_ADICITY: u32 = 32;
    const TWO_ADIC_ROOT_OF_UNITY: FpW = form(TWO_ADIC_ROOT_CANONICAL);
    const SQRT_PRECOMP: Option<SqrtPrecomputation<FpW>> = None;

    #[inline(always)]
    fn add_assign(a: &mut FpW, b: &FpW) {
        // 2p < 2^256: no carry out.
        a.0.add_with_carry(&b.0);
        if geq(&a.0 .0, &MODULUS_64X4) {
            a.0.sub_with_borrow(&Self::MODULUS);
        }
    }

    #[inline(always)]
    fn sub_assign(a: &mut FpW, b: &FpW) {
        if b.0 > a.0 {
            a.0.add_with_carry(&Self::MODULUS);
        }
        a.0.sub_with_borrow(&b.0);
    }

    #[inline(always)]
    fn double_in_place(a: &mut FpW) {
        a.0.mul2();
        if geq(&a.0 .0, &MODULUS_64X4) {
            a.0.sub_with_borrow(&Self::MODULUS);
        }
    }

    #[inline(always)]
    fn neg_in_place(a: &mut FpW) {
        if a.0 .0 != [0; 4] {
            let mut t = Self::MODULUS;
            t.sub_with_borrow(&a.0);
            a.0 = t;
        }
    }

    #[inline(always)]
    fn mul_assign(a: &mut FpW, b: &FpW) {
        a.0 .0 = mul_limbs(&a.0 .0, &b.0 .0);
    }

    #[inline(always)]
    fn square_in_place(a: &mut FpW) {
        a.0 .0 = square_limbs(&a.0 .0);
    }

    fn sum_of_products<const T: usize>(a: &[FpW; T], b: &[FpW; T]) -> FpW {
        // Up to 6 products fit the u64 columns before one reduction.
        let mut acc = Self::ZERO;
        for (ca, cb) in a.chunks(6).zip(b.chunks(6)) {
            let mut t = [0u64; 18];
            for (x, y) in ca.iter().zip(cb) {
                fp9::mul_wide(&from_64x4(x.0 .0), &from_64x4(y.0 .0), &mut t);
            }
            let mut r = fp9::reduce(&mut t);
            fp9::conditional_reduce(&mut r);
            fp9::conditional_reduce(&mut r);
            Self::add_assign(&mut acc, &Fp(BigInt(to_64x4(r)), PhantomData));
        }
        acc
    }

    fn inverse(a: &FpW) -> Option<FpW> {
        let x = mina_curves::pasta::Fp::from_bigint(Self::into_bigint(*a))?;
        let inv = ark_ff::Field::inverse(&x)?;
        Self::from_bigint(inv.into_bigint())
    }

    fn from_bigint(x: BigInt<4>) -> Option<FpW> {
        if geq(&x.0, &MODULUS_64X4) {
            return None;
        }
        const R2: [u64; 4] = mul_pow2_mod([1, 0, 0, 0], 2 * 261);
        Some(Fp(BigInt(mul_limbs(&x.0, &R2)), PhantomData))
    }

    fn into_bigint(x: FpW) -> BigInt<4> {
        BigInt(mul_limbs(&x.0 .0, &[1, 0, 0, 0]))
    }
}

pub fn from_ark(x: &mina_curves::pasta::Fp) -> FpW {
    FpW::from_bigint(x.into_bigint()).expect("below p")
}

pub fn to_ark(x: &FpW) -> mina_curves::pasta::Fp {
    mina_curves::pasta::Fp::from_bigint(x.into_bigint()).expect("below p")
}

pub fn sponge_params() -> &'static poseidon::SpongeParams<FpW> {
    use std::sync::OnceLock;
    static P: OnceLock<poseidon::SpongeParams<FpW>> = OnceLock::new();
    P.get_or_init(|| {
        let p = poseidon::fp::params();
        poseidon::SpongeParams {
            round_constants: p
                .round_constants
                .iter()
                .map(|rc| rc.map(|x| from_ark(&x)))
                .collect(),
            mds: p.mds.map(|row| row.map(|x| from_ark(&x))),
        }
    })
}

/// The unchanged generic permutation of the `poseidon` crate, on `FpW`.
pub fn permute(state: &mut [FpW; 3]) {
    poseidon::poseidon_block_cipher::<FpW, poseidon::PlonkSpongeConstantsKimchi>(
        sponge_params(),
        state,
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn print_two_adic_root() {
        use ark_ff::{FftField, PrimeField};
        let r = <mina_curves::pasta::Fp as FftField>::TWO_ADIC_ROOT_OF_UNITY;
        println!("{:#018x?}", r.into_bigint().0);
    }
}
