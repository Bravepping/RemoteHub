// Real ICE/DTLS/SCTP over localhost, with synthetic text only.
use super::tests::{start_server, TOKEN};
use hub_webrtc::{
    authenticated_relay::{
        AuthenticatedLoopbackRelay, AuthenticatedRelayEvent, ClientError, RelayToken, Role, RoomId,
        SignalKind,
    },
    lan_wire::Key,
    pinned_runtime::PinnedRuntime,
};
use rtc::ice::{mdns::MulticastDnsMode, network_type::NetworkType};
use std::{net::Ipv4Addr, sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::timeout};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceCandidateInit,
        RTCPeerConnectionIceEvent, RTCSessionDescription, SettingEngineBuilder,
    },
};

struct Events {
    ice: mpsc::Sender<RTCIceCandidateInit>,
    channels: mpsc::Sender<Arc<dyn DataChannel>>,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Events {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let candidate = event.candidate.to_json().unwrap();
        if candidate.candidate.split_whitespace().nth(1) == Some("1") {
            self.ice.try_send(candidate).unwrap();
        }
    }

    async fn on_data_channel(&self, channel: Arc<dyn DataChannel>) {
        self.channels.try_send(channel).unwrap();
    }
}
struct Peer {
    pc: Box<dyn PeerConnection>,
    ice: mpsc::Receiver<RTCIceCandidateInit>,
    channels: mpsc::Receiver<Arc<dyn DataChannel>>,
}
impl Peer {
    async fn new() -> Self {
        let (ice_tx, ice) = mpsc::channel(8);
        let (channel_tx, channels) = mpsc::channel(1);
        let local = Ipv4Addr::LOCALHOST.into();
        let pc = PeerConnectionBuilder::new()
            .with_setting_engine(
                SettingEngineBuilder::new()
                    .with_include_loopback_candidate(true)
                    .with_multicast_dns_mode(MulticastDnsMode::Disabled)
                    .with_network_types(vec![NetworkType::Udp4])
                    .build(),
            )
            .with_udp_addrs(vec!["127.0.0.1:0"])
            .with_runtime(Arc::new(PinnedRuntime {
                inner: webrtc::runtime::default_runtime().unwrap(),
                local,
                peer: local,
                counters: Default::default(),
            }))
            .with_handler(Arc::new(Events {
                ice: ice_tx,
                channels: channel_tx,
            }))
            .build()
            .await
            .unwrap();
        Self {
            pc: Box::new(pc),
            ice,
            channels,
        }
    }
}

// The relay forwards these inner authenticated frames without the pairing key.
async fn exchange(
    from: &mut AuthenticatedLoopbackRelay,
    to: &mut AuthenticatedLoopbackRelay,
    kind: SignalKind,
    payload: String,
) -> String {
    from.send_signal(kind, payload).await.unwrap();
    let AuthenticatedRelayEvent::Signal { kind: signal, data } =
        to.next_event(Duration::from_secs(3)).await.unwrap()
    else {
        panic!("expected authenticated signaling");
    };
    assert_eq!(signal, kind);
    data
}

async fn opened(channel: &Arc<dyn DataChannel>) {
    while let Some(event) = channel.poll().await {
        match event {
            DataChannelEvent::OnOpen => return,
            DataChannelEvent::OnClose | DataChannelEvent::OnError => break,
            _ => (),
        }
    }
    panic!("channel closed before opening");
}

async fn exercise(
    viewer: &mut Peer,
    host: &mut Peer,
    viewer_relay: &mut AuthenticatedLoopbackRelay,
    host_relay: &mut AuthenticatedLoopbackRelay,
) {
    let channel = viewer
        .pc
        .create_data_channel("relay-smoke", None)
        .await
        .unwrap();

    let offer = viewer.pc.create_offer(None).await.unwrap();
    viewer
        .pc
        .set_local_description(offer.clone())
        .await
        .unwrap();
    let offer = exchange(viewer_relay, host_relay, SignalKind::Offer, offer.sdp).await;
    host.pc
        .set_remote_description(RTCSessionDescription::offer(offer).unwrap())
        .await
        .unwrap();

    let answer = host.pc.create_answer(None).await.unwrap();
    host.pc.set_local_description(answer.clone()).await.unwrap();
    let answer = exchange(host_relay, viewer_relay, SignalKind::Answer, answer.sdp).await;
    viewer
        .pc
        .set_remote_description(RTCSessionDescription::answer(answer).unwrap())
        .await
        .unwrap();

    let viewer_ice = serde_json::to_string(&viewer.ice.recv().await.unwrap()).unwrap();
    let viewer_ice = exchange(viewer_relay, host_relay, SignalKind::Ice, viewer_ice).await;
    host.pc
        .add_ice_candidate(serde_json::from_str(&viewer_ice).unwrap())
        .await
        .unwrap();
    let host_ice = serde_json::to_string(&host.ice.recv().await.unwrap()).unwrap();
    let host_ice = exchange(host_relay, viewer_relay, SignalKind::Ice, host_ice).await;
    viewer
        .pc
        .add_ice_candidate(serde_json::from_str(&host_ice).unwrap())
        .await
        .unwrap();

    opened(&channel).await;
    let incoming = host.channels.recv().await.unwrap();
    opened(&incoming).await;
    assert_eq!(incoming.label().await.unwrap(), "relay-smoke");
    channel.send_text("synthetic-test").await.unwrap();
    loop {
        match incoming.poll().await {
            Some(DataChannelEvent::OnMessage(message)) => {
                assert!(message.is_string);
                assert_eq!(message.data.as_ref(), b"synthetic-test");
                break;
            }
            Some(
                DataChannelEvent::OnBufferedAmountHigh | DataChannelEvent::OnBufferedAmountLow,
            ) => {}
            _ => panic!("channel closed before the test message"),
        }
    }
}

#[tokio::test]
async fn native_webrtc_connects_with_authenticated_sdp_and_trickle_ice_over_v2_signal() {
    let (addr, _, server) = start_server().await;
    let token = RelayToken::new(TOKEN.to_owned()).unwrap();
    let room = RoomId::random();
    let key = Arc::new(Key::generate());
    let nonce = uuid::Uuid::new_v4().to_string();
    let mut host_relay = AuthenticatedLoopbackRelay::connect(
        addr,
        &token,
        RoomId::new(room.as_str().to_owned()).unwrap(),
        Role::Host,
        key.clone(),
        nonce.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(
        host_relay.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::Waiting
    ));
    let mut viewer_relay = AuthenticatedLoopbackRelay::connect(
        addr,
        &token,
        RoomId::new(room.as_str().to_owned()).unwrap(),
        Role::Viewer,
        key,
        nonce,
    )
    .await
    .unwrap();
    for relay in [&mut host_relay, &mut viewer_relay] {
        assert!(matches!(
            relay.next_event(Duration::from_secs(3)).await.unwrap(),
            AuthenticatedRelayEvent::Paired
        ));
    }
    let mut viewer = Peer::new().await;
    let mut host = Peer::new().await;
    let result = timeout(
        Duration::from_secs(12),
        exercise(&mut viewer, &mut host, &mut viewer_relay, &mut host_relay),
    )
    .await;
    viewer.pc.close().await.unwrap();
    host.pc.close().await.unwrap();
    result.expect("loopback negotiation timed out");
    viewer_relay.leave().await.unwrap();
    assert!(matches!(
        host_relay.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::PeerLeft
    ));
    assert_eq!(
        host_relay
            .send_signal(SignalKind::Ice, "ignored".into())
            .await,
        Err(ClientError::Closed)
    );
    host_relay.leave().await.unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

async fn mixed_pair(
    addr: std::net::SocketAddr,
    token: &RelayToken,
    room: &RoomId,
    key: Arc<Key>,
    nonce: &str,
) -> (AuthenticatedLoopbackRelay, hub_signal_v2::LoopbackRelay) {
    let mut host = AuthenticatedLoopbackRelay::connect(
        addr,
        token,
        RoomId::new(room.as_str().into()).unwrap(),
        Role::Host,
        key,
        nonce.into(),
    )
    .await
    .unwrap();
    assert!(matches!(
        host.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::Waiting
    ));
    let mut viewer = hub_signal_v2::LoopbackRelay::connect(
        addr,
        token,
        RoomId::new(room.as_str().into()).unwrap(),
        Role::Viewer,
    )
    .await
    .unwrap();
    assert!(matches!(
        host.next_event(Duration::from_secs(3)).await.unwrap(),
        AuthenticatedRelayEvent::Paired
    ));
    assert!(matches!(
        viewer.next_event(Duration::from_secs(3)).await.unwrap(),
        hub_signal_v2::RelayEvent::Paired
    ));
    (host, viewer)
}

#[tokio::test]
async fn authenticated_wrapper_rejects_wrong_mac_and_mismatched_signal_kind_then_closes() {
    use hub_webrtc::lan_wire::Codec;

    let (addr, _, server) = start_server().await;
    let token = RelayToken::new(TOKEN.to_owned()).unwrap();
    for wrong_mac in [true, false] {
        let room = RoomId::random();
        let nonce = uuid::Uuid::new_v4().to_string();
        let key = Arc::new(Key::generate());
        let (mut host, mut viewer) = mixed_pair(addr, &token, &room, key.clone(), &nonce).await;
        let mut test_peer = Codec::new_v2(
            if wrong_mac {
                Arc::new(Key::generate())
            } else {
                key
            },
            nonce,
            room.as_str(),
            false,
        )
        .unwrap();
        let fake_offer = test_peer
            .encode(
                if wrong_mac { "offer" } else { "answer" },
                "synthetic text".into(),
            )
            .unwrap();
        viewer
            .send_signal(
                SignalKind::Offer,
                serde_json::to_string(&fake_offer).unwrap(),
            )
            .await
            .unwrap();
        assert!(matches!(
            host.next_event(Duration::from_secs(3)).await,
            Err(ClientError::Protocol)
        ));
        assert!(matches!(
            host.next_event(Duration::ZERO).await,
            Err(ClientError::Closed)
        ));
        assert!(matches!(
            viewer.next_event(Duration::from_secs(3)).await.unwrap(),
            hub_signal_v2::RelayEvent::PeerLeft
        ));
        host.leave().await.unwrap();
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}

#[tokio::test]
async fn authenticated_wrapper_timeout_is_retryable_but_cancelled_poll_releases_peer() {
    let (addr, _, server) = start_server().await;
    let token = RelayToken::new(TOKEN.to_owned()).unwrap();
    let room = RoomId::random();
    let nonce = uuid::Uuid::new_v4().to_string();
    let (mut host, mut viewer) =
        mixed_pair(addr, &token, &room, Arc::new(Key::generate()), &nonce).await;
    assert!(matches!(
        host.next_event(Duration::from_millis(10)).await,
        Err(ClientError::Timeout)
    ));
    assert!(timeout(
        Duration::from_millis(10),
        host.next_event(Duration::from_secs(3))
    )
    .await
    .is_err());
    assert!(matches!(
        host.next_event(Duration::ZERO).await,
        Err(ClientError::Closed)
    ));
    assert!(matches!(
        viewer.next_event(Duration::from_secs(3)).await.unwrap(),
        hub_signal_v2::RelayEvent::PeerLeft
    ));
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
