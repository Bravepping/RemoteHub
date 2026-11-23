//! Exact authenticated candidate endpoints. No system firewall/routing changes.
use crate::direct_policy::{DirectGrant, VerifiedCandidates};
use std::{
    fmt,
    future::Future,
    io::{self, IoSliceMut},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, RwLock,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime,
    Transmit,
};

#[derive(Default)]
pub struct Counters {
    pub rejected_rx: AtomicU64,
    pub rejected_tx: AtomicU64,
    pub fault_dropped: AtomicU64,
    pub fault_direct_checks: AtomicU64,
}
pub struct Boundary {
    local: Ipv4Addr,
    scope: uuid::Uuid,
    server: Option<SocketAddr>,
    peers: RwLock<Option<Vec<(SocketAddr, crate::direct_policy::Kind)>>>,
    deadline: Instant,
    stopped: AtomicBool,
    drop_direct: bool,
    drop_turn: bool,
    pub counters: Counters,
}
impl fmt::Debug for Boundary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DirectBoundary([REDACTED])")
    }
}
impl Boundary {
    pub fn new(grant: &DirectGrant, now: u64) -> Result<Arc<Self>, String> {
        if !grant.current(now) {
            return Err("DIRECT_BOUNDARY_EXPIRED".into());
        }
        Self::build(
            grant.local(),
            grant.server().map(Into::into),
            Duration::from_secs(grant.expires_at() - now),
            grant.scope(),
        )
    }
    fn build(
        local: Ipv4Addr,
        server: Option<SocketAddr>,
        ttl: Duration,
        scope: uuid::Uuid,
    ) -> Result<Arc<Self>, String> {
        if ttl.is_zero() || ttl > Duration::from_secs(600) {
            return Err("DIRECT_BOUNDARY_TTL".into());
        }
        Ok(Arc::new(Self {
            local,
            scope,
            server,
            peers: RwLock::new(None),
            deadline: Instant::now() + ttl,
            stopped: AtomicBool::new(false),
            drop_direct: false,
            drop_turn: false,
            counters: Counters::default(),
        }))
    }
    #[cfg(all(test, feature = "v2-public-video-wss"))]
    pub(crate) fn loopback_fixture(server: SocketAddr, room: uuid::Uuid) -> Arc<Self> {
        Self::build(
            Ipv4Addr::LOCALHOST,
            Some(server),
            Duration::from_secs(120),
            room,
        )
        .unwrap()
    }

    /// Only the operator diagnostic sets packet-loss faults; product construction has none.
    pub(crate) fn diagnostic(
        grant: &DirectGrant,
        now: u64,
        drop_direct: bool,
        drop_turn: bool,
    ) -> Result<Arc<Self>, String> {
        let mut b = Self::new(grant, now)?;
        let m = Arc::get_mut(&mut b).ok_or("DIRECT_BOUNDARY_SHARED")?;
        m.drop_direct = drop_direct;
        m.drop_turn = drop_turn;
        Ok(b)
    }
    pub fn install_peer(&self, checked: VerifiedCandidates) -> Result<(), String> {
        if !self.active() || checked.scope != self.scope {
            self.stop();
            return Err("DIRECT_BOUNDARY_EXPIRED".into());
        }
        let mut peers = self.peers.write().map_err(|_| "DIRECT_BOUNDARY_LOCK")?;
        if peers.is_some() {
            self.stop();
            return Err("DIRECT_PEER_ALREADY_INSTALLED".into());
        }
        *peers = Some(
            checked
                .values
                .iter()
                .map(|c| (SocketAddr::V4(c.address), c.kind))
                .collect(),
        );
        Ok(())
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    #[cfg(feature = "v2-public-video-wss")]
    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }
    fn active(&self) -> bool {
        !self.stopped.load(Ordering::Acquire) && Instant::now() < self.deadline
    }
    fn allowed(&self, addr: SocketAddr) -> bool {
        self.active()
            && (Some(addr) == self.server
                || self.peers.read().ok().is_some_and(|p| {
                    p.as_ref()
                        .is_some_and(|p| p.iter().any(|(a, _)| *a == addr))
                }))
    }
    #[cfg(all(windows, feature = "p006-session-probe"))]
    pub(crate) fn drops_direct(&self) -> bool {
        self.drop_direct
    }

    fn record_dropped_direct_check(&self, addr: SocketAddr, data: &[u8]) {
        use crate::direct_policy::Kind;
        if !self.drop_direct
            || Some(addr) == self.server
            || data.len() < 20
            || data[..2] != [0, 1]
            || data[4..8] != [0x21, 0x12, 0xa4, 0x42]
        {
            return;
        }
        let signed_direct = self.peers.read().ok().is_some_and(|p| {
            p.as_ref().is_some_and(|ps| {
                ps.iter()
                    .any(|(a, kind)| *a == addr && matches!(kind, Kind::Host | Kind::Srflx))
                    && !ps
                        .iter()
                        .any(|(a, kind)| *a == addr && *kind == Kind::Relay)
            })
        });
        if signed_direct {
            self.counters
                .fault_direct_checks
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    fn dropped(&self, addr: SocketAddr) -> bool {
        if Some(addr) == self.server {
            self.drop_turn
        } else {
            self.drop_direct
        }
    }
}
#[derive(Debug)]
pub struct DirectRuntime {
    pub inner: Arc<dyn Runtime>,
    pub boundary: Arc<Boundary>,
}
impl Runtime for DirectRuntime {
    fn spawn(&self, f: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        self.inner.spawn(f)
    }
    fn spawn_reactor(
        &self,
        n: usize,
        f: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Box<dyn JoinHandle> {
        self.inner.spawn_reactor(n, f)
    }
    fn wrap_udp_socket(&self, s: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        if s.local_addr()?.ip() != IpAddr::V4(self.boundary.local) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "DIRECT_BIND_MISMATCH",
            ));
        }
        Ok(Arc::new(Socket {
            inner: self.inner.wrap_udp_socket(s)?,
            boundary: self.boundary.clone(),
        }))
    }
    fn wrap_tcp_listener(&self, _: std::net::TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "DIRECT_TCP_DISABLED",
        ))
    }
    fn connect_tcp<'a>(
        &'a self,
        _: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "DIRECT_TCP_DISABLED",
            ))
        })
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let a = host.parse::<SocketAddr>().map_err(|_| {
                io::Error::new(io::ErrorKind::PermissionDenied, "DIRECT_DNS_DISABLED")
            })?;
            if !self.boundary.allowed(a) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "DIRECT_ENDPOINT_REJECTED",
                ));
            }
            Ok(vec![a])
        })
    }
    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.inner.sleep(d)
    }
    fn interval(&self, d: Duration) -> Box<dyn AsyncInterval> {
        self.inner.interval(d)
    }
    fn block_on(&self, f: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.inner.block_on(f)
    }
}
#[derive(Debug)]
struct Socket {
    inner: Arc<dyn AsyncUdpSocket>,
    boundary: Arc<Boundary>,
}
impl AsyncUdpSocket for Socket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn poll_send(&self, cx: &mut Context<'_>, t: &Transmit<'_>) -> Poll<io::Result<usize>> {
        if !self.boundary.allowed(t.destination)
            || t.src_ip
                .is_some_and(|p| p != IpAddr::V4(self.boundary.local))
        {
            self.boundary
                .counters
                .rejected_tx
                .fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "DIRECT_ENDPOINT_REJECTED",
            )));
        }
        if self.boundary.dropped(t.destination) {
            self.boundary
                .record_dropped_direct_check(t.destination, t.contents);
            self.boundary
                .counters
                .fault_dropped
                .fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Ok(t.contents.len()));
        }
        self.inner.poll_send(cx, t)
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if !self.boundary.active() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "DIRECT_LEASE_EXPIRED",
            )));
        }
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(io::ErrorKind::InvalidInput.into()));
        }
        for _ in 0..32 {
            match self.inner.poll_recv(cx, &mut bufs[..1], &mut meta[..1]) {
                Poll::Ready(Ok(n)) if n > 0 && !self.boundary.allowed(meta[0].addr) => {
                    self.boundary
                        .counters
                        .rejected_rx
                        .fetch_add(1, Ordering::Relaxed);
                }
                Poll::Ready(Ok(n)) if n > 0 && self.boundary.dropped(meta[0].addr) => {
                    self.boundary
                        .counters
                        .fault_dropped
                        .fetch_add(1, Ordering::Relaxed);
                }
                result => return result,
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
    fn max_gso_segments(&self) -> usize {
        self.inner.max_gso_segments()
    }
    fn max_gro_segments(&self) -> usize {
        self.inner.max_gro_segments()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::direct_policy::{Candidate, Kind};
    #[tokio::test]
    async fn exact_udp_peer_atomic_install_revoke_and_faults() {
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let other = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        // Test-private loopback boundary, unavailable through a public DirectGrant.
        let b = Boundary::build(
            Ipv4Addr::LOCALHOST,
            None,
            Duration::from_secs(5),
            uuid::Uuid::new_v4(),
        )
        .unwrap();
        let rt = DirectRuntime {
            inner: webrtc::runtime::default_runtime().unwrap(),
            boundary: b.clone(),
        };
        let raw = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        raw.set_nonblocking(true).unwrap();
        let socket = rt.wrap_udp_socket(raw).unwrap();
        let address = peer.local_addr().unwrap();
        assert!(socket.send_to(b"denied", address).await.is_err());
        let checked = VerifiedCandidates {
            scope: b.scope,
            values: vec![Candidate {
                address: match address {
                    SocketAddr::V4(a) => a,
                    _ => unreachable!(),
                },
                kind: Kind::Host,
            }],
        };
        b.install_peer(checked).unwrap();
        other
            .send_to(b"bad", socket.local_addr().unwrap())
            .await
            .unwrap();
        let mut bytes = [0; 32];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), socket.recv_from(&mut bytes))
                .await
                .is_err()
        );
        assert_eq!(b.counters.rejected_rx.load(Ordering::Relaxed), 1);
        peer.send_to(b"good", socket.local_addr().unwrap())
            .await
            .unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes[..n], b"good");
        assert_eq!(b.counters.rejected_rx.load(Ordering::Relaxed), 1);
        assert!(socket
            .send_to(b"bad", other.local_addr().unwrap())
            .await
            .is_err());
        assert!(b
            .install_peer(VerifiedCandidates {
                values: vec![],
                scope: b.scope
            })
            .is_err());
        assert!(socket.send_to(b"stopped", address).await.is_err());
        assert!(rt.resolve_host("host.local:3478").await.is_err());
        assert!(rt.connect_tcp(address).await.is_err());
        assert!(rt
            .wrap_udp_socket(std::net::UdpSocket::bind("0.0.0.0:0").unwrap())
            .is_err());
    }
    #[tokio::test]
    async fn sender_fault_drops_signed_direct_checks_but_preserves_the_turn_socket() {
        let turn = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let relay = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server = turn.local_addr().unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let relay_addr = relay.local_addr().unwrap();
        let mut boundary = Boundary::build(
            Ipv4Addr::LOCALHOST,
            Some(server),
            Duration::from_secs(5),
            uuid::Uuid::new_v4(),
        )
        .unwrap();
        assert!(!boundary.dropped(peer_addr));
        Arc::get_mut(&mut boundary).unwrap().drop_direct = true;
        let candidate = |a, kind| match a {
            SocketAddr::V4(address) => Candidate { address, kind },
            _ => unreachable!(),
        };
        boundary
            .install_peer(VerifiedCandidates {
                scope: boundary.scope,
                values: vec![
                    candidate(peer_addr, Kind::Srflx),
                    candidate(relay_addr, Kind::Relay),
                ],
            })
            .unwrap();
        let rt = DirectRuntime {
            inner: webrtc::runtime::default_runtime().unwrap(),
            boundary: boundary.clone(),
        };
        let raw = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        raw.set_nonblocking(true).unwrap();
        let socket = rt.wrap_udp_socket(raw).unwrap();
        let mut binding = [0u8; 20];
        binding[1] = 1;
        binding[4..8].copy_from_slice(&[0x21, 0x12, 0xa4, 0x42]);
        assert_eq!(
            socket.send_to(&binding, peer_addr).await.unwrap(),
            binding.len()
        );
        assert!(peer.try_recv_from(&mut [0; 32]).is_err());
        socket.send_to(&binding, relay_addr).await.unwrap();
        socket.send_to(b"not a binding", peer_addr).await.unwrap();
        assert_eq!(boundary.counters.fault_dropped.load(Ordering::Relaxed), 3);
        assert_eq!(
            boundary
                .counters
                .fault_direct_checks
                .load(Ordering::Relaxed),
            1
        );
        socket.send_to(&binding, server).await.unwrap();
        let mut buf = [0; 32];
        let (n, _) = tokio::time::timeout(Duration::from_secs(1), turn.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], &binding);
        assert_eq!(
            boundary
                .counters
                .fault_direct_checks
                .load(Ordering::Relaxed),
            1
        );
        boundary.stop();
        assert!(socket.send_to(&binding, server).await.is_err());
    }

    #[test]
    fn foreign_session_and_expired_grants_fail_closed() {
        let grant = DirectGrant::local_diagnostic("192.168.1.2".parse().unwrap(), 1300, true, 1000)
            .unwrap();
        assert!(Boundary::new(&grant, 1300).is_err());
        let boundary = Boundary::new(&grant, 1000).unwrap();
        assert!(boundary
            .install_peer(VerifiedCandidates {
                values: vec![],
                scope: uuid::Uuid::new_v4()
            })
            .is_err());
        assert!(!boundary.active());
        let expired = Boundary::build(
            Ipv4Addr::LOCALHOST,
            None,
            Duration::from_millis(1),
            uuid::Uuid::new_v4(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(2));
        assert!(!expired.active());
    }
}
