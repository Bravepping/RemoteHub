//! Restrict the library socket boundary, including peer-reflexive ICE, to one explicitly selected IP.
use std::{
    fmt::Debug,
    future::Future,
    io::{self, IoSliceMut},
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::Duration,
};
use webrtc::runtime::{
    AsyncInterval, AsyncTcpListener, AsyncTcpStream, AsyncUdpSocket, JoinHandle, RecvMeta, Runtime,
    Transmit,
};

#[derive(Debug, Default)]
pub struct SocketCounters {
    pub rejected_rx: AtomicU64,
    pub rejected_tx: AtomicU64,
}
#[derive(Debug)]
pub struct PinnedRuntime {
    pub inner: Arc<dyn Runtime>,
    pub local: IpAddr,
    pub peer: IpAddr,
    pub counters: Arc<SocketCounters>,
}
impl Runtime for PinnedRuntime {
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
        if s.local_addr()?.ip() != self.local {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "LAN_BIND_MISMATCH",
            ));
        }
        Ok(Arc::new(PinnedSocket {
            inner: self.inner.wrap_udp_socket(s)?,
            local: self.local,
            peer: self.peer,
            counters: self.counters.clone(),
        }))
    }
    fn wrap_tcp_listener(&self, _: std::net::TcpListener) -> io::Result<Arc<dyn AsyncTcpListener>> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "WEBRTC_TCP_DISABLED",
        ))
    }
    fn connect_tcp<'a>(
        &'a self,
        _: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = io::Result<Arc<dyn AsyncTcpStream>>> + Send + 'a>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "WEBRTC_TCP_DISABLED",
            ))
        })
    }
    fn resolve_host<'a>(
        &'a self,
        host: &'a str,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<SocketAddr>>> + Send + 'a>> {
        Box::pin(async move {
            let addr = host
                .parse::<SocketAddr>()
                .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "DNS_DISABLED"))?;
            if addr.ip() != self.local && addr.ip() != self.peer {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "OFF_PEER_RESOLUTION",
                ));
            }
            Ok(vec![addr])
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
struct PinnedSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    local: IpAddr,
    peer: IpAddr,
    counters: Arc<SocketCounters>,
}
impl AsyncUdpSocket for PinnedSocket {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn poll_send(&self, cx: &mut Context<'_>, tx: &Transmit<'_>) -> Poll<io::Result<usize>> {
        if tx.destination.ip() != self.peer || tx.src_ip.is_some_and(|ip| ip != self.local) {
            self.counters.rejected_tx.fetch_add(1, Ordering::Relaxed);
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "OFF_PEER_UDP_SEND",
            )));
        }
        self.inner.poll_send(cx, tx)
    }
    fn max_gso_segments(&self) -> usize {
        self.inner.max_gso_segments()
    }
    fn max_gro_segments(&self) -> usize {
        self.inner.max_gro_segments()
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Err(io::ErrorKind::InvalidInput.into()));
        }
        for _ in 0..64 {
            match self.inner.poll_recv(cx, &mut bufs[..1], &mut meta[..1]) {
                Poll::Ready(Ok(n)) if n > 0 && meta[0].addr.ip() != self.peer => {
                    self.counters.rejected_rx.fetch_add(1, Ordering::Relaxed);
                }
                result => return result,
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn socket_boundary_drops_off_peer_datagrams_and_denies_other_destinations() {
        let local: IpAddr = "127.0.0.1".parse().unwrap();
        let counters = Arc::new(SocketCounters::default());
        let runtime = PinnedRuntime {
            inner: webrtc::runtime::default_runtime().unwrap(),
            local,
            peer: local,
            counters: counters.clone(),
        };
        let raw = std::net::UdpSocket::bind((local, 0)).unwrap();
        raw.set_nonblocking(true).unwrap();
        let socket = runtime.wrap_udp_socket(raw).unwrap();
        let address = socket.local_addr().unwrap();
        let bad = tokio::net::UdpSocket::bind("127.0.0.2:0").await.unwrap();
        let good = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        bad.send_to(b"off-peer", address).await.unwrap();
        let mut bytes = [0; 64];
        // Different UDP senders have no cross-socket arrival ordering guarantee.
        // Prove rejection before introducing the successful receive.
        assert!(
            tokio::time::timeout(Duration::from_millis(100), socket.recv_from(&mut bytes))
                .await
                .is_err()
        );
        assert_eq!(counters.rejected_rx.load(Ordering::Relaxed), 1);
        good.send_to(b"approved", address).await.unwrap();
        let (n, source) =
            tokio::time::timeout(Duration::from_secs(1), socket.recv_from(&mut bytes))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&bytes[..n], b"approved");
        assert_eq!(source.ip(), local);
        assert_eq!(counters.rejected_rx.load(Ordering::Relaxed), 1);
        assert_eq!(
            socket
                .send_to(b"no", "127.0.0.2:9".parse().unwrap())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(counters.rejected_tx.load(Ordering::Relaxed), 1);
        assert!(runtime.resolve_host("device.local:99").await.is_err());
        assert!(runtime.connect_tcp(address).await.is_err());
        let all = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
        assert!(runtime.wrap_udp_socket(all).is_err());
    }
}
