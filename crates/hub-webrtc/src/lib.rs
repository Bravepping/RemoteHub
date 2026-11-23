#[cfg(feature = "v2-loopback-signaling")]
pub mod authenticated_relay;
pub mod control;
#[cfg(windows)]
pub mod control_channel;
#[cfg(all(test, not(windows), not(feature = "p3-lan")))]
#[allow(dead_code)]
mod control_channel;
#[cfg(feature = "p006-direct-probe")]
pub mod direct_diagnostics;
#[cfg(feature = "p006-direct-probe")]
pub mod direct_policy;
#[cfg(feature = "p006-direct-probe")]
pub mod direct_probe;
#[cfg(feature = "p006-direct-probe")]
pub mod direct_runtime;
#[cfg(feature = "p006-turn-probe")]
pub mod ice_policy;
#[cfg(feature = "p3-lan")]
pub mod input_pair;
#[cfg(windows)]
pub mod lan;
#[cfg(all(feature = "v2-verified-lan", any(windows, test)))]
pub(crate) mod lan_device;
pub mod lan_wire;
pub mod pinned_runtime;
#[cfg(all(feature = "v2-private-wss-lan", any(windows, test)))]
mod private_wss_contract;
#[cfg(feature = "v2-public-video")]
pub mod public_video;
#[cfg(feature = "v2-public-video")]
mod public_video_route;
#[cfg(feature = "v2-public-video-wss")]
mod public_video_frames;
#[cfg(feature = "v2-public-video-wss")]
mod public_video_ice;
#[cfg(feature = "v2-public-video-wss")]
pub mod public_video_lifecycle;
#[cfg(feature = "v2-public-video-wss")]
pub mod public_video_wss;
#[cfg(all(windows, feature = "p006-relay"))]
pub mod receiver_evidence;
#[cfg(all(windows, feature = "p006-relay"))]
pub mod relay_client;
#[cfg(all(
    feature = "p006-relay",
    any(windows, all(test, feature = "p006-session-probe"))
))]
mod relay_config;
#[cfg(all(feature = "p006-session-probe", any(windows, test)))]
mod relay_diagnostic;
#[cfg(feature = "p006-turn-probe")]
pub mod relay_runtime;
#[cfg(feature = "p3-lan")]
pub mod remote_input;
#[cfg(feature = "p006-turn-probe")]
pub mod rendezvous_probe;
pub mod scope;
#[cfg(windows)]
pub mod session;
pub mod signaling;
#[cfg(feature = "p006-turn-probe")]
pub mod turn_probe;
pub mod video;

#[cfg(feature = "v2-verified-signaling")]
pub mod verified_wss;
