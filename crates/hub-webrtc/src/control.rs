//! control.v1 validates simulated events only. No platform input APIs are used.
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
pub const LABEL: &str = "control.v1";
pub const MAX_BYTES: usize = 1024;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrowserStats {
    pub state: String,
    pub sent_moves: u32,
    pub ack_received: u32,
    pub pings_sent: u32,
    pub pongs_received: u32,
    pub pending: u32,
    pub last_tx_seq: u64,
    pub last_rx_seq: u64,
    pub anomalies: u32,
    pub open_events: u32,
    pub message_events: u32,
    pub close_events: u32,
    pub error_events: u32,
    pub send_closed: bool,
    pub last_error: Option<String>,
}
impl BrowserStats {
    pub fn validate(&self) -> Result<(), String> {
        if !["connecting", "open", "closing", "closed", "failed"].contains(&self.state.as_str())
            || [
                self.sent_moves,
                self.ack_received,
                self.pings_sent,
                self.pongs_received,
                self.pending,
                self.anomalies,
                self.open_events,
                self.message_events,
                self.close_events,
                self.error_events,
            ]
            .iter()
            .any(|n| *n > 2000)
            || self.last_tx_seq > 1000
            || self.last_rx_seq > 1000
            || self.last_error.as_ref().is_some_and(|s| s.len() > 128)
        {
            return Err("CONTROL_METRICS_INVALID".into());
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Message {
    #[serde(rename = "mouse_move_sim")]
    Move {
        v: u8,
        seq: u64,
        session_id: String,
        x_norm: f64,
        y_norm: f64,
    },
    #[serde(rename = "ping")]
    Ping { v: u8, seq: u64, session_id: String },
    #[serde(rename = "pong")]
    Pong {
        v: u8,
        seq: u64,
        session_id: String,
        reply_to: u64,
    },
    #[serde(rename = "ack")]
    Ack {
        v: u8,
        seq: u64,
        session_id: String,
        ack_seq: u64,
    },
}
impl Message {
    pub fn header(&self) -> (u8, u64, &str) {
        match self {
            Self::Move {
                v, seq, session_id, ..
            }
            | Self::Ping { v, seq, session_id }
            | Self::Pong {
                v, seq, session_id, ..
            }
            | Self::Ack {
                v, seq, session_id, ..
            } => (*v, *seq, session_id),
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlStats {
    pub state: String,
    pub open_events: u32,
    pub message_events: u32,
    pub close_events: u32,
    pub error_events: u32,
    pub accepted_moves: u32,
    pub acks_sent: u32,
    pub pings_received: u32,
    pub pongs_sent: u32,
    pub last_rx_seq: u64,
    pub last_tx_seq: u64,
    pub rejected: u32,
    pub close_requested: bool,
    pub task_joined: bool,
    pub last_error: Option<String>,
}
pub struct Protocol {
    session: String,
    expires: Instant,
    window: Instant,
    window_count: u32,
    pub stats: ControlStats,
}
impl Protocol {
    pub fn new(session: String, lifetime: Duration) -> Result<Self, String> {
        uuid::Uuid::parse_str(&session).map_err(|_| "CONTROL_SESSION_INVALID")?;
        Ok(Self {
            session,
            expires: Instant::now() + lifetime,
            window: Instant::now(),
            window_count: 0,
            stats: ControlStats {
                state: "waiting".into(),
                ..Default::default()
            },
        })
    }
    pub fn process(
        &mut self,
        bytes: &[u8],
        text: bool,
        permitted: bool,
        now: Instant,
    ) -> Result<Message, String> {
        let result = self.checked(bytes, text, permitted, now);
        if let Err(error) = &result {
            self.stats.rejected += 1;
            self.stats.last_error = Some(error.clone());
        }
        result
    }
    fn checked(
        &mut self,
        bytes: &[u8],
        text: bool,
        permitted: bool,
        now: Instant,
    ) -> Result<Message, String> {
        if !permitted || now >= self.expires {
            return Err("CONTROL_NOT_AUTHORIZED".into());
        }
        if !text {
            return Err("CONTROL_TEXT_REQUIRED".into());
        }
        if bytes.is_empty() || bytes.len() > MAX_BYTES {
            return Err("CONTROL_MESSAGE_SIZE".into());
        }
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.window_count = 0;
        }
        self.window_count += 1;
        if self.window_count > 60 || self.stats.last_rx_seq >= 1000 {
            return Err("CONTROL_RATE_LIMIT".into());
        }
        let message: Message = serde_json::from_slice(bytes).map_err(|_| "CONTROL_SCHEMA")?;
        let (v, seq, session) = message.header();
        if v != 1 {
            return Err("CONTROL_VERSION".into());
        }
        if session != self.session {
            return Err("CONTROL_SESSION_MISMATCH".into());
        }
        if seq != self.stats.last_rx_seq + 1 || seq > 1000 {
            return Err("CONTROL_SEQUENCE".into());
        }
        let tx = self.stats.last_tx_seq + 1;
        let response = match message {
            Message::Move { x_norm, y_norm, .. } => {
                if !x_norm.is_finite()
                    || !y_norm.is_finite()
                    || !(0.0..=1.0).contains(&x_norm)
                    || !(0.0..=1.0).contains(&y_norm)
                {
                    return Err("CONTROL_COORDINATE".into());
                }
                self.stats.accepted_moves += 1;
                Message::Ack {
                    v: 1,
                    seq: tx,
                    session_id: self.session.clone(),
                    ack_seq: seq,
                }
            }
            Message::Ping { .. } => {
                self.stats.pings_received += 1;
                Message::Pong {
                    v: 1,
                    seq: tx,
                    session_id: self.session.clone(),
                    reply_to: seq,
                }
            }
            _ => return Err("CONTROL_DIRECTION".into()),
        };
        self.stats.last_rx_seq = seq;
        Ok(response)
    }
    pub fn sent(&mut self, message: &Message) {
        self.stats.last_tx_seq = message.header().1;
        match message {
            Message::Ack { .. } => self.stats.acks_sent += 1,
            Message::Pong { .. } => self.stats.pongs_sent += 1,
            _ => (),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    fn event(seq: u64) -> Vec<u8> {
        serde_json::to_vec(&Message::Move {
            v: 1,
            seq,
            session_id: ID.into(),
            x_norm: 0.3,
            y_norm: 0.7,
        })
        .unwrap()
    }
    #[test]
    fn hundred_simulated_events_have_contiguous_acks() {
        let mut p = Protocol::new(ID.into(), Duration::from_secs(30)).unwrap();
        let start = Instant::now();
        for seq in 1..=100 {
            let response = p
                .process(
                    &event(seq),
                    true,
                    true,
                    start + Duration::from_millis(seq * 50),
                )
                .unwrap();
            assert!(matches!(response,Message::Ack{ack_seq,..}if ack_seq==seq));
            p.sent(&response);
        }
        assert_eq!(p.stats.accepted_moves, 100);
        assert_eq!(p.stats.acks_sent, 100);
        assert_eq!(p.stats.last_rx_seq, 100);
    }
    #[test]
    fn rejects_permissions_expiry_replay_coordinates_schema_size_and_direction() {
        let start = Instant::now();
        let mut p = Protocol::new(ID.into(), Duration::from_secs(5)).unwrap();
        assert_eq!(
            p.process(&event(1), true, false, start).unwrap_err(),
            "CONTROL_NOT_AUTHORIZED"
        );
        assert_eq!(
            p.process(&event(1), true, true, start + Duration::from_secs(6))
                .unwrap_err(),
            "CONTROL_NOT_AUTHORIZED"
        );
        for bad in [
            String::from_utf8(event(1))
                .unwrap()
                .replace(ID, "00000000-0000-4000-8000-000000000002"),
            String::from_utf8(event(1)).unwrap().replace("0.3", "1.3"),
            String::from_utf8(event(1))
                .unwrap()
                .replace("\"v\":1", "\"v\":2"),
            "{\"v\":1,\"kind\":\"SendInput\"}".into(),
            "x".repeat(MAX_BYTES + 1),
        ] {
            assert!(p.process(bad.as_bytes(), true, true, start).is_err());
        }
        assert!(p.process(&event(1), false, true, start).is_err());
        let response = p.process(&event(1), true, true, start).unwrap();
        p.sent(&response);
        assert_eq!(
            p.process(&event(1), true, true, start).unwrap_err(),
            "CONTROL_SEQUENCE"
        );
        assert!(p.process(br#"{"v":1,"v":1,"kind":"ping","seq":2,"session_id":"00000000-0000-4000-8000-000000000001"}"#,true,true,start).is_err());
        let ack = serde_json::to_vec(&Message::Ack {
            v: 1,
            seq: 2,
            session_id: ID.into(),
            ack_seq: 1,
        })
        .unwrap();
        assert_eq!(
            p.process(&ack, true, true, start).unwrap_err(),
            "CONTROL_DIRECTION"
        );
    }
    #[test]
    fn fixed_window_and_total_limits_are_enforced() {
        let mut p = Protocol::new(ID.into(), Duration::from_secs(60)).unwrap();
        let start = Instant::now();
        for seq in 1..=60 {
            let r = p.process(&event(seq), true, true, start).unwrap();
            p.sent(&r);
        }
        assert_eq!(
            p.process(&event(61), true, true, start).unwrap_err(),
            "CONTROL_RATE_LIMIT"
        );
        for seq in 61..=1000 {
            let r = p
                .process(
                    &event(seq),
                    true,
                    true,
                    start + Duration::from_millis(seq * 50),
                )
                .unwrap();
            p.sent(&r);
        }
        assert_eq!(
            p.process(&event(1001), true, true, start + Duration::from_secs(51))
                .unwrap_err(),
            "CONTROL_RATE_LIMIT"
        );
    }
}
