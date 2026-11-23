//! Actual WSS and scoped ICE workers on explicit IPv4 loopback only.

use super::*;
use crate::{
    direct_policy::SelectedRoute, ice_policy::IceMode, public_video::PublicVideoScopedIce,
};
use futures_util::{SinkExt, StreamExt};
use hub_identity::pairing::PairingCandidate;
use hub_signal_v2::wire::{self, ClientFrame, Flow, ServerFrame, PATH, VERSION};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use sha2::{Digest, Sha256};
use std::net::{Ipv4Addr, SocketAddrV4};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{server::TlsStream, TlsAcceptor};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        http::header::AUTHORIZATION,
        Message,
    },
    WebSocketStream,
};
use webrtc::peer_connection::RTCConfigurationBuilder;

#[path = "../../hub-rendezvous-probe/src/bin/v2_signaling/public_video_fixture_net.rs"]
mod fixture_net;
use fixture_net::{FixtureNat, FixtureSide};

const WAIT: Duration = Duration::from_secs(6);
const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

type Socket = WebSocketStream<TlsStream<TcpStream>>;

struct RelayFixture {
    address: SocketAddr,
    trust: LoopbackTlsTrust,
    ca_hash: [u8; 32],
    task: JoinHandle<()>,
}

impl RelayFixture {
    async fn start() -> Self {
        let mut ca = CertificateParams::default();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
        let ca = CertifiedIssuer::self_signed(ca, KeyPair::generate().unwrap()).unwrap();
        let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).unwrap();
        let ca_hash = Sha256::digest(ca.der().as_ref()).into();
        let mut params = CertificateParams::new(vec![LOOPBACK.to_string()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let certificate = params.signed_by(&key, &ca).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .unwrap();
        let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(serve(listener, TlsAcceptor::from(Arc::new(config))));
        Self {
            address,
            trust,
            ca_hash,
            task,
        }
    }
}

impl Drop for RelayFixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn accept_peer(listener: &TcpListener, tls: &TlsAcceptor, role: Role) -> (Socket, String) {
    let (tcp, address) = listener.accept().await.unwrap();
    assert_eq!(address.ip(), std::net::IpAddr::V4(LOOPBACK));
    let stream = tls.accept(tcp).await.unwrap();
    let expected = format!("Bearer {TOKEN}");
    let mut ws = accept_hdr_async(stream, move |request: &Request, response: Response| {
        assert_eq!(request.uri().path_and_query().unwrap().as_str(), PATH);
        assert_eq!(request.headers()[AUTHORIZATION], expected.as_str());
        Ok(response)
    })
    .await
    .unwrap();
    match receive(&mut ws).await {
        Some(ClientFrame::Hello {
            version,
            role: actual,
            room,
        }) => {
            assert_eq!(version, VERSION);
            assert_eq!(actual, role);
            (ws, room)
        }
        _ => panic!("expected one fixture hello"),
    }
}

async fn receive(from: &mut Socket) -> Option<ClientFrame> {
    match from.next().await {
        Some(Ok(Message::Text(text))) => Some(wire::decode(&text).unwrap()),
        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => None,
        _ => panic!("unexpected fixture WebSocket frame"),
    }
}

async fn send(to: &mut Socket, frame: ServerFrame) {
    to.send(Message::Text(wire::encode(&frame).unwrap().into()))
        .await
        .unwrap();
}

async fn serve(listener: TcpListener, tls: TlsAcceptor) {
    let (mut host, room) = accept_peer(&listener, &tls, Role::Host).await;
    send(&mut host, ServerFrame::Waiting {}).await;
    let (mut viewer, viewer_room) = accept_peer(&listener, &tls, Role::Viewer).await;
    assert_eq!(room, viewer_room);
    drop(listener);
    send(&mut host, ServerFrame::Paired {}).await;
    send(&mut viewer, ServerFrame::Paired {}).await;
    let mut flow = Flow::default();
    let mut next_seq = [1, 1];
    loop {
        let (role, frame) = tokio::select! {
            value = receive(&mut host) => (Role::Host, value),
            value = receive(&mut viewer) => (Role::Viewer, value),
        };
        let (target, seq) = match role {
            Role::Host => (&mut viewer, &mut next_seq[0]),
            Role::Viewer => (&mut host, &mut next_seq[1]),
        };
        match frame {
            Some(ClientFrame::Signal {
                seq: given,
                signal,
                data,
            }) => {
                assert_eq!(given, *seq);
                assert!(signal.allows_sender(role));
                *seq += 1;
                flow = flow.after(signal).expect("ordered fixture signal");
                send(
                    target,
                    ServerFrame::Signal {
                        from: role,
                        seq: given,
                        signal,
                        data,
                    },
                )
                .await;
            }
            Some(ClientFrame::Leave {}) | None => {
                send(target, ServerFrame::PeerLeft {}).await;
                return;
            }
            _ => panic!("unexpected fixture hello"),
        }
    }
}

fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
    let expected = peer.public();
    local
        .verify_peer(
            &PairingCandidate::parse(&expected.pairing_code()).unwrap(),
            &expected.fingerprint,
        )
        .unwrap()
}

fn owner<'a>(
    proof: PublicVideoProof<'a>,
    now: u64,
    nat: &FixtureNat,
    room: Uuid,
    side: FixtureSide,
) -> Result<PublicVideoScopedIce<'a>, &'static str> {
    let stun = nat.stun_server();
    let endpoint: SocketAddrV4 = stun.urls[0].strip_prefix("stun:").unwrap().parse().unwrap();
    assert_eq!(*endpoint.ip(), LOOPBACK);
    PublicVideoScopedIce::new_loopback_fixture(
        proof,
        room,
        SocketAddrV4::new(LOOPBACK, 0),
        endpoint,
        RTCConfigurationBuilder::default()
            .with_ice_servers(vec![stun])
            .build(),
        nat.runtime(side),
        now,
    )
}

async fn exercise(abrupt: bool) {
    let mut tls = RelayFixture::start().await;
    let nat = FixtureNat::start().await.unwrap();
    let host = Identity::generate_ephemeral().unwrap();
    let viewer = Identity::generate_ephemeral().unwrap();
    let host_pin = pin(&host, &viewer);
    let viewer_pin = pin(&viewer, &host);
    let token = RelayToken::new(TOKEN.into()).unwrap();
    let key = Arc::new(Key::generate());
    let room = Uuid::new_v4();
    let wire_nonce = Uuid::new_v4().to_string();
    let label = Ipv4Addr::new(93, 184, 216, 34); // proof context only, never dialed
    let scope = PublicVideoScope {
        wss: SocketAddrV4::new(label, 8443),
        ca_sha256: tls.ca_hash,
        turn: SocketAddrV4::new(label, 3478),
        relay_ports: (49152, 49200),
        mode: IceMode::DirectPreferred,
        expires_at: wall_seconds().unwrap() + 120,
    };
    let cfg = |role, local_identity, peer| PublicVideoWssConfig {
        address: tls.address,
        token: &token,
        room,
        role,
        pairing_key: key.clone(),
        wire_nonce: wire_nonce.clone(),
        session_nonce: [9; 32],
        trust: &tls.trust,
        local_identity,
        peer,
        scope,
    };
    let h = PublicVideoWss::connect_loopback_wss(cfg(Role::Host, &host, &host_pin))
        .await
        .unwrap()
        .wait_for_host_registration()
        .await
        .unwrap();
    let v = PublicVideoWss::connect_loopback_wss(cfg(Role::Viewer, &viewer, &viewer_pin))
        .await
        .unwrap();
    let (h, v) = tokio::try_join!(
        h.establish_scoped_fixture(|proof, now| owner(proof, now, &nat, room, FixtureSide::Host)),
        v.establish_scoped_fixture(|proof, now| owner(proof, now, &nat, room, FixtureSide::Viewer)),
    )
    .unwrap();
    let (host_stop, host_rx) = watch::channel(false);
    let (_viewer_stop, viewer_rx) = watch::channel(false);
    let (host_state_tx, mut host_state) = watch::channel(ScopedWssState::Negotiating);
    let (viewer_state_tx, mut viewer_state) = watch::channel(ScopedWssState::Negotiating);
    let control = async {
        host_state
            .wait_for(|s| {
                matches!(
                    s,
                    ScopedWssState::HostPathVerified(SelectedRoute::ServerReflexiveDirect)
                )
            })
            .await
            .unwrap();
        viewer_state
            .wait_for(|s| matches!(s, ScopedWssState::Established))
            .await
            .unwrap();
        if abrupt {
            tls.task.abort();
        } else {
            host_stop.send_replace(true);
        }
    };
    let (host_result, viewer_result, ()) = timeout(WAIT, async {
        tokio::join!(
            h.run(host_rx, host_state_tx),
            v.run(viewer_rx, viewer_state_tx),
            control
        )
    })
    .await
    .expect("scoped native session and both teardown paths");
    if abrupt {
        assert!(host_result.is_err());
        assert!(viewer_result.is_err());
        assert!((&mut tls.task).await.unwrap_err().is_cancelled());
    } else {
        assert!(matches!(host_result, Ok(ScopedWssEnd::Stopped)));
        assert!(matches!(viewer_result, Ok(ScopedWssEnd::PeerLeft)));
        timeout(WAIT, &mut tls.task).await.unwrap().unwrap();
    }
    assert!(matches!(&*host_state.borrow(), ScopedWssState::Closed));
    assert!(matches!(&*viewer_state.borrow(), ScopedWssState::Closed));
}

#[tokio::test]
async fn scoped_loopback_wss_applies_signed_video_and_closes_both_roles() {
    exercise(false).await;
}

#[tokio::test]
async fn scoped_loopback_wss_closes_both_roles_when_transport_drops() {
    exercise(true).await;
}
