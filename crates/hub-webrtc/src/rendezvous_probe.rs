//! Offline authenticated envelope verifier, not a deployed WSS service or device identity.
use crate::{
    ice_policy::RelayGrant,
    signaling::{validate_relay_description, Negotiation},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    version: u8,
    session: String,
    direction: String,
    nonce: String,
    seq: u64,
    expires_at: u64,
    kind: String,
    payload: String,
    mac: String,
}
pub struct Verifier {
    key: Zeroizing<Vec<u8>>,
    session: String,
    nonce: String,
    expected_direction: String,
    last_seq: u64,
    window: u64,
    count: u32,
    closed: bool,
}
impl Verifier {
    pub fn new(
        key: Vec<u8>,
        grant: &RelayGrant,
        nonce: &str,
        expected_direction: &str,
    ) -> Result<Self, String> {
        let key = Zeroizing::new(key);
        if key.len() != 32
            || !uuid::Uuid::parse_str(nonce).is_ok_and(|id| !id.is_nil())
            || !["host-to-controller", "controller-to-host"].contains(&expected_direction)
        {
            return Err("PROBE_AUTH_CONTEXT_INVALID".into());
        }
        Ok(Self {
            key,
            session: grant.session().into(),
            nonce: nonce.into(),
            expected_direction: expected_direction.into(),
            last_seq: 0,
            window: 0,
            count: 0,
            closed: false,
        })
    }
    fn tag(&self, frame: &Frame) -> Result<Hmac<Sha256>, String> {
        let bytes = serde_json::to_vec(&(
            "rendezvous-probe.v1",
            frame.version,
            &frame.session,
            &frame.direction,
            &frame.nonce,
            frame.seq,
            frame.expires_at,
            &frame.kind,
            &frame.payload,
        ))
        .map_err(|_| "PROBE_AUTH_ENCODING")?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).map_err(|_| "PROBE_AUTH_KEY")?;
        mac.update(&bytes);
        Ok(mac)
    }
    /// Authenticate and apply the relay SDP policy as one fail-closed operation.
    pub fn accept_sdp(
        &mut self,
        text: &str,
        grant: &RelayGrant,
        now: u64,
        answer: bool,
    ) -> Result<Negotiation, String> {
        let result = self.checked(text, grant, now, answer);
        if result.is_err() {
            self.closed = true;
        }
        result
    }
    fn checked(
        &mut self,
        text: &str,
        grant: &RelayGrant,
        now: u64,
        answer: bool,
    ) -> Result<Negotiation, String> {
        if self.closed
            || !grant.is_current(now)
            || grant.session() != self.session
            || text.len() > 96 * 1024
        {
            return Err("PROBE_AUTH_EXPIRED_OR_CLOSED".into());
        }
        let frame: Frame = serde_json::from_str(text).map_err(|_| "PROBE_AUTH_FRAME")?;
        if frame.version != 1
            || frame.session != self.session
            || frame.direction != self.expected_direction
            || frame.nonce != self.nonce
            || frame.seq != self.last_seq + 1
            || frame.seq > 256
            || frame.expires_at <= now
            || frame.expires_at > now.saturating_add(600)
            || !grant.is_current(frame.expires_at.saturating_sub(1))
            || frame.kind != if answer { "answer" } else { "offer" }
            || frame.payload.len() > 64 * 1024
            || frame.mac.len() != 64
            || !frame.mac.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("PROBE_AUTH_SCOPE_OR_SEQUENCE".into());
        }
        if self.window != now {
            self.window = now;
            self.count = 0;
        }
        self.count += 1;
        if self.count > 10 {
            return Err("PROBE_AUTH_RATE".into());
        }
        let mut tag = [0; 32];
        for (i, value) in tag.iter_mut().enumerate() {
            *value = u8::from_str_radix(&frame.mac[i * 2..i * 2 + 2], 16)
                .map_err(|_| "PROBE_AUTH_MAC")?;
        }
        self.tag(&frame)?
            .verify_slice(&tag)
            .map_err(|_| "PROBE_AUTH_MAC")?;
        let accepted = validate_relay_description(&frame.payload, grant, now, answer)?;
        self.last_seq = frame.seq;
        Ok(accepted)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (RelayGrant, Verifier, String) {
        let (g, _) = crate::ice_policy::tests::fixture();
        let nonce = "00000000-0000-4000-8000-000000000002";
        let verifier = Verifier::new(vec![7; 32], &g, nonce, "controller-to-host").unwrap();
        let sdp=format!("v=0\r\na=group:BUNDLE 0 1\r\na=fingerprint:sha-256 {}\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=recvonly\r\na=mid:0\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=candidate:r 1 udp 1 8.8.8.8 49160 typ relay\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:1\r\na=sctp-port:5000\r\na=max-message-size:16384\r\n",["AB";32].join(":"));
        let mut f = Frame {
            version: 1,
            session: g.session().into(),
            direction: "controller-to-host".into(),
            nonce: nonce.into(),
            seq: 1,
            expires_at: 1300,
            kind: "offer".into(),
            payload: sdp,
            mac: String::new(),
        };
        f.mac = verifier
            .tag(&f)
            .unwrap()
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        (g, verifier, serde_json::to_string(&f).unwrap())
    }
    #[test]
    fn authenticated_relay_sdp_and_replay_reflection_expiry_are_fail_closed() {
        let (g, mut v, good) = fixture();
        assert!(v.accept_sdp(&good, &g, 1000, false).is_ok());
        assert!(v.accept_sdp(&good, &g, 1000, false).is_err());
        for mutate in [
            good.replace("controller-to-host", "host-to-controller"),
            good.replace("AB:AB", "CD:CD"),
            good.replace("49160 typ relay", "49161 typ relay"),
        ] {
            let (g, mut v, _) = fixture();
            assert!(v.accept_sdp(&mutate, &g, 1000, false).is_err());
            assert!(v.accept_sdp(&good, &g, 1000, false).is_err());
        }
        let (g, mut v, good) = fixture();
        assert!(v.accept_sdp(&good, &g, 1300, false).is_err());
    }
    #[test]
    fn authenticated_limits_and_context_are_enforced() {
        let (g, _, good) = fixture();
        assert!(Verifier::new(
            vec![7; 31],
            &g,
            "00000000-0000-4000-8000-000000000002",
            "controller-to-host"
        )
        .is_err());
        assert!(Verifier::new(
            vec![7; 32],
            &g,
            "00000000-0000-0000-0000-000000000000",
            "controller-to-host"
        )
        .is_err());
        for field in ["seq", "session", "nonce", "expires_at", "kind", "payload"] {
            let (g, mut v, _) = fixture();
            let mut frame: Frame = serde_json::from_str(&good).unwrap();
            match field {
                "seq" => frame.seq = 2,
                "session" => frame.session = "00000000-0000-4000-8000-000000000003".into(),
                "nonce" => frame.nonce = "00000000-0000-4000-8000-000000000003".into(),
                "expires_at" => frame.expires_at = 1601,
                "kind" => frame.kind = "unknown".into(),
                _ => frame.payload = "x".repeat(65537),
            }
            // Even correctly authenticated frames cannot bypass schema/scope limits.
            frame.mac = v
                .tag(&frame)
                .unwrap()
                .finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            assert!(v
                .accept_sdp(&serde_json::to_string(&frame).unwrap(), &g, 1000, false)
                .is_err());
            assert!(v.accept_sdp(&good, &g, 1000, false).is_err());
        }
        let (g, mut v, _) = fixture();
        for seq in 1..=11 {
            let mut frame: Frame = serde_json::from_str(&good).unwrap();
            frame.seq = seq;
            frame.mac = v
                .tag(&frame)
                .unwrap()
                .finalize()
                .into_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            let result = v.accept_sdp(&serde_json::to_string(&frame).unwrap(), &g, 1000, false);
            if seq <= 10 {
                assert!(result.is_ok());
            } else {
                assert!(matches!(result, Err(ref error) if error == "PROBE_AUTH_RATE"));
            }
        }
        let (g, mut v, _) = fixture();
        assert!(v
            .accept_sdp(&"x".repeat(96 * 1024 + 1), &g, 1000, false)
            .is_err());
    }
}
