//! Two-allocation TURN fixture for tests only. Real UDP is confined to one
//! IPv4 loopback socket and the two authenticated loopback peers. Relayed
//! addresses are labels embedded inside TURN messages, never OS destinations.

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
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    net::UdpSocket,
    task::JoinHandle,
    time::{interval, Instant},
};
use uuid::Uuid;
use webrtc::peer_connection::RTCIceServer;

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
}

pub(super) struct FixtureTurn {
    address: SocketAddrV4,
    username: String,
    credential: String,
    statistics: Arc<Mutex<TurnFixtureStats>>,
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
        let actor = FixtureState {
            nonce: Uuid::new_v4().simple().to_string(),
            username: username.clone(),
            key,
            crypto,
            lifetime: Duration::from_secs(lifetime_seconds.into()),
            allocations: HashMap::with_capacity(2),
            statistics: statistics.clone(),
        };
        let task = tokio::spawn(actor.run(udp));
        Ok(Self {
            address,
            username,
            credential,
            statistics,
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
            // STUN remains simulated too; expose the loopback reflector's
            // actual observation, which is not used in the relay-only test.
            return self.reply(
                source,
                &message,
                CLASS_SUCCESS_RESPONSE,
                false,
                vec![Box::new(XorMappedAddress {
                    ip: source.ip(),
                    port: source.port(),
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
                    ip: source.ip(),
                    port: source.port(),
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
