//! Exercise the public profile on real TLS, but bind only IPv4 loopback.
//! Linux can source distinct 127/8 peers without touching network settings.

use super::{
    public_video_profile::{public_video_router, PublicVideoApproval},
    tls_smoke::{make_ca, start_tls_router},
    *,
};
use hub_signal_v2::wire::PUBLIC_VIDEO_PATH;
use rustls::{pki_types::CertificateDer, ClientConfig, RootCertStore};
use tokio::{
    net::{TcpSocket, TcpStream},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::{
        client::IntoClientRequest, http::HeaderValue, Error as WsError, Message as Frame,
    },
    Connector, MaybeTlsStream, WebSocketStream,
};

const WAIT: Duration = Duration::from_secs(4);
const HOST_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);
const VIEWER_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 3);
const UNKNOWN_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 4);
const HOST_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const VIEWER_TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const WRONG_TOKEN: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Fixture {
    room: Uuid,
    address: SocketAddr,
    tls: Arc<ClientConfig>,
    task: JoinHandle<()>,
}

impl Fixture {
    async fn new(seconds: u64) -> Self {
        let room = Uuid::new_v4();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let approval = PublicVideoApproval::loopback_fixture(
            room,
            PeerSources {
                host: HOST_IP,
                viewer: VIEWER_IP,
            },
            HOST_TOKEN.to_owned(),
            VIEWER_TOKEN.to_owned(),
            now + seconds,
            now,
        )
        .unwrap();
        let ca = make_ca();
        let (address, task) =
            start_tls_router(&ca, "127.0.0.1", public_video_router(approval)).await;
        let mut roots = RootCertStore::empty();
        roots.add(CertificateDer::from(ca.der().as_ref())).unwrap();
        let tls = Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        Self {
            room,
            address,
            tls,
            task,
        }
    }

    async fn connect(&self, source: Ipv4Addr, token: &str, path: &str) -> Result<Client, WsError> {
        timeout(WAIT, async {
            let socket = TcpSocket::new_v4().unwrap();
            socket.bind(SocketAddr::from((source, 0))).unwrap();
            let tcp = socket.connect(self.address).await.unwrap();
            let mut request = format!("wss://{}{path}", self.address)
                .into_client_request()
                .unwrap();
            let mut auth = HeaderValue::from_str(&format!("Bearer {token}")).unwrap();
            auth.set_sensitive(true);
            request.headers_mut().insert(header::AUTHORIZATION, auth);
            // Cannot turn an unapproved TCP source into the approved host.
            request
                .headers_mut()
                .insert("x-forwarded-for", HeaderValue::from_static("127.0.0.2"));
            request
                .headers_mut()
                .insert("forwarded", HeaderValue::from_static("for=127.0.0.2"));
            client_async_tls_with_config(
                request,
                tcp,
                None,
                Some(Connector::Rustls(self.tls.clone())),
            )
            .await
            .map(|(socket, _)| socket)
        })
        .await
        .expect("bounded local TLS upgrade")
    }

    async fn pair(&self) -> (Client, Client) {
        let mut host = self
            .connect(HOST_IP, HOST_TOKEN, PUBLIC_VIDEO_PATH)
            .await
            .unwrap();
        hello(&mut host, self.room, Role::Host).await;
        assert!(matches!(receive(&mut host).await, ServerFrame::Waiting {}));
        let mut viewer = self
            .connect(VIEWER_IP, VIEWER_TOKEN, PUBLIC_VIDEO_PATH)
            .await
            .unwrap();
        hello(&mut viewer, self.room, Role::Viewer).await;
        assert!(matches!(receive(&mut host).await, ServerFrame::Paired {}));
        assert!(matches!(receive(&mut viewer).await, ServerFrame::Paired {}));
        (host, viewer)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn send(client: &mut Client, frame: ClientFrame) {
    timeout(
        WAIT,
        client.send(Frame::Text(signal_wire::encode(&frame).unwrap().into())),
    )
    .await
    .unwrap()
    .unwrap();
}

async fn hello(client: &mut Client, room: Uuid, role: Role) {
    send(
        client,
        ClientFrame::Hello {
            version: VERSION,
            room: room.simple().to_string(),
            role,
        },
    )
    .await;
}

async fn receive(client: &mut Client) -> ServerFrame {
    let frame = timeout(WAIT, client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Frame::Text(wire) = frame else {
        panic!("expected a V2 signal frame")
    };
    signal_wire::decode(&wire).unwrap()
}

async fn signal(client: &mut Client, seq: u64, signal: SignalKind) {
    send(
        client,
        ClientFrame::Signal {
            seq,
            signal,
            data: "fixture-only".into(),
        },
    )
    .await;
}

async fn forwarded(from: &mut Client, to: &mut Client, role: Role, seq: u64, kind: SignalKind) {
    signal(from, seq, kind).await;
    assert!(matches!(
        receive(to).await,
        ServerFrame::Signal { from, seq: seen, signal, data }
            if from == role && seen == seq && signal == kind && data == "fixture-only"
    ));
}

async fn assert_cannot_join(result: Result<Client, WsError>, room: Uuid, role: Role) {
    match result {
        Err(WsError::Http(response)) => {
            assert!(!response.status().is_success());
        }
        Ok(mut client) => {
            hello(&mut client, room, role).await;
            assert!(matches!(
                receive(&mut client).await,
                ServerFrame::Error { .. }
            ));
        }
        Err(other) => panic!("unexpected fixture transport error: {other}"),
    }
}

#[tokio::test]
async fn public_profile_binds_exact_source_token_room_and_retires_both_slots() {
    let fixture = Fixture::new(60).await;
    for (source, bearer) in [
        (UNKNOWN_IP, HOST_TOKEN),
        (HOST_IP, WRONG_TOKEN),
        (HOST_IP, VIEWER_TOKEN),
        (VIEWER_IP, HOST_TOKEN),
    ] {
        assert!(matches!(
            fixture.connect(source, bearer, PUBLIC_VIDEO_PATH).await,
            Err(WsError::Http(response)) if matches!(response.status(), StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED)
        ));
    }
    assert!(matches!(
        fixture.connect(HOST_IP, HOST_TOKEN, PATH).await,
        Err(WsError::Http(response)) if response.status() == StatusCode::NOT_FOUND
    ));
    for (source, token, room, role) in [
        (HOST_IP, HOST_TOKEN, Uuid::new_v4(), Role::Host),
        (HOST_IP, HOST_TOKEN, fixture.room, Role::Viewer),
        (VIEWER_IP, VIEWER_TOKEN, fixture.room, Role::Viewer),
    ] {
        assert_cannot_join(
            fixture.connect(source, token, PUBLIC_VIDEO_PATH).await,
            room,
            role,
        )
        .await;
    }

    // Rejected pre-admission attempts have not consumed a legitimate slot.
    let (mut host, mut viewer) = fixture.pair().await;
    // A duplicate cannot replace the live host or advance its sequence.
    assert_cannot_join(
        fixture
            .connect(HOST_IP, HOST_TOKEN, PUBLIC_VIDEO_PATH)
            .await,
        fixture.room,
        Role::Host,
    )
    .await;
    forwarded(
        &mut host,
        &mut viewer,
        Role::Host,
        1,
        SignalKind::HostChallenge,
    )
    .await;
    forwarded(
        &mut viewer,
        &mut host,
        Role::Viewer,
        1,
        SignalKind::ViewerChallenge,
    )
    .await;
    forwarded(&mut viewer, &mut host, Role::Viewer, 2, SignalKind::Offer).await;
    forwarded(&mut host, &mut viewer, Role::Host, 2, SignalKind::Answer).await;
    signal(&mut viewer, 3, SignalKind::Ice).await;
    assert!(matches!(
        receive(&mut viewer).await,
        ServerFrame::Error { .. }
    ));
    assert!(matches!(receive(&mut host).await, ServerFrame::PeerLeft {}));

    for (source, token, role) in [
        (HOST_IP, HOST_TOKEN, Role::Host),
        (VIEWER_IP, VIEWER_TOKEN, Role::Viewer),
    ] {
        assert_cannot_join(
            fixture.connect(source, token, PUBLIC_VIDEO_PATH).await,
            fixture.room,
            role,
        )
        .await;
    }
}

#[tokio::test]
async fn public_profile_rejects_offer_without_challenges_and_expires_at_idle() {
    let fixture = Fixture::new(60).await;
    let (mut host, mut viewer) = fixture.pair().await;
    signal(&mut viewer, 1, SignalKind::Offer).await;
    assert!(matches!(
        receive(&mut viewer).await,
        ServerFrame::Error { .. }
    ));
    assert!(matches!(receive(&mut host).await, ServerFrame::PeerLeft {}));

    let short = Fixture::new(2).await;
    let mut waiting = short
        .connect(HOST_IP, HOST_TOKEN, PUBLIC_VIDEO_PATH)
        .await
        .unwrap();
    hello(&mut waiting, short.room, Role::Host).await;
    assert!(matches!(
        receive(&mut waiting).await,
        ServerFrame::Waiting {}
    ));
    let ending = timeout(WAIT, waiting.next())
        .await
        .expect("idle room closed at approval expiry");
    assert!(matches!(
        ending,
        None | Some(Err(_)) | Some(Ok(Frame::Close(_)))
    ));
    assert_cannot_join(
        short
            .connect(VIEWER_IP, VIEWER_TOKEN, PUBLIC_VIDEO_PATH)
            .await,
        short.room,
        Role::Viewer,
    )
    .await;
}
