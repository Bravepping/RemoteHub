//! Outgoing SDP preparation for the in-process TURN fixture only.
//! A peer's signed bytes must still pass the strict validator unchanged.
use crate::public_fake_input_sdp;
use std::{fmt::Write, net::Ipv4Addr};

const REJECTED: &str = "PUBLIC_FAKE_INPUT_FIXTURE_SDP_REJECTED";
const MAX_SDP: usize = 4096;
const MAX_LINE: usize = 512;
const MEDIA: &str = "m=application 9 UDP/DTLS/SCTP webrtc-datachannel";

pub(crate) fn sanitize(
    sdp: &str,
    relay: Ipv4Addr,
    port_min: u16,
    port_max: u16,
    answer: bool,
) -> Result<String, &'static str> {
    if sdp.len() > MAX_SDP || !sdp.is_ascii() {
        return Err(REJECTED);
    }
    let mut clean = String::with_capacity(sdp.len());
    let mut media = false;
    let mut primary = None;
    let mut mirrored = false;
    let mut gathered = false;
    for line in sdp.strip_suffix("\r\n").ok_or(REJECTED)?.split("\r\n") {
        if line.len() > MAX_LINE
            || line.trim() != line
            || line.bytes().any(|b| b.is_ascii_control())
        {
            return Err(REJECTED);
        }
        if line.starts_with("m=") {
            if media || line != MEDIA {
                return Err(REJECTED);
            }
            media = true;
        }
        if line == "a=end-of-candidates" {
            if gathered || primary.is_none() {
                return Err(REJECTED);
            }
            gathered = true;
        }
        if let Some(value) = line.strip_prefix("a=candidate:") {
            if !media || gathered {
                return Err(REJECTED);
            }
            let fields = value.split(' ').collect::<Vec<_>>();
            let [foundation, component, "udp", priority, ip, port, "typ", "relay", "raddr", related_ip, "rport", related_port] =
                fields.as_slice()
            else {
                return Err(REJECTED);
            };
            if related_ip.parse::<Ipv4Addr>().is_err()
                || !related_port
                    .parse::<u16>()
                    .is_ok_and(|n| n.to_string() == *related_port)
            {
                return Err(REJECTED);
            }
            // Locked rtc emits the same candidate twice with only component
            // changed. Never let a second candidate hide as that mirror.
            let exact = [
                *foundation,
                *priority,
                *ip,
                *port,
                *related_ip,
                *related_port,
            ];
            match *component {
                "1" if primary.is_none() => {
                    primary = Some(exact);
                    write!(
                        &mut clean,
                        "a=candidate:{foundation} 1 udp {priority} {ip} {port} typ relay raddr 0.0.0.0 rport 0\r\n"
                    )
                    .map_err(|_| REJECTED)?;
                }
                "2" if !mirrored && primary == Some(exact) => mirrored = true,
                _ => return Err(REJECTED),
            }
        } else {
            clean.push_str(line);
            clean.push_str("\r\n");
        }
    }
    if !gathered {
        return Err(REJECTED);
    }
    public_fake_input_sdp::validate(&clean, relay, port_min, port_max, answer)?;
    Ok(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELAY: Ipv4Addr = Ipv4Addr::new(93, 184, 216, 34);
    const PRIMARY: &str = "a=candidate:r 1 udp 16777215 93.184.216.34 49160 typ relay raddr 127.0.0.1 rport 43210\r\n";
    const MIRROR: &str = "a=candidate:r 2 udp 16777215 93.184.216.34 49160 typ relay raddr 127.0.0.1 rport 43210\r\n";
    const END: &str = "a=end-of-candidates\r\n";

    fn native_sdp(candidates: &str, setup: &str) -> String {
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:sha-256 {}\r\na=group:BUNDLE 0\r\n{MEDIA}\r\nc=IN IP4 0.0.0.0\r\na=setup:{setup}\r\na=mid:0\r\na=sendrecv\r\na=sctp-port:5000\r\na=max-message-size:65536\r\na=ice-ufrag:ABCD\r\na=ice-pwd:abcdefghijklmnopqrstuv\r\n{candidates}{END}",
            ["AB"; 32].join(":")
        )
    }

    fn check(sdp: &str, answer: bool) -> Result<String, &'static str> {
        sanitize(sdp, RELAY, 49152, 49216, answer)
    }

    #[test]
    fn native_mirror_removed_and_only_related_tuple_redacted() {
        let native = native_sdp(&format!("{PRIMARY}{MIRROR}"), "actpass");
        let clean = check(&native, false).unwrap();
        let expected = PRIMARY.replace("127.0.0.1 rport 43210", "0.0.0.0 rport 0");
        assert_eq!(clean, native_sdp(&expected, "actpass"));
        assert_eq!(
            public_fake_input_sdp::validate(&clean, RELAY, 49152, 49216, false),
            Ok("93.184.216.34:49160".parse().unwrap())
        );
        assert!(check(&native_sdp(PRIMARY, "active"), true).is_ok());
        assert!(check(&native_sdp(PRIMARY, "active"), false).is_err());
    }

    #[test]
    fn unknown_media_route_or_second_allocation_is_not_hidden() {
        let offer = native_sdp(&format!("{PRIMARY}{MIRROR}"), "actpass");
        for bad in [
            native_sdp(MIRROR, "actpass"),
            native_sdp(&format!("{MIRROR}{PRIMARY}"), "actpass"),
            native_sdp(&format!("{PRIMARY}{PRIMARY}"), "actpass"),
            native_sdp(&format!("{PRIMARY}{MIRROR}{MIRROR}"), "actpass"),
            offer.replace(MIRROR, &MIRROR.replace("49160", "49161")),
            offer.replace(MIRROR, &MIRROR.replace("16777215", "16777214")),
            offer.replace(MIRROR, &MIRROR.replace("typ relay", "typ host")),
            offer.replace(MIRROR, &MIRROR.replace("127.0.0.1", "127.0.0.2")),
            offer.replace(MIRROR, &MIRROR.replace("rport 43210", "rport 43211")),
            offer.replace(
                MIRROR,
                &MIRROR.replace("rport 43210", "rport 43210 generation 0"),
            ),
            offer.replace("93.184.216.34", "8.8.8.8"),
            offer.replace(MEDIA, "m=video 9 UDP/TLS/RTP/SAVPF 102"),
            offer.replace(MEDIA, &format!("{MEDIA}\r\n{MEDIA}")),
            offer.replace("c=IN IP4 0.0.0.0", "c=IN IP4 127.0.0.1"),
            offer.replace(END, ""),
            offer
                .replace(MIRROR, "")
                .replace(END, &format!("{END}{MIRROR}")),
            offer.replace("\r\n", "\n"),
        ] {
            assert!(check(&bad, false).is_err(), "{bad:?}");
        }
    }
}
