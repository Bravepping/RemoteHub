use super::*;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::handshake::server::{Request, Response},
};

const LOCALHOST: Ipv4Addr = Ipv4Addr::LOCALHOST;

#[test]
fn public_endpoint_requires_exact_literal_addresses_and_bounded_approval() {
    let source = Ipv4Addr::new(192, 168, 80, 3);
    let relay = Ipv4Addr::new(93, 184, 216, 34);
    let endpoint = |source, relay, port, expires_at, now| {
        PublicVideoEndpoint::new(source, SocketAddrV4::new(relay, port), expires_at, now)
    };
    let approved = endpoint(source, relay, 48443, 110, 100).unwrap();
    assert_eq!(approved.source(), source);
    assert_eq!(approved.relay(), SocketAddrV4::new(relay, 48443));
    assert!(matches_bound_socket(
        approved.source(),
        approved.relay(),
        (source, 41000).into(),
        (relay, 48443).into()
    ));
    assert!(!matches_bound_socket(
        approved.source(),
        approved.relay(),
        (source, 41000).into(),
        (relay, 48444).into()
    ));
    assert!(!matches_bound_socket(
        approved.source(),
        approved.relay(),
        (Ipv4Addr::new(192, 168, 80, 4), 41000).into(),
        (relay, 48443).into()
    ));
    for prohibited in [
        LOCALHOST,
        Ipv4Addr::UNSPECIFIED,
        Ipv4Addr::new(169, 254, 2, 3),
        Ipv4Addr::new(100, 64, 2, 3),
        Ipv4Addr::new(192, 0, 2, 3),
        Ipv4Addr::new(198, 19, 2, 3),
        Ipv4Addr::new(224, 0, 0, 1),
        Ipv4Addr::BROADCAST,
    ] {
        assert!(matches!(
            endpoint(source, prohibited, 48443, 110, 100),
            Err(ClientError::PublicVideoEndpoint)
        ));
        assert!(matches!(
            endpoint(prohibited, relay, 48443, 110, 100),
            Err(ClientError::PublicVideoEndpoint)
        ));
    }
    assert!(matches!(
        endpoint(source, source, 48443, 110, 100),
        Err(ClientError::PublicVideoEndpoint)
    ));
    assert!(matches!(
        endpoint(source, relay, 443, 110, 100),
        Err(ClientError::PublicVideoEndpoint)
    ));
    assert!(matches!(
        endpoint(source, relay, 48443, 100, 100),
        Err(ClientError::PublicVideoExpiry)
    ));
    assert!(matches!(
        endpoint(source, relay, 48443, 701, 100),
        Err(ClientError::PublicVideoExpiry)
    ));
}

#[test]
fn public_lease_never_extends_after_wall_clock_rollback() {
    let started = Instant::now();
    let route = PublicVideoEndpoint::with_clock(
        Ipv4Addr::new(192, 168, 80, 3),
        SocketAddrV4::new(Ipv4Addr::new(93, 184, 216, 34), 48443),
        110,
        100,
        started,
    )
    .unwrap();
    assert_eq!(
        route.checked_deadline_at(99, started + Duration::from_secs(2)),
        Ok(started + Duration::from_secs(10))
    );
    assert_eq!(
        route.checked_deadline_at(99, started + Duration::from_secs(10)),
        Err(ClientError::PublicVideoExpiry)
    );
    assert_eq!(
        route.checked_deadline_at(110, started + Duration::from_secs(2)),
        Err(ClientError::PublicVideoExpiry)
    );
}

fn tls_fixture(san: &str) -> (LoopbackTlsTrust, TlsAcceptor) {
    let mut authority = CertificateParams::default();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    authority.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = CertifiedIssuer::self_signed(authority, KeyPair::generate().unwrap()).unwrap();
    let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).unwrap();
    let key = KeyPair::generate().unwrap();
    let mut leaf = CertificateParams::new(vec![san.to_owned()]).unwrap();
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf.signed_by(&key, &ca).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![leaf.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
    )
    .unwrap();
    (trust, TlsAcceptor::from(Arc::new(config)))
}

fn loopback_endpoint(listener: &TcpListener) -> PublicVideoEndpoint {
    let SocketAddr::V4(address) = listener.local_addr().unwrap() else {
        panic!("fixture must be IPv4");
    };
    assert_eq!(*address.ip(), LOCALHOST);
    let now = epoch_seconds().unwrap();
    // Only this colocated child test may bypass the public-IP gate. Route,
    // binding, TLS, WebSocket and wire behavior are the real production path.
    PublicVideoEndpoint::with_clock(LOCALHOST, address, now + 10, now, Instant::now()).unwrap()
}

#[tokio::test]
async fn verified_tls_precedes_bearer_and_uses_only_the_public_path_and_role() {
    let (trust, tls) = tls_fixture("127.0.0.1");
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let endpoint = loopback_endpoint(&listener);
    let token_text = "a".repeat(64);
    let token = RelayToken::new(token_text.clone()).unwrap();
    let room_text = "b".repeat(32);
    let room = RoomId::new(room_text.clone()).unwrap();

    let server = async {
        let (tcp, source) = listener.accept().await.unwrap();
        assert_eq!(source.ip(), LOCALHOST);
        let stream = tls.accept(tcp).await.unwrap();
        let mut socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
            assert_eq!(
                request.uri().path_and_query().unwrap().as_str(),
                PUBLIC_VIDEO_PATH
            );
            assert!(request.headers()[header::AUTHORIZATION] == format!("Bearer {token_text}"));
            Ok(response)
        })
        .await
        .unwrap();
        let hello = socket.next().await.unwrap().unwrap();
        match wire::decode::<ClientFrame>(hello.to_text().unwrap()).unwrap() {
            ClientFrame::Hello {
                version,
                room,
                role,
            } => {
                assert_eq!(version, VERSION);
                assert_eq!(room, room_text);
                assert_eq!(role, Role::Viewer);
            }
            _ => panic!("expected role-specific hello"),
        }
        let paired = wire::encode(&ServerFrame::Paired {}).unwrap();
        socket.send(Message::Text(paired.into())).await.unwrap();
        // A host-originated answer without the two challenges and offer
        // must still fail, even over the right TLS connection.
        let answer = wire::encode(&ServerFrame::Signal {
            from: Role::Host,
            seq: 1,
            signal: SignalKind::Answer,
            data: "unexpected answer".into(),
        })
        .unwrap();
        socket.send(Message::Text(answer.into())).await.unwrap();
    };
    let client = async {
        let mut relay = connect_public_video_wss(endpoint, &token, room, Role::Viewer, &trust)
            .await
            .unwrap();
        assert!(matches!(
            relay.next_event(IO_TIMEOUT).await,
            Ok(RelayEvent::Paired)
        ));
        assert!(matches!(
            relay.next_event(IO_TIMEOUT).await,
            Err(ClientError::Protocol)
        ));
        assert!(matches!(
            relay.next_event(IO_TIMEOUT).await,
            Err(ClientError::Closed)
        ));
    };
    tokio::time::timeout(IO_TIMEOUT, async {
        tokio::join!(server, client);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn wrong_certificate_ip_ends_tls_before_http_or_hello() {
    let (trust, tls) = tls_fixture("127.0.0.2");
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let endpoint = loopback_endpoint(&listener);
    let token = RelayToken::new("a".repeat(64)).unwrap();
    let server = async {
        let (tcp, source) = listener.accept().await.unwrap();
        assert_eq!(source.ip(), LOCALHOST);
        assert!(tls.accept(tcp).await.is_err());
    };
    let client = async {
        assert!(matches!(
            connect_public_video_wss(endpoint, &token, RoomId::random(), Role::Host, &trust).await,
            Err(ClientError::TlsHandshake)
        ));
    };
    tokio::time::timeout(IO_TIMEOUT, async {
        tokio::join!(server, client);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn public_wss_does_not_follow_even_a_local_redirect() {
    let (trust, tls) = tls_fixture("127.0.0.1");
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let other = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let redirect = format!("wss://{}{}", other.local_addr().unwrap(), PATH);
    let endpoint = loopback_endpoint(&listener);
    let token = RelayToken::new("a".repeat(64)).unwrap();
    let server = async {
        let (tcp, _) = listener.accept().await.unwrap();
        let stream = tls.accept(tcp).await.unwrap();
        let response = accept_hdr_async(stream, |_: &Request, _: Response| {
            Err(tokio_tungstenite::tungstenite::http::Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(header::LOCATION, redirect)
                .body(None)
                .unwrap())
        })
        .await;
        assert!(response.is_err());
    };
    let client = async {
        assert!(matches!(
            connect_public_video_wss(endpoint, &token, RoomId::random(), Role::Host, &trust).await,
            Err(ClientError::Transport)
        ));
    };
    tokio::time::timeout(IO_TIMEOUT, async {
        tokio::join!(server, client);
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), other.accept())
            .await
            .is_err()
    );
}
