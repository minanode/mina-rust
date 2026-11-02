//! Opt-in local relay load client. Never part of a production build.
use super::*;
use crate::transports::{datachannel::DataChannelConfig, dtls::DtlsState};
use aes_gcm::{
    Aes128Gcm, Nonce,
    aead::{AeadInPlace, KeyInit},
};
use core::time::Duration;
use std::time::Instant;

// Build authenticated DTLS application records directly, bypassing the sender's
// SCTP flow control. The plaintext is an empty SCTP packet: valid CRC/header,
// no DATA chunks, and no Mina channel-zero control messages.
fn record(key: &[u8], iv: &[u8], seq: u64, plaintext: &[u8]) -> Vec<u8> {
    let full_seq = (1u64 << 48) | seq;
    let mut nonce = [0u8; 12];
    nonce[..4].copy_from_slice(iv);
    nonce[4..].copy_from_slice(&full_seq.to_be_bytes());
    let mut aad = full_seq.to_be_bytes().to_vec();
    aad.extend_from_slice(&[23, 254, 253]);
    aad.extend_from_slice(&(plaintext.len() as u16).to_be_bytes());
    let mut encrypted = plaintext.to_vec();
    let tag = Aes128Gcm::new_from_slice(key)
        .unwrap()
        .encrypt_in_place_detached(Nonce::from_slice(&nonce), &aad, &mut encrypted)
        .unwrap();
    let mut packet = vec![23, 254, 253];
    packet.extend_from_slice(&full_seq.to_be_bytes());
    packet.extend_from_slice(&((8 + plaintext.len() + 16) as u16).to_be_bytes());
    packet.extend_from_slice(&full_seq.to_be_bytes());
    packet.extend(encrypted);
    packet.extend_from_slice(&tag);
    packet
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires MINA_RAW_SOAK_OFFER JSON file and MINA_RAW_SOAK_URL for an owned local relay"]
async fn raw_packet_relay_soak() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let url = std::env::var("MINA_RAW_SOAK_URL").unwrap();
    assert!(
        url.starts_with("http://127.0.0.1:"),
        "only an owned loopback relay"
    );
    let mut offer: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var_os("MINA_RAW_SOAK_OFFER").unwrap()).unwrap(),
    )
    .unwrap();
    let pc = PeerConnection::new(RtcConfiguration {
        bind_ip: None,
        enable_gcc: false,
        ice_servers: Vec::new(),
        ..Default::default()
    });
    let _dc = pc
        .create_data_channel(
            "",
            Some(DataChannelConfig {
                negotiated: Some(0),
                ..Default::default()
            }),
        )
        .unwrap();
    pc.set_local_description(pc.create_offer().await.unwrap())
        .unwrap();
    pc.wait_for_gathering_complete().await;
    offer["sdp"] = pc.local_description().unwrap().to_sdp_string().into();
    let answer: serde_json::Value = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
        .post(url)
        .json(&offer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sdp = answer["Accepted"]["sdp"]
        .as_str()
        .expect("relay accepted the test offer");
    pc.set_remote_description(SessionDescription::parse(SdpType::Answer, sdp).unwrap())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), pc.wait_for_connected())
        .await
        .unwrap()
        .unwrap();
    let dtls = pc.inner.dtls_transport.lock().clone().unwrap();
    let DtlsState::Connected(crypto, _) = dtls.get_state() else {
        panic!("DTLS connected");
    };
    // An SDP offerer is the DTLS server when the answering relay uses active.
    let local_is_client = sdp.contains("a=setup:passive");
    let (key, iv) = if local_is_client {
        (&crypto.keys.client_write_key, &crypto.keys.client_write_iv)
    } else {
        (&crypto.keys.server_write_key, &crypto.keys.server_write_iv)
    };
    let ice = pc.ice_transport();
    let socket = ice.get_selected_socket().unwrap();
    let remote = ice.get_selected_pair().unwrap().remote.address;
    assert!(
        remote.ip().is_loopback()
            || local_ip_address::list_afinet_netifas()
                .unwrap()
                .iter()
                .any(|(_, ip)| *ip == remote.ip()),
        "relay candidate must belong to this host"
    );
    let mode = std::env::var("MINA_RAW_SOAK_MODE").unwrap_or_else(|_| "sctp".into());
    let mut raw_sctp = vec![0x13, 0x88, 0x13, 0x88, 0, 0, 0, 0, 0, 0, 0, 0];
    let mut crc = !0u32;
    for byte in &raw_sctp {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f63b78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    let crc = !crc;
    raw_sctp[8..12].copy_from_slice(&crc.to_le_bytes());
    let mut datagrams = Vec::new();
    // The initial burst uses distinct record sequence numbers.
    for batch in 0..128 {
        let mut datagram = Vec::new();
        if mode == "sctp" {
            for i in 0..30 {
                datagram.extend(record(key, iv, 1_000_000 + batch * 30 + i, &raw_sctp));
            }
        } else {
            // One authenticated, large record creates expensive decryption work
            // behind the raw ingress queue, without application messages.
            datagram = record(key, iv, 1_000_000 + batch, &vec![0; 1200]);
        }
        datagrams.push(datagram);
    }
    if let Some(start_file) = std::env::var_os("MINA_RAW_SOAK_START_FILE") {
        println!("raw_soak ready=true mode={mode}");
        tokio::time::timeout(Duration::from_secs(30), async {
            while !std::path::Path::new(&start_file).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("local load barrier released");
    }
    let started = Instant::now();
    let mut sent = 0usize;
    // Bound offered traffic and runtime; this client targets only its own ICE tuple.
    for i in 0..200_000 {
        if started.elapsed() > Duration::from_secs(5) {
            break;
        }
        if socket
            .try_send_to(&datagrams[i % datagrams.len()], remote)
            .is_ok()
        {
            sent += 1;
        }
        if i % 64 == 0 {
            tokio::task::yield_now().await;
        }
    }
    println!(
        "raw_soak mode={mode} sent_datagrams={sent} bytes_per_datagram={} elapsed_ms={}",
        datagrams[0].len(),
        started.elapsed().as_millis()
    );
    // Keep the client alive long enough for relay-side teardown to be sampled.
    tokio::time::sleep(Duration::from_secs(3)).await;
    println!(
        "raw_soak peer_state={:?}",
        *pc.subscribe_peer_state().borrow()
    );
    pc.close();
}
