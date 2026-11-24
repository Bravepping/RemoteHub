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

pub(crate) enum RouteGrant {
    Direct(DirectGrant),
    Relay(RelayGrant),
}

/// Inert route configuration for one reserved video proof, never a substitute
/// for consent, device-signed SDP checks, or the live selected-pair gate.
/// Keep it in native memory; the generated RTC config contains TURN secrets.
pub struct PublicVideoIcePlan {
    scope: PublicVideoScope,
    local: Ipv4Addr,
    pub(crate) grant: RouteGrant,
    config: IceConfig,
    #[cfg(feature = "v2-public-control")]
    immediate_control_batch: bool,
}

impl PublicVideoIcePlan {
    pub(crate) fn prepare(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<Self, &'static str> {
        Self::prepare_for(scope, room, relay, config, now, false)
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn prepare_control_batch(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<Self, &'static str> {
        Self::prepare_for(scope, room, relay, config, now, true)
    }

    fn prepare_for(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
        immediate_control_batch: bool,
    ) -> Result<Self, &'static str> {
        #[cfg(not(feature = "v2-public-control"))]
        let _ = immediate_control_batch;
        #[cfg(feature = "v2-public-control")]
        if immediate_control_batch {
            scope.checked_control_batch(now)?;
        } else {
            scope.checked_mode(now)?;
        }
        #[cfg(not(feature = "v2-public-control"))]
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
        #[cfg(feature = "v2-public-control")]
        if immediate_control_batch {
            config
                .validate_immediate_control_batch(&relay, now)
                .map_err(|_| CONFIG_MISMATCH)?;
        } else {
            config
                .validate_mode(&relay, now, scope.mode)
                .map_err(|_| CONFIG_MISMATCH)?;
        }
        #[cfg(not(feature = "v2-public-control"))]
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
            #[cfg(feature = "v2-public-control")]
            immediate_control_batch,
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
        #[cfg(feature = "v2-public-control")]
        if self.immediate_control_batch {
            self.scope.checked_control_batch(now)?;
            let RouteGrant::Relay(ref grant) = self.grant else {
                return Err(CONFIG_MISMATCH);
            };
            return self
                .config
                .immediate_control_batch_configuration(grant, now)
                .map_err(|_| CONFIG_MISMATCH);
        }
        self.scope.checked_mode(now)?;
        match &self.grant {
            RouteGrant::Direct(grant) => grant.configuration(Some(&self.config), now),
            RouteGrant::Relay(grant) => self.config.native_configuration(grant, now),
        }
        .map_err(|_| CONFIG_MISMATCH)
    }
}
