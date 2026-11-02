#[allow(dead_code)]
#[path = "../../../../crates/ledger/src/proofs/circuit_pins.rs"]
mod circuit_pins;

#[cfg(test)]
mod canonical_bytes {
    use super::circuit_pins::CircuitPins;
    use std::io::Read;
    #[cfg_attr(not(target_family = "wasm"), test)]
    #[cfg_attr(target_family = "wasm", wasm_bindgen_test::wasm_bindgen_test)]
    fn canonical_ocaml_bytes_pass_on_both_networks() {
        for network in ["devnet", "mainnet"] {
            let manifest = if network == "devnet" {
                include_str!("../../../../tools/mesa-circuits/devnet-blobs.sha256")
            } else {
                include_str!("../../../../tools/mesa-circuits/mainnet-blobs.sha256")
            };
            let (digest, name) = manifest.lines().next().unwrap().split_once("  ").unwrap();
            let frame: &[u8] = if network == "devnet" {
                include_bytes!("../../../../crates/ledger/src/proofs/data/circuits/4.0.2devnet/step-step-proving-key-blockchain-snark-step-0-31e6069d62eefad69d12d2aa8c171c0c_gates.json.zst")
            } else {
                include_bytes!("../../../../crates/ledger/src/proofs/data/circuits/4.0.2mainnet/step-step-proving-key-blockchain-snark-step-0-12336df97ce8f76e9ed70f921cb69031_gates.json.zst")
            };
            let mut bytes = Vec::new();
            ruzstd::decoding::StreamingDecoder::new(frame)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            let filename = format!("4.0.2{network}/{name}");
            CircuitPins::parse(&format!("{filename}={digest}"))
                .unwrap()
                .check_hosted(&filename, &bytes)
                .unwrap();
        }
    }
}
