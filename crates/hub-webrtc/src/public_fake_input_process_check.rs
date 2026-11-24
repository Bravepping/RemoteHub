//! Two isolated role processes prove a fixed synthetic SCTP description pair
//! over real loopback TLS/WSS. No RTC connection, ICE, TURN, or input is started.

use crate::{
    ice_policy::IceMode,
    public_fake_input::{FakeInputProof, FakeInputProofScope, FakeInputTestMode},
    public_fake_input_profile::SyntheticProfile,
    public_fake_input_wss_fixture::{connect_role, FixtureSocket, FixtureWssServer},
    public_fake_input_wss_wire::{self as wire, Context, Frame, Kind},
    public_video::PublicVideoScope,
};
use hub_identity::{pairing::PairingCandidate, session::SessionRole, Identity};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4},
    os::{fd::AsFd, unix::fs::FileTypeExt},
    process::{ExitCode, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use uuid::Uuid;

const START: &str = "--two-process-loopback-only";
const HOST_WORKER: &str = "--internal-wss-host";
const VIEWER_WORKER: &str = "--internal-wss-viewer";
const PARENT_WAIT: Duration = Duration::from_secs(12);
const CHILD_WAIT: Duration = Duration::from_secs(15);
const MAX_CA: usize = 2048;
const PREFIX: [u8; 4] = *b"RHP1";
const CONNECTED: u8 = 0xA1;
const VERIFIED: u8 = 0xA2;
const CODE: usize = 68; // RH2- and 64 lowercase hex digits
const FINGERPRINT: usize = 64;
const CARD: usize = CODE + FINGERPRINT;
const HELLO: usize = PREFIX.len() + 1 + CARD;
// Version, role, port, expiry, room, generation, peer card, CA pin, DER length.
const BOOT: usize = PREFIX.len() + 1 + 2 + 8 + 16 + 32 + CARD + 32 + 2;
const RELAY_LABEL: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);
const HOST_PORT: u16 = 49152;
const VIEWER_PORT: u16 = 49153;
const WSS_CLOSED: &str = "FAKE_INPUT_FIXTURE_WSS_CLOSED";
const MODE: FakeInputTestMode = FakeInputTestMode::Enabled;

type Result<T> = std::result::Result<T, &'static str>;

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

    fn proof(self, port: u16, ca_sha256: [u8; 32]) -> FakeInputProofScope {
        // These public-looking labels are signed test metadata, never dialed.
        FakeInputProofScope {
            room: self.room,
            generation: self.generation,
            route: PublicVideoScope {
                wss: SocketAddrV4::new(RELAY_LABEL, port),
                ca_sha256,
                turn: SocketAddrV4::new(RELAY_LABEL, 3478),
                relay_ports: (HOST_PORT, VIEWER_PORT),
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
    async fn spawn(role: SessionRole) -> Result<Self> {
        let exe = std::env::current_exe().map_err(|_| "self executable")?;
        let arg = match role {
            SessionRole::Host => HOST_WORKER,
            SessionRole::Viewer => VIEWER_WORKER,
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
    ) -> Result<()> {
        let ca = server.ca_der();
        if ca.is_empty() || ca.len() > MAX_CA {
            return Err("fixture CA length");
        }
        let mut bytes = Vec::with_capacity(BOOT + ca.len());
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
        let mut pipe = self.boot.take().ok_or("duplicate worker bootstrap")?;
        pipe.write_all(&bytes)
            .await
            .map_err(|_| "worker bootstrap")?;
        pipe.shutdown().await.map_err(|_| "worker bootstrap EOF")
    }

    async fn expect(&mut self, expected: u8) -> Result<()> {
        match self.status.read_u8().await {
            Ok(actual) if actual == expected => Ok(()),
            _ => Err("worker status"),
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

async fn run_parent() -> Result<()> {
    let server = FixtureWssServer::start().await?;
    let scope = RunScope::new()?;
    let mut host = Worker::spawn(SessionRole::Host).await?;
    let mut viewer = match Worker::spawn(SessionRole::Viewer).await {
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
        host.bootstrap(&server, scope, &viewer_card).await?;
        host.expect(CONNECTED).await?;
        viewer.bootstrap(&server, scope, &host_card).await?;
        viewer.expect(CONNECTED).await?;

        tokio::try_join!(host.expect(VERIFIED), viewer.expect(VERIFIED))?;
        tokio::try_join!(host.finished(), viewer.finished())?;
        server.wait().await
    })
    .await
    .unwrap_or(Err("process check deadline"));
    if checked.is_err() {
        // kill() waits/reaps, even if a child was blocked on a private pipe.
        let _ = tokio::join!(host.child.kill(), viewer.child.kill());
    }
    checked
}

struct Bootstrap {
    port: u16,
    scope: FakeInputProofScope,
    peer: PublicCard,
    ca: Vec<u8>,
}

impl Bootstrap {
    fn read(role: SessionRole) -> Result<Self> {
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
        if input.read(&mut [0]).map_err(|_| "boot EOF")? != 0 {
            return Err("boot trailing bytes");
        }
        let ca_sha256: [u8; 32] = Sha256::digest(&ca).into();
        if ca_sha256 != pin {
            return Err("boot CA fingerprint");
        }
        let run = RunScope {
            expires: u64::from_be_bytes(expires.try_into().unwrap()),
            room: Uuid::from_bytes(room.try_into().unwrap()),
            generation: generation.try_into().unwrap(),
        };
        Ok(Self {
            port,
            scope: run.proof(port, ca_sha256),
            peer: PublicCard::parse(peer.try_into().unwrap())?,
            ca,
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
        SessionRole::Host => ("active", HOST_PORT, "CD"),
        SessionRole::Viewer => ("actpass", VIEWER_PORT, "AB"),
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

    match role {
        SessionRole::Viewer => wss.disconnect().await?,
        SessionRole::Host => {
            if wss.recv().await != Err(WSS_CLOSED) {
                return Err("excess frame or missing viewer close");
            }
        }
    }
    status(&[VERIFIED])
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

fn is_pipe(endpoint: impl AsFd) -> bool {
    endpoint
        .as_fd()
        .try_clone_to_owned()
        .and_then(|fd| File::from(fd).metadata())
        .is_ok_and(|meta| meta.file_type().is_fifo())
}

fn run_worker(role: SessionRole) -> Result<()> {
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

        let boot = Bootstrap::read(role)?;
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
    checked
}

pub fn run_wss_signed_sdp_process_check() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        eprintln!("Manual signed SDP diagnostic: run with {START}");
        return ExitCode::SUCCESS;
    };
    if args.next().is_some() {
        return ExitCode::from(2);
    }
    let is_parent = arg == OsStr::new(START);
    let result = if is_parent {
        runtime().and_then(|rt| rt.block_on(run_parent()))
    } else if arg == OsStr::new(HOST_WORKER) {
        run_worker(SessionRole::Host)
    } else if arg == OsStr::new(VIEWER_WORKER) {
        run_worker(SessionRole::Viewer)
    } else {
        return ExitCode::from(2);
    };
    match result {
        Ok(()) => {
            if is_parent {
                println!("WSS_SIGNED_SDP_TWO_PROCESS_PASS");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("signed SDP process check: {error}");
            ExitCode::FAILURE
        }
    }
}
