//! Test-only binary courier. A dedicated 127.0.0.1 WSS listener relays
//! opaque bytes between two fixture roles; it never interprets a proof,
//! signals acceptance, or participates in the SCTP marker path.

use crate::public_fake_input_wss_wire::MAX_FRAME_BYTES;
use futures_util::{SinkExt, StreamExt};
use hub_identity::session::SessionRole;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName},
    ClientConfig, RootCertStore, ServerConfig,
};
use sha2::{Digest, Sha256};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpSocket, TcpStream},
    task::JoinHandle,
    time::{timeout_at, Instant},
};
use tokio_rustls::{client, server, TlsAcceptor, TlsConnector};
use tokio_tungstenite::{
    accept_hdr_async_with_config, client_async_with_config,
    tungstenite::{
        handshake::server::{Request, Response},
        http,
        protocol::WebSocketConfig,
        Error, Message,
    },
    WebSocketStream,
};

const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
const HOST_PATH: &str = "/__remotehub-fixture/fake-input/host";
const VIEWER_PATH: &str = "/__remotehub-fixture/fake-input/viewer";
const LIFETIME: Duration = Duration::from_secs(20);
const IO_WAIT: Duration = Duration::from_secs(3);
const MAX_RELAYED: usize = 8;
const MAX_CA_DER_BYTES: usize = 4096;
const CLOSED: &str = "FAKE_INPUT_FIXTURE_WSS_CLOSED";
const IO: &str = "FAKE_INPUT_FIXTURE_WSS_IO";
const DEADLINE: &str = "FAKE_INPUT_FIXTURE_WSS_DEADLINE";
const INVALID: &str = "FAKE_INPUT_FIXTURE_WSS_INVALID_FRAME";
const TLS: &str = "FAKE_INPUT_FIXTURE_WSS_TLS";

type Result<T> = std::result::Result<T, &'static str>;
type RelaySocket = WebSocketStream<server::TlsStream<TcpStream>>;

#[cfg(any(test, feature = "v2-public-fake-input-loopback-check"))]
pub(super) struct FixtureWss {
    pub(super) host: FixtureSocket,
    pub(super) viewer: FixtureSocket,
    server: FixtureWssServer,
}

/// Loopback test relay independent of the role processes. Only public CA
/// DER and the allocated loopback port may leave this fixture owner.
pub(super) struct FixtureWssServer {
    ca_der: Vec<u8>,
    port: u16,
    task: RelayTask,
    deadline: Instant,
}

pub(super) struct FixtureSocket {
    socket: WebSocketStream<client::TlsStream<TcpStream>>,
    deadline: Instant,
    closed: bool,
}

#[cfg(any(test, feature = "v2-public-fake-input-loopback-check"))]
impl FixtureWss {
    pub(super) async fn start() -> Result<Self> {
        let server = FixtureWssServer::start().await?;
        // Establish the host before dialing viewer: the fixture binds each
        // role to a distinct URI in a fixed two-connection order.
        let host = connect_role(server.port(), server.ca_der(), SessionRole::Host).await?;
        let viewer = connect_role(server.port(), server.ca_der(), SessionRole::Viewer).await?;
        Ok(Self {
            host,
            viewer,
            server,
        })
    }

    pub(super) fn ca_sha256(&self) -> [u8; 32] {
        self.server.ca_sha256()
    }

    pub(super) fn port(&self) -> u16 {
        self.server.port()
    }

    pub(super) async fn close(mut self) -> Result<()> {
        let notify = self.host.disconnect().await;
        let until = io_deadline(self.server.deadline);
        let joined = self.server.wait_until(until).await;
        notify?;
        joined
    }
}

impl FixtureWssServer {
    pub(super) async fn start() -> Result<Self> {
        let deadline = Instant::now() + LIFETIME;
        let (tls, ca_der) = tls_material()?;
        let listener = TcpListener::bind((LOOPBACK, 0)).await.map_err(|_| IO)?;
        let SocketAddr::V4(endpoint) = listener.local_addr().map_err(|_| IO)? else {
            return Err(IO);
        };
        let task = RelayTask(Some(tokio::spawn(serve(listener, tls, endpoint, deadline))));
        Ok(Self {
            ca_der,
            port: endpoint.port(),
            task,
            deadline,
        })
    }

    pub(super) fn port(&self) -> u16 {
        self.port
    }

    pub(super) fn ca_der(&self) -> &[u8] {
        &self.ca_der
    }

    pub(super) fn ca_sha256(&self) -> [u8; 32] {
        Sha256::digest(self.ca_der()).into()
    }

    /// Wait for the two-role relay to finish, within its original hard
    /// lifetime. Canceling the wait or dropping the owner aborts the task.
    #[cfg(any(
        test,
        all(target_os = "linux", feature = "v2-public-fake-input-process-check")
    ))]
    pub(super) async fn wait(self) -> Result<()> {
        let deadline = self.deadline;
        self.wait_until(deadline).await
    }

    async fn wait_until(mut self, deadline: Instant) -> Result<()> {
        let task = self.task.0.as_mut().ok_or(CLOSED)?;
        let joined = timeout_at(deadline, task)
            .await
            .map_err(|_| DEADLINE)?
            .map_err(|_| IO)?;
        self.task.0.take();
        joined
    }
}

/// Dial exactly the already allocated 127.0.0.1 fixture port. The role
/// process must have received the single test CA through a trusted parent.
pub(super) async fn connect_role(
    port: u16,
    ca_der: &[u8],
    role: SessionRole,
) -> Result<FixtureSocket> {
    let endpoint = SocketAddrV4::new(LOOPBACK, port);
    let trust = client_trust(ca_der)?;
    let path = match role {
        SessionRole::Host => HOST_PATH,
        SessionRole::Viewer => VIEWER_PATH,
    };
    connect(endpoint, trust, path, Instant::now() + LIFETIME).await
}

struct RelayTask(Option<JoinHandle<Result<()>>>);

impl Drop for RelayTask {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

impl FixtureSocket {
    pub(super) async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        if self.closed {
            return Err(CLOSED);
        }
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(INVALID);
        }
        timeout_at(
            io_deadline(self.deadline),
            self.socket.send(Message::Binary(bytes.to_vec().into())),
        )
        .await
        .map_err(|_| DEADLINE)?
        .map_err(map_error)
    }

    pub(super) async fn recv(&mut self) -> Result<Vec<u8>> {
        if self.closed {
            return Err(CLOSED);
        }
        match timeout_at(io_deadline(self.deadline), self.socket.next())
            .await
            .map_err(|_| DEADLINE)?
        {
            Some(Ok(Message::Binary(bytes))) if bytes.len() <= MAX_FRAME_BYTES => {
                Ok(bytes.to_vec())
            }
            Some(Ok(Message::Close(_))) | None => {
                self.closed = true;
                Err(CLOSED)
            }
            Some(Err(error)) => {
                self.closed = true;
                Err(map_error(error))
            }
            _ => Err(INVALID),
        }
    }

    pub(super) async fn disconnect(&mut self) -> Result<()> {
        if std::mem::replace(&mut self.closed, true) {
            return Ok(());
        }
        match timeout_at(io_deadline(self.deadline), self.socket.close(None))
            .await
            .map_err(|_| DEADLINE)?
        {
            Ok(()) | Err(Error::ConnectionClosed | Error::AlreadyClosed) => Ok(()),
            Err(error) => Err(map_error(error)),
        }
    }
}

fn io_deadline(deadline: Instant) -> Instant {
    deadline.min(Instant::now() + IO_WAIT)
}

fn map_error(error: Error) -> &'static str {
    match error {
        Error::ConnectionClosed | Error::AlreadyClosed => CLOSED,
        _ => IO,
    }
}

fn websocket_limits() -> WebSocketConfig {
    WebSocketConfig::default()
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_FRAME_BYTES * 2)
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
}

fn tls_material() -> Result<(TlsAcceptor, Vec<u8>)> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut authority = CertificateParams::default();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    authority.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = CertifiedIssuer::self_signed(authority, KeyPair::generate().map_err(|_| TLS)?)
        .map_err(|_| TLS)?;
    let ca_der = ca.der().as_ref().to_vec();
    let key = KeyPair::generate().map_err(|_| TLS)?;
    let mut leaf = CertificateParams::new(vec![LOOPBACK.to_string()]).map_err(|_| TLS)?;
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf.signed_by(&key, &ca).map_err(|_| TLS)?;
    let tls = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| TLS)?
        .with_no_client_auth()
        .with_single_cert(
            vec![leaf.der().clone()],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .map_err(|_| TLS)?;
    Ok((TlsAcceptor::from(Arc::new(tls)), ca_der))
}

fn client_trust(ca_der: &[u8]) -> Result<Arc<ClientConfig>> {
    if ca_der.is_empty() || ca_der.len() > MAX_CA_DER_BYTES {
        return Err(TLS);
    }
    let mut roots = RootCertStore::empty();
    roots.add(CertificateDer::from(ca_der)).map_err(|_| TLS)?;
    let trust =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| TLS)?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(Arc::new(trust))
}

async fn connect(
    endpoint: SocketAddrV4,
    trust: Arc<ClientConfig>,
    path: &'static str,
    deadline: Instant,
) -> Result<FixtureSocket> {
    if *endpoint.ip() != LOOPBACK || endpoint.port() == 0 {
        return Err(IO);
    }
    timeout_at(io_deadline(deadline), async {
        let socket = TcpSocket::new_v4().map_err(|_| IO)?;
        socket.bind((LOOPBACK, 0).into()).map_err(|_| IO)?;
        let tcp = socket.connect(endpoint.into()).await.map_err(|_| IO)?;
        if tcp.local_addr().map_err(|_| IO)?.ip() != LOOPBACK
            || tcp.peer_addr().map_err(|_| IO)? != SocketAddr::V4(endpoint)
        {
            return Err(IO);
        }
        let stream = TlsConnector::from(trust)
            .connect(ServerName::IpAddress(LOOPBACK.into()), tcp)
            .await
            .map_err(|_| TLS)?;
        // No resolver, redirect, WS fallback, or system trust store.
        let (socket, _) = client_async_with_config(
            format!("wss://{endpoint}{path}"),
            stream,
            Some(websocket_limits()),
        )
        .await
        .map_err(|_| IO)?;
        Ok(FixtureSocket {
            socket,
            deadline,
            closed: false,
        })
    })
    .await
    .map_err(|_| DEADLINE)?
}

async fn accept(
    listener: &TcpListener,
    tls: &TlsAcceptor,
    endpoint: SocketAddrV4,
    path: &'static str,
    deadline: Instant,
) -> Result<RelaySocket> {
    // Child-process startup can take longer than a socket operation, but
    // cannot extend the server's original hard lifetime.
    let (tcp, source) = timeout_at(deadline, listener.accept())
        .await
        .map_err(|_| DEADLINE)?
        .map_err(|_| IO)?;
    timeout_at(io_deadline(deadline), async {
        if source.ip() != LOOPBACK
            || tcp.peer_addr().map_err(|_| IO)? != source
            || tcp.local_addr().map_err(|_| IO)? != SocketAddr::V4(endpoint)
        {
            return Err(IO);
        }
        let stream = tls.accept(tcp).await.map_err(|_| TLS)?;
        accept_hdr_async_with_config(
            stream,
            move |request: &Request, response: Response| {
                if request.headers().contains_key(http::header::ORIGIN) {
                    return Err(http::Response::builder()
                        .status(http::StatusCode::FORBIDDEN)
                        .body(None)
                        .expect("static rejection response"));
                }
                if request.uri().path_and_query().map(|p| p.as_str()) == Some(path) {
                    Ok(response)
                } else {
                    Err(http::Response::builder()
                        .status(http::StatusCode::NOT_FOUND)
                        .body(None)
                        .expect("static rejection response"))
                }
            },
            Some(websocket_limits()),
        )
        .await
        .map_err(|_| IO)
    })
    .await
    .map_err(|_| DEADLINE)?
}

async fn serve(
    listener: TcpListener,
    tls: TlsAcceptor,
    endpoint: SocketAddrV4,
    deadline: Instant,
) -> Result<()> {
    let mut host = accept(&listener, &tls, endpoint, HOST_PATH, deadline).await?;
    let mut viewer = accept(&listener, &tls, endpoint, VIEWER_PATH, deadline).await?;
    drop(listener);
    timeout_at(deadline, async {
        let mut frames = 0;
        loop {
            let (from_host, frame) = tokio::select! {
                frame = host.next() => (true, frame),
                frame = viewer.next() => (false, frame),
            };
            let other = if from_host { &mut viewer } else { &mut host };
            match frame {
                Some(Ok(Message::Binary(bytes))) if bytes.len() <= MAX_FRAME_BYTES => {
                    frames += 1;
                    if frames > MAX_RELAYED {
                        return Err(INVALID);
                    }
                    timeout_at(io_deadline(deadline), other.send(Message::Binary(bytes)))
                        .await
                        .map_err(|_| DEADLINE)?
                        .map_err(map_error)?;
                }
                Some(Ok(Message::Close(_))) | Some(Err(Error::ConnectionClosed)) | None => {
                    let _ = timeout_at(io_deadline(deadline), other.close(None)).await;
                    return Ok(());
                }
                Some(Err(error)) => return Err(map_error(error)),
                _ => return Err(INVALID),
            }
        }
    })
    .await
    .map_err(|_| DEADLINE)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn exchanges_only_opaque_loopback_frames_and_reports_peer_close() {
        let mut fixture = FixtureWss::start().await.unwrap();
        assert_ne!(fixture.ca_sha256(), [0; 32]);
        assert_ne!(fixture.port(), 0);
        let frame = vec![0x81; MAX_FRAME_BYTES];
        fixture.host.send(&frame).await.unwrap();
        assert_eq!(fixture.viewer.recv().await.unwrap(), frame);
        fixture.viewer.send(&[0, 255, 7]).await.unwrap();
        assert_eq!(fixture.host.recv().await.unwrap(), [0, 255, 7]);
        assert_eq!(
            fixture.host.send(&[0; MAX_FRAME_BYTES + 1]).await,
            Err(INVALID)
        );
        fixture.viewer.disconnect().await.unwrap();
        assert_eq!(fixture.host.recv().await, Err(CLOSED));
        fixture.close().await.unwrap();
    }
    #[tokio::test]
    async fn separate_role_dials_use_the_exported_root_and_reap_relay() {
        let server = FixtureWssServer::start().await.unwrap();
        let port = server.port();
        let ca = server.ca_der().to_vec();
        // Only inert public data crosses the relay/role ownership boundary.
        assert_eq!(<[u8; 32]>::from(Sha256::digest(&ca)), server.ca_sha256());
        let mut host = connect_role(port, &ca, SessionRole::Host).await.unwrap();
        let mut viewer = connect_role(port, &ca, SessionRole::Viewer).await.unwrap();
        viewer.send(b"opaque").await.unwrap();
        assert_eq!(host.recv().await.unwrap(), b"opaque");
        host.disconnect().await.unwrap();
        assert_eq!(viewer.recv().await, Err(CLOSED));
        server.wait().await.unwrap();
    }

    #[tokio::test]
    async fn independent_fixture_roots_are_not_interchangeable() {
        let (tls, server_ca) = tls_material().unwrap();
        let (_, wrong_ca) = tls_material().unwrap();
        assert_ne!(server_ca, wrong_ca);
        let wrong_root = client_trust(&wrong_ca).unwrap();
        let listener = TcpListener::bind((LOOPBACK, 0)).await.unwrap();
        let SocketAddr::V4(endpoint) = listener.local_addr().unwrap() else {
            panic!("fixture binds IPv4 only");
        };
        let deadline = Instant::now() + IO_WAIT;
        let (server, client) = tokio::join!(
            accept(&listener, &tls, endpoint, HOST_PATH, deadline),
            connect(endpoint, wrong_root, HOST_PATH, deadline),
        );
        assert!(matches!(server, Err(TLS)));
        assert!(matches!(client, Err(TLS)));
    }
}
