//! Load a blockchain verifier index from JSON for any network.
//!
//! Adapted from `MinaProtocol/mina-verify` (`crates/mina-verify/src/verifier_index.rs`,
//! Apache-2.0). mina-tree's own finalization (`make_verifier_index`) is private, so it
//! is mirrored here with public kimchi APIs.

use std::sync::{Arc, OnceLock};

use kimchi::{
    circuits::{
        constraints::FeatureFlags,
        expr::Linearization,
        lookup::lookups::{LookupFeatures, LookupPatterns},
        polynomials::permutation::{permutation_vanishing_polynomial, zk_w},
    },
    linearization::expr_linearization,
};
use ledger::proofs::{transaction::endos, VerifierIndex};
use mina_curves::pasta::Fq;
use poly_commitment::{ipa::SRS, SRS as _};

/// Supplies the `#[serde(skip)]` fields of a parsed index; [`finalize`] recomputes all
/// of them, so any valid index works as the template.
const TEMPLATE_INDEX_JSON: &str =
    include_str!("../../ledger/src/proofs/data/devnet_blockchain_verifier_index.json");

/// Parse and finalize a mina-tree verifier-index JSON (hex-string fields).
pub fn verifier_index_from_json(json: &str) -> Result<VerifierIndex<Fq>, serde_json::Error> {
    let index: VerifierIndex<Fq> = serde_json::from_str(json)?;
    Ok(finalize(index))
}

/// Parse either the OCaml `Pickles.Verification_key` JSON
/// (`{ "commitments", "index", "data" }`, as returned by the GraphQL
/// `blockchainVerificationKey` query) or a mina-tree verifier-index JSON.
pub fn verifier_index_auto(json: &str) -> Result<VerifierIndex<Fq>, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if value.get("commitments").is_some() && value.get("index").is_some() {
        pickles::verifier_index_from_pickles_json(json)
    } else {
        verifier_index_from_json(json).map_err(|e| e.to_string())
    }
}

fn once_lock_with<T>(value: T) -> OnceLock<T> {
    let cell = OnceLock::new();
    let _ = cell.set(value);
    cell
}

/// Attach the SRS and Lagrange basis, endomorphism, linearization and permutation
/// polynomials to a parsed index.
fn finalize(index: VerifierIndex<Fq>) -> VerifierIndex<Fq> {
    let domain = index.domain;
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
    linearization
        .index_terms
        .sort_by_key(|&(columns, _)| columns);
    let linearization = Linearization {
        constant_term: linearization.constant_term,
        index_terms: linearization.index_terms,
    };

    let srs = SRS::create(index.max_poly_size);
    srs.get_lagrange_basis(domain);

    let permutation_vanishing_polynomial_m =
        permutation_vanishing_polynomial(domain, index.zk_rows);
    let w = zk_w(domain, index.zk_rows);

    VerifierIndex::<Fq> {
        srs: Arc::new(srs),
        permutation_vanishing_polynomial_m: once_lock_with(permutation_vanishing_polynomial_m),
        w: once_lock_with(w),
        endo,
        linearization,
        powers_of_alpha,
        ..index
    }
}

/// Ingest the OCaml `Pickles.Verification_key.to_yojson` form.
///
/// Field elements are canonical big-endian hex (mina-tree uses little-endian) and
/// curve points are affine `[x, y]` pairs. The blockchain circuit uses no optional
/// gates or lookups, so those commitments are absent.
mod pickles {
    use ark_ff::PrimeField;
    use ark_poly::{EvaluationDomain, Radix2EvaluationDomain};
    use kimchi::circuits::wires::{COLUMNS, PERMUTS};
    use ledger::proofs::VerifierIndex;
    use mina_curves::pasta::{Fp, Fq, Pallas};
    use poly_commitment::commitment::PolyComm;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct PicklesVk {
        commitments: Commitments,
        index: Index,
    }

    #[derive(Deserialize)]
    struct Commitments {
        sigma_comm: Vec<[String; 2]>,
        coefficients_comm: Vec<[String; 2]>,
        generic_comm: [String; 2],
        psm_comm: [String; 2],
        complete_add_comm: [String; 2],
        mul_comm: [String; 2],
        emul_comm: [String; 2],
        endomul_scalar_comm: [String; 2],
    }

    #[derive(Deserialize)]
    struct Index {
        domain: Domain,
        max_poly_size: usize,
        public: usize,
        prev_challenges: usize,
        zk_rows: u64,
        shifts: Vec<String>,
    }

    #[derive(Deserialize)]
    struct Domain {
        log_size_of_group: u32,
    }

    fn field<F: PrimeField>(s: &str) -> Result<F, String> {
        let bytes = hex::decode(s.trim_start_matches("0x")).map_err(|e| e.to_string())?;
        Ok(F::from_be_bytes_mod_order(&bytes))
    }

    fn comm(xy: &[String; 2]) -> Result<PolyComm<Pallas>, String> {
        let point = Pallas::new_unchecked(field::<Fp>(&xy[0])?, field::<Fp>(&xy[1])?);
        if !point.is_on_curve() || !point.is_in_correct_subgroup_assuming_on_curve() {
            return Err("commitment point is not on the curve".to_string());
        }
        Ok(PolyComm {
            chunks: vec![point],
        })
    }

    fn comms<const N: usize>(points: &[[String; 2]]) -> Result<[PolyComm<Pallas>; N], String> {
        points
            .iter()
            .map(comm)
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| format!("expected {N} commitments"))
    }

    pub fn verifier_index_from_pickles_json(json: &str) -> Result<VerifierIndex<Fq>, String> {
        let vk: PicklesVk = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let template: VerifierIndex<Fq> =
            serde_json::from_str(super::TEMPLATE_INDEX_JSON).map_err(|e| e.to_string())?;

        let domain = Radix2EvaluationDomain::<Fq>::new(1usize << vk.index.domain.log_size_of_group)
            .ok_or_else(|| "invalid domain size".to_string())?;
        let shift: [Fq; PERMUTS] = vk
            .index
            .shifts
            .iter()
            .map(|s| field::<Fq>(s))
            .collect::<Result<Vec<_>, _>>()?
            .try_into()
            .map_err(|_| format!("expected {PERMUTS} shifts"))?;

        let index = VerifierIndex::<Fq> {
            domain,
            max_poly_size: vk.index.max_poly_size,
            zk_rows: vk.index.zk_rows,
            public: vk.index.public,
            prev_challenges: vk.index.prev_challenges,
            sigma_comm: comms::<PERMUTS>(&vk.commitments.sigma_comm)?,
            coefficients_comm: comms::<COLUMNS>(&vk.commitments.coefficients_comm)?,
            generic_comm: comm(&vk.commitments.generic_comm)?,
            psm_comm: comm(&vk.commitments.psm_comm)?,
            complete_add_comm: comm(&vk.commitments.complete_add_comm)?,
            mul_comm: comm(&vk.commitments.mul_comm)?,
            emul_comm: comm(&vk.commitments.emul_comm)?,
            endomul_scalar_comm: comm(&vk.commitments.endomul_scalar_comm)?,
            range_check0_comm: None,
            range_check1_comm: None,
            foreign_field_add_comm: None,
            foreign_field_mul_comm: None,
            xor_comm: None,
            rot_comm: None,
            shift,
            lookup_index: None,
            ..template
        };
        Ok(super::finalize(index))
    }
}
