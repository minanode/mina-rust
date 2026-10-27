//! The WASM fast path of the Poseidon permutation (`src/perm9.rs`) against
//! arkworks, for every parameter set the node uses.
//!
//! In WASM, `poseidon_block_cipher` runs Pasta Fp on 9x29-bit limbs; natively
//! it is arkworks itself. So these tests check the fast path when they run as
//! WASM, in Node:
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//! cargo +nightly-2026-07-15 test --release -p poseidon --target wasm32-unknown-unknown
//! ```
//!
//! Natively they check the reference below against the crate. The reference
//! uses arkworks arithmetic only: the crate's `full_round`, which has no fast
//! path, and a copy of its sponge.

#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use std::marker::PhantomData;

use ark_ff::{Field, PrimeField};
use mina_curves::pasta::{Fp, Fq};
use once_cell::sync::Lazy;
use poseidon::{
    full_round,
    hash::{self, legacy, LazyParam},
    poseidon_block_cipher, PlonkSpongeConstantsKimchi, PlonkSpongeConstantsLegacy, SpongeConstants,
    SpongeParams,
};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::wasm_bindgen_test as test;

/// How many random inputs of each kind. WASM is what these tests are for;
/// natively, in a debug build, fewer keep them quick.
const N: usize = if cfg!(target_arch = "wasm32") {
    1_000
} else {
    100
};

macro_rules! log {
    ($($t:tt)*) => {{
        #[cfg(target_arch = "wasm32")]
        wasm_bindgen_test::console_log!($($t)*);
        #[cfg(not(target_arch = "wasm32"))]
        println!($($t)*);
    }};
}

/// Deterministic pseudo-random field elements (xorshift64*), the same on
/// every platform, so a failure in WASM can be reproduced natively.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn field<F: PrimeField>(&mut self) -> F {
        let mut bytes = [0u8; 32];
        for chunk in bytes.chunks_mut(8) {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        F::from_le_bytes_mod_order(&bytes)
    }

    fn fields<F: PrimeField>(&mut self, n: usize) -> Vec<F> {
        (0..n).map(|_| self.field()).collect()
    }
}

/// Edge values and values next to 29-bit limb boundaries, as whole states.
fn edge_states<F: PrimeField>() -> Vec<[F; 3]> {
    let mut values = vec![F::ZERO, F::ONE, -F::ONE, -F::ONE.double()];
    for k in [29u64, 58, 116, 232, 253] {
        let two_k = F::from(2u64).pow([k]);
        values.extend([two_k, two_k - F::ONE, -two_k]);
    }
    let mut states: Vec<[F; 3]> = values.iter().map(|&v| [v, v, v]).collect();
    states.extend(values.windows(3).map(|w| [w[0], w[1], w[2]]));
    states
}

/// `poseidon_block_cipher` as it is without the fast path.
fn reference_permutation<F: Field, SC: SpongeConstants>(
    params: &SpongeParams<F>,
    state: &mut [F; 3],
) {
    if SC::PERM_INITIAL_ARK {
        for (i, x) in params.round_constants[0].iter().enumerate() {
            state[i] += x;
        }
        for r in 0..SC::PERM_ROUNDS_FULL {
            full_round::<F, SC>(params, state, r + 1);
        }
    } else {
        for r in 0..SC::PERM_ROUNDS_FULL {
            full_round::<F, SC>(params, state, r);
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Absorbed(usize),
    Squeezed(usize),
}

/// The crate's `Sponge`, on `reference_permutation`.
struct Reference<F: Field, SC: SpongeConstants> {
    params: &'static SpongeParams<F>,
    state: [F; 3],
    mode: Mode,
    constants: PhantomData<SC>,
}

impl<F: Field, SC: SpongeConstants> Reference<F, SC> {
    fn new(params: &'static SpongeParams<F>) -> Self {
        Self {
            params,
            state: [F::ZERO; 3],
            mode: Mode::Absorbed(0),
            constants: PhantomData,
        }
    }

    fn permute(&mut self) {
        reference_permutation::<F, SC>(self.params, &mut self.state);
    }

    fn absorb(&mut self, fields: &[F]) {
        if fields.is_empty() {
            self.mode = match self.mode {
                Mode::Absorbed(n) if n == SC::SPONGE_RATE => {
                    self.permute();
                    Mode::Absorbed(1)
                }
                Mode::Absorbed(n) => Mode::Absorbed(n + 1),
                Mode::Squeezed(_) => Mode::Absorbed(1),
            };
            return;
        }
        for x in fields {
            self.mode = match self.mode {
                Mode::Absorbed(n) if n == SC::SPONGE_RATE => {
                    self.permute();
                    self.state[0] += x;
                    Mode::Absorbed(1)
                }
                Mode::Absorbed(n) => {
                    self.state[n] += x;
                    Mode::Absorbed(n + 1)
                }
                Mode::Squeezed(_) => {
                    self.state[0] += x;
                    Mode::Absorbed(1)
                }
            };
        }
    }

    fn squeeze(&mut self) -> F {
        match self.mode {
            Mode::Squeezed(n) if n == SC::SPONGE_RATE => {
                self.permute();
                self.mode = Mode::Squeezed(1);
                self.state[0]
            }
            Mode::Squeezed(n) => {
                self.mode = Mode::Squeezed(n + 1);
                self.state[n]
            }
            Mode::Absorbed(_) => {
                self.permute();
                self.mode = Mode::Squeezed(1);
                self.state[0]
            }
        }
    }
}

fn check_permutation<F: PrimeField, SC: SpongeConstants>(
    name: &str,
    params: &SpongeParams<F>,
    seed: u64,
) -> usize {
    let mut rng = Rng(seed);
    let mut states = edge_states::<F>();
    states.extend((0..N).map(|_| [rng.field(), rng.field(), rng.field()]));
    for state in &states {
        let mut expected = *state;
        reference_permutation::<F, SC>(params, &mut expected);
        let mut got = *state;
        poseidon_block_cipher::<F, SC>(params, &mut got);
        assert_eq!(got, expected, "{name} permutation of {state:?}");
        // Chained, so later states are permutation outputs.
        let mut twice = got;
        poseidon_block_cipher::<F, SC>(params, &mut twice);
        reference_permutation::<F, SC>(params, &mut expected);
        assert_eq!(twice, expected, "{name} permutation of {got:?}");
    }
    2 * states.len()
}

#[test]
fn permutations_match_arkworks() {
    let mut count = 0;
    count +=
        check_permutation::<Fp, PlonkSpongeConstantsKimchi>("Kimchi Fp", poseidon::fp::params(), 1);
    count += check_permutation::<Fp, PlonkSpongeConstantsLegacy>(
        "legacy Fp",
        poseidon::fp_legacy::params(),
        2,
    );
    // No fast path for Fq: this checks it is left alone.
    count +=
        check_permutation::<Fq, PlonkSpongeConstantsKimchi>("Kimchi Fq", poseidon::fq::params(), 3);
    log!("{count} permutations matched arkworks");
}

/// The lengths hashed with each parameter: none, one, both halves of the
/// rate, one past it, and longer inputs up to an account's 41 fields.
const LENGTHS: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 10, 16, 41];

/// The field a parameter's sponge absorbed: its string padded with `*`, or
/// with zeros for the parameters of `hash_noinputs`.
fn domain_field(param: &LazyParam, noinput: bool) -> Fp {
    if !noinput {
        return hash::param_to_field(param.string());
    }
    let mut bytes = [0u8; 32];
    bytes[..param.string().len()].copy_from_slice(param.string().as_bytes());
    Fp::from_random_bytes(&bytes).expect("a valid field element")
}

fn check_params<SC: SpongeConstants>(
    params: &[&Lazy<Box<LazyParam>>],
    sponge_params: &'static SpongeParams<Fp>,
    noinput: bool,
    hash: fn(&LazyParam, &[Fp]) -> Fp,
    rng: &mut Rng,
) -> usize {
    let inputs_per_length = N / 100;
    let mut count = 0;
    for param in params {
        let param: &LazyParam = param;
        let name = param.string();
        let mut initial = Reference::<Fp, SC>::new(sponge_params);
        initial.absorb(&[domain_field(param, noinput)]);
        let last_squeezed = initial.squeeze();
        assert_eq!(param.state(), initial.state, "initial state of {name}");
        if noinput {
            assert_eq!(hash::hash_noinputs(param), last_squeezed, "{name}");
        }
        for len in LENGTHS {
            for _ in 0..inputs_per_length {
                let fields = rng.fields::<Fp>(len);
                let mut reference = Reference::<Fp, SC> {
                    params: sponge_params,
                    state: initial.state,
                    mode: initial.mode,
                    constants: PhantomData,
                };
                reference.absorb(&fields);
                let expected = reference.squeeze();
                assert_eq!(hash(param, &fields), expected, "{name}, {fields:?}");
                count += 1;
            }
        }
    }
    count
}

#[test]
fn every_hash_parameter_matches_arkworks() {
    let mut rng = Rng(4);
    let kimchi = check_params::<PlonkSpongeConstantsKimchi>(
        hash::params::ALL_PARAMS,
        poseidon::fp::params(),
        false,
        hash::hash_with_kimchi,
        &mut rng,
    );
    let noinput = check_params::<PlonkSpongeConstantsKimchi>(
        hash::params::ALL_NOINPUT_PARAMS,
        poseidon::fp::params(),
        true,
        hash::hash_with_kimchi,
        &mut rng,
    );
    let legacy = check_params::<PlonkSpongeConstantsLegacy>(
        legacy::params::ALL_PARAMS,
        poseidon::fp_legacy::params(),
        false,
        legacy::hash_with_kimchi,
        &mut rng,
    );
    let params = hash::params::ALL_PARAMS.len()
        + hash::params::ALL_NOINPUT_PARAMS.len()
        + legacy::params::ALL_PARAMS.len();
    log!(
        "{} hashes matched arkworks, over {params} parameters \
         ({kimchi} Kimchi, {noinput} no-input, {legacy} legacy)",
        kimchi + noinput + legacy,
    );
    // Every parameter the node uses: 36 Merkle tree levels, 6 coinbase tree
    // levels and the others in `hash::params`, 4 for `hash_noinputs`, 5
    // legacy.
    assert_eq!(hash::params::ALL_PARAMS.len(), 66);
    assert_eq!(hash::params::ALL_NOINPUT_PARAMS.len(), 4);
    assert_eq!(legacy::params::ALL_PARAMS.len(), 5);
}

fn check_hash_fields<F: PrimeField + poseidon::SpongeParamsForField<F>>(
    params: &'static SpongeParams<F>,
    rng: &mut Rng,
) -> usize {
    let mut count = 0;
    for len in LENGTHS {
        for _ in 0..N / 100 {
            let fields = rng.fields::<F>(len);
            let mut reference = Reference::<F, PlonkSpongeConstantsKimchi>::new(params);
            reference.absorb(&fields);
            assert_eq!(hash::hash_fields(&fields), reference.squeeze());
            count += 1;
        }
    }
    count
}

#[test]
fn hash_fields_matches_arkworks() {
    let mut rng = Rng(5);
    let count = check_hash_fields::<Fp>(poseidon::fp::params(), &mut rng)
        + check_hash_fields::<Fq>(poseidon::fq::params(), &mut rng);
    log!("{count} hash_fields matched arkworks");
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("after 1970")
        .as_secs_f64()
        * 1000.0
}

/// Prints how long a Merkle node hash takes with `hash_with_kimchi` and with
/// the reference, each in a chain of hashes from the same start. It asserts
/// nothing about speed, only that both chains end at the same hash.
#[test]
fn hash_speed() {
    let param = hash::params::get_merkle_param_for_height(0);
    let iterations = if cfg!(debug_assertions) { 20 } else { 2_000 };
    let mut rng = Rng(6);
    let (start_hash, other) = (rng.field::<Fp>(), rng.field::<Fp>());

    let start = now_ms();
    let mut fast = start_hash;
    for _ in 0..iterations {
        fast = hash::hash_with_kimchi(param, &[fast, other]);
    }
    let fast_ms = now_ms() - start;

    let start = now_ms();
    let mut expected = start_hash;
    for _ in 0..iterations {
        let mut reference = Reference::<Fp, PlonkSpongeConstantsKimchi> {
            params: poseidon::fp::params(),
            state: param.state(),
            mode: Mode::Squeezed(1),
            constants: PhantomData,
        };
        reference.absorb(&[expected, other]);
        expected = reference.squeeze();
    }
    let reference_ms = now_ms() - start;
    assert_eq!(fast, expected, "{iterations} chained hashes");

    let us = |ms: f64| ms * 1000.0 / iterations as f64;
    log!(
        "hash_with_kimchi, 2 fields: {:.1} us; arkworks reference: {:.1} us ({:.2}x)",
        us(fast_ms),
        us(reference_ms),
        reference_ms / fast_ms,
    );
}
