//! Optional synthetic-video diagnostic helpers.

// Reuse the binary's exact relay router and room implementation. Its CLI
// entry point is not called or exported by this library.
#[cfg(feature = "v2-synthetic-video")]
#[allow(dead_code)]
#[path = "bin/v2_signaling.rs"]
mod relay;

#[cfg(feature = "v2-synthetic-video")]
pub use relay::verified_synthetic::verified_synthetic_frames;
