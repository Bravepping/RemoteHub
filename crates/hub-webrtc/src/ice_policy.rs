//! Explicit TURN preparation policy; does not change NetworkScope or LAN constructors.
use serde::{Deserialize, Serialize};
use std::net::{Ipv4Addr, SocketAddrV4};
use webrtc::peer_connection::{
    RTCConfiguration, RTCConfigurationBuilder, RTCIceServer, RTCIceTransportPolicy,
};
use zeroize::Zeroize;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IceMode {
    Loopback,
    LanPinned,
    RelayOnly,
    DirectPreferred,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}
impl Drop for IceServer {
    fn drop(&mut self) {
        self.credential.zeroize();
        self.username.zeroize();
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IceConfig {
    pub mode: IceMode,
    pub ice_servers: Vec<IceServer>,
    pub expires_at: u64,
    pub session_id: String,
}
impl std::fmt::Debug for IceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IceConfig")
            .field("mode", &self.mode)
            .field("secrets", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
#[derive(Clone, Copy)]
pub struct RelayTarget {
    pub local: Ipv4Addr,
    pub turn: SocketAddrV4,
    pub relay: Ipv4Addr,
    pub port_min: u16,
    pub port_max: u16,
}
pub struct RelayGrant {
    target: RelayTarget,
    session: String,
    expires_at: u64,
}
pub fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_unspecified()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && a != 0
        && a < 224
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 192 && b == 0)
        && !(a == 198 && (b == 18 || b == 19))
}
impl RelayGrant {
    pub(crate) fn session(&self) -> &str {
        &self.session
    }
    #[cfg(feature = "p006-direct-probe")]
    pub(crate) fn expires_at(&self) -> u64 {
        self.expires_at
    }
    /// Called only after native experiment approval and authenticated session exchange.
    /// No Tauri command or remote service constructs grants in P006a.
    pub fn new(
        target: RelayTarget,
        session: &str,
        expires_at: u64,
        both_approved: bool,
    ) -> Result<Self, String> {
        if !both_approved {
            return Err("RELAY_APPROVAL_REQUIRED".into());
        }
        let id = uuid::Uuid::parse_str(session).map_err(|_| "RELAY_SESSION_INVALID")?;
        if id.is_nil()
            || !(target.local.is_private() || public_v4(target.local))
            || !public_v4(*target.turn.ip())
            || !public_v4(target.relay)
            || target.turn.port() < 1024
            || target.port_min < 49152
            || target.port_max < target.port_min
            || target.port_max as u32 - target.port_min as u32 > 255
        {
            return Err("RELAY_TARGET_INVALID".into());
        }
        Ok(Self {
            target,
            session: id.to_string(),
            expires_at,
        })
    }
    pub fn target(&self) -> RelayTarget {
        self.target
    }
    pub fn is_current(&self, now: u64) -> bool {
        now < self.expires_at
    }
    pub fn permits_candidate(&self, ip: Ipv4Addr, port: u16) -> bool {
        ip == self.target.relay && (self.target.port_min..=self.target.port_max).contains(&port)
    }
}
impl IceConfig {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > 8192 {
            return Err("ICE_CONFIG_SIZE".into());
        }
        serde_json::from_str(text).map_err(|_| "ICE_CONFIG_INVALID".into())
    }
    pub fn validate(&self, grant: &RelayGrant, now: u64) -> Result<(), String> {
        self.validate_mode(grant, now, IceMode::RelayOnly)
    }
    pub(crate) fn validate_mode(
        &self,
        grant: &RelayGrant,
        now: u64,
        expected: IceMode,
    ) -> Result<(), String> {
        self.validate_mode_with_max(grant, now, expected, 600)
    }

    #[cfg(feature = "v2-public-control")]
    pub fn validate_immediate_control_batch(
        &self,
        grant: &RelayGrant,
        now: u64,
    ) -> Result<(), String> {
        if self.expires_at != grant.expires_at {
            return Err("ICE_SESSION_EXPIRED_OR_MISMATCHED".into());
        }
        self.validate_mode_with_max(grant, now, IceMode::RelayOnly, 1800)
    }

    pub(crate) fn validate_mode_with_max(
        &self,
        grant: &RelayGrant,
        now: u64,
        expected: IceMode,
        max_ttl: u64,
    ) -> Result<(), String> {
        if self.mode != expected {
            return Err("ICE_MODE_NOT_ENABLED_IN_P006A".into());
        }
        if !grant.is_current(now)
            || self.session_id != grant.session
            || self.expires_at > grant.expires_at
            || !(60..=max_ttl).contains(&self.expires_at.saturating_sub(now))
        {
            return Err("ICE_SESSION_EXPIRED_OR_MISMATCHED".into());
        }
        if self.ice_servers.is_empty() || self.ice_servers.len() > 2 {
            return Err("ICE_SERVERS_LIMIT".into());
        }
        for server in &self.ice_servers {
            if server.urls.is_empty()
                || server.urls.len() > 2
                || server.credential.len() != 28
                || !server.credential.ends_with('=')
                || !server.credential.as_bytes()[..27]
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/".contains(b))
                || server.username != format!("{}:{}", self.expires_at, grant.session)
            {
                return Err("ICE_SHORT_CREDENTIAL_REQUIRED".into());
            }
            let expected = format!(
                "turn:{}:{}?transport=udp",
                grant.target.turn.ip(),
                grant.target.turn.port()
            );
            if server.urls.iter().any(|url| url != &expected) {
                return Err("ICE_UDP_ENDPOINT_NOT_APPROVED".into());
            }
        }
        Ok(())
    }
    pub fn native_configuration(
        &self,
        grant: &RelayGrant,
        now: u64,
    ) -> Result<RTCConfiguration, String> {
        self.validate(grant, now)?;
        Ok(self.relay_configuration())
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn immediate_control_batch_configuration(
        &self,
        grant: &RelayGrant,
        now: u64,
    ) -> Result<RTCConfiguration, String> {
        self.validate_immediate_control_batch(grant, now)?;
        Ok(self.relay_configuration())
    }

    fn relay_configuration(&self) -> RTCConfiguration {
        let servers = self
            .ice_servers
            .iter()
            .map(|s| RTCIceServer {
                urls: s.urls.clone(),
                username: s.username.clone(),
                credential: s.credential.clone(),
            })
            .collect();
        RTCConfigurationBuilder::default()
            .with_ice_servers(servers)
            .with_ice_transport_policy(RTCIceTransportPolicy::Relay)
            .build()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub const ID: &str = "00000000-0000-4000-8000-000000000001";
    pub fn fixture() -> (RelayGrant, IceConfig) {
        // These are pure config fixtures; no packets are sent to these addresses.
        let target = RelayTarget {
            local: "127.0.0.1".parse().unwrap(),
            turn: "8.8.8.8:3478".parse().unwrap(),
            relay: "8.8.8.8".parse().unwrap(),
            port_min: 49160,
            port_max: 49259,
        };
        // Test-only loopback bind permits a real no-gather PC build; public constructor rejects it.
        let grant = RelayGrant {
            target,
            session: ID.into(),
            expires_at: 1600,
        };
        let config = IceConfig {
            mode: IceMode::RelayOnly,
            ice_servers: vec![IceServer {
                urls: vec!["turn:8.8.8.8:3478?transport=udp".into()],
                username: format!("1300:{ID}"),
                credential: "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
            }],
            expires_at: 1300,
            session_id: ID.into(),
        };
        (grant, config)
    }
    #[test]
    fn real_native_config_uses_relay_policy_and_redacts_credentials() {
        let (g, c) = fixture();
        let config = c.native_configuration(&g, 1000).unwrap();
        assert_eq!(config.ice_transport_policy(), RTCIceTransportPolicy::Relay);
        let debug = format!("{c:?}");
        assert!(!debug.contains("AAAAAAAA"));
        assert!(!debug.contains(ID));
        assert!(!debug.contains("8.8.8.8"));
    }
    #[test]
    fn rejects_expiry_modes_unapproved_endpoint_tcp_tls_dns_and_private_relays() {
        let (g, mut c) = fixture();
        for mode in [
            IceMode::DirectPreferred,
            IceMode::LanPinned,
            IceMode::Loopback,
        ] {
            c.mode = mode;
            assert!(c.validate(&g, 1000).is_err());
        }
        c.mode = IceMode::RelayOnly;
        assert!(c.validate(&g, 1300).is_err());
        c.expires_at = 1700;
        assert!(c.validate(&g, 1000).is_err());
        c.expires_at = 1300;
        c.ice_servers[0].credential = "=".repeat(28);
        assert!(c.validate(&g, 1000).is_err());
        c.ice_servers[0].credential = String::new();
        assert!(c.validate(&g, 1000).is_err());
        c.ice_servers[0].credential = "AAAAAAAAAAAAAAAAAAAAAAAAAAA=".into();
        for url in [
            "turns:8.8.8.8:5349",
            "turn:8.8.8.8:3478?transport=tcp",
            "turn:8.8.4.4:3478?transport=udp",
            "turn:example.invalid:3478?transport=udp",
        ] {
            c.ice_servers[0].urls = vec![url.into()];
            assert!(c.validate(&g, 1000).is_err());
        }
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "224.0.0.1",
            "0.0.0.0",
            "198.18.0.1",
            "203.0.113.1",
        ] {
            assert!(!public_v4(ip.parse().unwrap()));
        }
        assert!(RelayGrant::new(g.target(), ID, 1600, false).is_err());
        assert!(RelayGrant::new(g.target(), ID, 1600, true).is_err()); // loopback bind
        assert!(IceConfig::parse(&"x".repeat(8193)).is_err());
    }
}
