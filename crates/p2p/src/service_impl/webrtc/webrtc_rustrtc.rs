//! The rustrtc backend (WN2-RTC prototype): the same interface as
//! `webrtc_rs`, over <https://github.com/restsend/rustrtc>.
//!
//! rustrtc reports data channel and connection events through streams
//! (`DataChannel::recv`, `subscribe_peer_state`), while this interface
//! registers callbacks. Each channel therefore runs one pump task that
//! dispatches its events to the callbacks registered so far, and keeps the
//! messages that arrive before `on_message` until it is registered.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use rustrtc::{
    transports::datachannel::{DataChannel, DataChannelConfig, DataChannelEvent, MESSAGE_OVERHEAD},
    IceCredentialType, IceServer, PeerConnection, PeerConnectionState, RtcConfiguration, SdpType,
    SessionDescription,
};
use tokio::{sync::watch, task::spawn_local};

use crate::{
    connection::P2pConnectionResponse,
    webrtc::{Answer, Offer, WebrtcConfig},
};

use super::{OnConnectionStateChangeHdlrFn, RTCChannelConfig, RTCConfig};

pub type Result<T> = std::result::Result<T, rustrtc::RtcError>;

const ICE_DISCONNECT: Duration = Duration::from_secs(4);
const ICE_FAILED: Duration = Duration::from_secs(6);
const ICE_DISCONNECT_GRACE: Duration = Duration::from_secs(1);
/// Messages kept for a channel before its `on_message` is registered, at
/// most, each counted with rustrtc's per-message overhead; past it the
/// connection is closed. rustrtc bounds what its own queues hold the same way
/// (4 MiB unread per association).
const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;

pub type RTCConnectionState = PeerConnectionState;

/// The settings every connection shares: rustrtc has no API object, the
/// candidates' ports and addresses go in each connection's configuration.
pub type Api = Arc<ApiConfig>;

#[derive(Debug, Default)]
pub struct ApiConfig {
    udp_ports: Option<(u16, u16)>,
    nat_ip: Option<String>,
}

/// rustrtc makes a fresh self-signed DTLS certificate per connection, as
/// the `webrtc_rs` backend does (it clears the configured one).
pub type RTCCertificate = ();

pub fn certificate_from_pem_key(_: &str) -> RTCCertificate {}

/// The WebRTC settings of `config` (W1): the candidates' UDP port range and
/// the NAT address. rustrtc advertises one external address, the first.
pub fn build_api(config: &WebrtcConfig) -> Api {
    if config.nat_ips.len() > 1 {
        mina_core::warn!(
            mina_core::log::system_time();
            summary = "WebRTC (rustrtc) uses only the first NAT address",
            nat_ips = config.nat_ips.len(),
        );
    }
    Arc::new(ApiConfig {
        udp_ports: config.udp_ports.map(|ports| (ports.min, ports.max)),
        nat_ip: config.nat_ips.first().map(ToString::to_string),
    })
}

pub struct RTCConnection(PeerConnection, bool);

type BoxFut = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type MessageHandler = Box<dyn FnMut(&[u8]) -> BoxFut + Send + Sync>;

#[derive(Default)]
struct Handlers {
    opened: bool,
    closed: bool,
    on_open: Option<Box<dyn FnOnce() -> BoxFut + Send + Sync>>,
    on_message: Option<MessageHandler>,
    on_close: Option<Box<dyn FnMut() -> BoxFut + Send + Sync>>,
    pending: VecDeque<bytes::Bytes>,
    pending_bytes: usize,
}

#[derive(Clone)]
pub struct RTCChannel {
    pc: PeerConnection,
    dc: Arc<DataChannel>,
    handlers: Arc<Mutex<Handlers>>,
}

#[derive(thiserror::Error, derive_more::From, Debug)]
pub enum RTCSignalingError {
    #[error("http request failed: {0}")]
    Http(reqwest::Error),
}

impl RTCConnection {
    pub async fn create(api: &Api, config: RTCConfig) -> Result<Self> {
        let mut configuration = RtcConfiguration {
            ice_servers: config.ice_servers.0.into_iter().map(Into::into).collect(),
            external_ip: api.nat_ip.clone(),
            // Data channels only: no media, no bandwidth estimation.
            enable_gcc: false,
            // A peer is `Disconnected` after this long without a packet
            // (rustrtc sends a STUN keepalive every second), which the driver
            // treats as closed: a closed tab frees its peer slot, and a dead
            // relay is noticed, within about 5 s. Defaults: 30 s and 120 s.
            ice_disconnect_threshold: ICE_DISCONNECT,
            ice_connection_timeout: ICE_FAILED,
            ice_disconnect_grace: ICE_DISCONNECT_GRACE,
            // Our channels are all negotiated: a channel the peer opens in
            // band is refused, and its data dropped.
            accept_remote_data_channels: false,
            ..Default::default()
        };
        if let Some((min, max)) = api.udp_ports {
            configuration.rtp_start_port = Some(min);
            configuration.rtp_end_port = Some(max);
        }
        Ok(Self(PeerConnection::new(configuration), true))
    }

    pub fn is_main(&self) -> bool {
        self.1
    }

    pub async fn channel_create(&self, config: RTCChannelConfig) -> Result<RTCChannel> {
        let dc = self.0.create_data_channel(
            config.label,
            Some(DataChannelConfig {
                ordered: true,
                negotiated: config.negotiated,
                ..Default::default()
            }),
        )?;
        // A negotiated channel made after the association is up opens at
        // once (vendor/rustrtc, "open negotiated data channels made after
        // SCTP is up").
        let chan = RTCChannel {
            pc: self.0.clone(),
            dc,
            handlers: Default::default(),
        };
        spawn_local(chan.clone().pump(self.0.subscribe_peer_state()));
        Ok(chan)
    }

    pub async fn offer_create(&self) -> Result<SessionDescription> {
        self.0.create_offer().await
    }

    pub async fn answer_create(&self) -> Result<SessionDescription> {
        self.0.create_answer().await
    }

    pub async fn local_desc_set(&self, desc: SessionDescription) -> Result<()> {
        self.0.set_local_description(desc)
    }

    pub async fn remote_desc_set(&self, desc: SessionDescription) -> Result<()> {
        self.0.set_remote_description(desc).await
    }

    pub async fn local_sdp(&self) -> Option<String> {
        self.0.local_description().map(|v| v.to_sdp_string())
    }

    pub fn connection_state(&self) -> RTCConnectionState {
        *self.0.subscribe_peer_state().borrow()
    }

    pub async fn wait_for_ice_gathering_complete(&self) {
        self.0.wait_for_gathering_complete().await
    }

    /// Calls `handler` with the current state, then on each change, until
    /// `Closed` or `Failed` (the driver handles both, with ready events).
    pub fn on_connection_state_change(&self, mut handler: OnConnectionStateChangeHdlrFn) {
        let mut states = self.0.subscribe_peer_state();
        spawn_local(async move {
            loop {
                let state = *states.borrow_and_update();
                handler(state).await;
                if matches!(
                    state,
                    PeerConnectionState::Closed | PeerConnectionState::Failed
                ) {
                    return;
                }
                if states.changed().await.is_err() {
                    handler(PeerConnectionState::Closed).await;
                    return;
                }
            }
        });
    }

    pub async fn close(self) {
        self.0.close();
    }
}

impl RTCChannel {
    /// Dispatches the channel's events until it closes, or its connection
    /// does (a closed connection may not close each channel).
    async fn pump(self, mut states: watch::Receiver<PeerConnectionState>) {
        loop {
            let event = tokio::select! {
                event = self.dc.recv() => event,
                res = states.wait_for(|s| {
                    matches!(s, PeerConnectionState::Closed | PeerConnectionState::Failed)
                }) => {
                    let _ = res;
                    None
                }
            };
            match event {
                Some(DataChannelEvent::Open) => self.opened(),
                Some(DataChannelEvent::Message(data)) => self.message(data),
                Some(DataChannelEvent::BufferedAmountLow(_)) => {}
                Some(DataChannelEvent::Close) | None => {
                    self.closed();
                    return;
                }
            }
        }
    }

    fn opened(&self) {
        let on_open = {
            let mut handlers = self.handlers.lock().unwrap();
            if handlers.opened {
                return;
            }
            handlers.opened = true;
            handlers.on_open.take()
        };
        if let Some(f) = on_open {
            spawn_local(f());
        }
    }

    fn message(&self, data: bytes::Bytes) {
        let fut = {
            let mut handlers = self.handlers.lock().unwrap();
            match handlers.on_message.as_mut() {
                Some(f) => f(&data),
                None => {
                    handlers.pending_bytes += data.len() + MESSAGE_OVERHEAD;
                    handlers.pending.push_back(data);
                    if handlers.pending_bytes > MAX_PENDING_BYTES {
                        handlers.pending.clear();
                        handlers.pending_bytes = 0;
                        drop(handlers);
                        self.pc.close();
                    }
                    return;
                }
            }
        };
        spawn_local(fut);
    }

    fn closed(&self) {
        let fut = {
            let mut handlers = self.handlers.lock().unwrap();
            if handlers.closed {
                return;
            }
            handlers.closed = true;
            handlers.on_close.as_mut().map(|f| f())
        };
        if let Some(fut) = fut {
            spawn_local(fut);
        }
    }

    pub fn on_open<Fut>(&self, f: impl FnOnce() -> Fut + Send + Sync + 'static)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut handlers = self.handlers.lock().unwrap();
        if handlers.opened {
            drop(handlers);
            spawn_local(f());
        } else {
            handlers.on_open = Some(Box::new(move || Box::pin(f())));
        }
    }

    /// Delivers, in order, the messages that arrived before this call.
    pub fn on_message<Fut>(&self, mut f: impl FnMut(&[u8]) -> Fut + Send + Sync + 'static)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut handlers = self.handlers.lock().unwrap();
        let pending = std::mem::take(&mut handlers.pending);
        handlers.pending_bytes = 0;
        let futs = pending.iter().map(|data| f(data)).collect::<Vec<_>>();
        handlers.on_message = Some(Box::new(move |data| Box::pin(f(data))));
        drop(handlers);
        for fut in futs {
            spawn_local(fut);
        }
    }

    /// rustrtc reports no channel errors apart from the close.
    pub fn on_error<Fut>(&self, _f: impl FnMut(rustrtc::RtcError) -> Fut + Send + Sync + 'static)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
    }

    pub fn on_close<Fut>(&self, mut f: impl FnMut() -> Fut + Send + Sync + 'static)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut handlers = self.handlers.lock().unwrap();
        if handlers.closed {
            drop(handlers);
            spawn_local(f());
        } else {
            handlers.on_close = Some(Box::new(move || Box::pin(f())));
        }
    }

    pub async fn send(&self, data: &bytes::Bytes) -> Result<usize> {
        self.pc.send_data(self.dc.id, data).await?;
        Ok(data.len())
    }

    /// rustrtc has no public call to close one channel (an SCTP stream
    /// reset); the channel closes with its connection. Unused with ready
    /// events, which keep channel 0 open; kept for the backend interface.
    #[allow(dead_code)]
    pub async fn close(&self) {}
}

pub async fn webrtc_signal_send(
    url: &str,
    offer: Offer,
) -> std::result::Result<P2pConnectionResponse, RTCSignalingError> {
    let client = reqwest::Client::new();
    let res = client.post(url).json(&offer).send().await?.json().await?;
    Ok(res)
}

impl Clone for RTCConnection {
    fn clone(&self) -> Self {
        Self(self.0.clone(), false)
    }
}

impl From<super::RTCConfigIceServer> for IceServer {
    fn from(value: super::RTCConfigIceServer) -> Self {
        IceServer {
            urls: value.urls,
            username: value.username,
            credential: value.credential,
            credential_type: IceCredentialType::Password,
        }
    }
}

impl TryFrom<Offer> for SessionDescription {
    type Error = rustrtc::RtcError;

    fn try_from(value: Offer) -> Result<Self> {
        SessionDescription::parse(SdpType::Offer, &value.sdp)
            .map_err(|e| rustrtc::RtcError::Protocol(e.to_string()))
    }
}

impl TryFrom<Answer> for SessionDescription {
    type Error = rustrtc::RtcError;

    fn try_from(value: Answer) -> Result<Self> {
        SessionDescription::parse(SdpType::Answer, &value.sdp)
            .map_err(|e| rustrtc::RtcError::Protocol(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn empty_early_messages_count_against_the_cap() {
        let pc = PeerConnection::new(RtcConfiguration::default());
        let dc = pc
            .create_data_channel(
                "",
                Some(DataChannelConfig {
                    negotiated: Some(1),
                    ..Default::default()
                }),
            )
            .unwrap();
        let chan = RTCChannel {
            pc: pc.clone(),
            dc,
            handlers: Default::default(),
        };
        let mut kept = 0;
        for _ in 0..100_000 {
            chan.message(bytes::Bytes::new());
            if *pc.subscribe_peer_state().borrow() == PeerConnectionState::Closed {
                break;
            }
            kept += 1;
        }
        assert_eq!(kept, MAX_PENDING_BYTES / MESSAGE_OVERHEAD);
        assert!(chan.handlers.lock().unwrap().pending.is_empty());
    }
}
