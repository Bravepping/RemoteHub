//! Offline proof contract for public video. Verifying an SDP proves its sender
//! and approved route context, not end-user consent or candidate eligibility.
use crate::ice_policy::{public_v4, IceConfig, IceMode, RelayGrant};
pub use crate::public_video_route::PublicVideoIcePlan;
#[cfg(feature = "v2-public-control")]
pub use crate::public_video_scoped_ice::PublicControlTransport;
#[cfg(feature = "v2-public-video-wss")]
pub use crate::public_video_scoped_ice::PublicVideoScopedIce;
use hub_identity::{
    pairing::VerifiedPeer,
    session::{
        PendingSessionProof, SessionBinding, SessionChallenge, SessionRole, MAX_SIGNAL_BYTES,
    },
    Identity,
};
use sha2::{Digest, Sha256};
use std::{net::SocketAddrV4, time::Duration};
use uuid::Uuid;

const MAX_SCOPE_TTL: u64 = 600;
#[cfg(feature = "v2-public-control-sustained")]
pub const SUSTAINED_CONTROL_SESSION_SECONDS: u64 = 1800;
#[cfg(feature = "v2-public-control-sustained")]
pub const SUSTAINED_CONTROL_STARTUP_ALLOWANCE_SECONDS: u64 = 60;
#[cfg(feature = "v2-public-control-sustained")]
pub const SUSTAINED_CONTROL_MEDIA_BOUND_SECONDS: u64 =
    SUSTAINED_CONTROL_SESSION_SECONDS + SUSTAINED_CONTROL_STARTUP_ALLOWANCE_SECONDS;
#[cfg(feature = "v2-public-control-sustained")]
pub const SUSTAINED_CONTROL_SCOPE_TTL_SECONDS: u64 = 2400;
#[cfg(feature = "v2-public-control-sustained")]
pub const SUSTAINED_CONTROL_BOOTSTRAP_MIN_SECONDS: u64 = 1860;
const PROOF_TTL: u64 = 15;
const INVALID_SCOPE: &str = "PUBLIC_VIDEO_SCOPE_INVALID";
const EXPIRED: &str = "PUBLIC_VIDEO_SCOPE_EXPIRED";
const IDENTITY: &str = "PUBLIC_VIDEO_IDENTITY_MISMATCH";
const SDP: &str = "PUBLIC_VIDEO_SDP_INVALID";
const REJECTED: &str = "PUBLIC_VIDEO_PROOF_REJECTED";

#[derive(Clone, Copy)]
enum Purpose {
    Video,
    #[cfg(feature = "v2-public-control")]
    Control,
    #[cfg(feature = "v2-public-control")]
    ControlBatch,
    #[cfg(feature = "v2-public-control-sustained")]
    ControlSustained,
}

impl Purpose {
    #[cfg(feature = "v2-public-control")]
    fn has_control(self) -> bool {
        match self {
            Self::Control | Self::ControlBatch => true,
            #[cfg(feature = "v2-public-control-sustained")]
            Self::ControlSustained => true,
            Self::Video => false,
        }
    }

    // Preserve legacy domains: the signed statement independently binds the route mode.
    fn domain(self) -> &'static [u8] {
        match self {
            Self::Video => b"RemoteHub.V2.public-video-only.sdp.v1\0",
            #[cfg(feature = "v2-public-control")]
            Self::Control => b"RemoteHub.V2.public-control-turn.sdp.v1\0",
            #[cfg(feature = "v2-public-control")]
            Self::ControlBatch => b"RemoteHub.V2.public-control-turn.immediate-batch.sdp.v1\0",
            #[cfg(feature = "v2-public-control-sustained")]
            Self::ControlSustained => b"RemoteHub.V2.public-control-turn.sustained-1800.sdp.v1\0",
        }
    }

    fn check(self, scope: PublicVideoScope, now: u64) -> Result<u8, &'static str> {
        match self {
            #[cfg(feature = "v2-public-control")]
            Self::ControlBatch => scope.checked_control_batch(now),
            #[cfg(feature = "v2-public-control-sustained")]
            Self::ControlSustained => scope.checked_control_sustained(now),
            _ => scope.checked_mode(now),
        }
    }
}

/// Both devices must establish these values independently of the relay.
/// No untrusted SDP or relay reply can select this scope for the caller.
#[derive(Clone, Copy)]
pub struct PublicVideoScope {
    pub wss: SocketAddrV4,
    pub ca_sha256: [u8; 32],
    pub turn: SocketAddrV4,
    pub relay_ports: (u16, u16),
    pub mode: IceMode,
    pub expires_at: u64,
}

impl PublicVideoScope {
    pub(crate) fn checked_mode(self, now: u64) -> Result<u8, &'static str> {
        self.checked_mode_with_max(now, MAX_SCOPE_TTL)
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn checked_control_batch(self, now: u64) -> Result<u8, &'static str> {
        self.checked_mode_with_max(now, 1800)
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub(crate) fn checked_control_sustained(self, now: u64) -> Result<u8, &'static str> {
        self.checked_mode_with_max(now, SUSTAINED_CONTROL_SCOPE_TTL_SECONDS)
    }

    fn checked_mode_with_max(self, now: u64, max_ttl: u64) -> Result<u8, &'static str> {
        let mode = match self.mode {
            IceMode::DirectPreferred => 1,
            IceMode::RelayOnly => 2,
            _ => return Err(INVALID_SCOPE),
        };
        let (first, last) = self.relay_ports;
        if !public_v4(*self.wss.ip())
            || self.wss.port() < 1024
            || self.ca_sha256 == [0; 32]
            || self.turn.ip() != self.wss.ip()
            || self.turn.port() < 1024
            || first < 49152
            || !last.checked_sub(first).is_some_and(|span| span <= 255)
        {
            return Err(INVALID_SCOPE);
        }
        if !(1..=max_ttl).contains(&self.expires_at.saturating_sub(now)) {
            return Err(EXPIRED);
        }
        Ok(mode)
    }

    fn statement(
        self,
        purpose: Purpose,
        role: SessionRole,
        offer: Option<[u8; 32]>,
        exact_sdp: &str,
        now: u64,
    ) -> Result<[u8; 32], &'static str> {
        let mode = purpose.check(self, now)?;
        if exact_sdp.is_empty() || exact_sdp.len() > MAX_SIGNAL_BYTES || exact_sdp.contains('\0') {
            return Err(SDP);
        }
        let mut hash = Sha256::new();
        hash.update(purpose.domain());
        // Viewer signs the offer; host signs the answer for that exact offer.
        hash.update([role as u8, mode]);
        hash.update(self.wss.ip().octets());
        hash.update(self.wss.port().to_be_bytes());
        hash.update(self.ca_sha256);
        hash.update(self.turn.ip().octets());
        hash.update(self.turn.port().to_be_bytes());
        hash.update(self.relay_ports.0.to_be_bytes());
        hash.update(self.relay_ports.1.to_be_bytes());
        hash.update(self.expires_at.to_be_bytes());
        if role == SessionRole::Host {
            hash.update(offer.ok_or(REJECTED)?);
        }
        hash.update(exact_sdp.as_bytes());
        Ok(hash.finalize().into())
    }
}

pub struct PublicVideoProof<'a> {
    local: &'a Identity,
    purpose: Purpose,
    #[cfg(feature = "v2-public-control")]
    control_scope: Option<crate::input_approval::InputScope>,
    role: SessionRole,
    room: Uuid,
    scope: PublicVideoScope,
    binding: SessionBinding,
    incoming: Option<PendingSessionProof>,
    outgoing_available: bool,
    offer: Option<[u8; 32]>,
}

impl<'a> PublicVideoProof<'a> {
    pub fn prepare(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        room: Uuid,
        session_nonce: [u8; 32],
        scope: PublicVideoScope,
        now: u64,
    ) -> Result<Self, &'static str> {
        Self::prepare_for(
            local,
            pinned_peer,
            role,
            room,
            session_nonce,
            scope,
            now,
            Purpose::Video,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_for(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        room: Uuid,
        session_nonce: [u8; 32],
        scope: PublicVideoScope,
        now: u64,
        purpose: Purpose,
    ) -> Result<Self, &'static str> {
        purpose.check(scope, now)?;
        let binding = pinned_peer
            .session_binding(&local.public(), role, *room.as_bytes(), session_nonce)
            .map_err(|_| IDENTITY)?;
        let ttl = Duration::from_secs(PROOF_TTL.min(scope.expires_at - now));
        let incoming =
            PendingSessionProof::issue(&binding, opposite(role), now, ttl).map_err(|_| REJECTED)?;
        Ok(Self {
            local,
            purpose,
            #[cfg(feature = "v2-public-control")]
            control_scope: purpose
                .has_control()
                .then(|| crate::input_approval::InputScope {
                    room: *room.as_bytes(),
                    generation: session_nonce,
                    viewer_key: match role {
                        SessionRole::Host => *pinned_peer.public_key(),
                        SessionRole::Viewer => local.public().public_key,
                    },
                }),
            role,
            room,
            scope,
            binding,
            incoming: Some(incoming),
            outgoing_available: true,
            offer: None,
        })
    }

    #[cfg(feature = "v2-public-video-wss")]
    pub(crate) fn role(&self) -> SessionRole {
        self.role
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn control_scope(&self) -> Option<crate::input_approval::InputScope> {
        self.control_scope
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn is_immediate_control_batch(&self) -> bool {
        matches!(self.purpose, Purpose::ControlBatch)
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub(crate) fn is_sustained_control(&self) -> bool {
        matches!(self.purpose, Purpose::ControlSustained)
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn has_round_deadline(&self) -> bool {
        #[cfg(feature = "v2-public-control-sustained")]
        if self.is_sustained_control() {
            return true;
        }
        self.is_immediate_control_batch()
    }

    #[cfg(feature = "v2-public-video-wss")]
    fn validate_exact_sdp(
        &self,
        sdp: &str,
        answer: bool,
        now: u64,
    ) -> Result<Vec<crate::direct_policy::Candidate>, &'static str> {
        match self.purpose {
            Purpose::Video => crate::public_video_ice::validate(sdp, self.scope, answer, now),
            #[cfg(feature = "v2-public-control")]
            Purpose::Control | Purpose::ControlBatch => {
                self.purpose.check(self.scope, now)?;
                crate::public_control_sdp::validate(sdp, self.scope, answer, now)
            }
            #[cfg(feature = "v2-public-control-sustained")]
            Purpose::ControlSustained => {
                self.purpose.check(self.scope, now)?;
                crate::public_control_sdp::validate_shape(sdp, self.scope, answer)
            }
        }
    }

    /// Validate a native-issued route grant against exactly this proof's
    /// reserved context, before using it for ICE. Does not authorize media.
    pub fn prepare_ice(
        &self,
        grant: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<PublicVideoIcePlan, &'static str> {
        #[cfg(feature = "v2-public-control-sustained")]
        if matches!(self.purpose, Purpose::ControlSustained) {
            return PublicVideoIcePlan::prepare_control_sustained(
                self.scope, self.room, grant, config, now,
            );
        }
        #[cfg(feature = "v2-public-control")]
        if self.is_immediate_control_batch() {
            return PublicVideoIcePlan::prepare_control_batch(
                self.scope, self.room, grant, config, now,
            );
        }
        PublicVideoIcePlan::prepare(self.scope, self.room, grant, config, now)
    }

    /// Prepare the physical UDP and logical TURN gates for this exact proof.
    /// Does not bind/dial sockets or authorize capture.
    #[cfg(feature = "v2-public-video-wss")]
    pub fn stage_ice(
        self,
        grant: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<PublicVideoScopedIce<'a>, &'static str> {
        let plan = self.prepare_ice(grant, config, now)?;
        PublicVideoScopedIce::new(self, plan, now)
    }

    #[cfg(feature = "v2-public-video-wss")]
    pub(crate) fn seal_video_sdp(
        &mut self,
        challenge: SessionChallenge,
        sdp: &str,
        now: u64,
    ) -> Result<([u8; 64], crate::direct_policy::VerifiedCandidates), &'static str> {
        let values = self.validate_exact_sdp(sdp, self.role == SessionRole::Host, now)?;
        let signature = self.seal(challenge, sdp, now)?;
        Ok((
            signature,
            crate::direct_policy::VerifiedCandidates {
                values,
                scope: self.room,
            },
        ))
    }

    #[cfg(feature = "v2-public-video-wss")]
    pub(crate) fn verify_video_sdp(
        &mut self,
        sdp: &str,
        signature: &[u8; 64],
        now: u64,
    ) -> Result<crate::direct_policy::VerifiedCandidates, &'static str> {
        let sdp = self.open(sdp, signature, now)?;
        let values = match self.validate_exact_sdp(sdp, self.role == SessionRole::Viewer, now) {
            Ok(values) => values,
            Err(error) => {
                // The signed description was authenticated, not admitted.
                // The consumed challenge stays consumed, and rejected SDP
                // cannot become the offer to which this host signs an answer.
                self.offer = None;
                self.outgoing_available = false;
                return Err(error);
            }
        };
        Ok(crate::direct_policy::VerifiedCandidates {
            values,
            scope: self.room,
        })
    }

    pub fn challenge(&self) -> Option<SessionChallenge> {
        self.incoming.as_ref().map(PendingSessionProof::challenge)
    }

    /// One attempt per direction. The host can only sign an answer to the
    /// exact offer it verified; the viewer checks against the offer it signed.
    pub fn seal(
        &mut self,
        peer_challenge: SessionChallenge,
        exact_sdp: &str,
        now: u64,
    ) -> Result<[u8; 64], &'static str> {
        if !std::mem::take(&mut self.outgoing_available) {
            return Err(REJECTED);
        }
        let statement =
            self.scope
                .statement(self.purpose, self.role, self.offer, exact_sdp, now)?;
        let signature = self
            .local
            .sign_session_proof(&self.binding, self.role, peer_challenge, &statement, now)
            .map_err(|_| REJECTED)?;
        if self.role == SessionRole::Viewer {
            self.offer = Some(statement);
        }
        Ok(signature)
    }

    pub fn open<'s>(
        &mut self,
        exact_sdp: &'s str,
        signature: &[u8; 64],
        now: u64,
    ) -> Result<&'s str, &'static str> {
        let mut proof = self.incoming.take().ok_or(REJECTED)?;
        let statement = self.scope.statement(
            self.purpose,
            opposite(self.role),
            self.offer,
            exact_sdp,
            now,
        )?;
        proof
            .verify(signature, &statement, now)
            .map_err(|_| REJECTED)?;
        if self.role == SessionRole::Host {
            self.offer = Some(statement);
        }
        Ok(exact_sdp)
    }
}

/// Authenticates the separate bundled video/control purpose and signed route mode.
/// Does not grant capture, input consent or access to a DataChannel.
#[cfg(feature = "v2-public-control")]
pub struct PublicControlProof<'a>(PublicVideoProof<'a>);

#[cfg(feature = "v2-public-control")]
impl<'a> PublicControlProof<'a> {
    pub fn prepare(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        room: Uuid,
        generation: [u8; 32],
        scope: PublicVideoScope,
        now: u64,
    ) -> Result<Self, &'static str> {
        PublicVideoProof::prepare_for(
            local,
            pinned_peer,
            role,
            room,
            generation,
            scope,
            now,
            Purpose::Control,
        )
        .map(Self)
    }

    pub fn prepare_immediate_batch(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        room: Uuid,
        generation: [u8; 32],
        scope: PublicVideoScope,
        now: u64,
    ) -> Result<Self, &'static str> {
        PublicVideoProof::prepare_for(
            local,
            pinned_peer,
            role,
            room,
            generation,
            scope,
            now,
            Purpose::ControlBatch,
        )
        .map(Self)
    }

    /// Prepare only a device-signed proof for an independently selected
    /// sustained invitation. This grants neither viewing nor keyboard input.
    #[cfg(feature = "v2-public-control-sustained")]
    pub fn prepare_sustained(
        local: &'a Identity,
        pinned_peer: &VerifiedPeer,
        role: SessionRole,
        room: Uuid,
        generation: [u8; 32],
        scope: PublicVideoScope,
        now: u64,
    ) -> Result<Self, &'static str> {
        PublicVideoProof::prepare_for(
            local,
            pinned_peer,
            role,
            room,
            generation,
            scope,
            now,
            Purpose::ControlSustained,
        )
        .map(Self)
    }

    pub fn challenge(&self) -> Option<SessionChallenge> {
        self.0.challenge()
    }

    pub fn seal(
        &mut self,
        challenge: SessionChallenge,
        sdp: &str,
        now: u64,
    ) -> Result<[u8; 64], &'static str> {
        if let Err(error) = self
            .0
            .validate_exact_sdp(sdp, self.0.role == SessionRole::Host, now)
        {
            self.fail();
            return Err(error);
        }
        self.0
            .seal(challenge, sdp, now)
            .inspect_err(|_| self.fail())
    }

    pub fn verify(
        &mut self,
        sdp: &str,
        signature: &[u8; 64],
        now: u64,
    ) -> Result<(), &'static str> {
        let result = (|| {
            self.0.open(sdp, signature, now)?;
            self.0
                .validate_exact_sdp(sdp, self.0.role == SessionRole::Viewer, now)?;
            Ok(())
        })();
        result.inspect_err(|_| self.fail())
    }

    /// Construct the shared scoped driver with the immutable control
    /// purpose. This still does not start ICE or authorize video/input.
    pub fn stage_ice(
        self,
        grant: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<PublicVideoScopedIce<'a>, &'static str> {
        self.into_inner().stage_ice(grant, config, now)
    }

    pub(crate) fn into_inner(self) -> PublicVideoProof<'a> {
        self.0
    }

    fn fail(&mut self) {
        self.0.incoming = None;
        self.0.outgoing_available = false;
        self.0.offer = None;
    }
}

fn opposite(role: SessionRole) -> SessionRole {
    match role {
        SessionRole::Host => SessionRole::Viewer,
        SessionRole::Viewer => SessionRole::Host,
    }
}

#[cfg(test)]
#[path = "public_video_tests.rs"]
mod tests;
