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
        token: Zeroizing::new(token),
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
            .serve(router(app).into_make_service())
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
        axum::serve(listener, plain_router).await.unwrap();
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
