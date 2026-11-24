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
#[cfg(feature = "v2-input-approval")]
pub mod input_approval;
#[cfg(feature = "v2-input-approval")]
pub mod input_queue;
#[cfg(windows)]
pub mod lan;
#[cfg(all(feature = "v2-verified-lan", any(windows, test)))]
pub(crate) mod lan_device;
#[cfg(feature = "p3-lan-input-queue-test")]
pub mod lan_input_queue;
pub mod lan_wire;
pub mod pinned_runtime;
#[cfg(all(feature = "v2-private-wss-lan", any(windows, test)))]
mod private_wss_contract;
#[cfg(feature = "v2-public-fake-input-proof")]
pub mod public_fake_input;
#[cfg(feature = "v2-public-fake-input-proof")]
mod public_fake_input_pair;
#[cfg(feature = "v2-public-fake-input-proof")]
pub mod public_fake_input_profile;
#[cfg(feature = "v2-public-fake-input-proof")]
mod public_fake_input_sdp;
#[cfg(any(
    all(test, feature = "v2-public-fake-input-proof"),
    feature = "v2-public-fake-input-loopback-check"
))]
mod public_fake_input_sdp_fixture;
#[cfg(any(
    all(test, feature = "v2-public-fake-input-proof"),
    feature = "v2-public-fake-input-loopback-check"
))]
mod public_fake_input_turn_tests;
#[cfg(any(
    all(test, feature = "v2-public-fake-input-proof"),
    feature = "v2-public-fake-input-loopback-check"
))]
mod public_fake_input_wss_fixture;
#[cfg(any(
    all(test, feature = "v2-public-fake-input-proof"),
    feature = "v2-public-fake-input-loopback-check"
))]
mod public_fake_input_wss_wire;
// Only the opt-in, harness-less loopback diagnostic calls this entry point.
#[cfg(feature = "v2-public-fake-input-loopback-check")]
#[doc(hidden)]
pub use public_fake_input_turn_tests::run_wss_turn_loopback_check;
#[cfg(feature = "v2-public-video")]
pub mod public_video;
#[cfg(feature = "v2-public-video-wss")]
mod public_video_frames;
#[cfg(feature = "v2-public-video-wss")]
mod public_video_ice;
#[cfg(feature = "v2-public-video-wss")]
pub mod public_video_lifecycle;
#[cfg(feature = "v2-public-video-wss")]
pub mod public_video_media;
#[cfg(feature = "v2-public-video")]
mod public_video_route;
#[cfg(feature = "v2-public-video-wss")]
mod public_video_scoped_ice;
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
