//! Integration-test-only RTC fixture. All physical packets stay on literal
//! IPv4 loopback. The public-looking relay tuples are only private test labels.
//! No arbitrary network destination, media or OS input API is exposed.
use crate::{
    public_fake_input::{ApprovedRelayPair, FakeInputProof},
    public_fake_input_pair, public_fake_input_sdp,
    public_fake_input_turn_fixture::{
        live_pair, wall, FixtureRuntime, Peer as InnerPeer, TurnFixtureConfig, LABEL, PROTOCOL,
        WAIT,
    },
    public_fake_input_turn_net as net,
};
use hub_identity::session::SessionRole;
use std::{
    io,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::{sleep, timeout, timeout_at, Instant};
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit},
    peer_connection::transport::SctpTransport,
    runtime::Runtime,
};

pub const RELAY_LABEL: Ipv4Addr = net::RELAY_LABEL;
pub const RELAY_PORTS: (u16, u16) = net::RELAY_PORTS;
const LEASE_SECONDS: u32 = 15;
const STEP: Duration = Duration::from_secs(5);
const MARKER: &str = "fixture:synthetic-admission-marker";
const ACK: &str = "fixture:synthetic-admission-ack";
const CLOSED: &str = "ADMISSION_RTC_CLOSED";
const STATE: &str = "ADMISSION_RTC_STATE";
const CHANNEL: &str = "ADMISSION_RTC_CHANNEL";
const PAIR: &str = "ADMISSION_RTC_PAIR";
type Result<T> = std::result::Result<T, &'static str>;

pub struct FixtureTurn {
    net: net::FixtureTurn,
    active: Arc<AtomicBool>,
    roles: AtomicU8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub allocations_created: usize,
    pub permissions_created: usize,
    pub forwarded_packets: usize,
    pub released: usize,
    pub expired: usize,
    pub active: usize,
    pub direct_forwarded: usize,
}

impl FixtureTurn {
    pub async fn start() -> io::Result<Self> {
        let net = net::FixtureTurn::start(LEASE_SECONDS).await?;
        net.block_direct(true);
        Ok(Self {
            net,
            active: Arc::new(AtomicBool::new(true)),
            roles: AtomicU8::new(0),
        })
    }

    pub fn server(&self) -> SocketAddrV4 {
        self.net.server()
    }

    pub fn stats(&self) -> Stats {
        let stats = self.net.stats();
        Stats {
            allocations_created: stats.allocations_created,
            permissions_created: stats.permissions_created,
            forwarded_packets: stats.forwarded_packets,
            released: stats.released,
            expired: stats.expired,
            active: stats.active,
            direct_forwarded: stats.direct_forwarded,
        }
    }

    /// Abort the fixture task and verify its local socket has been released.
    pub async fn close(self) -> Result<()> {
        let endpoint = self.server();
        drop(self);
        timeout(Duration::from_secs(1), async move {
            loop {
                match UdpSocket::bind(endpoint) {
                    Ok(_) => return Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                        sleep(Duration::from_millis(5)).await;
                    }
                    Err(_) => return Err(CLOSED),
                }
            }
        })
        .await
        .map_err(|_| CLOSED)?
    }
}

impl Drop for FixtureTurn {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

pub struct Peer {
    role: SessionRole,
    turn_active: Arc<AtomicBool>,
    live: Option<InnerPeer>,
    host_boot: Option<TurnFixtureConfig>,
    outgoing: Option<Arc<dyn DataChannel>>,
    approved: Arc<Mutex<Option<ApprovedRelayPair>>>,
    remote_started: bool,
    remote_installed: bool,
    local_gathered: bool,
    claimed: bool,
}

impl Peer {
    pub async fn start(turn: &FixtureTurn, role: SessionRole) -> Result<Self> {
        let mask = match role {
            SessionRole::Host => 1,
            SessionRole::Viewer => 2,
        };
        if turn.roles.fetch_or(mask, Ordering::AcqRel) & mask != 0 {
            return Err(STATE);
        }
        let config = TurnFixtureConfig::new(
            turn.server().port(),
            turn.net.username(),
            turn.net.credential(),
        )?;
        let mut result = Self {
            role,
            turn_active: turn.active.clone(),
            live: None,
            host_boot: None,
            outgoing: None,
            approved: Arc::default(),
            remote_started: false,
            remote_installed: false,
            local_gathered: false,
            claimed: false,
        };
        match role {
            SessionRole::Host => result.host_boot = Some(config),
            SessionRole::Viewer => {
                let peer = timeout(STEP, InnerPeer::start(&config))
                    .await
                    .map_err(|_| STATE)?;
                let channel = timeout(
                    STEP,
                    peer.pc.create_data_channel(
                        LABEL,
                        Some(RTCDataChannelInit {
                            protocol: PROTOCOL.into(),
                            ..Default::default()
                        }),
                    ),
                )
                .await
                .map_err(|_| CHANNEL)?
                .map_err(|_| CHANNEL)?;
                result.live = Some(peer);
                result.outgoing = Some(channel);
            }
        }
        Ok(result)
    }

    pub async fn local(&mut self) -> Result<String> {
        self.connected_fixture()?;
        if self.local_gathered || (self.role == SessionRole::Host && !self.remote_installed) {
            return Err(STATE);
        }
        let peer = self.live.as_mut().ok_or(CLOSED)?;
        let sdp = timeout(WAIT, peer.local(self.role == SessionRole::Host))
            .await
            .map_err(|_| STATE)?;
        self.local_gathered = true;
        Ok(sdp)
    }

    /// Verify the exact remote bytes before any remote candidate can enter RTC.
    /// A host does not even construct ICE until this one-use proof succeeds.
    pub async fn remote(
        &mut self,
        sdp: &str,
        signature: &[u8; 64],
        proof: &mut FakeInputProof<'_>,
    ) -> Result<()> {
        self.connected_fixture()?;
        if self.remote_started || (self.role == SessionRole::Viewer && !self.local_gathered) {
            return Err(STATE);
        }
        self.remote_started = true;
        let answer = self.role == SessionRole::Viewer;
        let tuple = public_fake_input_sdp::validate(
            sdp,
            RELAY_LABEL,
            RELAY_PORTS.0,
            RELAY_PORTS.1,
            answer,
        )?;
        proof.verify(sdp, signature, wall()).map_err(|e| e.code())?;
        if self.role == SessionRole::Host {
            let config = self.host_boot.take().ok_or(CLOSED)?;
            self.live = Some(
                timeout(STEP, InnerPeer::start(&config))
                    .await
                    .map_err(|_| STATE)?,
            );
        }
        let peer = self.live.as_ref().ok_or(CLOSED)?;
        timeout(STEP, peer.remote(sdp, answer, tuple))
            .await
            .map_err(|_| STATE)?;
        self.remote_installed = true;
        Ok(())
    }

    fn connected_fixture(&self) -> Result<()> {
        self.turn_active
            .load(Ordering::Acquire)
            .then_some(())
            .ok_or(CLOSED)
    }

    pub async fn open(&mut self) -> Result<Channel> {
        self.connected_fixture()?;
        if self.claimed || !self.remote_installed || !self.local_gathered {
            return Err(STATE);
        }
        self.claimed = true;
        const DCEP: &str = "ADMISSION_RTC_DCEP";
        const OPEN: &str = "ADMISSION_RTC_CHANNEL_OPEN";
        const PROFILE: &str = "ADMISSION_RTC_CHANNEL_PROFILE";
        let deadline = Instant::now() + WAIT;
        let peer = self.live.as_mut().ok_or(CLOSED)?;
        let channel = match self.role {
            SessionRole::Host => timeout_at(deadline, peer.channels.recv())
                .await
                .map_err(|_| DCEP)?
                .ok_or(DCEP)?,
            SessionRole::Viewer => self.outgoing.take().ok_or(DCEP)?,
        };
        timeout_at(deadline, async {
            loop {
                match channel.poll().await {
                    Some(DataChannelEvent::OnOpen) => break,
                    Some(DataChannelEvent::OnClose | DataChannelEvent::OnError) | None => {
                        return Err(OPEN)
                    }
                    _ => {}
                }
            }
            if channel.label().await.map_err(|_| PROFILE)? != LABEL
                || channel.protocol().await.map_err(|_| PROFILE)? != PROTOCOL
                || !channel.ordered().await.map_err(|_| PROFILE)?
                || channel.negotiated().await.map_err(|_| PROFILE)?
                || channel
                    .max_retransmits()
                    .await
                    .map_err(|_| PROFILE)?
                    .is_some()
                || channel
                    .max_packet_life_time()
                    .await
                    .map_err(|_| PROFILE)?
                    .is_some()
            {
                return Err(PROFILE);
            }
            Ok(())
        })
        .await
        .map_err(|_| OPEN)??;
        let sctp = timeout(STEP, peer.pc.sctp())
            .await
            .map_err(|_| PAIR)?
            .ok_or(PAIR)?;
        Ok(Channel {
            sctp,
            channel,
            role: self.role,
            runtime: peer.runtime.clone(),
            turn_active: self.turn_active.clone(),
            approved: self.approved.clone(),
            step: 0,
        })
    }

    pub async fn verify_pair(&self, proof: &mut FakeInputProof<'_>) -> Result<()> {
        *self.approved.lock().map_err(|_| CLOSED)? = None;
        self.connected_fixture()?;
        let peer = self.live.as_ref().ok_or(CLOSED)?;
        let sctp = timeout(STEP, peer.pc.sctp())
            .await
            .map_err(|_| PAIR)?
            .ok_or(PAIR)?;
        let actual = timeout(STEP, live_pair(&*sctp)).await.map_err(|_| PAIR)??;
        proof.verify_selected_relay_pair(Some(&actual), wall())?;
        let approved = proof.approved_relay_pair(wall()).ok_or(PAIR)?;
        let mut state = self.approved.lock().map_err(|_| CLOSED)?;
        *state = Some(approved);
        Ok(())
    }

    pub async fn close(mut self) -> Result<()> {
        if let Some(peer) = self.live.take() {
            peer.revoke();
            timeout(STEP, peer.pc.close())
                .await
                .map_err(|_| CLOSED)?
                .map_err(|_| CLOSED)?;
            timeout(STEP, peer.assert_runtime_revoked())
                .await
                .map_err(|_| CLOSED)?;
        }
        Ok(())
    }
}

/// Only one hardcoded viewer marker and one hardcoded host ACK are possible.
/// No raw datachannel, event payload or arbitrary marker is exposed.
pub struct Channel {
    sctp: Arc<dyn SctpTransport>,
    channel: Arc<dyn DataChannel>,
    role: SessionRole,
    runtime: Arc<FixtureRuntime>,
    turn_active: Arc<AtomicBool>,
    approved: Arc<Mutex<Option<ApprovedRelayPair>>>,
    step: u8,
}

impl Channel {
    async fn check(&self) -> Result<()> {
        let pair = self.approved.lock().map_err(|_| CLOSED)?.ok_or(PAIR)?;
        if !self.turn_active.load(Ordering::Acquire)
            || !self.runtime.allows_turn_peer(pair.remote().into())
        {
            return Err(CLOSED);
        }
        let actual = live_pair(&*self.sctp).await?;
        public_fake_input_pair::verify(pair.local(), pair.remote(), Some(&actual))
    }

    async fn transfer(
        &mut self,
        role: SessionRole,
        step: u8,
        text: &str,
        send: bool,
    ) -> Result<()> {
        if self.role != role || self.step != step {
            return Err(STATE);
        }
        // Consume the one-shot transition before any I/O or cancellation.
        self.step = u8::MAX;
        timeout(STEP, async {
            self.check().await?;
            if send {
                self.channel
                    .try_send_text(text)
                    .await
                    .map_err(|_| CHANNEL)?;
            } else {
                loop {
                    match self.channel.poll().await {
                        Some(DataChannelEvent::OnMessage(message))
                            if message.is_string && message.data.as_ref() == text.as_bytes() =>
                        {
                            break;
                        }
                        Some(
                            DataChannelEvent::OnBufferedAmountHigh
                            | DataChannelEvent::OnBufferedAmountLow,
                        ) => {}
                        _ => return Err(CHANNEL),
                    }
                }
            }
            self.check().await
        })
        .await
        .map_err(|_| CHANNEL)??;
        self.step = step + 1;
        Ok(())
    }

    pub async fn send_marker(&mut self) -> Result<()> {
        self.transfer(SessionRole::Viewer, 0, MARKER, true).await
    }

    pub async fn recv_marker(&mut self) -> Result<()> {
        self.transfer(SessionRole::Host, 0, MARKER, false).await
    }

    pub async fn send_ack(&mut self) -> Result<()> {
        self.transfer(SessionRole::Host, 1, ACK, true).await
    }

    pub async fn recv_ack(&mut self) -> Result<()> {
        self.transfer(SessionRole::Viewer, 1, ACK, false).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_slot_is_one_use_and_cannot_start_ice_before_a_proven_offer() {
        let turn = FixtureTurn::start().await.unwrap();
        assert_eq!(*turn.server().ip(), Ipv4Addr::LOCALHOST);
        let mut host = Peer::start(&turn, SessionRole::Host).await.unwrap();
        assert!(matches!(host.local().await, Err(STATE)));
        assert!(matches!(host.open().await, Err(STATE)));
        assert_eq!(turn.stats().allocations_created, 0);
        assert!(matches!(
            Peer::start(&turn, SessionRole::Host).await,
            Err(STATE)
        ));
        host.close().await.unwrap();
        turn.close().await.unwrap();
    }
}
