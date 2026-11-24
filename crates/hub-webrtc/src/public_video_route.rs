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

#[derive(Clone, Copy)]
enum Lease {
    Standard,
    #[cfg(feature = "v2-public-control")]
    Batch,
    #[cfg(feature = "v2-public-control-sustained")]
    Sustained,
}

/// Inert route configuration for one reserved video proof, never a substitute
/// for consent, device-signed SDP checks, or the live selected-pair gate.
/// Keep it in native memory; the generated RTC config contains TURN secrets.
pub struct PublicVideoIcePlan {
    scope: PublicVideoScope,
    local: Ipv4Addr,
    pub(crate) grant: RouteGrant,
    config: IceConfig,
    lease: Lease,
}

impl PublicVideoIcePlan {
    pub(crate) fn prepare(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<Self, &'static str> {
        Self::prepare_for(scope, room, relay, config, now, Lease::Standard)
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn prepare_control_batch(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<Self, &'static str> {
        Self::prepare_for(scope, room, relay, config, now, Lease::Batch)
    }

    #[cfg(feature = "v2-public-control-sustained")]
    pub(crate) fn prepare_control_sustained(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
    ) -> Result<Self, &'static str> {
        if scope.expires_at.saturating_sub(now)
            < crate::public_video::SUSTAINED_CONTROL_BOOTSTRAP_MIN_SECONDS
        {
            return Err(CONFIG_MISMATCH);
        }
        Self::prepare_for(scope, room, relay, config, now, Lease::Sustained)
    }

    fn prepare_for(
        scope: PublicVideoScope,
        room: Uuid,
        relay: RelayGrant,
        config: IceConfig,
        now: u64,
        lease: Lease,
    ) -> Result<Self, &'static str> {
        let max = match lease {
            Lease::Standard => {
                scope.checked_mode(now)?;
                None
            }
            #[cfg(feature = "v2-public-control")]
            Lease::Batch => {
                scope.checked_control_batch(now)?;
                Some(1800)
            }
            #[cfg(feature = "v2-public-control-sustained")]
            Lease::Sustained => {
                scope.checked_control_sustained(now)?;
                Some(crate::public_video::SUSTAINED_CONTROL_SCOPE_TTL_SECONDS)
            }
        };
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
        if config.mode != scope.mode
            || config.ice_servers.len() != 1
            || config.ice_servers[0].urls.len() != 1
        {
            return Err(CONFIG_MISMATCH);
        }
        if let Some(max) = max {
            if config.expires_at != scope.expires_at {
                return Err(CONFIG_MISMATCH);
            }
            config
                .validate_mode_with_max(&relay, now, scope.mode, max)
                .map_err(|_| CONFIG_MISMATCH)?;
        } else {
            config
                .validate_mode(&relay, now, scope.mode)
                .map_err(|_| CONFIG_MISMATCH)?;
        }
        let grant = match scope.mode {
            IceMode::DirectPreferred => {
                let grant = match lease {
                    Lease::Standard => DirectGrant::new(relay, Vec::new(), &config, now),
                    #[cfg(feature = "v2-public-control")]
                    Lease::Batch => DirectGrant::new_control_batch(relay, &config, now),
                    #[cfg(feature = "v2-public-control-sustained")]
                    Lease::Sustained => DirectGrant::new_control_sustained(relay, &config, now),
                };
                RouteGrant::Direct(grant.map_err(|_| CONFIG_MISMATCH)?)
            }
            IceMode::RelayOnly => RouteGrant::Relay(relay),
            _ => return Err(CONFIG_MISMATCH),
        };
        Ok(Self {
            scope,
            local: target.local,
            grant,
            config,
            lease,
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
        match self.lease {
            Lease::Standard => {
                self.scope.checked_mode(now)?;
            }
            #[cfg(feature = "v2-public-control")]
            Lease::Batch => {
                self.scope.checked_control_batch(now)?;
            }
            #[cfg(feature = "v2-public-control-sustained")]
            Lease::Sustained => {
                self.scope.checked_control_sustained(now)?;
            }
        }
        match (&self.grant, self.lease) {
            (RouteGrant::Direct(grant), _) => grant.configuration(Some(&self.config), now),
            (RouteGrant::Relay(grant), Lease::Standard) => {
                self.config.native_configuration(grant, now)
            }
            #[cfg(feature = "v2-public-control")]
            (RouteGrant::Relay(grant), Lease::Batch) => self
                .config
                .immediate_control_batch_configuration(grant, now),
            #[cfg(feature = "v2-public-control-sustained")]
            (RouteGrant::Relay(grant), Lease::Sustained) => {
                self.config.control_sustained_configuration(grant, now)
            }
        }
        .map_err(|_| CONFIG_MISMATCH)
    }
}
#[cfg(all(test, feature = "v2-public-control-sustained"))]
mod sustained_tests {
    use super::*;
    use crate::{
        ice_policy::{IceServer, RelayTarget},
        public_video::{
            SUSTAINED_CONTROL_BOOTSTRAP_MIN_SECONDS, SUSTAINED_CONTROL_SCOPE_TTL_SECONDS,
        },
    };

    #[test]
    fn sustained_plan_uses_only_exact_approved_route_and_bounded_credentials() {
        let room = Uuid::new_v4();
        let now = 5_000;
        let fixture = |mode, available| {
            let scope = PublicVideoScope {
                wss: "8.8.8.8:8443".parse().unwrap(),
                turn: "8.8.8.8:3478".parse().unwrap(),
                ca_sha256: [1; 32],
                relay_ports: (49_152, 49_180),
                mode,
                expires_at: now + available,
            };
            let relay = RelayGrant::new(
                RelayTarget {
                    local: "192.168.40.40".parse().unwrap(),
                    turn: scope.turn,
                    relay: *scope.turn.ip(),
                    port_min: scope.relay_ports.0,
                    port_max: scope.relay_ports.1,
                },
                &room.to_string(),
                scope.expires_at,
                true,
            )
            .unwrap();
            let config = IceConfig {
                mode,
                ice_servers: vec![IceServer {
                    urls: vec!["turn:8.8.8.8:3478?transport=udp".to_owned()],
                    username: format!("{}:{room}", scope.expires_at),
                    credential: format!("{}=", "a".repeat(27)),
                }],
                expires_at: scope.expires_at,
                session_id: room.to_string(),
            };
            (scope, relay, config)
        };
        for mode in [IceMode::DirectPreferred, IceMode::RelayOnly] {
            let (scope, relay, config) = fixture(mode, SUSTAINED_CONTROL_SCOPE_TTL_SECONDS);
            let plan =
                PublicVideoIcePlan::prepare_control_sustained(scope, room, relay, config, now)
                    .unwrap();
            assert_eq!(plan.expires_at(), scope.expires_at);
            assert!(plan.rtc_configuration(now).is_ok());
            assert!(plan.rtc_configuration(scope.expires_at).is_err());
            assert_eq!(
                matches!(plan.grant, RouteGrant::Direct(_)),
                mode == IceMode::DirectPreferred
            );

            let (scope, relay, config) = fixture(mode, SUSTAINED_CONTROL_SCOPE_TTL_SECONDS);
            assert!(
                PublicVideoIcePlan::prepare_control_batch(scope, room, relay, config, now).is_err()
            );
            let (scope, relay, config) = fixture(mode, SUSTAINED_CONTROL_BOOTSTRAP_MIN_SECONDS - 1);
            assert!(
                PublicVideoIcePlan::prepare_control_sustained(scope, room, relay, config, now)
                    .is_err()
            );
            let (scope, relay, mut config) = fixture(mode, SUSTAINED_CONTROL_SCOPE_TTL_SECONDS);
            config.expires_at -= 1;
            assert!(
                PublicVideoIcePlan::prepare_control_sustained(scope, room, relay, config, now)
                    .is_err()
            );
        }
    }
}
