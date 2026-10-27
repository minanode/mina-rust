//! A channel the peer opens in band (DCEP OPEN), then 8 MiB on it: does the
//! webrtc-rs fork, as the fallback backend uses it (negotiated channel 0, no
//! `on_data_channel` handler), keep any of it? The peer is rustrtc. A
//! counting allocator measures what stays live in the process, and B's
//! buffered amount what A acknowledged.
//!
//! The rustrtc side of the same question is tested in
//! `vendor/rustrtc/src/peer_connection.rs` (`remote_channel_tests`).
#![cfg(all(not(target_arch = "wasm32"), feature = "p2p-webrtc-rs"))]

use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::{
        atomic::{AtomicIsize, Ordering},
        Arc,
    },
    time::Duration,
};

use rustrtc::{
    transports::datachannel::{DataChannelConfig, DataChannelEvent},
    PeerConnection, RtcConfiguration, SdpType, SessionDescription,
};
use webrtc::{
    api::APIBuilder,
    data_channel::data_channel_init::RTCDataChannelInit,
    peer_connection::{
        configuration::RTCConfiguration, sdp::session_description::RTCSessionDescription,
    },
};

struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(
            new_size as isize - layout.size() as isize,
            Ordering::Relaxed,
        );
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

const MSG: usize = 16 * 1024;
const COUNT: usize = 512;

#[tokio::test(flavor = "multi_thread")]
async fn the_webrtc_rs_fallback_keeps_nothing_from_an_in_band_channel() {
    // A: webrtc-rs, negotiated channel 0 only, as the fallback backend.
    let api = APIBuilder::new().build();
    let a = Arc::new(
        api.new_peer_connection(RTCConfiguration::default())
            .await
            .unwrap(),
    );
    let a0 = a
        .create_data_channel(
            "",
            Some(RTCDataChannelInit {
                ordered: Some(true),
                negotiated: Some(0),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let (a0_open_tx, mut a0_open) = tokio::sync::mpsc::channel::<()>(1);
    a0.on_open(Box::new(move || {
        let _ = a0_open_tx.try_send(());
        Box::pin(async {})
    }));

    // B: rustrtc, the peer that opens a channel in band.
    let b = PeerConnection::new(RtcConfiguration {
        enable_gcc: false,
        ..Default::default()
    });
    let negotiated = |id| {
        Some(DataChannelConfig {
            ordered: true,
            negotiated: Some(id),
            ..Default::default()
        })
    };
    let b0 = b.create_data_channel("", negotiated(0)).unwrap();

    let offer = a.create_offer(None).await.unwrap();
    let mut gathered = a.gathering_complete_promise().await;
    a.set_local_description(offer).await.unwrap();
    let _ = gathered.recv().await;
    let offer = a.local_description().await.unwrap().sdp;
    b.set_remote_description(SessionDescription::parse(SdpType::Offer, &offer).unwrap())
        .await
        .unwrap();
    let answer = b.create_answer().await.unwrap();
    b.set_local_description(answer).unwrap();
    b.wait_for_gathering_complete().await;
    let answer = b.local_description().unwrap().to_sdp_string();
    a.set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(20), a0_open.recv())
        .await
        .expect("A's channel 0 opens");
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(20), b0.recv()).await,
        Ok(Some(DataChannelEvent::Open))
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = LIVE.load(Ordering::Relaxed);

    // B opens a channel in band and sends 8 MiB, for at most 30 s.
    let extra = b.create_data_channel("extra", None).unwrap();
    let payload = vec![7u8; MSG];
    let mut sent = 0usize;
    let _ = tokio::time::timeout(Duration::from_secs(30), async {
        for _ in 0..COUNT {
            if b.send_data(extra.id, &payload).await.is_err() {
                break;
            }
            sent += MSG;
        }
    })
    .await;
    drop(payload);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let kept = LIVE.load(Ordering::Relaxed) - before;
    let unacked = extra.buffered_amount();
    eprintln!(
        "webrtc-rs fallback: {sent} bytes offered, {unacked} of them unacknowledged, {kept} bytes more live afterwards (B's own send queue included)"
    );
    // A acknowledged none of it: SCTP acknowledges what it takes in, so A
    // took none, and B still holds every byte it offered (and sometimes the
    // 17-byte DCEP OPEN as well).
    assert!(unacked >= sent, "A acknowledged some of the in-band data");
    // What is live beyond B's own copy is bookkeeping, not data A kept.
    let beyond = kept - unacked as isize;
    assert!(
        beyond < 128 * 1024,
        "{beyond} bytes live beyond B's send queue: the in-band channel's data was kept"
    );
    b.close();
    a.close().await.unwrap();
}
