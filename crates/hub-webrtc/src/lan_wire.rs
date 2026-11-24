//! Bounded authenticated signaling only. The secret is never serialized or transmitted.
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 96 * 1024;
/// Size of the serialized authenticated string inside a V2 relay signal.
pub const MAX_RELAY_FRAME: usize = 32 * 1024;
pub struct Key([u8; 16]);
impl Key {
    pub fn generate() -> Self {
        Self(*uuid::Uuid::new_v4().as_bytes())
    }
    pub fn parse(code: &str) -> Result<Self, String> {
        let compact: String = code
            .chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .collect();
        if compact.len() != 32 || !compact.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("INVALID_PAIRING_CODE".into());
        }
        let mut key = [0u8; 16];
        for (i, item) in key.iter_mut().enumerate() {
            *item = u8::from_str_radix(&compact[i * 2..i * 2 + 2], 16)
                .map_err(|_| "INVALID_PAIRING_CODE")?;
        }
        Ok(Self(key))
    }
    pub fn display(&self) -> String {
        uuid::Uuid::from_bytes(self.0).to_string()
    }
}
#[cfg(all(
    feature = "v2-verified-lan",
    any(test, all(windows, feature = "v2-private-wss-lan"))
))]
impl Key {
    /// A distinct derivation domain; different purposes cannot reuse MACs
    /// from the LAN wire or from another bootstrap field.
    pub(crate) fn expand_wss(
        &self,
        purpose: &'static [u8],
        verified_transcript: &[u8; 32],
    ) -> Result<[u8; 32], String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).map_err(|_| "WIRE_KEY")?;
        mac.update(b"RemoteHub.LAN.device-wss-bootstrap.kdf.v1\0");
        mac.update(purpose);
        mac.update(&[0]);
        mac.update(verified_transcript);
        Ok(mac.finalize().into_bytes().into())
    }

    pub(crate) fn from_wss_material(material: [u8; 32]) -> Self {
        let mut key = [0; 16];
        key.copy_from_slice(&material[..16]);
        Self(key)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    version: u8,
    direction: String,
    pub nonce: String,
    seq: u64,
    pub kind: String,
    pub payload: String,
    tag: String,
}
enum Domain {
    LanV1,
    RelayV2 {
        room: String,
    },
    #[cfg(feature = "v2-public-control")]
    PublicControl {
        room: String,
        immediate_batch: bool,
    },
}

impl Domain {
    fn version(&self) -> u8 {
        match self {
            Self::LanV1 => 1,
            Self::RelayV2 { .. } => 2,
            #[cfg(feature = "v2-public-control")]
            Self::PublicControl { .. } => 2,
        }
    }

    fn check_relay_size(&self, frame: &Frame) -> Result<(), String> {
        if self.version() == 2
            && serde_json::to_vec(frame)
                .map_err(|_| "WIRE_SERIALIZE")?
                .len()
                > MAX_RELAY_FRAME
        {
            return Err("WIRE_TOO_LARGE".into());
        }
        Ok(())
    }
}

pub struct Codec {
    key: std::sync::Arc<Key>,
    domain: Domain,
    pub nonce: String,
    tx: u64,
    rx: u64,
    host: bool,
}
impl Codec {
    pub fn new(key: std::sync::Arc<Key>, nonce: String, host: bool) -> Result<Self, String> {
        Self::with_domain(key, nonce, host, Domain::LanV1)
    }

    /// Bind the authenticated payload to a locally chosen V2 room and session
    /// nonce. Use a fresh pairing key separate from the room and relay bearer.
    /// This authenticates payloads, but does not encrypt them or grant access.
    pub fn new_v2(
        key: std::sync::Arc<Key>,
        nonce: String,
        room: &str,
        host: bool,
    ) -> Result<Self, String> {
        if room.len() != 32
            || !room
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
            return Err("ROOM_FORMAT".into());
        }
        Self::with_domain(key, nonce, host, Domain::RelayV2 { room: room.into() })
    }

    #[cfg(feature = "v2-public-control")]
    pub(crate) fn new_public_control(
        key: std::sync::Arc<Key>,
        nonce: String,
        room: &str,
        host: bool,
        immediate_batch: bool,
    ) -> Result<Self, String> {
        if room.len() != 32
            || !room
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        {
            return Err("ROOM_FORMAT".into());
        }
        Self::with_domain(
            key,
            nonce,
            host,
            Domain::PublicControl {
                room: room.into(),
                immediate_batch,
            },
        )
    }

    fn with_domain(
        key: std::sync::Arc<Key>,
        nonce: String,
        host: bool,
        domain: Domain,
    ) -> Result<Self, String> {
        uuid::Uuid::parse_str(&nonce).map_err(|_| "INVALID_NONCE")?;
        Ok(Self {
            key,
            domain,
            nonce,
            tx: 0,
            rx: 0,
            host,
        })
    }
    fn tag(&self, frame: &Frame) -> Result<Hmac<Sha256>, String> {
        let bytes = match &self.domain {
            // Preserve the LAN v1 authenticated bytes for older builds.
            Domain::LanV1 => serde_json::to_vec(&(
                "RemoteHub.LAN.v1",
                frame.version,
                &frame.direction,
                &frame.nonce,
                frame.seq,
                &frame.kind,
                &frame.payload,
            )),
            Domain::RelayV2 { room } => serde_json::to_vec(&(
                "RemoteHub.Signaling.v2",
                room,
                frame.version,
                &frame.direction,
                &frame.nonce,
                frame.seq,
                &frame.kind,
                &frame.payload,
            )),
            #[cfg(feature = "v2-public-control")]
            Domain::PublicControl {
                room,
                immediate_batch,
            } => serde_json::to_vec(&(
                if *immediate_batch {
                    "RemoteHub.Signaling.public-control.immediate-batch.v1"
                } else {
                    "RemoteHub.Signaling.public-control.v1"
                },
                room,
                frame.version,
                &frame.direction,
                &frame.nonce,
                frame.seq,
                &frame.kind,
                &frame.payload,
            )),
        }
        .map_err(|_| "WIRE_SERIALIZE")?;
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key.0).map_err(|_| "WIRE_KEY")?;
        mac.update(&bytes);
        Ok(mac)
    }
    pub fn encode(&mut self, kind: &str, payload: String) -> Result<Frame, String> {
        if payload.len() > 64 * 1024 {
            return Err("WIRE_TOO_LARGE".into());
        }
        let mut frame = Frame {
            version: self.domain.version(),
            direction: if self.host { "A-to-B" } else { "B-to-A" }.into(),
            nonce: self.nonce.clone(),
            seq: self.tx,
            kind: kind.into(),
            payload,
            tag: String::new(),
        };
        frame.tag = self
            .tag(&frame)?
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.domain.check_relay_size(&frame)?;
        self.tx += 1;
        Ok(frame)
    }
    pub fn accept(&mut self, frame: Frame) -> Result<(String, String), String> {
        if frame.version != self.domain.version()
            || frame.nonce != self.nonce
            || frame.seq != self.rx
            || frame.direction != if self.host { "B-to-A" } else { "A-to-B" }
            || frame.tag.len() != 64
            || !frame.tag.bytes().all(|b| b.is_ascii_hexdigit())
            || frame.payload.len() > 64 * 1024
        {
            return Err("AUTH_OR_REPLAY_REJECTED".into());
        }
        self.domain.check_relay_size(&frame)?;
        let mut tag = [0u8; 32];
        for (i, item) in tag.iter_mut().enumerate() {
            *item = u8::from_str_radix(&frame.tag[i * 2..i * 2 + 2], 16)
                .map_err(|_| "AUTH_REJECTED")?;
        }
        self.tag(&frame)?
            .verify_slice(&tag)
            .map_err(|_| "AUTH_REJECTED")?;
        self.rx += 1;
        Ok((frame.kind, frame.payload))
    }
}
pub async fn read<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Frame, String> {
    let size = reader.read_u32().await.map_err(|_| "SIGNAL_DISCONNECTED")? as usize;
    if size == 0 || size > MAX_FRAME {
        return Err("WIRE_TOO_LARGE".into());
    }
    let mut data = vec![0; size];
    reader
        .read_exact(&mut data)
        .await
        .map_err(|_| "SIGNAL_DISCONNECTED")?;
    serde_json::from_slice(&data).map_err(|_| "WIRE_INVALID_JSON".into())
}
pub async fn write<W: AsyncWrite + Unpin>(writer: &mut W, frame: Frame) -> Result<(), String> {
    let bytes = serde_json::to_vec(&frame).map_err(|_| "WIRE_SERIALIZE")?;
    if bytes.len() > MAX_FRAME {
        return Err("WIRE_TOO_LARGE".into());
    }
    writer
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "SIGNAL_DISCONNECTED")?;
    writer
        .write_all(&bytes)
        .await
        .map_err(|_| "SIGNAL_DISCONNECTED".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[cfg(feature = "v2-public-control")]
    #[test]
    fn product_control_mac_rejects_the_video_domain_even_with_the_same_key_and_room() {
        let key = std::sync::Arc::new(Key::parse(&"ba".repeat(16)).unwrap());
        let mut video_host =
            Codec::new_v2(key.clone(), TEST_NONCE.into(), TEST_ROOM, true).unwrap();
        let mut control_viewer =
            Codec::new_public_control(key.clone(), TEST_NONCE.into(), TEST_ROOM, false, false)
                .unwrap();
        let video = video_host
            .encode("host_challenge", "fixed proof".into())
            .unwrap();
        assert!(control_viewer.accept(video).is_err());

        let mut control_host =
            Codec::new_public_control(key.clone(), TEST_NONCE.into(), TEST_ROOM, true, false)
                .unwrap();
        let mut batch_viewer =
            Codec::new_public_control(key.clone(), TEST_NONCE.into(), TEST_ROOM, false, true)
                .unwrap();
        let mut batch_host =
            Codec::new_public_control(key.clone(), TEST_NONCE.into(), TEST_ROOM, true, true)
                .unwrap();
        let mut video_viewer = Codec::new_v2(key, TEST_NONCE.into(), TEST_ROOM, false).unwrap();
        let control = control_host
            .encode("host_challenge", "fixed proof".into())
            .unwrap();
        let copy = serde_json::from_slice(&serde_json::to_vec(&control).unwrap()).unwrap();
        let batch_copy = serde_json::from_slice(&serde_json::to_vec(&control).unwrap()).unwrap();
        assert!(batch_viewer.accept(batch_copy).is_err());
        let batch = batch_host
            .encode("host_challenge", "fixed proof".into())
            .unwrap();
        let control_copy = serde_json::from_slice(&serde_json::to_vec(&batch).unwrap()).unwrap();
        assert!(control_viewer.accept(control_copy).is_err());
        assert!(batch_viewer.accept(batch).is_ok());
        assert!(video_viewer.accept(copy).is_err());
        assert_eq!(
            control_viewer.accept(control).unwrap(),
            ("host_challenge".to_owned(), "fixed proof".to_owned())
        );
    }

    const TEST_ROOM: &str = "00112233445566778899aabbccddeeff";
    const OTHER_ROOM: &str = "00112233445566778899aabbccddeefe";
    const TEST_NONCE: &str = "12345678-1234-4234-8234-123456789abc";

    #[test]
    fn lan_v1_frame_remains_byte_for_byte_compatible() {
        const GOLDEN: &str = concat!(
            r#"{"version":1,"direction":"B-to-A","nonce":"12345678-1234-4234-8234-123456789abc","#,
            r#""seq":0,"kind":"offer","payload":"test-sdp","tag":"#,
            r#""eaed2e25be1f88bf8145864de6e2488acf72415baa5ba72b4000ebb593a22eed"}"#,
        );
        let key = Arc::new(Key::parse(TEST_ROOM).unwrap());
        let mut client = Codec::new(key.clone(), TEST_NONCE.into(), false).unwrap();
        let encoded = client.encode("offer", "test-sdp".into()).unwrap();
        assert_eq!(serde_json::to_string(&encoded).unwrap(), GOLDEN);
        let mut host = Codec::new(key, TEST_NONCE.into(), true).unwrap();
        assert_eq!(
            host.accept(serde_json::from_str(GOLDEN).unwrap()).unwrap(),
            ("offer".into(), "test-sdp".into())
        );
    }

    #[test]
    fn relay_rejects_other_rooms_sessions_and_transport_domains() {
        let key = Arc::new(Key::generate());
        let mut client = Codec::new_v2(key.clone(), TEST_NONCE.into(), TEST_ROOM, false).unwrap();
        let mut host = Codec::new_v2(key.clone(), TEST_NONCE.into(), TEST_ROOM, true).unwrap();
        let mut other_room =
            Codec::new_v2(key.clone(), TEST_NONCE.into(), OTHER_ROOM, true).unwrap();
        let mut other_nonce = Codec::new_v2(
            key.clone(),
            uuid::Uuid::new_v4().to_string(),
            TEST_ROOM,
            true,
        )
        .unwrap();
        let mut lan_client = Codec::new(key.clone(), TEST_NONCE.into(), false).unwrap();
        let mut lan_host = Codec::new(key, TEST_NONCE.into(), true).unwrap();
        let relay =
            serde_json::to_string(&client.encode("offer", "test-sdp".into()).unwrap()).unwrap();

        let decode = || serde_json::from_str::<Frame>(&relay).unwrap();
        assert_eq!(other_room.accept(decode()).unwrap_err(), "AUTH_REJECTED");
        assert!(other_nonce.accept(decode()).is_err());
        assert!(lan_host.accept(decode()).is_err());
        let mut downgrade = decode();
        downgrade.version = 1;
        assert_eq!(lan_host.accept(downgrade).unwrap_err(), "AUTH_REJECTED");

        let mut upgrade = lan_client.encode("offer", "test-sdp".into()).unwrap();
        upgrade.version = 2;
        assert_eq!(host.accept(upgrade).unwrap_err(), "AUTH_REJECTED");
        assert_eq!(
            host.accept(decode()).unwrap(),
            ("offer".into(), "test-sdp".into())
        );
    }

    #[test]
    fn relay_authenticates_kind_payload_direction_and_sequence_before_advancing() {
        let key = Arc::new(Key::generate());
        let mut client = Codec::new_v2(key.clone(), TEST_NONCE.into(), TEST_ROOM, false).unwrap();
        let mut host = Codec::new_v2(key, TEST_NONCE.into(), TEST_ROOM, true).unwrap();
        let offer = serde_json::to_string(&client.encode("offer", "sdp".into()).unwrap()).unwrap();
        let ice =
            serde_json::to_string(&client.encode("ice", "candidate".into()).unwrap()).unwrap();
        let make_offer = || serde_json::from_str::<Frame>(&offer).unwrap();
        let make_ice = || serde_json::from_str::<Frame>(&ice).unwrap();
        let mut kind = make_offer();
        kind.kind = "ice".into();
        let mut payload = make_offer();
        payload.payload.push('!');
        assert!(client.accept(make_offer()).is_err());
        assert!(host.accept(make_ice()).is_err());
        assert!(host.accept(kind).is_err());
        assert!(host.accept(payload).is_err());
        assert_eq!(
            host.accept(make_offer()).unwrap(),
            ("offer".into(), "sdp".into())
        );
        assert!(host.accept(make_offer()).is_err());
        assert_eq!(
            host.accept(make_ice()).unwrap(),
            ("ice".into(), "candidate".into())
        );
        let answer = host.encode("answer", "reply".into()).unwrap();
        assert_eq!(
            client.accept(answer).unwrap(),
            ("answer".into(), "reply".into())
        );
    }

    #[test]
    fn relay_rejects_invalid_rooms_and_oversized_serialized_payloads() {
        let key = Arc::new(Key::generate());
        for room in [
            "",
            &TEST_ROOM[1..],
            "00112233445566778899AABBCCDDEEFF",
            "00112233445566778899aabbccddeefg",
        ] {
            assert_eq!(
                Codec::new_v2(key.clone(), TEST_NONCE.into(), room, false)
                    .err()
                    .as_deref(),
                Some("ROOM_FORMAT")
            );
        }

        let mut client = Codec::new_v2(key.clone(), TEST_NONCE.into(), TEST_ROOM, false).unwrap();
        let mut host = Codec::new_v2(key, TEST_NONCE.into(), TEST_ROOM, true).unwrap();
        assert_eq!(
            client
                .encode("offer", "\"".repeat(MAX_RELAY_FRAME / 2))
                .err()
                .as_deref(),
            Some("WIRE_TOO_LARGE")
        );
        let small = client.encode("offer", "fixture".into()).unwrap();
        assert_eq!(small.seq, 0);
        let mut oversized: Frame =
            serde_json::from_slice(&serde_json::to_vec(&small).unwrap()).unwrap();
        oversized.payload = "\"".repeat(MAX_RELAY_FRAME / 2);
        assert_eq!(host.accept(oversized).unwrap_err(), "WIRE_TOO_LARGE");
        assert!(host.accept(small).is_ok());
    }

    #[test]
    fn mac_rejects_tamper_replay_reflection_and_wrong_code() {
        let key = std::sync::Arc::new(Key::generate());
        let nonce = uuid::Uuid::new_v4().to_string();
        let mut client = Codec::new(key.clone(), nonce.clone(), false).unwrap();
        let mut host = Codec::new(key.clone(), nonce.clone(), true).unwrap();
        let good = client.encode("offer", "test-sdp".into()).unwrap();
        let bytes = serde_json::to_vec(&good).unwrap();
        let mut tampered: Frame = serde_json::from_slice(&bytes).unwrap();
        tampered.payload.push('x');
        assert!(host.accept(tampered).is_err());
        assert!(host.accept(good).is_ok());
        assert!(host
            .accept(serde_json::from_slice(&bytes).unwrap())
            .is_err());
        assert!(client
            .accept(serde_json::from_slice(&bytes).unwrap())
            .is_err());
        let mut wrong = Codec::new(std::sync::Arc::new(Key::generate()), nonce, true).unwrap();
        assert!(wrong
            .accept(serde_json::from_slice(&bytes).unwrap())
            .is_err());
        assert!(Key::parse("bad-code").is_err());
        assert!(Key::parse(&"é".repeat(16)).is_err());
        assert!(Key::parse(&key.display()).is_ok());
    }
    #[tokio::test]
    async fn oversized_prefix_fails_before_allocation() {
        let (mut tx, mut rx) = tokio::io::duplex(16);
        tx.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
        assert!(read(&mut rx).await.is_err());
    }
}
