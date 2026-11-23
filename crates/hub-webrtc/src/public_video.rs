//! Offline proof contract for public video. Verifying an SDP proves its sender
//! and approved route context, not end-user consent or candidate eligibility.
use crate::ice_policy::{public_v4, IceConfig, IceMode, RelayGrant};
pub use crate::public_video_route::PublicVideoIcePlan;
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
const PROOF_TTL: u64 = 15;
const INVALID_SCOPE: &str = "PUBLIC_VIDEO_SCOPE_INVALID";
const EXPIRED: &str = "PUBLIC_VIDEO_SCOPE_EXPIRED";
const IDENTITY: &str = "PUBLIC_VIDEO_IDENTITY_MISMATCH";
const SDP: &str = "PUBLIC_VIDEO_SDP_INVALID";
const REJECTED: &str = "PUBLIC_VIDEO_PROOF_REJECTED";

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
        if !(1..=MAX_SCOPE_TTL).contains(&self.expires_at.saturating_sub(now)) {
            return Err(EXPIRED);
        }
        Ok(mode)
    }

    fn statement(
        self,
        role: SessionRole,
        offer: Option<[u8; 32]>,
        exact_sdp: &str,
        now: u64,
    ) -> Result<[u8; 32], &'static str> {
        let mode = self.checked_mode(now)?;
        if exact_sdp.is_empty() || exact_sdp.len() > MAX_SIGNAL_BYTES || exact_sdp.contains('\0') {
            return Err(SDP);
        }
        let mut hash = Sha256::new();
        hash.update(b"RemoteHub.V2.public-video-only.sdp.v1\0");
        // Viewer signs the offer, host signs the answer. There is no input variant.
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
        scope.checked_mode(now)?;
        let binding = pinned_peer
            .session_binding(&local.public(), role, *room.as_bytes(), session_nonce)
            .map_err(|_| IDENTITY)?;
        let ttl = Duration::from_secs(PROOF_TTL.min(scope.expires_at - now));
        let incoming =
            PendingSessionProof::issue(&binding, opposite(role), now, ttl).map_err(|_| REJECTED)?;
        Ok(Self {
            local,
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

    /// Validate a native-issued route grant against exactly this proof's
    /// reserved context, before using it for ICE. Does not authorize media.
    pub fn prepare_ice(
        &self,
        grant: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<PublicVideoIcePlan, &'static str> {
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
        let values = crate::public_video_ice::validate(
            sdp,
            self.scope,
            self.role == SessionRole::Host,
            now,
        )?;
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
        let values = match crate::public_video_ice::validate(
            sdp,
            self.scope,
            self.role == SessionRole::Viewer,
            now,
        ) {
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
        let statement = self
            .scope
            .statement(self.role, self.offer, exact_sdp, now)?;
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
        let statement = self
            .scope
            .statement(opposite(self.role), self.offer, exact_sdp, now)?;
        proof
            .verify(signature, &statement, now)
            .map_err(|_| REJECTED)?;
        if self.role == SessionRole::Host {
            self.offer = Some(statement);
        }
        Ok(exact_sdp)
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
