//! The transport's own parsers fuzzed with what a peer could send: yamux
//! frames, multistream-select tokens, pubsub framing, every WebRTC channel's
//! messages, and a signaling offer as it arrives decrypted. See
//! `mina-peer-input-fuzz` for the mutations and the time budget.

use binprot::{BinProtRead, BinProtWrite};
use mina_p2p_messages::{
    rpc::{GetBestTipV2Response, GetTransitionChainV2},
    rpc_kernel::{PayloadBinprotReader, ResponsePayload},
};
use mina_peer_input_fuzz::{fuzz, Budget};

use crate::{
    channels::{
        best_tip::BestTipPropagationChannelMsg,
        rpc::{P2pRpcRequest, P2pRpcResponse, RpcChannelMsg},
        signaling::{
            discovery::SignalingDiscoveryChannelMsg, exchange::SignalingExchangeChannelMsg,
        },
        ChannelId, ChannelMsg,
    },
    identity::SecretKey,
    network::{select::token, yamux::P2pNetworkYamuxState},
    webrtc::{Host, Offer},
    P2pNetworkPubsubState,
};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/../p2p-messages/tests/files/mesa/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn best_tip() -> mina_p2p_messages::v2::MinaBlockBlockStableV2 {
    let bytes = fixture("get_best_tip_v3.bin");
    let tip = ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut &bytes[..]);
    tip.unwrap().0.unwrap().0.unwrap().data
}

fn channel_message(message: ChannelMsg) -> Vec<u8> {
    let mut bytes = Vec::new();
    message.encode(&mut bytes).unwrap();
    bytes
}

fn offer() -> Offer {
    Offer {
        sdp: "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n".to_owned(),
        chain_id: mina_core::MAINNET_CHAIN_ID,
        identity_pub_key: SecretKey::deterministic(1).public_key(),
        target_peer_id: SecretKey::deterministic(2).public_key().peer_id(),
        host: Host::Ipv4([127, 0, 0, 1].into()),
        listen_port: Some(3000),
    }
}

/// Each WebRTC channel's messages, read as the channel reads them, and a
/// block in one hashed as the node hashes it.
#[test]
fn webrtc_channel_messages() {
    let block = std::sync::Arc::new(best_tip());
    let chain = GetTransitionChainV2::response_payload(
        &mut &fixture("get_transition_chain_v2_563900_zkapp.bin")[..],
    )
    .unwrap()
    .unwrap();
    let key = SecretKey::deterministic(3).public_key();
    let encrypted = SecretKey::deterministic(4)
        .encrypt(&key, rand::thread_rng(), &offer())
        .unwrap();
    let corpus: Vec<(ChannelId, Vec<u8>)> = vec![
        (
            ChannelId::SignalingDiscovery,
            channel_message(
                SignalingDiscoveryChannelMsg::Discovered {
                    target_public_key: key.clone(),
                }
                .into(),
            ),
        ),
        (
            ChannelId::SignalingDiscovery,
            channel_message(
                SignalingDiscoveryChannelMsg::DiscoveredAccept(encrypted.clone()).into(),
            ),
        ),
        (
            ChannelId::SignalingExchange,
            channel_message(
                SignalingExchangeChannelMsg::OfferToYou {
                    offerer_pub_key: key.clone(),
                    offer: encrypted,
                }
                .into(),
            ),
        ),
        (
            ChannelId::BestTipPropagation,
            channel_message(BestTipPropagationChannelMsg::BestTip(block.clone()).into()),
        ),
        (
            ChannelId::Rpc,
            channel_message(
                RpcChannelMsg::Request(
                    1,
                    P2pRpcRequest::Block(block.header.protocol_state.previous_state_hash.clone()),
                )
                .into(),
            ),
        ),
        (
            ChannelId::Rpc,
            channel_message(
                RpcChannelMsg::Response(
                    2,
                    Some(P2pRpcResponse::Block(
                        chain.iter().next().unwrap().clone().into(),
                    )),
                )
                .into(),
            ),
        ),
    ];
    for id in ChannelId::iter_all() {
        let messages: Vec<_> = corpus
            .iter()
            .filter(|(channel, _)| *channel == id)
            .map(|(_, bytes)| bytes.clone())
            .collect();
        // Channels without a captured message start from another channel's.
        let messages = if messages.is_empty() {
            corpus.iter().map(|(_, bytes)| bytes.clone()).collect()
        } else {
            messages
        };
        fuzz(
            &format!("webrtc-{}", id.name()),
            &messages,
            &Budget::from_env(),
            |bytes| match ChannelMsg::decode(&mut &bytes[..], id) {
                Ok(ChannelMsg::BestTipPropagation(BestTipPropagationChannelMsg::BestTip(
                    block,
                ))) => mina_core::block::BlockWithHash::try_new(block).is_ok(),
                Ok(ChannelMsg::Rpc(RpcChannelMsg::Response(
                    _,
                    Some(P2pRpcResponse::Block(block)),
                ))) => mina_core::block::BlockWithHash::try_new(block).is_ok(),
                Ok(_) => true,
                Err(_) => false,
            },
        );
    }
}

/// An offer as it arrives decrypted from the signaling channel, or as JSON
/// over HTTP: parsed as the node parses it.
#[test]
fn signaling_offers() {
    let corpus = vec![serde_json::to_vec(&offer()).unwrap()];
    let report = fuzz("signaling-offer", &corpus, &Budget::from_env(), |bytes| {
        serde_json::from_slice::<Offer>(bytes).is_ok()
    });
    assert!(mina_peer_input_fuzz::replaying() || report.accepted > 0);
}

/// Bytes a peer sends on a yamux session, framed as the session frames them.
#[test]
fn yamux_frames() {
    let frame = |kind: u8, flags: u16, stream: u32, value: u32, data: &[u8]| {
        let mut bytes = vec![0, kind];
        bytes.extend_from_slice(&flags.to_be_bytes());
        bytes.extend_from_slice(&stream.to_be_bytes());
        bytes.extend_from_slice(&value.to_be_bytes());
        bytes.extend_from_slice(data);
        bytes
    };
    let corpus = vec![
        [
            frame(1, 1, 1, 256 * 1024, &[]),
            frame(0, 0, 1, 5, b"hello"),
            frame(2, 1, 0, 7, &[]),
        ]
        .concat(),
        frame(3, 0, 0, 0, &[]),
    ];
    fuzz("yamux", &corpus, &Budget::from_env(), |bytes| {
        let mut yamux = P2pNetworkYamuxState {
            message_size_limit: crate::Limit::Some(0x1000_0000),
            ..Default::default()
        };
        yamux.extend_buffer(bytes);
        yamux.parse_frames();
        yamux.terminated.is_none()
    });
}

/// Protocol names a peer proposes in multistream-select.
#[test]
fn select_tokens() {
    let corpus = vec![
        b"\x13/multistream/1.0.0\n\x10/coda/yamux/1.0.0\n".to_vec(),
        b"\x07/noise\n".to_vec(),
    ];
    fuzz("select", &corpus, &Budget::from_env(), |bytes| {
        let mut state = token::State::default();
        state.put(bytes);
        let mut parsed = 0;
        while let Ok(Some(_)) = state.parse_token() {
            parsed += 1;
        }
        parsed > 0
    });
}

/// Pubsub RPCs as a stream delivers them, in pieces, then each message's
/// data decoded as gossip.
#[test]
fn pubsub_framing() {
    use prost::Message;
    let block = best_tip();
    let mut data = vec![0; 8];
    mina_p2p_messages::gossip::GossipNetMessageV2::NewState(block.into())
        .binprot_write(&mut data)
        .unwrap();
    let len = (data.len() - 8) as u64;
    data[..8].copy_from_slice(&len.to_le_bytes());
    let rpc = crate::pb::Rpc {
        subscriptions: vec![],
        publish: vec![crate::pb::Message {
            from: Some(vec![1; 38]),
            data: Some(data),
            seqno: Some(vec![0; 8]),
            topic: "coda/consensus-messages/0.0.1".to_owned(),
            signature: Some(vec![2; 64]),
            key: None,
        }],
        control: None,
    };
    let corpus = vec![rpc.encode_length_delimited_to_vec()];
    fuzz("pubsub", &corpus, &Budget::from_env(), |bytes| {
        let mut buffer = Vec::new();
        let (first, second) = bytes.split_at(bytes.len() / 2);
        let mut rpcs = P2pNetworkPubsubState::take_complete_rpcs(&mut buffer, first);
        rpcs.extend(P2pNetworkPubsubState::take_complete_rpcs(
            &mut buffer,
            second,
        ));
        rpcs.iter().flat_map(|rpc| &rpc.publish).all(|message| {
            message
                .data
                .as_deref()
                .is_some_and(|data| mina_p2p_messages::gossip::decode_pubsub_data(data).is_ok())
        }) && !rpcs.is_empty()
    });
}

/// A streamed scan state may say it has at most as many trees as a scan
/// state can have; a part that says more is not taken.
#[test]
fn streamed_scan_state_tree_count_is_bounded() {
    use crate::channels::streaming_rpc::{
        staged_ledger_parts::{ScanStateBase, StagedLedgerPartsResponse},
        P2pStreamingRpcResponse,
    };
    let base = |trees: u32| {
        P2pStreamingRpcResponse::StagedLedgerParts(StagedLedgerPartsResponse::ScanStateBase(
            ScanStateBase {
                trees: trees.into(),
                acc: None,
                curr_job_seq_no: 0u64.into(),
                max_base_jobs: 128u64.into(),
                delay: 2u64.into(),
            },
        ))
    };
    // Mainnet: 2^7 base jobs and a work delay of 2, so 8 * 3 + 1 trees.
    assert!(base(1).is_within_bounds());
    assert!(base(25).is_within_bounds());
    assert!(!base(26).is_within_bounds());
    assert!(!base(u32::MAX).is_within_bounds());
}

/// A signaling message whose offer claims far more bytes than it has is an
/// error, read without allocating for the claim.
#[test]
fn a_signaling_offer_claiming_a_huge_length_is_refused() {
    let mut bytes =
        channel_message(SignalingDiscoveryChannelMsg::DiscoveredAccept(Vec::new().into()).into());
    assert_eq!(bytes.pop(), Some(0), "the empty offer's length");
    bytes.push(0xfc);
    bytes.extend_from_slice(&(1u64 << 61).to_le_bytes());
    bytes.extend_from_slice(&[1, 2, 3]);
    assert!(ChannelMsg::decode(&mut &bytes[..], ChannelId::SignalingDiscovery).is_err());
}
