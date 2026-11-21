use crate::TransportError;

pub(crate) const RTP_PAYLOAD_TYPE: u8 = 96;
pub(crate) const H264_FMTP: &str =
    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f";
const SDP_LIMIT: usize = 16 * 1024;

// rtc 0.21.1 generates a redundant component-2 copy of each local
// candidate. The experiment requires RTCP multiplexing and advertises only
// component 1. Never use this helper for incoming untrusted signaling.
pub(crate) fn omit_redundant_rtcp_candidate(sdp: &str) -> String {
    sdp.split_inclusive('\n')
        .filter(|line| {
            !line
                .strip_prefix("a=candidate:")
                .is_some_and(|value| value.split_ascii_whitespace().nth(1) == Some("2"))
        })
        .collect()
}

/// Validate every candidate, including ones embedded in SDP; no individual
/// candidate API is exposed. No name resolution or interface enumeration.
pub(crate) fn validate(sdp: &str, direction: &str) -> Result<u8, TransportError> {
    if sdp.len() > SDP_LIMIT
        || !sdp
            .bytes()
            .all(|b| matches!(b, b'\r' | b'\n' | b'\t' | b' '..=b'~'))
    {
        return Err(TransportError::Sdp);
    }
    let mut media = false;
    let mut saw_direction = false;
    let mut rtcp_mux = false;
    let mut rtpmap = false;
    let mut level = None;
    let mut candidates = 0;
    for line in sdp.lines() {
        if line == "a=ice-lite" || line.starts_with("a=remote-candidates:") {
            return Err(TransportError::IcePolicy);
        }
        if line != line.trim() {
            return Err(TransportError::Sdp);
        }
        if let Some(value) = line.strip_prefix("m=") {
            let parts = value.split_ascii_whitespace().collect::<Vec<_>>();
            if media
                || parts.len() != 4
                || parts[0] != "video"
                || !is_port(parts[1])
                || parts[2] != "UDP/TLS/RTP/SAVPF"
                || parts[3].parse::<u8>().ok() != Some(RTP_PAYLOAD_TYPE)
            {
                return Err(TransportError::Sdp);
            }
            media = true;
        } else if let Some(value) = line.strip_prefix("c=") {
            if !matches!(value, "IN IP4 0.0.0.0" | "IN IP4 127.0.0.1") {
                return Err(TransportError::IcePolicy);
            }
        } else if let Some(value) = line.strip_prefix("a=rtcp:") {
            let parts = value.split_ascii_whitespace().collect::<Vec<_>>();
            if !matches!(
                parts.as_slice(),
                [port] | [port, "IN", "IP4", "0.0.0.0" | "127.0.0.1"] if is_port(port)
            ) {
                return Err(TransportError::IcePolicy);
            }
        } else if let Some(value) = line.strip_prefix("a=candidate:") {
            validate_candidate(value)?;
            candidates += 1;
            if candidates > 8 {
                return Err(TransportError::IcePolicy);
            }
        } else if let Some(value) = line.strip_prefix("a=rtpmap:") {
            let parts = value.split_ascii_whitespace().collect::<Vec<_>>();
            if !media
                || rtpmap
                || !matches!(parts.as_slice(), [pt, codec]
                    if pt.parse::<u8>().ok() == Some(RTP_PAYLOAD_TYPE)
                        && codec.eq_ignore_ascii_case("H264/90000"))
            {
                return Err(TransportError::Sdp);
            }
            rtpmap = true;
        } else if let Some(value) = line.strip_prefix("a=fmtp:") {
            let (pt, params) = value.split_once(' ').ok_or(TransportError::Sdp)?;
            if !media || level.is_some() || pt.parse::<u8>().ok() != Some(RTP_PAYLOAD_TYPE) {
                return Err(TransportError::Sdp);
            }
            level = Some(profile_level(params)?);
        } else if matches!(
            line,
            "a=sendrecv" | "a=sendonly" | "a=recvonly" | "a=inactive"
        ) {
            if !media || saw_direction || line.strip_prefix("a=") != Some(direction) {
                return Err(TransportError::Sdp);
            }
            saw_direction = true;
        } else if line == "a=rtcp-mux" {
            rtcp_mux |= media;
        }
    }
    if !(media && saw_direction && rtcp_mux && rtpmap) {
        return Err(TransportError::Sdp);
    }
    if candidates == 0 {
        return Err(TransportError::NoLoopbackCandidate);
    }
    level.ok_or(TransportError::Sdp)
}

fn validate_candidate(value: &str) -> Result<(), TransportError> {
    let parts = value.split_ascii_whitespace().collect::<Vec<_>>();
    let [foundation, "1", protocol, priority, "127.0.0.1", port, "typ", "host", tail @ ..] =
        parts.as_slice()
    else {
        return Err(TransportError::IcePolicy);
    };
    if foundation.is_empty()
        || foundation.len() > 32
        || !foundation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
        || !protocol.eq_ignore_ascii_case("udp")
        || !is_port(port)
        || priority.parse::<u32>().is_err()
        || tail.len() % 2 != 0
        || tail.as_chunks::<2>().0.iter().any(|pair| match pair[0] {
            "generation" | "network-id" | "network-cost" => pair[1].parse::<u32>().is_err(),
            "ufrag" => !pair[1]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'-' | b'_')),
            _ => true,
        })
    {
        return Err(TransportError::IcePolicy);
    }
    Ok(())
}

fn profile_level(params: &str) -> Result<u8, TransportError> {
    let mut level = None;
    let mut mode = None;
    let mut asymmetry = None;
    for param in params.split(';') {
        let (key, value) = param
            .trim()
            .split_once('=')
            .ok_or(TransportError::Profile)?;
        match key {
            "profile-level-id" if level.is_none() => {
                if value.len() != 6
                    || !value.bytes().all(|b| b.is_ascii_hexdigit())
                    || !value[..2].eq_ignore_ascii_case("42")
                {
                    return Err(TransportError::Profile);
                }
                let iop =
                    u8::from_str_radix(&value[2..4], 16).map_err(|_| TransportError::Profile)?;
                let level_idc =
                    u8::from_str_radix(&value[4..6], 16).map_err(|_| TransportError::Profile)?;
                // Negotiated RFC 6184 Baseline profile. The sender offers level
                // 3.1 only; the answer may lower it, but must not raise it.
                if iop & 0x4f != 0 || !matches!(level_idc, 10..=13 | 20..=22 | 30..=31) {
                    return Err(TransportError::Profile);
                }
                level = Some(level_idc);
            }
            "packetization-mode" if mode.is_none() && value == "1" => mode = Some(()),
            "level-asymmetry-allowed" if asymmetry.is_none() && matches!(value, "0" | "1") => {
                asymmetry = Some(());
            }
            _ => return Err(TransportError::Profile),
        }
    }
    if mode.is_none() {
        return Err(TransportError::Profile);
    }
    level.ok_or(TransportError::Profile)
}

fn is_port(s: &str) -> bool {
    s.parse::<u16>().is_ok_and(|n| n != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = concat!(
        "m=video 9 UDP/TLS/RTP/SAVPF 96\r\n",
        "c=IN IP4 0.0.0.0\r\n",
        "a=rtcp:9 IN IP4 0.0.0.0\r\n",
        "a=rtcp-mux\r\n",
        "a=recvonly\r\n",
        "a=candidate:1 1 udp 1 127.0.0.1 51000 typ host generation 0 network-cost 50\r\n",
        "a=rtpmap:96 H264/90000\r\n",
        "a=fmtp:96 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42001f\r\n",
    );

    #[test]
    fn accepts_only_loopback_udp_host_and_h264_mode_one() {
        assert_eq!(validate(VALID, "recvonly").unwrap(), 31);
        for address in ["::1", "192.168.1.2", "0.0.0.0", "test.local", "127.0.0.2"] {
            assert!(matches!(
                validate(&VALID.replace("127.0.0.1", address), "recvonly"),
                Err(TransportError::IcePolicy)
            ));
        }
        for (a, b) in [
            ("typ host", "typ srflx"),
            ("udp", "tcp"),
            ("1 udp", "2 udp"),
            ("generation 0", "raddr 192.0.2.1"),
        ] {
            assert!(matches!(
                validate(&VALID.replace(a, b), "recvonly"),
                Err(TransportError::IcePolicy)
            ));
        }
        assert!(matches!(
            validate(
                &VALID.replace("packetization-mode=1", "packetization-mode=0"),
                "recvonly"
            ),
            Err(TransportError::Profile)
        ));
        assert!(validate(&VALID.replace("42001f", "42e01f"), "recvonly").is_err());
    }

    #[test]
    fn rejects_extra_routes_and_media_in_an_otherwise_valid_sdp() {
        for extra in [
            "a=candidate:1 1 udp 1 192.168.1.2 5000 typ host\r\n",
            "a=remote-candidates:1 192.168.1.2 5000\r\n",
            "m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n",
            "a=sendrecv\r\n",
            "a=rtcp:5000 IN IP4 192.168.1.2\r\n",
            " a=candidate:1 1 udp 1 192.168.1.2 5000 typ host\r\n",
        ] {
            assert!(validate(&format!("{VALID}{extra}"), "recvonly").is_err());
        }
    }
}
