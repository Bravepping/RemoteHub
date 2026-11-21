//! Ephemeral operator-approved experiment, never a persistent device identity.
use hmac::{Hmac, Mac};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use zeroize::Zeroize;

pub const MAX_FRAME: usize = 96 * 1024;
pub struct PairKey([u8; 32]);
impl Drop for PairKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
impl PairKey {
    pub fn generate() -> Result<Self, String> {
        let mut bytes = [0; 32];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| "RANDOM_UNAVAILABLE")?;
        Ok(Self(bytes))
    }
    pub fn parse(code: &str) -> Result<Self, String> {
        if code.len() != 64 || !code.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("PAIRING_CODE_INVALID".into());
        }
        let mut bytes = [0; 32];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = u8::from_str_radix(&code[i * 2..i * 2 + 2], 16)
                .map_err(|_| "PAIRING_CODE_INVALID")?;
        }
        Ok(Self(bytes))
    }
    pub fn display(&self) -> String {
        hex(&self.0)
    }
    pub fn room(&self) -> String {
        hex(&Sha256::digest(self.0))
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Sender,
    Receiver,
}
impl Role {
    pub fn direction(self) -> &'static str {
        match self {
            Self::Sender => "sender-to-receiver",
            Self::Receiver => "receiver-to-sender",
        }
    }
    pub fn opposite(self) -> Self {
        match self {
            Self::Sender => Self::Receiver,
            Self::Receiver => Self::Sender,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub version: u8,
    pub role: Role,
    pub room: String,
    pub seconds: u32,
}
impl Hello {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || !(5..=30).contains(&self.seconds)
            || self.room.len() != 64
            || !self.room.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("HELLO_INVALID".into());
        }
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: String,
    pub credential: String,
}
impl Drop for IceServer {
    fn drop(&mut self) {
        self.username.zeroize();
        self.credential.zeroize();
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ready {
    pub session_id: String,
    pub nonce: String,
    pub expires_at: u64,
    pub ice_servers: Vec<IceServer>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerEvent {
    Waiting,
    Ready(Ready),
    Failed { code: String },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub version: u8,
    pub session: String,
    pub direction: String,
    pub nonce: String,
    pub seq: u64,
    pub expires_at: u64,
    pub kind: String,
    pub payload: String,
    pub mac: String,
}
#[derive(Default)]
pub struct Sequence {
    last: u64,
    window: u64,
    count: u32,
}
impl Sequence {
    pub fn check(
        &mut self,
        f: &Frame,
        session: &str,
        nonce: &str,
        role: Role,
        expires: u64,
        now: u64,
    ) -> Result<(), String> {
        if now >= expires
            || f.expires_at != expires
            || f.session != session
            || f.nonce != nonce
            || f.version != 1
            || f.direction != role.direction()
            || f.seq != self.last + 1
            || f.seq > 256
            || f.payload.len() > 64 * 1024
            || f.mac.len() != 64
            || !f.mac.bytes().all(|b| b.is_ascii_hexdigit())
            || !["offer", "answer", "beat", "metrics", "stop"].contains(&f.kind.as_str())
        {
            return Err("RELAY_SCOPE_OR_REPLAY".into());
        }
        if self.window != now {
            self.window = now;
            self.count = 0;
        }
        self.count += 1;
        if self.count > 10 {
            return Err("RELAY_RATE".into());
        }
        self.last = f.seq;
        Ok(())
    }
}
pub struct Codec {
    key: Arc<PairKey>,
    session: String,
    nonce: String,
    expires: u64,
    role: Role,
    tx: u64,
    rx: Sequence,
    failed: bool,
}
impl Codec {
    pub fn new(key: Arc<PairKey>, ready: &Ready, role: Role, now: u64) -> Result<Self, String> {
        if !uuid::Uuid::parse_str(&ready.session_id).is_ok_and(|x| !x.is_nil())
            || !uuid::Uuid::parse_str(&ready.nonce).is_ok_and(|x| !x.is_nil())
            || !(60..=600).contains(&ready.expires_at.saturating_sub(now))
        {
            return Err("RELAY_CONTEXT_INVALID".into());
        }
        Ok(Self {
            key,
            session: ready.session_id.clone(),
            nonce: ready.nonce.clone(),
            expires: ready.expires_at,
            role,
            tx: 0,
            rx: Sequence::default(),
            failed: false,
        })
    }
    fn tag(&self, f: &Frame) -> Result<Hmac<Sha256>, String> {
        let bytes = serde_json::to_vec(&(
            "RemoteHub.Relay.v1",
            f.version,
            &f.session,
            &f.direction,
            &f.nonce,
            f.seq,
            f.expires_at,
            &f.kind,
            &f.payload,
        ))
        .map_err(|_| "RELAY_ENCODING")?;
        let mut m = Hmac::<Sha256>::new_from_slice(&self.key.0).map_err(|_| "RELAY_KEY")?;
        m.update(&bytes);
        Ok(m)
    }
    pub fn encode(&mut self, kind: &str, payload: String, now: u64) -> Result<String, String> {
        if self.failed
            || now >= self.expires
            || self.tx >= 256
            || payload.len() > 64 * 1024
            || !["offer", "answer", "beat", "metrics", "stop"].contains(&kind)
        {
            return Err("RELAY_SEND_NOT_ALLOWED".into());
        }
        self.tx += 1;
        let mut f = Frame {
            version: 1,
            session: self.session.clone(),
            direction: self.role.direction().into(),
            nonce: self.nonce.clone(),
            seq: self.tx,
            expires_at: self.expires,
            kind: kind.into(),
            payload,
            mac: String::new(),
        };
        f.mac = hex(&self.tag(&f)?.finalize().into_bytes());
        let text = serde_json::to_string(&f).map_err(|_| "RELAY_ENCODING")?;
        if text.len() > MAX_FRAME {
            self.failed = true;
            return Err("RELAY_FRAME_SIZE".into());
        }
        Ok(text)
    }
    pub fn accept(&mut self, text: &str, now: u64) -> Result<(String, String), String> {
        let result = self.checked(text, now);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn checked(&mut self, text: &str, now: u64) -> Result<(String, String), String> {
        if self.failed || text.len() > MAX_FRAME {
            return Err("RELAY_FRAME_REJECTED".into());
        }
        let f: Frame = serde_json::from_str(text).map_err(|_| "RELAY_FRAME_SCHEMA")?;
        self.rx.check(
            &f,
            &self.session,
            &self.nonce,
            self.role.opposite(),
            self.expires,
            now,
        )?;
        let mut tag = [0; 32];
        for (i, b) in tag.iter_mut().enumerate() {
            *b = u8::from_str_radix(&f.mac[i * 2..i * 2 + 2], 16).map_err(|_| "RELAY_MAC")?;
        }
        self.tag(&f)?.verify_slice(&tag).map_err(|_| "RELAY_MAC")?;
        Ok((f.kind, f.payload))
    }
}
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |v| v.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> Ready {
        Ready {
            session_id: uuid::Uuid::new_v4().to_string(),
            nonce: uuid::Uuid::new_v4().to_string(),
            expires_at: 1300,
            ice_servers: vec![],
        }
    }
    #[test]
    fn peer_mac_replay_tamper_and_context() {
        let k = Arc::new(PairKey::generate().unwrap());
        let r = ready();
        let mut a = Codec::new(k.clone(), &r, Role::Sender, 1000).unwrap();
        let mut b = Codec::new(k.clone(), &r, Role::Receiver, 1000).unwrap();
        let text = a.encode("offer", "sdp".into(), 1000).unwrap();
        assert_eq!(b.accept(&text, 1000).unwrap().0, "offer");
        assert!(b.accept(&text, 1000).is_err());
        for bad in [
            text.replace("sdp", "tampered"),
            text.replace("sender-to-receiver", "receiver-to-sender"),
        ] {
            let mut b = Codec::new(k.clone(), &r, Role::Receiver, 1000).unwrap();
            assert!(b.accept(&bad, 1000).is_err());
            assert!(b.accept(&text, 1000).is_err());
        }
        assert!(PairKey::parse("not-a-code").is_err());
        assert_eq!(PairKey::parse(&k.display()).unwrap().room(), k.room());
    }
    #[test]
    fn limits_expiry_and_strict_fields() {
        let k = Arc::new(PairKey::generate().unwrap());
        let r = ready();
        let mut a = Codec::new(k.clone(), &r, Role::Sender, 1000).unwrap();
        let mut b = Codec::new(k, &r, Role::Receiver, 1000).unwrap();
        for i in 0..11 {
            let f = a.encode("beat", String::new(), 1000).unwrap();
            assert_eq!(b.accept(&f, 1000).is_ok(), i < 10);
        }
        assert!(a.encode("offer", "x".repeat(65537), 1000).is_err());
        assert!(a.encode("beat", String::new(), 1300).is_err());
        assert!(a.encode("mouse_move_sim", String::new(), 1000).is_err());
    }
    #[test]
    fn rejects_changed_context_unknown_fields_oversize_and_post_stop_replay() {
        let k = Arc::new(PairKey::generate().unwrap());
        let r = ready();
        let mut a = Codec::new(k.clone(), &r, Role::Sender, 1000).unwrap();
        let good = a.encode("beat", String::new(), 1000).unwrap();
        for bad in [
            good.replace("\"version\":1", "\"version\":2"),
            good.replace(&r.session_id, &uuid::Uuid::new_v4().to_string()),
            good.replace(&r.nonce, &uuid::Uuid::new_v4().to_string()),
            good.replacen('{', "{\"extra\":true,", 1),
            "x".repeat(MAX_FRAME + 1),
        ] {
            let mut b = Codec::new(k.clone(), &r, Role::Receiver, 1000).unwrap();
            assert!(b.accept(&bad, 1000).is_err());
            assert!(b.accept(&good, 1000).is_err());
        }
        let mut b = Codec::new(k.clone(), &r, Role::Receiver, 1000).unwrap();
        assert!(b.accept(&good, 1300).is_err());
        let mut oversized = Codec::new(k, &r, Role::Sender, 1000).unwrap();
        assert!(oversized
            .encode("offer", "\"".repeat(64 * 1024), 1000)
            .is_err());
    }
}
