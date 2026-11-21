//! Bounded authenticated signaling only. The secret is never serialized or transmitted.
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 96 * 1024;
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
pub struct Codec {
    key: std::sync::Arc<Key>,
    pub nonce: String,
    tx: u64,
    rx: u64,
    host: bool,
}
impl Codec {
    pub fn new(key: std::sync::Arc<Key>, nonce: String, host: bool) -> Result<Self, String> {
        uuid::Uuid::parse_str(&nonce).map_err(|_| "INVALID_NONCE")?;
        Ok(Self {
            key,
            nonce,
            tx: 0,
            rx: 0,
            host,
        })
    }
    fn tag(&self, frame: &Frame) -> Result<Hmac<Sha256>, String> {
        let bytes = serde_json::to_vec(&(
            "RemoteHub.LAN.v1",
            frame.version,
            &frame.direction,
            &frame.nonce,
            frame.seq,
            &frame.kind,
            &frame.payload,
        ))
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
            version: 1,
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
        self.tx += 1;
        Ok(frame)
    }
    pub fn accept(&mut self, frame: Frame) -> Result<(String, String), String> {
        if frame.version != 1
            || frame.nonce != self.nonce
            || frame.seq != self.rx
            || frame.direction != if self.host { "B-to-A" } else { "A-to-B" }
            || frame.tag.len() != 64
            || !frame.tag.bytes().all(|b| b.is_ascii_hexdigit())
            || frame.payload.len() > 64 * 1024
        {
            return Err("AUTH_OR_REPLAY_REJECTED".into());
        }
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
