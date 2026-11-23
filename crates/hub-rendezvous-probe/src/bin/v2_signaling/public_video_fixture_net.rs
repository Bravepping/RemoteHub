//! Test-only NAT fixture: public-looking candidate labels never reach an OS
//! destination. Only two observed loopback UDP sockets and one mock STUN
//! responder exist. No DNS, TCP, TURN, interface, or firewall changes.

use rtc::stun::{
    message::{Message, Setter, BINDING_REQUEST, BINDING_SUCCESS},
    xoraddr::XorMappedAddress,
};
use std::{
    future::Future,
    io::{self, IoSliceMut},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, UdpSocket},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use webrtc::{
    peer_connection::RTCIceServer,
    runtime::{
        AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta,
        Runtime, Transmit,
    },
};

const LOOPBACK: Ipv4Addr = Ipv4Addr::LOCALHOST;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FixtureSide {
    Host,
    Viewer,
}

impl FixtureSide {
    pub(super) fn ip(self) -> Ipv4Addr {
        match self {
            // Labels only, never a destination passed to the OS.
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

    fn label(self, addr: SocketAddrV4) -> SocketAddr {
        SocketAddrV4::new(self.ip(), addr.port()).into()
    }
}

pub(super) struct FixtureNat {
    net: Arc<Network>,
    inner: Arc<dyn Runtime>,
    task: tokio::task::JoinHandle<()>,
}

impl FixtureNat {
    pub(super) async fn start() -> io::Result<Self> {
        let inner = webrtc::runtime::default_runtime()
            .ok_or_else(|| io::Error::other("fixture runtime unavailable"))?;
        let stun = tokio::net::UdpSocket::bind((LOOPBACK, 0)).await?;
        let SocketAddr::V4(stun_addr) = stun.local_addr()? else {
            return Err(denied());
        };
        let net = Arc::new(Network {
            sockets: Mutex::new([None, None]),
            stun_addr,
        });
        let task = tokio::spawn(stun_loop(stun, net.clone()));
        Ok(Self { net, inner, task })
    }

    pub(super) fn runtime(&self, side: FixtureSide) -> Arc<dyn Runtime> {
        Arc::new(FixtureRuntime {
            inner: self.inner.clone(),
            net: self.net.clone(),
            side,
        })
    }

    pub(super) fn stun_server(&self) -> RTCIceServer {
        RTCIceServer {
            urls: vec![format!("stun:{}", self.net.stun_addr)],
            ..Default::default()
        }
    }
}

impl Drop for FixtureNat {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug)]
struct Network {
    sockets: Mutex<[Option<SocketAddrV4>; 2]>,
    stun_addr: SocketAddrV4,
}

impl Network {
    fn register(&self, side: FixtureSide, addr: SocketAddrV4) -> io::Result<()> {
        let mut sockets = self.sockets.lock().map_err(|_| denied())?;
        if *addr.ip() != LOOPBACK
            || addr.port() == 0
            || addr == self.stun_addr
            || sockets[side.index()].is_some()
            || sockets[side.peer().index()] == Some(addr)
        {
            return Err(denied());
        }
        sockets[side.index()] = Some(addr);
        Ok(())
    }

    fn unregister(&self, side: FixtureSide, addr: SocketAddrV4) {
        if let Ok(mut sockets) = self.sockets.lock() {
            if sockets[side.index()] == Some(addr) {
                sockets[side.index()] = None;
            }
        }
    }

    fn wire_addr(&self, side: FixtureSide, destination: SocketAddr) -> Option<SocketAddr> {
        if destination == SocketAddr::V4(self.stun_addr) {
            return Some(destination);
        }
        let peer = side.peer();
        let addr = self.sockets.lock().ok()?[peer.index()]?;
        (destination == peer.label(addr)).then_some(SocketAddr::V4(addr))
    }

    fn source_label(&self, side: FixtureSide, source: SocketAddr) -> Option<SocketAddr> {
        if source == SocketAddr::V4(self.stun_addr) {
            return Some(source);
        }
        let peer = side.peer();
        let addr = self.sockets.lock().ok()?[peer.index()]?;
        (source == SocketAddr::V4(addr)).then_some(peer.label(addr))
    }

    fn mapped_client(&self, source: SocketAddr) -> Option<SocketAddr> {
        let sockets = self.sockets.lock().ok()?;
        for side in [FixtureSide::Host, FixtureSide::Viewer] {
            if let Some(addr) = sockets[side.index()] {
                if source == SocketAddr::V4(addr) {
                    return Some(side.label(addr));
                }
            }
        }
        None
    }
}

fn denied() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "off-fixture transport")
}

async fn stun_loop(socket: tokio::net::UdpSocket, net: Arc<Network>) {
    let mut bytes = [0_u8; 513];
    while let Ok((len, source)) = socket.recv_from(&mut bytes).await {
        let Some(mapped) = net.mapped_client(source) else {
            continue;
        };
        if !(20..=512).contains(&len) {
            continue;
        }
        let mut request = Message::new();
        request.raw = bytes[..len].to_vec();
        if request.decode().is_err() || request.typ != BINDING_REQUEST {
            continue;
        }
        let mut response = Message::new();
        if response
            .build(&[
                Box::new(request.transaction_id) as Box<dyn Setter>,
                Box::new(BINDING_SUCCESS),
                Box::new(XorMappedAddress {
                    ip: mapped.ip(),
                    port: mapped.port(),
                }),
            ])
            .is_ok()
        {
            // source was checked against an observed, exact IPv4 loopback socket.
            let _ = socket.send_to(&response.raw, source).await;
        }
    }
}

#[derive(Debug)]
struct FixtureRuntime {
    inner: Arc<dyn Runtime>,
    net: Arc<Network>,
    side: FixtureSide,
}

impl Runtime for FixtureRuntime {
    fn spawn(&self, task: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        self.inner.spawn(task)
    }

    fn spawn_reactor(
        &self,
        n: usize,
        task: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Box<dyn JoinHandle> {
        self.inner.spawn_reactor(n, task)
    }

    fn wrap_udp_socket(&self, socket: UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        let SocketAddr::V4(local) = socket.local_addr()? else {
            return Err(denied());
        };
        if *local.ip() != LOOPBACK {
            return Err(denied());
        }
        let inner = self.inner.wrap_udp_socket(socket)?;
        self.net.register(self.side, local)?;
        Ok(Arc::new(FixtureSocket {
            inner,
            local,
            net: self.net.clone(),
            side: self.side,
        }))
    }

    fn wrap_tcp_listener(&self, _: TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        Err(denied())
    }

    fn connect_tcp(
        &self,
        _: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + '_>> {
        Box::pin(async { Err(denied()) })
    }

    fn resolve_host<'a>(
        &'a self,
        address: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let requested = address.parse::<SocketAddr>().map_err(|_| denied())?;
            if requested != SocketAddr::V4(self.net.stun_addr) {
                return Err(denied());
            }
            Ok(vec![requested])
        })
    }

    fn now(&self) -> Instant {
        self.inner.now()
    }

    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.inner.sleep(d)
    }

    fn interval(&self, d: Duration) -> Box<dyn AsyncInterval> {
        self.inner.interval(d)
    }

    fn block_on(&self, task: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.inner.block_on(task)
    }
}

#[derive(Debug)]
struct FixtureSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    local: SocketAddrV4,
    net: Arc<Network>,
    side: FixtureSide,
}

impl Drop for FixtureSocket {
    fn drop(&mut self) {
        self.net.unregister(self.side, self.local);
    }
}

impl AsyncUdpSocket for FixtureSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local.into())
    }

    fn poll_send(&self, cx: &mut Context<'_>, tx: &Transmit<'_>) -> Poll<io::Result<usize>> {
        let Some(destination) = self.net.wire_addr(self.side, tx.destination) else {
            return Poll::Ready(Err(denied()));
        };
        if tx.src_ip.is_some_and(|ip| ip != IpAddr::V4(LOOPBACK)) {
            return Poll::Ready(Err(denied()));
        }
        let mut mapped = tx.clone();
        mapped.destination = destination;
        self.inner.poll_send(cx, &mapped)
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
                    if let Some(label) = self.net.source_label(self.side, meta[0].addr) {
                        meta[0].addr = label;
                        meta[0].dst_ip = Some(IpAddr::V4(LOOPBACK));
                        return Poll::Ready(Ok(1));
                    }
                }
                result => return result,
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[test]
fn fixture_nat_maps_only_current_exact_observed_sockets() {
    let stun_addr = SocketAddrV4::new(LOOPBACK, 34000);
    let host = SocketAddrV4::new(LOOPBACK, 34001);
    let viewer = SocketAddrV4::new(LOOPBACK, 34002);
    let same_ip_wrong_port = SocketAddrV4::new(LOOPBACK, 34003);
    let net = Network {
        sockets: Mutex::new([None, None]),
        stun_addr,
    };
    assert_eq!(
        net.wire_addr(FixtureSide::Host, FixtureSide::Viewer.label(viewer)),
        None
    );
    net.register(FixtureSide::Host, host).unwrap();
    net.register(FixtureSide::Viewer, viewer).unwrap();
    assert_eq!(
        net.wire_addr(FixtureSide::Host, FixtureSide::Viewer.label(viewer)),
        Some(viewer.into())
    );
    assert_eq!(
        net.source_label(FixtureSide::Host, viewer.into()),
        Some(FixtureSide::Viewer.label(viewer))
    );
    assert_eq!(
        net.mapped_client(host.into()),
        Some(FixtureSide::Host.label(host))
    );
    assert_eq!(net.mapped_client(same_ip_wrong_port.into()), None);
    assert_eq!(
        net.source_label(FixtureSide::Host, same_ip_wrong_port.into()),
        None
    );
    assert_eq!(net.wire_addr(FixtureSide::Host, viewer.into()), None);
    assert_eq!(
        net.wire_addr(
            FixtureSide::Host,
            FixtureSide::Viewer.label(same_ip_wrong_port)
        ),
        None
    );
    net.unregister(FixtureSide::Viewer, viewer);
    assert_eq!(
        net.wire_addr(FixtureSide::Host, FixtureSide::Viewer.label(viewer)),
        None
    );
}
