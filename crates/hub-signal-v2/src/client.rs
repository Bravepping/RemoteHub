use crate::wire::{
    self, ClientFrame, Flow, MAX_WIRE, PATH, PUBLIC_VIDEO_PATH, ROOM_LIFETIME, Role, ServerFrame,
    SignalKind, VERSION,
};
#[cfg(feature = "public-control")]
use crate::wire::{PUBLIC_CONTROL_ADMISSION_HEADER, PUBLIC_CONTROL_PATH, PublicControlAdmission};
use futures_util::{SinkExt, StreamExt};
use rustls::{
    ClientConfig, RootCertStore,
    pki_types::{CertificateDer, ServerName},
};
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpSocket, TcpStream},
    time::{Instant, timeout_at},
};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{
        Error as WsError, Message,
        client::IntoClientRequest,
        http::{HeaderValue, StatusCode, header},
        protocol::WebSocketConfig,
    },
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
pub struct LoopbackTlsTrust(pub(super) Arc<ClientConfig>);

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

/// Native-only bearer for the distinctly provisioned control route.
#[cfg(feature = "public-control")]
pub struct PublicControlToken(Zeroizing<String>);

#[cfg(feature = "public-control")]
impl PublicControlToken {
    pub fn new(value: String) -> Result<Self, ClientError> {
        let value = Zeroizing::new(value);
        if !wire::valid_public_control_token(&value) {
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
    #[error("V2 public video requires an approved literal IPv4 source and public relay socket")]
    PublicVideoEndpoint,
    #[error("unable to bind or verify the public video relay source socket")]
    PublicVideoBind,
    #[error("public video relay approval is expired or too long")]
    PublicVideoExpiry,
    #[error("invalid relay token")]
    Token,
    #[error("invalid relay room")]
    Room,
    #[error("invalid relay trust configuration")]
    TlsTrust,
    #[error("relay TLS handshake or certificate verification failed")]
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

    #[cfg(test)]
    fn matches_socket(self, local: SocketAddr, peer: SocketAddr) -> bool {
        matches_bound_socket(self.source, self.relay, local, peer)
    }

    async fn dial(self) -> Result<TcpStream, ClientError> {
        dial_bound(self.source, self.relay, ClientError::PrivateLanBind).await
    }
}

/// Literal source and relay tuple admitted by the caller for a short window.
/// This type neither grants device/media permissions nor initiates a connection.
pub struct PublicVideoEndpoint {
    source: Ipv4Addr,
    relay: SocketAddrV4,
    expires_at: u64,
    deadline: Instant,
    max_lease: Duration,
}

impl PublicVideoEndpoint {
    pub fn new(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, ClientError> {
        Self::check_addresses(source, relay)?;
        Self::with_clock(source, relay, expires_at, now, Instant::now())
    }

    fn check_addresses(source: Ipv4Addr, relay: SocketAddrV4) -> Result<(), ClientError> {
        if !(source.is_private() || public_unicast(source))
            || !public_unicast(*relay.ip())
            || relay.port() < 1024
        {
            return Err(ClientError::PublicVideoEndpoint);
        }
        Ok(())
    }

    fn with_clock(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
        started_at: Instant,
    ) -> Result<Self, ClientError> {
        Self::with_lease(source, relay, expires_at, now, started_at, ROOM_LIFETIME)
    }

    fn with_lease(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
        started_at: Instant,
        max_lease: Duration,
    ) -> Result<Self, ClientError> {
        let ttl = approval_remaining(expires_at, now, max_lease)?;
        Ok(Self {
            source,
            relay,
            expires_at,
            deadline: started_at + ttl,
            max_lease,
        })
    }

    pub fn source(&self) -> Ipv4Addr {
        self.source
    }

    pub fn relay(&self) -> SocketAddrV4 {
        self.relay
    }

    fn checked_deadline(&self) -> Result<Instant, ClientError> {
        self.checked_deadline_at(epoch_seconds()?, Instant::now())
    }

    fn checked_deadline_at(
        &self,
        now: u64,
        monotonic_now: Instant,
    ) -> Result<Instant, ClientError> {
        let remaining = approval_remaining(self.expires_at, now, self.max_lease)?;
        if monotonic_now >= self.deadline {
            return Err(ClientError::PublicVideoExpiry);
        }
        Ok(self.deadline.min(monotonic_now + remaining))
    }
}

fn public_unicast(ip: Ipv4Addr) -> bool {
    // Match the scoped media policy's conservative public IPv4 ranges.
    let [a, b, _, _] = ip.octets();
    a != 0
        && a < 224
        && !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_documentation()
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 192 && b == 0)
        && !(a == 198 && (b == 18 || b == 19))
}

/// An explicitly configured route, not media or input permission.
#[cfg(feature = "public-control")]
pub struct PublicControlEndpoint(PublicVideoEndpoint, PublicControlAdmission);

#[cfg(feature = "public-control")]
impl PublicControlEndpoint {
    pub fn new(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, ClientError> {
        PublicVideoEndpoint::new(source, relay, expires_at, now)
            .map(|endpoint| Self(endpoint, PublicControlAdmission::Legacy))
    }

    /// Outer signaling invitation for one independently approved test-batch slot.
    /// This is not the shorter media/input activity deadline.
    pub fn new_immediate_batch(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, ClientError> {
        Self::with_admission(
            source,
            relay,
            expires_at,
            now,
            Instant::now(),
            PublicControlAdmission::ImmediateV1,
        )
    }

    /// The sustained product's independently scoped signaling room. It
    /// begins no connection and does not authorize capture or input.
    pub fn new_basic_sustained(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
    ) -> Result<Self, ClientError> {
        Self::with_admission(
            source,
            relay,
            expires_at,
            now,
            Instant::now(),
            PublicControlAdmission::BasicSustainedV1,
        )
    }

    fn with_admission(
        source: Ipv4Addr,
        relay: SocketAddrV4,
        expires_at: u64,
        now: u64,
        started_at: Instant,
        admission: PublicControlAdmission,
    ) -> Result<Self, ClientError> {
        PublicVideoEndpoint::check_addresses(source, relay)?;
        let endpoint = Self(
            PublicVideoEndpoint::with_lease(
                source,
                relay,
                expires_at,
                now,
                started_at,
                admission.max_lease(),
            )?,
            admission,
        );
        endpoint.checked_admission_at(now, started_at)?;
        Ok(endpoint)
    }

    pub fn is_immediate_batch(&self) -> bool {
        self.1 == PublicControlAdmission::ImmediateV1
    }

    fn checked_admission_at(
        &self,
        now: u64,
        monotonic_now: Instant,
    ) -> Result<Instant, ClientError> {
        let deadline = self.0.checked_deadline_at(now, monotonic_now)?;
        if deadline.saturating_duration_since(monotonic_now) < self.1.minimum_at_admission() {
            return Err(ClientError::PublicVideoExpiry);
        }
        Ok(deadline)
    }
}

fn epoch_seconds() -> Result<u64, ClientError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| ClientError::PublicVideoExpiry)
}

fn approval_remaining(
    expires_at: u64,
    now: u64,
    max_lease: Duration,
) -> Result<Duration, ClientError> {
    let remaining = Duration::from_secs(expires_at.saturating_sub(now));
    if remaining.is_zero() || remaining > max_lease {
        return Err(ClientError::PublicVideoExpiry);
    }
    Ok(remaining)
}

fn matches_bound_socket(
    source: Ipv4Addr,
    relay: SocketAddrV4,
    local: SocketAddr,
    peer: SocketAddr,
) -> bool {
    matches!(local, SocketAddr::V4(v4) if *v4.ip() == source && v4.port() != 0)
        && peer == SocketAddr::V4(relay)
}

pub(super) async fn dial_bound(
    source: Ipv4Addr,
    relay: SocketAddrV4,
    bind_error: ClientError,
) -> Result<TcpStream, ClientError> {
    let socket = TcpSocket::new_v4().map_err(|_| bind_error)?;
    socket
        .bind(SocketAddr::V4(SocketAddrV4::new(source, 0)))
        .map_err(|_| bind_error)?;
    let tcp = socket
        .connect(SocketAddr::V4(relay))
        .await
        .map_err(|_| ClientError::Transport)?;
    let local = tcp.local_addr().map_err(|_| bind_error)?;
    let peer = tcp.peer_addr().map_err(|_| bind_error)?;
    if !matches_bound_socket(source, relay, local, peer) {
        return Err(bind_error);
    }
    Ok(tcp)
}

enum Route<'a> {
    Loopback(SocketAddr, Option<&'a LoopbackTlsTrust>),
    PrivateLan(PrivateLanEndpoint, &'a LoopbackTlsTrust),
    PublicVideo(PublicVideoEndpoint, &'a LoopbackTlsTrust),
    #[cfg(feature = "public-control")]
    PublicControl(PublicControlEndpoint, &'a LoopbackTlsTrust),
}

impl Route<'_> {
    fn server(&self) -> Result<SocketAddr, ClientError> {
        match *self {
            Self::Loopback(address, _) if address.ip().is_loopback() && address.port() != 0 => {
                Ok(address)
            }
            Self::Loopback(..) => Err(ClientError::Endpoint),
            Self::PrivateLan(endpoint, _) => Ok(SocketAddr::V4(endpoint.relay)),
            Self::PublicVideo(ref endpoint, _) => Ok(SocketAddr::V4(endpoint.relay)),
            #[cfg(feature = "public-control")]
            Self::PublicControl(ref endpoint, _) => Ok(SocketAddr::V4(endpoint.0.relay)),
        }
    }

    fn trust(&self) -> Option<&LoopbackTlsTrust> {
        match *self {
            Self::Loopback(_, trust) => trust,
            Self::PrivateLan(_, trust) | Self::PublicVideo(_, trust) => Some(trust),
            #[cfg(feature = "public-control")]
            Self::PublicControl(_, trust) => Some(trust),
        }
    }

    fn path(&self) -> &'static str {
        match self {
            Self::PublicVideo(..) => PUBLIC_VIDEO_PATH,
            #[cfg(feature = "public-control")]
            Self::PublicControl(..) => PUBLIC_CONTROL_PATH,
            _ => PATH,
        }
    }

    fn max_lease(&self) -> Duration {
        match self {
            #[cfg(feature = "public-control")]
            Self::PublicControl(endpoint, _) => endpoint.0.max_lease,
            _ => ROOM_LIFETIME,
        }
    }

    fn checked_deadline(&self, fallback: Instant) -> Result<Instant, ClientError> {
        match self {
            Self::PublicVideo(endpoint, _) => endpoint.checked_deadline().map(|d| d.min(fallback)),
            #[cfg(feature = "public-control")]
            Self::PublicControl(endpoint, _) => {
                endpoint.0.checked_deadline().map(|d| d.min(fallback))
            }
            _ => Ok(fallback),
        }
    }

    fn checked_admission(&self, fallback: Instant) -> Result<Instant, ClientError> {
        #[cfg(feature = "public-control")]
        if let Self::PublicControl(endpoint, _) = self {
            return endpoint
                .checked_admission_at(epoch_seconds()?, Instant::now())
                .map(|deadline| deadline.min(fallback));
        }
        self.checked_deadline(fallback)
    }

    #[cfg(feature = "public-control")]
    fn admission_header(&self) -> Option<HeaderValue> {
        match self {
            Self::PublicControl(endpoint, _) => {
                endpoint.1.header_value().map(HeaderValue::from_static)
            }
            _ => None,
        }
    }

    fn into_approval(self) -> Option<PublicVideoEndpoint> {
        match self {
            Self::PublicVideo(endpoint, _) => Some(endpoint),
            #[cfg(feature = "public-control")]
            Self::PublicControl(endpoint, _) => Some(endpoint.0),
            _ => None,
        }
    }

    async fn dial(&self, server: SocketAddr) -> Result<TcpStream, ClientError> {
        match *self {
            Self::Loopback(..) => TcpStream::connect(server)
                .await
                .map_err(|_| ClientError::Transport),
            Self::PrivateLan(endpoint, _) => endpoint.dial().await,
            Self::PublicVideo(ref endpoint, _) => Self::dial_public(endpoint).await,
            #[cfg(feature = "public-control")]
            Self::PublicControl(ref endpoint, _) => Self::dial_public(&endpoint.0).await,
        }
    }

    async fn dial_public(endpoint: &PublicVideoEndpoint) -> Result<TcpStream, ClientError> {
        endpoint.checked_deadline()?;
        dial_bound(
            endpoint.source,
            endpoint.relay,
            ClientError::PublicVideoBind,
        )
        .await
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
    LoopbackRelay::connect_with_route(Route::PrivateLan(endpoint, trust), &token.0, room, role)
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

/// Dedicated public-video relay connection. Establishing it does not authorize
/// the peer, capture, media routing, or input.
pub struct PublicVideoRelay(LoopbackRelay);

/// Connect only to the approved literal IPv4 socket, bound to its exact
/// source. The only trust root is the independently provisioned DER CA;
/// rustls must verify TLS for the relay IP before sending the bearer.
/// This client never follows redirects or attempts plaintext/legacy fallback.
pub async fn connect_public_video_wss(
    endpoint: PublicVideoEndpoint,
    token: &RelayToken,
    room: RoomId,
    role: Role,
    trust: &LoopbackTlsTrust,
) -> Result<PublicVideoRelay, ClientError> {
    LoopbackRelay::connect_with_route(Route::PublicVideo(endpoint, trust), &token.0, room, role)
        .await
        .map(PublicVideoRelay)
}

impl PublicVideoRelay {
    pub async fn send_signal(&mut self, kind: SignalKind, data: String) -> Result<(), ClientError> {
        let result = self.0.send_signal(kind, data).await;
        if result.is_err() {
            self.0.socket.take();
        }
        result
    }

    pub async fn next_event(&mut self, wait: Duration) -> Result<RelayEvent, ClientError> {
        self.0.next_event(wait).await
    }

    pub async fn leave(self) -> Result<(), ClientError> {
        self.0.leave().await
    }
}

/// Scoped signaling for the control route, not device or input approval.
#[cfg(feature = "public-control")]
pub struct PublicControlRelay(LoopbackRelay);

#[cfg(feature = "public-control")]
pub async fn connect_public_control_wss(
    endpoint: PublicControlEndpoint,
    token: &PublicControlToken,
    room: RoomId,
    role: Role,
    trust: &LoopbackTlsTrust,
) -> Result<PublicControlRelay, ClientError> {
    LoopbackRelay::connect_with_route(Route::PublicControl(endpoint, trust), &token.0, room, role)
        .await
        .map(PublicControlRelay)
}

#[cfg(feature = "public-control")]
impl PublicControlRelay {
    pub async fn send_signal(&mut self, kind: SignalKind, data: String) -> Result<(), ClientError> {
        let result = self.0.send_signal(kind, data).await;
        if result.is_err() {
            self.0.socket.take();
        }
        result
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
    approval: Option<PublicVideoEndpoint>,
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
        Self::connect_with_route(Route::Loopback(address, None), &token.0, room, role).await
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
        Self::connect_with_route(Route::Loopback(address, Some(trust)), &token.0, room, role).await
    }

    async fn connect_with_route(
        route: Route<'_>,
        token: &str,
        room: RoomId,
        role: Role,
    ) -> Result<Self, ClientError> {
        let address = route.server()?;
        let config = WebSocketConfig::default()
            .write_buffer_size(0)
            .max_write_buffer_size(MAX_WIRE * 2)
            .max_frame_size(Some(MAX_WIRE))
            .max_message_size(Some(MAX_WIRE));
        let started_at = Instant::now();
        let deadline = route.checked_admission(started_at + route.max_lease())?;
        let handshake_deadline = deadline.min(started_at + IO_TIMEOUT);
        let socket = timeout_at(handshake_deadline, async {
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
            route.checked_admission(deadline)?;
            let path = route.path();
            let mut request = format!("{scheme}://{address}{path}")
                .into_client_request()
                .map_err(|_| ClientError::Endpoint)?;
            let auth = Zeroizing::new(format!("Bearer {token}"));
            let mut auth_header = HeaderValue::from_str(&auth).map_err(|_| ClientError::Token)?;
            auth_header.set_sensitive(true);
            request
                .headers_mut()
                .insert(header::AUTHORIZATION, auth_header);
            #[cfg(feature = "public-control")]
            if let Some(admission) = route.admission_header() {
                request
                    .headers_mut()
                    .insert(PUBLIC_CONTROL_ADMISSION_HEADER, admission);
            }

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
            route.checked_deadline(deadline)?;
            Ok::<_, ClientError>(socket)
        })
        .await
        .map_err(|_| ClientError::Timeout)??;
        let mut relay = Self {
            socket: Some(socket),
            deadline,
            approval: route.into_approval(),
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

    fn effective_deadline(&self) -> Option<Instant> {
        let deadline = match &self.approval {
            Some(approval) => approval.checked_deadline().ok()?.min(self.deadline),
            None => self.deadline,
        };
        (Instant::now() < deadline).then_some(deadline)
    }

    fn flow_after(&self, signal: SignalKind) -> Option<Flow> {
        if self.approval.is_some() {
            self.flow.after_public_video(signal)
        } else {
            self.flow.after(signal)
        }
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
        let next_flow = self.flow_after(kind).ok_or(ClientError::Protocol)?;
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
        let Some(deadline) = self.effective_deadline().filter(|_| self.socket.is_some()) else {
            self.socket.take();
            return Err(ClientError::Closed);
        };
        let deadline = deadline.min(Instant::now() + wait.min(MAX_POLL_TIMEOUT));
        let outcome = timeout_at(deadline, self.read()).await;
        if self.effective_deadline().is_none() {
            self.socket.take();
            return Err(ClientError::Closed);
        }
        match outcome {
            Err(_) => Err(ClientError::Timeout),
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
        let Some(deadline) = self.effective_deadline() else {
            self.socket.take();
            return Err(ClientError::Closed);
        };
        let text = wire::encode(&frame).map_err(|_| ClientError::Payload)?;
        // Abort rather than reuse an indeterminate sequence if the caller
        // cancels a write after bytes have been queued into the socket.
        let mut socket = self.socket.take().ok_or(ClientError::Closed)?;
        let deadline = deadline.min(Instant::now() + IO_TIMEOUT);
        timeout_at(deadline, socket.send(Message::Text(text.into())))
            .await
            .map_err(|_| ClientError::Timeout)?
            .map_err(|_| ClientError::Transport)?;
        if self.effective_deadline().is_none() {
            return Err(ClientError::Closed);
        }
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
                self.flow = self.flow_after(signal).ok_or(ClientError::Protocol)?;
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

#[cfg(test)]
#[path = "public_video_client_tests.rs"]
mod public_video_client_tests;
