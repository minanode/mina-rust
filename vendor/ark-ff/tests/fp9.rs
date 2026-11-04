//! Tests of the nine-limb wasm32 multiplication (`fp9`) against `num-bigint`.
//!
//! The limb functions are tested on every target. The field tests use the
//! `fp9` path only on wasm32, or natively with `RUSTFLAGS="--cfg ark_ff_fp9"`;
//! otherwise they check the 64-bit code against the same reference.

use ark_ff::{
    fields::models::fp::fp9::{modulus_limbs, mul, pack, square, unpack, usable, MASK},
    BigInt, BigInteger, Field, Fp256, MontBackend, MontConfig, PrimeField, Zero,
};
use num_bigint::BigUint;

#[derive(MontConfig)]
#[modulus = "28948022309329048855892746252171976963363056481941560715954676764349967630337"]
#[generator = "5"]
pub struct PallasBaseConfig;
type PallasBase = Fp256<MontBackend<PallasBaseConfig, 4>>;

#[derive(MontConfig)]
#[modulus = "28948022309329048855892746252171976963363056481941647379679742748393362948097"]
#[generator = "5"]
pub struct VestaBaseConfig;
type VestaBase = Fp256<MontBackend<VestaBaseConfig, 4>>;

#[derive(MontConfig)]
#[modulus = "21888242871839275222246405745257275088548364400416034343698204186575808495617"]
#[generator = "5"]
pub struct Bn254ScalarConfig;
type Bn254Scalar = Fp256<MontBackend<Bn254ScalarConfig, 4>>;

#[derive(MontConfig)]
#[modulus = "52435875175126190479447740508185965837690552500527637822603658699938581184513"]
#[generator = "7"]
pub struct Bls12ScalarConfig;
type Bls12Scalar = Fp256<MontBackend<Bls12ScalarConfig, 4>>;

#[derive(MontConfig)]
#[modulus = "57896044618658097711785492504343953926634992332820282019728792003956564819949"]
#[generator = "2"]
pub struct Curve25519Config;

const MODULI: [[u64; 4]; 4] = [
    PallasBaseConfig::MODULUS.0,
    VestaBaseConfig::MODULUS.0,
    Bn254ScalarConfig::MODULUS.0,
    Bls12ScalarConfig::MODULUS.0,
];

fn big(limbs: &[u64; 4]) -> BigUint {
    let mut bytes = [0u8; 32];
    for (i, limb) in limbs.iter().enumerate() {
        bytes[8 * i..8 * i + 8].copy_from_slice(&limb.to_le_bytes());
    }
    BigUint::from_bytes_le(&bytes)
}

fn limbs(v: &BigUint) -> [u64; 4] {
    let mut bytes = v.to_bytes_le();
    assert!(bytes.len() <= 32);
    bytes.resize(32, 0);
    let mut out = [0u64; 4];
    for (i, limb) in out.iter_mut().enumerate() {
        *limb = u64::from_le_bytes(bytes[8 * i..8 * i + 8].try_into().unwrap());
    }
    out
}

/// -p^-1 mod 2^64, by Newton's iteration.
fn neg_inv(p0: u64) -> u64 {
    let mut inv = 1u64;
    for _ in 0..6 {
        inv = inv.wrapping_mul(2u64.wrapping_sub(p0.wrapping_mul(inv)));
    }
    assert_eq!(p0.wrapping_mul(inv), 1);
    inv.wrapping_neg()
}

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545f4914f6cdd1d)
    }

    fn below(&mut self, p: &BigUint) -> BigUint {
        big(&[self.next(), self.next(), self.next(), self.next()]) % p
    }
}

/// Values below `p`: 0, 1, 2, p - 1, p - 2, and values next to powers of two
/// at and around the 29- and 64-bit limb boundaries.
fn edges(p: &BigUint) -> Vec<BigUint> {
    let one = BigUint::from(1u32);
    let mut values = vec![
        BigUint::from(0u32),
        one.clone(),
        BigUint::from(2u32),
        p - &one,
        p - BigUint::from(2u32),
    ];
    for bit in [
        29u32, 58, 63, 64, 87, 116, 128, 145, 192, 203, 232, 253, 254,
    ] {
        let power = &one << bit;
        for v in [&power - &one, power.clone(), &power + &one] {
            if &v < p {
                values.push(v);
            }
        }
    }
    values
}

#[test]
fn limbs_round_trip() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for _ in 0..10000 {
        let a = [rng.next(), rng.next(), rng.next(), rng.next() >> 1];
        assert_eq!(pack(&unpack(&a, 0)), a);
        let mut shifted = BigUint::from(0u32);
        for (k, limb) in unpack(&a, 5).iter().enumerate() {
            assert!(*limb <= MASK);
            shifted += BigUint::from(*limb) << (29 * k);
        }
        assert_eq!(shifted, big(&a) << 5u32);
    }
}

#[test]
fn products_match_montgomery_reference() {
    for modulus in MODULI {
        assert!(usable(&modulus));
        let p = big(&modulus);
        let p9 = modulus_limbs(&modulus);
        let inv = neg_inv(modulus[0]) & MASK;
        // R^-1 mod p, with arkworks' R = 2^256.
        let r_inv = (BigUint::from(1u32) << 256u32).modpow(&(&p - 2u32), &p);
        let check = |a: &BigUint, b: &BigUint| {
            let (x, y) = (limbs(a), limbs(b));
            let product = big(&mul(&x, &y, &p9, inv));
            assert!(product < &p + &p, "product not below 2p");
            assert_eq!(product % &p, a * b * &r_inv % &p, "{a} * {b}");
            let squared = big(&square(&x, &p9, inv));
            assert!(squared < &p + &p, "square not below 2p");
            assert_eq!(squared % &p, a * a * &r_inv % &p, "{a}^2");
        };
        let edges = edges(&p);
        for a in &edges {
            for b in &edges {
                check(a, b);
            }
        }
        let mut rng = Rng(modulus[0] | 1);
        for _ in 0..20000 {
            check(&rng.below(&p), &rng.below(&p));
        }
    }
}

#[test]
fn dispatch_follows_target() {
    let expected = cfg!(any(target_arch = "wasm32", ark_ff_fp9));
    assert_eq!(
        ark_ff::fields::fp9_enabled::<PallasBaseConfig, 4>(),
        expected
    );
    assert_eq!(
        ark_ff::fields::fp9_enabled::<VestaBaseConfig, 4>(),
        expected
    );
}

#[test]
fn refuses_unsupported_moduli() {
    assert!(!usable(&[1, 0, 0, 1 << 63]));
    assert!(!usable(&[1, 0, 0]));
    assert!(!usable(&[1, 0, 0, 0, 0]));
    assert_eq!(modulus_limbs(&[1, 0, 0, u64::MAX]), [0; 9]);
    assert!(PallasBaseConfig::FP9 && VestaBaseConfig::FP9);
    assert!(Bn254ScalarConfig::FP9 && Bls12ScalarConfig::FP9);
}

fn field_ops<F: PrimeField<BigInt = ark_ff::BigInt<4>>>() {
    let p = big(&F::MODULUS.0);
    let to_field = |v: &BigUint| F::from_bigint(ark_ff::BigInt(limbs(v))).unwrap();
    let to_big = |f: F| big(&f.into_bigint().0);
    let mut rng = Rng(F::MODULUS.0[1] | 1);
    let mut values = edges(&p);
    values.extend((0..2000).map(|_| rng.below(&p)));
    for (i, a) in values.iter().enumerate() {
        let b = &values[(i * 7 + 3) % values.len()];
        let c = &values[(i * 13 + 5) % values.len()];
        let (fa, fb, fc) = (to_field(a), to_field(b), to_field(c));
        assert_eq!(to_big(fa * fb), a * b % &p);
        assert_eq!(to_big(fa.square()), a * a % &p);
        let mut doubled = fa;
        doubled.square_in_place();
        assert_eq!(to_big(doubled * fa), a * a * a % &p);
        assert_eq!(
            to_big(F::sum_of_products(&[fa, fb], &[fb, fc])),
            (a * b + b * c) % &p
        );
        assert_eq!(
            to_big(F::sum_of_products(&[fa, fb, fc], &[fc, fa, fb])),
            (a * c + b * a + c * b) % &p
        );
        if !fa.is_zero() {
            assert_eq!(fa * fa.inverse().unwrap(), F::one());
        }
        let root = fa.square().sqrt().unwrap();
        assert!(root == fa || root == -fa);
        assert_eq!(fa.into_bigint().to_bytes_le(), limbs_bytes(a));
    }
}

fn limbs_bytes(v: &BigUint) -> Vec<u8> {
    let mut bytes = v.to_bytes_le();
    bytes.resize(32, 0);
    bytes
}

#[test]
fn field_operations_match_reference() {
    field_ops::<PallasBase>();
    field_ops::<VestaBase>();
    field_ops::<Bn254Scalar>();
    field_ops::<Bls12Scalar>();
}

/// `fp9::mul` and `fp9::square` against arkworks' own 64-bit Montgomery code,
/// which the field operations use on targets other than wasm32: 3,000,000
/// random canonical pairs per field, and every pair of edge values (0, 1,
/// p - 1 to p - 32, and 2^k - 2 to 2^k + 2 for every k below p). Five fields,
/// one of them 2^255 - 19, the largest kind `usable` accepts. About 23 million
/// pairs; run it in release:
/// `cargo test --release --manifest-path vendor/ark-ff/Cargo.toml --test fp9 -- --ignored`
#[cfg(not(any(target_arch = "wasm32", ark_ff_fp9)))]
#[test]
#[ignore = "23 million pairs; run in release with --ignored"]
fn millions_of_pairs_match_the_64_bit_code() {
    fn run<C: MontConfig<4>>(name: &str) {
        type F<C> = Fp256<MontBackend<C, 4>>;
        let p9 = modulus_limbs(&C::MODULUS.0);
        let inv = C::INV & MASK;
        let canonical = |limbs: [u64; 4]| {
            let mut r = BigInt(limbs);
            let mut twice = C::MODULUS;
            twice.mul2();
            assert!(r < twice, "{name}: result not below 2p");
            if r >= C::MODULUS {
                r.sub_with_borrow(&C::MODULUS);
            }
            r
        };
        let check = |a: BigInt<4>, b: BigInt<4>| {
            let (fa, fb) = (F::<C>::new_unchecked(a), F::<C>::new_unchecked(b));
            assert_eq!(canonical(mul(&a.0, &b.0, &p9, inv)), (fa * fb).0, "{name}");
            assert_eq!(canonical(square(&a.0, &p9, inv)), fa.square().0, "{name}");
        };
        let mut edges = vec![BigInt::from(0u64), BigInt::from(1u64)];
        for d in 1u64..=32 {
            let mut x = C::MODULUS;
            x.sub_with_borrow(&BigInt::from(d));
            edges.push(x);
        }
        for bit in 0..255 {
            let mut power = BigInt([0; 4]);
            power.0[bit / 64] = 1 << (bit % 64);
            for d in -2i64..=2 {
                let mut x = power;
                let borrowed = if d < 0 {
                    x.sub_with_borrow(&BigInt::from(d.unsigned_abs()))
                } else {
                    x.add_with_carry(&BigInt::from(d as u64))
                };
                if !borrowed && x < C::MODULUS {
                    edges.push(x);
                }
            }
        }
        edges.sort();
        edges.dedup();
        for &a in &edges {
            for &b in &edges {
                check(a, b);
            }
        }
        let mut rng = Rng(C::MODULUS.0[0] ^ 0x6f374abcc918e027);
        let mut random = || loop {
            let x = BigInt([rng.next(), rng.next(), rng.next(), rng.next() >> 1]);
            if x < C::MODULUS {
                break x;
            }
        };
        for _ in 0..3_000_000 {
            check(random(), random());
        }
        println!(
            "{name}: 3000000 random pairs and {} edge pairs match",
            edges.len() * edges.len()
        );
    }
    run::<PallasBaseConfig>("Pasta Fp");
    run::<VestaBaseConfig>("Pasta Fq");
    run::<Bn254ScalarConfig>("BN254 scalar");
    run::<Bls12ScalarConfig>("BLS12-381 scalar");
    run::<Curve25519Config>("2^255 - 19");
}
