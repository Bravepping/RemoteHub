//! Synthetic TLS credentials are generated in memory; this only binds 127.0.0.1.
use super::*;
use hub_signal_v2::{ClientError, LoopbackRelay, LoopbackTlsTrust, RelayEvent, RelayToken, RoomId};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};

const WAIT: Duration = Duration::from_secs(3);

pub(super) fn make_ca() -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

pub(super) fn fixture() -> (Arc<App>, RelayToken) {
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let native = RelayToken::new(token.clone()).unwrap();
    let app = Arc::new(App {
        access: Access::Loopback(Zeroizing::new(token)),
        rooms: Mutex::new(HashMap::new()),
        active: AtomicUsize::new(0),
    });
    (app, native)
}

pub(super) async fn start_tls(
    ca: &CertifiedIssuer<'static, KeyPair>,
    san: &'static str,
    app: Arc<App>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    start_tls_router(ca, san, router(app)).await
}

pub(super) async fn start_tls_router(
    ca: &CertifiedIssuer<'static, KeyPair>,
    san: &'static str,
    router: Router,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut params = CertificateParams::new(vec![san.to_string()]).unwrap();
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate().unwrap();
    let certificate = params.signed_by(&key, ca).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        certificate.pem().into_bytes(),
        key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .serve(router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });
    (addr, task)
}

#[tokio::test]
async fn native_wss_uses_existing_axum_tls_route_and_relays_both_roles() {
    let (app, token) = fixture();
    let ca = make_ca();
    let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).unwrap();
    let (addr, server) = start_tls(&ca, "127.0.0.1", app.clone()).await;
    let random_room = RoomId::random();
    let room = || RoomId::new(random_room.as_str().to_owned()).unwrap();
    let (_, wrong_token) = fixture();
    assert!(matches!(
        LoopbackRelay::connect_wss(addr, &wrong_token, room(), Role::Host, &trust).await,
        Err(ClientError::Unauthorized)
    ));
    let mut host = LoopbackRelay::connect_wss(addr, &token, room(), Role::Host, &trust)
        .await
        .unwrap();
    assert!(matches!(
        host.next_event(WAIT).await,
        Ok(RelayEvent::Waiting)
    ));
    let mut viewer = LoopbackRelay::connect_wss(addr, &token, room(), Role::Viewer, &trust)
        .await
        .unwrap();
    assert!(matches!(
        host.next_event(WAIT).await,
        Ok(RelayEvent::Paired)
    ));
    assert!(matches!(
        viewer.next_event(WAIT).await,
        Ok(RelayEvent::Paired)
    ));

    viewer
        .send_signal(SignalKind::Offer, "TEST_WSS_OFFER_ONLY".into())
        .await
        .unwrap();
    assert!(matches!(
        host.next_event(WAIT).await,
        Ok(RelayEvent::Signal { kind: SignalKind::Offer, data }) if data == "TEST_WSS_OFFER_ONLY"
    ));
    host.send_signal(SignalKind::Answer, "TEST_WSS_ANSWER_ONLY".into())
        .await
        .unwrap();
    assert!(matches!(
        viewer.next_event(WAIT).await,
        Ok(RelayEvent::Signal { kind: SignalKind::Answer, data }) if data == "TEST_WSS_ANSWER_ONLY"
    ));
    viewer.leave().await.unwrap();
    assert!(matches!(
        host.next_event(WAIT).await,
        Ok(RelayEvent::PeerLeft)
    ));
    assert!(app.rooms.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn native_wss_rejects_wrong_ca_wrong_ip_and_plain_ws_without_fallback() {
    let (app, token) = fixture();
    let ca = make_ca();
    let trust = LoopbackTlsTrust::from_ca_der(ca.der().as_ref()).unwrap();
    let other_ca = make_ca();
    let other_trust = LoopbackTlsTrust::from_ca_der(other_ca.der().as_ref()).unwrap();
    let (right_ip, trusted_server) = start_tls(&ca, "127.0.0.1", app.clone()).await;
    let (wrong_ip, wrong_ip_server) = start_tls(&ca, "127.0.0.2", app.clone()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let plain_addr = listener.local_addr().unwrap();
    let plain_router = router(app.clone());
    let plain = tokio::spawn(async move {
        axum::serve(
            listener,
            plain_router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    for (addr, trust) in [
        (right_ip, &other_trust),
        (wrong_ip, &trust),
        (plain_addr, &trust),
    ] {
        assert!(matches!(
            LoopbackRelay::connect_wss(addr, &token, RoomId::random(), Role::Host, trust).await,
            Err(ClientError::TlsHandshake)
        ));
    }
    assert!(matches!(
        LoopbackRelay::connect_wss(
            "192.0.2.1:48620".parse().unwrap(),
            &token,
            RoomId::random(),
            Role::Host,
            &trust,
        )
        .await,
        Err(ClientError::Endpoint)
    ));
    assert_eq!(app.active.load(Ordering::Acquire), 0);
    assert!(app.rooms.lock().unwrap().is_empty());
    trusted_server.abort();
    wrong_ip_server.abort();
    plain.abort();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn private_wss_checks_real_socket_source_and_token_and_hello_role() {
    use super::tests::{private_app, TOKEN, WRONG_TOKEN};
    use rustls::{pki_types::CertificateDer, ClientConfig, RootCertStore};
    use tokio::net::{TcpSocket, TcpStream};
    use tokio_tungstenite::{
        client_async_tls_with_config,
        tungstenite::{
            client::IntoClientRequest, http::HeaderValue, Error as WsError, Message as Ws,
        },
        Connector, MaybeTlsStream, WebSocketStream,
    };

    type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;
    async fn recv(client: &mut Client) -> ServerFrame {
        let message = tokio::time::timeout(WAIT, client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Ws::Text(text) = message else {
            panic!("expected relay text")
        };
        signal_wire::decode(&text).unwrap()
    }
    async fn hello(client: &mut Client, room: &RoomId, role: Role) {
        let message = signal_wire::encode(&ClientFrame::Hello {
            version: VERSION,
            room: room.as_str().to_owned(),
            role,
        })
        .unwrap();
        client.send(Ws::Text(message.into())).await.unwrap();
    }

    // Linux routes all of 127/8 to loopback; no network aliases/writes needed.
    // Only the socket-source policy is under test here. Production configuration
    // rejects 127/8 for private mode, separately covered by validate_bind tests.
    let host_ip = Ipv4Addr::new(127, 0, 0, 2);
    let viewer_ip = Ipv4Addr::new(127, 0, 0, 3);
    let other_ip = Ipv4Addr::new(127, 0, 0, 4);
    let app = private_app(PeerSources {
        host: host_ip,
        viewer: viewer_ip,
    });
    let ca = make_ca();
    let (addr, task) = start_tls(&ca, "127.0.0.1", app.clone()).await;
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ca.der().as_ref())).unwrap();
    let tls_client = Arc::new(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let connect = |source: Ipv4Addr, token: &'static str| {
        let tls_client = tls_client.clone();
        async move {
            tokio::time::timeout(WAIT, async {
                let socket = TcpSocket::new_v4().unwrap();
                socket.bind(SocketAddr::from((source, 0))).unwrap();
                let stream = socket.connect(addr).await.unwrap();
                let mut request = format!("wss://{addr}{PATH}").into_client_request().unwrap();
                let mut value = HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
                value.set_sensitive(true);
                request.headers_mut().insert(header::AUTHORIZATION, value);
                // The advertised endpoint is deliberately unrelated to the real source.
                request
                    .headers_mut()
                    .insert("x-forwarded-for", HeaderValue::from_static("127.0.0.2"));
                request
                    .headers_mut()
                    .insert("forwarded", HeaderValue::from_static("for=127.0.0.2"));
                client_async_tls_with_config(
                    request,
                    stream,
                    None,
                    Some(Connector::Rustls(tls_client)),
                )
                .await
                .map(|(socket, _)| socket)
            })
            .await
            .unwrap()
        }
    };

    assert!(matches!(connect(other_ip, TOKEN).await,
        Err(WsError::Http(response)) if response.status() == StatusCode::FORBIDDEN));
    assert!(matches!(connect(host_ip, WRONG_TOKEN).await,
        Err(WsError::Http(response)) if response.status() == StatusCode::UNAUTHORIZED));
    assert!(matches!(connect(viewer_ip, TOKEN).await,
        Err(WsError::Http(response)) if response.status() == StatusCode::UNAUTHORIZED));
    assert_eq!(app.active.load(Ordering::Acquire), 0);

    let room = RoomId::random();
    let mut swapped_role = connect(host_ip, TOKEN).await.unwrap();
    hello(&mut swapped_role, &room, Role::Viewer).await;
    assert!(
        matches!(recv(&mut swapped_role).await, ServerFrame::Error { code }
        if code == "ROLE_SOURCE_MISMATCH")
    );
    assert!(app.rooms.lock().unwrap().is_empty());

    let mut host = connect(host_ip, TOKEN).await.unwrap();
    hello(&mut host, &room, Role::Host).await;
    assert!(matches!(recv(&mut host).await, ServerFrame::Waiting {}));
    let mut viewer = connect(viewer_ip, WRONG_TOKEN).await.unwrap();
    hello(&mut viewer, &room, Role::Viewer).await;
    assert!(matches!(recv(&mut host).await, ServerFrame::Paired {}));
    assert!(matches!(recv(&mut viewer).await, ServerFrame::Paired {}));
    let leave = signal_wire::encode(&ClientFrame::Leave {}).unwrap();
    viewer.send(Ws::Text(leave.into())).await.unwrap();
    assert!(matches!(recv(&mut host).await, ServerFrame::PeerLeft {}));
    assert!(app.rooms.lock().unwrap().is_empty());
    task.abort();
}
