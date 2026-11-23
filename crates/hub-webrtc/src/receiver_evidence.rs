//! Local diagnostic journal. Has no network, grant or input side effects.
use crate::control::BrowserStats;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Write, path::Path};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    PreClose,
    Cleanup,
}
#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Capture {
    Live,
    LastKnown,
    Unavailable,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cleanup {
    pub control_closed: Option<bool>,
    pub peer_closed: Option<bool>,
    pub tracks_stopped: Option<bool>,
    pub native_resources_released: Option<bool>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiverEvidence {
    pub version: u8,
    pub stage: Stage,
    pub capture: Capture,
    pub sampled_at_ms: Option<u64>,
    pub recorded_at_ms: u64,
    pub snapshot: Option<Value>,
    pub errors: Vec<String>,
    pub cleanup: Cleanup,
}
fn safe_code(s: &str) -> bool {
    [
        "RECEIVER_INTERNAL_ERROR",
        "RECEIVER_EVIDENCE_TIMEOUT",
        "RECEIVER_EVIDENCE_SCHEMA",
        "RECEIVER_EVIDENCE_METRICS",
        "RECEIVER_EVIDENCE_SIZE",
        "RECEIVER_EVIDENCE_ENCODING",
        "RECEIVER_EVIDENCE_DIRECTORY",
        "RECEIVER_EVIDENCE_OWNER",
        "RECEIVER_EVIDENCE_STAGE_REPLAY",
        "RECEIVER_EVIDENCE_EXISTS_OR_WRITE",
        "RECEIVER_EVIDENCE_WRITE",
        "RECEIVER_EVIDENCE_WRONG_ROLE",
        "RECEIVER_EVIDENCE_LOCK",
        "BROWSER_PC_MISSING",
        "BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED",
        "BROWSER_SELECTED_RELAY_UNVERIFIED",
        "P006_CLEANUP_PENDING",
        "P006_WSS_CLOSED",
        "P006_SIGNAL_LEASE_EXPIRED",
        "P006_TASK_FAILED",
        "P006_STOPPED",
        "P006_RECEIVER_STOPPED",
        "P006_SIGNAL_CLOSED",
        "REPORT_WRITE",
        "REPORT_EXISTS_OR_WRITE",
        "WINDOW_FINAL_STATS_UNAVAILABLE",
        "DIRECT_SELECTED_PAIR_NOT_APPROVED",
        "CONTROL_CHANNEL_ERROR",
        "CONTROL_RESPONSE_INVALID",
        "CONTROL_SEND_FAILED",
        "CONTROL_SEND_LIMIT",
        "DIAG_GETSTATS_FAILURE",
        "BROWSER_RELAY_CANDIDATE_MISSING",
        "BROWSER_RELAY_GATHER_TIMEOUT",
        "BROWSER_RELAY_SDP_INVALID",
        "BROWSER_RELAY_CANDIDATE_UNVERIFIED",
        "BROWSER_RELAY_PC_CREATE_FAILED",
        "BROWSER_RELAY_OFFER_CREATE_FAILED",
        "BROWSER_RELAY_LOCAL_SDP_APPLY_FAILED",
        "BROWSER_RELAY_ANSWER_APPLY_FAILED",
        "R06_EXPLICIT_MODE_REQUIRED",
        "R06_REQUESTED_MODE_MISMATCH",
        "R06_RELAY_PATH_METRIC_INVALID",
        "R06_RELAY_SDP_INVALID",
        "R06_RELAY_SDP_NOT_REDACTED",
        "R06_RELAY_CANDIDATE_INVALID",
        "R06_SYNTHETIC_8_SECONDS_REQUIRED",
        "R06_INVITATION_WINDOW_EXCEEDS_20_MINUTES",
        "METRICS_INVALID",
        "RELAY_APPROVAL_REQUIRED",
        "RELAY_APPROVAL_EXPIRED",
        "RELAY_CANDIDATE_LIMIT",
        "RELAY_CANDIDATE_REQUIRED",
        "RELAY_GRANT_REQUIRED",
        "ICE_MODE_NOT_ENABLED_IN_P006A",
        "ICE_CONFIG_INVALID",
        "ICE_SERVERS_LIMIT",
        "ICE_SHORT_CREDENTIAL_OR_ENDPOINT_INVALID",
        "SDP_SIZE_OR_ENCODING",
        "H264_BASELINE_UNAVAILABLE",
        "H264_BASELINE_42001F_MODE1_REQUIRED",
        "P006_CONFIG_EXPIRED_OR_INVALID",
        "P006_CONFIG_FILE_POLICY",
        "P006_CONFIG_READ",
        "P006_CONFIG_SCHEMA",
        "P006_INVITATION_ROLE_MISMATCH",
        "P006_PRIVATE_CONFIG_REQUIRED",
        "P006_CONFIG_INVALID",
        "P006_ICE_CONFIG_INVALID",
        "P006_CA_INVALID",
        "P006_WSS_TLS_OR_AUTH_FAILED",
        "P006_WSS_CONNECT_TIMEOUT",
    ]
    .contains(&s)
}
fn nullable_enum(v: &Value, allowed: &[&str]) -> bool {
    v.is_null() || v.as_str().is_some_and(|s| allowed.contains(&s))
}
pub fn validate_snapshot(value: &Value) -> Result<(), String> {
    let object = value.as_object().ok_or("RECEIVER_EVIDENCE_METRICS")?;
    if serde_json::to_vec(value)
        .map_err(|_| "RECEIVER_EVIDENCE_ENCODING")?
        .len()
        > 4096
    {
        return Err("RECEIVER_EVIDENCE_SIZE".into());
    }
    for (key, v) in object {
        let valid = match key.as_str() {
            "elapsedMs" | "currentTime" | "firstFrameMs" | "totalFreezesDuration" => {
                v.is_null()
                    || v.as_f64()
                        .is_some_and(|n| n.is_finite() && (0.0..=1e12).contains(&n))
            }
            "framesDecoded" | "framesReceived" | "packetsReceived" | "bytesReceived"
            | "videoWidth" | "videoHeight" | "framesRendered" | "freezeCount" => {
                v.is_null() || v.as_u64().is_some_and(|n| n <= 9_007_199_254_740_991)
            }
            "packetsLost" => {
                v.is_null()
                    || v.as_i64()
                        .is_some_and(|n| n.unsigned_abs() <= 9_007_199_254_740_991)
            }
            "peerMatches" | "selectedRemoteLoopback" => v.is_null() || v.is_boolean(),
            "connectionState" => nullable_enum(
                v,
                &[
                    "new",
                    "connecting",
                    "connected",
                    "disconnected",
                    "failed",
                    "closed",
                ],
            ),
            "iceState" => nullable_enum(
                v,
                &[
                    "new",
                    "checking",
                    "connected",
                    "completed",
                    "disconnected",
                    "failed",
                    "closed",
                ],
            ),
            "connectionPath" => nullable_enum(v, &["direct", "relay", "unknown"]),
            "localCandidateType" | "remoteCandidateType" => {
                nullable_enum(v, &["host", "srflx", "relay", "prflx"])
            }
            "codec" => nullable_enum(v, &["video/H264", "video/h264"]),
            "decoderImplementation" => v.is_null(),
            "fmtp" => {
                v.is_null()
                    || v.as_str().is_some_and(|s| {
                        s.len() <= 256
                            && s.split(';').all(|p| {
                                let Some((key, value)) = p.trim().split_once('=') else {
                                    return false;
                                };
                                match key {
                                    "profile-level-id" => {
                                        value.len() == 6
                                            && value.bytes().all(|c| c.is_ascii_hexdigit())
                                    }
                                    "packetization-mode" | "level-asymmetry-allowed" => {
                                        ["0", "1"].contains(&value)
                                    }
                                    _ => false,
                                }
                            })
                    })
            }
            "control" => serde_json::from_value::<BrowserStats>(v.clone())
                .ok()
                .is_some_and(|s| {
                    s.validate().is_ok() && s.last_error.as_ref().is_none_or(|s| safe_code(s))
                }),
            _ => false,
        };
        if !valid {
            return Err("RECEIVER_EVIDENCE_METRICS".into());
        }
    }
    Ok(())
}
impl ReceiverEvidence {
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.errors.len() > 8
            || self.errors.iter().any(|s| !safe_code(s))
            || self.recorded_at_ms > 9_007_199_254_740_991
            || self.sampled_at_ms.is_some_and(|n| n > self.recorded_at_ms)
            || ((self.capture == Capture::Unavailable)
                != (self.snapshot.is_none() && self.sampled_at_ms.is_none()))
            || (self.capture != Capture::Unavailable
                && (self.snapshot.is_none() || self.sampled_at_ms.is_none()))
        {
            return Err("RECEIVER_EVIDENCE_SCHEMA".into());
        }
        if let Some(v) = &self.snapshot {
            validate_snapshot(v)?;
        }
        Ok(())
    }
    pub fn unavailable(stage: Stage) -> Self {
        Self {
            version: 1,
            stage,
            capture: Capture::Unavailable,
            sampled_at_ms: None,
            recorded_at_ms: hub_probe_wire::now() * 1000,
            snapshot: None,
            errors: vec!["WINDOW_FINAL_STATS_UNAVAILABLE".into()],
            cleanup: Cleanup::default(),
        }
    }
}
#[derive(Default)]
pub struct Journal {
    pre_saved: bool,
    complete: bool,
}
impl Journal {
    pub fn complete(&self) -> bool {
        self.complete
    }
    pub fn save(
        &mut self,
        dir: &Path,
        owner: &str,
        mut evidence: ReceiverEvidence,
        native_released: bool,
    ) -> Result<String, String> {
        evidence.validate()?;
        let id = uuid::Uuid::parse_str(owner).map_err(|_| "RECEIVER_EVIDENCE_OWNER")?;
        if id.is_nil() {
            return Err("RECEIVER_EVIDENCE_OWNER".into());
        }
        if self.complete
            || (evidence.stage == Stage::PreClose && self.pre_saved)
            || (evidence.stage == Stage::Cleanup && !self.pre_saved)
        {
            return Err("RECEIVER_EVIDENCE_STAGE_REPLAY".into());
        }
        evidence.cleanup.native_resources_released = Some(native_released);
        let suffix = match evidence.stage {
            Stage::PreClose => "receiver-pre-close",
            Stage::Cleanup => "receiver-final",
        };
        let name = format!("{id}-{suffix}.json");
        let data =
            serde_json::to_vec_pretty(&evidence).map_err(|_| "RECEIVER_EVIDENCE_ENCODING")?;
        if data.len() > 16384 {
            return Err("RECEIVER_EVIDENCE_SIZE".into());
        }
        std::fs::create_dir_all(dir).map_err(|_| "RECEIVER_EVIDENCE_DIRECTORY")?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(&name))
            .map_err(|_| "RECEIVER_EVIDENCE_EXISTS_OR_WRITE")?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|_| "RECEIVER_EVIDENCE_WRITE")?;
        self.pre_saved = true;
        self.complete = evidence.stage == Stage::Cleanup;
        Ok(name)
    }
    pub fn save_missing(
        &mut self,
        dir: &Path,
        owner: &str,
        native_released: bool,
    ) -> Result<(), String> {
        if !self.pre_saved {
            self.save(
                dir,
                owner,
                ReceiverEvidence::unavailable(Stage::PreClose),
                native_released,
            )?;
        }
        if !self.complete {
            self.save(
                dir,
                owner,
                ReceiverEvidence::unavailable(Stage::Cleanup),
                native_released,
            )?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> ReceiverEvidence {
        ReceiverEvidence {
            version: 1,
            stage: Stage::PreClose,
            capture: Capture::Live,
            sampled_at_ms: Some(1000),
            recorded_at_ms: 1001,
            snapshot: Some(
                serde_json::json!({"framesDecoded":12,"peerMatches":false,"connectionPath":"unknown","iceState":"failed"}),
            ),
            errors: vec!["BROWSER_SELECTED_DIRECT_PATH_UNVERIFIED".into()],
            cleanup: Cleanup::default(),
        }
    }
    #[test]
    fn failed_path_is_evidence_not_permission_and_private_payload_is_rejected() {
        let good = sample();
        assert!(good.validate().is_ok());
        for bad in [
            serde_json::json!({"sdp":"private"}),
            serde_json::json!({"framesDecoded":-1}),
            serde_json::json!({"codec":"192.168.1.2"}),
            serde_json::json!({"fmtp":"address=192.168.1.2"}),
        ] {
            let mut e = good.clone();
            e.snapshot = Some(bad);
            assert!(e.validate().is_err());
        }
        let mut e = good.clone();
        e.errors = vec!["private address 192.168.1.2".into()];
        assert!(e.validate().is_err());
        e = good.clone();
        e.capture = Capture::Unavailable;
        assert!(e.validate().is_err());
        e = good;
        e.sampled_at_ms = Some(2000);
        assert!(e.validate().is_err());
        assert!(serde_json::from_value::<ReceiverEvidence>(
            serde_json::json!({"version":1,"sdp":"secret"})
        )
        .is_err());
    }
    #[test]
    fn actual_file_journal_is_ordered_non_overwriting_and_native_cleanup_is_authoritative() {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = std::env::temp_dir().join(format!("remotehub-receiver-{id}"));
        let mut journal = Journal::default();
        let mut e = sample();
        e.stage = Stage::Cleanup;
        assert!(journal.save(&dir, &id, e, false).is_err());
        assert!(journal.save(&dir, "../escape", sample(), false).is_err());
        let name = journal.save(&dir, &id, sample(), false).unwrap();
        assert!(journal.save(&dir, &id, sample(), true).is_err());
        let first: Value =
            serde_json::from_slice(&std::fs::read(dir.join(&name)).unwrap()).unwrap();
        assert_eq!(first["cleanup"]["nativeResourcesReleased"], false);
        journal.save_missing(&dir, &id, true).unwrap();
        assert!(journal.complete());
        let last: Value = serde_json::from_slice(
            &std::fs::read(dir.join(format!("{id}-receiver-final.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(last["capture"], "unavailable");
        assert!(last["snapshot"].is_null());
        assert_eq!(last["cleanup"]["nativeResourcesReleased"], true);
        for p in std::fs::read_dir(&dir).unwrap() {
            std::fs::remove_file(p.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(&dir).unwrap();
    }
}
