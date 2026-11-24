//! Two isolated role processes exchange signed SCTP descriptions over literal
//! loopback TLS/WSS. The separate TURN case adds one fixed SCTP round trip;
//! neither case calls media or input APIs.

use crate::{
    ice_policy::IceMode,
    public_fake_input::{FakeInputProof, FakeInputProofScope, FakeInputTestMode},
    public_fake_input_profile::SyntheticProfile,
    public_fake_input_turn_fixture::{opened, text, Peer, TurnFixtureConfig, LABEL, PROTOCOL},
    public_fake_input_turn_net::{FixtureTurn, RELAY_LABEL, RELAY_PORTS},
    public_fake_input_wss_fixture::{connect_role, FixtureSocket, FixtureWssServer},
    public_fake_input_wss_wire::{self as wire, Context, Frame, Kind},
    public_video::PublicVideoScope,
};
use hub_identity::{
    pairing::PairingCandidate,
    session::{SessionChallenge, SessionRole},
    Identity,
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4},
    process::{ExitCode, Stdio},
    sync::{mpsc, Arc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use uuid::Uuid;
use webrtc::data_channel::{DataChannel, RTCDataChannelInit};
use zeroize::Zeroizing;

const START: &str = "--two-process-loopback-only";
const START_TURN: &str = "--two-process-turn-loopback-only";
const HOST_WORKER: &str = "--internal-wss-host";
const VIEWER_WORKER: &str = "--internal-wss-viewer";
const TURN_HOST_WORKER: &str = "--internal-wss-turn-host";
const TURN_VIEWER_WORKER: &str = "--internal-wss-turn-viewer";
const PARENT_WAIT: Duration = Duration::from_secs(12);
const CHILD_WAIT: Duration = Duration::from_secs(15);
const MAX_CA: usize = 2048;
const PREFIX: [u8; 4] = *b"RHP1";
const CONNECTED: u8 = 0xA1;
const VERIFIED: u8 = 0xA2;
const RELAY_READY: u8 = 0xA3;
const MARKER_GO: u8 = 0xA4;
const RELAY_DONE: u8 = 0xA5;
const TURN_CRED: usize = 32;
const TURN_BOOT: usize = 2 + TURN_CRED * 2;
const MARKER: &str = "fixture:two-process-marker";
const ACK: &str = "fixture:two-process-marker-ack";
const CODE: usize = 68; // RH2- and 64 lowercase hex digits
const FINGERPRINT: usize = 64;
const CARD: usize = CODE + FINGERPRINT;
const HELLO: usize = PREFIX.len() + 1 + CARD;
// Version, role, port, expiry, room, generation, peer card, CA pin, DER length.
const BOOT: usize = PREFIX.len() + 1 + 2 + 8 + 16 + 32 + CARD + 32 + 2;
const MODE: FakeInputTestMode = FakeInputTestMode::Enabled;

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Signed,
    Turn,
}

fn role_byte(role: SessionRole) -> u8 {
    match role {
        SessionRole::Host => 1,
        SessionRole::Viewer => 2,
    }
}

fn wall() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| "clock")
}

struct PublicCard([u8; CARD]);

impl PublicCard {
    fn from_local(id: &Identity) -> Result<Self> {
        let public = id.public();
        let mut bytes = [0; CARD];
        bytes[..CODE].copy_from_slice(public.pairing_code().as_bytes());
        bytes[CODE..].copy_from_slice(public.fingerprint.as_bytes());
        Self::parse(bytes)
    }

    fn parse(bytes: [u8; CARD]) -> Result<Self> {
        let card = Self(bytes);
        let (key, fingerprint) = card.parts()?;
        if key.fingerprint() != fingerprint {
            return Err("public key/fingerprint mismatch");
        }
        Ok(card)
    }

    fn parts(&self) -> Result<(PairingCandidate, &str)> {
        let code = std::str::from_utf8(&self.0[..CODE]).map_err(|_| "public code")?;
        let key = PairingCandidate::parse(code).map_err(|_| "public code")?;
        let fp = std::str::from_utf8(&self.0[CODE..]).map_err(|_| "public fingerprint")?;
        Ok((key, fp))
    }
}

#[derive(Clone, Copy)]
struct RunScope {
    expires: u64,
    room: Uuid,
    generation: [u8; 32],
}

impl RunScope {
    fn new() -> Result<Self> {
        let mut generation = [0; 32];
        generation[..16].copy_from_slice(Uuid::new_v4().as_bytes());
        generation[16..].copy_from_slice(Uuid::new_v4().as_bytes());
        Ok(Self {
            expires: wall()?.checked_add(25).ok_or("clock")?,
            room: Uuid::new_v4(),
            generation,
        })
    }

    fn proof(self, port: u16, turn_port: u16, ca_sha256: [u8; 32]) -> FakeInputProofScope {
        // These public-looking labels are signed test metadata, never dialed.
        FakeInputProofScope {
            room: self.room,
            generation: self.generation,
            route: PublicVideoScope {
                wss: SocketAddrV4::new(RELAY_LABEL, port),
                ca_sha256,
                turn: SocketAddrV4::new(RELAY_LABEL, turn_port),
                relay_ports: RELAY_PORTS,
                mode: IceMode::RelayOnly,
                expires_at: self.expires,
            },
        }
    }
}

struct Worker {
    child: Child,
    boot: Option<ChildStdin>,
    status: ChildStdout,
    role: SessionRole,
}

impl Worker {
    async fn spawn(role: SessionRole, lane: Lane) -> Result<Self> {
        let exe = std::env::current_exe().map_err(|_| "self executable")?;
        let arg = match (role, lane) {
            (SessionRole::Host, Lane::Signed) => HOST_WORKER,
            (SessionRole::Viewer, Lane::Signed) => VIEWER_WORKER,
            (SessionRole::Host, Lane::Turn) => TURN_HOST_WORKER,
            (SessionRole::Viewer, Lane::Turn) => TURN_VIEWER_WORKER,
        };
        let mut child = Command::new(exe)
            .arg(arg)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| "worker spawn")?;
        match (child.stdin.take(), child.stdout.take()) {
            (Some(boot), Some(status)) => Ok(Self {
                child,
                boot: Some(boot),
                status,
                role,
            }),
            _ => {
                let _ = child.kill().await;
                Err("worker pipes")
            }
        }
    }

    async fn hello(&mut self) -> Result<PublicCard> {
        let mut bytes = [0; HELLO];
        self.status
            .read_exact(&mut bytes)
            .await
            .map_err(|_| "worker hello")?;
        if bytes[..4] != PREFIX || bytes[4] != role_byte(self.role) {
            return Err("worker hello role");
        }
        PublicCard::parse(bytes[5..].try_into().unwrap())
    }

    async fn bootstrap(
        &mut self,
        server: &FixtureWssServer,
        scope: RunScope,
        peer: &PublicCard,
        turn: Option<&FixtureTurn>,
    ) -> Result<()> {
        let ca = server.ca_der();
        if ca.is_empty() || ca.len() > MAX_CA {
            return Err("fixture CA length");
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(BOOT + ca.len() + TURN_BOOT));
        bytes.extend_from_slice(&PREFIX);
        bytes.push(role_byte(self.role));
        bytes.extend_from_slice(&server.port().to_be_bytes());
        bytes.extend_from_slice(&scope.expires.to_be_bytes());
        bytes.extend_from_slice(scope.room.as_bytes());
        bytes.extend_from_slice(&scope.generation);
        bytes.extend_from_slice(&peer.0);
        bytes.extend_from_slice(&server.ca_sha256());
        bytes.extend_from_slice(&(ca.len() as u16).to_be_bytes());
        bytes.extend_from_slice(ca);
        if let Some(turn) = turn {
            if *turn.server().ip() != Ipv4Addr::LOCALHOST
                || turn.username().len() != TURN_CRED
                || turn.credential().len() != TURN_CRED
            {
                return Err("fixture TURN bootstrap shape");
            }
            bytes.extend_from_slice(&turn.server().port().to_be_bytes());
            bytes.extend_from_slice(turn.username().as_bytes());
            bytes.extend_from_slice(turn.credential().as_bytes());
        }
        self.boot
            .as_mut()
            .ok_or("duplicate worker bootstrap")?
            .write_all(&bytes)
            .await
            .map_err(|_| "worker bootstrap")?;
        if turn.is_none() {
            self.close_boot().await?;
        }
        Ok(())
    }

    async fn close_boot(&mut self) -> Result<()> {
        let mut pipe = self.boot.take().ok_or("worker bootstrap not open")?;
        pipe.shutdown().await.map_err(|_| "worker bootstrap EOF")
    }

    async fn release_marker(&mut self) -> Result<()> {
        self.boot
            .as_mut()
            .ok_or("worker gate not open")?
            .write_u8(MARKER_GO)
            .await
            .map_err(|_| "worker marker gate")?;
        self.close_boot().await
    }

    async fn expect(&mut self, expected: u8) -> Result<()> {
        match self.status.read_u8().await {
            Ok(actual) if actual == expected => Ok(()),
            result => {
                let cause = match result {
                    Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => "eof",
                    Err(_) => "pipe_error",
                    Ok(_) => "unexpected_status",
                };
                let role = role_name(self.role);
                let phase = match expected {
                    CONNECTED => "wss_connect",
                    VERIFIED => "signed_sdp_close",
                    RELAY_READY => "turn_ready",
                    RELAY_DONE => "turn_done",
                    _ => "unknown",
                };
                eprintln!("fixture parent: role={role} phase={phase} {cause}");
                Err("worker status")
            }
        }
    }

    async fn finished(&mut self) -> Result<()> {
        let exit = self.child.wait().await.map_err(|_| "worker wait")?;
        if !exit.success() {
            return Err("worker exit");
        }
        match self.status.read(&mut [0]).await {
            Ok(0) => Ok(()),
            _ => Err("worker status trailing bytes"),
        }
    }
}

async fn run_parent(lane: Lane) -> Result<()> {
    let server = FixtureWssServer::start().await?;
    let turn = if lane == Lane::Turn {
        Some(start_turn(server.port()).await?)
    } else {
        None
    };
    let scope = RunScope::new()?;
    let mut host = Worker::spawn(SessionRole::Host, lane).await?;
    let mut viewer = match Worker::spawn(SessionRole::Viewer, lane).await {
        Ok(worker) => worker,
        Err(error) => {
            let _ = host.child.kill().await;
            return Err(error);
        }
    };
    let checked = timeout(PARENT_WAIT, async {
        let parent_id = std::process::id();
        if !matches!((host.child.id(), viewer.child.id()), (Some(host), Some(viewer))
            if host != viewer && host != parent_id && viewer != parent_id)
        {
            return Err("roles must be separate child processes");
        }
        let (host_card, viewer_card) = tokio::try_join!(host.hello(), viewer.hello())?;
        if host_card.0 == viewer_card.0 {
            return Err("roles must have separate identities");
        }

        // The fixture listener upgrades the host path before the viewer path.
        host.bootstrap(&server, scope, &viewer_card, turn.as_ref())
            .await?;
        host.expect(CONNECTED).await?;
        viewer
            .bootstrap(&server, scope, &host_card, turn.as_ref())
            .await?;
        viewer.expect(CONNECTED).await?;

        if let Some(turn) = turn.as_ref() {
            // Both isolated roles checked their actual relay paths before
            // either gets permission to send the fixed SCTP marker.
            tokio::try_join!(host.expect(RELAY_READY), viewer.expect(RELAY_READY))?;
            let before = turn.stats();
            if before.allocations_created != 2
                || before.active != 2
                || before.permissions_created < 2
                || before.direct_forwarded != 0
            {
                return Err("TURN allocations or direct path before marker");
            }
            tokio::try_join!(host.release_marker(), viewer.release_marker())?;
            tokio::try_join!(host.expect(RELAY_DONE), viewer.expect(RELAY_DONE))?;
            let after = turn.stats();
            if after.allocations_created != 2
                || after.forwarded_packets <= before.forwarded_packets
                || after.direct_forwarded != 0
            {
                return Err("TURN forwarding or direct path after marker");
            }
        } else {
            tokio::try_join!(host.expect(VERIFIED), viewer.expect(VERIFIED))?;
        }
        tokio::try_join!(host.finished(), viewer.finished())?;
        server.wait().await?;
        if let Some(turn) = turn {
            stop_turn(turn).await?;
        }
        Ok(())
    })
    .await
    .unwrap_or(Err("process check deadline"));
    if checked.is_err() {
        // kill() waits/reaps, even if a child was blocked on a private pipe.
        let _ = tokio::join!(host.child.kill(), viewer.child.kill());
    }
    checked
}

async fn start_turn(wss_port: u16) -> Result<FixtureTurn> {
    if wss_port < 1024 {
        return Err("fixture WSS port");
    }
    // TCP and UDP ephemeral assignments can collide numerically. Bound the
    // retries so the two separately bound fixture ports remain distinct.
    for _ in 0..3 {
        let turn = FixtureTurn::start(20)
            .await
            .map_err(|_| "loopback TURN fixture")?;
        if turn.server().port() >= 1024 && turn.server().port() != wss_port {
            turn.block_direct(true);
            return Ok(turn);
        }
    }
    Err("distinct fixture TURN port")
}

async fn stop_turn(turn: FixtureTurn) -> Result<()> {
    let endpoint = turn.server();
    drop(turn);
    // This confirms the local UDP listener exited, not deletion of remote
    // allocations. The worker's traffic gate is revoked before RTC close.
    timeout(Duration::from_secs(1), async {
        loop {
            match tokio::net::UdpSocket::bind(endpoint).await {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(_) => return Err("TURN listener rebind"),
            }
        }
    })
    .await
    .map_err(|_| "TURN listener shutdown")?
}

struct Bootstrap {
    port: u16,
    scope: FakeInputProofScope,
    peer: PublicCard,
    ca: Vec<u8>,
    turn: Option<TurnFixtureConfig>,
}

impl Bootstrap {
    fn read(role: SessionRole, lane: Lane) -> Result<Self> {
        let mut input = io::stdin().lock();
        let mut bytes = [0; BOOT];
        input.read_exact(&mut bytes).map_err(|_| "boot header")?;
        if bytes[..4] != PREFIX || bytes[4] != role_byte(role) {
            return Err("boot role");
        }
        let [port, expires, room, generation, peer, pin, len] = [
            &bytes[5..7],
            &bytes[7..15],
            &bytes[15..31],
            &bytes[31..63],
            &bytes[63..63 + CARD],
            &bytes[63 + CARD..95 + CARD],
            &bytes[95 + CARD..],
        ];
        let port = u16::from_be_bytes(port.try_into().unwrap());
        let len = u16::from_be_bytes(len.try_into().unwrap()) as usize;
        if port == 0 || !(1..=MAX_CA).contains(&len) {
            return Err("boot bounds");
        }
        let mut ca = vec![0; len];
        input.read_exact(&mut ca).map_err(|_| "boot CA")?;
        let ca_sha256: [u8; 32] = Sha256::digest(&ca).into();
        if ca_sha256 != pin {
            return Err("boot CA fingerprint");
        }
        let (turn_port, turn) = if lane == Lane::Turn {
            let mut block = Zeroizing::new([0; TURN_BOOT]);
            input
                .read_exact(&mut *block)
                .map_err(|_| "boot TURN fixture")?;
            let turn_port = u16::from_be_bytes(block[..2].try_into().unwrap());
            if port < 1024 || turn_port < 1024 || port == turn_port {
                return Err("boot TURN port");
            }
            let username =
                std::str::from_utf8(&block[2..2 + TURN_CRED]).map_err(|_| "boot TURN username")?;
            let credential =
                std::str::from_utf8(&block[2 + TURN_CRED..]).map_err(|_| "boot TURN credential")?;
            (
                turn_port,
                Some(TurnFixtureConfig::new(turn_port, username, credential)?),
            )
        } else {
            if input.read(&mut [0]).map_err(|_| "boot EOF")? != 0 {
                return Err("boot trailing bytes");
            }
            (3478, None)
        };
        let run = RunScope {
            expires: u64::from_be_bytes(expires.try_into().unwrap()),
            room: Uuid::from_bytes(room.try_into().unwrap()),
            generation: generation.try_into().unwrap(),
        };
        Ok(Self {
            port,
            scope: run.proof(port, turn_port, ca_sha256),
            peer: PublicCard::parse(peer.try_into().unwrap())?,
            ca,
            turn,
        })
    }
}

fn status(bytes: &[u8]) -> Result<()> {
    let mut out = io::stdout().lock();
    out.write_all(bytes).map_err(|_| "worker status pipe")?;
    out.flush().map_err(|_| "worker status flush")
}

fn fixture_sdp(role: SessionRole) -> String {
    // Fixed inert SDP fixtures: no actual credentials, network, or RTC objects.
    let (setup, port, fingerprint) = match role {
        SessionRole::Host => ("active", RELAY_PORTS.0, "CD"),
        SessionRole::Viewer => ("actpass", RELAY_PORTS.1, "AB"),
    };
    format!(
        "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:sha-256 {}\r\na=group:BUNDLE 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=setup:{setup}\r\na=mid:0\r\na=sctp-port:5000\r\na=ice-ufrag:ABCD\r\na=ice-pwd:abcdefghijklmnopqrstuv\r\na=candidate:r 1 udp 16777215 {RELAY_LABEL} {port} typ relay raddr 0.0.0.0 rport 0\r\na=end-of-candidates\r\n",
        [fingerprint; 32].join(":")
    )
}

async fn send(
    wss: &mut FixtureSocket,
    context: Context,
    kind: Kind,
    frame: Frame<'_>,
) -> Result<()> {
    let bytes = wire::encode(context, kind, frame)?;
    wss.send(&bytes).await
}

async fn role_exchange(
    role: SessionRole,
    boot: Bootstrap,
    mut proof: FakeInputProof<'_>,
) -> Result<()> {
    let context = Context {
        room: boot.scope.room,
        generation: boot.scope.generation,
    };
    let mut wss = connect_role(boot.port, &boot.ca, role).await?;
    status(&[CONNECTED])?;
    let (own_challenge, other_challenge, own_sdp, other_sdp, other_role) = match role {
        SessionRole::Host => (
            Kind::HostChallenge,
            Kind::ViewerChallenge,
            Kind::Answer,
            Kind::Offer,
            SessionRole::Viewer,
        ),
        SessionRole::Viewer => (
            Kind::ViewerChallenge,
            Kind::HostChallenge,
            Kind::Offer,
            Kind::Answer,
            SessionRole::Host,
        ),
    };
    let challenge = proof.challenge(wall()?).ok_or("local challenge")?;
    send(
        &mut wss,
        context,
        own_challenge,
        Frame::Challenge(challenge),
    )
    .await?;
    let inbound = wss.recv().await?;
    let Frame::Challenge(peer_challenge) = wire::decode(context, other_challenge, &inbound)? else {
        return Err("peer challenge");
    };

    if let Some(turn) = boot.turn {
        let (peer, channel) = relay_sdp(
            role,
            &mut wss,
            context,
            boot.scope,
            &turn,
            &mut proof,
            peer_challenge,
        )
        .await?;
        return relay_marker(role, &mut wss, peer, channel, &mut proof).await;
    }

    // A host must verify the exact signed offer before sealing the answer.
    if role == SessionRole::Host {
        verify_inbound(&mut wss, context, other_sdp, other_role, &mut proof).await?;
    }
    let sdp = fixture_sdp(role);
    let signature = proof
        .seal(peer_challenge, &sdp, wall()?)
        .map_err(|e| e.code())?;
    send(
        &mut wss,
        context,
        own_sdp,
        Frame::SignedSdp {
            sdp: &sdp,
            signature,
        },
    )
    .await?;
    if role == SessionRole::Viewer {
        verify_inbound(&mut wss, context, other_sdp, other_role, &mut proof).await?;
    }
    if !proof.is_complete(wall()?) {
        return Err("both descriptions not bound");
    }

    close_wss(role, &mut wss).await?;
    status(&[VERIFIED])
}

async fn close_wss(role: SessionRole, wss: &mut FixtureSocket) -> Result<()> {
    match role {
        SessionRole::Viewer => wss.disconnect_confirmed().await,
        SessionRole::Host => wss.wait_for_close().await,
    }
}

async fn relay_sdp(
    role: SessionRole,
    wss: &mut FixtureSocket,
    ctx: Context,
    scope: FakeInputProofScope,
    turn: &TurnFixtureConfig,
    proof: &mut FakeInputProof<'_>,
    challenge: SessionChallenge,
) -> Result<(Peer, Arc<dyn DataChannel>)> {
    match role {
        SessionRole::Host => {
            let offer = verified_native_sdp(wss, ctx, Kind::Offer, proof).await?;
            let remote =
                scope.validate_relay_sctp_description(&offer, SessionRole::Viewer, wall()?)?;
            // No host RTC or installed peer tuple before exact SDP verification.
            let mut peer = Peer::start(turn).await;
            peer.remote(&offer, false, remote).await;
            let answer = peer.local(true).await;
            let signature = proof
                .seal(challenge, &answer, wall()?)
                .map_err(|e| e.code())?;
            send(
                wss,
                ctx,
                Kind::Answer,
                Frame::SignedSdp {
                    sdp: &answer,
                    signature,
                },
            )
            .await?;
            let channel = peer.channels.recv().await.ok_or("relay host channel")?;
            Ok((peer, channel))
        }
        SessionRole::Viewer => {
            let mut peer = Peer::start(turn).await;
            let channel = peer
                .pc
                .create_data_channel(
                    LABEL,
                    Some(RTCDataChannelInit {
                        protocol: PROTOCOL.into(),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|_| "relay viewer channel")?;
            let offer = peer.local(false).await;
            let signature = proof
                .seal(challenge, &offer, wall()?)
                .map_err(|e| e.code())?;
            send(
                wss,
                ctx,
                Kind::Offer,
                Frame::SignedSdp {
                    sdp: &offer,
                    signature,
                },
            )
            .await?;
            let answer = verified_native_sdp(wss, ctx, Kind::Answer, proof).await?;
            let remote = proof
                .approved_relay_pair(wall()?)
                .ok_or("relay proof incomplete")?
                .remote();
            peer.remote(&answer, true, remote).await;
            Ok((peer, channel))
        }
    }
}

async fn verified_native_sdp(
    wss: &mut FixtureSocket,
    context: Context,
    kind: Kind,
    proof: &mut FakeInputProof<'_>,
) -> Result<String> {
    let inbound = wss.recv().await?;
    let Frame::SignedSdp { sdp, signature } = wire::decode(context, kind, &inbound)? else {
        return Err("signed native SDP frame");
    };
    proof
        .verify(sdp, &signature, wall()?)
        .map_err(|e| e.code())?;
    // Copy the exact verified UTF-8 bytes without sanitizing remote SDP.
    Ok(sdp.to_owned())
}

async fn wait_marker() -> Result<()> {
    tokio::task::spawn_blocking(|| {
        let mut input = io::stdin().lock();
        let mut message = [0];
        input
            .read_exact(&mut message)
            .map_err(|_| "missing relay marker gate")?;
        if message != [MARKER_GO]
            || input
                .read(&mut message)
                .map_err(|_| "relay marker gate EOF")?
                != 0
        {
            return Err("invalid relay marker gate");
        }
        Ok(())
    })
    .await
    .map_err(|_| "relay marker gate task")?
}

async fn relay_marker(
    role: SessionRole,
    wss: &mut FixtureSocket,
    peer: Peer,
    channel: Arc<dyn DataChannel>,
    proof: &mut FakeInputProof<'_>,
) -> Result<()> {
    opened(&channel).await;
    if channel.label().await.ok().as_deref() != Some(LABEL)
        || channel.protocol().await.ok().as_deref() != Some(PROTOCOL)
        || channel.ordered().await.ok() != Some(true)
        || channel.negotiated().await.ok() != Some(false)
        || channel.max_retransmits().await.ok() != Some(None)
        || channel.max_packet_life_time().await.ok() != Some(None)
    {
        return Err("relay SCTP channel policy");
    }
    peer.verify_pair(proof).await;
    status(&[RELAY_READY])?;

    let transfer = async {
        wait_marker().await?;
        peer.verify_pair(proof).await;
        match role {
            SessionRole::Host => {
                text(&channel, MARKER).await;
                peer.verify_pair(proof).await;
            }
            SessionRole::Viewer => {
                channel
                    .try_send_text(MARKER)
                    .await
                    .map_err(|_| "relay marker send")?;
                text(&channel, ACK).await;
                peer.verify_pair(proof).await;
            }
        }
        Ok::<(), &'static str>(())
    };
    tokio::select! {
        biased;
        _ = wss.recv() => return Err("unexpected WSS activity before SCTP completion"),
        result = transfer => result?,
    }
    if role == SessionRole::Host {
        channel
            .try_send_text(ACK)
            .await
            .map_err(|_| "relay ACK send")?;
    }
    // Only viewer, after actual ACK receipt, sends a WSS close. Host detects
    // it before revoking RTC so its queued ACK cannot be cut off locally.
    close_wss(role, wss).await?;
    peer.revoke();
    peer.pc.close().await.map_err(|_| "relay RTC close")?;
    peer.assert_runtime_revoked().await;
    status(&[RELAY_DONE])
}

async fn verify_inbound(
    wss: &mut FixtureSocket,
    context: Context,
    kind: Kind,
    signer: SessionRole,
    proof: &mut FakeInputProof<'_>,
) -> Result<()> {
    let inbound = wss.recv().await?;
    let Frame::SignedSdp { sdp, signature } = wire::decode(context, kind, &inbound)? else {
        return Err("signed SDP frame");
    };
    if sdp != fixture_sdp(signer) {
        return Err("unexpected fixture SDP");
    }
    proof.verify(sdp, &signature, wall()?).map_err(|e| e.code())
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "test runtime")
}

#[cfg(target_os = "linux")]
fn is_pipe(endpoint: impl std::os::fd::AsFd) -> bool {
    use std::{fs::File, os::unix::fs::FileTypeExt};

    endpoint
        .as_fd()
        .try_clone_to_owned()
        .and_then(|fd| File::from(fd).metadata())
        .is_ok_and(|meta| meta.file_type().is_fifo())
}

#[cfg(windows)]
fn is_pipe(endpoint: impl std::os::windows::io::AsHandle) -> bool {
    use std::{os::windows::io::AsRawHandle, ptr};
    use windows_sys::Win32::{
        Storage::FileSystem::{GetFileType, FILE_TYPE_PIPE},
        System::Pipes::GetNamedPipeInfo,
    };

    let handle = endpoint.as_handle();
    let raw = handle.as_raw_handle();
    // SAFETY: the standard I/O handle is borrowed and remains live for
    // both queries. GetNamedPipeInfo takes optional output pointers.
    // GetFileType alone also classifies sockets as PIPE on Windows.
    unsafe {
        GetFileType(raw) == FILE_TYPE_PIPE
            && GetNamedPipeInfo(
                raw,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            ) != 0
    }
}

#[cfg(test)]
mod pipe_tests {
    use super::*;

    #[test]
    fn distinguishes_local_pipe_ends_from_a_regular_file() {
        let (reader, writer) = io::pipe().unwrap();
        assert!(is_pipe(reader));
        assert!(is_pipe(writer));
        let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
        assert!(!is_pipe(file));
    }
}

fn role_name(role: SessionRole) -> &'static str {
    match role {
        SessionRole::Host => "host",
        SessionRole::Viewer => "viewer",
    }
}

fn run_worker(role: SessionRole, lane: Lane) -> Result<()> {
    if !is_pipe(io::stdin()) || !is_pipe(io::stdout()) {
        return Err("worker requires private pipes");
    }
    // Also bound direct misuse of the internal argument with an idle pipe.
    let (done, stop) = mpsc::channel::<()>();
    let watchdog = thread::Builder::new()
        .spawn(move || {
            if matches!(
                stop.recv_timeout(CHILD_WAIT),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                std::process::exit(3);
            }
        })
        .map_err(|_| "worker watchdog")?;

    let checked = (|| {
        let id = Identity::generate_ephemeral().map_err(|_| "worker identity")?;
        let local = PublicCard::from_local(&id)?;
        let mut hello = [0; HELLO];
        hello[..4].copy_from_slice(&PREFIX);
        hello[4] = role_byte(role);
        hello[5..].copy_from_slice(&local.0);
        status(&hello)?;

        let boot = Bootstrap::read(role, lane)?;
        let (candidate, fp) = boot.peer.parts()?;
        let pinned = id.verify_peer(&candidate, fp).map_err(|_| "peer pin")?;
        SyntheticProfile {
            role,
            mode: MODE,
            scope: boot.scope,
            local_fingerprint: local.parts()?.1,
            peer_fingerprint: fp,
        }
        .validate(role, &id, &pinned, wall()?)
        .map_err(|_| "role profile")?;
        let proof = FakeInputProof::prepare(&id, &pinned, role, boot.scope, MODE, wall()?)
            .map_err(|e| e.code())?;
        runtime()?.block_on(role_exchange(role, boot, proof))
    })();
    let _ = done.send(());
    let _ = watchdog.join();
    if let Err(code) = checked {
        let role = role_name(role);
        eprintln!("fixture worker: role={role} {code}");
    }
    checked
}

pub fn run_wss_signed_sdp_process_check() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        eprintln!("Manual loopback diagnostic: use {START} or {START_TURN}");
        return ExitCode::SUCCESS;
    };
    if args.next().is_some() {
        return ExitCode::from(2);
    }
    let lane = if arg == OsStr::new(START) {
        Some(Lane::Signed)
    } else if arg == OsStr::new(START_TURN) {
        Some(Lane::Turn)
    } else {
        None
    };
    let result = if let Some(lane) = lane {
        runtime().and_then(|rt| rt.block_on(run_parent(lane)))
    } else if arg == OsStr::new(HOST_WORKER) {
        run_worker(SessionRole::Host, Lane::Signed)
    } else if arg == OsStr::new(VIEWER_WORKER) {
        run_worker(SessionRole::Viewer, Lane::Signed)
    } else if arg == OsStr::new(TURN_HOST_WORKER) {
        run_worker(SessionRole::Host, Lane::Turn)
    } else if arg == OsStr::new(TURN_VIEWER_WORKER) {
        run_worker(SessionRole::Viewer, Lane::Turn)
    } else {
        return ExitCode::from(2);
    };
    match result {
        Ok(()) => {
            match lane {
                Some(Lane::Signed) => println!("WSS_SIGNED_SDP_TWO_PROCESS_PASS"),
                Some(Lane::Turn) => println!("WSS_TURN_SCTP_TWO_PROCESS_PASS"),
                None => (),
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("signed SDP process check: {error}");
            ExitCode::FAILURE
        }
    }
}
