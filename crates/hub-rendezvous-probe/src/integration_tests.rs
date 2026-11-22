use super::*;
use hub_probe_wire::{Codec, PairKey};
use tokio_tungstenite::{
    connect_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, Message as ClientMessage},
    Connector,
};

fn fixture() -> Arc<App> {
    Arc::new(App {
        config: Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            public_ipv4: "8.8.8.8".parse().unwrap(),
            sender_invitation: "a".repeat(64),
            receiver_invitation: "b".repeat(64),
            invitation_expires_at: now() + 3600,
            realm: "test.invalid".into(),
            certificate: String::new(),
            private_key: String::new(),
        },
        issuer: Issuer::new(vec![b'x'; 32], "test.invalid".into()).unwrap(),
        rooms: Mutex::new(HashMap::new()),
        retired: Mutex::new(Vec::new()),
        connections: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn rooms_require_both_roles_reject_replay_and_retire() {
    let app = fixture();
    let (a, mut ar) = mpsc::channel(8);
    let (b, mut br) = mpsc::channel(8);
    let h = Hello {
        mode: Default::default(),
        version: 1,
        role: Role::Sender,
        room: "c".repeat(64),
        seconds: 30,
    };
    app.join(&h, a.clone()).unwrap();
    assert!(matches!(ar.recv().await, Some(Message::Text(_))));
    assert!(app.join(&h, a).is_err());
    let receiver = Hello {
        role: Role::Receiver,
        ..h
    };
    app.join(&receiver, b).unwrap();
    let Message::Text(r) = ar.recv().await.unwrap() else {
        panic!("ready")
    };
    let ServerEvent::Ready(ready) = serde_json::from_str(&r).unwrap() else {
        panic!("ready")
    };
    assert_eq!(ready.ice_servers.len(), 1);
    assert!(br.recv().await.is_some());
    let mut codec = Codec::new(
        Arc::new(PairKey::generate().unwrap()),
        &ready,
        Role::Receiver,
        now(),
    )
    .unwrap();
    let text = codec
        .encode("offer", "sdp-test-only".into(), now())
        .unwrap();
    assert!(!app
        .forward(&receiver.room, Role::Receiver, text.clone())
        .unwrap());
    assert!(app.forward(&receiver.room, Role::Receiver, text).is_err());
    app.leave(&receiver.room, "PEER_STOPPED");
    assert!(app.rooms.lock().unwrap().is_empty());
    let (tx, _) = mpsc::channel(8);
    assert!(app
        .join(
            &Hello {
                role: Role::Sender,
                ..receiver
            },
            tx
        )
        .is_err());
    for _ in 0..8 {
        assert!(app.reserve_connection());
    }
    assert!(!app.reserve_connection());
    drop(ConnectionLease(app.clone()));
    assert!(app.reserve_connection());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_wss_validates_tls_invitation_origin_roles_and_authenticated_frames() {
    wss_case(hub_probe_wire::PathMode::RelayOnly).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_wss_direct_mode_authenticates_path_policy_before_video_sdp() {
    wss_case(hub_probe_wire::PathMode::DirectPreferred).await;
}
async fn wss_case(mode: hub_probe_wire::PathMode) {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certificate = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
    let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
        certificate.cert.pem().into_bytes(),
        certificate.signing_key.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate.cert.der().clone()).unwrap();
    let connector = Connector::Rustls(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ));
    let untrusted = Connector::Rustls(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(rustls::RootCertStore::empty())
            .with_no_client_auth(),
    ));
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = fixture();
    let router = Router::new()
        .route("/probe", get(upgrade))
        .with_state(app.clone());
    let server = tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, tls)
            .unwrap()
            .serve(router.into_make_service())
            .await
            .unwrap()
    });
    let request = |host: &str, token: &str, origin: &str| {
        let mut r = format!("wss://{host}:{port}/probe")
            .into_client_request()
            .unwrap();
        r.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
        r.headers_mut().insert("origin", origin.parse().unwrap());
        r
    };
    for (host, token, origin, tls) in [
        ("127.0.0.1", "a".repeat(64), "remotehub-p006b", untrusted),
        (
            "localhost",
            "a".repeat(64),
            "remotehub-p006b",
            connector.clone(),
        ),
        (
            "127.0.0.1",
            "0".repeat(64),
            "remotehub-p006b",
            connector.clone(),
        ),
        (
            "127.0.0.1",
            "a".repeat(64),
            "unapproved-origin",
            connector.clone(),
        ),
    ] {
        assert!(tokio::time::timeout(
            Duration::from_secs(3),
            connect_async_tls_with_config(request(host, &token, origin), None, true, Some(tls))
        )
        .await
        .expect("negative TLS/auth handshake timed out")
        .is_err());
    }
    let (mut a, _) = tokio::time::timeout(
        Duration::from_secs(3),
        connect_async_tls_with_config(
            request("127.0.0.1", &"a".repeat(64), "remotehub-p006b"),
            None,
            true,
            Some(connector.clone()),
        ),
    )
    .await
    .expect("sender handshake timeout")
    .unwrap();
    let (mut b, _) = tokio::time::timeout(
        Duration::from_secs(3),
        connect_async_tls_with_config(
            request("127.0.0.1", &"b".repeat(64), "remotehub-p006b"),
            None,
            true,
            Some(connector),
        ),
    )
    .await
    .expect("receiver handshake timeout")
    .unwrap();
    let key = Arc::new(PairKey::generate().unwrap());
    for (ws, role) in [(&mut a, Role::Sender), (&mut b, Role::Receiver)] {
        ws.send(ClientMessage::Text(
            serde_json::to_string(&Hello {
                mode,
                version: 1,
                role,
                room: key.room(),
                seconds: 30,
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();
        if role == Role::Sender {
            let waiting = tokio::time::timeout(Duration::from_secs(3), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(
                matches!(waiting, ClientMessage::Text(t) if matches!(serde_json::from_str::<ServerEvent>(&t).unwrap(), ServerEvent::Waiting))
            );
        }
    }
    async fn ready(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Ready {
        loop {
            let m = tokio::time::timeout(Duration::from_secs(3), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if let ClientMessage::Text(t) = m {
                if let ServerEvent::Ready(r) = serde_json::from_str(&t).unwrap() {
                    return r;
                }
            }
        }
    }
    let ra = ready(&mut a).await;
    let rb = ready(&mut b).await;
    assert_eq!(ra.expires_at, rb.expires_at);
    let mut ac = Codec::new(key.clone(), &ra, Role::Sender, now()).unwrap();
    let mut bc = Codec::new(key, &rb, Role::Receiver, now()).unwrap();
    if mode == hub_probe_wire::PathMode::DirectPreferred {
        let policy =
            hub_probe_wire::PathExchange::new(vec!["192.168.1.2".parse().unwrap()]).unwrap();
        a.send(ClientMessage::Text(
            ac.encode("path", policy.payload().unwrap(), now())
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
        let ClientMessage::Text(text) = tokio::time::timeout(Duration::from_secs(3), b.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("path frame")
        };
        let (kind, payload) = bc.accept(&text, now()).unwrap();
        assert_eq!(kind, "path");
        let mut incoming = hub_probe_wire::PathExchange::new(vec![]).unwrap();
        incoming.accept(&payload).unwrap();
        assert!(incoming.may_publish_private_local("192.168.1.2".parse().unwrap()));
        b.send(ClientMessage::Text(
            bc.encode("path", policy.payload().unwrap(), now())
                .unwrap()
                .into(),
        ))
        .await
        .unwrap();
        let ClientMessage::Text(text) = tokio::time::timeout(Duration::from_secs(3), a.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("path frame")
        };
        assert_eq!(ac.accept(&text, now()).unwrap().0, "path");
    }
    let offer = bc.encode("offer", "test-only-sdp".into(), now()).unwrap();
    b.send(ClientMessage::Text(offer.clone().into()))
        .await
        .unwrap();
    let ClientMessage::Text(text) = tokio::time::timeout(Duration::from_secs(3), a.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
    else {
        panic!("frame")
    };
    assert_eq!(ac.accept(&text, now()).unwrap().1, "test-only-sdp");
    b.send(ClientMessage::Text(offer.into())).await.unwrap();
    let next = tokio::time::timeout(Duration::from_secs(3), a.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(next,ClientMessage::Text(t) if t.contains("failed")));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(app.rooms.lock().unwrap().is_empty());
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn direct_room_rejects_mismatched_mode_before_creating_receiver_grants() {
    let app = fixture();
    let (a, mut ar) = mpsc::channel(8);
    let (b, _) = mpsc::channel(8);
    let sender = Hello {
        mode: hub_probe_wire::PathMode::DirectPreferred,
        version: 1,
        role: Role::Sender,
        room: "d".repeat(64),
        seconds: 30,
    };
    app.join(&sender, a).unwrap();
    let _ = ar.recv().await;
    let wrong = Hello {
        mode: hub_probe_wire::PathMode::RelayOnly,
        role: Role::Receiver,
        ..sender
    };
    assert_eq!(app.join(&wrong, b).unwrap_err(), "PATH_MODE_MISMATCH");
    assert!(app.rooms.lock().unwrap()[&wrong.room].channels[1].is_none());
    app.leave(&wrong.room, "PEER_STOPPED");
    assert!(app.rooms.lock().unwrap().is_empty());
}
