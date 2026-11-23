//! The synthetic relay diagnostic publishes no ICE related/base address.
//! Keep the raw local SDP inside the ICE agent and require a canonical wire form.
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddrV4},
};

pub(crate) type Endpoints = BTreeSet<SocketAddrV4>;

pub(crate) fn redact(sdp: &str) -> Result<String, String> {
    let mut output = String::new();
    let mut count = 0;
    if sdp.len() > 65536 || sdp.contains('\0') {
        return Err("R06_RELAY_SDP_INVALID".into());
    }
    for line in sdp.lines() {
        if line.trim() != line || line.starts_with("a=remote-candidates:") {
            return Err("R06_RELAY_SDP_INVALID".into());
        }
        let public = if let Some(c) = line.strip_prefix("a=candidate:") {
            let p: Vec<_> = c.split_ascii_whitespace().collect();
            if p.get(1) == Some(&"2") {
                continue;
            }
            if p.len() < 8
                || p[0].len() > 64
                || p[1] != "1"
                || !p[2].eq_ignore_ascii_case("udp")
                || p[3].parse::<u32>().is_err()
                || p[4].parse::<Ipv4Addr>().is_err()
                || !p[5].parse::<u16>().is_ok_and(|v| v > 0)
                || p[6] != "typ"
                || p[7] != "relay"
            {
                return Err("R06_RELAY_CANDIDATE_INVALID".into());
            }
            count += 1;
            if count > 32 {
                return Err("R06_RELAY_CANDIDATE_INVALID".into());
            }
            format!(
                "a=candidate:{} 1 udp {} {} {} typ relay raddr 0.0.0.0 rport 0",
                p[0], p[3], p[4], p[5]
            )
        } else if let Some(origin) = line.strip_prefix("o=") {
            let p: Vec<_> = origin.split_ascii_whitespace().collect();
            if p.len() != 6 || p[1].parse::<u64>().is_err() || p[2].parse::<u64>().is_err() {
                return Err("R06_RELAY_SDP_INVALID".into());
            }
            format!("o=- {} {} IN IP4 0.0.0.0", p[1], p[2])
        } else if line.starts_with("c=") {
            "c=IN IP4 0.0.0.0".into()
        } else if line.starts_with("a=rtcp:") {
            "a=rtcp:9 IN IP4 0.0.0.0".into()
        } else {
            line.to_owned()
        };
        output.push_str(&public);
        output.push_str("\r\n");
    }
    if count == 0 {
        return Err("R06_RELAY_CANDIDATE_INVALID".into());
    }
    Ok(output)
}

pub(crate) fn endpoints(sdp: &str) -> Result<Endpoints, String> {
    // Compare lines (not platform newline conventions), before passing SDP to ICE.
    let sanitized = redact(sdp)?;
    if !sdp.lines().eq(sanitized.lines()) {
        return Err("R06_RELAY_SDP_NOT_REDACTED".into());
    }
    sdp.lines()
        .filter_map(|line| line.strip_prefix("a=candidate:"))
        .map(|candidate| {
            let mut p = candidate.split_ascii_whitespace().skip(4);
            let ip = p
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("R06_RELAY_CANDIDATE_INVALID")?;
            let port = p
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or("R06_RELAY_CANDIDATE_INVALID")?;
            Ok(SocketAddrV4::new(ip, port))
        })
        .collect()
}

pub(crate) fn matches(set: &Endpoints, c: &webrtc::peer_connection::RTCIceCandidate) -> bool {
    c.component == 1
        && c.typ.to_string() == "relay"
        && c.protocol.to_string() == "udp"
        && c.address
            .parse()
            .is_ok_and(|ip| set.contains(&SocketAddrV4::new(ip, c.port)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use webrtc::peer_connection::{RTCIceCandidate, RTCIceCandidateType, RTCIceProtocol};

    #[test]
    fn redaction_and_exact_selection_do_not_trust_related_endpoints_or_other_ports() {
        let raw="o=host 12 2 IN IP4 192.168.1.5\r\nc=IN IP4 192.168.1.5\r\na=rtcp:12345 IN IP4 192.168.1.5\r\na=candidate:f 1 UDP 123 192.0.2.1 49160 typ relay raddr 192.168.1.5 rport 55000 generation 1\r\na=candidate:z 2 UDP 123 192.0.2.1 49161 typ relay\r\n";
        assert!(endpoints(raw).is_err());
        let safe = redact(raw).unwrap();
        assert!(!safe.contains("192.168.1.5"));
        assert!(!safe.contains("55000"));
        assert!(!safe.contains("generation"));
        assert!(safe.contains("raddr 0.0.0.0 rport 0"));
        let candidate = safe
            .lines()
            .find_map(|line| line.strip_prefix("a=candidate:"))
            .unwrap();
        assert!(rtc::ice::candidate::unmarshal_candidate(candidate).is_ok());
        let endpoints = endpoints(&safe).unwrap();
        let mut selected = RTCIceCandidate {
            address: "192.0.2.1".into(),
            port: 49160,
            component: 1,
            typ: RTCIceCandidateType::Relay,
            protocol: RTCIceProtocol::Udp,
            ..Default::default()
        };
        assert!(matches(&endpoints, &selected));
        selected.port = 49161;
        assert!(!matches(&endpoints, &selected));
        selected.port = 49160;
        selected.typ = RTCIceCandidateType::Host;
        assert!(!matches(&endpoints, &selected));
        assert!(endpoints.len() == 1);
        assert!(redact(&(safe.clone() + "a=remote-candidates:1 192.0.2.1 49160\r\n")).is_err());
        assert!(redact(&safe.replace("a=candidate:", " a=candidate:")).is_err());
    }
}
