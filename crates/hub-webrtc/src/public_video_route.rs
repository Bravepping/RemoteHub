//! Bridge an existing native-approved relay grant to the reserved video proof.
//! Building a plan/config does not open sockets, start media, or grant access.
use crate::{
    direct_policy::DirectGrant,
    ice_policy::{IceConfig, IceMode, RelayGrant},
    public_video::PublicVideoScope,
};
use std::net::Ipv4Addr;
use uuid::Uuid;
use webrtc::peer_connection::RTCConfiguration;

const GRANT_MISMATCH: &str = "PUBLIC_VIDEO_ICE_GRANT_MISMATCH";
const CONFIG_MISMATCH: &str = "PUBLIC_VIDEO_ICE_CONFIG_MISMATCH";

enum RouteGrant {
    Direct(DirectGrant),
    Relay(RelayGrant),
}

/// Inert route configuration for one reserved video proof, never a substitute
/// for consent, device-signed SDP checks, or the live selected-pair gate.
/// Keep it in native memory; the generated RTC config contains TURN secrets.
pub struct PublicVideoIcePlan {
    scope: PublicVideoScope,
    local: Ipv4Addr,
    grant: RouteGrant,
    config: IceConfig,
}

impl PublicVideoIcePlan {
    pub(crate) fn prepare(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<Self, &'static str> {
        scope.checked_mode(now)?;
        let target = relay.target();
        if relay.session() != room.to_string()
            || relay.expires_at() != scope.expires_at
            || target.turn != scope.turn
            || target.relay != *scope.turn.ip()
            || (target.port_min, target.port_max) != scope.relay_ports
        {
            return Err(GRANT_MISMATCH);
        }
        // Exactly one TURN URI. DirectPreferred adds the same endpoint's STUN
        // entry locally; the server cannot supply a second network destination.
        if config.ice_servers.len() != 1 || config.ice_servers[0].urls.len() != 1 {
            return Err(CONFIG_MISMATCH);
        }
        config
            .validate_mode(&relay, now, scope.mode)
            .map_err(|_| CONFIG_MISMATCH)?;
        let grant = match scope.mode {
            IceMode::DirectPreferred => RouteGrant::Direct(
                DirectGrant::new(relay, Vec::new(), &config, now).map_err(|_| CONFIG_MISMATCH)?,
            ),
            IceMode::RelayOnly => RouteGrant::Relay(relay),
            _ => return Err(CONFIG_MISMATCH),
        };
        Ok(Self {
            scope,
            local: target.local,
            grant,
            config,
        })
    }

    pub fn local_ip(&self) -> Ipv4Addr {
        self.local
    }

    pub fn expires_at(&self) -> u64 {
        self.scope.expires_at.min(self.config.expires_at)
    }

    /// Public video does not publish private host candidates.
    pub fn host_candidates_allowed(&self) -> bool {
        match &self.grant {
            RouteGrant::Direct(grant) => grant.host_candidates_allowed(false),
            RouteGrant::Relay(_) => false,
        }
    }

    /// Rechecks expiry and configuration at point of use. The session owner
    /// must still enforce the runtime socket boundary and stop by this expiry.
    pub fn rtc_configuration(&self, now: u64) -> Result<RTCConfiguration, &'static str> {
        self.scope.checked_mode(now)?;
        match &self.grant {
            RouteGrant::Direct(grant) => grant.configuration(Some(&self.config), now),
            RouteGrant::Relay(grant) => self.config.native_configuration(grant, now),
        }
        .map_err(|_| CONFIG_MISMATCH)
    }
}
