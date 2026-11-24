//! Test-only binary courier. A dedicated 127.0.0.1 WSS listener relays
//! opaque bytes between two fixture roles; it never interprets a proof,
//! signals acceptance, or participates in the SCTP marker path.

use crate::public_fake_input_wss_wire::MAX_FRAME_BYTES;
use futures_util::{SinkExt, StreamExt};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use rustls::{
    pki_types::{PrivatePkcs8KeyDer, ServerName},
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
const CLOSED: &str = "FAKE_INPUT_FIXTURE_WSS_CLOSED";
const IO: &str = "FAKE_INPUT_FIXTURE_WSS_IO";
const DEADLINE: &str = "FAKE_INPUT_FIXTURE_WSS_DEADLINE";
const INVALID: &str = "FAKE_INPUT_FIXTURE_WSS_INVALID_FRAME";
const TLS: &str = "FAKE_INPUT_FIXTURE_WSS_TLS";

type Result<T> = std::result::Result<T, &'static str>;
type RelaySocket = WebSocketStream<server::TlsStream<TcpStream>>;

pub(super) struct FixtureWss {
    pub(super) host: FixtureSocket,
    pub(super) viewer: FixtureSocket,
    ca_sha256: [u8; 32],
    port: u16,
    task: RelayTask,
    deadline: Instant,
}

pub(super) struct FixtureSocket {
    socket: WebSocketStream<client::TlsStream<TcpStream>>,
    deadline: Instant,
    closed: bool,
}

impl FixtureWss {
    pub(super) async fn start() -> Result<Self> {
        let deadline = Instant::now() + LIFETIME;
        let (trust, tls, ca_sha256) = tls_material()?;
        let listener = TcpListener::bind((LOOPBACK, 0)).await.map_err(|_| IO)?;
        let SocketAddr::V4(endpoint) = listener.local_addr().map_err(|_| IO)? else {
            return Err(IO);
        };
        let task = RelayTask(Some(tokio::spawn(serve(listener, tls, endpoint, deadline))));
        // The two clients have exact, distinct, fixture-only paths. Complete
        // the host handshake before dialing viewer to keep the role order known.
        let host = connect(endpoint, trust.clone(), HOST_PATH, deadline).await?;
        let viewer = connect(endpoint, trust, VIEWER_PATH, deadline).await?;
        Ok(Self {
            host,
            viewer,
            ca_sha256,
            port: endpoint.port(),
            task,
            deadline,
        })
    }

    pub(super) fn ca_sha256(&self) -> [u8; 32] {
        self.ca_sha256
    }

    pub(super) fn port(&self) -> u16 {
        self.port
    }

    pub(super) async fn close(mut self) -> Result<()> {
        let notify = self.host.disconnect().await;
        let task = self.task.0.as_mut().ok_or(CLOSED)?;
        let joined = timeout_at(io_deadline(self.deadline), task)
            .await
            .map_err(|_| DEADLINE)?
            .map_err(|_| IO)?;
        self.task.0.take();
        notify?;
        joined
    }
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

fn tls_material() -> Result<(Arc<ClientConfig>, TlsAcceptor, [u8; 32])> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut authority = CertificateParams::default();
    authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    authority.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca = CertifiedIssuer::self_signed(authority, KeyPair::generate().map_err(|_| TLS)?)
        .map_err(|_| TLS)?;
    let mut roots = RootCertStore::empty();
    roots.add(ca.der().clone()).map_err(|_| TLS)?;
    let ca_sha256 = Sha256::digest(ca.der().as_ref()).into();
    let trust = ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|_| TLS)?
        .with_root_certificates(roots)
        .with_no_client_auth();
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
    Ok((Arc::new(trust), TlsAcceptor::from(Arc::new(tls)), ca_sha256))
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
    timeout_at(io_deadline(deadline), async {
        let (tcp, source) = listener.accept().await.map_err(|_| IO)?;
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
    async fn independent_fixture_roots_are_not_interchangeable() {
        let (_, tls, server_hash) = tls_material().unwrap();
        let (wrong_root, _, wrong_hash) = tls_material().unwrap();
        assert_ne!(server_hash, wrong_hash);
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
