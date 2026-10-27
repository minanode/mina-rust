//! P6a: converting a command a peer sent us must fail cleanly, never panic.
//!
//! Any libp2p or WebRTC peer can send us a transaction, and the pool converts
//! it with `UserCommand::try_from` in the state machine's thread before any
//! check. Field elements come from the wire as 32 raw bytes and memos as
//! strings of any length, so both can be out of range.

use ark_ff::{BigInteger256, PrimeField};
use mina_curves::pasta::{Fp, Fq};
use mina_p2p_messages::{
    bigint::BigInt,
    binprot::BinProtRead,
    rpc::GetBestTipV2Response,
    rpc_kernel::ResponsePayload,
    string::CharString,
    v2::{MinaBaseSignedCommandMemoStableV1, MinaBaseUserCommandStableV2},
};
use mina_tree::{proofs::transaction::InnerCurve, scan_state::transaction_logic::UserCommand};

/// A signed command from Mesa mainnet block 564003, captured by
/// `mina-rpc-capture` for the p2p-messages tests.
fn payment() -> MinaBaseUserCommandStableV2 {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../p2p-messages/tests/files/mesa/get_best_tip_v3.bin"
    );
    let bytes = std::fs::read(path).unwrap();
    let payload = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]).unwrap();
    let block = payload.0.unwrap().0.unwrap().data;
    let diff = block.body.staged_ledger_diff.diff.clone();
    diff.0
        .commands
        .into_iter()
        .chain(diff.1.into_iter().flat_map(|d| d.commands))
        .map(|c| c.data)
        .find(|c| matches!(c, MinaBaseUserCommandStableV2::SignedCommand(_)))
        .expect("a signed command")
}

fn with_signed(
    f: impl FnOnce(&mut mina_p2p_messages::v2::MinaBaseSignedCommandStableV2),
) -> MinaBaseUserCommandStableV2 {
    let mut cmd = payment();
    let MinaBaseUserCommandStableV2::SignedCommand(signed) = &mut cmd else {
        panic!("the fixture holds a signed command");
    };
    f(signed);
    cmd
}

fn p() -> BigInt {
    BigInt::from(Fp::MODULUS)
}

fn q() -> BigInt {
    BigInt::from(Fq::MODULUS)
}

fn max() -> BigInt {
    BigInt::from(BigInteger256::new([u64::MAX; 4]))
}

#[test]
fn the_fixture_converts() {
    assert!(UserCommand::try_from(&payment()).is_ok());
}

#[test]
fn a_signature_rx_at_the_modulus_is_an_error() {
    for rx in [p(), max()] {
        let cmd = with_signed(|c| {
            let mut sig = (*c.signature).clone();
            sig.0 = rx.clone();
            c.signature = sig.into();
        });
        assert!(UserCommand::try_from(&cmd).is_err());
    }
}

#[test]
fn a_signature_s_at_the_scalar_modulus_is_an_error() {
    for s in [q(), max()] {
        let cmd = with_signed(|c| {
            let mut sig = (*c.signature).clone();
            sig.1 = s.clone();
            c.signature = sig.into();
        });
        assert!(UserCommand::try_from(&cmd).is_err());
    }
}

#[test]
fn a_public_key_at_the_modulus_is_an_error() {
    let cmd = with_signed(|c| {
        let mut signer = (*c.signer).clone();
        signer.x = p();
        c.signer = signer.into();
    });
    assert!(UserCommand::try_from(&cmd).is_err());

    let cmd = with_signed(|c| {
        let mut fee_payer = (*c.payload.common.fee_payer_pk).clone();
        fee_payer.x = max();
        c.payload.common.fee_payer_pk = fee_payer.into();
    });
    assert!(UserCommand::try_from(&cmd).is_err());
}

#[test]
fn a_memo_that_is_not_34_bytes_is_an_error() {
    for len in [0, 1, 33, 35, 1000] {
        let cmd = with_signed(|c| {
            c.payload.common.memo =
                MinaBaseSignedCommandMemoStableV1(CharString::from(vec![1u8; len]));
        });
        assert!(UserCommand::try_from(&cmd).is_err(), "memo of {len} bytes");
    }
}

/// Verification keys in zkApp commands carry curve points, and ark-ec's
/// `Affine::new` asserts that a point is on the curve.
#[test]
fn a_curve_point_off_the_curve_is_an_error() {
    let one = BigInt::from(Fp::from(1u64));
    assert!(InnerCurve::<Fp>::try_from(&(one.clone(), one)).is_err());
    assert!(InnerCurve::<Fp>::try_from(&(p(), BigInt::zero())).is_err());

    // The point at infinity is encoded as (0, 0), and a real point passes.
    assert!(InnerCurve::<Fp>::try_from(&(BigInt::zero(), BigInt::zero())).is_ok());
    let g = InnerCurve::<Fp>::random();
    let xy: (BigInt, BigInt) = g.clone().into();
    assert_eq!(InnerCurve::<Fp>::try_from(&xy).unwrap(), g);
}
