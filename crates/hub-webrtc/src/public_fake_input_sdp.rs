//! Offline, native SCTP-only SDP gate for the synthetic-marker diagnostic.
//! Does not negotiate, authorize input, or open a socket.
use crate::ice_policy::public_v4;
use std::net::{Ipv4Addr, SocketAddrV4};

const POLICY: &str = "PUBLIC_FAKE_INPUT_SDP_POLICY";
const MAX_SDP: usize = 4096;
const MAX_LINE: usize = 512;
const MEDIA: &str = "m=application 9 UDP/DTLS/SCTP webrtc-datachannel";
const MSID: u16 = 1 << 0;
const EXTMAP: u16 = 1 << 1;
const DIRECTION: u16 = 1 << 2;
const SCTP_PORT: u16 = 1 << 3;
const SETUP: u16 = 1 << 4;
const UFRAG: u16 = 1 << 5;
const PASSWORD: u16 = 1 << 6;
const GATHERED: u16 = 1 << 7;
const MESSAGE_SIZE: u16 = 1 << 8;
const REQUIRED: u16 = SCTP_PORT | SETUP | UFRAG | PASSWORD;

/// Validate exact signed bytes. The caller must use the returned tuple, not
/// just the allowed TURN range, when checking the live selected candidate.
pub(crate) fn validate(
    sdp: &str,
    relay: Ipv4Addr,
    port_min: u16,
    port_max: u16,
    answer: bool,
) -> Result<SocketAddrV4, &'static str> {
    if !public_v4(relay)
        || port_min < 49152
        || port_max < port_min
        || port_max - port_min > 255
        || !sdp.is_ascii()
        || sdp.len() > MAX_SDP
    {
        return Err(POLICY);
    }
    // Signed bodies are complete native descriptions, never trickled. Keep
    // the same CRLF form the native serializer and independent parser see.
    let mut lines = sdp.strip_suffix("\r\n").ok_or(POLICY)?.split("\r\n");
    if lines.next() != Some("v=0")
        || !lines.next().is_some_and(valid_origin)
        || lines.next() != Some("s=-")
        || lines.next() != Some("t=0 0")
    {
        return Err(POLICY);
    }

    let mut media = false;
    let mut connection = false;
    let mut bundle = None;
    let mut mid = None;
    let mut candidate = None;
    let mut fingerprint = None;
    let mut seen = 0;
    for line in lines {
        if line.len() > MAX_LINE
            || line.trim() != line
            || line.bytes().any(|b| b.is_ascii_control())
        {
            return Err(POLICY);
        }
        if !media && line == MEDIA {
            media = true;
            continue;
        }
        if media && !connection {
            if line != "c=IN IP4 0.0.0.0" {
                return Err(POLICY);
            }
            connection = true;
            continue;
        }
        if let Some(value) = line.strip_prefix("a=fingerprint:sha-256 ") {
            if !valid_fingerprint(value) || fingerprint.replace(value).is_some() {
                return Err(POLICY);
            }
            continue;
        }
        if !media {
            let flag = match line {
                "a=msid-semantic:WMS *" => MSID,
                "a=extmap-allow-mixed" => EXTMAP,
                _ => {
                    let id = line.strip_prefix("a=group:BUNDLE ").ok_or(POLICY)?;
                    if !valid_mid(id) || bundle.replace(id).is_some() {
                        return Err(POLICY);
                    }
                    continue;
                }
            };
            mark(&mut seen, flag)?;
            continue;
        }
        let flag = match line {
            "a=sendrecv" => DIRECTION,
            "a=sctp-port:5000" => SCTP_PORT,
            "a=setup:actpass" if !answer => SETUP,
            "a=setup:active" | "a=setup:passive" if answer => SETUP,
            "a=end-of-candidates" if candidate.is_some() => GATHERED,
            _ => {
                let (name, value) = line.split_once(':').ok_or(POLICY)?;
                match name {
                    "a=ice-ufrag" if valid_ice(value, 4) => UFRAG,
                    "a=ice-pwd" if valid_ice(value, 22) => PASSWORD,
                    // The native default (64 KiB) is an upper bound, not
                    // permission for anything beyond fixed markers.
                    "a=max-message-size"
                        if decimal::<u32>(value).is_some_and(|n| (1..=65536).contains(&n)) =>
                    {
                        MESSAGE_SIZE
                    }
                    "a=mid" if valid_mid(value) && mid.replace(value).is_none() => continue,
                    "a=candidate" if seen & GATHERED == 0 && candidate.is_none() => {
                        candidate = Some(validate_candidate(value, relay, port_min, port_max)?);
                        continue;
                    }
                    _ => return Err(POLICY),
                }
            }
        };
        mark(&mut seen, flag)?;
    }

    if !connection
        || mid.is_none()
        || mid != bundle
        || fingerprint.is_none()
        || seen & REQUIRED != REQUIRED
    {
        return Err(POLICY);
    }
    candidate.ok_or(POLICY)
}

fn mark(seen: &mut u16, flag: u16) -> Result<(), &'static str> {
    if *seen & flag != 0 {
        return Err(POLICY);
    }
    *seen |= flag;
    Ok(())
}

fn validate_candidate(
    value: &str,
    relay: Ipv4Addr,
    port_min: u16,
    port_max: u16,
) -> Result<SocketAddrV4, &'static str> {
    let fields: Vec<_> = value.split(' ').collect();
    let [foundation, "1", "udp", priority, ip, port, "typ", "relay", "raddr", "0.0.0.0", "rport", "0"] =
        fields.as_slice()
    else {
        return Err(POLICY);
    };
    if foundation.is_empty()
        || foundation.len() > 32
        || !foundation.bytes().all(ice_byte)
        || !decimal::<u32>(priority).is_some_and(|n| n != 0)
        || *ip != relay.to_string()
    {
        return Err(POLICY);
    }
    let port = decimal::<u16>(port)
        .filter(|port| (port_min..=port_max).contains(port))
        .ok_or(POLICY)?;
    Ok(SocketAddrV4::new(relay, port))
}

fn valid_origin(line: &str) -> bool {
    // Bound this line separately before constructing any fields from it.
    if line.len() > MAX_LINE {
        return false;
    }
    let fields: Vec<_> = line.split(' ').collect();
    matches!(fields.as_slice(), ["o=-", id, version, "IN", "IP4", "0.0.0.0"]
        if decimal::<u64>(id).is_some() && decimal::<u64>(version).is_some())
}

fn decimal<T: std::str::FromStr + ToString>(value: &str) -> Option<T> {
    value.parse::<T>().ok().filter(|n| n.to_string() == value)
}

fn valid_mid(value: &str) -> bool {
    (1..=16).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

fn ice_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/')
}

fn valid_ice(value: &str, min: usize) -> bool {
    (min..=256).contains(&value.len()) && value.bytes().all(ice_byte)
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 95
        && value
            .split(':')
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);
    const END: &str = "a=end-of-candidates\r\n";
    const CANDIDATE: &str =
        "a=candidate:r 1 udp 16777215 8.8.8.8 49160 typ relay raddr 0.0.0.0 rport 0\r\n";

    fn descriptor(setup: &str) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:sha-256 {}\r\na=group:BUNDLE 0\r\n{MEDIA}\r\nc=IN IP4 0.0.0.0\r\na=setup:{setup}\r\na=mid:0\r\na=sendrecv\r\na=sctp-port:5000\r\na=max-message-size:65536\r\na=ice-ufrag:ABCD\r\na=ice-pwd:abcdefghijklmnopqrstuv\r\n{CANDIDATE}{END}",
            ["AB"; 32].join(":")
        )
    }

    fn check(sdp: &str, answer: bool) -> Result<SocketAddrV4, &'static str> {
        validate(sdp, RELAY, 49152, 49216, answer)
    }

    #[test]
    fn native_sanitized_offer_and_answer_return_the_exact_tuple() {
        use webrtc::peer_connection::RTCSessionDescription;
        let endpoint = SocketAddrV4::new(RELAY, 49160);
        let offer = descriptor("actpass");
        let answer = descriptor("passive");
        assert_eq!(check(&offer, false), Ok(endpoint));
        assert_eq!(check(&answer, true), Ok(endpoint));
        // Parse with the same locked SDP type the diagnostic will use.
        RTCSessionDescription::offer(offer).unwrap();
        RTCSessionDescription::answer(answer).unwrap();
        assert_eq!(check(&descriptor("active"), true), Ok(endpoint));
        assert_eq!(check(&descriptor("active"), false), Err(POLICY));
        assert_eq!(check(&descriptor("actpass"), true), Err(POLICY));
    }

    #[test]
    fn routes_are_single_numeric_exact_and_redacted() {
        let good = descriptor("actpass");
        for bad in [
            good.replace("typ relay", "typ host"),
            good.replace("typ relay", "typ srflx"),
            good.replace("typ relay", "typ prflx"),
            good.replace("8.8.8.8", "relay.invalid"),
            good.replace("8.8.8.8", "8.8.4.4"),
            good.replace("8.8.8.8", "::ffff:8.8.8.8"),
            good.replace("49160", "49151"),
            good.replace("49160", "049160"),
            good.replace("r 1 udp", "r 2 udp"),
            good.replace("r 1 udp", "r 1 tcp"),
            good.replace("raddr 0.0.0.0", "raddr 192.168.1.5"),
            good.replace("rport 0", "rport 12345"),
            good.replace("rport 0", "rport 0 generation 0"),
            good.replace(CANDIDATE, ""),
            good.replace(END, &format!("{CANDIDATE}{END}")),
            good.replace(
                END,
                &format!("{}{END}", CANDIDATE.replace("49160", "49161")),
            ),
            good.replace(CANDIDATE, "")
                .replace(END, &format!("{END}{CANDIDATE}")),
        ] {
            assert_eq!(check(&bad, false), Err(POLICY), "{bad:?}");
        }
        assert_eq!(
            validate(&good, "127.0.0.1".parse().unwrap(), 49152, 49216, false),
            Err(POLICY)
        );
        assert_eq!(validate(&good, RELAY, 49216, 49152, false), Err(POLICY));
    }

    #[test]
    fn media_attributes_fingerprint_and_bundle_are_unambiguous() {
        let good = descriptor("actpass");
        for bad in [
            good.replace(MEDIA, "m=video 9 UDP/TLS/RTP/SAVPF 102"),
            good.replace(MEDIA, "m=application 9 TCP/DTLS/SCTP webrtc-datachannel"),
            good.replace(MEDIA, "m=application 0 UDP/DTLS/SCTP webrtc-datachannel"),
            good.replace(MEDIA, "m=application 9 UDP/DTLS/SCTP webrtc-datachannel 2"),
            good.replace(MEDIA, &format!("{MEDIA}\r\n{MEDIA}")),
            good.replace("a=group:BUNDLE 0", "a=group:BUNDLE 0 1"),
            good.replace("a=mid:0", "a=mid:1"),
            good.replace("a=mid:0", "a=mid:0\r\na=mid:0"),
            good.replace("a=setup:actpass", "a=setup:actpass\r\na=setup:active"),
            good.replace("a=ice-ufrag:ABCD", "a=ice-ufrag:ABCD\r\na=ice-ufrag:ABCD"),
            good.replace("a=ice-pwd:abcdefghijklmnopqrstuv", "a=ice-pwd:short"),
            good.replace("a=sctp-port:5000\r\n", ""),
            good.replace("a=max-message-size:65536", "a=max-message-size:0"),
            good.replace("a=fingerprint:sha-256", "a=fingerprint:sha-1"),
            good.replace("AB:AB", "GG:GG"),
            good.replace(
                END,
                &format!("a=fingerprint:sha-256 {}\r\n{END}", ["AB"; 32].join(":")),
            ),
            good.replace("c=IN IP4 0.0.0.0", "c=IN IP4 192.168.1.6"),
            good.replace("o=- 1 2 IN IP4 0.0.0.0", "o=- 1 2 IN IP4 192.168.1.6"),
            good.replace(
                END,
                &format!("a=remote-candidates:1 8.8.4.4 49160\r\n{END}"),
            ),
            good.replace(END, &format!("a=ice-options:trickle\r\n{END}")),
            good.replacen("\r\n", "\n", 1),
            format!("{good}{}", "x".repeat(MAX_SDP)),
        ] {
            assert_eq!(check(&bad, false), Err(POLICY), "{bad:?}");
        }
    }
}
