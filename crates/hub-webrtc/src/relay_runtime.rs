//! Physical TURN endpoint guard, separate from logical relay-candidate validation.
use crate::pinned_runtime::{PinnedRuntime, SocketCounters};
use std::{
    future::Future,
    io::{self, IoSliceMut},
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime,
    Transmit,
};
#[derive(Debug)]
pub struct RelayRuntime {
    pub(crate) base: PinnedRuntime,
    pub(crate) endpoint: SocketAddr,
    pub(crate) deadline: Instant,
    pub(crate) round_deadline: Option<Arc<OnceLock<Instant>>>,
    pub(crate) stopped: Arc<AtomicBool>,
}
impl Runtime for RelayRuntime {
    fn spawn(&self, f: Pin<Box<dyn Future<Output = ()> + Send>>) -> Box<dyn JoinHandle> {
        self.base.spawn(f)
    }
    fn spawn_reactor(
        &self,
        n: usize,
        f: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Box<dyn JoinHandle> {
        self.base.spawn_reactor(n, f)
    }
    fn wrap_udp_socket(&self, s: std::net::UdpSocket) -> io::Result<Arc<dyn AsyncUdpSocket>> {
        Ok(Arc::new(RelaySocket {
            inner: self.base.wrap_udp_socket(s)?,
            endpoint: self.endpoint,
            deadline: self.deadline,
            round_deadline: self.round_deadline.clone(),
            stopped: self.stopped.clone(),
            counters: self.base.counters.clone(),
        }))
    }
    fn wrap_tcp_listener(&self, s: std::net::TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        self.base.wrap_tcp_listener(s)
    }
    fn connect_tcp<'a>(
        &'a self,
        s: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        self.base.connect_tcp(s)
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let addr = host.parse::<SocketAddr>().map_err(|_| {
                io::Error::new(io::ErrorKind::PermissionDenied, "RELAY_DNS_DISABLED")
            })?;
            if addr != self.endpoint {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "RELAY_ENDPOINT_REJECTED",
                ));
            }
            Ok(vec![addr])
        })
    }
    fn sleep(&self, d: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.base.sleep(d)
    }
    fn interval(&self, d: Duration) -> Box<dyn AsyncInterval> {
        self.base.interval(d)
    }
    fn block_on(&self, f: Pin<Box<dyn Future<Output = ()> + '_>>) {
        self.base.block_on(f)
    }
}
#[derive(Debug)]
struct RelaySocket {
    inner: Arc<dyn AsyncUdpSocket>,
    endpoint: SocketAddr,
    deadline: Instant,
    round_deadline: Option<Arc<OnceLock<Instant>>>,
    stopped: Arc<AtomicBool>,
    counters: Arc<SocketCounters>,
}
impl RelaySocket {
    fn active(&self) -> io::Result<()> {
        let deadline = self
            .round_deadline
            .as_ref()
            .and_then(|d| d.get())
            .copied()
            .map_or(self.deadline, |d| self.deadline.min(d));
        if self.stopped.load(Ordering::Acquire) || Instant::now() >= deadline {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "RELAY_LEASE_EXPIRED",
            ))
        } else {
            Ok(())
        }
    }
}
impl AsyncUdpSocket for RelaySocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn poll_send(&self, cx: &mut Context<'_>, t: &Transmit<'_>) -> Poll<io::Result<usize>> {
        if let Err(e) = self.active() {
            return Poll::Ready(Err(e));
        }
        if t.destination != self.endpoint {
            self.counters.rejected_tx.fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "RELAY_ENDPOINT_REJECTED",
            )));
        }
        self.inner.poll_send(cx, t)
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if let Err(e) = self.active() {
            return Poll::Ready(Err(e));
        }
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(io::ErrorKind::InvalidInput.into()));
        }
        for _ in 0..32 {
            match self.inner.poll_recv(cx, &mut bufs[..1], &mut meta[..1]) {
                Poll::Ready(Ok(n)) if n > 0 && meta[0].addr != self.endpoint => {
                    self.counters.rejected_rx.fetch_add(1, Ordering::Relaxed);
                }
                r => return r,
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
    use std::net::IpAddr;
    #[tokio::test]
    async fn tightening_the_outer_socket_invalidates_queued_access_without_actor_stop() {
        let target = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let endpoint = target.local_addr().unwrap();
        let deadline = Arc::new(OnceLock::new());
        let local = endpoint.ip();
        let runtime = RelayRuntime {
            base: PinnedRuntime {
                inner: webrtc::runtime::default_runtime().unwrap(),
                local,
                peer: local,
                counters: Arc::default(),
            },
            endpoint,
            deadline: Instant::now() + Duration::from_secs(90),
            round_deadline: Some(deadline.clone()),
            stopped: Arc::default(),
        };
        let raw = std::net::UdpSocket::bind((local, 0)).unwrap();
        raw.set_nonblocking(true).unwrap();
        let socket = runtime.wrap_udp_socket(raw).unwrap();
        assert!(socket.send_to(b"before", endpoint).await.is_ok());
        deadline.set(Instant::now()).unwrap();
        assert!(!runtime.stopped.load(Ordering::Acquire));
        assert_eq!(
            socket.send_to(b"after", endpoint).await.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        let mut buffer = [0; 32];
        assert_eq!(
            socket.recv_from(&mut buffer).await.unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[tokio::test]
    async fn real_local_udp_enforces_server_port_and_revocation() {
        let server = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let wrong = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let endpoint = server.local_addr().unwrap();
        let counters = Arc::new(SocketCounters::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let runtime = RelayRuntime {
            base: PinnedRuntime {
                inner: webrtc::runtime::default_runtime().unwrap(),
                local,
                peer: local,
                counters: counters.clone(),
            },
            endpoint,
            deadline: Instant::now() + Duration::from_secs(5),
            round_deadline: None,
            stopped: stopped.clone(),
        };
        let raw = std::net::UdpSocket::bind((local, 0)).unwrap();
        raw.set_nonblocking(true).unwrap();
        let socket = runtime.wrap_udp_socket(raw).unwrap();
        wrong
            .send_to(b"bad-port", socket.local_addr().unwrap())
            .await
            .unwrap();
        let mut buf = [0; 64];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), socket.recv_from(&mut buf))
                .await
                .is_err()
        );
        assert_eq!(counters.rejected_rx.load(Ordering::Relaxed), 1);
        server
            .send_to(b"approved", socket.local_addr().unwrap())
            .await
            .unwrap();
        let (n, source) = tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"approved");
        assert_eq!(source, endpoint);
        assert_eq!(counters.rejected_rx.load(Ordering::Relaxed), 1);
        socket.send_to(b"approved-client", endpoint).await.unwrap();
        let (n, _) = tokio::time::timeout(Duration::from_secs(1), server.recv_from(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"approved-client");
        assert!(socket
            .send_to(b"no", wrong.local_addr().unwrap())
            .await
            .is_err());
        stopped.store(true, Ordering::Release);
        assert!(socket.send_to(b"no", endpoint).await.is_err());
        assert!(socket.recv_from(&mut buf).await.is_err());
        assert!(runtime
            .resolve_host("unapproved.invalid:3478")
            .await
            .is_err());
        let expired = RelayRuntime {
            base: runtime.base,
            endpoint,
            deadline: Instant::now() - Duration::from_millis(1),
            round_deadline: None,
            stopped: Arc::new(AtomicBool::new(false)),
        };
        let raw = std::net::UdpSocket::bind((local, 0)).unwrap();
        raw.set_nonblocking(true).unwrap();
        assert!(expired
            .wrap_udp_socket(raw)
            .unwrap()
            .send_to(b"expired", endpoint)
            .await
            .is_err());
    }
}
