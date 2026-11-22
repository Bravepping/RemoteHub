//! Device-verified LAN signaling with WSS limited to inline offer/answer.
//! The authenticated TCP socket remains the session's lifetime/control path.

use super::{
    check_verified_socket, reversed_scope, validate_verified_sdp, LanScope, LanSession, Session,
    VerifiedLanConfig,
};
use crate::{
    authenticated_relay::{
        AuthenticatedLoopbackRelay, ClientError, PrivateLanEndpoint, Role, RoomId, SignalKind,
    },
    lan_device::{self, WssBootstrap},
    lan_wire::Codec,
    private_wss_contract::{
        answer_digest, check_relay_event, expect_tcp, send_tcp, unexpected_tcp, ExpectedRelayEvent,
        ACCEPTED, AUTH_FAILED, READY,
    },
};
use std::{
    future::Future,
    net::SocketAddrV4,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, time::timeout};

pub use crate::authenticated_relay::{LoopbackTlsTrust, RelayToken};

const SETUP_TIMEOUT: Duration = Duration::from_secs(14);
const STAGE_TIMEOUT: Duration = Duration::from_secs(5);

/// Explicit private relay credentials and independently pinned devices for one
/// user-approved video-only session. The media route still comes from LanScope.
pub struct PrivateWssLanConfig {
    pub device: VerifiedLanConfig,
    pub relay: SocketAddrV4,
    pub token: Arc<RelayToken>,
    pub trust: Arc<LoopbackTlsTrust>,
}

impl PrivateWssLanConfig {
    pub(super) fn validate(&self, scope: LanScope) -> Result<(), String> {
        self.device.validate(scope)?;
        self.endpoint(scope)?;
        Ok(())
    }

    fn endpoint(&self, scope: LanScope) -> Result<PrivateLanEndpoint, String> {
        PrivateLanEndpoint::new(scope.local, *self.relay.ip(), self.relay.port())
            .map_err(relay_error)
    }
}

impl LanSession {
    pub(super) async fn host_wss_main(
        &self,
        mut socket: TcpStream,
        auth: PrivateWssLanConfig,
    ) -> Result<(), String> {
        let deadline = Instant::now() + SETUP_TIMEOUT;
        check_verified_socket(&socket, self.scope, self.port, true)?;
        let (mut codec, mut proof, bootstrap) = self
            .wss_stage(None, stage_deadline(deadline), async {
                lan_device::authenticate_host_wss(
                    &mut socket,
                    self.secret()?,
                    &auth.device.local_identity,
                    &auth.device.peer,
                    self.scope,
                    self.port,
                    auth.relay,
                )
                .await
                .map_err(|_| AUTH_FAILED.to_owned())
            })
            .await?;
        let session_nonce = bootstrap.session_nonce;

        let (mut relay, challenges) = self
            .wss_stage(Some(&socket), stage_deadline(deadline), async {
                let (mut relay, challenges) =
                    connect_relay(self.scope, &auth, bootstrap, Role::Host).await?;
                expect_relay_event(&mut relay, ExpectedRelayEvent::Waiting).await?;
                Ok((relay, challenges))
            })
            .await?;
        self.wss_stage(None, stage_deadline(deadline), async {
            send_tcp(&mut socket, &mut codec, READY, String::new()).await
        })
        .await?;

        let offer = self
            .wss_stage(Some(&socket), stage_deadline(deadline), async {
                complete_challenges(&mut relay, challenges, Role::Host).await?;
                let signed = receive_signal(&mut relay, SignalKind::Offer).await?;
                let offer = proof.open(&signed).map_err(|_| AUTH_FAILED.to_owned())?;
                validate_verified_sdp(&offer, self.scope, false)?;
                Ok(offer)
            })
            .await?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err("CANCELLED".into());
        }
        self.update(|status| {
            status.authenticated = true;
            status.state = "negotiating".into();
        });
        let source = Session::new_lan(self.seconds, self.scope)?;
        *self.source.lock().unwrap_or_else(|p| p.into_inner()) = Some(source.clone());
        let answer = self
            .wss_stage(Some(&socket), deadline, source.signal(offer))
            .await?;
        validate_verified_sdp(&answer, reversed_scope(self.scope), true)?;
        let accepted = answer_digest(&session_nonce, &answer);
        let signed = proof.seal(answer).map_err(|_| AUTH_FAILED.to_owned())?;
        drop(auth);

        // The viewer may send its TCP acknowledgement as soon as the answer
        // reaches it, so stop peeking TCP while publishing the answer.
        self.wss_stage(None, stage_deadline(deadline), async {
            relay
                .send_signal(SignalKind::Answer, signed)
                .await
                .map_err(relay_error)?;
            expect_tcp(&mut socket, &mut codec, ACCEPTED, &accepted).await
        })
        .await?;
        // Drop both WSS directions before the established TCP control loop.
        drop(relay);
        self.control(socket, codec).await
    }

    pub(super) async fn receive_wss_answer(
        &self,
        socket: &mut TcpStream,
        auth: PrivateWssLanConfig,
        offer: String,
        deadline: Instant,
    ) -> Result<(Codec, String), String> {
        check_verified_socket(socket, self.scope, self.port, false)?;
        let (mut codec, mut proof, bootstrap) = self
            .wss_stage(None, stage_deadline(deadline), async {
                lan_device::authenticate_viewer_wss(
                    socket,
                    self.secret()?,
                    &auth.device.local_identity,
                    &auth.device.peer,
                    self.scope,
                    self.port,
                    auth.relay,
                )
                .await
                .map_err(|_| AUTH_FAILED.to_owned())
            })
            .await?;
        let session_nonce = bootstrap.session_nonce;
        self.wss_stage(None, stage_deadline(deadline), async {
            expect_tcp(socket, &mut codec, READY, "").await
        })
        .await?;
        let (relay, answer) = self
            .wss_stage(Some(socket), deadline, async {
                let (mut relay, challenges) =
                    connect_relay(self.scope, &auth, bootstrap, Role::Viewer).await?;
                complete_challenges(&mut relay, challenges, Role::Viewer).await?;
                relay
                    .send_signal(
                        SignalKind::Offer,
                        proof.seal(offer).map_err(|_| AUTH_FAILED.to_owned())?,
                    )
                    .await
                    .map_err(relay_error)?;
                let answer = receive_signal(&mut relay, SignalKind::Answer).await?;
                let answer = proof.open(&answer).map_err(|_| AUTH_FAILED.to_owned())?;
                validate_verified_sdp(&answer, self.scope, true)?;
                Ok((relay, answer))
            })
            .await?;
        drop(auth);
        self.wss_stage(None, stage_deadline(deadline), async {
            send_tcp(
                socket,
                &mut codec,
                ACCEPTED,
                answer_digest(&session_nonce, &answer),
            )
            .await
        })
        .await?;
        drop(relay);
        if self.cancelled.load(Ordering::Acquire) {
            return Err("CANCELLED".into());
        }
        Ok((codec, answer))
    }

    async fn wss_stage<T>(
        &self,
        monitor: Option<&TcpStream>,
        deadline: Instant,
        work: impl Future<Output = Result<T, String>>,
    ) -> Result<T, String> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("LAN_WSS_SETUP_TIMEOUT".into());
        }
        tokio::select! {
            biased;
            _ = self.cancelled() => Err("CANCELLED".into()),
            error = unexpected_tcp(monitor) => Err(error),
            result = timeout(remaining, work) => {
                result.map_err(|_| "LAN_WSS_SETUP_TIMEOUT".to_owned())?
            }
        }
    }
}

fn stage_deadline(overall: Instant) -> Instant {
    overall.min(Instant::now() + STAGE_TIMEOUT)
}

struct RelayChallenges {
    host: String,
    viewer: String,
}

async fn connect_relay(
    scope: LanScope,
    auth: &PrivateWssLanConfig,
    bootstrap: WssBootstrap,
    role: Role,
) -> Result<(AuthenticatedLoopbackRelay, RelayChallenges), String> {
    let relay = AuthenticatedLoopbackRelay::connect_private_lan_wss(
        auth.endpoint(scope)?,
        &auth.token,
        RoomId::new(bootstrap.room).map_err(relay_error)?,
        role,
        bootstrap.signal_key,
        bootstrap.codec_nonce,
        &auth.trust,
    )
    .await
    .map_err(relay_error)?;
    Ok((
        relay,
        RelayChallenges {
            host: bootstrap.host_challenge,
            viewer: bootstrap.viewer_challenge,
        },
    ))
}

async fn complete_challenges(
    relay: &mut AuthenticatedLoopbackRelay,
    challenges: RelayChallenges,
    role: Role,
) -> Result<(), String> {
    expect_relay_event(relay, ExpectedRelayEvent::Paired).await?;
    let (out_kind, out, in_kind, expected) = match role {
        Role::Host => (
            SignalKind::HostChallenge,
            challenges.host,
            SignalKind::ViewerChallenge,
            challenges.viewer,
        ),
        Role::Viewer => (
            SignalKind::ViewerChallenge,
            challenges.viewer,
            SignalKind::HostChallenge,
            challenges.host,
        ),
    };
    relay
        .send_signal(out_kind, out)
        .await
        .map_err(relay_error)?;
    if receive_signal(relay, in_kind).await? != expected {
        return Err(AUTH_FAILED.into());
    }
    Ok(())
}

async fn expect_relay_event(
    relay: &mut AuthenticatedLoopbackRelay,
    expected: ExpectedRelayEvent,
) -> Result<Option<String>, String> {
    let event = relay.next_event(SETUP_TIMEOUT).await.map_err(relay_error)?;
    check_relay_event(event, expected)
}

async fn receive_signal(
    relay: &mut AuthenticatedLoopbackRelay,
    signal: SignalKind,
) -> Result<String, String> {
    expect_relay_event(relay, ExpectedRelayEvent::Signal(signal))
        .await?
        .ok_or_else(|| AUTH_FAILED.to_owned())
}

fn relay_error(error: ClientError) -> String {
    match error {
        ClientError::PrivateLanEndpoint => "LAN_WSS_ENDPOINT",
        ClientError::PrivateLanBind => "LAN_WSS_BIND",
        ClientError::TlsHandshake | ClientError::TlsTrust => "LAN_WSS_TLS_FAILED",
        ClientError::Timeout => "LAN_WSS_SETUP_TIMEOUT",
        ClientError::Transport => "LAN_WSS_TRANSPORT_FAILED",
        _ => AUTH_FAILED,
    }
    .into()
}
