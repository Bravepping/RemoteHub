//! Opt-in, single-video, IPv4-loopback-only WebRTC transport. No remote listener or data channel.

mod sdp;
mod session;
pub mod signed_signal;
mod video;

pub use session::{Session, TransportStats};
pub use video::MediaFrame;

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("P004_WEBRTC: WebRTC operation failed")]
    WebRtc,
    #[error("P004_ICE_NO_LOOPBACK: no eligible 127.0.0.1 UDP host candidate")]
    NoLoopbackCandidate,
    #[error("P004_ICE_POLICY: candidate or selected pair is outside the loopback policy")]
    IcePolicy,
    #[error("P004_SDP: invalid single-track H.264 signaling")]
    Sdp,
    #[error("P004_ICE_TIMEOUT: loopback ICE operation timed out")]
    Timeout,
    #[error("P004_ICE_CLOSED: peer connection is not connected")]
    Disconnected,
    #[error("P004_H264_PROFILE: H.264 SPS is incompatible with the negotiated format")]
    Profile,
    #[error("P004_H264_AU: malformed Annex B video access unit")]
    AccessUnit,
    #[error("P004_H264_PTS: invalid or nonmonotonic encoder timestamps")]
    Timestamp,
    #[error("P004_RANDOM: secure random source unavailable")]
    Random,
}

impl From<webrtc::error::Error> for TransportError {
    fn from(_: webrtc::error::Error) -> Self {
        // Do not expose SDP, identities, ICE passwords or addresses in application logs/UI.
        Self::WebRtc
    }
}
