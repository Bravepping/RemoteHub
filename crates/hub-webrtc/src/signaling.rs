use serde::Serialize;
use std::net::IpAddr;

#[derive(Clone, Default, Serialize)]
pub struct Negotiation {
    pub payload_type: u8,
    pub fmtp: String,
    pub candidate_types: Vec<String>,
}

/// Only a single recvonly video m-line is accepted. Remote host addresses never reach ICE.
pub fn validate_offer(sdp: &str) -> Result<Negotiation, String> {
    validate_description(sdp, crate::scope::NetworkScope::Loopback, false)
}

pub fn validate_description(
    sdp: &str,
    scope: crate::scope::NetworkScope,
    answer: bool,
) -> Result<Negotiation, String> {
    validate_policy(sdp, DescriptionScope::Existing(scope), answer, false)
}
pub fn validate_control_description(
    sdp: &str,
    scope: crate::scope::NetworkScope,
    answer: bool,
) -> Result<Negotiation, String> {
    validate_policy(sdp, DescriptionScope::Existing(scope), answer, true)
}
enum DescriptionScope {
    Existing(crate::scope::NetworkScope),
    #[cfg(feature = "p006-turn-probe")]
    Relay(crate::ice_policy::RelayTarget),
}
impl DescriptionScope {
    fn requires_fingerprint(&self) -> bool {
        match self {
            Self::Existing(scope) => scope.is_lan(),
            #[cfg(feature = "p006-turn-probe")]
            Self::Relay(_) => true,
        }
    }
    fn permits_peer(&self, ip: IpAddr) -> bool {
        match self {
            Self::Existing(scope) => scope.permits_peer(ip),
            #[cfg(feature = "p006-turn-probe")]
            Self::Relay(target) => ip == IpAddr::V4(target.relay),
        }
    }
    fn candidate(&self, ip: IpAddr, port: u16, kind: &str) -> bool {
        match self {
            Self::Existing(scope) => kind == "host" && scope.permits_peer(ip) && port > 0,
            #[cfg(feature = "p006-turn-probe")]
            Self::Relay(target) => {
                kind == "relay"
                    && match ip {
                        IpAddr::V4(ip) => {
                            ip == target.relay
                                && (target.port_min..=target.port_max).contains(&port)
                        }
                        _ => false,
                    }
            }
        }
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Existing(scope) => {
                if scope.is_lan() {
                    "host/pinned-lan/udp"
                } else {
                    "host/loopback/udp"
                }
            }
            #[cfg(feature = "p006-turn-probe")]
            Self::Relay(_) => "relay/approved-range/udp",
        }
    }
}
#[cfg(feature = "p006-turn-probe")]
pub fn validate_relay_description(
    sdp: &str,
    grant: &crate::ice_policy::RelayGrant,
    now: u64,
    answer: bool,
) -> Result<Negotiation, String> {
    if !grant.is_current(now) {
        return Err("RELAY_APPROVAL_EXPIRED".into());
    }
    if sdp.len() > 64 * 1024 {
        return Err("SDP_SIZE_OR_ENCODING".into());
    }
    let mut count = 0;
    for line in sdp.lines() {
        if line.starts_with("m=") {
            count = 0;
        }
        if line.starts_with("a=candidate:") {
            count += 1;
            if count > 16 || line.split_whitespace().count() > 32 {
                return Err("RELAY_CANDIDATE_LIMIT".into());
            }
        }
    }
    validate_policy(sdp, DescriptionScope::Relay(grant.target()), answer, true)
}
fn validate_policy(
    sdp: &str,
    scope: DescriptionScope,
    answer: bool,
    control: bool,
) -> Result<Negotiation, String> {
    if sdp.len() > 64 * 1024 || sdp.contains('\0') {
        return Err("SDP_SIZE_OR_ENCODING".into());
    }
    let lines: Vec<_> = sdp.lines().map(str::trim).collect();
    let media: Vec<_> = lines.iter().filter(|line| line.starts_with("m=")).collect();
    if media.len() != if control { 2 } else { 1 }
        || !media[0].starts_with("m=video ")
        || !media[0].contains(" UDP/TLS/RTP/SAVPF ")
        || !lines.contains(&if answer { "a=sendonly" } else { "a=recvonly" })
    {
        return Err("SINGLE_RECVONLY_VIDEO_REQUIRED".into());
    }
    if control {
        if !scope.requires_fingerprint() {
            return Err("CONTROL_LAN_SCOPE_REQUIRED".into());
        }
        let app = sdp
            .split("m=application ")
            .nth(1)
            .ok_or("CONTROL_APPLICATION_REQUIRED")?;
        let app_lines: Vec<_> = app.lines().map(str::trim).collect();
        let m: Vec<_> = media[1].split_whitespace().collect();
        if m.len() != 4
            || m[0] != "m=application"
            || m[1] == "0"
            || !m[1].parse::<u16>().is_ok_and(|p| p > 0)
            || m[2] != "UDP/DTLS/SCTP"
            || m[3] != "webrtc-datachannel"
            || !lines.contains(&"a=rtcp-mux")
        {
            return Err("CONTROL_SCTP_MEDIA".into());
        }
        let mids: Vec<_> = lines
            .iter()
            .filter_map(|s| s.strip_prefix("a=mid:"))
            .collect();
        let bundle: Vec<_> = lines
            .iter()
            .filter_map(|s| s.strip_prefix("a=group:BUNDLE "))
            .collect();
        if mids.len() != 2
            || mids[0] == mids[1]
            || mids.iter().any(|m| m.is_empty() || m.len() > 32)
            || bundle.len() != 1
            || bundle[0].split_whitespace().collect::<Vec<_>>() != mids
        {
            return Err("CONTROL_SINGLE_BUNDLE_REQUIRED".into());
        }
        let ports: Vec<_> = app_lines
            .iter()
            .filter_map(|s| s.strip_prefix("a=sctp-port:"))
            .collect();
        let sizes: Vec<_> = app_lines
            .iter()
            .filter_map(|s| s.strip_prefix("a=max-message-size:"))
            .collect();
        if ports.len() != 1
            || !ports[0].parse::<u16>().is_ok_and(|p| p > 0)
            || sizes.len() != 1
            || !sizes[0]
                .parse::<u32>()
                .is_ok_and(|s| (1..=16 * 1024 * 1024).contains(&s))
            || lines.iter().any(|s| s.starts_with("a=sctpmap:"))
        {
            return Err("CONTROL_SCTP_PARAMETERS".into());
        }
        let session = sdp.split("m=").next().unwrap_or_default();
        for section in sdp.split("m=").skip(1) {
            if section
                .lines()
                .filter(|s| s.trim().starts_with("a=mid:"))
                .count()
                != 1
            {
                return Err("CONTROL_SINGLE_BUNDLE_REQUIRED".into());
            }
            if !session
                .lines()
                .chain(section.lines())
                .any(|s| s.trim().starts_with("a=fingerprint:sha-256 "))
            {
                return Err("SHA256_DTLS_FINGERPRINT_REQUIRED".into());
            }
        }
    }
    let mut candidates = Vec::new();
    let mut application = false;
    for line in &lines {
        if line.starts_with("m=") {
            application = line.starts_with("m=application ");
        }
        if line.starts_with("a=remote-candidates:")
            || (!answer && line.starts_with("a=ssrc:"))
            || (!application && line.starts_with("a=sendrecv"))
            || (!application && !answer && line.starts_with("a=sendonly"))
            || (!application && answer && line.starts_with("a=recvonly"))
        {
            return Err("UNEXPECTED_REMOTE_MEDIA_OR_CANDIDATE".into());
        }
        if let Some(address) = line.strip_prefix("c=IN IP4 ") {
            if address != "0.0.0.0"
                && !address
                    .parse::<IpAddr>()
                    .is_ok_and(|ip| scope.permits_peer(ip))
            {
                return Err("OFF_HOST_CONNECTION_ADDRESS".into());
            }
        }
        if line.starts_with("c=") && !line.starts_with("c=IN IP4 ") {
            return Err("IPV4_LOOPBACK_ONLY".into());
        }
        if let Some(candidate) = line.strip_prefix("a=candidate:") {
            let fields: Vec<_> = candidate.split_whitespace().collect();
            if fields.len() < 8
                || !fields[2].eq_ignore_ascii_case("udp")
                || fields[6] != "typ"
                || !(fields[1] == "1"
                    || (answer && fields[1] == "2" && lines.contains(&"a=rtcp-mux")))
                || !fields[4]
                    .parse::<IpAddr>()
                    .ok()
                    .zip(fields[5].parse::<u16>().ok())
                    .is_some_and(|(ip, port)| scope.candidate(ip, port, fields[7]))
            {
                return Err("OFF_HOST_ICE_REJECTED".into());
            }
            candidates.push(scope.label().into());
        }
    }
    if scope.requires_fingerprint() {
        let fingerprints: Vec<_> = lines
            .iter()
            .filter_map(|line| line.strip_prefix("a=fingerprint:sha-256 "))
            .collect();
        if fingerprints.is_empty()
            || fingerprints.iter().any(|hash| {
                let parts: Vec<_> = hash.split(':').collect();
                parts.len() != 32
                    || parts
                        .iter()
                        .any(|p| p.len() != 2 || u8::from_str_radix(p, 16).is_err())
            })
        {
            return Err("SHA256_DTLS_FINGERPRINT_REQUIRED".into());
        }
    }
    let video_lines: Vec<_> = lines
        .iter()
        .skip_while(|line| !line.starts_with("m=video "))
        .take_while(|line| !line.starts_with("m=application "))
        .copied()
        .collect();
    for line in &video_lines {
        if let Some(value) = line.strip_prefix("a=rtpmap:") {
            if let Some((pt, codec)) = value.split_once(' ') {
                if codec.eq_ignore_ascii_case("H264/90000") {
                    let fmtp = video_lines
                        .iter()
                        .find_map(|l| l.strip_prefix(&format!("a=fmtp:{pt} ")))
                        .unwrap_or_default();
                    let params: Vec<_> = fmtp.split(';').map(str::trim).collect();
                    if params.contains(&"packetization-mode=1")
                        && params
                            .iter()
                            .any(|p| p.eq_ignore_ascii_case("profile-level-id=42001f"))
                    {
                        return Ok(Negotiation {
                            payload_type: pt.parse::<u8>().map_err(|_| "INVALID_PAYLOAD_TYPE")?,
                            fmtp: fmtp.into(),
                            candidate_types: candidates,
                        });
                    }
                }
            }
        }
    }
    Err("H264_BASELINE_42001F_MODE1_REQUIRED".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    const OFFER: &str = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=recvonly\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\n";
    #[cfg(feature = "p006-turn-probe")]
    #[test]
    fn relay_sdp_is_separate_from_lan_and_rejects_scope_expiry_and_tamper() {
        let (grant, _) = crate::ice_policy::tests::fixture();
        let good=format!("v=0\r\na=group:BUNDLE 0 1\r\na=fingerprint:sha-256 {}\r\n{}a=mid:0\r\na=rtcp-mux\r\na=candidate:r 1 udp 1 8.8.8.8 49160 typ relay\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:1\r\na=sctp-port:5000\r\na=max-message-size:16384\r\n",["AB";32].join(":"),OFFER.strip_prefix("v=0\r\n").unwrap());
        assert!(validate_relay_description(&good, &grant, 1000, false).is_ok());
        assert!(validate_offer(&good).is_err());
        assert!(validate_control_description(
            &good,
            crate::scope::NetworkScope::Lan(crate::scope::LanScope {
                local: "10.0.0.2".parse().unwrap(),
                peer: "10.0.0.3".parse().unwrap()
            }),
            false
        )
        .is_err());
        for bad in [
            good.replace("49160 typ relay", "49260 typ relay"),
            good.replace("8.8.8.8 49160", "8.8.4.4 49160"),
            good.replace("typ relay", "typ host"),
            good.replace("AB:AB", "AB:ZZ"),
            good.clone() + "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
            good.clone() + &"a=candidate:r 1 udp 1 8.8.8.8 49160 typ relay\r\n".repeat(17),
        ] {
            assert!(validate_relay_description(&bad, &grant, 1000, false).is_err());
        }
        assert!(validate_relay_description(&good, &grant, 1600, false).is_err());
        assert!(validate_relay_description(&"x".repeat(65537), &grant, 1000, false).is_err());
    }
    #[test]
    fn control_policy_is_explicit_and_rejects_extra_media_missing_bundle_and_bad_sctp() {
        use crate::scope::{LanScope, NetworkScope};
        let scope = NetworkScope::Lan(
            LanScope::new("10.0.0.2".parse().unwrap(), "10.0.0.3".parse().unwrap()).unwrap(),
        );
        let session = format!(
            "v=0\r\na=group:BUNDLE 0 1\r\na=fingerprint:sha-256 {}\r\n",
            ["AB"; 32].join(":")
        );
        let video = OFFER.strip_prefix("v=0\r\n").unwrap();
        let good=format!("{session}{video}a=mid:0\r\na=rtcp-mux\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:1\r\na=sctp-port:5000\r\na=max-message-size:262144\r\n");
        assert!(validate_control_description(&good, scope, false).is_ok());
        assert!(validate_offer(&good).is_err());
        assert!(validate_description(&good, scope, false).is_err());
        assert!(validate_control_description(&good, NetworkScope::Loopback, false).is_err());
        for bad in [
            good.replace("BUNDLE 0 1", "BUNDLE 0"),
            good.replace("a=mid:1", "a=mid:0"),
            good.replace("a=sctp-port:5000", "a=sctp-port:0"),
            good.replace("a=max-message-size:262144", "a=max-message-size:0"),
            good.replace("UDP/DTLS/SCTP", "TCP/DTLS/SCTP"),
            good.clone() + "m=audio 9 UDP/TLS/RTP/SAVPF 111\r\n",
            good.clone() + "a=candidate:1 1 UDP 1 10.0.0.4 5000 typ host\r\n",
        ] {
            assert!(validate_control_description(&bad, scope, false).is_err());
        }
    }
    #[test]
    fn lan_sdp_requires_authenticated_dtls_identity_and_exact_peer() {
        use crate::scope::{LanScope, NetworkScope};
        let scope = NetworkScope::Lan(
            LanScope::new(
                "192.168.10.2".parse().unwrap(),
                "192.168.10.3".parse().unwrap(),
            )
            .unwrap(),
        );
        assert!(validate_description(OFFER, scope, false).is_err());
        let offer=format!("{OFFER}a=fingerprint:sha-256 {}\r\na=candidate:1 1 UDP 1 192.168.10.3 12345 typ host\r\n",["AA";32].join(":"));
        assert!(validate_description(&offer, scope, false).is_ok());
        assert!(
            validate_description(&offer.replace("192.168.10.3", "192.168.10.4"), scope, false)
                .is_err()
        );
        assert!(
            validate_description(&offer.replace("typ host", "typ relay"), scope, false).is_err()
        );
        assert!(validate_description(&offer.replace("AA:AA", "AA:ZZ"), scope, false).is_err());
        assert!(validate_description(&offer.replace("recvonly", "sendonly"), scope, true).is_ok());
        let answer = offer.replace("recvonly", "sendonly")
            + "a=rtcp-mux\r\na=candidate:2 2 UDP 1 192.168.10.3 12346 typ host\r\n";
        assert!(validate_description(&answer, scope, true).is_ok());
        assert!(validate_description(&answer.replace("a=rtcp-mux\r\n", ""), scope, true).is_err());
        assert!(validate_description(
            &answer.replace("192.168.10.3 12346", "192.168.10.4 12346"),
            scope,
            true
        )
        .is_err());
        assert!(
            validate_description(&answer.replace("sendonly", "recvonly"), scope, false).is_err()
        );
        assert!(validate_description(
            &(offer + "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n"),
            scope,
            false
        )
        .is_err());
    }
    #[test]
    fn signaling_rejects_external_candidates_data_and_wrong_codec() {
        assert_eq!(validate_offer(OFFER).unwrap().payload_type, 102);
        assert!(validate_offer(
            &(OFFER.to_owned() + "a=candidate:1 1 UDP 1 127.0.0.1 12345 typ host\r\n")
        )
        .is_ok());
        for candidate in [
            "1 1 UDP 1 192.0.2.1 9 typ host",
            "1 1 UDP 1 127.0.0.1 9 typ relay",
            "1 1 TCP 1 127.0.0.1 9 typ host",
            "1 1 UDP 1 private.local 9 typ host",
        ] {
            assert!(
                validate_offer(&(OFFER.to_owned() + "a=candidate:" + candidate + "\r\n")).is_err()
            );
        }
        assert!(validate_offer(
            &(OFFER.to_owned() + "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n")
        )
        .is_err());
        assert!(validate_offer(&OFFER.replace("42001f", "42e01f")).is_err());
    }
}
