use crate::wire::{
    self, ClientFrame, Flow, Role, ServerFrame, SignalKind, MAX_WIRE, PATH, ROOM_LIFETIME, VERSION,
};
use futures_util::{SinkExt, StreamExt};
use rustls::{
    pki_types::{CertificateDer, ServerName},
    ClientConfig, RootCertStore,
};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpSocket, TcpStream},
    time::{timeout, timeout_at, Instant},
};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::{
    client_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{header, HeaderValue, StatusCode},
        protocol::WebSocketConfig,
        Error as WsError, Message,
    },
    WebSocketStream,
};
use uuid::Uuid;
use zeroize::Zeroizing;

const IO_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_POLL_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_TRUST_ANCHOR_DER: usize = 64 * 1024;

trait RelayIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> RelayIo for T {}

/// Trust configuration provisioned independently of the relay, not obtained
/// from the endpoint being connected to. It does not authorize a remote peer.
pub struct LoopbackTlsTrust(Arc<ClientConfig>);

impl LoopbackTlsTrust {
    /// Trust exactly this DER CA for this connector, without using system
    /// roots or altering system trust. Rustls checks the chain and IP SAN.
    pub fn from_ca_der(ca_der: &[u8]) -> Result<Self, ClientError> {
        if ca_der.is_empty() || ca_der.len() > MAX_TRUST_ANCHOR_DER {
            return Err(ClientError::TlsTrust);
        }
        let mut roots = RootCertStore::empty();
        roots
            .add(CertificateDer::from(ca_der))
            .map_err(|_| ClientError::TlsTrust)?;
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .map_err(|_| ClientError::TlsTrust)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        Ok(Self(Arc::new(config)))
    }
}

/// Native-only 256-bit bearer. Never log, serialize, or forward to JavaScript.
pub struct RelayToken(Zeroizing<String>);

impl RelayToken {
    pub fn new(value: String) -> Result<Self, ClientError> {
        let value = Zeroizing::new(value);
        if !wire::valid_token(&value) {
            return Err(ClientError::Token);
        }
        Ok(Self(value))
    }
}

/// Full random room identifier, not a short presentation or identity code.
pub struct RoomId(String);

impl RoomId {
    pub fn random() -> Self {
        Self(Uuid::new_v4().simple().to_string())
    }

    pub fn new(value: String) -> Result<Self, ClientError> {
        if !wire::valid_room(&value) {
            return Err(ClientError::Room);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    #[error("V2 relay endpoint must be a literal loopback socket")]
    Endpoint,
    #[error(
        "V2 private LAN relay requires literal RFC1918 source and relay IPv4 addresses and a port"
    )]
    PrivateLanEndpoint,
    #[error("unable to bind or verify the private LAN relay source socket")]
    PrivateLanBind,
    #[error("invalid relay token")]
    Token,
    #[error("invalid relay room")]
    Room,
    #[error("invalid loopback relay trust configuration")]
    TlsTrust,
    #[error("loopback relay TLS handshake or certificate verification failed")]
    TlsHandshake,
    #[error("relay rejected the WebSocket authorization")]
    Unauthorized,
    #[error("relay rejected the signaling session")]
    Rejected,
    #[error("signaling connection or write failed")]
    Transport,
    #[error("signaling operation timed out")]
    Timeout,
    #[error("signaling connection is closed")]
    Closed,
    #[error("signaling frame or order was rejected")]
    Protocol,
    #[error("signaling payload is empty, invalid or oversized")]
    Payload,
}

/// Explicit relay route only; this is separate from the eventual media peer.
/// A relay may deliberately be co-located on the bound source address.
#[derive(Clone, Copy)]
pub struct PrivateLanEndpoint {
    source: Ipv4Addr,
    relay: SocketAddrV4,
}

impl PrivateLanEndpoint {
    pub fn new(source: Ipv4Addr, relay: Ipv4Addr, port: u16) -> Result<Self, ClientError> {
        let private_host = |ip: Ipv4Addr| ip.is_private() && !matches!(ip.octets()[3], 0 | 255);
        if !private_host(source) || !private_host(relay) || port == 0 {
            return Err(ClientError::PrivateLanEndpoint);
        }
        Ok(Self {
            source,
            relay: SocketAddrV4::new(relay, port),
        })
    }

    pub fn source(self) -> Ipv4Addr {
        self.source
    }

    pub fn relay(self) -> SocketAddrV4 {
        self.relay
    }

    fn matches_socket(self, local: SocketAddr, peer: SocketAddr) -> bool {
        matches!(local, SocketAddr::V4(v4) if *v4.ip() == self.source && v4.port() != 0)
            && peer == SocketAddr::V4(self.relay)
    }

    async fn dial(self) -> Result<TcpStream, ClientError> {
        let socket = TcpSocket::new_v4().map_err(|_| ClientError::PrivateLanBind)?;
        socket
            .bind(SocketAddr::V4(SocketAddrV4::new(self.source, 0)))
            .map_err(|_| ClientError::PrivateLanBind)?;
        let tcp = socket
            .connect(SocketAddr::V4(self.relay))
            .await
            .map_err(|_| ClientError::Transport)?;
        let local = tcp.local_addr().map_err(|_| ClientError::PrivateLanBind)?;
        let peer = tcp.peer_addr().map_err(|_| ClientError::PrivateLanBind)?;
        if !self.matches_socket(local, peer) {
            return Err(ClientError::PrivateLanBind);
        }
        Ok(tcp)
    }
}

enum Route<'a> {
    Loopback(SocketAddr, Option<&'a LoopbackTlsTrust>),
    PrivateLan(PrivateLanEndpoint, &'a LoopbackTlsTrust),
}

impl Route<'_> {
    fn server(&self) -> Result<SocketAddr, ClientError> {
        match *self {
            Self::Loopback(address, _) if address.ip().is_loopback() && address.port() != 0 => {
                Ok(address)
            }
            Self::Loopback(..) => Err(ClientError::Endpoint),
            Self::PrivateLan(endpoint, _) => Ok(SocketAddr::V4(endpoint.relay)),
        }
    }

    fn trust(&self) -> Option<&LoopbackTlsTrust> {
        match *self {
            Self::Loopback(_, trust) => trust,
            Self::PrivateLan(_, trust) => Some(trust),
        }
    }

    async fn dial(&self, server: SocketAddr) -> Result<TcpStream, ClientError> {
        match *self {
            Self::Loopback(..) => TcpStream::connect(server)
                .await
                .map_err(|_| ClientError::Transport),
            Self::PrivateLan(endpoint, _) => endpoint.dial().await,
        }
    }
}

/// Deliberately selected private-LAN route, always with verified TLS. This
/// connection authenticates the relay only, not a device or media session.
pub struct PrivateLanRelay(LoopbackRelay);

/// Connect only to the specified numeric relay, from the specified source
/// address. Verify its pinned CA and exact IPv4 SAN before sending the token.
/// Does not resolve names, use a proxy or redirects, or fall back to WS.
pub async fn connect_private_lan_wss(
    endpoint: PrivateLanEndpoint,
    token: &RelayToken,
    room: RoomId,
    role: Role,
    trust: &LoopbackTlsTrust,
) -> Result<PrivateLanRelay, ClientError> {
    LoopbackRelay::connect_with_route(Route::PrivateLan(endpoint, trust), token, room, role)
        .await
        .map(PrivateLanRelay)
}

impl PrivateLanRelay {
    pub async fn send_signal(&mut self, kind: SignalKind, data: String) -> Result<(), ClientError> {
        self.0.send_signal(kind, data).await
    }

    pub async fn next_event(&mut self, wait: Duration) -> Result<RelayEvent, ClientError> {
        self.0.next_event(wait).await
    }

    pub async fn leave(self) -> Result<(), ClientError> {
        self.0.leave().await
    }
}

/// Not serializable or Debug so raw signaling cannot accidentally reach logs
/// or an IPC response. The product must separately authenticate any SDP/ICE.
pub enum RelayEvent {
    Waiting,
    Paired,
    Signal { kind: SignalKind, data: String },
    PeerLeft,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum JoinPhase {
    Hello,
    Waiting,
    Paired,
}

pub struct LoopbackRelay {
    socket: Option<WebSocketStream<Box<dyn RelayIo>>>,
    deadline: Instant,
    role: Role,
    phase: JoinPhase,
    flow: Flow,
    sent: u64,
    received: u64,
}

impl LoopbackRelay {
    /// No URL input, DNS, TLS downgrade, or redirect following. Plain WS is
    /// supported only on 127.0.0.0/8 or ::1 for this synthetic lab seam.
    pub async fn connect(
        address: SocketAddr,
        token: &RelayToken,
        room: RoomId,
        role: Role,
    ) -> Result<Self, ClientError> {
        Self::connect_with_route(Route::Loopback(address, None), token, room, role).await
    }

    /// Use WSS with an independently provisioned trust anchor. The HTTP
    /// Authorization header is built and sent only after verified TLS.
    /// Connection or trust failure is terminal; there is no plain fallback.
    pub async fn connect_wss(
        address: SocketAddr,
        token: &RelayToken,
        room: RoomId,
        role: Role,
        trust: &LoopbackTlsTrust,
    ) -> Result<Self, ClientError> {
        Self::connect_with_route(Route::Loopback(address, Some(trust)), token, room, role).await
    }

    async fn connect_with_route(
        route: Route<'_>,
        token: &RelayToken,
        room: RoomId,
        role: Role,
    ) -> Result<Self, ClientError> {
        let address = route.server()?;
        let config = WebSocketConfig::default()
            .write_buffer_size(0)
            .max_write_buffer_size(MAX_WIRE * 2)
            .max_frame_size(Some(MAX_WIRE))
            .max_message_size(Some(MAX_WIRE));
        let deadline = Instant::now() + ROOM_LIFETIME;
        let socket = timeout(IO_TIMEOUT, async {
            // SocketAddr makes the TCP target explicit independently of the HTTP Host.
            let tcp = route.dial(address).await?;
            let (scheme, stream): (&str, Box<dyn RelayIo>) = match route.trust() {
                Some(trust) => {
                    let name = ServerName::IpAddress(address.ip().into());
                    let tls = TlsConnector::from(trust.0.clone())
                        .connect(name, tcp)
                        .await
                        .map_err(|_| ClientError::TlsHandshake)?;
                    ("wss", Box::new(tls))
                }
                None => ("ws", Box::new(tcp)),
            };
            let mut request = format!("{scheme}://{address}{PATH}")
                .into_client_request()
                .map_err(|_| ClientError::Endpoint)?;
            let auth = Zeroizing::new(format!("Bearer {}", token.0.as_str()));
            let mut auth_header = HeaderValue::from_str(&auth).map_err(|_| ClientError::Token)?;
            auth_header.set_sensitive(true);
            request
                .headers_mut()
                .insert(header::AUTHORIZATION, auth_header);
            let (socket, _) = client_async_with_config(request, stream, Some(config))
                .await
                .map_err(|error| match error {
                    WsError::Http(response)
                        if matches!(
                            response.status(),
                            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
                        ) =>
                    {
                        ClientError::Unauthorized
                    }
                    _ => ClientError::Transport,
                })?;
            Ok::<_, ClientError>(socket)
        })
        .await
        .map_err(|_| ClientError::Timeout)??;
        let mut relay = Self {
            socket: Some(socket),
            deadline,
            role,
            phase: JoinPhase::Hello,
            flow: Flow::default(),
            sent: 0,
            received: 0,
        };
        relay
            .write(ClientFrame::Hello {
                version: VERSION,
                room: room.0,
                role,
            })
            .await?;
        Ok(relay)
    }

    /// Only sends signaling. Caller must still obtain native approval and
    /// authenticate the intended peer/session before any media or input.
    pub async fn send_signal(&mut self, kind: SignalKind, data: String) -> Result<(), ClientError> {
        if !kind.valid_payload(&data) {
            return Err(ClientError::Payload);
        }
        if self.socket.is_none() {
            return Err(ClientError::Closed);
        }
        if self.phase != JoinPhase::Paired || !kind.allows_sender(self.role) {
            return Err(ClientError::Protocol);
        }
        let next_flow = self.flow.after(kind).ok_or(ClientError::Protocol)?;
        let seq = self.sent.checked_add(1).ok_or(ClientError::Protocol)?;
        self.write(ClientFrame::Signal {
            seq,
            signal: kind,
            data,
        })
        .await?;
        self.sent = seq;
        self.flow = next_flow;
        Ok(())
    }

    pub async fn next_event(&mut self, wait: Duration) -> Result<RelayEvent, ClientError> {
        if self.socket.is_none() || Instant::now() >= self.deadline {
            self.socket.take();
            return Err(ClientError::Closed);
        }
        let deadline = self
            .deadline
            .min(Instant::now() + wait.min(MAX_POLL_TIMEOUT));
        let outcome = timeout_at(deadline, self.read()).await;
        match outcome {
            Err(_) if Instant::now() < self.deadline => Err(ClientError::Timeout),
            Err(_) => {
                self.socket.take();
                Err(ClientError::Closed)
            }
            Ok(Ok(RelayEvent::PeerLeft)) => {
                self.socket.take();
                Ok(RelayEvent::PeerLeft)
            }
            Ok(Err(error)) => {
                self.socket.take();
                Err(error)
            }
            Ok(Ok(event)) => Ok(event),
        }
    }

    /// Leaving consumes this session. If the notification fails, dropping the
    /// socket still releases the server room and prevents later sends.
    pub async fn leave(mut self) -> Result<(), ClientError> {
        self.write(ClientFrame::Leave {}).await
    }

    async fn write(&mut self, frame: ClientFrame) -> Result<(), ClientError> {
        if Instant::now() >= self.deadline {
            self.socket.take();
            return Err(ClientError::Closed);
        }
        let text = wire::encode(&frame).map_err(|_| ClientError::Payload)?;
        // Abort rather than reuse an indeterminate sequence if the caller
        // cancels a write after bytes have been queued into the socket.
        let mut socket = self.socket.take().ok_or(ClientError::Closed)?;
        let deadline = self.deadline.min(Instant::now() + IO_TIMEOUT);
        timeout_at(deadline, socket.send(Message::Text(text.into())))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::Transport)?;
        self.socket = Some(socket);
        Ok(())
    }

    async fn read(&mut self) -> Result<RelayEvent, ClientError> {
        loop {
            let socket = self.socket.as_mut().ok_or(ClientError::Closed)?;
            match socket.next().await {
                Some(Ok(Message::Ping(_))) => {
                    // Tungstenite has queued the pong; flush within next_event's deadline.
                    socket.flush().await.map_err(|_| ClientError::Transport)?;
                }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Text(text))) => {
                    let frame = wire::decode(&text).map_err(|_| ClientError::Protocol)?;
                    return self.accept(frame);
                }
                Some(Ok(Message::Close(_))) | None => return Err(ClientError::Closed),
                Some(Err(_)) => return Err(ClientError::Transport),
                Some(Ok(_)) => return Err(ClientError::Protocol),
            }
        }
    }

    fn accept(&mut self, frame: ServerFrame) -> Result<RelayEvent, ClientError> {
        match frame {
            ServerFrame::Waiting {}
                if self.role == Role::Host && self.phase == JoinPhase::Hello =>
            {
                self.phase = JoinPhase::Waiting;
                Ok(RelayEvent::Waiting)
            }
            ServerFrame::Paired {}
                if matches!(
                    (self.role, self.phase),
                    (Role::Host, JoinPhase::Waiting) | (Role::Viewer, JoinPhase::Hello)
                ) =>
            {
                self.phase = JoinPhase::Paired;
                Ok(RelayEvent::Paired)
            }
            ServerFrame::Signal {
                from,
                seq,
                signal,
                data,
            } if self.phase == JoinPhase::Paired
                && from == self.role.peer()
                && signal.allows_sender(from)
                && self.received.checked_add(1) == Some(seq)
                && signal.valid_payload(&data) =>
            {
                self.flow = self.flow.after(signal).ok_or(ClientError::Protocol)?;
                self.received = seq;
                Ok(RelayEvent::Signal { kind: signal, data })
            }
            ServerFrame::PeerLeft {} => Ok(RelayEvent::PeerLeft),
            // Discard arbitrary text from the relay, including error-code payloads.
            ServerFrame::Error { .. } => Err(ClientError::Rejected),
            _ => Err(ClientError::Protocol),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_routes_only_accept_literal_rfc1918_hosts_and_nonzero_ports() {
        let src = Ipv4Addr::new(192, 168, 40, 11);
        let dst = Ipv4Addr::new(192, 168, 40, 12);
        assert!(PrivateLanEndpoint::new(src, dst, 48620).is_ok());
        assert!(PrivateLanEndpoint::new(src, src, 48620).is_ok()); // Deliberate co-location.
        assert!(matches!(
            PrivateLanEndpoint::new(src, dst, 0),
            Err(ClientError::PrivateLanEndpoint)
        ));
        for addr in [
            Ipv4Addr::UNSPECIFIED,
            Ipv4Addr::LOCALHOST,
            Ipv4Addr::BROADCAST,
            Ipv4Addr::new(169, 254, 3, 4),
            Ipv4Addr::new(100, 64, 3, 4),
            Ipv4Addr::new(192, 0, 2, 2),
            Ipv4Addr::new(192, 168, 40, 0),
            Ipv4Addr::new(192, 168, 40, 255),
        ] {
            assert!(matches!(
                PrivateLanEndpoint::new(src, addr, 48620),
                Err(ClientError::PrivateLanEndpoint)
            ));
            assert!(matches!(
                PrivateLanEndpoint::new(addr, dst, 48620),
                Err(ClientError::PrivateLanEndpoint)
            ));
        }
    }

    #[test]
    fn connected_private_route_must_preserve_the_selected_source_and_relay_exactly() {
        let addr = |value: &str| value.parse::<SocketAddr>().unwrap();
        let src = "10.1.2.3".parse::<Ipv4Addr>().unwrap();
        let dst = "10.1.2.4".parse::<Ipv4Addr>().unwrap();
        let route = PrivateLanEndpoint::new(src, dst, 48620).unwrap();
        assert_eq!(route.source(), src);
        assert_eq!(route.relay(), SocketAddrV4::new(dst, 48620));
        assert!(route.matches_socket(addr("10.1.2.3:40000"), addr("10.1.2.4:48620")));
        for (local, peer) in [
            ("10.1.2.8:40000", "10.1.2.4:48620"),
            ("10.1.2.3:0", "10.1.2.4:48620"),
            ("10.1.2.3:40000", "10.1.2.5:48620"),
            ("10.1.2.3:40000", "10.1.2.4:48621"),
            ("[::ffff:10.1.2.3]:40000", "10.1.2.4:48620"),
        ] {
            assert!(!route.matches_socket(addr(local), addr(peer)));
        }
        let colocated = PrivateLanEndpoint::new(src, src, 48620).unwrap();
        assert!(colocated.matches_socket(addr("10.1.2.3:40000"), addr("10.1.2.3:48620")));
        assert!(matches!(
            Route::Loopback(addr("10.1.2.4:48620"), None).server(),
            Err(ClientError::Endpoint)
        ));
    }
}
