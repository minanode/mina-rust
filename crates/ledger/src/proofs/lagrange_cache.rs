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
//!   SHA-256 like the verifier index caches.
//!
//! A cache is used only if it names this SRS (its SHA-256 over the SRS points)
//! and the domain size, and every point is on the curve. Anything else, a
//! missing file included, falls back to computing the basis.

use std::{
    any::TypeId,
    io::{self, Read},
    path::{Path, PathBuf},
};

use ark_ec::AffineRepr;
use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
use ark_serialize::{Compress, Validate};
use mina_curves::pasta::{Pallas, Vesta};
use poly_commitment::{commitment::PolyComm, ipa::SRS, SRS as _};
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

/// The Lagrange basis of `srs` for `domain_size` in `bytes`, checked: the
/// header must name this SRS and size, every point must be on the curve, and
/// nothing may follow the last point.
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
    (0..domain_size)
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
        .collect()
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
fn write<G: poly_commitment::commitment::CommitmentCurve>(
    srs: &SRS<G>,
    domain_size: usize,
) -> io::Result<()> {
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

/// Makes the Lagrange basis of `srs` for `domain` available in its cache:
/// from a cache file if there is a valid one, otherwise computed, and then,
/// natively, written for the next start.
pub fn prepare<G: AffineRepr + poly_commitment::commitment::CommitmentCurve>(
    srs: &SRS<G>,
    domain: Radix2EvaluationDomain<G::ScalarField>,
) {
    use mina_core::log::system_time;

    let domain_size = domain.size();
    let path = relative_path::<G>(domain_size);
    match read(&path).and_then(|bytes| decode(srs, domain_size, &bytes)) {
        Ok(basis) => {
            srs.lagrange_bases().set_once(domain_size, basis);
            mina_core::debug!(system_time(); kind = "ledger proofs", path = path.display().to_string(), message = "Lagrange basis read from its cache");
            return;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            mina_core::warn!(system_time(); kind = "ledger proofs", path = path.display().to_string(), message = "Lagrange basis cache not used", error = error.to_string());
        }
    }
    srs.get_lagrange_basis(domain);
    #[cfg(not(target_family = "wasm"))]
    if let Err(error) = write(srs, domain_size) {
        mina_core::warn!(system_time(); kind = "ledger proofs", message = "cannot store the Lagrange basis cache", error = error.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_srs() -> SRS<Vesta> {
        SRS::<Vesta>::create(16)
    }

    fn basis(srs: &SRS<Vesta>, n: usize) -> Vec<PolyComm<Vesta>> {
        let domain = Radix2EvaluationDomain::new(n).unwrap();
        srs.get_lagrange_basis(domain).clone()
    }

    #[test]
    fn round_trip_matches_computed_basis() {
        let srs = new_srs();
        for n in [8, 16, 32] {
            let computed = basis(&srs, n);
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
        let bytes = encode(&srs, &basis(&srs, 16));
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
        point[56 + 3] ^= 1;
        assert!(refused(&point, 16));
        // A basis for the other curve's SRS.
        let pallas = SRS::<Pallas>::create(16);
        let domain = Radix2EvaluationDomain::new(16).unwrap();
        let other = encode(&pallas, &pallas.get_lagrange_basis(domain));
        assert!(refused(&other, 16));
    }

    #[test]
    fn names() {
        assert_eq!(file_name::<Vesta>(1 << 16), "lagrange_basis_vesta_16.bin");
        assert_eq!(file_name::<Pallas>(1 << 15), "lagrange_basis_pallas_15.bin");
    }
}
