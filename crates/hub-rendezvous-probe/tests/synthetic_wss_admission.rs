//! Dedicated Linux loopback fixture, not an executable relay or product route.
//! Different 127/8 source addresses exercise the actual socket-peer gate.
#![cfg(target_os = "linux")]

use axum::{
    extract::{
        rejection::ExtensionRejection,
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, OriginalUri, State,
    },
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use axum_server::{tls_rustls::RustlsConfig, Handle};
use futures_util::{SinkExt, StreamExt};
use hub_identity::{
    pairing::{PairingCandidate, VerifiedPeer},
    session::{SessionChallenge, SessionRole},
    Identity,
};
use hub_signal_v2::{
    synthetic::{
        self, connect_loopback_wss, ClientError as SyntheticClientError, Flow, Kind, Scope,
        SyntheticEndpoint, SyntheticInvitation, SyntheticRelay, MAX_FRAME_BYTES, PAIRED, PATH,
    },
    wire::PUBLIC_VIDEO_PATH,
    Role,
};
use hub_webrtc::{
    ice_policy::IceMode,
    public_fake_input::{
        Error as ProofError, FakeInputProof, FakeInputProofScope, FakeInputTestMode,
    },
    public_video::PublicVideoScope,
};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use ring::{
    digest::{digest, SHA256},
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use rustls::{
    pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer},
    ClientConfig, RootCertStore, ServerConfig,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::{TcpSocket, TcpStream},
    sync::mpsc,
    task::JoinHandle,
    time::{timeout, timeout_at, Instant},
};
use tokio_tungstenite::{
    client_async_tls_with_config,
    tungstenite::{client::IntoClientRequest, Error as ClientError, Message as ClientMessage},
    Connector, MaybeTlsStream, WebSocketStream,
};
use uuid::Uuid;
use zeroize::Zeroizing;

const HOST_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 2);
const VIEWER_IP: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 3);
const ROLE: &str = "x-rh-fixture-role";
const ROOM: &str = "x-rh-fixture-room";
const GENERATION: &str = "x-rh-fixture-generation";
const LEASE: Duration = Duration::from_secs(12);
const IO_WAIT: Duration = Duration::from_secs(3);
const AUTH_LABEL: &[u8] = b"RemoteHub synthetic fixture invitation";
const RHF1_HEADER: usize = 54;
const RELAY_LABEL: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);
const OFFER_PORT: u16 = 49152;
const ANSWER_PORT: u16 = 49153;

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn index(role: Role) -> usize {
    match role {
        Role::Host => 0,
        Role::Viewer => 1,
    }
}

fn random32() -> [u8; 32] {
    let mut bytes = [0; 32];
    SystemRandom::new().fill(&mut bytes).expect("fixture RNG");
    bytes
}

fn fixture_ca() -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

fn wall() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn ca_pin(der: &[u8]) -> [u8; 32] {
    digest(&SHA256, der).as_ref().try_into().unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn generation(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return None;
    }
    let mut decoded = [0; 32];
    for (index, byte) in decoded.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(decoded)
}

fn invitation_matches(expected: &str, candidate: &str) -> bool {
    if generation(candidate).is_none() {
        return false;
    }
    let expected = hmac::Key::new(hmac::HMAC_SHA256, expected.as_bytes());
    let candidate = hmac::Key::new(hmac::HMAC_SHA256, candidate.as_bytes());
    hmac::verify(
        &candidate,
        AUTH_LABEL,
        hmac::sign(&expected, AUTH_LABEL).as_ref(),
    )
    .is_ok()
}

struct Slot {
    source: Ipv4Addr,
    role: Role,
    invitation: Zeroizing<String>,
}

struct Gate {
    used: [bool; 2],
    active: [bool; 2],
    outbound: [Option<mpsc::Sender<Message>>; 2],
    terminal: bool,
    flow: Flow,
}

impl Gate {
    fn close(&mut self) {
        self.terminal = true;
        self.active = [false; 2];
        self.flow.close();
        self.outbound = [None, None];
    }
}

struct App {
    slots: [Slot; 2],
    room: Uuid,
    generation: [u8; 32],
    scope: Scope,
    expires: Instant,
    gate: Mutex<Gate>,
}

impl App {
    fn new(expires: Instant) -> Arc<Self> {
        let room = Uuid::new_v4();
        let generation = random32();
        let scope = Scope::new(room, generation).unwrap();
        let slots = [(HOST_IP, Role::Host), (VIEWER_IP, Role::Viewer)].map(|(source, role)| Slot {
            source,
            role,
            invitation: Zeroizing::new(hex(&random32())),
        });
        assert!(slots[0].invitation.as_str() != slots[1].invitation.as_str());
        Arc::new(Self {
            slots,
            room,
            generation,
            scope,
            expires,
            gate: Mutex::new(Gate {
                used: [false; 2],
                active: [false; 2],
                outbound: [None, None],
                terminal: false,
                flow: Flow::new(scope),
            }),
        })
    }

    fn reserve(
        &self,
        peer: IpAddr,
        target: &str,
        headers: &HeaderMap,
    ) -> Option<(Role, mpsc::Receiver<Message>)> {
        let slot = self
            .slots
            .iter()
            .find(|slot| peer == IpAddr::V4(slot.source))?;
        let role = match only_header(headers, ROLE)? {
            "host" => Role::Host,
            "viewer" => Role::Viewer,
            _ => return None,
        };
        let raw_room = only_header(headers, ROOM)?;
        let room = Uuid::try_parse(raw_room).ok()?;
        if raw_room != room.simple().to_string() || headers.contains_key(header::ORIGIN) {
            return None;
        }
        let offered = Scope::new(room, generation(only_header(headers, GENERATION)?)?).ok()?;
        synthetic::admit(target, self.scope, offered, slot.role, role).ok()?;
        let token =
            only_header(headers, header::AUTHORIZATION.as_str())?.strip_prefix("Bearer ")?;
        if !invitation_matches(&slot.invitation, token) {
            return None;
        }

        // No invalid caller can claim a role. Mark it irreversibly within
        // the same lock that installs the queue and checks the deadline.
        let slot = index(role);
        let mut gate = self.gate.lock().ok()?;
        if gate.terminal || gate.used[slot] || Instant::now() >= self.expires {
            return None;
        }
        // One internal readiness message + at most two peer proof frames.
        let (tx, rx) = mpsc::channel(3);
        gate.used[slot] = true;
        gate.outbound[slot] = Some(tx);
        Some((role, rx))
    }

    fn activate(&self, role: Role) -> bool {
        let Ok(mut gate) = self.gate.lock() else {
            return false;
        };
        let slot = index(role);
        if gate.terminal
            || gate.active[slot]
            || gate.outbound[slot].is_none()
            || Instant::now() >= self.expires
        {
            return false;
        }
        gate.active[slot] = true;
        if gate.active == [true; 2]
            && !gate.outbound.iter().all(|tx| {
                tx.as_ref()
                    .is_some_and(|tx| tx.try_send(Message::Text(PAIRED.into())).is_ok())
            })
        {
            gate.close();
            return false;
        }
        // The relay queues its readiness for both roles in the same critical
        // section as activation. No accepted RHF1 bytes can get ahead of it.
        true
    }

    fn paired(&self) -> bool {
        Instant::now() < self.expires
            && self
                .gate
                .lock()
                .is_ok_and(|gate| !gate.terminal && gate.active == [true; 2])
    }

    fn forward(&self, role: Role, bytes: axum::body::Bytes) -> bool {
        let Ok(mut gate) = self.gate.lock() else {
            return false;
        };
        if gate.terminal
            || Instant::now() >= self.expires
            || gate.active != [true; 2]
            || gate.flow.accept(role, &bytes).is_err()
            || !gate.outbound[1 - index(role)]
                .as_ref()
                .is_some_and(|tx| tx.try_send(Message::Binary(bytes)).is_ok())
        {
            gate.close();
            return false;
        }
        true
    }

    fn close(&self) {
        if let Ok(mut gate) = self.gate.lock() {
            gate.close();
        }
    }
}

fn only_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let mut entries = headers.get_all(name).iter();
    let first = entries.next()?.to_str().ok()?;
    entries.next().is_none().then_some(first)
}

async fn upgrade(
    State(app): State<Arc<App>>,
    peer: Result<ConnectInfo<SocketAddr>, ExtensionRejection>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    socket: WebSocketUpgrade,
) -> Response {
    let reservation = peer.ok().and_then(|ConnectInfo(peer)| {
        app.reserve(peer.ip(), uri.path_and_query()?.as_str(), &headers)
    });
    let Some((role, outbound)) = reservation else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let failed = app.clone();
    socket
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_FRAME_BYTES * 2)
        .max_frame_size(MAX_FRAME_BYTES)
        .max_message_size(MAX_FRAME_BYTES)
        .on_failed_upgrade(move |_| failed.close())
        .on_upgrade(move |socket| serve(socket, app, role, outbound))
}

struct Session(Arc<App>);

impl Drop for Session {
    fn drop(&mut self) {
        self.0.close();
    }
}

async fn serve(
    mut socket: WebSocket,
    app: Arc<App>,
    role: Role,
    mut outbound: mpsc::Receiver<Message>,
) {
    let lease = Session(app);
    let _ = timeout_at(lease.0.expires, async {
        if !lease.0.activate(role) {
            return;
        }
        loop {
            tokio::select! {
                frame = socket.recv() => {
                    match frame {
                        Some(Ok(Message::Binary(bytes))) => {
                            if !lease.0.forward(role, bytes) { break; }
                        }
                        _ => break,
                    }
                }
                pending = outbound.recv() => {
                    let Some(frame) = pending else { break };
                    // Dropping a sender does not discard an already-buffered
                    // frame. Recheck the terminal gate before this write.
                    if !lease.0.paired() || socket.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
    .await;
    drop(lease);
    let _ = timeout(
        Duration::from_millis(150),
        socket.send(Message::Close(None)),
    )
    .await;
}

struct Fixture {
    app: Arc<App>,
    address: SocketAddr,
    trust: Arc<ClientConfig>,
    ca_der: Vec<u8>,
    handle: Handle<SocketAddr>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl Fixture {
    async fn start(app: Arc<App>) -> Self {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let ca = fixture_ca();
        let ca_der = ca.der().as_ref().to_vec();
        let mut params = CertificateParams::new(vec![Ipv4Addr::LOCALHOST.to_string()]).unwrap();
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let leaf = params.signed_by(&key, &ca).unwrap();
        let private = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
        let tls = ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone()], private)
            .unwrap();
        let mut roots = RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        let trust = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = Handle::new();
        let router = Router::new()
            .route(PATH, get(upgrade))
            .with_state(app.clone());
        let server =
            axum_server::from_tcp_rustls(listener, RustlsConfig::from_config(Arc::new(tls)))
                .unwrap()
                .handle(handle.clone());
        let task = tokio::spawn(async move {
            server
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await
        });
        Self {
            app,
            address,
            trust: Arc::new(trust),
            ca_der,
            handle,
            task: Some(task),
        }
    }

    async fn connect(
        &self,
        source: Ipv4Addr,
        path: &str,
        role: Role,
        mutate: impl FnOnce(&mut HeaderMap),
    ) -> Result<Client, ClientError> {
        let mut request = format!("wss://{}{path}", self.address)
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert(
            ROLE,
            match role {
                Role::Host => "host",
                Role::Viewer => "viewer",
            }
            .parse()
            .unwrap(),
        );
        headers.insert(ROOM, self.app.room.simple().to_string().parse().unwrap());
        headers.insert(GENERATION, hex(&self.app.generation).parse().unwrap());
        let mut authorization: axum::http::HeaderValue =
            format!("Bearer {}", self.app.slots[index(role)].invitation.as_str())
                .parse()
                .unwrap();
        authorization.set_sensitive(true);
        headers.insert(header::AUTHORIZATION, authorization);
        mutate(headers);
        assert!(source.is_loopback());
        let tcp = TcpSocket::new_v4().unwrap();
        tcp.bind((source, 0).into()).unwrap();
        let tcp = timeout(IO_WAIT, tcp.connect(self.address))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tcp.local_addr().unwrap().ip(), IpAddr::V4(source));
        timeout(
            IO_WAIT,
            client_async_tls_with_config(
                request,
                tcp,
                None,
                Some(Connector::Rustls(self.trust.clone())),
            ),
        )
        .await
        .unwrap()
        .map(|(socket, _)| socket)
    }

    async fn pinned_client(
        &self,
        role: Role,
        ca_der: &[u8],
        expected_pin: [u8; 32],
    ) -> Result<SyntheticRelay, SyntheticClientError> {
        let SocketAddr::V4(address) = self.address else {
            unreachable!("fixture binds IPv4")
        };
        let slot = &self.app.slots[index(role)];
        let now = wall();
        let endpoint = SyntheticEndpoint::new(slot.source, address, now + LEASE.as_secs(), now)?;
        let invitation = SyntheticInvitation::new(
            self.app.scope,
            role,
            slot.invitation.as_str().to_owned(),
            expected_pin,
        )?;
        connect_loopback_wss(endpoint, invitation, ca_der).await
    }

    fn proof_scope(&self) -> FakeInputProofScope {
        FakeInputProofScope {
            room: self.app.room,
            generation: self.app.generation,
            route: PublicVideoScope {
                // Labels bound into the proof only. The client's physical
                // destination is its exact pinned 127/8 socket; no TURN runs.
                wss: SocketAddrV4::new(RELAY_LABEL, self.address.port()),
                ca_sha256: ca_pin(&self.ca_der),
                turn: SocketAddrV4::new(RELAY_LABEL, 3478),
                relay_ports: (OFFER_PORT, ANSWER_PORT),
                mode: IceMode::RelayOnly,
                expires_at: wall() + LEASE.as_secs(),
            },
        }
    }

    fn frame(&self, tag: [u8; 2], body: &[u8]) -> Vec<u8> {
        let mut bytes = b"RHF1".to_vec();
        bytes.extend_from_slice(&tag);
        bytes.extend_from_slice(self.app.room.as_bytes());
        bytes.extend_from_slice(&self.app.generation);
        bytes.extend_from_slice(body);
        bytes
    }

    async fn finish(mut self) {
        self.app.close();
        self.handle.shutdown();
        timeout(IO_WAIT, self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.app.close();
        self.handle.shutdown();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn denied(result: Result<Client, ClientError>, expected: StatusCode) {
    assert!(matches!(result, Err(ClientError::Http(response)) if response.status() == expected));
}

async fn raw_paired(client: &mut Client) {
    let message = timeout(IO_WAIT, client.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(message, ClientMessage::Text(PAIRED.into()));
}

async fn no_frame(client: &mut Client) {
    // A failed gate closes instead of returning a retryable session.
    let next = timeout(IO_WAIT, client.next()).await.unwrap();
    assert!(matches!(
        next,
        Some(Ok(ClientMessage::Close(_))) | Some(Err(_)) | None
    ));
}

async fn relay(sender: &mut Client, receiver: &mut Client, bytes: Vec<u8>) {
    sender
        .send(ClientMessage::Binary(bytes.clone().into()))
        .await
        .unwrap();
    let received = timeout(IO_WAIT, receiver.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(received, ClientMessage::Binary(bytes.into()));
}

#[tokio::test]
async fn authorizes_before_upgrade_and_relays_one_exact_synthetic_pair() {
    let fixture = Fixture::start(App::new(Instant::now() + LEASE)).await;
    denied(
        fixture
            .connect(HOST_IP, PUBLIC_VIDEO_PATH, Role::Host, |_| {})
            .await,
        StatusCode::NOT_FOUND,
    );
    denied(
        fixture
            .connect(Ipv4Addr::LOCALHOST, PATH, Role::Host, |headers| {
                headers.insert("x-forwarded-for", "127.0.0.2".parse().unwrap());
            })
            .await,
        StatusCode::FORBIDDEN,
    );
    denied(
        fixture.connect(HOST_IP, PATH, Role::Viewer, |_| {}).await,
        StatusCode::FORBIDDEN,
    );
    denied(
        fixture
            .connect(HOST_IP, PATH, Role::Host, |headers| {
                headers.insert(
                    header::AUTHORIZATION,
                    format!("Bearer {}", "0".repeat(64)).parse().unwrap(),
                );
            })
            .await,
        StatusCode::FORBIDDEN,
    );
    denied(
        fixture
            .connect(HOST_IP, &format!("{PATH}?not=approved"), Role::Host, |_| {})
            .await,
        StatusCode::FORBIDDEN,
    );
    denied(
        fixture
            .connect(HOST_IP, PATH, Role::Host, |headers| {
                headers.insert(ROOM, Uuid::new_v4().simple().to_string().parse().unwrap());
            })
            .await,
        StatusCode::FORBIDDEN,
    );
    denied(
        fixture
            .connect(HOST_IP, PATH, Role::Host, |headers| {
                headers.insert(GENERATION, hex(&random32()).parse().unwrap());
            })
            .await,
        StatusCode::FORBIDDEN,
    );

    let mut host = fixture
        .connect(HOST_IP, PATH, Role::Host, |_| {})
        .await
        .unwrap();
    denied(
        fixture.connect(HOST_IP, PATH, Role::Host, |_| {}).await,
        StatusCode::FORBIDDEN,
    );
    let mut viewer = fixture
        .connect(VIEWER_IP, PATH, Role::Viewer, |_| {})
        .await
        .unwrap();
    tokio::join!(raw_paired(&mut host), raw_paired(&mut viewer));

    let mut challenge = [9; 48];
    challenge[32..40].copy_from_slice(&100_u64.to_be_bytes());
    challenge[40..].copy_from_slice(&120_u64.to_be_bytes());
    let sdp = b"v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n";
    let mut signed = (sdp.len() as u16).to_be_bytes().to_vec();
    signed.extend([5; 64]);
    signed.extend_from_slice(sdp);
    relay(&mut host, &mut viewer, fixture.frame([1, 1], &challenge)).await;
    relay(&mut viewer, &mut host, fixture.frame([2, 2], &challenge)).await;
    relay(&mut viewer, &mut host, fixture.frame([3, 2], &signed)).await;
    relay(&mut host, &mut viewer, fixture.frame([4, 1], &signed)).await;
    assert!(fixture.app.gate.lock().unwrap().flow.is_complete());

    viewer
        .send(ClientMessage::Binary(fixture.frame([3, 2], &signed).into()))
        .await
        .unwrap();
    tokio::join!(no_frame(&mut host), no_frame(&mut viewer));
    denied(
        fixture.connect(VIEWER_IP, PATH, Role::Viewer, |_| {}).await,
        StatusCode::FORBIDDEN,
    );
    fixture.finish().await;
}

#[tokio::test]
async fn rejects_expired_invite_and_closes_premature_unpaired_flow() {
    let fixture = Fixture::start(App::new(Instant::now())).await;
    denied(
        fixture.connect(HOST_IP, PATH, Role::Host, |_| {}).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(fixture.app.gate.lock().unwrap().used, [false, false]);
    fixture.finish().await;

    let fixture = Fixture::start(App::new(Instant::now() + LEASE)).await;
    let mut host = fixture
        .connect(HOST_IP, PATH, Role::Host, |_| {})
        .await
        .unwrap();
    let mut challenge = [9; 48];
    challenge[32..40].copy_from_slice(&100_u64.to_be_bytes());
    challenge[40..].copy_from_slice(&120_u64.to_be_bytes());
    host.send(ClientMessage::Binary(
        fixture.frame([1, 1], &challenge).into(),
    ))
    .await
    .unwrap();
    no_frame(&mut host).await;
    denied(
        fixture.connect(VIEWER_IP, PATH, Role::Viewer, |_| {}).await,
        StatusCode::FORBIDDEN,
    );
    fixture.finish().await;
}

fn pin(local: &Identity, peer: &Identity) -> VerifiedPeer {
    let peer = peer.public();
    let candidate = PairingCandidate::parse(&peer.pairing_code()).unwrap();
    // The expected full fingerprint comes from the independently generated
    // local fixture identity, not from the untrusted WSS messages.
    local.verify_peer(&candidate, &peer.fingerprint).unwrap()
}

fn fixed_sdp(role: SessionRole) -> String {
    let (setup, port, fingerprint) = match role {
        SessionRole::Host => ("active", ANSWER_PORT, "CD"),
        SessionRole::Viewer => ("actpass", OFFER_PORT, "AB"),
    };
    format!(
        concat!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\n",
            "a=fingerprint:sha-256 {fingerprint}\r\na=group:BUNDLE 0\r\n",
            "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\n",
            "a=setup:{setup}\r\na=mid:0\r\na=sctp-port:5000\r\na=ice-ufrag:ABCD\r\n",
            "a=ice-pwd:abcdefghijklmnopqrstuv\r\n",
            "a=candidate:r 1 udp 16777215 {relay} {port} typ relay raddr 0.0.0.0 rport 0\r\n",
            "a=end-of-candidates\r\n",
        ),
        fingerprint = [fingerprint; 32].join(":"),
        setup = setup,
        port = port,
        relay = RELAY_LABEL,
    )
}

async fn pinned_transfer(
    sender: &mut SyntheticRelay,
    receiver: &mut SyntheticRelay,
    kind: Kind,
    frame: Vec<u8>,
) -> Vec<u8> {
    assert_eq!(sender.send_frame(frame.clone()).await.unwrap(), kind);
    let (received_kind, received) = receiver.recv_frame().await.unwrap();
    assert_eq!(received_kind, kind);
    assert!(
        received == frame,
        "relay must preserve the exact frame bytes"
    );
    received
}

fn received_challenge(frame: &[u8]) -> SessionChallenge {
    SessionChallenge::from_bytes(frame[RHF1_HEADER..].try_into().unwrap()).unwrap()
}

fn signed_body(sdp: &str, signature: [u8; 64]) -> Vec<u8> {
    let mut body = u16::try_from(sdp.len()).unwrap().to_be_bytes().to_vec();
    body.extend_from_slice(&signature);
    body.extend_from_slice(sdp.as_bytes());
    body
}

fn received_sdp(frame: &[u8]) -> (&str, &[u8; 64]) {
    let (length, body) = frame[RHF1_HEADER..].split_at(2);
    let (signature, sdp) = body.split_at(64);
    assert_eq!(
        usize::from(u16::from_be_bytes(length.try_into().unwrap())),
        sdp.len()
    );
    (
        std::str::from_utf8(sdp).unwrap(),
        signature.try_into().unwrap(),
    )
}

async fn pinned_proof_exchange(fixture: &Fixture, tamper_offer: bool) {
    let pin_ca = ca_pin(&fixture.ca_der);
    let mut host = fixture
        .pinned_client(Role::Host, &fixture.ca_der, pin_ca)
        .await
        .unwrap();
    let mut viewer = fixture
        .pinned_client(Role::Viewer, &fixture.ca_der, pin_ca)
        .await
        .unwrap();
    // Each independent client sees its own exact transport readiness on its
    // pinned socket. The four signed frames start only after this control.
    tokio::try_join!(host.wait_paired(), viewer.wait_paired()).unwrap();

    let host_id = Identity::generate_ephemeral().unwrap();
    let viewer_id = Identity::generate_ephemeral().unwrap();
    assert!(host_id.public().public_key != viewer_id.public().public_key);
    let pinned_viewer = pin(&host_id, &viewer_id);
    let pinned_host = pin(&viewer_id, &host_id);
    let scope = fixture.proof_scope();
    let mode = FakeInputTestMode::Enabled;
    let mut host_proof = FakeInputProof::prepare(
        &host_id,
        &pinned_viewer,
        SessionRole::Host,
        scope,
        mode,
        wall(),
    )
    .unwrap();
    let mut viewer_proof = FakeInputProof::prepare(
        &viewer_id,
        &pinned_host,
        SessionRole::Viewer,
        scope,
        mode,
        wall(),
    )
    .unwrap();

    let host_wire = pinned_transfer(
        &mut host,
        &mut viewer,
        Kind::HostChallenge,
        fixture.frame([1, 1], &host_proof.challenge(wall()).unwrap().to_bytes()),
    )
    .await;
    let viewer_wire = pinned_transfer(
        &mut viewer,
        &mut host,
        Kind::ViewerChallenge,
        fixture.frame([2, 2], &viewer_proof.challenge(wall()).unwrap().to_bytes()),
    )
    .await;
    let offer = fixed_sdp(SessionRole::Viewer);
    let mut offer_signature = viewer_proof
        .seal(received_challenge(&host_wire), &offer, wall())
        .unwrap();
    if tamper_offer {
        offer_signature[0] ^= 1;
    }
    let offer_wire = pinned_transfer(
        &mut viewer,
        &mut host,
        Kind::Offer,
        fixture.frame([3, 2], &signed_body(&offer, offer_signature)),
    )
    .await;
    let (received_offer, signature) = received_sdp(&offer_wire);
    let verified = host_proof.verify(received_offer, signature, wall());
    if tamper_offer {
        assert_eq!(verified, Err(ProofError::Rejected));
        assert!(host_proof.approved_relay_pair(wall()).is_none());
        assert!(viewer_proof.approved_relay_pair(wall()).is_none());
        assert!(!fixture.app.gate.lock().unwrap().flow.is_complete());
        host.close().await.unwrap();
        // The structurally valid tampered offer reached the endpoint, which
        // rejected it. No host answer should be emitted or received.
        assert_eq!(viewer.recv_frame().await, Err(SyntheticClientError::Closed));
        return;
    }
    verified.unwrap();

    let answer = fixed_sdp(SessionRole::Host);
    let answer_signature = host_proof
        .seal(received_challenge(&viewer_wire), &answer, wall())
        .unwrap();
    let answer_wire = pinned_transfer(
        &mut host,
        &mut viewer,
        Kind::Answer,
        fixture.frame([4, 1], &signed_body(&answer, answer_signature)),
    )
    .await;
    let (received_answer, signature) = received_sdp(&answer_wire);
    viewer_proof
        .verify(received_answer, signature, wall())
        .unwrap();
    let host_pair = host_proof.approved_relay_pair(wall()).unwrap();
    let viewer_pair = viewer_proof.approved_relay_pair(wall()).unwrap();
    assert_eq!(
        host_pair.local(),
        SocketAddrV4::new(RELAY_LABEL, ANSWER_PORT)
    );
    assert_eq!(
        viewer_pair.local(),
        SocketAddrV4::new(RELAY_LABEL, OFFER_PORT)
    );
    assert_eq!(host_pair.local(), viewer_pair.remote());
    assert_eq!(viewer_pair.local(), host_pair.remote());
    assert!(fixture.app.gate.lock().unwrap().flow.is_complete());
    let _ = viewer.close().await;
    let _ = host.close().await;
}

#[tokio::test]
async fn pinned_clients_prove_the_exact_sctp_pair_wrong_ca_does_not_spend_invite() {
    let fixture = Fixture::start(App::new(Instant::now() + LEASE)).await;
    let wrong_ca = fixture_ca();
    let wrong_der = wrong_ca.der().as_ref();
    assert!(ca_pin(wrong_der) != ca_pin(&fixture.ca_der));
    assert!(matches!(
        fixture
            .pinned_client(Role::Host, wrong_der, ca_pin(&fixture.ca_der))
            .await,
        Err(SyntheticClientError::CaPin)
    ));
    // Matching pin for the wrong root gets as far as the real listener, but
    // TLS fails before the HTTP upgrade can present or spend an invitation.
    assert!(matches!(
        fixture
            .pinned_client(Role::Host, wrong_der, ca_pin(wrong_der))
            .await,
        Err(SyntheticClientError::Tls)
    ));
    assert_eq!(fixture.app.gate.lock().unwrap().used, [false; 2]);
    // Reuse the unchanged invitation against the real pinned root.
    pinned_proof_exchange(&fixture, false).await;
    fixture.finish().await;
}

#[tokio::test]
async fn structurally_valid_tampered_signature_is_denied_by_endpoint() {
    let fixture = Fixture::start(App::new(Instant::now() + LEASE)).await;
    pinned_proof_exchange(&fixture, true).await;
    fixture.finish().await;
}
