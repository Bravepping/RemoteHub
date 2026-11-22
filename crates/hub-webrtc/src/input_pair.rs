//! Endpoint-free diagnostics for the product's strict authenticated LAN candidate check.
use serde::Serialize;
use std::net::Ipv4Addr;

#[derive(Clone, Serialize)]
pub struct CandidateDiagnostic {
    pub approved: bool,
    pub reason: &'static str,
    pub candidate_type: &'static str,
    pub protocol: &'static str,
    pub approved_count: usize,
    pub same_address_count: usize,
    pub same_port_count: usize,
    pub exact_match_count: usize,
}
#[derive(Clone, Serialize)]
pub struct PairDiagnostic {
    pub local: CandidateDiagnostic,
    pub remote: CandidateDiagnostic,
}
pub fn diagnose(
    sdp: &str,
    address: &str,
    port: u16,
    protocol: &str,
    kind: &str,
    expected: Ipv4Addr,
) -> CandidateDiagnostic {
    let rows: Vec<Vec<_>> = sdp
        .lines()
        .filter_map(|l| l.strip_prefix("a=candidate:"))
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|p| p.len() >= 8 && p[1] == "1" && p[6] == "typ")
        .collect();
    let same_address_count = rows.iter().filter(|p| p[4] == address).count();
    let same_port_count = rows
        .iter()
        .filter(|p| p[5].parse::<u16>() == Ok(port))
        .count();
    let exact_match_count = rows
        .iter()
        .filter(|p| {
            p[4] == address
                && p[5].parse::<u16>() == Ok(port)
                && p[2].eq_ignore_ascii_case(protocol)
                && p[7] == kind
        })
        .count();
    let reason = if address.parse::<Ipv4Addr>() != Ok(expected) {
        "ADDRESS_OUT_OF_APPROVED_SCOPE"
    } else if protocol != "udp" {
        "PROTOCOL_NOT_APPROVED"
    } else if kind != "host" {
        "CANDIDATE_TYPE_NOT_APPROVED"
    } else if rows.is_empty() {
        "NO_PUBLISHED_CANDIDATES"
    } else if exact_match_count > 0 {
        "EXACT_MATCH"
    } else if rows
        .iter()
        .any(|p| p[4] == address && p[5].parse::<u16>() == Ok(port))
    {
        "CANDIDATE_TYPE_OR_PROTOCOL_MISMATCH"
    } else if same_address_count > 0 {
        "PORT_CHANGED"
    } else if same_port_count > 0 {
        "ADDRESS_CHANGED"
    } else {
        "ENDPOINT_NOT_PUBLISHED"
    };
    CandidateDiagnostic {
        approved: reason == "EXACT_MATCH",
        reason,
        candidate_type: match kind {
            "host" => "host",
            "prflx" => "prflx",
            "srflx" => "srflx",
            "relay" => "relay",
            _ => "unknown",
        },
        protocol: match protocol {
            "udp" => "udp",
            "tcp" => "tcp",
            _ => "unknown",
        },
        approved_count: rows.len(),
        same_address_count,
        same_port_count,
        exact_match_count,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_host_endpoint_only_and_no_sensitive_diagnostic_fields() {
        let ip = Ipv4Addr::new(10, 31, 47, 2);
        let sdp = format!("a=candidate:f 1 udp 1 {ip} 51001 typ host\r\n");
        let check = |a: &str, p, proto, kind| diagnose(&sdp, a, p, proto, kind, ip);
        let approved = check(&ip.to_string(), 51001, "udp", "host");
        assert!(approved.approved);
        assert!(!serde_json::to_string(&approved)
            .unwrap()
            .contains(&ip.to_string()));
        assert_eq!(
            check(&ip.to_string(), 51002, "udp", "host").reason,
            "PORT_CHANGED"
        );
        assert_eq!(
            check(&ip.to_string(), 51001, "udp", "prflx").reason,
            "CANDIDATE_TYPE_NOT_APPROVED"
        );
        assert_eq!(
            check(&ip.to_string(), 51001, "tcp", "host").reason,
            "PROTOCOL_NOT_APPROVED"
        );
        assert_eq!(
            check(
                &Ipv4Addr::new(10, 31, 47, 3).to_string(),
                51001,
                "udp",
                "host"
            )
            .reason,
            "ADDRESS_OUT_OF_APPROVED_SCOPE"
        );
        assert_eq!(
            diagnose("", &ip.to_string(), 51001, "udp", "host", ip).reason,
            "NO_PUBLISHED_CANDIDATES"
        );
        let changed = sdp.replace("typ host", "typ prflx");
        assert_eq!(
            diagnose(&changed, &ip.to_string(), 51001, "udp", "host", ip).reason,
            "CANDIDATE_TYPE_OR_PROTOCOL_MISMATCH"
        );
        let changed = sdp.replace(&ip.to_string(), &Ipv4Addr::new(10, 31, 47, 3).to_string());
        assert_eq!(
            diagnose(&changed, &ip.to_string(), 51001, "udp", "host", ip).reason,
            "ADDRESS_CHANGED"
        );
        assert_eq!(
            diagnose(&changed, &ip.to_string(), 51002, "udp", "host", ip).reason,
            "ENDPOINT_NOT_PUBLISHED"
        );
    }
}
