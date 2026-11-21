//! Server-only coturn REST issuer. This module exposes no HTTP/WSS listener.
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use uuid::Uuid;
use zeroize::Zeroizing;

pub const DEFAULT_TTL: u64 = 300;
pub const MIN_TTL: u64 = 60;
pub const MAX_TTL: u64 = 600;

/// Never serialize or Debug shared TURN secrets or returned bearer credentials.
pub struct Issuer {
    secret: Zeroizing<Vec<u8>>,
    realm: String,
}
pub struct Ticket {
    username: Zeroizing<String>,
    credential: Zeroizing<String>,
    pub expires_at: u64,
}
impl Ticket {
    pub fn username(&self) -> &str {
        &self.username
    }
    pub fn credential(&self) -> &str {
        &self.credential
    }
}
impl std::fmt::Debug for Issuer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Issuer { secret: [REDACTED], realm: [REDACTED] }")
    }
}
impl std::fmt::Debug for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ticket")
            .field("credential", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}
#[derive(Clone, Copy)]
pub enum Role {
    Controller,
    Host,
}
impl Role {
    fn index(self) -> usize {
        match self {
            Self::Controller => 0,
            Self::Host => 1,
        }
    }
}

/// Trusted server authentication boundary, NOT a DTO accepting client-supplied booleans.
/// A future WSS authentication layer must construct this only after verifying session MACs.
pub struct ApprovedSession {
    id: Uuid,
    expires_at: u64,
    approved: [bool; 2],
    revoked: bool,
    last_seq: [u64; 2],
    issued: [u8; 2],
}
impl ApprovedSession {
    pub fn from_authenticated_context(
        authenticated: bool,
        id: &str,
        expires_at: u64,
    ) -> Result<Self, &'static str> {
        if !authenticated {
            return Err("TURN_SESSION_UNAUTHENTICATED");
        }
        let id = Uuid::parse_str(id).map_err(|_| "TURN_SESSION_INVALID")?;
        if id.is_nil() {
            return Err("TURN_SESSION_INVALID");
        }
        Ok(Self {
            id,
            expires_at,
            approved: [false; 2],
            revoked: false,
            last_seq: [0; 2],
            issued: [0; 2],
        })
    }
    /// Trusted manual-approval callback; this is not an unprotected remote endpoint.
    pub fn set_manual_approval(&mut self, role: Role, approved: bool) {
        self.approved[role.index()] = approved;
    }
    pub fn revoke(&mut self) {
        self.revoked = true;
        self.approved = [false; 2];
    }
}
pub struct Request<'a> {
    pub session_id: &'a str,
    pub realm: &'a str,
    pub role: Role,
    pub seq: u64,
    pub issued_at: u64,
    pub ttl: u64,
}
impl Issuer {
    pub fn from_server_environment(realm: String) -> Result<Self, &'static str> {
        let secret = Zeroizing::new(
            std::env::var("REMOTEHUB_TURN_SHARED_SECRET").map_err(|_| "TURN_SECRET_UNAVAILABLE")?,
        );
        Self::new(secret.as_bytes().to_vec(), realm)
    }
    pub fn new(secret: Vec<u8>, realm: String) -> Result<Self, &'static str> {
        let secret = Zeroizing::new(secret);
        if !(32..=128).contains(&secret.len())
            || !secret
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b"+/_=-".contains(b))
        {
            return Err("TURN_SECRET_INVALID");
        }
        if realm.is_empty()
            || realm.len() > 128
            || !realm
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        {
            return Err("TURN_REALM_INVALID");
        }
        Ok(Self { secret, realm })
    }
    pub fn issue(
        &self,
        session: &mut ApprovedSession,
        request: Request<'_>,
        now: u64,
    ) -> Result<Ticket, &'static str> {
        if session.revoked || !session.approved.iter().all(|v| *v) || now >= session.expires_at {
            return Err("TURN_SESSION_NOT_APPROVED");
        }
        if Uuid::parse_str(request.session_id).map_err(|_| "TURN_SESSION_INVALID")? != session.id {
            return Err("TURN_SESSION_MISMATCH");
        }
        if request.realm != self.realm {
            return Err("TURN_REALM_MISMATCH");
        }
        if now.abs_diff(request.issued_at) > 30 {
            return Err("TURN_CLOCK_SKEW");
        }
        if !(MIN_TTL..=MAX_TTL).contains(&request.ttl) {
            return Err("TURN_TTL_RANGE");
        }
        let expires = now.checked_add(request.ttl).ok_or("TURN_TIME_OVERFLOW")?;
        if expires > session.expires_at {
            return Err("TURN_TICKET_OUTLIVES_SESSION");
        }
        let index = request.role.index();
        if request.seq != session.last_seq[index] + 1 || session.issued[index] >= 2 {
            return Err("TURN_ISSUANCE_REPLAY_OR_QUOTA");
        }
        let username = Zeroizing::new(format!("{expires}:{}", session.id));
        let credential = Zeroizing::new(password(&self.secret, &username));
        session.last_seq[index] = request.seq;
        session.issued[index] += 1;
        Ok(Ticket {
            username,
            credential,
            expires_at: expires,
        })
    }
}
fn password(secret: &[u8], username: &str) -> String {
    let mut mac = Hmac::<Sha1>::new_from_slice(secret).expect("HMAC accepts arbitrary key lengths");
    mac.update(username.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    fn issuer() -> Issuer {
        Issuer::new(
            b"0123456789abcdef0123456789abcdef".to_vec(),
            "unit.invalid".into(),
        )
        .unwrap()
    }
    fn session() -> ApprovedSession {
        let mut s = ApprovedSession::from_authenticated_context(true, ID, 2000).unwrap();
        s.set_manual_approval(Role::Host, true);
        s.set_manual_approval(Role::Controller, true);
        s
    }
    fn request() -> Request<'static> {
        Request {
            session_id: ID,
            realm: "unit.invalid",
            role: Role::Controller,
            seq: 1,
            issued_at: 1000,
            ttl: 300,
        }
    }
    #[test]
    fn rfc2202_known_hmac_vector_and_coturn_timestamp_username() {
        assert_eq!(
            password(&[0x0b; 20], "Hi There"),
            "thcxhlUFcmTii8C2+zeMjvFGvgA="
        );
        let t = issuer().issue(&mut session(), request(), 1000).unwrap();
        assert_eq!(t.username(), format!("1300:{ID}"));
        assert_eq!(t.expires_at, 1300);
        // Independent Python standard-library HMAC/base64 fixture, not runtime-generated expectation.
        assert_eq!(t.credential(), "0ldqFwNKnG3DNOQTbQ9ED2WAfr8=");
    }
    #[test]
    fn unauthenticated_unapproved_expired_revoked_and_cross_session_rejected() {
        assert!(ApprovedSession::from_authenticated_context(false, ID, 2000).is_err());
        let mut no = ApprovedSession::from_authenticated_context(true, ID, 2000).unwrap();
        assert!(issuer().issue(&mut no, request(), 1000).is_err());
        no.set_manual_approval(Role::Host, true);
        assert!(issuer().issue(&mut no, request(), 1000).is_err());
        let mut expired = session();
        expired.expires_at = 1000;
        assert!(issuer().issue(&mut expired, request(), 1000).is_err());
        let mut revoked = session();
        revoked.revoke();
        assert!(issuer().issue(&mut revoked, request(), 1000).is_err());
        let mut wrong = request();
        wrong.session_id = "00000000-0000-4000-8000-000000000002";
        assert!(issuer().issue(&mut session(), wrong, 1000).is_err());
    }
    #[test]
    fn ttl_clock_realm_replay_and_quota_bounds() {
        for ttl in [0, 59, 601, u64::MAX] {
            let mut r = request();
            r.ttl = ttl;
            assert!(issuer().issue(&mut session(), r, 1000).is_err());
        }
        for time in [969, 1031] {
            let mut r = request();
            r.issued_at = time;
            assert!(issuer().issue(&mut session(), r, 1000).is_err());
        }
        for time in [970, 1030] {
            let mut r = request();
            r.issued_at = time;
            assert!(issuer().issue(&mut session(), r, 1000).is_ok());
        }
        let mut r = request();
        r.realm = "other.invalid";
        assert!(issuer().issue(&mut session(), r, 1000).is_err());
        let mut short = session();
        short.expires_at = 1299;
        assert!(issuer().issue(&mut short, request(), 1000).is_err());
        let i = issuer();
        let mut s = session();
        assert!(i.issue(&mut s, request(), 1000).is_ok());
        assert!(i.issue(&mut s, request(), 1000).is_err());
        let mut r = request();
        r.seq = 2;
        assert!(i.issue(&mut s, r, 1000).is_ok());
        let mut r = request();
        r.seq = 3;
        assert!(i.issue(&mut s, r, 1000).is_err());
    }
    #[test]
    fn secret_validation_and_debug_are_redacted() {
        assert!(Issuer::new(vec![b'a'; 31], "unit.invalid".into()).is_err());
        assert!(Issuer::new(vec![b'a'; 32], "bad realm".into()).is_err());
        let i = issuer();
        let t = i.issue(&mut session(), request(), 1000).unwrap();
        let log = format!("{i:?} {t:?}");
        assert!(!log.contains(t.credential()));
        assert!(!log.contains("0123456789abcdef"));
        assert!(!log.contains(ID));
    }
}
