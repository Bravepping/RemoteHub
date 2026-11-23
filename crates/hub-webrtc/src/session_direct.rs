//! DirectPreferred integration of the existing video/control Session.
use super::*;
use crate::direct_policy::{Candidate, DirectGrant};
pub struct DirectSettings {
    pub grant: Arc<DirectGrant>,
    pub config: Option<crate::ice_policy::IceConfig>,
    pub publish_private_local: bool,
}
impl Session {
    pub fn new_direct(
        seconds: u32,
        settings: DirectSettings,
        control_session: String,
    ) -> Result<Arc<Self>, String> {
        settings
            .grant
            .configuration(settings.config.as_ref(), hub_probe_wire::now())?;
        if settings.grant.session_id() != control_session {
            return Err("DIRECT_CONTROL_SESSION_MISMATCH".into());
        }
        let boundary =
            crate::direct_runtime::Boundary::new(&settings.grant, hub_probe_wire::now())?;
        let mut session = Self::configured(
            seconds,
            crate::scope::NetworkScope::Loopback,
            Some(control_session.clone()),
        )?;
        let inner = Arc::get_mut(&mut session).ok_or("SESSION_ALREADY_SHARED")?;
        inner.control = Some(crate::control_channel::Endpoint::new_direct_guarded(
            control_session,
            inner.shared.stop.clone(),
            seconds + 60,
        )?);
        inner.direct = Some(settings);
        inner.direct_boundary = Some(boundary);
        Ok(session)
    }
    /// Only compiled for explicitly invoked no-desktop operator diagnostics, never a Tauri command.
    #[cfg(feature = "p006-session-probe")]
    pub fn new_direct_synthetic(
        seconds: u32,
        settings: DirectSettings,
        session: String,
    ) -> Result<Arc<Self>, String> {
        let mut s = Self::new_direct(seconds, settings, session)?;
        let inner = Arc::get_mut(&mut s).ok_or("SESSION_ALREADY_SHARED")?;
        inner.synthetic = true;
        inner
            .shared
            .update(|s| s.source_kind = "synthetic_nv12".into());
        Ok(s)
    }
    #[cfg(feature = "p006-session-probe")]
    pub fn new_direct_synthetic_transport_test(
        seconds: u32,
        settings: DirectSettings,
        id: String,
        drop_direct: bool,
        redact_direct_sdp: bool,
    ) -> Result<Arc<Self>, String> {
        let mut s = Self::new_direct_synthetic(seconds, settings, id)?;
        let inner = Arc::get_mut(&mut s).ok_or("SESSION_ALREADY_SHARED")?;
        inner.redact_direct_sdp = redact_direct_sdp;
        let grant = &inner.direct.as_ref().ok_or("DIRECT_POLICY_MISSING")?.grant;
        inner.direct_boundary = Some(crate::direct_runtime::Boundary::diagnostic(
            grant,
            hub_probe_wire::now(),
            drop_direct,
            false,
        )?);
        inner.shared.update(|s| {
            s.diagnostic_transport = Some(DiagnosticTransport {
                drop_direct,
                dropped_packets: 0,
            })
        });
        Ok(s)
    }
    pub(super) fn accept_direct_offer(&self, sdp: &str) -> Result<Option<Negotiation>, String> {
        let Some(d) = &self.direct else {
            return Ok(None);
        };
        if self.started.load(Ordering::Acquire) || self.shared.stop.is_stopped() {
            self.cancel();
            return Err("DIRECT_STOPPED_OR_REPLAYED".into());
        }
        #[cfg(feature = "p006-session-probe")]
        if self.redact_direct_sdp {
            if let Err(error) = crate::relay_diagnostic::require_direct_wire(sdp) {
                self.cancel();
                return Err(error);
            }
        }
        let outcome = crate::signaling::validate_direct_video_description(
            sdp,
            &d.grant,
            hub_probe_wire::now(),
            false,
            false,
        );
        let (negotiation, remote) = match outcome {
            Ok(v) => v,
            Err(e) => {
                self.cancel();
                return Err(e);
            }
        };
        if let Err(e) = self
            .direct_boundary
            .as_ref()
            .ok_or("DIRECT_BOUNDARY_MISSING")?
            .install_peer(remote.clone())
        {
            self.cancel();
            return Err(e);
        }
        *self
            .direct_remote
            .lock()
            .map_err(|_| "DIRECT_CANDIDATE_LOCK")? = Some(remote);
        Ok(Some(negotiation))
    }
    pub(super) fn direct_answer(&self, sdp: String) -> Result<String, String> {
        let Some(d) = &self.direct else {
            return Ok(sdp);
        };
        let answer = crate::direct_policy::publish_sdp(&sdp, &d.grant, d.publish_private_local);
        #[cfg(feature = "p006-session-probe")]
        let answer = if self.redact_direct_sdp {
            crate::relay_diagnostic::redact_direct(&answer)?
        } else {
            answer
        };
        let (_, local) = crate::signaling::validate_direct_video_description(
            &answer,
            &d.grant,
            hub_probe_wire::now(),
            true,
            true,
        )?;
        *self
            .direct_local
            .lock()
            .map_err(|_| "DIRECT_CANDIDATE_LOCK")? = Some(local);
        Ok(answer)
    }
    pub(super) fn publish_direct_candidate_stats(&self) {
        if let Ok(local) = self.direct_local.lock() {
            if let Some(local) = local.as_ref() {
                self.shared.update(|s| {
                    s.native_candidates = local
                        .candidates()
                        .iter()
                        .map(|c| {
                            format!(
                                "{}/approved/udp",
                                match c.kind {
                                    crate::direct_policy::Kind::Host => "host",
                                    crate::direct_policy::Kind::Srflx => "srflx",
                                    crate::direct_policy::Kind::Relay => "relay",
                                }
                            )
                        })
                        .collect()
                });
            }
        }
    }
    pub(super) async fn verify_direct_pair(
        &self,
        pc: &Arc<dyn PeerConnection>,
    ) -> Result<(), String> {
        let d = self.direct.as_ref().ok_or("DIRECT_POLICY_MISSING")?;
        if self.shared.stop.is_stopped() || !d.grant.current(hub_probe_wire::now()) {
            return Err("DIRECT_SESSION_INACTIVE".into());
        }
        let sctp = pc.sctp().await.ok_or("DIRECT_SCTP_MISSING")?;
        let pair = sctp
            .transport()
            .ice_transport()
            .get_selected_candidate_pair()
            .await
            .map_err(|_| "DIRECT_PAIR_READ_FAILED")?
            .ok_or("DIRECT_PAIR_NOT_SELECTED")?;
        self.verify_direct_values(pair.local(), pair.remote())
    }
    fn verify_direct_values(
        &self,
        actual_local: &webrtc::peer_connection::RTCIceCandidate,
        actual_remote: &webrtc::peer_connection::RTCIceCandidate,
    ) -> Result<(), String> {
        let d = self.direct.as_ref().ok_or("DIRECT_POLICY_MISSING")?;
        if self.shared.stop.is_stopped() {
            return Err("DIRECT_SESSION_INACTIVE".into());
        }
        let convert = |c: &webrtc::peer_connection::RTCIceCandidate| -> Result<Candidate, String> {
            if c.protocol.to_string() != "udp" {
                return Err("DIRECT_PAIR_PROTOCOL".into());
            }
            Candidate::parse(&format!("x 1 udp 1 {} {} typ {}", c.address, c.port, c.typ))
        };
        let local = self
            .direct_local
            .lock()
            .map_err(|_| "DIRECT_CANDIDATE_LOCK")?;
        let remote = self
            .direct_remote
            .lock()
            .map_err(|_| "DIRECT_CANDIDATE_LOCK")?;
        let mut diagnostic = crate::direct_diagnostics::inspect(
            &d.grant,
            actual_local,
            actual_remote,
            local.as_ref(),
            remote.as_ref(),
            hub_probe_wire::now(),
        );
        diagnostic.private_local_publication_approved = Some(d.publish_private_local);
        let approved = diagnostic.approved;
        self.shared.update(|s| {
            if s.direct_pair_diagnostic
                .as_ref()
                .is_none_or(|old| old.approved)
            {
                s.direct_pair_diagnostic = Some(diagnostic);
            }
        });
        if !approved {
            return Err("DIRECT_SELECTED_PAIR_NOT_APPROVED".into());
        }
        let summary = crate::direct_policy::selected(
            convert(actual_local)?,
            convert(actual_remote)?,
            local.as_ref().ok_or("DIRECT_LOCAL_CANDIDATES_MISSING")?,
            remote.as_ref().ok_or("DIRECT_REMOTE_CANDIDATES_MISSING")?,
        )?;
        self.shared.update(|s| s.native_path = Some(summary));
        if let Some(control) = &self.control {
            control.approve_verified_path();
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> DirectSettings {
        DirectSettings {
            grant: Arc::new(
                DirectGrant::local_diagnostic(
                    "192.168.1.2".parse().unwrap(),
                    hub_probe_wire::now() + 180,
                    true,
                    hub_probe_wire::now(),
                )
                .unwrap(),
            ),
            config: None,
            publish_private_local: true,
        }
    }
    fn offer() -> String {
        format!("v=0\r\na=group:BUNDLE 0 1\r\na=fingerprint:sha-256 {}\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\na=mid:0\r\na=recvonly\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 packetization-mode=1;profile-level-id=42001f\r\na=candidate:h 1 udp 1 192.168.1.2 50000 typ host\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\na=mid:1\r\na=sctp-port:5000\r\na=max-message-size:16384\r\n",["AB";32].join(":"))
    }
    #[tokio::test]
    async fn rejected_actual_pair_keeps_control_closed_and_preserves_first_failure() {
        use crate::{direct_diagnostics::Reason, direct_policy::VerifiedCandidates};
        use webrtc::peer_connection::{RTCIceCandidate, RTCIceCandidateType, RTCIceProtocol};
        let settings = settings();
        let id = settings.grant.session_id();
        let scope = settings.grant.scope();
        let s = Session::new_direct(5, settings, id).unwrap();
        let candidate = Candidate::parse("x 1 udp 1 192.168.1.2 50000 typ host").unwrap();
        let approved = VerifiedCandidates {
            values: vec![candidate],
            scope,
        };
        *s.direct_local.lock().unwrap() = Some(approved.clone());
        *s.direct_remote.lock().unwrap() = Some(approved);
        let local = RTCIceCandidate {
            address: "192.168.1.2".into(),
            port: 50001,
            component: 1,
            protocol: RTCIceProtocol::Udp,
            typ: RTCIceCandidateType::Host,
            ..Default::default()
        };
        assert!(s.verify_direct_values(&local, &local).is_err());
        assert!(!s.control.as_ref().unwrap().is_permitted());
        assert_eq!(
            s.stats().direct_pair_diagnostic.unwrap().local.reason,
            Reason::PortChanged
        );
        s.stop().await.unwrap();
        assert!(s.stats().resources_released);
        assert_eq!(s.stats().stream.frames_input, 0);
        assert_eq!(
            s.stats().direct_pair_diagnostic.unwrap().local.reason,
            Reason::PortChanged
        );
    }
    #[tokio::test]
    async fn wrong_control_session_and_bad_media_cannot_start_video_or_open_permissions() {
        let d = settings();
        assert!(Session::new_direct(5, d, uuid::Uuid::new_v4().to_string()).is_err());
        let d = settings();
        let id = d.grant.session_id();
        let s = Session::new_direct(5, d, id).unwrap();
        assert!(!s.control.as_ref().unwrap().is_permitted());
        assert!(s.signal(offer().replace("video ", "audio ")).await.is_err());
        assert_eq!(s.stats().stream.frames_input, 0);
        assert_eq!(s.stats().frames_sent, 0);
        s.stop().await.unwrap();
        assert!(s.stats().resources_released);
    }
    #[tokio::test]
    async fn valid_video_policy_is_separate_and_replay_revokes_without_capture() {
        let d = settings();
        let id = d.grant.session_id();
        let s = Session::new_direct(5, d, id).unwrap();
        let good = offer();
        assert!(crate::signaling::validate_control_description(
            &good,
            crate::scope::NetworkScope::Loopback,
            false
        )
        .is_err());
        assert!(s.accept_direct_offer(&good).unwrap().is_some());
        assert!(s.accept_direct_offer(&good).is_err());
        assert!(s.shared.stop.is_stopped());
        assert!(!s.control.as_ref().unwrap().is_permitted());
        s.stop().await.unwrap();
        assert_eq!(s.stats().frames_sent, 0);
    }
    #[test]
    fn outgoing_private_candidates_require_counterparty_permission() {
        let d = settings();
        let raw = offer();
        let hidden = crate::direct_policy::publish_sdp(&raw, &d.grant, false);
        assert!(!hidden.contains("a=candidate:"));
        assert!(crate::signaling::validate_direct_video_description(
            &hidden,
            &d.grant,
            hub_probe_wire::now(),
            false,
            true
        )
        .is_err());
        assert!(crate::signaling::validate_direct_video_description(
            &raw,
            &d.grant,
            hub_probe_wire::now(),
            false,
            false
        )
        .is_ok());
    }
}
