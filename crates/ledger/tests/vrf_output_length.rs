//! A block's VRF output is a byte string of any length on the wire. Its proof
//! is checked against the state hash OCaml computes for it, which reads the
//! first 253 bits, and converting it for the blockchain circuit fails cleanly
//! when it is shorter.

use mina_curves::pasta::Fp;
use mina_p2p_messages::{
    bigint::InvalidBigInt,
    binprot::BinProtRead,
    v2::{ConsensusVrfOutputTruncatedStableV1, MinaBlockBlockStableV2, MinaBlockHeaderStableV2},
};
use mina_tree::{
    proofs::{
        block::ProtocolState,
        verification::verify_block,
        verifiers::{block_verifier_index_of, BlockVerifier},
    },
    verifier::get_srs,
};

/// Mainnet block 563978 as an OCaml node gossiped it: an 8-byte length, the
/// `NewState` tag, then the block.
fn header() -> MinaBlockHeaderStableV2 {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../web-verifier/tests/fixtures/mainnet-563978.gossipbin"
    );
    let bytes = std::fs::read(path).unwrap();
    MinaBlockBlockStableV2::binprot_read(&mut &bytes[9..])
        .unwrap()
        .header
}

fn vrf_output(header: &MinaBlockHeaderStableV2) -> Vec<u8> {
    header
        .protocol_state
        .body
        .consensus_state
        .last_vrf_output
        .to_vec()
}

fn with_vrf_output(header: &MinaBlockHeaderStableV2, bytes: Vec<u8>) -> MinaBlockHeaderStableV2 {
    let mut header = header.clone();
    header.protocol_state.body.consensus_state.last_vrf_output =
        ConsensusVrfOutputTruncatedStableV1(bytes.into());
    header
}

/// Outputs that OCaml hashes as the block's own: the top 3 bits of byte 31
/// set, and bytes past the 32nd.
fn same_hash_variants(output: &[u8]) -> Vec<Vec<u8>> {
    assert_eq!(output.len(), 32);
    assert_eq!(output[31] >> 5, 0);
    let mut top_bits = output.to_vec();
    top_bits[31] |= 0xE0;
    vec![
        top_bits.clone(),
        [output, &[0xAA][..]].concat(),
        [output, &[0xFF; 32][..]].concat(),
        [&top_bits[..], &[0xFF; 100][..]].concat(),
    ]
}

fn short_variants(output: &[u8]) -> Vec<Vec<u8>> {
    [0, 1, 31]
        .into_iter()
        .map(|len| output[..len].to_vec())
        .collect()
}

/// The protocol state as the blockchain circuit takes it.
fn converted(header: &MinaBlockHeaderStableV2) -> Result<ProtocolState, InvalidBigInt> {
    ProtocolState::try_from(&header.protocol_state)
}

fn state_hash(header: &MinaBlockHeaderStableV2) -> Fp {
    header
        .protocol_state
        .try_hash()
        .unwrap()
        .to_field()
        .unwrap()
}

#[test]
fn a_short_vrf_output_is_a_circuit_conversion_error() {
    let header = header();
    for bytes in short_variants(&vrf_output(&header)) {
        let len = bytes.len();
        let short = with_vrf_output(&header, bytes);
        assert!(converted(&short).is_err(), "{len} bytes");
    }
}

/// The circuit takes the first 253 bits, as OCaml's `Truncated.typ` does,
/// and its protocol state hashes as the wire type does.
#[test]
fn the_circuit_takes_the_first_253_bits() {
    let header = header();
    let state = converted(&header).unwrap();
    assert_eq!(state.hashes().0, state_hash(&header));
    for bytes in same_hash_variants(&vrf_output(&header)) {
        let variant = with_vrf_output(&header, bytes);
        let variant_state = converted(&variant).unwrap();
        assert_eq!(
            variant_state.body.consensus_state.last_vrf_output,
            state.body.consensus_state.last_vrf_output
        );
        assert_eq!(variant_state.hashes().0, state.hashes().0);
        assert_eq!(state_hash(&variant), state_hash(&header));
    }
}

/// OCaml checks a block's proof against its state hash, so it accepts the
/// block's proof with any output that hashes as the block's own. With a
/// shorter output the hash differs and the proof fails, without a panic.
#[test]
fn a_block_proof_is_checked_against_the_state_hash_ocaml_computes() {
    let verifier = BlockVerifier::from_index(block_verifier_index_of("mainnet").unwrap());
    let srs = get_srs::<Fp>();
    let header = header();
    assert!(verify_block(&header, &verifier, &srs));
    let output = vrf_output(&header);
    for bytes in same_hash_variants(&output) {
        let len = bytes.len();
        let variant = with_vrf_output(&header, bytes);
        assert!(verify_block(&variant, &verifier, &srs), "{len} bytes");
    }
    for bytes in short_variants(&output) {
        let len = bytes.len();
        let short = with_vrf_output(&header, bytes);
        assert!(!verify_block(&short, &verifier, &srs), "{len} bytes");
    }
}
