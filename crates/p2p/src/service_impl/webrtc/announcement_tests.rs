//! Maximum protocol-valid announcement batches through the real peer loop.
use super::*;
use crate::channels::transaction::{TransactionInfo, TransactionPropagationChannelMsg};

async fn connected_pair() -> (RTCConnection, RTCChannel, RTCConnection, RTCChannel) {
    let api = build_api(&webrtc::WebrtcConfig::default());
    let a = RTCConnection::create(
        &api,
        RTCConfig {
            ice_servers: Vec::new().into(),
            certificate: (),
            seed: [0; 32],
        },
    )
    .await
    .unwrap();
    let b = RTCConnection::create(
        &api,
        RTCConfig {
            ice_servers: Vec::new().into(),
            certificate: (),
            seed: [0; 32],
        },
    )
    .await
    .unwrap();
    let a0 = a
        .channel_create(RTCChannelConfig {
            label: "",
            negotiated: Some(0),
        })
        .await
        .unwrap();
    let b0 = b
        .channel_create(RTCChannelConfig {
            label: "",
            negotiated: Some(0),
        })
        .await
        .unwrap();
    let (atx, arx) = oneshot::channel();
    let (btx, brx) = oneshot::channel();
    let mut atx = Some(atx);
    let mut btx = Some(btx);
    a0.on_open(move || {
        if let Some(tx) = atx.take() {
            let _ = tx.send(());
        }
        std::future::ready(())
    });
    b0.on_open(move || {
        if let Some(tx) = btx.take() {
            let _ = tx.send(());
        }
        std::future::ready(())
    });
    let offer = a.offer_create().await.unwrap();
    a.local_desc_set(offer).await.unwrap();
    a.wait_for_ice_gathering_complete().await;
    let offer =
        rustrtc::SessionDescription::parse(rustrtc::SdpType::Offer, &a.local_sdp().await.unwrap())
            .unwrap();
    b.remote_desc_set(offer).await.unwrap();
    let answer = b.answer_create().await.unwrap();
    b.local_desc_set(answer).await.unwrap();
    b.wait_for_ice_gathering_complete().await;
    let answer =
        rustrtc::SessionDescription::parse(rustrtc::SdpType::Answer, &b.local_sdp().await.unwrap())
            .unwrap();
    a.remote_desc_set(answer).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        arx.await.unwrap();
        brx.await.unwrap();
    })
    .await
    .unwrap();
    (a, a0, b, b0)
}

fn assert_protocol_accepts(count: u8, info: &TransactionInfo) {
    use crate::channels::transaction::{
        P2pChannelsTransactionAction as A, P2pChannelsTransactionState as S,
        TransactionPropagationState as T,
    };
    use redux::EnablingCondition;
    let time = redux::Timestamp::ZERO;
    let peer_id = SecretKey::from_bytes([2; 32]).public_key().peer_id();
    let enabled = ChannelId::without_signaling().collect();
    let config = crate::P2pConfig {
        libp2p_port: None,
        listen_port: None,
        identity_pub_key: SecretKey::from_bytes([3; 32]).public_key(),
        initial_peers: vec![],
        external_addrs: vec![],
        enabled_channels: enabled,
        timeouts: Default::default(),
        limits: Default::default(),
        peer_discovery: false,
        seed_only: false,
        meshsub: Default::default(),
        catchup_rpc: Default::default(),
    };
    let mut state = crate::P2pState::new(
        config,
        crate::P2pCallbacks::default(),
        &mina_core::MAINNET_CHAIN_ID,
    );
    let mut ready = crate::P2pPeerStatusReady::new(true, time, &state.config.enabled_channels);
    ready.channels.transaction = S::Ready {
        time,
        local: T::WaitingForRequest { time },
        remote: T::WaitingForRequest { time },
        next_send_index: 0,
    };
    state.peers.insert(
        peer_id,
        crate::P2pPeerState {
            is_libp2p: false,
            dial_opts: None,
            identify: None,
            status: crate::P2pPeerStatus::Ready(Box::new(ready)),
        },
    );
    assert!(A::RequestReceived {
        peer_id,
        limit: count
    }
    .is_enabled(&state, time));
    if let S::Ready { remote, .. } = &mut state
        .get_ready_peer_mut(&peer_id)
        .unwrap()
        .channels
        .transaction
    {
        *remote = T::Requested {
            time,
            requested_limit: count,
        };
    }
    assert!(A::ResponseSend {
        peer_id,
        transactions: vec![info.clone(); count as usize],
        first_index: 0,
        last_index: u64::from(count) - 1
    }
    .is_enabled(&state, time));
}

async fn batch(count: u8, announced: bool, blocked_cancel: Option<bool>) {
    let (a, a0, b, b0) = connected_pair().await;
    let failure = Arc::new(Failure::default());
    let aborter = Aborter::default();
    let listener = ready::Ch0Listener::listen(&a0, failure.clone());
    let (_auth, ready) = ready::start(
        a0,
        listener,
        ConnectionAuthEncrypted::try_from(&[1; 92][..]).unwrap(),
        aborter.aborted(),
        failure.clone(),
    );
    // Exercise both legacy fallback and an already-announced modern peer.
    let (cmd, rx) = queue::channel(Kind::Command, PEER_COMMANDS, PEER_BYTES, failure.clone());
    let (events, mut event_rx) = mpsc::unbounded_channel::<P2pEvent>();
    let event_sender = bounded_events(events, failure.clone());
    let peer_id = SecretKey::from_bytes([2; 32]).public_key().peer_id();
    let ready_observer = ready.clone();
    let task_failure = failure.clone();
    let aborted = aborter.aborted();
    let handle = spawn_local(async move {
        tokio::select! {
            _ = task_failure.wait() => {},
            _ = peer_loop(peer_id, event_sender, rx, a, aborted, task_failure.clone(), ready) => {},
        }
    });
    cmd.tracked_send(PeerCmd::ChannelOpen(ChannelId::TransactionPropagation))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = event_rx.recv().await.unwrap();
            if matches!(
                event.unwrapped(),
                P2pEvent::Channel(P2pChannelEvent::Opened(
                    _,
                    ChannelId::TransactionPropagation,
                    Ok(())
                ))
            ) {
                break;
            }
        }
    })
    .await
    .unwrap();
    let remote_channel = b
        .channel_create(RTCChannelConfig {
            label: ChannelId::TransactionPropagation.name(),
            negotiated: Some(ChannelId::TransactionPropagation.to_u16()),
        })
        .await
        .unwrap();
    let (received, mut received_rx) = mpsc::channel(1024);
    let mut reassembler = MsgReassembler::new(ChannelId::TransactionPropagation);
    remote_channel.on_message(move |bytes| {
        reassembler.feed(bytes, |msg| {
            received.try_send(msg.unwrap()).unwrap();
            true
        });
        std::future::ready(())
    });
    if announced {
        b0.send(&bytes::Bytes::copy_from_slice(&ready::ready_msg(
            ChannelId::TransactionPropagation,
        )))
        .await
        .unwrap();
        assert!(tokio::time::timeout(
            Duration::from_secs(2),
            ready_observer.wait(ChannelId::TransactionPropagation)
        )
        .await
        .unwrap());
        sleep(Duration::from_millis(100)).await;
    }
    cmd.tracked_send(PeerCmd::ChannelSend(
        MsgId::first(),
        TransactionPropagationChannelMsg::WillSend { count }.into(),
    ))
    .unwrap();
    for nonce in 0..count {
        let info = TransactionInfo {
            hash: (&[nonce; 32]).into(),
            // Public key from the checked-in hashing fixture, no secret material.
            fee_payer: "B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t"
                .parse()
                .unwrap(),
            fee: 1,
            nonce: u32::from(nonce),
        };
        if nonce == 0 {
            assert_protocol_accepts(count, &info);
        }
        cmd.tracked_send(PeerCmd::ChannelSend(
            MsgId::first(),
            TransactionPropagationChannelMsg::Transaction(info).into(),
        ))
        .unwrap();
    }
    if blocked_cancel.is_some() {
        // Transport stress beyond a single protocol batch: fill the sender
        // while readiness is blocked, then cancel rather than delivering it.
        for _ in 0..CHANNEL_SENDS {
            cmd.tracked_send(PeerCmd::ChannelSend(
                MsgId::first(),
                TransactionPropagationChannelMsg::GetNext { limit: 1 }.into(),
            ))
            .unwrap();
        }
    }
    // Yield to the real peer_loop while its sender awaits the legacy fallback.
    sleep(Duration::from_millis(100)).await;
    assert!(
        !failure.is_failed(),
        "valid batch must not overload the peer"
    );
    if let Some(use_peer_abort) = blocked_cancel {
        assert_eq!(cmd.len(), usize::from(count) + 1 + CHANNEL_SENDS);
        assert!(
            !handle.is_finished(),
            "the full channel must apply backpressure"
        );
        if use_peer_abort {
            drop(aborter);
        } else {
            failure.cancel();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            handle.await.unwrap();
            while cmd.len() != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancellation drains queued and pending sends");
        assert!(!failure.is_failed(), "backpressure is not an overload");
        return;
    }
    tokio::time::timeout(Duration::from_secs(10), async {
        let first = received_rx.recv().await.unwrap();
        assert!(matches!(first, ChannelMsg::TransactionPropagation(
            TransactionPropagationChannelMsg::WillSend { count: n }
        ) if n == count));
        for expected_nonce in 0..count {
            let msg = received_rx.recv().await.unwrap();
            assert!(matches!(msg, ChannelMsg::TransactionPropagation(
                TransactionPropagationChannelMsg::Transaction(info)
            ) if info.nonce == u32::from(expected_nonce)));
        }
        // Drain completion events too: every admitted message must finish.
        let mut sent = 0;
        while sent < usize::from(count) + 1 {
            let event = event_rx.recv().await.unwrap();
            match event.unwrapped() {
                P2pEvent::Channel(P2pChannelEvent::Sent(_, _, _, Ok(()))) => sent += 1,
                other => panic!("unexpected event: {other:?}"),
            }
        }
    })
    .await
    .expect("the entire batch is delivered in order");
    assert!(!failure.is_failed());
    assert_eq!(cmd.len(), 0);
    failure.cancel();
    handle.await.unwrap();
}

#[tokio::test]
async fn legacy_readiness_delivers_valid_announcement_batches() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for count in [32, 64, u8::MAX] {
                batch(count, false, None).await;
            }
        })
        .await;
}

#[tokio::test]
async fn announced_readiness_delivers_valid_announcement_batches() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for count in [32, 65, u8::MAX] {
                batch(count, true, None).await;
            }
        })
        .await;
}

#[tokio::test]
async fn blocked_send_backpressure_is_cancellable_and_releases_reservations() {
    tokio::task::LocalSet::new()
        .run_until(async {
            for use_peer_abort in [false, true] {
                batch(u8::MAX, false, Some(use_peer_abort)).await;
            }
        })
        .await;
}
