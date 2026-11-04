//! Integrity policy for hosted circuit assets. Prover digests come from the
//! canonical OCaml manifests on both native and WebAssembly targets.

use std::{collections::BTreeMap, path::Path};

const DEVNET: &str = include_str!("../../../../tools/mesa-circuits/devnet-blobs.sha256");
const MAINNET: &str = include_str!("../../../../tools/mesa-circuits/mainnet-blobs.sha256");

/// Only prover files in the manifests, the two optional verifier caches and
/// the optional Lagrange basis caches are hosted circuit assets. Cache pins describe this build's serialized
/// indices; prover pins must also agree with the canonical manifests.
pub(super) fn canonical_pin(filename: &str) -> std::io::Result<Option<&'static str>> {
    use std::path::Component;

    let mut parts = Path::new(filename).components();
    let parsed = match (parts.next(), parts.next(), parts.next()) {
        (Some(Component::Normal(directory)), Some(Component::Normal(name)), None) => {
            directory.to_str().zip(name.to_str())
        }
        _ => None,
    };
    if let Some((directory, name)) = parsed {
        let manifest = match directory {
            "4.0.2devnet" => Some(DEVNET),
            "4.0.2mainnet" => Some(MAINNET),
            _ => None,
        };
        if let Some(manifest) = manifest {
            for line in manifest.lines() {
                if let Some((digest, entry)) = line.split_once("  ") {
                    if entry == name {
                        return Ok(Some(digest));
                    }
                }
            }
            if matches!(
                name,
                "block_verifier_index.postcard" | "transaction_verifier_index.postcard"
            ) || is_lagrange_cache(name)
            {
                return Ok(None);
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("circuit file {filename} refused: no canonical SHA-256 for this file"),
    ))
}

/// Whether `name` is a Lagrange basis cache, `lagrange_basis_<curve>_<log2
/// n>.bin` (`super::lagrange_cache`). Like the verifier caches, it has a build
/// pin and no canonical one.
fn is_lagrange_cache(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix("lagrange_basis_")
        .and_then(|rest| rest.strip_suffix(".bin"))
    else {
        return false;
    };
    let Some((curve, log2)) = rest.split_once('_') else {
        return false;
    };
    matches!(curve, "pallas" | "vesta")
        && (1..=2).contains(&log2.len())
        && log2.bytes().all(|b| b.is_ascii_digit())
}

/// Build pins for every hosted asset, including same-origin assets. Prover
/// pins must agree with the canonical manifests; hashing staged files alone
/// cannot establish their authenticity. Verifier caches retain build pins.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CircuitPins(BTreeMap<String, [u8; 32]>);

impl CircuitPins {
    #[cfg(any(test, target_family = "wasm"))]
    pub(super) fn from_build(value: Option<&str>) -> Result<Self, String> {
        value
            .ok_or_else(|| "this build has no circuit file SHA-256 pins".to_owned())
            .and_then(Self::parse)
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let mut pins = BTreeMap::new();
        for entry in s.split(';').map(str::trim) {
            if entry.is_empty() || entry == "-" {
                continue;
            }
            let (file, hex_digest) = entry
                .rsplit_once('=')
                .ok_or_else(|| format!("expected <file>=<sha256>, got `{entry}`"))?;
            let mut digest = [0u8; 32];
            hex::decode_to_slice(hex_digest, &mut digest)
                .map_err(|err| format!("bad SHA-256 for {file}: {err}"))?;
            if pins.insert(file.to_owned(), digest).is_some() {
                return Err(format!("{file} is pinned twice"));
            }
        }
        Ok(Self(pins))
    }

    /// Require a build pin and, for a prover file, the canonical OCaml pin.
    pub fn check_hosted_pin(&self, filename: &str) -> std::io::Result<()> {
        let expected = canonical_pin(filename)?;
        let pinned = self.0.get(filename).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("circuit file {filename} refused: this build pins no SHA-256 for it"),
            )
        })?;
        if let Some(expected) = expected {
            if hex::encode(pinned) != expected {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("circuit file {filename} refused: this build's SHA-256 does not match the canonical manifest ({expected})"),
                ));
            }
        }
        Ok(())
    }

    /// Check hosted bytes before any decoding or construction of a prover.
    pub fn check_hosted(&self, filename: &str, bytes: &[u8]) -> std::io::Result<()> {
        self.check_hosted_pin(filename)?;
        self.check(filename, bytes)
    }

    /// Refuses `bytes` unless they are `filename`'s pinned bytes.
    pub fn check(&self, filename: &str, bytes: &[u8]) -> std::io::Result<()> {
        use sha2::{Digest, Sha256};

        let refuse = |why: String| {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("circuit file {filename} refused: {why}"),
            ))
        };
        let Some(pinned) = self.0.get(filename) else {
            return refuse("this build pins no SHA-256 for it".to_owned());
        };
        let digest = Sha256::digest(bytes);
        if digest.as_slice() != pinned.as_slice() {
            return refuse(format!(
                "its SHA-256 is {}, this build pins {}",
                hex::encode(digest),
                hex::encode(pinned)
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MERGE: &str = "step-step-proving-key-transaction-snark-merge-1-f970ffc5f94017c2d8b47516c890c21f_rows_rev.bin";

    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn canonical_pins_match_both_ocaml_manifests() {
        for (directory, manifest) in [("4.0.2devnet", DEVNET), ("4.0.2mainnet", MAINNET)] {
            assert_eq!(manifest.lines().count(), 24);
            for line in manifest.lines() {
                let (digest, name) = line.split_once("  ").unwrap();
                let file = format!("{directory}/{name}");
                assert_eq!(canonical_pin(&file).unwrap(), Some(digest));
                CircuitPins::parse(&format!("{file}={digest}"))
                    .unwrap()
                    .check_hosted_pin(&file)
                    .unwrap();
            }
        }
    }

    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn changed_prover_bytes_and_self_signed_build_pins_are_refused() {
        for directory in ["4.0.2devnet", "4.0.2mainnet"] {
            let file = format!("{directory}/{MERGE}");
            let expected = canonical_pin(&file).unwrap().unwrap();
            let pins = CircuitPins::parse(&format!("{file}={expected}")).unwrap();
            let error = pins.check_hosted(&file, b"changed prover").unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
            assert!(error.to_string().contains("its SHA-256"));
            let pins = CircuitPins::parse(&format!("{file}={}", "00".repeat(32))).unwrap();
            assert!(pins
                .check_hosted_pin(&file)
                .unwrap_err()
                .to_string()
                .contains("canonical manifest"));
        }
    }

    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn missing_and_malformed_build_pins_are_refused() {
        assert!(CircuitPins::from_build(None)
            .unwrap_err()
            .contains("no circuit file"));
        assert!(CircuitPins::from_build(Some("broken")).is_err());
        for value in ["", "-"] {
            let pins = CircuitPins::from_build(Some(value)).unwrap();
            for directory in ["4.0.2devnet", "4.0.2mainnet"] {
                let file = format!("{directory}/{MERGE}");
                assert!(pins
                    .check_hosted_pin(&file)
                    .unwrap_err()
                    .to_string()
                    .contains("pins no SHA-256"));
            }
        }
    }

    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn unknown_files_and_networks_are_refused_even_with_build_pins() {
        for file in [
            "4.0.2devnet/unknown_gates.json",
            "4.0.2mainnet/unknown_rows_rev.bin",
            "other/unknown.bin",
            "4.0.2devnet/../unknown.bin",
            "/4.0.2mainnet/unknown.bin",
        ] {
            let pins = CircuitPins::parse(&format!("{file}={}", "00".repeat(32))).unwrap();
            assert!(pins
                .check_hosted(file, b"unknown")
                .unwrap_err()
                .to_string()
                .contains("no canonical SHA-256"));
        }
    }

    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn lagrange_caches_need_a_build_pin() {
        const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        for file in [
            "4.0.2mainnet/lagrange_basis_vesta_16.bin",
            "4.0.2devnet/lagrange_basis_pallas_15.bin",
        ] {
            assert_eq!(canonical_pin(file).unwrap(), None);
            let pins = CircuitPins::parse(&format!("{file}={DIGEST}")).unwrap();
            pins.check_hosted(file, b"abc").unwrap();
            assert!(pins.check_hosted(file, b"abd").is_err());
            assert!(CircuitPins::default().check_hosted(file, b"abc").is_err());
        }
        for name in [
            "lagrange_basis_other_16.bin",
            "lagrange_basis_vesta_.bin",
            "lagrange_basis_vesta_1x.bin",
            "lagrange_basis_vesta_123.bin",
            "lagrange_basis_vesta_16.bin.partial",
            "lagrange_basis_vesta16.bin",
        ] {
            assert!(
                canonical_pin(&format!("4.0.2mainnet/{name}")).is_err(),
                "{name}"
            );
        }
    }

    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn verifier_cache_keeps_build_integrity_check() {
        // SHA-256's standard "abc" vector; caches have build-specific bytes.
        const DIGEST: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let file = "4.0.2devnet/block_verifier_index.postcard";
        assert_eq!(canonical_pin(file).unwrap(), None);
        let pins = CircuitPins::parse(&format!("{file}={DIGEST}")).unwrap();
        pins.check_hosted(file, b"abc").unwrap();
        assert!(pins.check_hosted(file, b"abd").is_err());
        assert!(CircuitPins::default().check_hosted(file, b"abc").is_err());
    }
}
