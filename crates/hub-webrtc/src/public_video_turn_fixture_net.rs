//! Two-allocation TURN fixture for tests only. Real UDP is confined to one
//! IPv4 loopback socket and the two authenticated loopback peers. Relayed
//! addresses and optional server-reflexive NAT addresses are labels, never
//! OS destinations. The simulated fault is per fixture, never system-wide.

use rtc::{
    crypto::RTCCryptoProvider,
    stun::{
        attributes::{ATTR_NONCE, ATTR_REALM, ATTR_USERNAME},
        error_code::{CODE_ALLOC_MISMATCH, CODE_FORBIDDEN, CODE_UNAUTHORIZED},
        fingerprint::FINGERPRINT,
        integrity::MessageIntegrity,
        message::{
            Getter, Message, MessageClass, MessageType, Setter, TransactionId,
            CLASS_ERROR_RESPONSE, CLASS_INDICATION, CLASS_REQUEST, CLASS_SUCCESS_RESPONSE,
            METHOD_ALLOCATE, METHOD_BINDING, METHOD_CHANNEL_BIND, METHOD_CREATE_PERMISSION,
            METHOD_DATA, METHOD_REFRESH, METHOD_SEND,
        },
        textattrs::TextAttribute,
        xoraddr::XorMappedAddress,
    },
    turn::proto::{
        chandata::ChannelData, channum::ChannelNumber, data::Data, lifetime::Lifetime,
        peeraddr::PeerAddress, relayaddr::RelayedAddress, reqtrans::RequestedTransport, PROTO_UDP,
    },
};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    io::{self, IoSliceMut},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, UdpSocket as StdUdpSocket},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::{Duration, Instant as StdInstant},
};
use tokio::{
    net::UdpSocket,
    task::JoinHandle,
    time::{interval, Instant},
};
use uuid::Uuid;
use webrtc::{
    peer_connection::RTCIceServer,
    runtime::{
        AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket,
        JoinHandle as RuntimeJoinHandle, RecvMeta, Runtime, Transmit,
    },
};

const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;
const REALM: &str = "turn.fixture.invalid";
const MAX_PACKET: usize = 2048;
pub(super) const RELAY_LABEL: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);
pub(super) const RELAY_PORTS: (u16, u16) = (49152, 49153);

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TurnFixtureStats {
    pub(super) allocations_created: usize,
    pub(super) permissions_created: usize,
    pub(super) forwarded_packets: usize,
    pub(super) released: usize,
    pub(super) expired: usize,
    pub(super) active: usize,
    pub(super) direct_forwarded: usize,
    pub(super) direct_dropped: usize,
    pub(super) direct_checks_dropped: usize,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum FixtureDirectSide {
    Host,
    Viewer,
}

impl FixtureDirectSide {
    pub(super) fn ip(self) -> Ipv4Addr {
        match self {
            Self::Host => Ipv4Addr::new(1, 1, 1, 1),
            Self::Viewer => Ipv4Addr::new(8, 8, 4, 4),
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Host => 0,
            Self::Viewer => 1,
        }
    }

    fn peer(self) -> Self {
        match self {
            Self::Host => Self::Viewer,
            Self::Viewer => Self::Host,
        }
    }

    fn label(self, socket: SocketAddrV4) -> SocketAddr {
        SocketAddrV4::new(self.ip(), socket.port()).into()
    }
}

pub(super) struct FixtureTurn {
    address: SocketAddrV4,
    username: String,
    credential: String,
    statistics: Arc<Mutex<TurnFixtureStats>>,
    direct: Arc<DirectNetwork>,
    task: JoinHandle<()>,
}

impl FixtureTurn {
    pub(super) async fn start(lifetime_seconds: u32) -> io::Result<Self> {
        if !(1..=30).contains(&lifetime_seconds) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture TTL out of range",
            ));
        }
        let udp = UdpSocket::bind((LOOPBACK, 0)).await?;
        let SocketAddr::V4(address) = udp.local_addr()? else {
            return Err(io::Error::other("fixture requires IPv4"));
        };
        let username = Uuid::new_v4().simple().to_string();
        let credential = Uuid::new_v4().simple().to_string();
        let crypto = rtc::crypto::default_provider().map_err(io::Error::other)?;
        let key = MessageIntegrity::long_term_integrity_key(
            username.clone(),
            REALM.into(),
            credential.clone(),
            crypto.crypto(),
        )
        .map_err(io::Error::other)?;
        let statistics = Arc::new(Mutex::new(TurnFixtureStats::default()));
        let direct = Arc::new(DirectNetwork {
            server: address,
            sockets: Mutex::new([None, None]),
            blocked: AtomicBool::new(false),
            statistics: statistics.clone(),
        });
        let actor = FixtureState {
            nonce: Uuid::new_v4().simple().to_string(),
            username: username.clone(),
            key,
            crypto,
            lifetime: Duration::from_secs(lifetime_seconds.into()),
            allocations: HashMap::with_capacity(2),
            direct: direct.clone(),
            statistics: statistics.clone(),
        };
        let task = tokio::spawn(actor.run(udp));
        Ok(Self {
            address,
            username,
            credential,
            statistics,
            direct,
            task,
        })
    }

    pub(super) fn server(&self) -> SocketAddrV4 {
        self.address
    }

    pub(super) fn username(&self) -> &str {
        &self.username
    }

    pub(super) fn credential(&self) -> &str {
        &self.credential
    }

    pub(super) fn ice_server(&self) -> RTCIceServer {
        RTCIceServer {
            urls: vec![format!("turn:{}?transport=udp", self.server())],
            username: self.username().to_owned(),
            credential: self.credential().to_owned(),
        }
    }

    pub(super) fn combined_ice_server(&self) -> RTCIceServer {
        let mut server = self.ice_server();
        server.urls.push(format!("stun:{}", self.server()));
        server
    }

    pub(super) fn direct_candidate(&self, side: FixtureDirectSide) -> Option<SocketAddrV4> {
        let socket = self.direct.sockets.lock().ok()?[side.index()]?;
        Some(SocketAddrV4::new(side.ip(), socket.port()))
    }

    pub(super) fn runtime(&self, side: FixtureDirectSide) -> Arc<dyn Runtime> {
        Arc::new(DirectFixtureRuntime {
            inner: webrtc::runtime::default_runtime().expect("fixture requires tokio runtime"),
            network: self.direct.clone(),
            side,
        })
    }

    // Affect only exact NAT peer datagrams, never requests to the fixture
    // listener or TURN relayed packets handled at that listener.
    pub(super) fn block_direct(&self, block: bool) {
        self.direct.blocked.store(block, Ordering::Release);
    }

    pub(super) fn stats(&self) -> TurnFixtureStats {
        *self.statistics.lock().expect("fixture statistics")
    }
}

impl Drop for FixtureTurn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Allocation {
    label: SocketAddr,
    expires: Instant,
    permissions: HashSet<SocketAddr>,
    channel: Option<(ChannelNumber, SocketAddr)>,
}

struct FixtureState {
    nonce: String,
    username: String,
    key: Vec<u8>,
    crypto: Arc<dyn RTCCryptoProvider>,
    lifetime: Duration,
    allocations: HashMap<SocketAddr, Allocation>,
    direct: Arc<DirectNetwork>,
    statistics: Arc<Mutex<TurnFixtureStats>>,
}

struct Datagram {
    destination: SocketAddr,
    bytes: Vec<u8>,
    forwarded: bool,
}

impl FixtureState {
    async fn run(mut self, socket: UdpSocket) {
        let mut bytes = [0; MAX_PACKET + 1];
        let mut expiry = interval(Duration::from_millis(20));
        loop {
            tokio::select! {
                _ = expiry.tick() => self.expire(Instant::now()),
                received = socket.recv_from(&mut bytes) => {
                    let Ok((len, source)) = received else { return; };
                    if source.ip() != std::net::IpAddr::V4(LOOPBACK) || len > MAX_PACKET {
                        continue;
                    }
                    let now = Instant::now();
                    self.expire(now);
                    if let Some(packet) = self.receive(source, &bytes[..len], now) {
                        assert_eq!(packet.destination.ip(), std::net::IpAddr::V4(LOOPBACK));
                        if socket.send_to(&packet.bytes, packet.destination).await.is_ok()
                            && packet.forwarded
                        {
                            self.statistics.lock().unwrap().forwarded_packets += 1;
                        }
                    }
                }
            }
        }
    }

    fn expire(&mut self, now: Instant) {
        let previous = self.allocations.len();
        self.allocations
            .retain(|_, allocation| now < allocation.expires);
        let mut stats = self.statistics.lock().unwrap();
        stats.expired += previous - self.allocations.len();
        stats.active = self.allocations.len();
    }

    fn receive(&mut self, source: SocketAddr, bytes: &[u8], now: Instant) -> Option<Datagram> {
        if ChannelData::is_channel_data(bytes) {
            let mut data = ChannelData {
                raw: bytes.to_vec(),
                ..Default::default()
            };
            data.decode().ok()?;
            let (number, peer) = self.allocations.get(&source)?.channel?;
            return (number == data.number)
                .then(|| self.forward(source, peer, data.data))
                .flatten();
        }

        let mut message = Message::new();
        message.raw = bytes.to_vec();
        message.decode().ok()?;
        if message.typ.class == CLASS_INDICATION {
            if message.typ.method != METHOD_SEND {
                return None;
            }
            let mut data = Data::default();
            data.get_from(&message).ok()?;
            return self.forward(source, logical_peer(&message)?, data.0);
        }
        if message.typ.class != CLASS_REQUEST {
            return None;
        }
        if message.typ.method == METHOD_BINDING {
            // Combined tests see the label of the exact registered socket.
            // The TURN-only tests continue seeing their observed loopback address.
            let mapped = self.direct.mapped_client(source).unwrap_or(source);
            return self.reply(
                source,
                &message,
                CLASS_SUCCESS_RESPONSE,
                false,
                vec![Box::new(XorMappedAddress {
                    ip: mapped.ip(),
                    port: mapped.port(),
                })],
            );
        }
        if !self.authenticated(&mut message) {
            return self.reply(
                source,
                &message,
                CLASS_ERROR_RESPONSE,
                false,
                vec![
                    Box::new(CODE_UNAUTHORIZED),
                    Box::new(TextAttribute::new(ATTR_REALM, REALM.into())),
                    Box::new(TextAttribute::new(ATTR_NONCE, self.nonce.clone())),
                ],
            );
        }

        match message.typ.method {
            METHOD_ALLOCATE => self.allocate(source, &message, now),
            METHOD_CREATE_PERMISSION => self.permission(source, &message),
            METHOD_CHANNEL_BIND => self.channel(source, &message),
            METHOD_REFRESH => self.refresh(source, &message, now),
            _ => None,
        }
    }

    fn authenticated(&self, message: &mut Message) -> bool {
        let matches = |attribute, expected: &str| {
            TextAttribute::get_from_as(message, attribute).is_ok_and(|value| value.text == expected)
        };
        matches(ATTR_USERNAME, &self.username)
            && matches(ATTR_REALM, REALM)
            && matches(ATTR_NONCE, &self.nonce)
            && MessageIntegrity::check(message, &self.key, self.crypto.crypto()).is_ok()
    }

    fn allocate(
        &mut self,
        source: SocketAddr,
        request: &Message,
        now: Instant,
    ) -> Option<Datagram> {
        let mut transport = RequestedTransport::default();
        transport.get_from(request).ok()?;
        if transport.protocol != PROTO_UDP {
            return None;
        }
        let mut stats = self.statistics.lock().unwrap();
        if !self.allocations.contains_key(&source) {
            let port = match stats.allocations_created {
                0 => RELAY_PORTS.0,
                1 => RELAY_PORTS.1,
                _ => {
                    return self.reply(
                        source,
                        request,
                        CLASS_ERROR_RESPONSE,
                        true,
                        vec![Box::new(CODE_FORBIDDEN)],
                    )
                }
            };
            self.allocations.insert(
                source,
                Allocation {
                    label: SocketAddrV4::new(RELAY_LABEL, port).into(),
                    expires: now + self.lifetime,
                    permissions: HashSet::with_capacity(1),
                    channel: None,
                },
            );
            stats.allocations_created += 1;
            stats.active = self.allocations.len();
        }
        let allocation = self.allocations.get(&source)?;
        let mapped = self.direct.mapped_client(source).unwrap_or(source);
        self.reply(
            source,
            request,
            CLASS_SUCCESS_RESPONSE,
            true,
            vec![
                Box::new(RelayedAddress {
                    ip: allocation.label.ip(),
                    port: allocation.label.port(),
                }),
                Box::new(XorMappedAddress {
                    ip: mapped.ip(),
                    port: mapped.port(),
                }),
                Box::new(Lifetime(remaining(allocation, now))),
            ],
        )
    }

    fn peer_is_allocated(&self, source: SocketAddr, label: SocketAddr) -> bool {
        self.allocations
            .iter()
            .any(|(address, allocation)| *address != source && allocation.label == label)
    }

    fn permission(&mut self, source: SocketAddr, request: &Message) -> Option<Datagram> {
        let peer = logical_peer(request)?;
        if !self.peer_is_allocated(source, peer) {
            return self.reply(
                source,
                request,
                CLASS_ERROR_RESPONSE,
                true,
                vec![Box::new(CODE_FORBIDDEN)],
            );
        }
        if self.allocations.get_mut(&source)?.permissions.insert(peer) {
            self.statistics.lock().unwrap().permissions_created += 1;
        }
        self.reply(source, request, CLASS_SUCCESS_RESPONSE, true, vec![])
    }

    fn channel(&mut self, source: SocketAddr, request: &Message) -> Option<Datagram> {
        let peer = logical_peer(request)?;
        let mut number = ChannelNumber::default();
        number.get_from(request).ok()?;
        let allocation = self.allocations.get_mut(&source)?;
        if !number.valid()
            || !allocation.permissions.contains(&peer)
            || allocation
                .channel
                .is_some_and(|mapped| mapped != (number, peer))
        {
            return self.reply(
                source,
                request,
                CLASS_ERROR_RESPONSE,
                true,
                vec![Box::new(CODE_FORBIDDEN)],
            );
        }
        allocation.channel = Some((number, peer));
        self.reply(source, request, CLASS_SUCCESS_RESPONSE, true, vec![])
    }

    fn refresh(&mut self, source: SocketAddr, request: &Message, now: Instant) -> Option<Datagram> {
        let Some(allocation) = self.allocations.get(&source) else {
            return self.reply(
                source,
                request,
                CLASS_ERROR_RESPONSE,
                true,
                vec![Box::new(CODE_ALLOC_MISMATCH)],
            );
        };
        let mut requested = Lifetime::default();
        requested.get_from(request).ok()?;
        let lifetime = if requested.0.is_zero() {
            self.allocations.remove(&source);
            let mut stats = self.statistics.lock().unwrap();
            stats.released += 1;
            stats.active = self.allocations.len();
            Duration::ZERO
        } else {
            // Fixed short test lease, even on a nonzero Refresh; never
            // extend the fixture's hard stop or count one as a release.
            remaining(allocation, now)
        };
        self.reply(
            source,
            request,
            CLASS_SUCCESS_RESPONSE,
            true,
            vec![Box::new(Lifetime(lifetime))],
        )
    }

    fn forward(&self, source: SocketAddr, to: SocketAddr, data: Vec<u8>) -> Option<Datagram> {
        let from = self.allocations.get(&source)?;
        if data.is_empty() || data.len() > MAX_PACKET || !from.permissions.contains(&to) {
            return None;
        }
        let (destination, target) = self
            .allocations
            .iter()
            .find(|(_, allocation)| allocation.label == to)?;
        if !target.permissions.contains(&from.label) {
            return None;
        }
        // Use a typed indication irrespective of how the sender wrapped the
        // payload. The real clients may originate either Send or ChannelData.
        let mut message = Message::new();
        message
            .build(&[
                Box::new(TransactionId::new()),
                Box::new(MessageType::new(METHOD_DATA, CLASS_INDICATION)),
                Box::new(PeerAddress {
                    ip: from.label.ip(),
                    port: from.label.port(),
                }),
                Box::new(Data(data)),
                Box::new(FINGERPRINT),
            ])
            .ok()?;
        Some(Datagram {
            destination: *destination,
            bytes: message.raw,
            forwarded: true,
        })
    }

    fn reply(
        &self,
        destination: SocketAddr,
        request: &Message,
        class: MessageClass,
        authenticated: bool,
        attrs: Vec<Box<dyn Setter>>,
    ) -> Option<Datagram> {
        let mut setters: Vec<Box<dyn Setter + '_>> = vec![
            Box::new(request.transaction_id),
            Box::new(MessageType::new(request.typ.method, class)),
        ];
        setters.extend(attrs);
        if authenticated {
            setters.push(Box::new(MessageIntegrity::new_raw_integrity_with_provider(
                self.key.clone(),
                self.crypto.crypto(),
            )));
        }
        setters.push(Box::new(FINGERPRINT));
        let mut response = Message::new();
        response.build(&setters).ok()?;
        Some(Datagram {
            destination,
            bytes: response.raw,
            forwarded: false,
        })
    }
}

fn logical_peer(message: &Message) -> Option<SocketAddr> {
    let mut peer = PeerAddress::default();
    peer.get_from(message).ok()?;
    Some(SocketAddr::new(peer.ip, peer.port))
}

fn remaining(allocation: &Allocation, now: Instant) -> Duration {
    Duration::from_secs(
        allocation
            .expires
            .saturating_duration_since(now)
            .as_secs()
            .max(1),
    )
}

#[derive(Debug)]
struct DirectNetwork {
    server: SocketAddrV4,
    sockets: Mutex<[Option<SocketAddrV4>; 2]>,
    blocked: AtomicBool,
    statistics: Arc<Mutex<TurnFixtureStats>>,
}

impl DirectNetwork {
    fn register(&self, side: FixtureDirectSide, socket: SocketAddrV4) -> io::Result<()> {
        let mut sockets = self.sockets.lock().map_err(|_| direct_denied())?;
        if *socket.ip() != LOOPBACK
            || socket.port() == 0
            || socket == self.server
            || sockets[side.index()].is_some()
            || sockets[side.peer().index()] == Some(socket)
        {
            return Err(direct_denied());
        }
        sockets[side.index()] = Some(socket);
        Ok(())
    }

    fn unregister(&self, side: FixtureDirectSide, socket: SocketAddrV4) {
        if let Ok(mut sockets) = self.sockets.lock() {
            if sockets[side.index()] == Some(socket) {
                sockets[side.index()] = None;
            }
        }
    }

    fn observed_peer(&self, side: FixtureDirectSide) -> Option<SocketAddrV4> {
        self.sockets.lock().ok()?[side.peer().index()]
    }

    fn mapped_client(&self, source: SocketAddr) -> Option<SocketAddr> {
        let sockets = self.sockets.lock().ok()?;
        [FixtureDirectSide::Host, FixtureDirectSide::Viewer]
            .into_iter()
            .find_map(|side| {
                let socket = sockets[side.index()]?;
                (source == SocketAddr::V4(socket)).then_some(side.label(socket))
            })
    }

    fn record_drop(&self, contents: &[u8]) {
        let mut stats = self.statistics.lock().expect("fixture statistics");
        stats.direct_dropped += 1;
        if contents.len() >= 20
            && contents[..2] == [0, 1]
            && contents[4..8] == [0x21, 0x12, 0xa4, 0x42]
        {
            stats.direct_checks_dropped += 1;
        }
    }
}

fn direct_denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "off-fixture direct transport",
    )
}

#[derive(Debug)]
struct DirectFixtureRuntime {
    inner: Arc<dyn Runtime>,
    network: Arc<DirectNetwork>,
    side: FixtureDirectSide,
}

impl Runtime for DirectFixtureRuntime {
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn RuntimeJoinHandle> {
        self.inner.spawn(task)
    }

    fn spawn_reactor(
        &self,
        n: usize,
        task: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Box<dyn RuntimeJoinHandle> {
        self.inner.spawn_reactor(n, task)
    }

    fn wrap_udp_socket(&self, socket: StdUdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        let SocketAddr::V4(local) = socket.local_addr()? else {
            return Err(direct_denied());
        };
        if *local.ip() != LOOPBACK {
            return Err(direct_denied());
        }
        let inner = self.inner.wrap_udp_socket(socket)?;
        self.network.register(self.side, local)?;
        Ok(Arc::new(DirectFixtureSocket {
            inner,
            local,
            network: self.network.clone(),
            side: self.side,
        }))
    }

    fn wrap_tcp_listener(&self, _: TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        Err(direct_denied())
    }

    fn connect_tcp(
        &self,
        _: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + '_>> {
        Box::pin(async { Err(direct_denied()) })
    }

    fn resolve_host<'a>(
        &'a self,
        requested: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let address = requested.parse().map_err(|_| direct_denied())?;
            if address != SocketAddr::V4(self.network.server) {
                return Err(direct_denied());
            }
            Ok(vec![address])
        })
    }

    fn now(&self) -> StdInstant {
        self.inner.now()
    }

    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.inner.sleep(d)
    }

    fn interval(&self, d: Duration) -> Box<dyn AsyncInterval> {
        self.inner.interval(d)
    }

    fn block_on(&self, task: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.inner.block_on(task);
    }
}

#[derive(Debug)]
struct DirectFixtureSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    local: SocketAddrV4,
    network: Arc<DirectNetwork>,
    side: FixtureDirectSide,
}

impl Drop for DirectFixtureSocket {
    fn drop(&mut self) {
        self.network.unregister(self.side, self.local);
    }
}

impl AsyncUdpSocket for DirectFixtureSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local.into())
    }

    fn poll_send(&self, cx: &mut Context<'_>, tx: &Transmit<'_>) -> Poll<io::Result<usize>> {
        if tx.src_ip.is_some_and(|ip| ip != IpAddr::V4(LOOPBACK)) {
            return Poll::Ready(Err(direct_denied()));
        }
        if tx.destination == SocketAddr::V4(self.network.server) {
            return self.inner.poll_send(cx, tx);
        }
        let Some(peer) = self.network.observed_peer(self.side) else {
            return Poll::Ready(Err(direct_denied()));
        };
        if tx.destination != self.side.peer().label(peer) {
            return Poll::Ready(Err(direct_denied()));
        }
        if self.network.blocked.load(Ordering::Acquire) {
            self.network.record_drop(tx.contents);
            return Poll::Ready(Ok(tx.contents.len()));
        }
        let mut mapped = tx.clone();
        mapped.destination = peer.into();
        let result = self.inner.poll_send(cx, &mapped);
        if matches!(&result, Poll::Ready(Ok(n)) if *n > 0) {
            self.network.statistics.lock().unwrap().direct_forwarded += 1;
        }
        result
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        buffers: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if buffers.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(io::ErrorKind::InvalidInput.into()));
        }
        for _ in 0..32 {
            match self.inner.poll_recv(cx, &mut buffers[..1], &mut meta[..1]) {
                Poll::Ready(Ok(1)) => {
                    if meta[0].addr == SocketAddr::V4(self.network.server) {
                        return Poll::Ready(Ok(1));
                    }
                    if let Some(peer) = self.network.observed_peer(self.side) {
                        if meta[0].addr == SocketAddr::V4(peer)
                            && !self.network.blocked.load(Ordering::Acquire)
                        {
                            meta[0].addr = self.side.peer().label(peer);
                            meta[0].dst_ip = Some(IpAddr::V4(LOOPBACK));
                            return Poll::Ready(Ok(1));
                        }
                    }
                }
                result => return result,
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
