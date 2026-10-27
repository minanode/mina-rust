#[cfg(feature = "p2p-webrtc-ready-events")]
mod ready;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
mod webrtc_cpp;
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "p2p-webrtc-rs",
    not(feature = "p2p-webrtc-cpp")
))]
mod webrtc_rs;
#[cfg(all(
    not(target_arch = "wasm32"),
    not(feature = "p2p-webrtc-rs"),
    not(feature = "p2p-webrtc-cpp")
))]
mod webrtc_rustrtc;
#[cfg(all(feature = "p2p-webrtc-ready-events", feature = "p2p-webrtc-cpp"))]
compile_error!("p2p-webrtc-ready-events is not implemented for the p2p-webrtc-cpp backend");

use std::{collections::BTreeMap, future::Future, pin::Pin, sync::Arc, time::Duration};

use mina_core::bug_condition;
use serde::Serialize;
use tokio::sync::Semaphore;

#[cfg(not(target_arch = "wasm32"))]
use tokio::task::spawn_local;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local;

use mina_core::channels::{mpsc, oneshot, Aborted, Aborter};

use crate::{
    channels::{ChannelId, ChannelMsg, MsgId},
    connection::outgoing::P2pConnectionOutgoingInitOpts,
    identity::{EncryptableType, PublicKey, SecretKey},
    webrtc,
    webrtc::{ConnectionAuth, ConnectionAuthEncrypted},
    P2pChannelEvent, P2pConnectionEvent, P2pEvent, PeerId,
};

#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "p2p-webrtc-rs",
    not(feature = "p2p-webrtc-cpp")
))]
mod imports {
    pub use super::webrtc_rs::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}
#[cfg(all(
    not(target_arch = "wasm32"),
    not(feature = "p2p-webrtc-rs"),
    not(feature = "p2p-webrtc-cpp")
))]
mod imports {
    pub use super::webrtc_rustrtc::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}
#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
mod imports {
    pub use super::webrtc_cpp::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}
#[cfg(target_arch = "wasm32")]
mod imports {
    pub use super::web::{
        build_api, certificate_from_pem_key, webrtc_signal_send, Api, RTCCertificate, RTCChannel,
        RTCConnection, RTCConnectionState, RTCSignalingError,
    };
}

use imports::*;
pub use imports::{webrtc_signal_send, RTCSignalingError};

/// The WebRTC implementation of this build, for `mina --version` and
/// `mina build-info`.
#[cfg(target_arch = "wasm32")]
pub const BACKEND: &str = "browser";
#[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
pub const BACKEND: &str = "libdatachannel";
#[cfg(all(
    not(target_arch = "wasm32"),
    feature = "p2p-webrtc-rs",
    not(feature = "p2p-webrtc-cpp")
))]
pub const BACKEND: &str = "webrtc-rs";
#[cfg(all(
    not(target_arch = "wasm32"),
    not(feature = "p2p-webrtc-rs"),
    not(feature = "p2p-webrtc-cpp")
))]
pub const BACKEND: &str = "rustrtc";

use super::TaskSpawner;

/// 16KB.
const CHUNK_SIZE: usize = 16 * 1024;

pub enum Cmd {
    PeerAdd { args: PeerAddArgs, aborted: Aborted },
}

#[derive(Debug)]
pub enum PeerCmd {
    PeerHttpOfferSend(String, webrtc::Offer),
    AnswerSet(webrtc::Answer),
    ConnectionAuthorizationSend(Option<ConnectionAuthEncrypted>),
    ChannelOpen(ChannelId),
    ChannelSend(MsgId, ChannelMsg),
}

enum PeerCmdInternal {
    ChannelOpened(ChannelId, Result<RTCChannel, Error>),
    ChannelClosed(ChannelId),
}

enum PeerCmdAll {
    External(Box<PeerCmd>),
    Internal(PeerCmdInternal),
}

pub struct P2pServiceCtx {
    pub cmd_sender: mpsc::TrackedUnboundedSender<Cmd>,
    pub peers: BTreeMap<PeerId, PeerState>,
}

pub struct PeerAddArgs {
    peer_id: PeerId,
    kind: PeerConnectionKind,
    event_sender: Arc<dyn Fn(P2pEvent) -> Option<()> + Send + Sync + 'static>,
    cmd_receiver: mpsc::TrackedUnboundedReceiver<PeerCmd>,
}

pub enum PeerConnectionKind {
    Outgoing,
    Incoming(Box<webrtc::Offer>),
}

pub struct PeerState {
    pub cmd_sender: mpsc::TrackedUnboundedSender<PeerCmd>,
    pub abort: Aborter,
}

#[derive(thiserror::Error, derive_more::From, Debug)]
pub(super) enum Error {
    #[cfg(all(
        not(target_arch = "wasm32"),
        feature = "p2p-webrtc-rs",
        not(feature = "p2p-webrtc-cpp")
    ))]
    #[error("{0}")]
    Rtc(::webrtc::Error),
    #[cfg(all(
        not(target_arch = "wasm32"),
        not(feature = "p2p-webrtc-rs"),
        not(feature = "p2p-webrtc-cpp")
    ))]
    #[error("{0}")]
    Rtc(::rustrtc::RtcError),
    #[cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp"))]
    #[error("{0}")]
    Rtc(::datachannel::Error),
    #[cfg(target_arch = "wasm32")]
    #[error("js error: {0:?}")]
    RtcJs(String),
    #[error("signaling error: {0}")]
    Signaling(RTCSignalingError),
    #[error("unexpected cmd received")]
    UnexpectedCmd,
    #[from(ignore)]
    #[error("channel closed")]
    ChannelClosed,
}

#[cfg(target_arch = "wasm32")]
impl From<wasm_bindgen::JsValue> for Error {
    fn from(value: wasm_bindgen::JsValue) -> Self {
        Error::RtcJs(format!("{value:?}"))
    }
}

pub type OnConnectionStateChangeHdlrFn = Box<
    dyn (FnMut(RTCConnectionState) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>)
        + Send
        + Sync,
>;

pub struct RTCConfig {
    pub ice_servers: RTCConfigIceServers,
    pub certificate: RTCCertificate,
    pub seed: [u8; 32],
}

#[derive(Serialize)]
pub struct RTCConfigIceServers(Vec<RTCConfigIceServer>);
pub type RTCConfigIceServer = webrtc::IceServer;

#[derive(Serialize)]
pub struct RTCChannelConfig {
    pub label: &'static str,
    pub negotiated: Option<u16>,
}

impl Default for RTCConfigIceServers {
    fn default() -> Self {
        Self(webrtc::IceServer::defaults())
    }
}

impl From<Vec<RTCConfigIceServer>> for RTCConfigIceServers {
    fn from(value: Vec<RTCConfigIceServer>) -> Self {
        Self(value)
    }
}

impl std::ops::Deref for RTCConfigIceServers {
    type Target = Vec<RTCConfigIceServer>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
impl Drop for RTCConnection {
    fn drop(&mut self) {
        if self.is_main() {
            let cloned = self.clone();
            spawn_local(async move {
                let _ = cloned.close().await;
            });
        }
    }
}

async fn sleep(dur: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    let fut = tokio::time::sleep(dur);
    #[cfg(target_arch = "wasm32")]
    let fut = gloo_timers::future::TimeoutFuture::new(dur.as_millis() as u32);
    fut.await
}

async fn wait_for_ice_gathering_complete(pc: &mut RTCConnection) {
    let timeout = sleep(Duration::from_secs(3));

    tokio::select! {
        _ = timeout => {}
        _ = pc.wait_for_ice_gathering_complete() => {}
    }
}

async fn peer_start(
    api: Api,
    args: PeerAddArgs,
    abort: Aborted,
    closed: mpsc::Sender<()>,
    certificate: RTCCertificate,
    rng_seed: [u8; 32],
    ice_servers: Arc<Vec<RTCConfigIceServer>>,
) {
    let PeerAddArgs {
        peer_id,
        kind,
        event_sender,
        mut cmd_receiver,
    } = args;
    let is_outgoing = matches!(kind, PeerConnectionKind::Outgoing);

    let config = RTCConfig {
        ice_servers: ice_servers.to_vec().into(),
        certificate,
        seed: rng_seed,
    };
    let fut = async {
        let mut pc = RTCConnection::create(&api, config).await?;
        let main_channel = pc
            .channel_create(RTCChannelConfig {
                label: "",
                negotiated: Some(0),
            })
            .await?;

        let offer = match kind {
            PeerConnectionKind::Incoming(offer) => (*offer).try_into()?,
            PeerConnectionKind::Outgoing => pc.offer_create().await?,
        };

        if is_outgoing {
            pc.local_desc_set(offer).await?;
            wait_for_ice_gathering_complete(&mut pc).await;
        } else {
            pc.remote_desc_set(offer).await?;
        }

        Result::<_, Error>::Ok((pc, main_channel))
    };

    #[allow(unused_mut)]
    let (mut pc, mut main_channel) = match fut.await {
        Ok(v) => v,
        Err(err) => {
            event_sender(P2pConnectionEvent::OfferSdpReady(peer_id, Err(err.to_string())).into());
            return;
        }
    };

    let (main_channel_open_tx, main_channel_open) = oneshot::channel::<()>();
    let mut main_channel_open_tx = Some(main_channel_open_tx);
    main_channel.on_open(move || {
        if let Some(tx) = main_channel_open_tx.take() {
            let _ = tx.send(());
        }
        std::future::ready(())
    });
    #[cfg(feature = "p2p-webrtc-ready-events")]
    let ch0_listener = ready::Ch0Listener::listen(&main_channel);

    let answer = if is_outgoing {
        let answer_fut = async {
            let sdp = pc.local_sdp().await.unwrap();
            event_sender(P2pConnectionEvent::OfferSdpReady(peer_id, Ok(sdp)).into())
                .ok_or(Error::ChannelClosed)?;
            match cmd_receiver.recv().await.ok_or(Error::ChannelClosed)?.0 {
                PeerCmd::PeerHttpOfferSend(url, offer) => {
                    let answer = webrtc_signal_send(&url, offer).await?;
                    event_sender(P2pConnectionEvent::AnswerReceived(peer_id, answer).into())
                        .ok_or(Error::ChannelClosed)?;

                    if let PeerCmd::AnswerSet(v) =
                        cmd_receiver.recv().await.ok_or(Error::ChannelClosed)?.0
                    {
                        return Ok(v);
                    }
                }
                PeerCmd::AnswerSet(v) => return Ok(v),
                _cmd => {
                    return Err(Error::UnexpectedCmd);
                }
            }
            Err(Error::ChannelClosed)
        };
        answer_fut.await.and_then(|v| Ok(v.try_into()?))
    } else {
        pc.answer_create().await.map_err(Error::from)
    };
    let Ok(answer) = answer else {
        return;
    };

    if is_outgoing {
        if let Err(err) = pc.remote_desc_set(answer).await {
            let err = Error::from(err).to_string();
            let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, Err(err)).into());
        }
    } else {
        let fut = async {
            pc.local_desc_set(answer).await?;
            wait_for_ice_gathering_complete(&mut pc).await;
            Ok(pc.local_sdp().await.unwrap())
        };
        let res = fut.await.map_err(|err: Error| err.to_string());
        let is_err = res.is_err();
        let is_err = is_err
            || event_sender(P2pConnectionEvent::AnswerSdpReady(peer_id, res).into()).is_none();
        if is_err {
            return;
        }
    }

    let (connected_tx, connected) = oneshot::channel();
    if matches!(pc.connection_state(), RTCConnectionState::Connected) {
        connected_tx.send(Ok(())).unwrap();
    } else {
        let mut connected_tx = Some(connected_tx);
        pc.on_connection_state_change(Box::new(move |state| {
            match state {
                RTCConnectionState::Connected => {
                    if let Some(connected_tx) = connected_tx.take() {
                        let _ = connected_tx.send(Ok(()));
                    }
                }
                #[cfg(not(feature = "p2p-webrtc-ready-events"))]
                RTCConnectionState::Disconnected | RTCConnectionState::Closed => {
                    if let Some(connected_tx) = connected_tx.take() {
                        let _ = connected_tx.send(Err("disconnected"));
                    } else {
                        let _ = closed.try_send(());
                    }
                }
                // A connection whose ICE checks fail goes to `Failed` without
                // `Disconnected`; it ends here instead of waiting for the
                // state machine's timeout.
                #[cfg(feature = "p2p-webrtc-ready-events")]
                RTCConnectionState::Disconnected
                | RTCConnectionState::Closed
                | RTCConnectionState::Failed => {
                    if let Some(connected_tx) = connected_tx.take() {
                        let _ = connected_tx.send(Err("disconnected"));
                    } else {
                        let _ = closed.try_send(());
                    }
                }
                _ => {}
            }
            Box::pin(std::future::ready(()))
        }));
    }
    match connected
        .await
        .map_err(|_| Error::ChannelClosed.to_string())
        .and_then(|res| res.map_err(|v| v.to_string()))
    {
        Ok(_) => {}
        Err(err) => {
            let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, Err(err)).into());
            return;
        }
    }

    // Exchange encrypted connection authorization messsages. Makes sure
    // there is a link between peer identity and connection.
    #[cfg(feature = "p2p-webrtc-ready-events")]
    {
        let auth = match cmd_receiver.recv().await {
            None => return,
            Some(msg) => match msg.0 {
                PeerCmd::ConnectionAuthorizationSend(Some(auth)) => auth,
                PeerCmd::ConnectionAuthorizationSend(None) => return,
                cmd => {
                    bug_condition!("unexpected peer cmd! Expected `PeerCmd::ConnectionAuthorizationSend`. received: {cmd:?}");
                    return;
                }
            },
        };
        let _ = main_channel_open.await;
        let (remote_auth_rx, ready) = ready::start(main_channel, ch0_listener, auth);
        let res = remote_auth_rx
            .await
            .map_err(|_| "didn't receive connection authentication message".to_owned());
        let is_err = res.is_err();
        let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, res).into());
        if is_err {
            return;
        }
        // Channel 0 stays open: it carries the ready messages.
        peer_loop(peer_id, event_sender, cmd_receiver, pc, abort, ready).await
    }
    #[cfg(not(feature = "p2p-webrtc-ready-events"))]
    {
        let (remote_auth_tx, remote_auth_rx) = oneshot::channel::<ConnectionAuthEncrypted>();
        let mut remote_auth_tx = Some(remote_auth_tx);
        main_channel.on_message(move |data| {
            if let Some(tx) = remote_auth_tx.take() {
                if let Ok(auth) = data.try_into() {
                    let _ = tx.send(auth);
                }
            }
            #[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
            std::future::ready(())
        });
        let msg = match cmd_receiver.recv().await {
            None => return,
            Some(msg) => msg,
        };
        match msg.0 {
            PeerCmd::ConnectionAuthorizationSend(None) => {
                // eprintln!("PeerCmd::ConnectionAuthorizationSend(None)");
                return;
            }
            PeerCmd::ConnectionAuthorizationSend(Some(auth)) => {
                let _ = main_channel_open.await;

                // Add a delay for sending messages after channel
                // was opened. Some initial messages get lost otherwise.
                // TODO(binier): find deeper cause and fix it.
                sleep(Duration::from_secs(1)).await;
                let _ = main_channel
                    .send(&bytes::Bytes::copy_from_slice(auth.as_ref()))
                    .await;

                let res = match remote_auth_rx.await {
                    Err(_) => Err("didn't receive connection authentication message".to_owned()),
                    Ok(remote_auth) => Ok(remote_auth),
                };
                let is_err = res.is_err();
                let _ = event_sender(P2pConnectionEvent::Finalized(peer_id, res).into());
                if is_err {
                    return;
                }
            }
            cmd => {
                bug_condition!("unexpected peer cmd! Expected `PeerCmd::ConnectionAuthorizationSend`. received: {cmd:?}");
                return;
            }
        }

        let _ = main_channel.close().await;

        peer_loop(peer_id, event_sender, cmd_receiver, pc, abort).await
    }
}

/// A channel buffer keeps at most this much capacity between messages
/// (WN-MEM). `Vec::clear` keeps the capacity of the largest message the channel
/// has carried, such as a staged-ledger answer of tens of MB on the RPC
/// channel, and in a web node's WASM memory that stayed taken for the life of
/// the connection.
const KEEP_BUF_CAPACITY: usize = 1024 * 1024;

/// Empty `buf` for the next message, freeing a large allocation.
fn release_large(buf: &mut Vec<u8>) {
    buf.clear();
    if buf.capacity() > KEEP_BUF_CAPACITY {
        *buf = Vec::new();
    }
}

/// Append `bytes` of a message of `len` bytes. Grows by doubling, as `Vec`
/// would, so the allocation stays paced by the bytes received, but never past
/// `len`: doubling a 40 MB message's buffer would otherwise take 64 MB, and
/// copying it 96 MB at once.
fn extend_within(buf: &mut Vec<u8>, bytes: &[u8], len: usize) {
    let need = buf.len().saturating_add(bytes.len());
    if need > buf.capacity() {
        let target = need
            .max(buf.capacity().saturating_mul(2))
            .min(len.max(need));
        buf.reserve_exact(target - buf.len());
    }
    buf.extend_from_slice(bytes);
}

/// Take the next length-prefixed message of `chan_id` from `msg`, which holds
/// one chunk received on the channel; `buf` and `len` carry a partly received
/// message between chunks.
fn process_msg(
    chan_id: ChannelId,
    buf: &mut Vec<u8>,
    len: &mut u32,
    msg: &mut &[u8],
) -> Result<Option<ChannelMsg>, String> {
    let len = if buf.is_empty() {
        if msg.len() < 4 {
            return Err("WebRTCMessageTooSmall".to_owned());
        } else {
            *len = u32::from_be_bytes(msg[..4].try_into().expect("Size checked above"));
            *msg = &msg[4..];
            let len = *len as usize;
            if len > chan_id.max_msg_size() {
                return Err(format!(
                    "ChannelMsgLenOverLimit; len: {}, limit: {}",
                    len,
                    chan_id.max_msg_size()
                ));
            }
            len
        }
    } else {
        *len as usize
    };
    let bytes_left = len - buf.len();

    if bytes_left > msg.len() {
        extend_within(buf, msg, len);
        *msg = &[];
        return Ok(None);
    }

    extend_within(buf, &msg[..bytes_left], len);
    *msg = &msg[bytes_left..];
    let msg = ChannelMsg::decode(&mut &buf[..], chan_id).map_err(|err| err.to_string())?;
    release_large(buf);
    Ok(Some(msg))
}

struct Channel {
    id: ChannelId,
    msg_sender: ChannelMsgSender,
}

type ChannelMsgSender = mpsc::UnboundedSender<(MsgId, Vec<u8>, Option<mpsc::Tracker>)>;

struct MsgBuffer {
    buf: Vec<u8>,
}

impl MsgBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
        }
    }

    fn encode(&mut self, msg: &ChannelMsg) -> Result<Vec<u8>, std::io::Error> {
        self.buf.clear();
        if let Err(error) = msg.encode(&mut self.buf) {
            self.buf.clear();
            return Err(error);
        }
        let len_encoded = (self.buf.len() as u32).to_be_bytes();
        let encoded = len_encoded
            .into_iter()
            .chain(self.buf.iter().cloned())
            .collect();
        release_large(&mut self.buf);
        Ok(encoded)
    }
}

/// Reassembles length-prefixed channel messages from data channel chunks.
///
/// After the first framing or decode error the stream position is unknown, so
/// the rest of that chunk and every later chunk on the channel are dropped;
/// the single error reported makes the caller disconnect the peer.
struct MsgReassembler {
    chan_id: ChannelId,
    buf: Vec<u8>,
    len: u32,
    failed: bool,
}

impl MsgReassembler {
    fn new(chan_id: ChannelId) -> Self {
        Self {
            chan_id,
            buf: Vec::new(),
            len: 0,
            failed: false,
        }
    }

    fn feed(&mut self, mut data: &[u8], mut on_msg: impl FnMut(Result<ChannelMsg, String>)) {
        if self.failed {
            return;
        }
        while !data.is_empty() {
            match process_msg(self.chan_id, &mut self.buf, &mut self.len, &mut data) {
                Ok(None) => {}
                Ok(Some(msg)) => on_msg(Ok(msg)),
                Err(err) => {
                    self.failed = true;
                    self.buf = Vec::new();
                    self.len = 0;
                    on_msg(Err(err));
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod reassembler_tests {
    use super::*;
    use crate::channels::rpc::RpcChannelMsg;

    fn feed_all(r: &mut MsgReassembler, chunks: &[&[u8]]) -> Vec<Result<ChannelMsg, String>> {
        let mut out = Vec::new();
        for chunk in chunks {
            r.feed(chunk, |res| out.push(res));
        }
        out
    }

    fn encoded(msg: &ChannelMsg) -> Vec<u8> {
        MsgBuffer::new(1024).encode(msg).unwrap()
    }

    #[test]
    fn short_first_chunk_reports_one_error_and_stops() {
        let mut r = MsgReassembler::new(ChannelId::Rpc);
        let valid = encoded(&ChannelMsg::Rpc(RpcChannelMsg::Response(1, None)));
        let out = feed_all(&mut r, &[&[0u8], &valid]);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_err());
    }

    #[test]
    fn undecodable_message_reports_one_error_and_stops() {
        let mut r = MsgReassembler::new(ChannelId::Rpc);
        let mut bad = 3u32.to_be_bytes().to_vec();
        bad.extend_from_slice(&[0xff, 0xff, 0xff]);
        let valid = encoded(&ChannelMsg::Rpc(RpcChannelMsg::Response(1, None)));
        let mut chunk = bad.clone();
        chunk.extend_from_slice(&valid);
        let out = feed_all(&mut r, &[&chunk, &valid]);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_err());
        assert!(r.buf.is_empty());
    }

    /// A message that fails to decode gives back its buffer, however large.
    #[test]
    fn large_undecodable_message_frees_its_buffer() {
        let mut r = MsgReassembler::new(ChannelId::Rpc);
        let len = 2 * KEEP_BUF_CAPACITY;
        let mut bad = (len as u32).to_be_bytes().to_vec();
        bad.resize(4 + len, 0xff);
        let chunks: Vec<&[u8]> = bad.chunks(CHUNK_SIZE).collect();
        let out = feed_all(&mut r, &chunks);
        assert_eq!(out.len(), 1);
        assert!(out[0].is_err());
        assert_eq!(r.buf.capacity(), 0);
    }

    #[test]
    fn valid_messages_split_across_chunks_are_reassembled() {
        let mut r = MsgReassembler::new(ChannelId::Rpc);
        let a = encoded(&ChannelMsg::Rpc(RpcChannelMsg::Response(1, None)));
        let b = encoded(&ChannelMsg::Rpc(RpcChannelMsg::Response(2, None)));
        let mut both = a.clone();
        both.extend_from_slice(&b);
        let out = feed_all(&mut r, &[&a[..5], &a[5..], &b, &both]);
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|r| r.is_ok()));
    }
}

#[cfg(test)]
mod reassembly_tests {
    use super::*;
    use crate::channels::rpc::{P2pRpcResponse, RpcChannelMsg};
    use mina_p2p_messages::{
        binprot::BinProtRead, rpc::GetBestTipV2Response, rpc_kernel::ResponsePayload,
    };

    fn block_response() -> ChannelMsg {
        let bytes = include_bytes!("../../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let response =
            ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut bytes.as_slice()).unwrap();
        let block = Arc::new(response.0.unwrap().0.unwrap().data);
        ChannelMsg::Rpc(RpcChannelMsg::Response(
            7,
            Some(P2pRpcResponse::Block(block)),
        ))
    }

    /// A message sent in 16 KiB chunks, as `ChannelSend` does, comes out whole,
    /// and the buffer never holds more than the message.
    #[test]
    fn chunked_message_reassembles_within_its_length() {
        let msg = block_response();
        let encoded = MsgBuffer::new(64 * 1024).encode(&msg).unwrap();
        let body_len = encoded.len() - 4;
        assert!(body_len > 2 * CHUNK_SIZE, "the test needs several chunks");
        let (mut buf, mut len) = (Vec::new(), 0u32);
        let mut decoded = Vec::new();
        for chunk in encoded.chunks(CHUNK_SIZE) {
            let mut data = chunk;
            while !data.is_empty() {
                if let Some(msg) =
                    process_msg(msg.channel_id(), &mut buf, &mut len, &mut data).unwrap()
                {
                    decoded.push(msg);
                }
                assert!(buf.capacity() <= body_len);
            }
        }
        assert_eq!(decoded.len(), 1);
        let mut again = Vec::new();
        decoded[0].encode(&mut again).unwrap();
        assert_eq!(again, encoded[4..]);
        assert!(buf.is_empty());
    }

    /// Two messages in one chunk both come out.
    #[test]
    fn two_messages_in_one_chunk() {
        let a = ChannelMsg::Rpc(RpcChannelMsg::Response(1, None));
        let b = ChannelMsg::Rpc(RpcChannelMsg::Response(2, None));
        let mut encoder = MsgBuffer::new(1024);
        let mut chunk = encoder.encode(&a).unwrap();
        chunk.extend(encoder.encode(&b).unwrap());
        let (mut buf, mut len) = (Vec::new(), 0u32);
        let mut data = &chunk[..];
        let first = process_msg(a.channel_id(), &mut buf, &mut len, &mut data).unwrap();
        let second = process_msg(a.channel_id(), &mut buf, &mut len, &mut data).unwrap();
        assert!(matches!(
            first,
            Some(ChannelMsg::Rpc(RpcChannelMsg::Response(1, None)))
        ));
        assert!(matches!(
            second,
            Some(ChannelMsg::Rpc(RpcChannelMsg::Response(2, None)))
        ));
        assert!(data.is_empty());
    }

    #[test]
    fn large_buffers_are_freed_between_messages() {
        let mut small = Vec::with_capacity(KEEP_BUF_CAPACITY);
        small.extend_from_slice(&[1; 100]);
        release_large(&mut small);
        assert!(small.is_empty());
        assert_eq!(small.capacity(), KEEP_BUF_CAPACITY);

        let mut large = Vec::with_capacity(KEEP_BUF_CAPACITY + 1);
        large.extend_from_slice(&[1; 100]);
        release_large(&mut large);
        assert!(large.is_empty());
        assert_eq!(large.capacity(), 0);
    }

    #[test]
    fn growth_doubles_but_stops_at_the_length() {
        let len = 40 * 1024 * 1024;
        let mut buf = Vec::new();
        let chunk = vec![0u8; CHUNK_SIZE];
        let mut capacities = Vec::new();
        while buf.len() < len {
            let take = CHUNK_SIZE.min(len - buf.len());
            extend_within(&mut buf, &chunk[..take], len);
            if capacities.last() != Some(&buf.capacity()) {
                capacities.push(buf.capacity());
            }
        }
        assert_eq!(buf.capacity(), len);
        // Paced by the bytes received: about log2(len / CHUNK_SIZE) steps.
        assert!(capacities.len() < 16, "{capacities:?}");
        assert!(capacities.windows(2).all(|w| w[1] <= 2 * w[0]));
    }
}

#[cfg(test)]
mod pc2_tests {
    use super::*;
    use crate::channels::rpc::{P2pRpcResponse, RpcChannelMsg};
    use mina_p2p_messages::{
        binprot::BinProtRead, proof_cache::ProofStore, rpc::GetBestTipV2Response,
        rpc_kernel::ResponsePayload, v2,
    };

    #[test]
    fn pc2_unavailable_proof_does_not_poison_the_next_channel_message() {
        let bytes = include_bytes!("../../../../p2p-messages/tests/files/mesa/get_best_tip_v3.bin");
        let response =
            ResponsePayload::<GetBestTipV2Response>::binprot_read(&mut bytes.as_slice()).unwrap();
        let block = Arc::new(response.0.unwrap().0.unwrap().data);
        let work = block
            .body
            .staged_ledger_diff
            .diff
            .0
            .completed_works
            .iter()
            .next()
            .unwrap();
        let proof = match &work.proofs {
            v2::TransactionSnarkWorkTStableV2Proofs::One(p) => &p.0.proof,
            v2::TransactionSnarkWorkTStableV2Proofs::Two((p, _)) => &p.0.proof,
        };
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/test-data/proof-cache/channel-{}",
            std::process::id()
        ));
        let store = ProofStore::dir(&dir).unwrap();
        proof.cache_in(&store).unwrap();
        let file = std::fs::read_dir(&dir)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::remove_file(file).unwrap();
        let msg = ChannelMsg::Rpc(RpcChannelMsg::Response(
            1,
            Some(P2pRpcResponse::Block(block)),
        ));
        let mut buffer = MsgBuffer::new(1024);
        assert!(buffer.encode(&msg).is_err());
        assert!(buffer.buf.is_empty());
        let valid = ChannelMsg::Rpc(RpcChannelMsg::Response(2, None));
        assert_eq!(
            buffer.encode(&valid).unwrap(),
            MsgBuffer::new(1024).encode(&valid).unwrap()
        );
        drop(msg);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

struct Channels {
    list: Vec<Channel>,
}

impl Channels {
    fn new() -> Self {
        Self {
            list: Vec::with_capacity(32),
        }
    }

    fn get_msg_sender(&self, id: ChannelId) -> Option<&ChannelMsgSender> {
        self.list.iter().find(|c| c.id == id).map(|c| &c.msg_sender)
    }

    fn add(&mut self, id: ChannelId, msg_sender: ChannelMsgSender) {
        self.list.push(Channel { id, msg_sender });
    }

    fn remove(&mut self, id: ChannelId) -> bool {
        match self.list.iter().position(|c| c.id == id) {
            None => false,
            Some(index) => {
                self.list.remove(index);
                true
            }
        }
    }
}

// TODO(binier): remove unwraps
#[allow(unused_mut)]
async fn peer_loop(
    peer_id: PeerId,
    event_sender: Arc<dyn Fn(P2pEvent) -> Option<()> + Send + Sync + 'static>,
    mut cmd_receiver: mpsc::TrackedUnboundedReceiver<PeerCmd>,
    mut pc: RTCConnection,
    aborted: Aborted,
    #[cfg(feature = "p2p-webrtc-ready-events")] ready: ready::Ready,
) {
    // TODO(binier): maybe use small_vec (stack allocated) or something like that.
    let mut channels = Channels::new();
    let mut msg_buf = MsgBuffer::new(64 * 1024);

    let (internal_cmd_sender, mut internal_cmd_receiver) =
        mpsc::unbounded_channel::<PeerCmdInternal>();

    while matches!(pc.connection_state(), RTCConnectionState::Connected) {
        let (cmd, _tracker) = tokio::select! {
            cmd = cmd_receiver.recv() => match cmd {
                None => return,
                Some(cmd) => (PeerCmdAll::External(Box::new(cmd.0)), Some(cmd.1)),
            },
            cmd = internal_cmd_receiver.recv() => match cmd {
                None => return,
                Some(cmd) => (PeerCmdAll::Internal(cmd), None),
            },
        };
        match cmd {
            PeerCmdAll::External(cmd) => match *cmd {
                PeerCmd::PeerHttpOfferSend(..)
                | PeerCmd::AnswerSet(_)
                | PeerCmd::ConnectionAuthorizationSend(_) => {
                    bug_condition!("unexpected peer cmd");
                }
                PeerCmd::ChannelOpen(id) => {
                    let chan = pc
                        .channel_create(RTCChannelConfig {
                            label: id.name(),
                            negotiated: Some(id.to_u16()),
                        })
                        .await;
                    let internal_cmd_sender = internal_cmd_sender.clone();
                    let fut = async move {
                        let internal_cmd_sender_clone = internal_cmd_sender.clone();
                        let result = async move {
                            let chan = chan?;

                            let (done_tx, mut done_rx) = mpsc::channel::<Result<(), Error>>(1);

                            let done_tx_clone = done_tx.clone();
                            chan.on_open(move || {
                                let _ = done_tx_clone.try_send(Ok(()));
                                std::future::ready(())
                            });

                            let done_tx_clone = done_tx.clone();
                            let internal_cmd_sender = internal_cmd_sender_clone.clone();
                            chan.on_error(move |err| {
                                if done_tx_clone.try_send(Err(err.into())).is_err() {
                                    let _ = internal_cmd_sender
                                        .send(PeerCmdInternal::ChannelClosed(id));
                                }
                                std::future::ready(())
                            });

                            let done_tx_clone = done_tx.clone();
                            let internal_cmd_sender = internal_cmd_sender_clone.clone();
                            chan.on_close(move || {
                                if done_tx_clone.try_send(Err(Error::ChannelClosed)).is_err() {
                                    let _ = internal_cmd_sender
                                        .send(PeerCmdInternal::ChannelClosed(id));
                                }
                                std::future::ready(())
                            });

                            done_rx.recv().await.ok_or(Error::ChannelClosed)??;

                            Ok(chan)
                        };

                        let _ = internal_cmd_sender
                            .send(PeerCmdInternal::ChannelOpened(id, result.await));
                    };
                    let mut aborted = aborted.clone();
                    spawn_local(async move {
                        tokio::select! {
                            _ = aborted.wait() => {}
                            _ = fut => {}
                        }
                    });
                }
                PeerCmd::ChannelSend(msg_id, msg) => {
                    let id = msg.channel_id();
                    let err = match channels.get_msg_sender(id) {
                        Some(msg_sender) => match msg_buf.encode(&msg) {
                            Ok(encoded) => match msg_sender.send((msg_id, encoded, _tracker)) {
                                Ok(_) => None,
                                Err(_) => Some("ChannelMsgMpscSendFailed".to_owned()),
                            },
                            Err(err) => Some(err.to_string()),
                        },
                        None => Some("ChannelNotOpen".to_owned()),
                    };
                    if let Some(err) = err {
                        let _ = event_sender(
                            P2pChannelEvent::Sent(peer_id, id, msg_id, Err(err)).into(),
                        );
                    }
                }
            },
            PeerCmdAll::Internal(PeerCmdInternal::ChannelOpened(chan_id, result)) => {
                let (sender_tx, mut sender_rx) = mpsc::unbounded_channel();
                let (chan, res) = match result {
                    Ok(chan) => {
                        channels.add(chan_id, sender_tx);
                        (Some(chan), Ok(()))
                    }
                    Err(err) => (None, Err(err.to_string())),
                };

                #[allow(unused_mut)]
                if let Some(mut chan) = chan {
                    let mut reassembler = MsgReassembler::new(chan_id);
                    let event_sender_clone = event_sender.clone();

                    chan.on_message(move |data| {
                        reassembler.feed(data, |res| {
                            let _ = event_sender_clone(
                                P2pChannelEvent::Received(peer_id, res.map(Box::new)).into(),
                            );
                        });
                        #[cfg(not(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-cpp")))]
                        std::future::ready(())
                    });
                    // Our handler is registered: the peer may send.
                    #[cfg(feature = "p2p-webrtc-ready-events")]
                    ready.announce(chan_id);
                    #[cfg(feature = "p2p-webrtc-ready-events")]
                    let ready = ready.clone();

                    let event_sender = event_sender.clone();
                    let fut = async move {
                        // Add a delay for sending messages after channel
                        // was opened. Some initial messages get lost otherwise.
                        // TODO(binier): find deeper cause and fix it.
                        #[cfg(not(feature = "p2p-webrtc-ready-events"))]
                        sleep(Duration::from_secs(3)).await;
                        // Until the peer's channel is listening (ready.rs).
                        #[cfg(feature = "p2p-webrtc-ready-events")]
                        ready.wait(chan_id).await;

                        while let Some((msg_id, encoded, _tracker)) = sender_rx.recv().await {
                            let encoded = bytes::Bytes::from(encoded);
                            let mut chunks =
                                encoded.chunks(CHUNK_SIZE).map(|b| encoded.slice_ref(b));
                            let result = loop {
                                let Some(chunk) = chunks.next() else {
                                    break Ok(());
                                };
                                if let Err(err) = chan
                                    .send(&chunk)
                                    .await
                                    .map_err(|e| format!("{e:?}"))
                                    .and_then(|n| match n == chunk.len() {
                                        false => Err("NotAllBytesWritten".to_owned()),
                                        true => Ok(()),
                                    })
                                {
                                    break Err(err);
                                }
                            };

                            let _ = event_sender(
                                P2pChannelEvent::Sent(peer_id, chan_id, msg_id, result).into(),
                            );
                        }
                    };

                    let mut aborted = aborted.clone();
                    spawn_local(async move {
                        tokio::select! {
                            _ = aborted.wait() => {}
                            _ = fut => {}
                        }
                    });
                }

                let _ = event_sender(P2pChannelEvent::Opened(peer_id, chan_id, res).into());
            }
            PeerCmdAll::Internal(PeerCmdInternal::ChannelClosed(id)) => {
                channels.remove(id);
                let _ = event_sender(P2pChannelEvent::Closed(peer_id, id).into());
            }
        }
    }
}

pub trait P2pServiceWebrtc: redux::Service {
    type Event: From<P2pEvent> + Send + Sync + 'static;

    fn random_pick(
        &mut self,
        list: &[P2pConnectionOutgoingInitOpts],
    ) -> Option<P2pConnectionOutgoingInitOpts>;

    fn event_sender(&self) -> &mpsc::UnboundedSender<Self::Event>;

    fn cmd_sender(&self) -> &mpsc::TrackedUnboundedSender<Cmd>;

    fn peers(&mut self) -> &mut BTreeMap<PeerId, PeerState>;

    fn init<S: TaskSpawner>(
        secret_key: SecretKey,
        spawner: S,
        rng_seed: [u8; 32],
        config: webrtc::WebrtcConfig,
    ) -> P2pServiceCtx {
        const MAX_PEERS: usize = 500;
        let (cmd_sender, mut cmd_receiver) = mpsc::tracked_unbounded_channel();

        let certificate = certificate_from_pem_key(secret_key.to_pem().as_str());
        let ice_servers = Arc::new(config.ice_servers());

        spawner.spawn_main("webrtc", async move {
            #[allow(clippy::all)]
            let api = build_api(&config);
            let conn_permits = Arc::new(Semaphore::const_new(MAX_PEERS));
            while let Some(cmd) = cmd_receiver.recv().await {
                match cmd.0 {
                    Cmd::PeerAdd { args, aborted } => {
                        #[allow(clippy::all)]
                        let api = api.clone();
                        let conn_permits = conn_permits.clone();
                        let peer_id = args.peer_id;
                        let event_sender = args.event_sender.clone();
                        let certificate = certificate.clone();
                        let ice_servers = ice_servers.clone();
                        spawn_local(async move {
                            let Ok(_permit) = conn_permits.try_acquire() else {
                                // state machine shouldn't allow this to happen.
                                bug_condition!("P2P WebRTC Semaphore acquisition failed!");
                                return;
                            };
                            let (closed_tx, mut closed) = mpsc::channel(1);
                            let event_sender_clone = event_sender.clone();
                            spawn_local(async move {
                                // to avoid sending closed multiple times
                                let _ = closed.recv().await;
                                event_sender_clone(P2pConnectionEvent::Closed(peer_id).into());
                            });
                            tokio::select! {
                                _ = peer_start(api, args, aborted.clone(), closed_tx.clone(), certificate, rng_seed, ice_servers) => {}
                                _ = aborted.wait() => {
                                }
                            }

                            // delay dropping permit to give some time for cleanup.
                            sleep(Duration::from_millis(100)).await;
                            let _ = closed_tx.send(()).await;
                        });
                    }
                }
            }
        });

        P2pServiceCtx {
            cmd_sender,
            peers: Default::default(),
        }
    }

    fn outgoing_init(&mut self, peer_id: PeerId) {
        let (peer_cmd_sender, peer_cmd_receiver) = mpsc::tracked_unbounded_channel();
        let aborter = Aborter::default();
        let aborted = aborter.aborted();

        self.peers().insert(
            peer_id,
            PeerState {
                cmd_sender: peer_cmd_sender,
                abort: aborter,
            },
        );
        let event_sender = self.event_sender().clone();
        let event_sender =
            Arc::new(move |p2p_event: P2pEvent| event_sender.send(p2p_event.into()).ok());
        let _ = self.cmd_sender().tracked_send(Cmd::PeerAdd {
            args: PeerAddArgs {
                peer_id,
                kind: PeerConnectionKind::Outgoing,
                event_sender,
                cmd_receiver: peer_cmd_receiver,
            },
            aborted,
        });
    }

    fn incoming_init(&mut self, peer_id: PeerId, offer: webrtc::Offer) {
        let (peer_cmd_sender, peer_cmd_receiver) = mpsc::tracked_unbounded_channel();
        let aborter = Aborter::default();
        let aborted = aborter.aborted();

        self.peers().insert(
            peer_id,
            PeerState {
                cmd_sender: peer_cmd_sender,
                abort: aborter,
            },
        );
        let event_sender = self.event_sender().clone();
        let event_sender =
            Arc::new(move |p2p_event: P2pEvent| event_sender.send(p2p_event.into()).ok());
        let _ = self.cmd_sender().tracked_send(Cmd::PeerAdd {
            args: PeerAddArgs {
                peer_id,
                kind: PeerConnectionKind::Incoming(Box::new(offer)),
                event_sender,
                cmd_receiver: peer_cmd_receiver,
            },
            aborted,
        });
    }

    fn set_answer(&mut self, peer_id: PeerId, answer: webrtc::Answer) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer.cmd_sender.tracked_send(PeerCmd::AnswerSet(answer));
        }
    }

    fn http_signaling_request(&mut self, url: String, offer: webrtc::Offer) {
        if let Some(peer) = self.peers().get(&offer.target_peer_id) {
            let _ = peer
                .cmd_sender
                .tracked_send(PeerCmd::PeerHttpOfferSend(url, offer));
        }
    }

    fn disconnect(&mut self, peer_id: PeerId) -> bool {
        // TODO(binier): improve
        // By removing the peer, `abort` gets dropped which will
        // cause `peer_loop` to end.
        if let Some(_peer) = self.peers().remove(&peer_id) {
            // if peer.abort.receiver_count() > 0 {
            //     // peer disconnection not yet finished
            //     return false;
            // }
        } else {
            mina_core::error!(mina_core::log::system_time(); "`disconnect` shouldn't be used for libp2p peers");
        }
        true
    }

    fn channel_open(&mut self, peer_id: PeerId, id: ChannelId) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer.cmd_sender.tracked_send(PeerCmd::ChannelOpen(id));
        }
    }

    fn channel_send(&mut self, peer_id: PeerId, msg_id: MsgId, msg: ChannelMsg) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer
                .cmd_sender
                .tracked_send(PeerCmd::ChannelSend(msg_id, msg));
        }
    }

    fn encrypt<T: EncryptableType>(
        &mut self,
        other_pk: &PublicKey,
        message: &T,
    ) -> Result<T::Encrypted, Box<dyn std::error::Error>>;

    fn decrypt<T: EncryptableType>(
        &mut self,
        other_pub_key: &PublicKey,
        encrypted: &T::Encrypted,
    ) -> Result<T, Box<dyn std::error::Error>>;

    fn auth_send(
        &mut self,
        peer_id: PeerId,
        _other_pub_key: &PublicKey,
        auth: Option<ConnectionAuthEncrypted>,
    ) {
        if let Some(peer) = self.peers().get(&peer_id) {
            let _ = peer
                .cmd_sender
                .tracked_send(PeerCmd::ConnectionAuthorizationSend(auth));
        }
    }

    fn auth_encrypt_and_send(
        &mut self,
        peer_id: PeerId,
        other_pub_key: &PublicKey,
        auth: ConnectionAuth,
    );

    fn auth_decrypt(
        &mut self,
        other_pub_key: &PublicKey,
        auth: ConnectionAuthEncrypted,
    ) -> Option<ConnectionAuth>;
}

impl P2pServiceCtx {
    pub fn pending_cmds(&self) -> usize {
        self.peers
            .iter()
            .fold(self.cmd_sender.len(), |acc, (_, peer)| {
                acc + peer.cmd_sender.len()
            })
    }
}
