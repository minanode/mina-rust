//! P6a: we must never forward gossip that OCaml can't decode, because OCaml
//! bans the peer that sends it such a message (and its IP) for a day. These
//! tests feed `decode_pubsub_data` the shapes a peer could wrap genuine
//! transactions, snark work and blocks in, and check that we accept exactly
//! what OCaml's bin_prot v0.14.0 reader accepts: malformed framing and invalid
//! codes are refused, non-minimal integers and trailing bytes are not.

use binprot::{BinProtRead, BinProtWrite};
use mina_p2p_messages::{
    gossip::{decode_pubsub_data, GossipDecodeError, GossipNetMessageV2},
    list::List,
    number::Number,
    rpc::GetBestTipV2Response,
    rpc_kernel::ResponsePayload,
    v2::{
        MinaBaseFeeWithProverStableV1, MinaBaseUserCommandStableV2, MinaBlockBlockStableV2,
        MinaStateBlockchainStateValueStableV2LedgerProofStatement,
        MinaStateSnarkedLedgerStateStableV2, NetworkPoolSnarkPoolDiffVersionedStableV2,
        NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1,
        NetworkPoolTransactionPoolDiffVersionedStableV2, TransactionSnarkWorkStatementStableV2,
        TransactionSnarkWorkTStableV2Proofs, UnsignedExtendedUInt32StableV1,
    },
};

fn fixture<T: BinProtRead>(name: &str) -> T {
    let path = format!("{}/tests/files/mesa/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let payload = ResponsePayload::<T>::binprot_read(&mut &bytes[..]).unwrap();
    payload.0.expect("ok response").0
}

/// Mesa mainnet user commands, from the captured blocks 564003 and 563900.
fn mesa_commands() -> Vec<MinaBaseUserCommandStableV2> {
    let tip = fixture::<GetBestTipV2Response>("get_best_tip_v3.bin").expect("a best tip");
    let chain: List<MinaBlockBlockStableV2> =
        fixture::<Option<List<MinaBlockBlockStableV2>>>("get_transition_chain_v2_563900_zkapp.bin")
            .expect("some blocks");
    std::iter::once(tip.data)
        .chain(chain)
        .flat_map(|block| {
            let diff = block.body.staged_ledger_diff.diff.clone();
            diff.0
                .commands
                .into_iter()
                .chain(diff.1.into_iter().flat_map(|d| d.commands))
                .map(|c| c.data)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn mesa_signed_command() -> MinaBaseUserCommandStableV2 {
    mesa_commands()
        .into_iter()
        .find(|c| matches!(c, MinaBaseUserCommandStableV2::SignedCommand(_)))
        .expect("a signed command")
}

fn mesa_zkapp_command() -> MinaBaseUserCommandStableV2 {
    mesa_commands()
        .into_iter()
        .find(|c| matches!(c, MinaBaseUserCommandStableV2::ZkappCommand(_)))
        .expect("a zkApp command")
}

fn diff_of(commands: Vec<MinaBaseUserCommandStableV2>, nonce: u64) -> GossipNetMessageV2 {
    GossipNetMessageV2::TransactionPoolDiff {
        message: NetworkPoolTransactionPoolDiffVersionedStableV2(commands.into_iter().collect()),
        nonce: Number::from(nonce),
    }
}

/// A transaction pool diff with one signed Mesa payment, without the size
/// header.
fn tx_pool_diff() -> Vec<u8> {
    encode(&tx_diff())
}

/// Pubsub data as OCaml writes it: an 8-byte little-endian length, then the
/// bin_prot body.
fn framed(header_len: u64, body: &[u8]) -> Vec<u8> {
    let mut data = header_len.to_le_bytes().to_vec();
    data.extend_from_slice(body);
    data
}

fn encode(message: &GossipNetMessageV2) -> Vec<u8> {
    let mut bytes = Vec::new();
    message.binprot_write(&mut bytes).unwrap();
    bytes
}

fn tx_diff() -> GossipNetMessageV2 {
    diff_of(vec![mesa_signed_command()], 0)
}

/// The diff with its one signed command's fee payer nonce set to `nonce`.
fn tx_diff_with_nonce(nonce: u32) -> Vec<u8> {
    let mut message = tx_diff();
    let GossipNetMessageV2::TransactionPoolDiff { message: diff, .. } = &mut message else {
        panic!("the fixture is a transaction pool diff");
    };
    let MinaBaseUserCommandStableV2::SignedCommand(cmd) = diff.0.front_mut().unwrap() else {
        panic!("the fixture holds a signed command");
    };
    cmd.payload.common.nonce = UnsignedExtendedUInt32StableV1(Number(nonce));
    encode(&message)
}

/// Where the fee payer nonce is in the encoded diff, when it is one byte.
fn nonce_offset() -> usize {
    let a = tx_diff_with_nonce(5);
    let b = tx_diff_with_nonce(6);
    assert_eq!(a.len(), b.len());
    let diffs: Vec<usize> = (0..a.len()).filter(|i| a[*i] != b[*i]).collect();
    assert_eq!(diffs.len(), 1, "the nonce is one byte");
    assert_eq!(a[diffs[0]], 5);
    diffs[0]
}

#[test]
fn accepts_the_unchanged_diff() {
    let body = tx_pool_diff();
    let data = framed(body.len() as u64, &body);
    let message = decode_pubsub_data(&data).unwrap();
    assert_eq!(message, tx_diff());
}

/// A zkApp with Mesa's 32-field state, several commands in one diff, and the
/// nonce OCaml's rebroadcast uses (the time in nanoseconds) all pass.
#[test]
fn accepts_zkapps_multi_command_diffs_and_large_nonces() {
    for message in [
        diff_of(vec![mesa_zkapp_command()], 0),
        diff_of(mesa_commands(), 1_792_383_635_000_000_000),
        diff_of(vec![], 7),
    ] {
        let body = encode(&message);
        let data = framed(body.len() as u64, &body);
        assert_eq!(decode_pubsub_data(&data).unwrap(), message);
    }
}

#[test]
fn refuses_a_header_claiming_more_bytes_than_the_body_padded_with_junk() {
    // The header covers the body and 4 junk bytes, so the reader stops 4
    // bytes short of where OCaml requires it to end.
    let mut body = tx_pool_diff();
    body.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    let data = framed(body.len() as u64, &body);
    assert_eq!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::LengthMismatch {
            consumed: tx_pool_diff().len(),
            claimed: body.len(),
        })
    );
}

#[test]
fn refuses_a_header_claiming_more_bytes_than_there_are() {
    let data = framed(tx_pool_diff().len() as u64 + 100, &tx_pool_diff());
    assert!(matches!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::SizeHeaderTooLarge { .. })
    ));
    let data = framed(u64::MAX, &tx_pool_diff());
    assert!(matches!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::SizeHeaderTooLarge { .. })
    ));
}

#[test]
fn refuses_a_header_claiming_fewer_bytes_than_the_body() {
    let data = framed(tx_pool_diff().len() as u64 - 1, &tx_pool_diff());
    assert!(matches!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::Binprot(_))
    ));
}

#[test]
fn refuses_data_shorter_than_the_header() {
    assert_eq!(
        decode_pubsub_data(&[1, 2, 3]),
        Err(GossipDecodeError::MissingSizeHeader(3))
    );
    assert!(decode_pubsub_data(&framed(0, &[])).is_err());
}

/// OCaml ignores bytes after the header's length (Core's `read_bin_prot`
/// checks only that the reader stops at the header's length), so we do too,
/// for every message kind: forwarding them is safe.
#[test]
fn accepts_trailing_bytes_after_a_transaction_diff() {
    let mut body = tx_pool_diff();
    body.extend_from_slice(&[0, 1, 2]);
    let data = framed(tx_pool_diff().len() as u64, &body);
    assert_eq!(decode_pubsub_data(&data).unwrap(), tx_diff());
}

/// 0x85 is not a bin_prot integer code (`bin_read_int32` raises
/// `Int32_code`): bin_prot writes 133 as `0xfe 0x85 0x00`. Our reader used to
/// take the byte as 133.
#[test]
fn refuses_an_invalid_integer_prefix_byte() {
    let i = nonce_offset();
    let mut body = tx_diff_with_nonce(5);
    body[i] = 0x85;
    assert!(GossipNetMessageV2::binprot_read(&mut &body[..]).is_err());
    let data = framed(body.len() as u64, &body);
    assert!(matches!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::Binprot(_))
    ));
}

/// The nonce 5 written in three bytes instead of one: bin_prot's reader
/// accepts it (`0xfe` and two bytes, whatever the value), so OCaml takes such
/// a diff, and so do we. We forward the bytes as received, which OCaml
/// decodes.
#[test]
fn accepts_a_non_minimal_integer_as_bin_prot_does() {
    let i = nonce_offset();
    let canonical = tx_diff_with_nonce(5);
    let mut body = canonical[..i].to_vec();
    body.extend_from_slice(&[0xfe, 0x05, 0x00]);
    body.extend_from_slice(&canonical[i + 1..]);

    let data = framed(body.len() as u64, &body);
    let decoded = decode_pubsub_data(&data).unwrap();
    assert_eq!(encode(&decoded), canonical, "the same value as nonce 5");
    // The same nonce in the 4-byte form.
    let mut body = canonical[..i].to_vec();
    body.extend_from_slice(&[0xfd, 0x05, 0x00, 0x00, 0x00]);
    body.extend_from_slice(&canonical[i + 1..]);
    let data = framed(body.len() as u64, &body);
    assert_eq!(encode(&decode_pubsub_data(&data).unwrap()), canonical);
    // But not the 8-byte form: the nonce is an OCaml `int32`, and
    // `bin_read_int32` has none (`Int32_code`).
    let mut body = canonical[..i].to_vec();
    body.extend_from_slice(&[0xfc, 5, 0, 0, 0, 0, 0, 0, 0]);
    body.extend_from_slice(&canonical[i + 1..]);
    let data = framed(body.len() as u64, &body);
    assert!(decode_pubsub_data(&data).is_err());
}

/// The diff's outer nonce is an OCaml `int` (63 bits): `0xfc` with a value
/// past 2^62 is `Int_overflow` there, and must be here.
#[test]
fn refuses_a_gossip_nonce_outside_ocamls_int() {
    let body = tx_pool_diff();
    // The diff ends with its nonce, 0: one byte.
    assert_eq!(*body.last().unwrap(), 0);
    let prefix = &body[..body.len() - 1];

    let with_nonce = |nonce: &[u8]| {
        let mut b = prefix.to_vec();
        b.extend_from_slice(nonce);
        framed(b.len() as u64, &b)
    };
    let max: i64 = (1 << 62) - 1;
    let mut ok = vec![0xfc];
    ok.extend_from_slice(&max.to_le_bytes());
    assert!(decode_pubsub_data(&with_nonce(&ok)).is_ok());
    let mut over = vec![0xfc];
    over.extend_from_slice(&(max + 1).to_le_bytes());
    assert!(decode_pubsub_data(&with_nonce(&over)).is_err());
    assert!(decode_pubsub_data(&with_nonce(&[0x85])).is_err());
}

/// A snark pool diff from a Mesa block's completed work.
fn snark_diff(nonce: u64) -> GossipNetMessageV2 {
    let tip = fixture::<GetBestTipV2Response>("get_best_tip_v3.bin").expect("a best tip");
    let work = tip
        .data
        .body
        .staged_ledger_diff
        .diff
        .0
        .completed_works
        .iter()
        .next()
        .expect("completed work in block 564003")
        .clone();
    let statement = |proof: &mina_p2p_messages::v2::LedgerProofProdStableV2| {
        let s = &proof.0.statement;
        MinaStateSnarkedLedgerStateStableV2(
            MinaStateBlockchainStateValueStableV2LedgerProofStatement {
                source: s.source.clone(),
                target: s.target.clone(),
                connecting_ledger_left: s.connecting_ledger_left.clone(),
                connecting_ledger_right: s.connecting_ledger_right.clone(),
                supply_increase: s.supply_increase.clone(),
                fee_excess: s.fee_excess.clone(),
                sok_digest: (),
            },
        )
    };
    let statement = match &work.proofs {
        TransactionSnarkWorkTStableV2Proofs::One(p) => {
            TransactionSnarkWorkStatementStableV2::One(statement(p))
        }
        TransactionSnarkWorkTStableV2Proofs::Two((a, b)) => {
            TransactionSnarkWorkStatementStableV2::Two((statement(a), statement(b)))
        }
    };
    GossipNetMessageV2::SnarkPoolDiff {
        message: NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(Box::new((
            statement,
            NetworkPoolSnarkPoolDiffVersionedStableV2AddSolvedWork1 {
                proof: work.proofs.clone(),
                fee: MinaBaseFeeWithProverStableV1 {
                    fee: work.fee.clone(),
                    prover: work.prover.clone(),
                },
            },
        ))),
        nonce: Number::from(nonce),
    }
}

/// The review's case: a snark pool diff whose size header matches, with the
/// invalid code 0x85 for its outer nonce. bin_prot's `bin_read_int` refuses
/// it, so OCaml bans whoever sends it; we used to accept and, after
/// verifying the work, forward it.
#[test]
fn refuses_a_snark_diff_with_an_invalid_nonce_code() {
    let body = encode(&snark_diff(0));
    assert_eq!(*body.last().unwrap(), 0, "the nonce is the last byte");
    let data = framed(body.len() as u64, &body);
    assert!(matches!(
        decode_pubsub_data(&data).unwrap(),
        GossipNetMessageV2::SnarkPoolDiff { .. }
    ));

    let mut bad = body.clone();
    *bad.last_mut().unwrap() = 0x85;
    let data = framed(bad.len() as u64, &bad);
    assert!(matches!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::Binprot(_))
    ));

    // The smallest snark diff, `Empty`, too.
    for (bytes, ok) in [(vec![1u8, 1, 0], true), (vec![1, 1, 0x85], false)] {
        let data = framed(bytes.len() as u64, &bytes);
        assert_eq!(decode_pubsub_data(&data).is_ok(), ok, "{bytes:?}");
    }
}

fn block_message() -> GossipNetMessageV2 {
    let tip = fixture::<GetBestTipV2Response>("get_best_tip_v3.bin").expect("a best tip");
    GossipNetMessageV2::NewState(std::sync::Arc::new(tip.data))
}

/// The first byte where two encodings differ.
fn first_difference(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .zip(b)
        .position(|(x, y)| x != y)
        .expect("they differ")
}

/// Blocks get the same decoder: an invalid integer code (0x84 for the
/// protocol version's `transaction`, an OCaml `int` of 4) and an invalid
/// option code (2 for `proposed_protocol_version_opt`) are refused.
#[test]
fn refuses_malformed_blocks() {
    let block = block_message();
    let bytes = encode(&block);
    let data = framed(bytes.len() as u64, &bytes);
    assert_eq!(decode_pubsub_data(&data).unwrap(), block);

    let GossipNetMessageV2::NewState(b) = &block else {
        unreachable!()
    };
    let mut other = (**b).clone();
    other.header.current_protocol_version.transaction = Number(5);
    let i = first_difference(&bytes, &encode(&GossipNetMessageV2::NewState(other.into())));
    assert_eq!(bytes[i], 4);
    let mut bad = bytes.clone();
    bad[i] = 0x84;
    assert!(decode_pubsub_data(&framed(bad.len() as u64, &bad)).is_err());
    // 4 written as `0xfe 0x04 0x00` is fine, as in OCaml.
    let mut wide = bytes[..i].to_vec();
    wide.extend_from_slice(&[0xfe, 0x04, 0x00]);
    wide.extend_from_slice(&bytes[i + 1..]);
    assert_eq!(
        decode_pubsub_data(&framed(wide.len() as u64, &wide)).unwrap(),
        block
    );

    let mut other = (**b).clone();
    assert!(other.header.proposed_protocol_version_opt.is_none());
    other.header.proposed_protocol_version_opt =
        Some(other.header.current_protocol_version.clone());
    let i = first_difference(&bytes, &encode(&GossipNetMessageV2::NewState(other.into())));
    assert_eq!(bytes[i], 0, "the option's None");
    let mut bad = bytes.clone();
    bad[i] = 2;
    assert!(decode_pubsub_data(&framed(bad.len() as u64, &bad)).is_err());
}

/// A field element at or above the Pasta moduli is a decode failure in
/// OCaml (`of_bigint`), so it is here. The payment's signature `rx` is the
/// first field after the payload.
#[test]
fn refuses_a_field_element_at_the_modulus() {
    let mut message = tx_diff();
    let GossipNetMessageV2::TransactionPoolDiff { message: diff, .. } = &mut message else {
        unreachable!()
    };
    let MinaBaseUserCommandStableV2::SignedCommand(cmd) = diff.0.front_mut().unwrap() else {
        unreachable!()
    };
    let mut sig = (*cmd.signature).clone();
    sig.1 = mina_p2p_messages::bigint::BigInt::from_bytes([0xff; 32]);
    cmd.signature = sig.into();
    let mut body = Vec::new();
    message.binprot_write(&mut body).unwrap();
    assert!(decode_pubsub_data(&framed(body.len() as u64, &body)).is_err());
}

/// For other messages we keep OCaml's rule: the header's length is what is
/// decoded, and what follows it is ignored (OCaml's `read_bin_prot` does the
/// same, so forwarding them is safe).
#[test]
fn ignores_trailing_bytes_after_a_block() {
    let tip = fixture::<GetBestTipV2Response>("get_best_tip_v3.bin").expect("a best tip");
    let block = encode(&GossipNetMessageV2::NewState(std::sync::Arc::new(tip.data)));
    let mut body = block.clone();
    body.extend_from_slice(&[9, 9, 9]);
    let data = framed(block.len() as u64, &body);
    let message = decode_pubsub_data(&data).unwrap();
    assert!(matches!(message, GossipNetMessageV2::NewState(_)));

    // But a header that doesn't match the reader is still refused.
    let data = framed(body.len() as u64, &body);
    assert!(matches!(
        decode_pubsub_data(&data),
        Err(GossipDecodeError::LengthMismatch { .. })
    ));
}

/// Level 2's fixture messages, for `tools/tx-gossip-probe`: run with
/// `P6_PROBE_OUT=<dir>` and `--ignored`. Real Mesa commands re-encoded by us,
/// framed as our node publishes them.
#[test]
#[ignore = "writes Level 2 probe messages to $P6_PROBE_OUT"]
fn write_level2_fixture_messages() {
    let out = std::path::PathBuf::from(std::env::var("P6_PROBE_OUT").expect("P6_PROBE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let frame = |message: &GossipNetMessageV2| {
        let body = encode(message);
        framed(body.len() as u64, &body)
    };
    let commands = mesa_commands();
    let files = [
        (
            "fixture-signed.bin",
            diff_of(vec![mesa_signed_command()], 0),
        ),
        ("fixture-zkapp.bin", diff_of(vec![mesa_zkapp_command()], 0)),
        (
            "fixture-multi.bin",
            diff_of(commands.clone(), 1_792_383_635_000_000_000),
        ),
    ];
    for (name, message) in files {
        std::fs::write(out.join(name), frame(&message)).unwrap();
    }
    println!("{} commands in the multi diff", commands.len());
}

/// OCaml's Fp reader rejects the entire [p, q) interval. These fields include
/// snark metadata which the proof verifier does not consume: forwarding cannot
/// rely on later ledger conversions to catch an invalid field encoding.
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/crypto/kimchi_pasta_snarky_backend/field.ml#L180-L189>
/// Binding: <https://github.com/o1-labs/proof-systems/blob/dfb9a27b800c8f32afd85ee8cddbddfc29a5733b/kimchi-stubs/src/arkworks/pasta_fp.rs#L267-L276>
#[test]
fn refuses_fp_values_between_the_pasta_moduli_before_verification() {
    use ark_ff::{BigInteger, PrimeField};
    use mina_curves::pasta::{Fp, Fq};
    use mina_p2p_messages::bigint::BigInt;

    let mut q_minus_one = Fq::MODULUS;
    q_minus_one.0[0] -= 1;
    for value in [Fp::MODULUS, q_minus_one] {
        let invalid = BigInt::from_bytes(value.to_bytes_le().try_into().unwrap());
        let mut tx = tx_diff();
        let GossipNetMessageV2::TransactionPoolDiff { message, .. } = &mut tx else {
            unreachable!()
        };
        let MinaBaseUserCommandStableV2::SignedCommand(cmd) = message.0.front_mut().unwrap() else {
            unreachable!()
        };
        let mut signature = (*cmd.signature).clone();
        signature.0 = invalid.clone();
        cmd.signature = signature.into();

        let mut work = snark_diff(0);
        let GossipNetMessageV2::SnarkPoolDiff {
            message: NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(work_data),
            ..
        } = &mut work
        else {
            unreachable!()
        };
        // The advertised statement is discarded in favour of the statement
        // inside the proof. Its bytes still go to the next peer unchanged.
        let statement = match &mut work_data.0 {
            TransactionSnarkWorkStatementStableV2::One(s) => s,
            TransactionSnarkWorkStatementStableV2::Two((s, _)) => s,
        };
        statement.0.source.local_state.transaction_commitment = invalid.clone();

        let mut prover = snark_diff(0);
        let GossipNetMessageV2::SnarkPoolDiff {
            message: NetworkPoolSnarkPoolDiffVersionedStableV2::AddSolvedWork(work_data),
            ..
        } = &mut prover
        else {
            unreachable!()
        };
        work_data.1.fee.prover = mina_p2p_messages::v2::NonZeroCurvePointUncompressedStableV1 {
            x: invalid.clone(),
            is_odd: false,
        }
        .into();

        let mut block = block_message();
        let GossipNetMessageV2::NewState(b) = &mut block else {
            unreachable!()
        };
        let b = std::sync::Arc::make_mut(b);
        std::sync::Arc::make_mut(&mut b.header.protocol_state_proof)
            .0
            .prev_evals
            .evals
            .public_input
            .0 = invalid;

        for message in [tx, work, prover, block] {
            let body = encode(&message);
            assert!(matches!(
                decode_pubsub_data(&framed(body.len() as u64, &body)),
                Err(GossipDecodeError::Binprot(_))
            ));
        }
    }
}

/// Fq values in [p, q) are valid encodings. Tightening the untyped BigInt
/// reader to Fp's bound would silently reject valid signature/proof scalars.
#[test]
fn accepts_fq_values_between_the_pasta_moduli() {
    use ark_ff::{BigInteger, PrimeField};
    use mina_curves::pasta::{Fp, Fq};
    use mina_p2p_messages::bigint::BigInt;

    let mut q_minus_one = Fq::MODULUS;
    q_minus_one.0[0] -= 1;
    for value in [Fp::MODULUS, q_minus_one] {
        let valid = BigInt::from_bytes(value.to_bytes_le().try_into().unwrap());
        let mut tx = tx_diff();
        let GossipNetMessageV2::TransactionPoolDiff { message, .. } = &mut tx else {
            unreachable!()
        };
        let MinaBaseUserCommandStableV2::SignedCommand(cmd) = message.0.front_mut().unwrap() else {
            unreachable!()
        };
        let mut signature = (*cmd.signature).clone();
        signature.1 = valid.clone();
        cmd.signature = signature.into();

        let mut block = block_message();
        let GossipNetMessageV2::NewState(b) = &mut block else {
            unreachable!()
        };
        let b = std::sync::Arc::make_mut(b);
        let proof = &mut std::sync::Arc::make_mut(&mut b.header.protocol_state_proof).0;
        proof.proof.ft_eval1 = valid.clone();
        proof.proof.evaluations.z.0 = valid.clone();
        proof.proof.bulletproof.z_1 = valid.clone();
        proof.proof.bulletproof.z_2 = valid.clone();
        proof
            .statement
            .proof_state
            .messages_for_next_wrap_proof
            .challenge_polynomial_commitment
            .0 = valid;
        for message in [tx, block] {
            let body = encode(&message);
            assert_eq!(
                decode_pubsub_data(&framed(body.len() as u64, &body)).unwrap(),
                message
            );
        }
    }
}
