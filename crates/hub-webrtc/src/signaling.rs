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
    if sdp.len() > 64 * 1024 || sdp.contains('\0') {
        return Err("SDP_SIZE_OR_ENCODING".into());
    }
    let lines: Vec<_> = sdp.lines().map(str::trim).collect();
    let media: Vec<_> = lines.iter().filter(|line| line.starts_with("m=")).collect();
    if media.len() != 1
        || !media[0].starts_with("m=video ")
        || !media[0].contains(" UDP/TLS/RTP/SAVPF ")
        || !lines.contains(&"a=recvonly")
    {
        return Err("SINGLE_RECVONLY_VIDEO_REQUIRED".into());
    }
    let mut candidates = Vec::new();
    for line in &lines {
        if line.starts_with("a=remote-candidates:")
            || line.starts_with("a=ssrc:")
            || line.starts_with("a=sendrecv")
            || line.starts_with("a=sendonly")
        {
            return Err("UNEXPECTED_REMOTE_MEDIA_OR_CANDIDATE".into());
        }
        if let Some(address) = line.strip_prefix("c=IN IP4 ") {
            if address != "0.0.0.0" && address != "127.0.0.1" {
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
                || fields[7] != "host"
                || fields[1] != "1"
                || !fields[4].parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
                || !fields[5].parse::<u16>().is_ok_and(|port| port > 0)
            {
                return Err("OFF_HOST_ICE_REJECTED".into());
            }
            candidates.push("host/loopback/udp".into());
        }
    }
    for line in &lines {
        if let Some(value) = line.strip_prefix("a=rtpmap:") {
            if let Some((pt, codec)) = value.split_once(' ') {
                if codec.eq_ignore_ascii_case("H264/90000") {
                    let fmtp = lines
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
