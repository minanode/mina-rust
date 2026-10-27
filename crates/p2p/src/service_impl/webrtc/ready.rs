//! Ready events on channel 0 (feature `p2p-webrtc-ready-events`), in place
//! of the driver's sleeps for lost messages.
//!
//! A message sent on a data channel is lost when the peer has not yet made
//! the channel, or has not yet registered its message handler: the SCTP
//! stacks drop data for an unknown stream (rustrtc; the webrtc-rs fork since
//! `6f1a6dd2`), and webrtc-rs and browsers drop a message that arrives before
//! `on_message`. Both sides make channels 1 to 8 on their own once the peer is
//! ready, so the driver waited 1 s before the connection authentication and
//! 3 s before a channel's first message.
//!
//! Instead:
//!
//! - channel 0's handler is registered when the channel is made, before the
//!   offer and answer, and keeps what arrives;
//! - the authentication is sent as soon as channel 0 is open, and once more
//!   after [`AUTH_RESEND`] for a peer without ready events (which registers
//!   its handler only once connected, then takes the first message it gets
//!   as the authentication), unless the peer has announced a channel;
//! - once its handler for channel `X` is registered, each side sends
//!   [`ready_msg`] for `X` on channel 0 (always after the authentication and
//!   its resend, so a peer without ready events still gets an authentication
//!   first, and ignores the rest);
//! - a channel's first message waits for the peer's ready message for it, or
//!   [`FALLBACK`], the old delay, for a peer that sends none.
//!
//! Channel 0 stays open for the life of the connection.

use std::{sync::Arc, time::Duration};

use mina_core::channels::{mpsc, oneshot};
use tokio::sync::watch;

use crate::{channels::ChannelId, webrtc::ConnectionAuthEncrypted};

use super::{sleep, spawn_local, RTCChannel};

/// The old delay before a channel's first message, kept for peers without
/// ready events.
pub(super) const FALLBACK: Duration = Duration::from_secs(3);
/// When the authentication is sent again, for peers without ready events:
/// the old delay before it.
pub(super) const AUTH_RESEND: Duration = Duration::from_secs(1);

const READY_MAGIC: &[u8; 6] = b"MNARDY";
const READY_LEN: usize = READY_MAGIC.len() + 2;

/// "My channel `id` is made and listening", on channel 0. Its length differs
/// from the authentication's (92 bytes).
pub(super) fn ready_msg(id: ChannelId) -> [u8; READY_LEN] {
    let mut msg = [0; READY_LEN];
    msg[..READY_MAGIC.len()].copy_from_slice(READY_MAGIC);
    msg[READY_MAGIC.len()..].copy_from_slice(&id.to_u16().to_be_bytes());
    msg
}

/// The channel id of a ready message, if `data` is one.
pub(super) fn parse_ready(data: &[u8]) -> Option<u16> {
    let id = data.strip_prefix(READY_MAGIC.as_slice())?;
    Some(u16::from_be_bytes(id.try_into().ok()?))
}

fn bit(id: u16) -> u32 {
    1u32.checked_shl(u32::from(id)).unwrap_or(0)
}

/// Channel 0's messages, kept from the moment its handler is registered.
pub(super) struct Ch0Listener(mpsc::UnboundedReceiver<Vec<u8>>);

impl Ch0Listener {
    pub(super) fn listen(main_channel: &RTCChannel) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        main_channel.on_message(move |data| {
            let _ = tx.send(data.to_vec());
            #[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
            std::future::ready(())
        });
        Self(rx)
    }
}

enum Ch0Cmd {
    ResendAuth,
    Ready(ChannelId),
}

/// The peer's ready messages, and a way to send ours, for `peer_loop`.
#[derive(Clone)]
pub(super) struct Ready {
    cmd: mpsc::UnboundedSender<Ch0Cmd>,
    peer_ready: watch::Receiver<u32>,
}

impl Ready {
    /// Tell the peer our channel `id` is listening.
    pub(super) fn announce(&self, id: ChannelId) {
        let _ = self.cmd.send(Ch0Cmd::Ready(id));
    }

    /// Wait until the peer's channel `id` is listening, or [`FALLBACK`].
    /// Returns whether the peer said so.
    pub(super) async fn wait(&self, id: ChannelId) -> bool {
        let mut peer_ready = self.peer_ready.clone();
        let bit = bit(id.to_u16());
        tokio::select! {
            res = peer_ready.wait_for(|ready| ready & bit != 0) => res.is_ok(),
            _ = sleep(FALLBACK) => false,
        }
    }
}

/// Starts channel 0's reader and sender, sends our authentication, and
/// returns the peer's authentication (when it comes) and the [`Ready`]
/// handle.
pub(super) fn start(
    main_channel: RTCChannel,
    listener: Ch0Listener,
    auth: ConnectionAuthEncrypted,
) -> (oneshot::Receiver<ConnectionAuthEncrypted>, Ready) {
    let (auth_tx, auth_rx) = oneshot::channel();
    let (ready_tx, peer_ready) = watch::channel(0u32);
    let (cmd, mut cmd_rx) = mpsc::unbounded_channel::<Ch0Cmd>();

    // Reader: the first message of the authentication's size is the peer's
    // authentication; ready messages set the peer's channel bits.
    let mut listener = listener;
    spawn_local(async move {
        let mut auth_tx = Some(auth_tx);
        while let Some(data) = listener.0.recv().await {
            if let Some(id) = parse_ready(&data) {
                ready_tx.send_modify(|ready| *ready |= bit(id));
            } else if let Some(tx) = auth_tx.take() {
                match ConnectionAuthEncrypted::try_from(data.as_slice()) {
                    Ok(auth) => {
                        let _ = tx.send(auth);
                    }
                    Err(_) => auth_tx = Some(tx),
                }
            }
        }
    });

    // Sender: everything on channel 0 goes through here, in order.
    let auth = bytes::Bytes::copy_from_slice(auth.as_ref());
    let resend_cmd = cmd.clone();
    let ready = Ready {
        cmd,
        peer_ready: peer_ready.clone(),
    };
    let main_channel = Arc::new(main_channel);
    spawn_local(async move {
        let _ = main_channel.send(&auth).await;
        let mut resent = false;
        while let Some(cmd) = cmd_rx.recv().await {
            let peer_has_ready_events = *peer_ready.borrow() != 0;
            if !resent && !peer_has_ready_events {
                // Before any ready message, and at AUTH_RESEND at the latest.
                let _ = main_channel.send(&auth).await;
            }
            resent = true;
            if let Ch0Cmd::Ready(id) = cmd {
                let msg = bytes::Bytes::copy_from_slice(&ready_msg(id));
                let _ = main_channel.send(&msg).await;
            }
        }
    });
    spawn_local(async move {
        sleep(AUTH_RESEND).await;
        let _ = resend_cmd.send(Ch0Cmd::ResendAuth);
    });

    (auth_rx, ready)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_messages_round_trip() {
        for id in [
            ChannelId::SignalingDiscovery,
            ChannelId::BestTipPropagation,
            ChannelId::StreamingRpc,
        ] {
            assert_eq!(parse_ready(&ready_msg(id)), Some(id.to_u16()));
        }
    }

    #[test]
    fn other_channel_0_messages_are_not_ready_messages() {
        // The authentication is 92 bytes, a ready message 8.
        assert_eq!(parse_ready(&[0u8; 92]), None);
        assert_eq!(parse_ready(b"MNARDY"), None);
        assert_eq!(parse_ready(b"MNARDY\x00\x01\x02"), None);
        assert_eq!(parse_ready(b"XNARDY\x00\x01"), None);
        assert_ne!(READY_LEN, 92);
    }

    #[test]
    fn channel_bits() {
        assert_eq!(bit(0), 1);
        assert_eq!(bit(8), 256);
        assert_eq!(bit(40), 0);
    }
}

/// The protocol between two peers, over the rustrtc backend on loopback.
#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    not(feature = "p2p-webrtc-rs"),
    not(feature = "p2p-webrtc-cpp")
))]
mod protocol_tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{
        service_impl::webrtc::{
            build_api, RTCChannelConfig, RTCConfig, RTCConnection, RTCConnectionState,
        },
        webrtc::WebrtcConfig,
    };

    fn auth(byte: u8) -> ConnectionAuthEncrypted {
        ConnectionAuthEncrypted::try_from(&[byte; 92][..]).unwrap()
    }

    async fn peer() -> RTCConnection {
        let api = build_api(&WebrtcConfig::default());
        RTCConnection::create(
            &api,
            RTCConfig {
                ice_servers: Vec::new().into(),
                certificate: (),
                seed: [0; 32],
            },
        )
        .await
        .unwrap()
    }

    async fn channel(pc: &RTCConnection, id: ChannelId) -> RTCChannel {
        pc.channel_create(RTCChannelConfig {
            label: id.name(),
            negotiated: Some(id.to_u16()),
        })
        .await
        .unwrap()
    }

    fn opened(chan: &RTCChannel) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        let mut tx = Some(tx);
        chan.on_open(move || {
            if let Some(tx) = tx.take() {
                let _ = tx.send(());
            }
            std::future::ready(())
        });
        rx
    }

    /// Two connected peers, each with channel 0 open and listening.
    async fn pair() -> (
        (RTCConnection, RTCChannel, Ch0Listener),
        (RTCConnection, RTCChannel, Ch0Listener),
    ) {
        let (a, b) = (peer().await, peer().await);
        // Channel 0 is negotiated with id 0, as the driver makes it.
        let (a0, b0) = (
            a.channel_create(RTCChannelConfig {
                label: "",
                negotiated: Some(0),
            })
            .await
            .unwrap(),
            b.channel_create(RTCChannelConfig {
                label: "",
                negotiated: Some(0),
            })
            .await
            .unwrap(),
        );
        let (la, lb) = (Ch0Listener::listen(&a0), Ch0Listener::listen(&b0));
        let (oa, ob) = (opened(&a0), opened(&b0));
        let offer = a.offer_create().await.unwrap();
        a.local_desc_set(offer).await.unwrap();
        a.wait_for_ice_gathering_complete().await;
        let sdp = a.local_sdp().await.unwrap();
        let offer = rustrtc::SessionDescription::parse(rustrtc::SdpType::Offer, &sdp).unwrap();
        b.remote_desc_set(offer).await.unwrap();
        let answer = b.answer_create().await.unwrap();
        b.local_desc_set(answer).await.unwrap();
        b.wait_for_ice_gathering_complete().await;
        let sdp = b.local_sdp().await.unwrap();
        let answer = rustrtc::SessionDescription::parse(rustrtc::SdpType::Answer, &sdp).unwrap();
        a.remote_desc_set(answer).await.unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            oa.await.unwrap();
            ob.await.unwrap();
        })
        .await
        .expect("channel 0 opens");
        assert!(matches!(
            a.connection_state(),
            RTCConnectionState::Connected
        ));
        ((a, a0, la), (b, b0, lb))
    }

    #[tokio::test]
    async fn both_sides_authenticate_and_announced_channels_are_ready_at_once() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let ((a, a0, la), (b, b0, lb)) = pair().await;
                let (a_auth, a_ready) = start(a0, la, auth(1));
                let (b_auth, b_ready) = start(b0, lb, auth(2));
                let got_by_a = tokio::time::timeout(Duration::from_secs(5), a_auth).await;
                let got_by_b = tokio::time::timeout(Duration::from_secs(5), b_auth).await;
                assert_eq!(got_by_a.unwrap().unwrap().as_ref(), auth(2).as_ref());
                assert_eq!(got_by_b.unwrap().unwrap().as_ref(), auth(1).as_ref());

                let _a3 = channel(&a, ChannelId::BestTipPropagation).await;
                let _b3 = channel(&b, ChannelId::BestTipPropagation).await;
                a_ready.announce(ChannelId::BestTipPropagation);
                b_ready.announce(ChannelId::BestTipPropagation);
                let t0 = Instant::now();
                assert!(a_ready.wait(ChannelId::BestTipPropagation).await);
                assert!(b_ready.wait(ChannelId::BestTipPropagation).await);
                assert!(t0.elapsed() < Duration::from_secs(1), "{:?}", t0.elapsed());
            })
            .await;
    }

    #[tokio::test]
    async fn a_channel_the_peer_never_announces_falls_back_to_the_old_delay() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let ((_a, a0, la), (_b, b0, lb)) = pair().await;
                let (_a_auth, a_ready) = start(a0, la, auth(1));
                // A peer without ready events: it authenticates, announces
                // nothing.
                let (_b_auth, _b_ready) = start(b0, lb, auth(2));
                let t0 = Instant::now();
                assert!(!a_ready.wait(ChannelId::SnarkPropagation).await);
                let waited = t0.elapsed();
                assert!(
                    waited >= FALLBACK && waited < FALLBACK + Duration::from_secs(1),
                    "{waited:?}"
                );
            })
            .await;
    }
}
