//! Strict native-only scoped ICE SDP for the separately signed product-control
//! purpose. Exactly one bundled H.264 section and one reliable SCTP media
//! section may use the same signed transport description.
use crate::{direct_policy::Candidate, public_video::PublicVideoScope};

pub(crate) const ERROR: &str = "PUBLIC_CONTROL_SDP_POLICY";
const VIDEO: &str = "m=video 9 UDP/TLS/RTP/SAVPF 102";
const SCTP: &str = "m=application 9 UDP/DTLS/SCTP webrtc-datachannel";
const NEUTRAL: &str = "c=IN IP4 0.0.0.0";
const MEDIA_MESSAGE_MAX: u32 = 65536;

type Result<T> = std::result::Result<T, &'static str>;

#[derive(Default)]
struct Transport<'a> {
    mid: Option<&'a str>,
    ufrag: Option<&'a str>,
    pwd: Option<&'a str>,
    setup: Option<&'a str>,
}

fn set<'a>(slot: &mut Option<&'a str>, value: &'a str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(ERROR);
    }
    Ok(())
}

fn token(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
}

impl<'a> Transport<'a> {
    fn read(&mut self, line: &'a str, answer: bool) -> Result<bool> {
        if let Some(value) = line.strip_prefix("a=mid:") {
            if !token(value, 1, 16) {
                return Err(ERROR);
            }
            set(&mut self.mid, value)?;
        } else if let Some(value) = line.strip_prefix("a=ice-ufrag:") {
            if !token(value, 4, 256) {
                return Err(ERROR);
            }
            set(&mut self.ufrag, value)?;
        } else if let Some(value) = line.strip_prefix("a=ice-pwd:") {
            if !token(value, 22, 256) {
                return Err(ERROR);
            }
            set(&mut self.pwd, value)?;
        } else if let Some(value) = line.strip_prefix("a=setup:") {
            if if answer {
                !matches!(value, "active" | "passive")
            } else {
                value != "actpass"
            } {
                return Err(ERROR);
            }
            set(&mut self.setup, value)?;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn same_bundle(&self, other: &Self) -> bool {
        self.ufrag.is_some()
            && self.ufrag == other.ufrag
            && self.pwd.is_some()
            && self.pwd == other.pwd
            && self.setup.is_some()
            && self.setup == other.setup
    }
}

pub(crate) fn validate(
    sdp: &str,
    scope: PublicVideoScope,
    answer: bool,
    now: u64,
) -> Result<Vec<Candidate>> {
    scope.checked_control_batch(now).map_err(|_| ERROR)?;
    validate_shape(sdp, scope, answer)
}

pub(super) fn validate_shape(
    sdp: &str,
    scope: PublicVideoScope,
    answer: bool,
) -> Result<Vec<Candidate>> {
    if sdp.len() > hub_identity::session::MAX_SIGNAL_BYTES
        || !sdp.is_ascii()
        || !sdp.starts_with("v=0\r\n")
    {
        return Err(ERROR);
    }
    let (video_only, app) = sdp.split_once(SCTP).ok_or(ERROR)?;
    if !video_only.ends_with("\r\n") {
        return Err(ERROR);
    }
    let (session, video) = video_only.split_once(VIDEO).ok_or(ERROR)?;
    if !session.ends_with("\r\n") {
        return Err(ERROR);
    }
    // Verify exact section boundaries: no forward-search or alternate line
    // endings can make the application parser and downstream RTC disagree.
    let mut bundle = None;
    let mut fingerprint = false;
    let mut origin = false;
    for line in session.lines() {
        if let Some(value) = line.strip_prefix("a=group:BUNDLE ") {
            set(&mut bundle, value)?;
        } else if line.starts_with("o=") && !origin {
            origin = true;
        } else if line.starts_with("a=fingerprint:sha-256 ") && !fingerprint {
            fingerprint = true;
        } else if line.starts_with("a=fingerprint:")
            || line.starts_with("m=")
            || line.starts_with("a=group:")
            || line.starts_with("a=ice-")
            || line.starts_with("a=setup:")
            || line.starts_with("a=mid:")
            || line.starts_with("a=candidate:")
            || line.starts_with("a=end-of-candidates")
            || line.starts_with("a=send")
            || line.starts_with("a=recv")
        {
            return Err(ERROR);
        }
    }
    if !fingerprint || !origin {
        return Err(ERROR);
    }
    let lines = |rest: &str| {
        if !rest.starts_with("\r\n") || !rest.ends_with("\r\n") {
            return Err(ERROR);
        }
        Ok(())
    };
    lines(video)?;
    lines(app)?;
    let mut video_transport = Transport::default();
    let mut video_connection = false;
    let mut gathered = false;
    for line in video.lines().skip(1) {
        if line == NEUTRAL {
            video_connection = true;
        } else if line == "a=end-of-candidates" && !gathered {
            gathered = true;
        } else if gathered && line.starts_with("a=candidate:") {
            return Err(ERROR);
        } else if !video_transport.read(line, answer)?
            && (line.starts_with("a=group:")
                || line.starts_with("a=ice-options:")
                || line == "a=end-of-candidates"
                || line.starts_with("m="))
        {
            return Err(ERROR);
        }
    }
    if !video_connection || !gathered {
        return Err(ERROR);
    }

    let mut app_transport = Transport::default();
    let mut connection = false;
    let mut direction = false;
    let mut port = false;
    let mut size = false;
    let app = app
        .strip_prefix("\r\n")
        .and_then(|rest| rest.strip_suffix("\r\n"))
        .ok_or(ERROR)?;
    for line in app.split("\r\n") {
        if app_transport.read(line, answer)? {
            continue;
        }
        if line == NEUTRAL && !connection {
            connection = true;
        } else if line == "a=sendrecv" && !direction {
            direction = true;
        } else if line == "a=sctp-port:5000" && !port {
            port = true;
        } else if let Some(value) = line.strip_prefix("a=max-message-size:") {
            if size
                || !value.parse::<u32>().is_ok_and(|limit| {
                    (crate::control::MAX_BYTES as u32..=MEDIA_MESSAGE_MAX).contains(&limit)
                        && limit.to_string() == value
                })
            {
                return Err(ERROR);
            }
            size = true;
        } else {
            // No ICE candidates, alternate fingerprint, unbundled setup,
            // or any extension capable of selecting an alternate path here.
            return Err(ERROR);
        }
    }
    let mut mids = bundle.ok_or(ERROR)?.split(' ');
    let expected_video = video_transport.mid.ok_or(ERROR)?;
    let expected_app = app_transport.mid.ok_or(ERROR)?;
    if expected_video == expected_app
        || mids.next() != Some(expected_video)
        || mids.next() != Some(expected_app)
        || mids.next().is_some()
        || !video_transport.same_bundle(&app_transport)
        || !(connection && direction && port && size)
    {
        return Err(ERROR);
    }

    // Reuse the existing video/candidate allowlist without relaxing the
    // video-only public entry point. Its codec parser doesn't inspect BUNDLE;
    // both exact ordered MIDs were checked above against signed bytes.
    let candidates =
        crate::public_video_ice::validate_shape(video_only, scope, answer).map_err(|_| ERROR)?;
    if scope.mode == crate::ice_policy::IceMode::RelayOnly && candidates.len() != 1 {
        return Err(ERROR);
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ice_policy::IceMode,
        public_video::{PublicControlProof, PublicVideoProof},
    };
    use hub_identity::{pairing::PairingCandidate, session::SessionRole, Identity};
    use uuid::Uuid;

    const NOW: u64 = 1000;
    const GENERATION: [u8; 32] = [0xb7; 32];
    const CANDIDATE: &str = "a=candidate:r 1 udp 1 8.8.8.8 49152 typ relay raddr 0.0.0.0 rport 0";

    fn scope() -> PublicVideoScope {
        PublicVideoScope {
            wss: "8.8.8.8:8443".parse().unwrap(),
            ca_sha256: [7; 32],
            turn: "8.8.8.8:3478".parse().unwrap(),
            relay_ports: (49152, 49200),
            mode: IceMode::RelayOnly,
            expires_at: NOW + 90,
        }
    }

    fn description(answer: bool) -> String {
        let fingerprint = ["AB"; 32].join(":");
        let direction = if answer { "sendonly" } else { "recvonly" };
        let setup = if answer { "active" } else { "actpass" };
        let credentials =
            format!("a=ice-ufrag:abcd\r\na=ice-pwd:abcdefghijklmnopqrstuv\r\na=setup:{setup}\r\n");
        let candidate = if answer {
            CANDIDATE.replace("49152", "49153")
        } else {
            CANDIDATE.to_owned()
        };
        format!(
            "v=0\r\no=- 1 2 IN IP4 0.0.0.0\r\ns=-\r\nt=0 0\r\na=fingerprint:sha-256 {fingerprint}\r\na=group:BUNDLE 0 1\r\n{VIDEO}\r\n{NEUTRAL}\r\na=mid:0\r\n{credentials}a={direction}\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\n{candidate}\r\na=end-of-candidates\r\n{SCTP}\r\n{NEUTRAL}\r\na=mid:1\r\n{credentials}a=sendrecv\r\na=sctp-port:5000\r\na=max-message-size:65536\r\n",
        )
    }

    #[test]
    fn exact_turn_bundle_rejects_duplicate_or_alternate_routing() {
        let sdp = description(false);
        let answer = description(true);
        assert_eq!(validate(&sdp, scope(), false, NOW).unwrap().len(), 1);
        assert_eq!(validate(&answer, scope(), true, NOW).unwrap().len(), 1);
        assert!(crate::public_video_ice::validate(&sdp, scope(), false, NOW).is_err());
        assert!(validate(&description(true), scope(), false, NOW).is_err());
        for bad in [
            sdp.replace("BUNDLE 0 1", "BUNDLE 1 0"),
            sdp.replace("a=mid:1", "a=mid:0"),
            sdp.replace("a=mid:1", "a=mid:1\r\na=ice-ufrag:wrong"),
            sdp.replace("a=mid:1\r\na=ice-ufrag:abcd", "a=mid:1\r\na=ice-ufrag:efgh"),
            sdp.replace("a=mid:1", "a=mid:1\r\na=setup:passive"),
            sdp.replace("a=sctp-port:5000", "a=sctp-port:5001"),
            sdp.replace("a=max-message-size:65536", "a=max-message-size:65537"),
            sdp.replace("a=sendrecv", "a=recvonly"),
            sdp.replace("a=end-of-candidates\r\n", ""),
            sdp.replace("a=rtcp-mux", "a=rtcp-mux\r\na=ice-options:trickle"),
            sdp.replace("typ relay", "typ host"),
            sdp.replace("raddr 0.0.0.0", "raddr 192.168.1.4"),
            sdp.replace("49152", "49151"),
            sdp.replace("c=IN IP4 0.0.0.0", "c=IN IP4 10.0.0.1"),
            sdp.replace("video 9", "audio 9"),
            sdp.replace("a=mid:1", &format!("a=mid:1\r\n{CANDIDATE}")),
            sdp.replace(
                "a=mid:1",
                &format!("a=mid:1\r\na=fingerprint:sha-256 {}", ["AB"; 32].join(":")),
            ),
            format!(
                "{}\r\n",
                sdp[..sdp.find(SCTP).unwrap() + SCTP.len()].trim_end()
            ),
        ] {
            assert!(matches!(validate(&bad, scope(), false, NOW), Err(ERROR)));
        }
    }

    #[cfg(feature = "v2-public-control-sustained")]
    #[test]
    fn sustained_proof_has_its_own_domain_and_fixed_bounded_scope() {
        use crate::public_video::{
            SUSTAINED_CONTROL_SCOPE_TTL_SECONDS, SUSTAINED_CONTROL_SESSION_SECONDS,
        };
        assert_eq!(SUSTAINED_CONTROL_SESSION_SECONDS, 1800);
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let host_pin = host
            .verify_peer(
                &PairingCandidate::parse(&viewer.pairing_code()).unwrap(),
                &viewer.public().fingerprint,
            )
            .unwrap();
        let viewer_pin = viewer
            .verify_peer(
                &PairingCandidate::parse(&host.pairing_code()).unwrap(),
                &host.public().fingerprint,
            )
            .unwrap();
        let room = Uuid::new_v4();
        let extended = PublicVideoScope {
            expires_at: NOW + SUSTAINED_CONTROL_SCOPE_TTL_SECONDS,
            ..scope()
        };
        let sustained_host = |scope| {
            PublicControlProof::prepare_sustained(
                &host,
                &host_pin,
                SessionRole::Host,
                room,
                GENERATION,
                scope,
                NOW,
            )
        };
        let sustained_viewer = |scope| {
            PublicControlProof::prepare_sustained(
                &viewer,
                &viewer_pin,
                SessionRole::Viewer,
                room,
                GENERATION,
                scope,
                NOW,
            )
        };
        assert!(sustained_host(PublicVideoScope {
            expires_at: extended.expires_at + 1,
            ..extended
        })
        .is_err());
        assert!(sustained_host(PublicVideoScope {
            expires_at: NOW,
            ..extended
        })
        .is_err());
        assert!(PublicControlProof::prepare_immediate_batch(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            extended,
            NOW
        )
        .is_err());

        let offer = description(false);
        let answer = description(true);
        let mut receiver = sustained_host(extended).unwrap();
        let mut sender = sustained_viewer(extended).unwrap();
        let signature = sender
            .seal(receiver.challenge().unwrap(), &offer, NOW)
            .unwrap();
        receiver.verify(&offer, &signature, NOW).unwrap();
        let response = receiver
            .seal(sender.challenge().unwrap(), &answer, NOW)
            .unwrap();
        sender.verify(&answer, &response, NOW).unwrap();
        assert!(sender.verify(&answer, &response, NOW).is_err());

        // Overlapping expiry removes timing as the reason for rejecting an
        // old-purpose signature. Exact bytes are identical.
        let short = scope();
        let mut new = sustained_host(short).unwrap();
        let mut old = PublicControlProof::prepare_immediate_batch(
            &viewer,
            &viewer_pin,
            SessionRole::Viewer,
            room,
            GENERATION,
            short,
            NOW,
        )
        .unwrap();
        let old_sig = old.seal(new.challenge().unwrap(), &offer, NOW).unwrap();
        assert!(new.verify(&offer, &old_sig, NOW).is_err());

        let mut old = PublicControlProof::prepare(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            short,
            NOW,
        )
        .unwrap();
        let mut new = sustained_viewer(short).unwrap();
        let new_sig = new.seal(old.challenge().unwrap(), &offer, NOW).unwrap();
        assert!(old.verify(&offer, &new_sig, NOW).is_err());

        let mut receiver = sustained_host(extended).unwrap();
        let mut sender = sustained_viewer(extended).unwrap();
        let signature = sender
            .seal(receiver.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(receiver.verify(&offer, &signature, NOW + 15).is_err());
    }

    #[test]
    fn control_signatures_bind_purpose_scope_and_exact_offer() {
        let host = Identity::generate_ephemeral().unwrap();
        let viewer = Identity::generate_ephemeral().unwrap();
        let host_pin = host
            .verify_peer(
                &PairingCandidate::parse(&viewer.pairing_code()).unwrap(),
                &viewer.public().fingerprint,
            )
            .unwrap();
        let viewer_pin = viewer
            .verify_peer(
                &PairingCandidate::parse(&host.pairing_code()).unwrap(),
                &host.public().fingerprint,
            )
            .unwrap();
        let room = Uuid::new_v4();
        let host_p = || {
            PublicControlProof::prepare(
                &host,
                &host_pin,
                SessionRole::Host,
                room,
                GENERATION,
                scope(),
                NOW,
            )
            .unwrap()
        };
        let viewer_p = || {
            PublicControlProof::prepare(
                &viewer,
                &viewer_pin,
                SessionRole::Viewer,
                room,
                GENERATION,
                scope(),
                NOW,
            )
            .unwrap()
        };
        let offer = description(false);
        let answer = description(true);
        let mut a = host_p();
        let mut b = viewer_p();
        let offer_sig = b.seal(a.challenge().unwrap(), &offer, NOW).unwrap();
        a.verify(&offer, &offer_sig, NOW).unwrap();
        let answer_sig = a.seal(b.challenge().unwrap(), &answer, NOW).unwrap();
        b.verify(&answer, &answer_sig, NOW).unwrap();
        assert!(b.verify(&answer, &answer_sig, NOW).is_err());

        let mut video_host = PublicVideoProof::prepare(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            scope(),
            NOW,
        )
        .unwrap();
        let control_signed = viewer_p()
            .seal(video_host.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(video_host.open(&offer, &control_signed, NOW).is_err());

        let mut control_host = host_p();
        let mut video_viewer = PublicVideoProof::prepare(
            &viewer,
            &viewer_pin,
            SessionRole::Viewer,
            room,
            GENERATION,
            scope(),
            NOW,
        )
        .unwrap();
        let video_signed = video_viewer
            .seal(control_host.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(control_host.verify(&offer, &video_signed, NOW).is_err());
        assert!(control_host.challenge().is_none());

        let mut batch_host = PublicControlProof::prepare_immediate_batch(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            scope(),
            NOW,
        )
        .unwrap();
        let ordinary_signed = viewer_p()
            .seal(batch_host.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(batch_host.verify(&offer, &ordinary_signed, NOW).is_err());

        let far_scope = PublicVideoScope {
            expires_at: NOW + 1800,
            ..scope()
        };
        let mut expire_host = PublicControlProof::prepare_immediate_batch(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            far_scope,
            NOW,
        )
        .unwrap();
        let mut expire_viewer = PublicControlProof::prepare_immediate_batch(
            &viewer,
            &viewer_pin,
            SessionRole::Viewer,
            room,
            GENERATION,
            far_scope,
            NOW,
        )
        .unwrap();
        let signature = expire_viewer
            .seal(expire_host.challenge().unwrap(), &offer, NOW)
            .unwrap();
        expire_host.verify(&offer, &signature, NOW).unwrap();
        assert!(PublicControlProof::prepare(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            far_scope,
            NOW
        )
        .is_err());
        assert!(PublicControlProof::prepare_immediate_batch(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            PublicVideoScope {
                expires_at: NOW + 1801,
                ..scope()
            },
            NOW
        )
        .is_err());

        let mut wrong_generation = PublicControlProof::prepare(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            [0x10; 32],
            scope(),
            NOW,
        )
        .unwrap();
        let mismatched = viewer_p()
            .seal(wrong_generation.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(wrong_generation.verify(&offer, &mismatched, NOW).is_err());
        let mut automatic = PublicControlProof::prepare(
            &host,
            &host_pin,
            SessionRole::Host,
            room,
            GENERATION,
            PublicVideoScope {
                mode: IceMode::DirectPreferred,
                ..scope()
            },
            NOW,
        )
        .unwrap();
        let relay_signature = viewer_p()
            .seal(automatic.challenge().unwrap(), &offer, NOW)
            .unwrap();
        assert!(automatic.verify(&offer, &relay_signature, NOW).is_err());
    }
}
