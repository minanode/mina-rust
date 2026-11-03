//! Lagrange basis caches of the prover SRS.
//!
//! Each prover index needs the SRS's commitments to the Lagrange basis of its
//! domain. Computing them is an inverse FFT over curve points, with a scalar
//! multiplication in every butterfly: most of a prover's setup, 16 of 25 s for
//! the block provers in Chrome. They depend only on the SRS and the domain
//! size, so a node keeps them, as it keeps its verifier indexes:
//!
//! - natively, a node that computes a basis writes it to
//!   `~/.mina/circuit-blobs/<circuits>/lagrange_basis_<curve>_<log2 n>.bin`
//!   and later reads it from the local circuit sources;
//! - in WASM, the prover fetches it with the circuit files, pinned by
//!   SHA-256 like the verifier index caches. The web build ships only caches
//!   that `mina internal check-lagrange-cache` accepts ([`check_file`]).
//!
//! A cache is used only if it names this SRS (its SHA-256 over the SRS points)
//! and the domain size, every point is on the curve, and the basis passes a
//! random linear combination check against the SRS ([`check_basis`]), so a
//! cache of valid points in the wrong places or with the wrong values is
//! refused too. Anything else, a missing file included, falls back to
//! computing the basis.

use std::{
    any::TypeId,
    io::{self, Read},
    path::{Path, PathBuf},
};

use ark_ec::{AffineRepr, VariableBaseMSM};
use ark_ff::UniformRand;
use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use ark_serialize::{Compress, Validate};
use mina_curves::pasta::{Fp, Fq, Pallas, Vesta};
use poly_commitment::{
    commitment::{CommitmentCurve, PolyComm},
    ipa::SRS,
    SRS as _,
};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"MINALGB1";

/// `lagrange_basis_<curve>_<log2 n>.bin`, for the curve of the SRS points.
pub fn file_name<G: 'static>(domain_size: usize) -> String {
    let curve = if TypeId::of::<G>() == TypeId::of::<Vesta>() {
        "vesta"
    } else if TypeId::of::<G>() == TypeId::of::<Pallas>() {
        "pallas"
    } else {
        "other"
    };
    format!(
        "lagrange_basis_{curve}_{}.bin",
        domain_size.trailing_zeros()
    )
}

/// The curve and the log2 of the domain size in a cache's file name.
fn parse_name(name: &str) -> Option<(&str, u32)> {
    let rest = name.strip_prefix("lagrange_basis_")?.strip_suffix(".bin")?;
    let (curve, log2) = rest.split_once('_')?;
    let valid = matches!(curve, "pallas" | "vesta")
        && (1..=2).contains(&log2.len())
        && log2.bytes().all(|b| b.is_ascii_digit());
    Some((curve, log2.parse().ok().filter(|_| valid)?))
}

fn relative_path<G: 'static>(domain_size: usize) -> PathBuf {
    let circuits_config = mina_core::NetworkConfig::global().circuits_config;
    Path::new(circuits_config.directory_name).join(file_name::<G>(domain_size))
}

/// SHA-256 over the SRS's points, compressed.
fn srs_digest<G: AffineRepr>(srs: &SRS<G>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update((srs.g.len() as u64).to_le_bytes());
    let mut buf = Vec::with_capacity(64);
    for point in srs.g.iter().chain(std::iter::once(&srs.h)) {
        buf.clear();
        point
            .serialize_compressed(&mut buf)
            .expect("serializing to a Vec does not fail");
        hasher.update(&buf);
    }
    hasher.finalize().into()
}

/// The cache file's bytes for `basis`, the Lagrange basis of `srs` for its
/// size.
pub fn encode<G: AffineRepr>(srs: &SRS<G>, basis: &[PolyComm<G>]) -> Vec<u8> {
    let chunks = basis.first().map_or(0, |c| c.chunks.len());
    let point_size = G::zero().uncompressed_size();
    let mut out = Vec::with_capacity(56 + basis.len() * chunks * point_size);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(basis.len() as u64).to_le_bytes());
    out.extend_from_slice(&(chunks as u64).to_le_bytes());
    out.extend_from_slice(&srs_digest(srs));
    for commitment in basis {
        assert_eq!(
            commitment.chunks.len(),
            chunks,
            "uneven Lagrange commitments"
        );
        for point in &commitment.chunks {
            point
                .serialize_uncompressed(&mut out)
                .expect("serializing to a Vec does not fail");
        }
    }
    out
}

fn invalid(why: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, why.into())
}

/// Checks that `basis` is the Lagrange basis of `srs` for `domain`.
///
/// The Lagrange polynomials and the monomials span the same polynomials, so
/// for any values `s_i`, committing to the polynomial that takes them on the
/// domain gives the same point either way: `sum_i s_i L_i` from the basis, and
/// `sum_j r_j g_j` from the SRS, where `r` is the inverse FFT of `s`. With more
/// points than the SRS, chunk `k` of each `L_i` pairs with `r_j` for `j` in
/// chunk `k`. The `s_i` come from the OS's random number generator after the
/// basis is read, so a basis that differs from the true one anywhere passes
/// with probability 1 over the field size. Costs two multi-scalar
/// multiplications and one FFT per chunk, small beside computing the basis.
pub fn check_basis<G: AffineRepr>(
    srs: &SRS<G>,
    domain: Radix2EvaluationDomain<G::ScalarField>,
    basis: &[PolyComm<G>],
) -> io::Result<()> {
    let n = domain.size();
    let srs_size = srs.g.len();
    if n == 0 || srs_size == 0 || basis.len() != n {
        return Err(invalid("Lagrange basis cache: wrong size"));
    }
    let chunks = n.div_ceil(srs_size);
    if basis.iter().any(|c| c.chunks.len() != chunks) {
        return Err(invalid("Lagrange basis cache: wrong number of chunks"));
    }
    let mut rng = rand::rngs::OsRng;
    let s: Vec<G::ScalarField> = (0..n).map(|_| G::ScalarField::rand(&mut rng)).collect();
    let r = domain.ifft(&s);
    for k in 0..chunks {
        let start = k * srs_size;
        let len = srs_size.min(n - start);
        let points: Vec<G> = basis.iter().map(|c| c.chunks[k]).collect();
        let from_basis = G::Group::msm_unchecked(&points, &s);
        let from_srs = G::Group::msm_unchecked(&srs.g[..len], &r[start..start + len]);
        if from_basis != from_srs {
            return Err(invalid(
                "Lagrange basis cache: not the Lagrange basis of this SRS",
            ));
        }
    }
    Ok(())
}

/// The Lagrange basis of `srs` for `domain_size` in `bytes`, checked: the
/// header must name this SRS and size, every point must be on the curve,
/// nothing may follow the last point, and the points must pass
/// [`check_basis`].
pub fn decode<G: AffineRepr>(
    srs: &SRS<G>,
    domain_size: usize,
    bytes: &[u8],
) -> io::Result<Vec<PolyComm<G>>> {
    let mut reader = bytes;
    let mut header = [0u8; 56];
    reader
        .read_exact(&mut header)
        .map_err(|_| invalid("Lagrange basis cache: short header"))?;
    if &header[..8] != MAGIC {
        return Err(invalid("Lagrange basis cache: not a cache file"));
    }
    let size = u64::from_le_bytes(header[8..16].try_into().unwrap());
    let chunks = u64::from_le_bytes(header[16..24].try_into().unwrap());
    if size != domain_size as u64 {
        return Err(invalid(format!(
            "Lagrange basis cache: for a domain of {size}, not {domain_size}"
        )));
    }
    let expected_chunks = domain_size.div_ceil(srs.g.len().max(1)) as u64;
    if chunks != expected_chunks {
        return Err(invalid("Lagrange basis cache: wrong number of chunks"));
    }
    if header[24..56] != srs_digest(srs) {
        return Err(invalid("Lagrange basis cache: made from another SRS"));
    }
    let point_size = G::zero().uncompressed_size();
    if reader.len() as u64 != size * chunks * point_size as u64 {
        return Err(invalid("Lagrange basis cache: wrong length"));
    }
    let domain = Radix2EvaluationDomain::new(domain_size)
        .filter(|domain| domain.size() == domain_size)
        .ok_or_else(|| invalid("Lagrange basis cache: not a domain size"))?;
    let basis = (0..domain_size)
        .map(|_| {
            let chunks = (0..chunks)
                .map(|_| {
                    // Validate::Yes checks that the point is on the curve; the
                    // Pasta curves have cofactor 1, so no other check is needed.
                    G::deserialize_with_mode(&mut reader, Compress::No, Validate::Yes)
                        .map_err(|e| invalid(format!("Lagrange basis cache: {e}")))
                })
                .collect::<io::Result<Vec<G>>>()?;
            Ok(PolyComm { chunks })
        })
        .collect::<io::Result<Vec<_>>>()?;
    check_basis(srs, domain, &basis)?;
    Ok(basis)
}

/// Checks the cache file at `path` against the prover SRS of the curve its
/// name gives, as the prover would before using it. For the web build, before
/// it pins a cache (`mina internal check-lagrange-cache`).
pub fn check_file(path: &Path) -> io::Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let (curve, log2) = parse_name(name)
        .filter(|(_, log2)| *log2 <= 30)
        .ok_or_else(|| invalid(format!("{name}: not a Lagrange basis cache name")))?;
    let bytes = std::fs::read(path)?;
    let domain_size = 1usize << log2;
    // The step proofs (Fp) commit on Vesta, the wraps (Fq) on Pallas.
    match curve {
        "vesta" => {
            let srs = crate::verifier::get_srs_mut::<Fp>();
            let srs = srs.lock().map_err(|_| io::Error::other("SRS lock"))?;
            decode(&srs, domain_size, &bytes).map(drop)
        }
        _ => {
            let srs = crate::verifier::get_srs_mut::<Fq>();
            let srs = srs.lock().map_err(|_| io::Error::other("SRS lock"))?;
            decode(&srs, domain_size, &bytes).map(drop)
        }
    }
}

#[cfg(not(target_family = "wasm"))]
fn read(path: &Path) -> io::Result<Vec<u8>> {
    super::circuit_blobs::read_local(&path)
}

#[cfg(target_family = "wasm")]
fn read(path: &Path) -> io::Result<Vec<u8>> {
    super::circuit_blobs::fetch_blocking(&path)
}

#[cfg(not(target_family = "wasm"))]
fn write<G: CommitmentCurve>(srs: &SRS<G>, domain_size: usize) -> io::Result<()> {
    let path = super::circuit_blobs::home_base_dir()
        .ok_or_else(|| io::Error::other("$HOME is not set"))?
        .join(relative_path::<G>(domain_size));
    let basis = srs.get_lagrange_basis_from_domain_size(domain_size);
    let bytes = encode(srs, &basis);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Write and rename, so that a reader never sees a partial file.
    let partial = path.with_extension("bin.partial");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(partial, path)
}

/// Gives `srs` its Lagrange basis for `domain`: the one in `cached` if it
/// passes [`decode`], otherwise the computed one. The error says why the cache
/// was not used.
fn install<G: CommitmentCurve>(
    srs: &SRS<G>,
    domain: Radix2EvaluationDomain<G::ScalarField>,
    cached: io::Result<Vec<u8>>,
) -> io::Result<()> {
    match cached.and_then(|bytes| decode(srs, domain.size(), &bytes)) {
        Ok(basis) => {
            srs.lagrange_bases().set_once(domain.size(), basis);
            Ok(())
        }
        Err(error) => {
            srs.get_lagrange_basis(domain);
            Err(error)
        }
    }
}

/// Makes the Lagrange basis of `srs` for `domain` available in its cache:
/// from a cache file if there is a valid one, otherwise computed, and then,
/// natively, written for the next start (replacing a refused file).
pub fn prepare<G: CommitmentCurve>(srs: &SRS<G>, domain: Radix2EvaluationDomain<G::ScalarField>) {
    use mina_core::log::system_time;

    let domain_size = domain.size();
    let path = relative_path::<G>(domain_size);
    match install(srs, domain, read(&path)) {
        Ok(()) => {
            mina_core::debug!(system_time(); kind = "ledger proofs", path = path.display().to_string(), message = "Lagrange basis read from its cache");
            return;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            mina_core::warn!(system_time(); kind = "ledger proofs", path = path.display().to_string(), message = "Lagrange basis cache not used", error = error.to_string());
        }
    }
    #[cfg(not(target_family = "wasm"))]
    if let Err(error) = write(srs, domain_size) {
        mina_core::warn!(system_time(); kind = "ledger proofs", message = "cannot store the Lagrange basis cache", error = error.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_poly::Evaluations;
    use ark_serialize::CanonicalSerialize;

    const HEADER: usize = 56;

    fn new_srs() -> SRS<Vesta> {
        SRS::<Vesta>::create(16)
    }

    fn domain(n: usize) -> Radix2EvaluationDomain<Fp> {
        Radix2EvaluationDomain::new(n).unwrap()
    }

    /// The basis as the prover computes it, from an SRS of its own.
    fn computed(n: usize) -> Vec<PolyComm<Vesta>> {
        new_srs().get_lagrange_basis(domain(n)).clone()
    }

    fn point_size() -> usize {
        Vesta::zero().uncompressed_size()
    }

    /// Swaps point records `a` and `b` of an encoded cache.
    fn swap(bytes: &[u8], a: usize, b: usize) -> Vec<u8> {
        let size = point_size();
        let (a, b) = (HEADER + a * size, HEADER + b * size);
        let mut out = bytes.to_vec();
        out[a..a + size].copy_from_slice(&bytes[b..b + size]);
        out[b..b + size].copy_from_slice(&bytes[a..a + size]);
        out
    }

    /// Writes `point` over point record `at` of an encoded cache.
    fn put(bytes: &[u8], at: usize, point: Vesta) -> Vec<u8> {
        let size = point_size();
        let mut record = Vec::new();
        point.serialize_uncompressed(&mut record).unwrap();
        let mut out = bytes.to_vec();
        out[HEADER + at * size..HEADER + (at + 1) * size].copy_from_slice(&record);
        out
    }

    #[test]
    fn round_trip_matches_computed_basis() {
        let srs = new_srs();
        for n in [8, 16, 32] {
            let computed = computed(n);
            let bytes = encode(&srs, &computed);
            assert_eq!(decode(&srs, n, &bytes).unwrap(), computed);

            // A fresh SRS given the decoded basis returns it without computing.
            let fresh = new_srs();
            fresh
                .lagrange_bases()
                .set_once(n, decode(&fresh, n, &bytes).unwrap());
            assert_eq!(*fresh.get_lagrange_basis_from_domain_size(n), computed);
        }
    }

    #[test]
    fn refuses_other_files() {
        let srs = new_srs();
        let bytes = encode(&srs, &computed(16));
        let refused = |bytes: &[u8], n: usize| decode(&srs, n, bytes).is_err();
        assert!(!refused(&bytes, 16));
        // Another domain size, truncated, extended, wrong magic.
        assert!(refused(&bytes, 8));
        assert!(refused(&bytes[..bytes.len() - 1], 16));
        assert!(refused(&[&bytes[..], &[0]].concat(), 16));
        let mut magic = bytes.clone();
        magic[0] ^= 1;
        assert!(refused(&magic, 16));
        // Another SRS: its digest differs.
        let mut digest = bytes.clone();
        digest[30] ^= 1;
        assert!(refused(&digest, 16));
        // A point moved off the curve.
        let mut point = bytes.clone();
        point[HEADER + 3] ^= 1;
        assert!(refused(&point, 16));
        // A basis for the other curve's SRS.
        let pallas = SRS::<Pallas>::create(16);
        let domain = Radix2EvaluationDomain::new(16).unwrap();
        let other = encode(&pallas, &pallas.get_lagrange_basis(domain));
        assert!(refused(&other, 16));
    }

    /// Valid curve points, the right header and length, but not the basis:
    /// only `check_basis` can refuse these.
    #[test]
    fn refuses_valid_points_that_are_not_the_basis() {
        let srs = new_srs();
        for n in [8, 16, 32] {
            let basis = computed(n);
            let chunks = basis[0].chunks.len();
            let points = n * chunks;
            let bytes = encode(&srs, &basis);
            let mut wrong = vec![
                // Two commitments swapped (the review's reproduction).
                swap(&bytes, 0, chunks),
                swap(&bytes, points - chunks, points - 2 * chunks),
                // One point replaced by another basis point, by its negation,
                // by its double, by an SRS point.
                put(&bytes, 3, basis[5].chunks[0]),
                put(&bytes, 0, -basis[0].chunks[0]),
                put(&bytes, 1, (basis[0].chunks[0] + basis[0].chunks[0]).into()),
                put(&bytes, points - 1, srs.g[0]),
            ];
            if chunks > 1 {
                // The two chunks of one commitment swapped, and the second
                // chunks of two commitments swapped. With twice the SRS's
                // points, chunk 1 of L_i is (-1)^i times chunk 0, so L_1's
                // differ and L_0's do not.
                wrong.push(swap(&bytes, chunks, chunks + 1));
                wrong.push(swap(&bytes, 1, chunks + 1));
            }
            for (case, wrong) in wrong.iter().enumerate() {
                assert_ne!(wrong, &bytes, "n {n}, case {case} changes nothing");
                let error = decode(&srs, n, wrong).unwrap_err();
                assert!(
                    error.to_string().contains("not the Lagrange basis"),
                    "n {n}, case {case}: {error}"
                );
            }
        }
    }

    /// A refused cache leaves the SRS with the computed basis, so that every
    /// commitment is the same as without a cache.
    #[test]
    fn install_falls_back_to_the_computed_basis() {
        let n = 16;
        let trusted = new_srs();
        let basis = computed(n);
        let bytes = encode(&trusted, &basis);
        let mut evaluations = vec![Fp::from(0u64); n];
        evaluations[0] = Fp::from(1u64);
        let evaluations = Evaluations::from_vec_and_domain(evaluations, domain(n));
        let expected = trusted.commit_evaluations_non_hiding(domain(n), &evaluations);

        let cases: [(io::Result<Vec<u8>>, bool); 4] = [
            (Ok(bytes.clone()), true),
            (Ok(swap(&bytes, 0, 1)), false),
            (Ok(bytes[..100].to_vec()), false),
            (Err(io::ErrorKind::NotFound.into()), false),
        ];
        for (case, (cached, used)) in cases.into_iter().enumerate() {
            let srs = new_srs();
            assert_eq!(
                install(&srs, domain(n), cached).is_ok(),
                used,
                "case {case}"
            );
            assert_eq!(
                *srs.get_lagrange_basis_from_domain_size(n),
                basis,
                "case {case}"
            );
            assert_eq!(
                srs.commit_evaluations_non_hiding(domain(n), &evaluations),
                expected,
                "case {case}"
            );
        }
    }

    #[test]
    fn names() {
        assert_eq!(file_name::<Vesta>(1 << 16), "lagrange_basis_vesta_16.bin");
        assert_eq!(file_name::<Pallas>(1 << 15), "lagrange_basis_pallas_15.bin");
        assert_eq!(
            parse_name("lagrange_basis_vesta_16.bin"),
            Some(("vesta", 16))
        );
        assert_eq!(
            parse_name("lagrange_basis_pallas_14.bin"),
            Some(("pallas", 14))
        );
        for name in [
            "lagrange_basis_other_16.bin",
            "lagrange_basis_vesta_.bin",
            "lagrange_basis_vesta_123.bin",
            "lagrange_basis_vesta_16.bin.partial",
        ] {
            assert_eq!(parse_name(name), None, "{name}");
        }
    }
}
