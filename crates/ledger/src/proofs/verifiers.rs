use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use anyhow::Context;
use mina_core::{debug, info, log::system_time, warn};
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// ab84160: kimchi `VerifierIndex` uses `std::sync::OnceLock` (not once_cell's
/// `OnceCell`) for `permutation_vanishing_polynomial_m`/`w`. This helper builds a
/// pre-filled `OnceLock`, mirroring kimchi's own construction in `verifier_index.rs`.
fn once_lock_with<T>(value: T) -> std::sync::OnceLock<T> {
    let cell = std::sync::OnceLock::new();
    let _ = cell.set(value);
    cell
}

use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use kimchi::{
    circuits::{
        constraints::FeatureFlags,
        expr::Linearization,
        lookup::lookups::{LookupFeatures, LookupPatterns},
        polynomials::permutation::{permutation_vanishing_polynomial, zk_w},
    },
    linearization::expr_linearization,
    mina_curves::pasta::Pallas,
};
use mina_curves::pasta::{Fp, Fq};
use poly_commitment::{ipa::SRS, SRS as _};

use crate::{proofs::BACKEND_TOCK_ROUNDS_N, VerificationKey};

use super::{
    transaction::{endos, InnerCurve},
    wrap::{Domain, Domains},
    VerifierIndex,
};

#[derive(Clone, Copy)]
enum Kind {
    BlockVerifier,
    TransactionVerifier,
}

impl Kind {
    pub fn to_str(self) -> &'static str {
        match self {
            Self::BlockVerifier => "block_verifier_index",
            Self::TransactionVerifier => "transaction_verifier_index",
        }
    }

    pub fn filename(self) -> String {
        format!("{}.postcard", self.to_str())
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_str())
    }
}

fn cache_filename(kind: Kind) -> PathBuf {
    let circuits_config = mina_core::NetworkConfig::global().circuits_config;
    Path::new(circuits_config.directory_name).join(kind.filename())
}

#[cfg(not(target_family = "wasm"))]
fn cache_path(kind: Kind) -> Option<PathBuf> {
    super::circuit_blobs::home_base_dir().map(|p| p.join(cache_filename(kind)))
}

macro_rules! read_cache {
    ($kind: expr, $digest: expr) => {{
        // The cache is this node's own (`write_cache`): no release publishes
        // it, so it is only read locally.
        #[cfg(not(target_family = "wasm"))]
        let data = super::circuit_blobs::read_local(&cache_filename($kind))
            .context("reading the verifier index cache")?;
        #[cfg(target_family = "wasm")]
        let data = super::circuit_blobs::fetch(&cache_filename($kind))
            .await
            .context("fetching verifier index failed")?;
        let mut slice = data.as_slice();
        let mut d = [0; 32];
        // source digest
        slice.read_exact(&mut d).context("reading source digest")?;
        if d != $digest {
            // A cache made from another embedded index, or not a cache at
            // all, as a server's fallback page for a missing file is.
            anyhow::bail!(
                "source digest verification failed: the cache was made from {}, this build embeds {}",
                hex::encode(&d[..8]),
                hex::encode(&$digest[..8])
            );
        }

        // index digest
        slice.read_exact(&mut d).context("reading index digest")?;

        let mut hasher = Sha256::new();
        hasher.update(slice);
        let digest = hasher.finalize();
        if d != digest.as_slice() {
            anyhow::bail!("verifier index digest verification failed");
        }
        Ok(super::caching::verifier_index_from_bytes(slice)?)
    }};
}

#[cfg(not(target_family = "wasm"))]
fn read_cache(kind: Kind, digest: &[u8]) -> anyhow::Result<VerifierIndex<Fq>> {
    read_cache!(kind, digest)
}

#[cfg(target_family = "wasm")]
async fn read_cache(kind: Kind, digest: &[u8]) -> anyhow::Result<VerifierIndex<Fq>> {
    read_cache!(kind, digest)
}

#[cfg(not(target_family = "wasm"))]
fn write_cache(kind: Kind, index: &VerifierIndex<Fq>, digest: &[u8]) -> anyhow::Result<()> {
    use std::{fs::File, io::Write};

    let path = cache_path(kind)
        .ok_or_else(|| anyhow::anyhow!("$HOME env not set, so can't cache verifier index"))?;
    let bytes = super::caching::verifier_index_to_bytes(index)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let Some(parent) = path.parent() else {
        anyhow::bail!("cannot get parent for {path:?}");
    };
    std::fs::create_dir_all(parent).context("creating cache file parent directory")?;
    let mut file = File::create(path).context("creating cache file")?;
    file.write_all(digest).context("storing source digest")?;
    file.write_all(&hasher.finalize())
        .context("storing verifier index digest")?;
    file.write_all(&bytes)
        .context("storing verifier index into cache file")?;
    Ok(())
}

macro_rules! make_with_ext_cache {
    ($kind: expr, $data: expr) => {{
        let verifier_index: VerifierIndex<Fq> = serde_json::from_str($data).unwrap();
        let mut hasher = Sha256::new();
        hasher.update($data);
        let src_index_digest = hasher.finalize();

        #[cfg(not(target_family = "wasm"))]
        let cache = read_cache($kind, &src_index_digest);
        #[cfg(target_family = "wasm")]
        let cache = read_cache($kind, &src_index_digest).await;

        match cache {
            Ok(verifier_index) => {
                debug!(system_time(); "Verifier index is loaded");
                verifier_index
            }
            Err(err) => {
                if is_not_cached(&err) {
                    // A first start (RC7-FIXES: not a warning).
                    info!(
                        system_time();
                        "The {} isn't cached yet: building it from the embedded one",
                        $kind.to_str().replace('_', " ")
                    );
                } else {
                    warn!(system_time(); "Cannot load verifier index: {err:#}");
                }
                let index = make_verifier_index(verifier_index);
                #[cfg(not(target_family = "wasm"))]
                if let Err(err) = write_cache($kind, &index, &src_index_digest) {
                    warn!(system_time(); "Cannot store verifier index to cache file: {err}");
                } else {
                    debug!(system_time(); "Stored verifier index to cache file");
                }
                index
            }
        }
    }}
}

/// A cache read that failed only because the cache isn't there: a first
/// start, not a problem.
fn is_not_cached(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(not(target_family = "wasm"))]
fn make_with_ext_cache(kind: Kind, data: &str) -> VerifierIndex<Fq> {
    make_with_ext_cache!(kind, data)
}

#[cfg(target_family = "wasm")]
async fn make_with_ext_cache(kind: Kind, data: &str) -> VerifierIndex<Fq> {
    make_with_ext_cache!(kind, data)
}

/// Verifier index for block proofs (consensus layer / block selection).
/// Lazily initialized and cached globally.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockVerifier(Arc<VerifierIndex<Fq>>);

/// Verifier index for transaction proofs (execution layer / transaction confirmation).
/// Lazily initialized and cached globally.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TransactionVerifier(Arc<VerifierIndex<Fq>>);

static BLOCK_VERIFIER: OnceCell<BlockVerifier> = OnceCell::new();
static TX_VERIFIER: OnceCell<TransactionVerifier> = OnceCell::new();

impl BlockVerifier {
    /// Identity of the embedded network-specific verifier index used by make().
    pub fn source_sha256() -> String {
        super::util::sha256_sum(Self::src_json().as_bytes())
    }

    fn kind() -> Kind {
        Kind::BlockVerifier
    }

    /// The block verifier, if it was made already: making it is async in a
    /// browser.
    pub fn get() -> Option<Self> {
        BLOCK_VERIFIER.get().cloned()
    }

    /// Makes `index` the block verifier, in place of the embedded one, for a
    /// runtime fork whose blockchain circuits differ from the release's (see
    /// [`super::fork_circuits`]). Only before the verifier is first made.
    pub fn install(index: VerifierIndex<Fq>) -> Result<(), String> {
        BLOCK_VERIFIER
            .set(Self(Arc::new(index)))
            .map_err(|_| "the block verifier was already made".to_owned())
    }

    /// A block verifier of this index, whichever network the process runs:
    /// for tests that check another network's blocks
    /// ([`block_verifier_index_of`]).
    pub fn from_index(index: VerifierIndex<Fq>) -> Self {
        Self(Arc::new(index))
    }

    fn src_json() -> &'static str {
        let network_name = mina_core::NetworkConfig::global().name;
        match network_name {
            "mainnet" => include_str!("data/mainnet_blockchain_verifier_index.json"),
            "devnet" => include_str!("data/devnet_blockchain_verifier_index.json"),
            other => panic!("get_verifier_index: unknown network '{other}'"),
        }
    }
}

impl TransactionVerifier {
    fn kind() -> Kind {
        Kind::TransactionVerifier
    }

    fn src_json() -> &'static str {
        let network_name = mina_core::NetworkConfig::global().name;
        match network_name {
            "mainnet" => include_str!("data/mainnet_transaction_verifier_index.json"),
            "devnet" => include_str!("data/devnet_transaction_verifier_index.json"),
            other => panic!("get_verifier_index: unknown network '{other}'"),
        }
    }

    pub fn get() -> Option<Self> {
        TX_VERIFIER.get().cloned()
    }
}

#[cfg(not(target_family = "wasm"))]
impl BlockVerifier {
    /// Creates or returns cached block verifier index from embedded JSON data.
    /// Network-specific (mainnet/devnet). Uses external cache for faster loading.
    pub fn make() -> Self {
        BLOCK_VERIFIER
            .get_or_init(|| {
                Self(Arc::new(make_with_ext_cache(
                    Self::kind(),
                    Self::src_json(),
                )))
            })
            .clone()
    }
}

#[cfg(target_family = "wasm")]
impl BlockVerifier {
    pub async fn make() -> Self {
        if let Some(v) = BLOCK_VERIFIER.get() {
            v.clone()
        } else {
            let verifier = Self(Arc::new(
                make_with_ext_cache(Self::kind(), Self::src_json()).await,
            ));
            BLOCK_VERIFIER.get_or_init(move || verifier).clone()
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl TransactionVerifier {
    /// Creates or returns cached transaction verifier index from embedded JSON.
    /// Network-specific (mainnet/devnet). Uses external cache for faster loading.
    pub fn make() -> Self {
        TX_VERIFIER
            .get_or_init(|| {
                Self(Arc::new(make_with_ext_cache(
                    Self::kind(),
                    Self::src_json(),
                )))
            })
            .clone()
    }
}

#[cfg(target_family = "wasm")]
impl TransactionVerifier {
    pub async fn make() -> Self {
        if let Some(v) = TX_VERIFIER.get() {
            v.clone()
        } else {
            let verifier = Self(Arc::new(
                make_with_ext_cache(Self::kind(), Self::src_json()).await,
            ));
            TX_VERIFIER.get_or_init(move || verifier).clone()
        }
    }
}

impl std::ops::Deref for BlockVerifier {
    type Target = VerifierIndex<Fq>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::Deref for TransactionVerifier {
    type Target = VerifierIndex<Fq>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<BlockVerifier> for Arc<VerifierIndex<Fq>> {
    fn from(value: BlockVerifier) -> Self {
        value.0
    }
}

impl From<TransactionVerifier> for Arc<VerifierIndex<Fq>> {
    fn from(value: TransactionVerifier) -> Self {
        value.0
    }
}

/// The embedded mainnet transaction verifier index, built without the global
/// network config or the cache file, so that tests which verify real mainnet
/// proofs can run next to tests that use the default network.
#[cfg(test)]
pub(crate) fn mainnet_transaction_verifier_index() -> VerifierIndex<Fq> {
    let json = include_str!("data/mainnet_transaction_verifier_index.json");
    make_verifier_index(serde_json::from_str(json).unwrap())
}

/// The embedded blockchain verifier index of `network` ("mainnet" or
/// "devnet"), built without the global network config, the global verifier
/// or the cache file: for checking a proof against another network's key, as
/// the negative control that a proof made for one network does not verify on
/// the other.
pub fn block_verifier_index_of(network: &str) -> anyhow::Result<VerifierIndex<Fq>> {
    let json = match network {
        "mainnet" => include_str!("data/mainnet_blockchain_verifier_index.json"),
        "devnet" => include_str!("data/devnet_blockchain_verifier_index.json"),
        other => anyhow::bail!("unknown network {other}"),
    };
    Ok(make_verifier_index(serde_json::from_str(json)?))
}

/// As [`block_verifier_index_of`], for transaction proofs (snark work).
pub fn transaction_verifier_index_of(network: &str) -> anyhow::Result<VerifierIndex<Fq>> {
    let json = match network {
        "mainnet" => include_str!("data/mainnet_transaction_verifier_index.json"),
        "devnet" => include_str!("data/devnet_transaction_verifier_index.json"),
        other => anyhow::bail!("unknown network {other}"),
    };
    Ok(make_verifier_index(serde_json::from_str(json)?))
}

fn make_verifier_index(index: VerifierIndex<Fq>) -> VerifierIndex<Fq> {
    let domain = index.domain;
    let max_poly_size: usize = index.max_poly_size;
    let (endo, _) = endos::<Fq>();

    let feature_flags = FeatureFlags {
        range_check0: false,
        range_check1: false,
        foreign_field_add: false,
        foreign_field_mul: false,
        xor: false,
        rot: false,
        lookup_features: LookupFeatures {
            patterns: LookupPatterns {
                xor: false,
                lookup: false,
                range_check: false,
                foreign_field_mul: false,
            },
            joint_lookup_used: false,
            uses_runtime_tables: false,
        },
    };

    let (mut linearization, powers_of_alpha) = expr_linearization(Some(&feature_flags), true);

    let linearization = Linearization {
        constant_term: linearization.constant_term,
        index_terms: {
            // Make the verifier index deterministic
            linearization
                .index_terms
                .sort_by_key(|&(columns, _)| columns);
            linearization.index_terms
        },
    };

    // <https://github.com/o1-labs/proof-systems/blob/2702b09063c7a48131173d78b6cf9408674fd67e/kimchi/src/verifier_index.rs#L310-L314>
    let srs = {
        let srs = SRS::create(max_poly_size);
        srs.get_lagrange_basis(domain);
        Arc::new(srs)
    };

    // <https://github.com/o1-labs/proof-systems/blob/2702b09063c7a48131173d78b6cf9408674fd67e/kimchi/src/verifier_index.rs#L319>
    let permutation_vanishing_polynomial_m =
        permutation_vanishing_polynomial(domain, index.zk_rows);

    // <https://github.com/o1-labs/proof-systems/blob/2702b09063c7a48131173d78b6cf9408674fd67e/kimchi/src/verifier_index.rs#L324>
    let w = zk_w(domain, index.zk_rows);

    VerifierIndex::<Fq> {
        srs,
        permutation_vanishing_polynomial_m: once_lock_with(permutation_vanishing_polynomial_m),
        w: once_lock_with(w),
        endo,
        linearization,
        powers_of_alpha,
        ..index
    }
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/crypto/kimchi_bindings/stubs/src/pasta_fq_plonk_verifier_index.rs#L213>
/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/common.ml#L16C1-L25C58>
pub fn make_shifts(domain: &Radix2EvaluationDomain<Fq>) -> [Fq; kimchi::circuits::wires::PERMUTS] {
    super::public_input::plonk_checks::make_shifts(domain)
}

// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/common.ml#L27>
pub fn wrap_domains(proofs_verified: usize) -> Domains {
    let h = match proofs_verified {
        0 => 13,
        1 => 14,
        2 => 15,
        _ => unreachable!(),
    };

    Domains {
        h: Domain::Pow2RootsOfUnity(h),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The SRS of zkApp verifier indexes, with the Lagrange basis of `domain`
/// in its cache: the process's SRS on the wrap curve
/// ([`get_srs::<Fq>`](crate::verifier::get_srs)), shared by every zkApp
/// verifier index, as OCaml shares its one SRS between side-loaded keys.
/// It used to be made again, with its basis, for every proof: about 1 s of
/// an 8-vCPU machine.
///
/// The SRS's cache computes a missing basis while it holds its lock. A panic
/// there would poison the lock and fail every later zkApp proof, and a rayon
/// thread of the computation that took a job needing the same basis would
/// wait for itself. So the basis is computed on a copy of the SRS that no
/// other thread sees, then stored in the shared one, and the verifier only
/// ever reads a stored basis. Threads that need a new basis at the same
/// time may each compute it; the first one stored is kept.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/crypto/pickles/side_loaded_verification_key.ml#L179-L183>
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/crypto/pickles/pickles.ml#L307-L311>
pub fn zkapp_verifier_srs(domain: Radix2EvaluationDomain<Fq>) -> Arc<SRS<Pallas>> {
    let srs = crate::verifier::get_srs::<Fq>();
    debug_assert_eq!(srs.g.len(), 1 << BACKEND_TOCK_ROUNDS_N);
    store_lagrange_basis(&srs, domain);
    srs
}

/// Stores the Lagrange basis of `domain` in `srs`'s cache if it isn't there,
/// computed on a private copy of `srs` ([`zkapp_verifier_srs`]).
fn store_lagrange_basis(srs: &SRS<Pallas>, domain: Radix2EvaluationDomain<Fq>) {
    if srs.lagrange_bases().contains_key(&domain.size()) {
        return;
    }
    let basis = {
        let private = SRS::<Pallas>::new(srs.g.clone(), srs.h);
        #[cfg(test)]
        test_hooks::computing_basis(domain.size());
        let basis = private.get_lagrange_basis(domain).clone();
        basis
    };
    srs.lagrange_bases().set_once(domain.size(), basis);
}

/// What every zkApp verifier index of a wrap domain holds besides its key's
/// commitments, by domain (2^13, 2^14, 2^15): the shared SRS with the
/// domain's Lagrange basis, the linearization of the wrap circuit and the
/// domain's constants. Made once per domain, without the lock held, for the
/// reasons [`zkapp_verifier_srs`] gives; the first one stored is kept.
static ZKAPP_INDEX_TEMPLATES: Mutex<[Option<Arc<VerifierIndex<Fq>>>; 3]> =
    Mutex::new([None, None, None]);

fn zkapp_index_template(log2_size: u64) -> Arc<VerifierIndex<Fq>> {
    // `wrap_domains` gives 2^13 to 2^15.
    let slot = log2_size as usize - 13;
    if let Some(template) = lock(&ZKAPP_INDEX_TEMPLATES)[slot].clone() {
        return template;
    }
    let template = Arc::new(make_zkapp_index_template(log2_size));
    lock(&ZKAPP_INDEX_TEMPLATES)[slot]
        .get_or_insert(template)
        .clone()
}

/// <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/pickles/side_loaded_verification_key.ml#L206>
fn make_zkapp_index_template(log2_size: u64) -> VerifierIndex<Fq> {
    use ark_ec::AffineRepr as _;

    let public = 40; // Is that constant ?

    let domain: Radix2EvaluationDomain<Fq> = Radix2EvaluationDomain::new(1 << log2_size).unwrap();

    let srs = zkapp_verifier_srs(domain);

    let feature_flags = FeatureFlags {
        range_check0: false,
        range_check1: false,
        foreign_field_add: false,
        foreign_field_mul: false,
        rot: false,
        xor: false,
        lookup_features: LookupFeatures {
            patterns: LookupPatterns {
                xor: false,
                lookup: false,
                range_check: false,
                foreign_field_mul: false,
            },
            joint_lookup_used: false,
            uses_runtime_tables: false,
        },
    };

    let (endo_q, _endo_r) = endos::<Fq>();
    let (linearization, powers_of_alpha) = expr_linearization(Some(&feature_flags), true);

    let shift = make_shifts(&domain);

    // <https://github.com/MinaProtocol/mina/blob/047375688f93546d4bdd58c75674394e3faae1f4/src/lib/pickles/side_loaded_verification_key.ml#L232>
    let zk_rows = 3;

    // Each key's commitments replace these ([`make_zkapp_verifier_index`]).
    let placeholder = poly_commitment::PolyComm {
        chunks: vec![Pallas::generator()],
    };

    // Note: Verifier index is converted from OCaml here:
    // <https://github.com/MinaProtocol/mina/blob/bfd1009abdbee78979ff0343cc73a3480e862f58/src/lib/crypto/kimchi_bindings/stubs/src/pasta_fq_plonk_verifier_index.rs#L58>

    VerifierIndex::<Fq> {
        domain,
        max_poly_size: 1 << BACKEND_TOCK_ROUNDS_N,
        srs,
        public,
        prev_challenges: 2,
        sigma_comm: std::array::from_fn(|_| placeholder.clone()),
        coefficients_comm: std::array::from_fn(|_| placeholder.clone()),
        generic_comm: placeholder.clone(),
        psm_comm: placeholder.clone(),
        complete_add_comm: placeholder.clone(),
        mul_comm: placeholder.clone(),
        emul_comm: placeholder.clone(),
        endomul_scalar_comm: placeholder,
        range_check0_comm: None,
        range_check1_comm: None,
        foreign_field_add_comm: None,
        foreign_field_mul_comm: None,
        xor_comm: None,
        rot_comm: None,
        shift,
        permutation_vanishing_polynomial_m: once_lock_with(permutation_vanishing_polynomial(
            domain, zk_rows,
        )),
        w: { once_lock_with(zk_w(domain, zk_rows)) },
        endo: endo_q,
        lookup_index: None,
        linearization,
        powers_of_alpha,
        zk_rows,
    }
}

/// The verifier index of a zkApp verification key: its wrap domain's
/// template ([`zkapp_index_template`]) with the key's commitments. Making it
/// costs a copy of the template's linearization; the SRS, its Lagrange basis
/// and the linearization are made once per domain, not per key or proof.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/c9e2eac5344da621828c1e96c074b8bdce665b51/src/lib/crypto/pickles/side_loaded_verification_key.ml#L160-L225>
pub fn make_zkapp_verifier_index(vk: &VerificationKey) -> VerifierIndex<Fq> {
    #[cfg(test)]
    test_hooks::making_index(vk);
    let d = wrap_domains(vk.actual_wrap_domain_size.to_int());
    let template = zkapp_index_template(d.h.log2_size());

    let make_poly = |poly: &InnerCurve<Fp>| poly_commitment::PolyComm {
        chunks: vec![poly.to_affine()],
    };

    VerifierIndex::<Fq> {
        sigma_comm: vk.wrap_index.sigma.each_ref().map(make_poly),
        coefficients_comm: vk.wrap_index.coefficients.each_ref().map(make_poly),
        generic_comm: make_poly(&vk.wrap_index.generic),
        psm_comm: make_poly(&vk.wrap_index.psm),
        complete_add_comm: make_poly(&vk.wrap_index.complete_add),
        mul_comm: make_poly(&vk.wrap_index.mul),
        emul_comm: make_poly(&vk.wrap_index.emul),
        endomul_scalar_comm: make_poly(&vk.wrap_index.endomul_scalar),
        ..(*template).clone()
    }
}

/// zkApp verifier indexes kept, the most recently used. A block's proofs
/// use a few keys, and an index costs about the size of the linearization,
/// so the bound is on memory, smaller in a browser.
const ZKAPP_VERIFIER_INDEXES: usize = if cfg!(target_family = "wasm") { 16 } else { 64 };

struct ZkappVerifierIndexes {
    /// Each key, its index, and when it was last used.
    entries: Vec<(VerificationKey, Arc<VerifierIndex<Fq>>, u64)>,
    uses: u64,
}

static ZKAPP_VERIFIER_INDEX_CACHE: Mutex<ZkappVerifierIndexes> = Mutex::new(ZkappVerifierIndexes {
    entries: Vec::new(),
    uses: 0,
});

impl ZkappVerifierIndexes {
    fn get(&mut self, vk: &VerificationKey) -> Option<Arc<VerifierIndex<Fq>>> {
        self.uses += 1;
        let now = self.uses;
        let (_, index, used) = self.entries.iter_mut().find(|(key, _, _)| key == vk)?;
        *used = now;
        Some(index.clone())
    }
}

/// The verifier index of a zkApp verification key, from a bounded cache of
/// the most recently used ([`ZKAPP_VERIFIER_INDEXES`]).
///
/// An account update names its key by hash, and `verifier::common::check`
/// checks that the key it verifies with has that hash. The cache compares
/// whole keys, not hashes: an index is only ever used for the key it was
/// made from, and comparing costs less than hashing. The lock is not held
/// while an index is made; two threads that miss at the same time may each
/// make it, and the first one stored is kept.
pub fn zkapp_verifier_index(vk: &VerificationKey) -> Arc<VerifierIndex<Fq>> {
    if let Some(index) = lock(&ZKAPP_VERIFIER_INDEX_CACHE).get(vk) {
        return index;
    }
    let index = Arc::new(make_zkapp_verifier_index(vk));
    let mut cache = lock(&ZKAPP_VERIFIER_INDEX_CACHE);
    if let Some(first) = cache.get(vk) {
        return first;
    }
    if cache.entries.len() >= ZKAPP_VERIFIER_INDEXES {
        let oldest = (0..cache.entries.len()).min_by_key(|&i| cache.entries[i].2);
        if let Some(oldest) = oldest {
            cache.entries.swap_remove(oldest);
        }
    }
    let now = cache.uses;
    cache.entries.push((vk.clone(), index.clone(), now));
    index
}

/// Panics on demand where the zkApp verifier's caches make what they keep,
/// to show that a panic there poisons nothing.
#[cfg(test)]
mod test_hooks {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    use crate::VerificationKey;

    /// The domain size whose next Lagrange basis computation panics.
    pub static PANIC_COMPUTING_BASIS: AtomicUsize = AtomicUsize::new(0);

    /// Keys whose next verifier index, on any thread, panics while it is made.
    pub static PANIC_MAKING_INDEX: Mutex<Vec<VerificationKey>> = Mutex::new(Vec::new());

    pub fn computing_basis(size: usize) {
        if PANIC_COMPUTING_BASIS
            .compare_exchange(size, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            panic!("test hook: a panic while a Lagrange basis is computed");
        }
    }

    pub fn making_index(vk: &VerificationKey) {
        let panic = {
            let mut keys = super::lock(&PANIC_MAKING_INDEX);
            let at = keys.iter().position(|key| key == vk);
            at.map(|at| keys.swap_remove(at)).is_some()
        };
        if panic {
            panic!("test hook: a panic while a zkApp verifier index is made");
        }
    }
}

/// The next verifier index made for `vk`, on any thread, panics.
#[cfg(test)]
pub(crate) fn panic_making_next_zkapp_index(vk: &VerificationKey) {
    lock(&test_hooks::PANIC_MAKING_INDEX).push(vk.clone());
}

#[cfg(test)]
mod zkapp_index_tests {
    use super::*;
    use ark_ec::AffineRepr as _;
    use std::sync::atomic::Ordering;

    fn key(n: u64) -> VerificationKey {
        let mut key = (*VerificationKey::dummy()).clone();
        key.actual_wrap_domain_size = crate::ProofVerified::N0;
        key.wrap_index.generic =
            InnerCurve::of_affine((Pallas::generator() * Fq::from(n + 2)).into());
        key
    }

    /// One index per key: the same `Arc` for an equal key, another for a key
    /// that differs in a commitment, each with its key's commitments, and
    /// the SRS, basis and linearization of their domain shared.
    #[test]
    fn a_key_gets_its_own_index_and_shares_the_rest() {
        let (a, b) = (key(1), key(2));
        let index_a = zkapp_verifier_index(&a);
        assert!(Arc::ptr_eq(&index_a, &zkapp_verifier_index(&a.clone())));
        let index_b = zkapp_verifier_index(&b);
        assert!(!Arc::ptr_eq(&index_a, &index_b));
        assert_eq!(
            index_a.generic_comm.chunks,
            vec![a.wrap_index.generic.to_affine()]
        );
        assert_eq!(
            index_b.generic_comm.chunks,
            vec![b.wrap_index.generic.to_affine()]
        );
        assert!(Arc::ptr_eq(&index_a.srs, &index_b.srs));
        assert!(Arc::ptr_eq(&index_a.srs, &crate::verifier::get_srs::<Fq>()));
        assert!(index_a.srs.lagrange_bases().contains_key(&(1 << 13)));
        assert_eq!(index_a.domain.size(), 1 << 13);
    }

    /// The cache keeps at most `ZKAPP_VERIFIER_INDEXES` keys, dropping the
    /// least recently used.
    #[test]
    fn the_index_cache_is_bounded() {
        let kept = key(1000);
        let first = zkapp_verifier_index(&kept);
        for n in 0..2 * ZKAPP_VERIFIER_INDEXES as u64 {
            zkapp_verifier_index(&key(2000 + n));
            // Used again, so it is never the least recently used.
            assert!(Arc::ptr_eq(&first, &zkapp_verifier_index(&kept)));
        }
        assert!(lock(&ZKAPP_VERIFIER_INDEX_CACHE).entries.len() <= ZKAPP_VERIFIER_INDEXES);
    }

    /// A panic while an index is made leaves the cache usable, with no
    /// index for that key: the lock isn't held while an index is made.
    #[test]
    fn a_panic_making_an_index_poisons_nothing() {
        let key = key(3000);
        panic_making_next_zkapp_index(&key);
        assert!(std::panic::catch_unwind(|| zkapp_verifier_index(&key)).is_err());
        assert!(!ZKAPP_VERIFIER_INDEX_CACHE.is_poisoned());
        assert!(lock(&ZKAPP_VERIFIER_INDEX_CACHE).get(&key).is_none());
        let index = zkapp_verifier_index(&key);
        assert!(Arc::ptr_eq(&index, &zkapp_verifier_index(&key)));
    }

    /// A panic while a Lagrange basis is computed leaves the SRS's cache
    /// unpoisoned and without that basis, and the next use computes it: the
    /// SRS's lock is held only to store a computed basis. The SRS here is
    /// small and private to the test.
    #[test]
    fn a_panic_computing_a_lagrange_basis_poisons_nothing() {
        const SIZE: usize = 1 << 7;
        let srs = SRS::<Pallas>::create(SIZE);
        let domain = Radix2EvaluationDomain::<Fq>::new(SIZE).unwrap();
        test_hooks::PANIC_COMPUTING_BASIS.store(SIZE, Ordering::SeqCst);
        assert!(std::panic::catch_unwind(|| store_lagrange_basis(&srs, domain)).is_err());
        assert!(!srs.lagrange_bases().contains_key(&SIZE));
        store_lagrange_basis(&srs, domain);
        assert!(srs.lagrange_bases().contains_key(&SIZE));
        let fresh = SRS::<Pallas>::create(SIZE);
        assert_eq!(
            *srs.get_lagrange_basis(domain),
            *fresh.get_lagrange_basis(domain)
        );
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod rc7_cache_tests {
    use anyhow::Context;

    /// RC7-FIXES: a missing verifier index cache is looked for only locally
    /// and reads as "not cached yet", which the node logs at INFO; a damaged
    /// or stale cache still warns.
    #[test]
    fn rc7_a_missing_verifier_index_cache_is_not_cached_and_not_fetched() {
        let error = super::super::circuit_blobs::read_local(&"4.0.2devnet/rc7_missing.postcard")
            .context("reading the verifier index cache")
            .unwrap_err();
        assert!(super::is_not_cached(&error), "{error:#}");
        assert!(!format!("{error:#}").contains("github"), "{error:#}");
        assert!(!super::is_not_cached(&anyhow::anyhow!(
            "source digest verification failed"
        )));
    }
}
