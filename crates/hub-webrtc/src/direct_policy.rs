//! P006c opt-in policy. Existing loopback/LAN/relay-only entry points are unchanged.
use crate::ice_policy::{public_v4, IceConfig, IceMode, RelayGrant};
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddrV4};
use webrtc::peer_connection::{
    RTCConfiguration, RTCConfigurationBuilder, RTCIceServer, RTCIceTransportPolicy,
};

pub struct DirectGrant {
    relay: Option<RelayGrant>,
    local: Ipv4Addr,
    private_peers: Vec<Ipv4Addr>,
    expires_at: u64,
    max_ttl: u64,
    scope: uuid::Uuid,
}
impl DirectGrant {
    /// Business callers must authenticate the session and obtain both native approvals first.
    /// No desktop IPC constructs this grant in the first P006c slice.
    pub fn new(
        relay: RelayGrant,
        private_peers: Vec<Ipv4Addr>,
        config: &IceConfig,
        now: u64,
    ) -> Result<Self, String> {
        Self::new_with_ttl(relay, private_peers, config, now, 600)
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn new_control_batch(
        relay: RelayGrant,
        config: &IceConfig,
        now: u64,
    ) -> Result<Self, String> {
        if config.expires_at != relay.expires_at() {
            return Err("DIRECT_CONFIG_EXPIRY_MISMATCH".into());
        }
        Self::new_with_ttl(relay, Vec::new(), config, now, 1800)
    }

    fn new_with_ttl(
        relay: RelayGrant,
        private_peers: Vec<Ipv4Addr>,
        config: &IceConfig,
        now: u64,
        max_ttl: u64,
    ) -> Result<Self, String> {
        config.validate_mode_with_max(&relay, now, IceMode::DirectPreferred, max_ttl)?;
        if !relay.is_current(now)
            || private_peers.len() > 2
            || private_peers.iter().any(|p| !p.is_private())
        {
            return Err("DIRECT_GRANT_SCOPE".into());
        }
        let local = relay.target().local;
        let scope = uuid::Uuid::parse_str(relay.session()).map_err(|_| "DIRECT_SESSION_INVALID")?;
        // TTL comes from the same authenticated session, never a fresh local extension.
        let expires_at = config.expires_at.min(relay.expires_at());
        if !(60..=max_ttl).contains(&expires_at.saturating_sub(now)) {
            return Err("DIRECT_GRANT_EXPIRY".into());
        }
        Ok(Self {
            relay: Some(relay),
            local,
            private_peers,
            expires_at,
            max_ttl,
            scope,
        })
    }
    /// Explicit operator fixture: only this one private local address, no STUN/TURN/public peers.
    pub fn local_diagnostic(
        local: Ipv4Addr,
        expires_at: u64,
        approved: bool,
        now: u64,
    ) -> Result<Self, String> {
        if !approved || !local.is_private() || !(60..=600).contains(&expires_at.saturating_sub(now))
        {
            return Err("DIRECT_LOCAL_APPROVAL_REQUIRED".into());
        }
        Ok(Self {
            relay: None,
            local,
            private_peers: vec![local],
            expires_at,
            max_ttl: 600,
            scope: uuid::Uuid::new_v4(),
        })
    }
    /// Real sockets in unit tests only; production grants still reject loopback.
    #[cfg(test)]
    pub(crate) fn loopback_test(now: u64) -> Self {
        Self {
            relay: None,
            local: Ipv4Addr::LOCALHOST,
            private_peers: vec![Ipv4Addr::LOCALHOST],
            expires_at: now + 120,
            max_ttl: 600,
            scope: uuid::Uuid::new_v4(),
        }
    }

    #[cfg(feature = "p006-session-probe")]
    pub fn local_session_diagnostic(
        local: Ipv4Addr,
        expires: u64,
        session: &str,
        now: u64,
    ) -> Result<Self, String> {
        let mut grant = Self::local_diagnostic(local, expires, true, now)?;
        let id = uuid::Uuid::parse_str(session).map_err(|_| "DIRECT_LOCAL_SESSION_INVALID")?;
        if id.is_nil() {
            return Err("DIRECT_LOCAL_SESSION_INVALID".into());
        }
        grant.scope = id;
        Ok(grant)
    }
    pub fn local(&self) -> Ipv4Addr {
        self.local
    }
    pub(crate) fn host_candidates_allowed(&self, private_local_approved: bool) -> bool {
        !self.local.is_private() || private_local_approved
    }
    pub(crate) fn scope(&self) -> uuid::Uuid {
        self.scope
    }
    pub fn session_id(&self) -> String {
        self.scope.to_string()
    }
    pub fn local_candidate_allowed(&self, candidate: &Candidate) -> bool {
        (candidate.kind == Kind::Host
            && *candidate.address.ip() == self.local
            && candidate.address.port() >= 1024)
            || self.candidate_allowed(candidate)
    }
    pub(crate) fn permits_address(&self, ip: Ipv4Addr, local: bool) -> bool {
        (local && ip == self.local)
            || self.private_peers.contains(&ip)
            || (self.relay.is_some() && public_v4(ip))
    }
    pub fn expires_at(&self) -> u64 {
        self.expires_at
    }
    pub fn current(&self, now: u64) -> bool {
        now < self.expires_at
    }
    pub fn server(&self) -> Option<SocketAddrV4> {
        self.relay.as_ref().map(|g| g.target().turn)
    }
    pub fn candidate_allowed(&self, candidate: &Candidate) -> bool {
        let ip = *candidate.address.ip();
        match candidate.kind {
            Kind::Relay => self
                .relay
                .as_ref()
                .is_some_and(|g| g.permits_candidate(ip, candidate.address.port())),
            Kind::Host => {
                candidate.address.port() >= 1024
                    && (self.private_peers.contains(&ip) || (self.relay.is_some() && public_v4(ip)))
            }
            Kind::Srflx => {
                candidate.address.port() >= 1024 && self.relay.is_some() && public_v4(ip)
            }
        }
    }
    pub fn configuration(
        &self,
        config: Option<&IceConfig>,
        now: u64,
    ) -> Result<RTCConfiguration, String> {
        if !self.current(now) {
            return Err("DIRECT_APPROVAL_EXPIRED".into());
        }
        let mut servers = vec![];
        match (&self.relay, config) {
            (Some(grant), Some(config)) => {
                config.validate_mode_with_max(
                    grant,
                    now,
                    IceMode::DirectPreferred,
                    self.max_ttl,
                )?;
                if config.expires_at != self.expires_at {
                    return Err("DIRECT_CONFIG_EXPIRY_MISMATCH".into());
                }
                servers.extend(config.ice_servers.iter().map(|s| RTCIceServer {
                    urls: s.urls.clone(),
                    username: s.username.clone(),
                    credential: s.credential.clone(),
                }));
                let endpoint = grant.target().turn;
                servers.push(RTCIceServer {
                    urls: vec![format!("stun:{endpoint}")],
                    ..Default::default()
                });
            }
            (None, None) => (),
            _ => return Err("DIRECT_CONFIG_SCOPE_MISMATCH".into()),
        }
        Ok(RTCConfigurationBuilder::default()
            .with_ice_servers(servers)
            .with_ice_transport_policy(RTCIceTransportPolicy::All)
            .build())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Host,
    Srflx,
    Relay,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub address: SocketAddrV4,
    pub kind: Kind,
}
impl Candidate {
    pub fn parse(text: &str) -> Result<Self, String> {
        let f: Vec<_> = text.split_whitespace().collect();
        if f.len() < 8
            || f.len() > 32
            || f[1] != "1"
            || !f[2].eq_ignore_ascii_case("udp")
            || f[6] != "typ"
            || f[0].is_empty()
            || f[0].len() > 64
            || f[3].parse::<u32>().is_err()
        {
            return Err("DIRECT_CANDIDATE_FORMAT".into());
        }
        let ip: Ipv4Addr = f[4].parse().map_err(|_| "DIRECT_CANDIDATE_IP")?;
        let port: u16 = f[5].parse().map_err(|_| "DIRECT_CANDIDATE_PORT")?;
        let kind = match f[7] {
            "host" => Kind::Host,
            "srflx" => Kind::Srflx,
            "relay" => Kind::Relay,
            _ => return Err("DIRECT_CANDIDATE_TYPE".into()),
        };
        Ok(Self {
            address: SocketAddrV4::new(ip, port),
            kind,
        })
    }
}
#[derive(Clone)]
pub struct VerifiedCandidates {
    pub(crate) values: Vec<Candidate>,
    pub(crate) scope: uuid::Uuid,
}
impl VerifiedCandidates {
    pub fn candidates(&self) -> &[Candidate] {
        &self.values
    }
}
pub(crate) fn candidates(
    sdp: &str,
    grant: &DirectGrant,
    now: u64,
) -> Result<VerifiedCandidates, String> {
    scoped_candidates(sdp, grant, now, false)
}
pub(crate) fn scoped_candidates(
    sdp: &str,
    grant: &DirectGrant,
    now: u64,
    local: bool,
) -> Result<VerifiedCandidates, String> {
    if !grant.current(now) || sdp.len() > 65536 || sdp.contains('\0') {
        return Err("DIRECT_SDP_EXPIRED_OR_SIZE".into());
    }
    let mut values = vec![];
    for line in sdp.lines().map(str::trim) {
        if let Some(text) = line.strip_prefix("a=candidate:") {
            if values.len() >= 16 {
                return Err("DIRECT_CANDIDATE_BUDGET".into());
            }
            let c = Candidate::parse(text)?;
            if !(if local {
                grant.local_candidate_allowed(&c)
            } else {
                grant.candidate_allowed(&c)
            }) {
                return Err("DIRECT_CANDIDATE_SCOPE".into());
            }
            values.push(c);
        }
    }
    if values.is_empty() {
        return Err("DIRECT_CANDIDATE_REQUIRED".into());
    }
    Ok(VerifiedCandidates {
        values,
        scope: grant.scope,
    })
}
/// Publish only component 1 and private host IPs explicitly allowed by the other side.
/// Remaining candidates are strictly validated, not silently accepted.
pub fn publish_sdp(sdp: &str, grant: &DirectGrant, private_local_allowed: bool) -> String {
    sdp.lines()
        .filter(|l| {
            if !l.starts_with("a=candidate:") {
                return true;
            }
            let f: Vec<_> = l[12..].split_whitespace().collect();
            if f.get(1) == Some(&"2") {
                return false;
            }
            !(f.get(7) == Some(&"host")
                && f.get(4).is_some_and(|p| *p == grant.local.to_string())
                && !grant.host_candidates_allowed(private_local_allowed))
        })
        .map(|l| {
            if l.starts_with("c=") {
                "c=IN IP4 0.0.0.0"
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
        + "\r\n"
}
/// Diagnostic single application policy, separate from production video+control validation.
pub fn data_sdp(sdp: &str, grant: &DirectGrant, now: u64) -> Result<VerifiedCandidates, String> {
    let checked = candidates(sdp, grant, now)?;
    let lines: Vec<_> = sdp.lines().map(str::trim).collect();
    let media: Vec<_> = lines.iter().filter(|s| s.starts_with("m=")).collect();
    let mids: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=mid:"))
        .collect();
    let bundle: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=group:BUNDLE "))
        .collect();
    let hashes: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=fingerprint:sha-256 "))
        .collect();
    let ports: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=sctp-port:"))
        .collect();
    let ufrags: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=ice-ufrag:"))
        .collect();
    let passwords: Vec<_> = lines
        .iter()
        .filter_map(|s| s.strip_prefix("a=ice-pwd:"))
        .collect();
    if media.len() != 1
        || !media[0].starts_with("m=application ")
        || media[0].split_whitespace().count() != 4
        || !media[0].contains(" UDP/DTLS/SCTP webrtc-datachannel")
        || media[0]
            .split_whitespace()
            .nth(1)
            .is_none_or(|p| !p.parse::<u16>().is_ok_and(|v| v > 0))
        || mids.len() != 1
        || mids[0].is_empty()
        || mids[0].len() > 32
        || bundle.len() != 1
        || bundle[0] != mids[0]
        || hashes.is_empty()
        || hashes.len() > 2
        || hashes.iter().any(|h| *h != hashes[0])
        || ufrags.len() != 1
        || !(4..=256).contains(&ufrags[0].len())
        || passwords.len() != 1
        || !(22..=256).contains(&passwords[0].len())
        || hashes.iter().any(|h| {
            h.split(':').count() != 32
                || h.split(':')
                    .any(|p| p.len() != 2 || u8::from_str_radix(p, 16).is_err())
        })
        || ports.len() != 1
        || !ports[0].parse::<u16>().is_ok_and(|v| v > 0)
        || lines.iter().any(|s| {
            s.starts_with("a=remote-candidates:")
                || s.starts_with("a=sctpmap:")
                || (s.starts_with("c=") && *s != "c=IN IP4 0.0.0.0")
        })
    {
        return Err("DIRECT_DATA_SDP_POLICY".into());
    }
    Ok(checked)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectedRoute {
    PrivateHostDirect,
    PublicHostDirect,
    ServerReflexiveDirect,
    TurnRelay,
}

#[derive(Clone, Serialize)]
pub struct PathSummary {
    pub local_type: Kind,
    pub remote_type: Kind,
    pub protocol: &'static str,
    pub path: &'static str,
    pub route: SelectedRoute,
}
pub fn selected(
    local: Candidate,
    remote: Candidate,
    local_candidates: &VerifiedCandidates,
    remote_candidates: &VerifiedCandidates,
) -> Result<PathSummary, String> {
    if local_candidates.scope != remote_candidates.scope {
        return Err("DIRECT_SELECTED_PAIR_SCOPE_MISMATCH".into());
    }
    if !local_candidates.values.contains(&local) || !remote_candidates.values.contains(&remote) {
        return Err("DIRECT_SELECTED_PAIR_NOT_APPROVED".into());
    }
    let route = match (local.kind, remote.kind) {
        (Kind::Relay, _) | (_, Kind::Relay) => SelectedRoute::TurnRelay,
        (Kind::Srflx, _) | (_, Kind::Srflx) => SelectedRoute::ServerReflexiveDirect,
        (Kind::Host, Kind::Host)
            if public_v4(*local.address.ip()) || public_v4(*remote.address.ip()) =>
        {
            SelectedRoute::PublicHostDirect
        }
        (Kind::Host, Kind::Host) => SelectedRoute::PrivateHostDirect,
    };
    Ok(PathSummary {
        local_type: local.kind,
        remote_type: remote.kind,
        protocol: "udp",
        path: if route == SelectedRoute::TurnRelay {
            "relay"
        } else {
            "direct"
        },
        route,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grant() -> DirectGrant {
        DirectGrant::local_diagnostic("192.168.1.2".parse().unwrap(), 1300, true, 1000).unwrap()
    }
    fn sdp() -> String {
        format!(
            "v=0\r\na=group:BUNDLE 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:0\r\nc=IN IP4 0.0.0.0\r\na=ice-ufrag:abcd\r\na=ice-pwd:abcdefghijklmnopqrstuvwx\r\na=fingerprint:sha-256 {}\r\na=sctp-port:5000\r\na=candidate:x 1 udp 123 192.168.1.2 50000 typ host\r\n",
            ["AB"; 32].join(":")
        )
    }
    #[test]
    fn private_native_host_gathering_needs_the_same_publication_approval() {
        let g = grant();
        assert!(!g.host_candidates_allowed(false));
        assert!(g.host_candidates_allowed(true));
        assert!(!publish_sdp(&sdp(), &g, false).contains("typ host"));
        assert!(publish_sdp(&sdp(), &g, true).contains("typ host"));
    }

    #[test]
    fn rejects_scope_size_fingerprint_media_expiry_and_dangerous_addresses() {
        let g = grant();
        let good = sdp();
        assert!(data_sdp(&good, &g, 1000).is_ok());
        for ip in [
            "192.168.1.3",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "198.18.0.1",
            "224.0.0.1",
            "0.0.0.0",
            "8.8.8.8",
            "device.local",
            "::1",
        ] {
            assert!(data_sdp(&good.replace("192.168.1.2", ip), &g, 1000).is_err());
        }
        for bad in [
            good.replace("udp", "tcp"),
            good.replace("typ host", "typ prflx"),
            good.replace("AB:AB", "AB:ZZ"),
            good.clone() + "m=video 9 UDP/TLS/RTP/SAVPF 102\r\n",
            good.replace("50000 typ", "80 typ"),
            good.clone() + &"a=candidate:x 1 udp 123 192.168.1.2 50001 typ host\r\n".repeat(16),
        ] {
            assert!(data_sdp(&bad, &g, 1000).is_err());
        }
        assert!(data_sdp(&good, &g, 1300).is_err());
        assert!(DirectGrant::local_diagnostic(g.local, 1300, false, 1000).is_err());
        assert!(data_sdp(&"x".repeat(65537), &g, 1000).is_err());
    }
    #[test]
    fn selected_route_reports_only_approved_candidate_types_without_endpoints() {
        let (relay, mut config) = crate::ice_policy::tests::fixture();
        config.mode = IceMode::DirectPreferred;
        let g =
            DirectGrant::new(relay, vec!["192.168.1.2".parse().unwrap()], &config, 1000).unwrap();
        let values = candidates(
            "a=candidate:a 1 udp 1 192.168.1.2 50000 typ host\r\n\
             a=candidate:b 1 udp 1 8.8.8.8 50001 typ host\r\n\
             a=candidate:c 1 udp 1 8.8.8.8 50002 typ srflx\r\n\
             a=candidate:d 1 udp 1 8.8.8.8 49160 typ relay\r\n",
            &g,
            1000,
        )
        .unwrap();
        let c = values.candidates();
        for (a, b, route, label) in [
            (
                0,
                0,
                SelectedRoute::PrivateHostDirect,
                "private_host_direct",
            ),
            (0, 1, SelectedRoute::PublicHostDirect, "public_host_direct"),
            (
                2,
                0,
                SelectedRoute::ServerReflexiveDirect,
                "server_reflexive_direct",
            ),
            (2, 3, SelectedRoute::TurnRelay, "turn_relay"),
            (3, 0, SelectedRoute::TurnRelay, "turn_relay"),
        ] {
            let pair = selected(c[a], c[b], &values, &values).unwrap();
            assert_eq!(pair.route, route);
            let report = serde_json::to_value(pair).unwrap();
            assert_eq!(report["route"], label);
            assert_eq!(report.as_object().unwrap().len(), 5);
            assert!(!report.to_string().contains("8.8.8.8"));
            assert!(!report.to_string().contains("192.168.1.2"));
        }
    }

    #[test]
    fn direct_is_explicit_and_old_relay_entry_remains_closed() {
        let (g, mut c) = crate::ice_policy::tests::fixture();
        c.mode = IceMode::DirectPreferred;
        assert!(c.native_configuration(&g, 1000).is_err());
        let direct = DirectGrant::new(g, vec![], &c, 1000).unwrap();
        assert_eq!(
            direct
                .configuration(Some(&c), 1000)
                .unwrap()
                .ice_transport_policy(),
            RTCIceTransportPolicy::All
        );
        assert!(direct.configuration(None, 1000).is_err());
        let c1 = Candidate {
            address: "8.8.8.8:49160".parse().unwrap(),
            kind: Kind::Relay,
        };
        let c2 = Candidate {
            address: "192.168.1.2:50000".parse().unwrap(),
            kind: Kind::Host,
        };
        let v = VerifiedCandidates {
            values: vec![c1, c2],
            scope: direct.scope,
        };
        assert_eq!(selected(c1, c2, &v, &v).unwrap().path, "relay");
        assert_eq!(selected(c2, c2, &v, &v).unwrap().path, "direct");
        let mut foreign = v.clone();
        foreign.scope = uuid::Uuid::new_v4();
        assert!(selected(c2, c2, &v, &foreign).is_err());
        assert!(selected(
            c1,
            c2,
            &VerifiedCandidates {
                values: vec![],
                scope: direct.scope
            },
            &v
        )
        .is_err());
    }
}
