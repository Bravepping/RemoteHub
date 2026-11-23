//! No-I/O candidate policy for the signed, video-only WSS fixture.

use crate::{
    direct_policy::{Candidate, Kind},
    ice_policy::{public_v4, IceMode},
    public_video::PublicVideoScope,
    signaling::validate_public_video_description,
};
use hub_identity::session::MAX_SIGNAL_BYTES;
use std::net::IpAddr;

const POLICY: &str = "PUBLIC_VIDEO_SDP_POLICY";
const CANDIDATE_LIMIT: usize = 16;

pub(crate) fn candidate_allowed(
    scope: PublicVideoScope,
    ip: IpAddr,
    port: u16,
    kind: &str,
) -> bool {
    let IpAddr::V4(ip) = ip else { return false };
    match (scope.mode, kind) {
        (IceMode::DirectPreferred | IceMode::RelayOnly, "relay") => {
            ip == *scope.turn.ip() && (scope.relay_ports.0..=scope.relay_ports.1).contains(&port)
        }
        (IceMode::DirectPreferred, "host" | "srflx") => public_v4(ip) && port >= 1024,
        _ => false,
    }
}

pub(crate) fn validate(
    sdp: &str,
    scope: PublicVideoScope,
    answer: bool,
    now: u64,
) -> Result<Vec<Candidate>, &'static str> {
    scope.checked_mode(now)?;
    if sdp.len() > MAX_SIGNAL_BYTES || !sdp.is_ascii() || !canonical_lines(sdp) {
        return Err(POLICY);
    }
    let mut media: Option<Vec<&str>> = None;
    let mut rtpmap = [false; 128];
    let mut fmtp = [false; 128];
    let mut candidates = Vec::new();
    let mut fingerprint = false;
    let mut direction = false;
    let mut mux = false;
    let mut origin = false;
    let mut connection = [false; 2];
    let mut rtcp = false;
    for line in sdp.lines().skip(1) {
        if line.trim() != line
            || line.len() > 2048
            || line.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(POLICY);
        }
        if let Some(value) = line.strip_prefix("m=") {
            let fields: Vec<_> = value.split(' ').collect();
            if media.is_some()
                || fields.len() < 4
                || fields[0] != "video"
                || fields[1] != "9"
                || fields[2] != "UDP/TLS/RTP/SAVPF"
                || fields[3..]
                    .iter()
                    .any(|pt| !decimal::<u8>(pt) || pt.parse::<u8>().unwrap() > 127)
            {
                return Err(POLICY);
            }
            media = Some(fields);
        } else if let Some(value) = line.strip_prefix("a=rtpmap:") {
            track_payload_declaration(value, &media.as_ref().ok_or(POLICY)?[3..], &mut rtpmap)?;
        } else if let Some(value) = line.strip_prefix("a=fmtp:") {
            track_payload_declaration(value, &media.as_ref().ok_or(POLICY)?[3..], &mut fmtp)?;
        } else if let Some(value) = line.strip_prefix("a=candidate:") {
            if media.is_none() || candidates.len() == CANDIDATE_LIMIT {
                return Err(POLICY);
            }
            let candidate = parse_candidate(value, scope)?;
            if candidates.contains(&candidate) {
                return Err(POLICY);
            }
            candidates.push(candidate);
        } else if let Some(value) = line.strip_prefix("a=fingerprint:") {
            let Some(hash) = value.strip_prefix("sha-256 ") else {
                return Err(POLICY);
            };
            if fingerprint || !valid_fingerprint(hash) {
                return Err(POLICY);
            }
            fingerprint = true;
        } else if matches!(
            line,
            "a=recvonly" | "a=sendonly" | "a=inactive" | "a=sendrecv"
        ) {
            if media.is_none()
                || direction
                || line != if answer { "a=sendonly" } else { "a=recvonly" }
            {
                return Err(POLICY);
            }
            direction = true;
        } else if line == "a=rtcp-mux" {
            if media.is_none() || mux {
                return Err(POLICY);
            }
            mux = true;
        } else if line.starts_with("a=rtcp:") {
            if media.is_none() || rtcp || line != "a=rtcp:9 IN IP4 0.0.0.0" {
                return Err(POLICY);
            }
            rtcp = true;
        } else if line.starts_with("c=") {
            let section = usize::from(media.is_some());
            if connection[section] || line != "c=IN IP4 0.0.0.0" {
                return Err(POLICY);
            }
            connection[section] = true;
        } else if let Some(value) = line.strip_prefix("o=") {
            let fields: Vec<_> = value.split(' ').collect();
            if media.is_some()
                || origin
                || fields.len() != 6
                || fields[0] != "-"
                || !decimal::<u64>(fields[1])
                || !decimal::<u64>(fields[2])
                || fields[3..] != ["IN", "IP4", "0.0.0.0"]
            {
                return Err(POLICY);
            }
            origin = true;
        } else if !non_routing_line(line, answer) {
            // Reject other media or destination-bearing extensions instead
            // of silently accepting them for a downstream SDP parser.
            return Err(POLICY);
        }
    }
    if !fingerprint || !direction || !mux || candidates.is_empty() {
        return Err(POLICY);
    }
    let negotiation = validate_public_video_description(sdp, scope, answer).map_err(|_| POLICY)?;
    if !media.ok_or(POLICY)?[3..].contains(&negotiation.payload_type.to_string().as_str()) {
        return Err(POLICY);
    }
    Ok(candidates)
}

fn canonical_lines(sdp: &str) -> bool {
    if sdp.starts_with("v=0\r\n") {
        sdp.split_inclusive('\n').all(|line| line.ends_with("\r\n"))
    } else if sdp.starts_with("v=0\n") {
        sdp.ends_with('\n') && !sdp.contains('\r')
    } else {
        false
    }
}

fn track_payload_declaration(
    value: &str,
    media: &[&str],
    declared: &mut [bool; 128],
) -> Result<(), &'static str> {
    let (field, body) = value.split_once(' ').ok_or(POLICY)?;
    let pt = field.parse::<usize>().map_err(|_| POLICY)?;
    if body.is_empty()
        || body.starts_with(' ')
        || !media.contains(&field)
        || pt >= declared.len()
        || std::mem::replace(&mut declared[pt], true)
    {
        return Err(POLICY);
    }
    Ok(())
}

fn parse_candidate(value: &str, scope: PublicVideoScope) -> Result<Candidate, &'static str> {
    let candidate = Candidate::parse(value).map_err(|_| POLICY)?;
    let fields: Vec<_> = value.split(' ').collect();
    let kind = match candidate.kind {
        Kind::Host => "host",
        Kind::Srflx => "srflx",
        Kind::Relay => "relay",
    };
    if fields.len() != if candidate.kind == Kind::Host { 8 } else { 12 }
        || fields[0].len() > 32
        || !fields[0]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
        || fields[1] != "1"
        || fields[2] != "udp"
        || !decimal::<u32>(fields[3])
        || fields[4] != candidate.address.ip().to_string()
        || fields[5] != candidate.address.port().to_string()
        || fields[7] != kind
        || (candidate.kind != Kind::Host && fields[8..] != ["raddr", "0.0.0.0", "rport", "0"])
        || !candidate_allowed(
            scope,
            (*candidate.address.ip()).into(),
            candidate.address.port(),
            kind,
        )
    {
        return Err(POLICY);
    }
    Ok(candidate)
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 95
        && value
            .split(':')
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn decimal<T: std::str::FromStr + ToString>(value: &str) -> bool {
    value
        .parse::<T>()
        .is_ok_and(|number| number.to_string() == value)
}

fn non_routing_line(line: &str, answer: bool) -> bool {
    matches!(
        line,
        "s=-"
            | "t=0 0"
            | "a=rtcp-rsize"
            | "a=end-of-candidates"
            | "a=ice-options:trickle"
            | "a=extmap-allow-mixed"
    ) || if answer {
        matches!(line, "a=setup:active" | "a=setup:passive")
    } else {
        line == "a=setup:actpass"
    } || [
        "a=mid:",
        "a=group:BUNDLE ",
        "a=ice-ufrag:",
        "a=ice-pwd:",
        "a=rtcp-fb:",
        "a=extmap:",
        "a=msid:",
        "a=msid-semantic:",
        "a=ssrc:",
        "a=ssrc-group:",
    ]
    .into_iter()
    .any(|prefix| {
        line.strip_prefix(prefix)
            .is_some_and(|value| !value.is_empty())
    }) || ["b=AS:", "b=TIAS:"]
        .into_iter()
        .any(|prefix| line.strip_prefix(prefix).is_some_and(decimal::<u32>))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(mode: IceMode) -> PublicVideoScope {
        PublicVideoScope {
            wss: "8.8.8.8:8443".parse().unwrap(),
            ca_sha256: [7; 32],
            turn: "8.8.8.8:3478".parse().unwrap(),
            relay_ports: (49152, 49300),
            mode,
            expires_at: 1100,
        }
    }

    fn sdp(candidate: &str) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\na=rtcp:9 IN IP4 0.0.0.0\r\na=rtcp-mux\r\na=recvonly\r\na=fingerprint:sha-256 {}\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=candidate:{candidate}\r\n",
            ["AA"; 32].join(":")
        )
    }

    #[test]
    fn public_modes_require_exact_routed_and_redacted_candidates() {
        let direct = scope(IceMode::DirectPreferred);
        let relay = scope(IceMode::RelayOnly);
        let offer = sdp("h 1 udp 1 1.1.1.1 50002 typ host");
        let answer = sdp("r 1 udp 2 8.8.8.8 49152 typ relay raddr 0.0.0.0 rport 0")
            .replace("a=recvonly", "a=sendonly");
        assert_eq!(validate(&offer, direct, false, 1000).unwrap().len(), 1);
        assert!(validate(&offer, relay, false, 1000).is_err());
        assert!(validate(&answer, relay, true, 1000).is_ok());
        assert!(validate(&answer, direct, true, 1000).is_ok());
        assert!(validate(&answer.replace("49152", "49151"), relay, true, 1000).is_err());
        assert!(validate(&answer.replace("8.8.8.8", "1.1.1.1"), relay, true, 1000).is_err());
        assert!(validate(
            &answer.replace("raddr 0.0.0.0", "raddr 192.168.1.4"),
            direct,
            true,
            1000
        )
        .is_err());
        assert!(matches!(
            validate(&offer, direct, false, 1100),
            Err("PUBLIC_VIDEO_SCOPE_EXPIRED")
        ));
    }

    #[test]
    fn public_video_rejects_alternate_media_routes_and_ambiguous_dtls() {
        let good = sdp("s 1 udp 1 1.1.1.1 50002 typ srflx raddr 0.0.0.0 rport 0");
        let scope = scope(IceMode::DirectPreferred);
        assert!(validate(&good, scope, false, 1000).is_ok());
        assert!(validate(&good.replace("\r\n", "\n"), scope, false, 1000).is_ok());
        for bad in [
            good.replace("1.1.1.1", "192.168.1.4"),
            good.replace("a=candidate:s 1", "a=candidate:s 2"),
            good.replace("a=rtcp:9 IN IP4 0.0.0.0", "a=rtcp:90 IN IP4 1.1.1.1"),
            good.replace("a=rtcp-mux", "a=sctp-port:5000"),
            good.clone() + "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n",
            good.clone() + "a=source-filter: incl IN IP4 1.1.1.1 1.1.1.2\r\n",
            format!("{good}a=fingerprint:sha-256 {}\r\n", ["BB"; 32].join(":")),
            good.replace("a=recvonly", "a=sendrecv"),
            good.clone() + "a=rtpmap:102 VP8/90000\r\n",
            good.clone() + "a=fmtp:102 packetization-mode=0\r\n",
            good.clone() + "a=rtpmap:123 H264/90000\r\n",
            good.replacen("a=recvonly\r\n", "a=recvonly\n", 1),
        ] {
            assert!(matches!(validate(&bad, scope, false, 1000), Err(POLICY)));
        }
    }
}
