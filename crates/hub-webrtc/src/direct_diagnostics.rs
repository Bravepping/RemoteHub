//! Privacy-preserving explanations. Never installs endpoints or grants permission.
use crate::direct_policy::{Candidate, DirectGrant, Kind, VerifiedCandidates};
use serde::Serialize;
use std::net::{Ipv4Addr, SocketAddrV4};
use webrtc::peer_connection::RTCIceCandidate;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    Matched,
    SessionInactive,
    CandidateSetMissing,
    SessionScopeMismatch,
    ProtocolRejected,
    ComponentRejected,
    CandidateTypeRejected,
    AddressRangeRejected,
    TypeMismatch,
    CandidateTypeAbsentFromSdp,
    PortChanged,
    AddressChanged,
    EndpointNotInApprovedSdp,
}
#[derive(Clone, Serialize)]
pub struct SideDiagnostic {
    pub reason: Reason,
    pub candidate_type: &'static str,
    pub protocol: &'static str,
    pub address_class: &'static str,
    pub approved_count: usize,
    pub same_address_count: usize,
    pub same_port_count: usize,
    pub same_type_count: usize,
    pub same_endpoint_count: usize,
    pub exact_match_count: usize,
}
#[derive(Clone, Serialize)]
pub struct PairDiagnostic {
    pub version: u8,
    pub approved: bool,
    pub local_set_source: &'static str,
    pub remote_set_source: &'static str,
    pub private_local_publication_approved: Option<bool>,
    pub local: SideDiagnostic,
    pub remote: SideDiagnostic,
}
fn address_class(ip: Option<Ipv4Addr>) -> &'static str {
    let Some(ip) = ip else {
        return "invalid_or_ipv6";
    };
    let [a, b, _, _] = ip.octets();
    if ip.is_loopback() {
        "loopback"
    } else if ip.is_unspecified() {
        "unspecified"
    } else if ip.is_link_local() {
        "link_local"
    } else if ip.is_multicast() {
        "multicast"
    } else if ip.is_private() {
        "private"
    } else if a == 100 && (64..=127).contains(&b) {
        "shared"
    } else if a == 198 && (b == 18 || b == 19) {
        "benchmark"
    } else if crate::ice_policy::public_v4(ip) {
        "public"
    } else {
        "reserved"
    }
}
fn side(
    grant: &DirectGrant,
    actual: &RTCIceCandidate,
    set: Option<&VerifiedCandidates>,
    local: bool,
    now: u64,
) -> SideDiagnostic {
    let ip = actual.address.parse::<Ipv4Addr>().ok();
    let kind = match actual.typ.to_string().as_str() {
        "host" => Some(Kind::Host),
        "srflx" => Some(Kind::Srflx),
        "relay" => Some(Kind::Relay),
        _ => None,
    };
    let label = match actual.typ.to_string().as_str() {
        "host" => "host",
        "srflx" => "srflx",
        "relay" => "relay",
        "prflx" => "prflx",
        _ => "unsupported",
    };
    let protocol = match actual.protocol.to_string().as_str() {
        "udp" => "udp",
        "tcp" => "tcp",
        _ => "unsupported",
    };
    let values = set.map(|s| s.candidates()).unwrap_or_default();
    let candidate = ip.zip(kind).map(|(ip, kind)| Candidate {
        address: SocketAddrV4::new(ip, actual.port),
        kind,
    });
    let count = |f: &dyn Fn(&Candidate) -> bool| values.iter().filter(|v| f(v)).count();
    let mut d = SideDiagnostic {
        reason: Reason::EndpointNotInApprovedSdp,
        candidate_type: label,
        protocol,
        address_class: address_class(ip),
        approved_count: values.len(),
        same_address_count: count(&|v| Some(*v.address.ip()) == ip),
        same_port_count: count(&|v| v.address.port() == actual.port),
        same_type_count: count(&|v| Some(v.kind) == kind),
        same_endpoint_count: count(&|v| {
            Some(*v.address.ip()) == ip && v.address.port() == actual.port
        }),
        exact_match_count: count(&|v| Some(*v) == candidate),
    };
    d.reason = if !grant.current(now) {
        Reason::SessionInactive
    } else if set.is_none_or(|s| s.candidates().is_empty()) {
        Reason::CandidateSetMissing
    } else if set.is_some_and(|s| s.scope != grant.scope()) {
        Reason::SessionScopeMismatch
    } else if protocol != "udp" {
        Reason::ProtocolRejected
    } else if actual.component != 1 {
        Reason::ComponentRejected
    } else if kind.is_none() {
        Reason::CandidateTypeRejected
    } else if candidate.is_none_or(|c| {
        if local {
            !grant.local_candidate_allowed(&c)
        } else {
            !grant.candidate_allowed(&c)
        }
    }) {
        Reason::AddressRangeRejected
    } else if d.exact_match_count > 0 {
        Reason::Matched
    } else if d.same_endpoint_count > 0 {
        Reason::TypeMismatch
    } else if d.same_type_count == 0 {
        Reason::CandidateTypeAbsentFromSdp
    } else if values
        .iter()
        .any(|v| Some(*v.address.ip()) == ip && Some(v.kind) == kind)
    {
        Reason::PortChanged
    } else if values
        .iter()
        .any(|v| v.address.port() == actual.port && Some(v.kind) == kind)
    {
        Reason::AddressChanged
    } else {
        Reason::EndpointNotInApprovedSdp
    };
    d
}
pub fn inspect(
    grant: &DirectGrant,
    local: &RTCIceCandidate,
    remote: &RTCIceCandidate,
    local_set: Option<&VerifiedCandidates>,
    remote_set: Option<&VerifiedCandidates>,
    now: u64,
) -> PairDiagnostic {
    let local = side(grant, local, local_set, true, now);
    let remote = side(grant, remote, remote_set, false, now);
    PairDiagnostic {
        version: 1,
        approved: local.reason == Reason::Matched && remote.reason == Reason::Matched,
        local_set_source: "validated_published_answer",
        remote_set_source: "authenticated_validated_offer",
        private_local_publication_approved: None,
        local,
        remote,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc::peer_connection::{RTCIceCandidateType, RTCIceProtocol};
    fn actual(ip: &str, port: u16, typ: RTCIceCandidateType) -> RTCIceCandidate {
        RTCIceCandidate {
            address: ip.into(),
            port,
            typ,
            protocol: RTCIceProtocol::Udp,
            component: 1,
            ..Default::default()
        }
    }
    #[test]
    fn precise_failures_remain_rejected_and_export_no_endpoints() {
        let g = DirectGrant::local_diagnostic("192.168.1.2".parse().unwrap(), 1300, true, 1000)
            .unwrap();
        let good = actual("192.168.1.2", 50000, RTCIceCandidateType::Host);
        let set = VerifiedCandidates {
            values: vec![Candidate::parse("x 1 udp 1 192.168.1.2 50000 typ host").unwrap()],
            scope: g.scope(),
        };
        assert!(inspect(&g, &good, &good, Some(&set), Some(&set), 1000).approved);
        let mut changed = good.clone();
        changed.port = 50001;
        let report = inspect(&g, &changed, &good, Some(&set), Some(&set), 1000);
        assert_eq!(report.local.reason, Reason::PortChanged);
        assert!(!report.approved);
        let text = serde_json::to_string(&report).unwrap();
        assert!(
            !text.contains("192.168.1.2") && !text.contains("50000") && !text.contains("50001")
        );
        assert_eq!(
            inspect(&g, &good, &good, None, Some(&set), 1000)
                .local
                .reason,
            Reason::CandidateSetMissing
        );
        assert_eq!(
            inspect(&g, &good, &good, Some(&set), Some(&set), 1300)
                .local
                .reason,
            Reason::SessionInactive
        );
        let mut wrong_scope = set.clone();
        wrong_scope.scope = uuid::Uuid::new_v4();
        assert_eq!(
            inspect(&g, &good, &good, Some(&wrong_scope), Some(&set), 1000)
                .local
                .reason,
            Reason::SessionScopeMismatch
        );
        let mut bad = good.clone();
        bad.address = "169.254.169.254".into();
        assert_eq!(
            inspect(&g, &bad, &good, Some(&set), Some(&set), 1000)
                .local
                .reason,
            Reason::AddressRangeRejected
        );
        bad = good.clone();
        bad.typ = RTCIceCandidateType::Prflx;
        assert_eq!(
            inspect(&g, &bad, &good, Some(&set), Some(&set), 1000)
                .local
                .reason,
            Reason::CandidateTypeRejected
        );
        bad = good.clone();
        bad.protocol = RTCIceProtocol::Tcp;
        assert_eq!(
            inspect(&g, &bad, &good, Some(&set), Some(&set), 1000)
                .local
                .reason,
            Reason::ProtocolRejected
        );
    }
    #[test]
    fn type_and_address_changes_are_diagnostic_only() {
        let (relay, mut ice) = crate::ice_policy::tests::fixture();
        ice.mode = crate::ice_policy::IceMode::DirectPreferred;
        let g = DirectGrant::new(relay, vec![], &ice, 1000).unwrap();
        let a = actual("8.8.4.4", 50000, RTCIceCandidateType::Srflx);
        let set = VerifiedCandidates {
            values: vec![Candidate::parse("x 1 udp 1 8.8.4.4 50000 typ host").unwrap()],
            scope: g.scope(),
        };
        assert_eq!(
            inspect(&g, &a, &a, Some(&set), Some(&set), 1000)
                .local
                .reason,
            Reason::TypeMismatch
        );
        let mut a = a;
        a.typ = RTCIceCandidateType::Host;
        a.address = "1.1.1.1".into();
        assert_eq!(
            inspect(&g, &a, &a, Some(&set), Some(&set), 1000)
                .remote
                .reason,
            Reason::AddressChanged
        );
        a.port = 50002;
        assert_eq!(
            inspect(&g, &a, &a, Some(&set), Some(&set), 1000)
                .remote
                .reason,
            Reason::EndpointNotInApprovedSdp
        );
    }
    #[test]
    fn omitted_host_is_not_silently_reapproved_as_reflexive_candidate() {
        let (relay, mut ice) = crate::ice_policy::tests::fixture();
        ice.mode = crate::ice_policy::IceMode::DirectPreferred;
        let g = DirectGrant::new(relay, vec![], &ice, 1000).unwrap();
        let host = actual(&g.local().to_string(), 50000, RTCIceCandidateType::Host);
        let set = VerifiedCandidates {
            values: vec![Candidate::parse("x 1 udp 1 8.8.4.4 50000 typ srflx").unwrap()],
            scope: g.scope(),
        };
        let report = inspect(&g, &host, &host, Some(&set), Some(&set), 1000);
        assert!(!report.approved);
        assert_eq!(report.local.reason, Reason::CandidateTypeAbsentFromSdp);
        assert!(crate::direct_policy::selected(
            Candidate {
                address: std::net::SocketAddrV4::new(g.local(), 50000),
                kind: Kind::Host
            },
            set.values[0],
            &set,
            &set
        )
        .is_err());
        println!(
            "DIAG_OFFLINE_FIXTURE {}",
            serde_json::to_string(&report).unwrap()
        );
    }
}
