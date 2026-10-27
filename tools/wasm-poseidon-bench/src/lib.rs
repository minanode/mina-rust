//! Benchmarks for research task R8: how fast Pasta Fp multiplication and the
//! Poseidon permutation of `poseidon::hash::hash_with_kimchi` run natively and
//! in WASM, and how much a WASM-friendly field representation gains.
//!
//! The same kernels are run by `src/bin/native_bench.rs` natively and by
//! `web/bench-core.mjs` in a browser or Node through the `r8_*` exports.

#![allow(clippy::needless_range_loop)]

pub mod backend;
pub mod fp9;
pub mod perm9;
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
pub mod simd;

use ark_ff::{AdditiveGroup, Field, PrimeField, UniformRand};
use mina_curves::pasta::Fp;
use poseidon::{
    hash::{hash_with_kimchi, params::get_merkle_param_for_height},
    PlonkSpongeConstantsKimchi,
};

use fp9::Fp9;

/// Deterministic field elements, the same on every platform.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    fn next_u64(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn fp(&mut self) -> Fp {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        Fp::from_le_bytes_mod_order(&bytes)
    }
}

impl ark_std::rand::RngCore for Rng {
    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
    fn next_u64(&mut self) -> u64 {
        Rng::next_u64(self)
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for b in dest {
            *b = self.next_u64() as u8;
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), ark_std::rand::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

fn low32(x: &Fp) -> u32 {
    x.into_bigint().0[0] as u32
}

fn low32_9(x: &Fp9) -> u32 {
    x.0[0]
}

fn ark_perm(state: &mut [Fp; 3]) {
    poseidon::poseidon_block_cipher::<Fp, PlonkSpongeConstantsKimchi>(
        poseidon::fp::params(),
        state,
    );
}

/// A named benchmark kernel.
pub type Kernel = (&'static str, fn(u32) -> u32);

/// Benchmark kernels. Each runs `n` dependent operations and returns some bits
/// of the result so nothing is optimised away.
pub const KINDS: &[Kernel] = &[
    ("ark Fp mul (chain)", ark_mul),
    ("ark Fp square (chain)", ark_square),
    ("ark Fp add (chain)", ark_add),
    ("ark Poseidon permutation", ark_permutation),
    ("ark hash_with_kimchi, 2 fields", ark_hash2),
    ("ark hash_with_kimchi, 10 fields", ark_hash10),
    ("arkworks Fp<Fp9Config> mul (chain)", backend_mul),
    ("arkworks Fp<Fp9Config> square (chain)", backend_square),
    (
        "arkworks Fp<Fp9Config>, poseidon crate permutation",
        backend_permutation,
    ),
    ("fp9 mul (chain)", fp9_mul),
    ("fp9 mul CIOS (chain)", fp9_mul_cios),
    ("fp9 square (chain)", fp9_square),
    ("fp9 add (chain)", fp9_add),
    ("fp9 Poseidon permutation", fp9_permutation),
    ("fp9 Poseidon permutation, CIOS mul", fp9_permutation_cios),
    ("fp9 Poseidon permutation, lazy MDS", fp9_permutation_lazy),
    (
        "fp9 Poseidon permutation, lazy, no cond. sub.",
        fp9_permutation_lazy_nr,
    ),
    ("fp9 hash, 2 fields, lazy, no cond. sub.", fp9_hash2_lazy),
    (
        "fp9 hash, 2 fields, arkworks Fp in and out",
        fp9_hash2_ark_io,
    ),
    (
        "fp9 hash, 10 fields, arkworks Fp in and out",
        fp9_hash10_ark_io,
    ),
    ("fp9 hash, 10 fields, lazy, no cond. sub.", fp9_hash10_lazy),
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    ("simd fp9 mul x2 lanes (chain)", simd::bench_mul2),
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    ("simd fp9 Poseidon permutation", simd::bench_permutation),
    #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
    (
        "simd fp9 Poseidon permutation, 2 at once",
        simd::bench_permutation_pairs,
    ),
];

fn ark_mul(n: u32) -> u32 {
    let mut rng = Rng::new(1);
    let mut x = rng.fp();
    let y = rng.fp();
    for _ in 0..n {
        x *= y;
    }
    low32(&x)
}

fn ark_square(n: u32) -> u32 {
    let mut x = Rng::new(2).fp();
    for _ in 0..n {
        x.square_in_place();
    }
    low32(&x)
}

fn ark_add(n: u32) -> u32 {
    let mut rng = Rng::new(3);
    let mut x = rng.fp();
    let y = rng.fp();
    for _ in 0..n {
        x += y;
    }
    low32(&x)
}

fn ark_permutation(n: u32) -> u32 {
    let mut rng = Rng::new(4);
    let mut state = [rng.fp(), rng.fp(), rng.fp()];
    for _ in 0..n {
        ark_perm(&mut state);
    }
    low32(&state[0])
}

fn ark_hash2(n: u32) -> u32 {
    let mut rng = Rng::new(5);
    let param = get_merkle_param_for_height(0);
    let mut h = rng.fp();
    let other = rng.fp();
    for _ in 0..n {
        h = hash_with_kimchi(param, &[h, other]);
    }
    low32(&h)
}

fn ark_hash10(n: u32) -> u32 {
    let mut rng = Rng::new(6);
    let param = &poseidon::hash::params::MINA_ACCOUNT;
    let mut fields: Vec<Fp> = (0..10).map(|_| rng.fp()).collect();
    for _ in 0..n {
        fields[0] = hash_with_kimchi(param, &fields);
    }
    low32(&fields[0])
}

fn backend_mul(n: u32) -> u32 {
    let mut rng = Rng::new(1);
    let mut x = backend::from_ark(&rng.fp());
    let y = backend::from_ark(&rng.fp());
    for _ in 0..n {
        x *= y;
    }
    x.0 .0[0] as u32
}

fn backend_square(n: u32) -> u32 {
    let mut x = backend::from_ark(&Rng::new(2).fp());
    for _ in 0..n {
        x.square_in_place();
    }
    x.0 .0[0] as u32
}

fn backend_permutation(n: u32) -> u32 {
    let mut rng = Rng::new(4);
    let mut state = [rng.fp(), rng.fp(), rng.fp()].map(|x| backend::from_ark(&x));
    for _ in 0..n {
        backend::permute(&mut state);
    }
    state[0].0 .0[0] as u32
}

fn fp9_mul(n: u32) -> u32 {
    let mut rng = Rng::new(1);
    let mut x = Fp9::from_ark(&rng.fp());
    let y = Fp9::from_ark(&rng.fp());
    for _ in 0..n {
        x = x.mul(&y);
    }
    low32_9(&x)
}

fn fp9_mul_cios(n: u32) -> u32 {
    let mut rng = Rng::new(1);
    let mut x = Fp9::from_ark(&rng.fp());
    let y = Fp9::from_ark(&rng.fp());
    for _ in 0..n {
        x = x.mul_cios(&y);
    }
    low32_9(&x)
}

fn fp9_square(n: u32) -> u32 {
    let mut x = Fp9::from_ark(&Rng::new(2).fp());
    for _ in 0..n {
        x = x.square();
    }
    low32_9(&x)
}

fn fp9_add(n: u32) -> u32 {
    let mut rng = Rng::new(3);
    let mut x = Fp9::from_ark(&rng.fp());
    let y = Fp9::from_ark(&rng.fp());
    for _ in 0..n {
        x = x.add(&y);
    }
    low32_9(&x)
}

fn fp9_state(seed: u64) -> [Fp9; 3] {
    let mut rng = Rng::new(seed);
    [rng.fp(), rng.fp(), rng.fp()].map(|x| Fp9::from_ark(&x))
}

fn fp9_permutation(n: u32) -> u32 {
    let params = perm9::params();
    let mut state = fp9_state(4);
    for _ in 0..n {
        perm9::permute(params, &mut state);
    }
    low32_9(&state[0])
}

fn fp9_permutation_cios(n: u32) -> u32 {
    let params = perm9::params();
    let mut state = fp9_state(4);
    for _ in 0..n {
        perm9::permute_cios(params, &mut state);
    }
    low32_9(&state[0])
}

fn fp9_permutation_lazy(n: u32) -> u32 {
    let params = perm9::params();
    let mut state = fp9_state(4);
    for _ in 0..n {
        perm9::permute_lazy(params, &mut state);
    }
    low32_9(&state[0])
}

fn fp9_permutation_lazy_nr(n: u32) -> u32 {
    let params = perm9::params();
    let mut state = fp9_state(4);
    for _ in 0..n {
        perm9::permute_lazy_nr(params, &mut state);
    }
    low32_9(&state[0])
}

fn fp9_hash2_lazy(n: u32) -> u32 {
    let mut rng = Rng::new(5);
    let init = perm9::lazy_param_state9(get_merkle_param_for_height(0));
    let mut h = Fp9::from_ark(&rng.fp());
    let other = Fp9::from_ark(&rng.fp());
    for _ in 0..n {
        h = perm9::hash_with_kimchi9(perm9::permute_lazy_nr, &init, &[h, other]);
    }
    low32_9(&h)
}

/// What a drop-in `hash_with_kimchi` would do: arkworks `Fp` in and out,
/// converted with one product each, the sponge's initial state precomputed.
pub fn hash_with_kimchi_fp9(init: &[Fp9; 3], fields: &[Fp]) -> Fp {
    let mut buf = [Fp9::ZERO; 16];
    let long: Vec<Fp9>;
    let fields9: &[Fp9] = if fields.len() <= buf.len() {
        for (b, f) in buf.iter_mut().zip(fields) {
            *b = Fp9::from_ark_mont(f);
        }
        &buf[..fields.len()]
    } else {
        long = fields.iter().map(Fp9::from_ark_mont).collect();
        &long
    };
    perm9::hash_with_kimchi9(perm9::permute_lazy_nr, init, fields9).to_ark_mont()
}

fn fp9_hash2_ark_io(n: u32) -> u32 {
    let mut rng = Rng::new(5);
    let init = perm9::lazy_param_state9(get_merkle_param_for_height(0));
    let mut h = rng.fp();
    let other = rng.fp();
    for _ in 0..n {
        h = hash_with_kimchi_fp9(&init, &[h, other]);
    }
    low32(&h)
}

fn fp9_hash10_ark_io(n: u32) -> u32 {
    let mut rng = Rng::new(6);
    let init = perm9::lazy_param_state9(&poseidon::hash::params::MINA_ACCOUNT);
    let mut fields: Vec<Fp> = (0..10).map(|_| rng.fp()).collect();
    for _ in 0..n {
        fields[0] = hash_with_kimchi_fp9(&init, &fields);
    }
    low32(&fields[0])
}

fn fp9_hash10_lazy(n: u32) -> u32 {
    let mut rng = Rng::new(6);
    let init = perm9::lazy_param_state9(&poseidon::hash::params::MINA_ACCOUNT);
    let mut fields: Vec<Fp9> = (0..10).map(|_| Fp9::from_ark(&rng.fp())).collect();
    for _ in 0..n {
        fields[0] = perm9::hash_with_kimchi9(perm9::permute_lazy_nr, &init, &fields);
    }
    low32_9(&fields[0])
}

/// Compares every fast path with arkworks and the `poseidon` crate on
/// deterministic inputs. Returns the number of mismatches (0 is good).
pub fn check() -> u32 {
    let mut failures = 0u32;
    let mut fail = |ok: bool| {
        if !ok {
            failures += 1;
        }
    };
    let mut rng = Rng::new(42);

    // Edge values as well as random ones.
    let minus_one = -Fp::ONE;
    let mut values = vec![Fp::ZERO, Fp::ONE, minus_one, minus_one.double()];
    for _ in 0..2000 {
        values.push(Fp::rand(&mut rng));
    }
    {
        use ark_ff::{FftField, One, Zero};
        use backend::{from_ark, to_ark, FpW};
        fail(to_ark(&FpW::one()) == Fp::ONE);
        fail(FpW::one().is_one() && FpW::zero().is_zero());
        fail(to_ark(&<FpW as FftField>::GENERATOR) == <Fp as FftField>::GENERATOR);
        fail(
            to_ark(&<FpW as FftField>::TWO_ADIC_ROOT_OF_UNITY)
                == <Fp as FftField>::TWO_ADIC_ROOT_OF_UNITY,
        );
        for w in values.windows(3) {
            let (a, b, c) = (w[0], w[1], w[2]);
            let (aw, bw, cw) = (from_ark(&a), from_ark(&b), from_ark(&c));
            fail(to_ark(&aw) == a);
            fail(to_ark(&(aw * bw)) == a * b);
            fail(to_ark(&aw.square()) == a.square());
            fail(to_ark(&(aw + bw)) == a + b);
            fail(to_ark(&(aw - bw)) == a - b);
            fail(to_ark(&(-aw)) == -a);
            fail(to_ark(&aw.double()) == a.double());
            fail(aw.inverse().map(|x| to_ark(&x)) == a.inverse());
            fail(
                to_ark(&FpW::sum_of_products(&[aw, bw, cw], &[cw, aw, bw]))
                    == Fp::sum_of_products(&[a, b, c], &[c, a, b]),
            );
            fail((aw == bw) == (a == b));
        }
    }
    for w in values.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (a9, b9) = (Fp9::from_ark(&a), Fp9::from_ark(&b));
        fail(a9.to_ark() == a);
        fail(Fp9::from_ark_mont(&a).to_ark() == a);
        fail(a9.to_ark_mont() == a);
        fail(a9.mul(&b9).to_ark() == a * b);
        fail(a9.mul_cios(&b9).to_ark() == a * b);
        fail(a9.square().to_ark() == a.square());
        fail(a9.add(&b9).to_ark() == a + b);
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        {
            let [x, y] = simd::mul2(&[a9, b9], &[b9, a9]);
            fail(x.to_ark() == a * b);
            fail(y.to_ark() == a * b);
        }
    }

    let params = perm9::params();
    for _ in 0..50 {
        let state = [rng.fp(), rng.fp(), rng.fp()];
        let mut expected = state;
        ark_perm(&mut expected);
        let state9 = state.map(|x| Fp9::from_ark(&x));
        type Perm = fn(&perm9::Params9, &mut [Fp9; 3]);
        #[allow(unused_mut)]
        let mut perms: Vec<Perm> = vec![
            perm9::permute,
            perm9::permute_cios,
            perm9::permute_lazy,
            perm9::permute_lazy_nr,
        ];
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        perms.push(simd::permute);
        for perm in perms {
            let mut s = state9;
            perm(params, &mut s);
            fail(s.map(|x| x.to_ark()) == expected);
        }
        let mut sw = state.map(|x| backend::from_ark(&x));
        backend::permute(&mut sw);
        fail(sw.map(|x| backend::to_ark(&x)) == expected);
        #[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
        {
            let other = [rng.fp(), rng.fp(), rng.fp()];
            let mut expected2 = other;
            ark_perm(&mut expected2);
            let mut pair = [state9, other.map(|x| Fp9::from_ark(&x))];
            simd::permute2(params, &mut pair);
            fail(pair[0].map(|x| x.to_ark()) == expected);
            fail(pair[1].map(|x| x.to_ark()) == expected2);
        }
    }

    for len in [0usize, 1, 2, 3, 10, 41] {
        let fields: Vec<Fp> = (0..len).map(|_| rng.fp()).collect();
        let fields9: Vec<Fp9> = fields.iter().map(Fp9::from_ark).collect();
        for param in [
            get_merkle_param_for_height(0),
            get_merkle_param_for_height(34),
            &poseidon::hash::params::MINA_ACCOUNT,
        ] {
            let expected = hash_with_kimchi(param, &fields);
            let init = perm9::lazy_param_state9(param);
            fail(perm9::hash_with_kimchi9(perm9::permute, &init, &fields9).to_ark() == expected);
            fail(
                perm9::hash_with_kimchi9(perm9::permute_lazy, &init, &fields9).to_ark() == expected,
            );
            fail(
                perm9::hash_with_kimchi9(perm9::permute_lazy_nr, &init, &fields9).to_ark()
                    == expected,
            );
            fail(hash_with_kimchi_fp9(&init, &fields) == expected);
        }
    }
    failures
}

#[cfg(target_arch = "wasm32")]
mod exports {
    use super::*;

    fn no_randomness(_: &mut [u8]) -> Result<(), getrandom::Error> {
        Err(getrandom::Error::UNSUPPORTED)
    }
    getrandom::register_custom_getrandom!(no_randomness);

    #[no_mangle]
    pub extern "C" fn r8_check() -> u32 {
        check()
    }

    #[no_mangle]
    pub extern "C" fn r8_kinds() -> u32 {
        KINDS.len() as u32
    }

    #[no_mangle]
    pub extern "C" fn r8_name_ptr(kind: u32) -> *const u8 {
        KINDS[kind as usize].0.as_ptr()
    }

    #[no_mangle]
    pub extern "C" fn r8_name_len(kind: u32) -> u32 {
        KINDS[kind as usize].0.len() as u32
    }

    #[no_mangle]
    pub extern "C" fn r8_bench(kind: u32, n: u32) -> u32 {
        (KINDS[kind as usize].1)(n)
    }
}
