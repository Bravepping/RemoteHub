//! Native-only host source for one explicitly consented scoped video run.
//! No screen bytes are exposed to IPC, persisted, or included in errors.

use hub_encode_win::{h264::EncodedVideoFrame, lifecycle::StopToken};
use hub_webrtc::{
    public_video_media::ScopedVideoSender, public_video_wss::ScopedWssState, video::Packetizer,
};
use std::{
    sync::{Arc, OnceLock},
    time::{Duration, Instant as MonotonicInstant},
};
use tokio::{
    sync::{mpsc, watch},
    time::{timeout, Instant},
};

#[cfg(not(feature = "v2-public-video-60s-test"))]
pub(crate) const CAPTURE_WINDOW: Duration = Duration::from_secs(15);
#[cfg(feature = "v2-public-video-60s-test")]
pub(crate) const CAPTURE_WINDOW: Duration =
    Duration::from_secs(hub_encode_win::PUBLIC_VIDEO_TEST_SECONDS as u64);
const PATH_WAIT: Duration = Duration::from_secs(25);
const SEND_WAIT: Duration = Duration::from_millis(250);
// Bound the native handoff before packetization allocates anything. The scoped
// sender independently enforces its final packet count and per-payload limits.
const MAX_ANNEX_B_BYTES: usize = 256 * 1024;
const PATH_TIMEOUT: &str = "PUBLIC_VIDEO_CAPTURE_PATH_TIMEOUT";
const WORKER_FAILED: &str = "PUBLIC_VIDEO_CAPTURE_WORKER_FAILED";
const BAD_AU: &str = "PUBLIC_VIDEO_CAPTURE_INVALID_AU";
const BACKPRESSURE: &str = "PUBLIC_VIDEO_CAPTURE_BACKPRESSURE";
const STOPPED: &str = "PUBLIC_VIDEO_CAPTURE_STOPPED";
const TRANSPORT_CLOSED: &str = "PUBLIC_VIDEO_MEDIA_CLOSED";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureEnd {
    LocalStop,
    TransportEnded,
    Limit,
    SourceEnded,
}

#[derive(Debug)]
pub(crate) struct CaptureOutcome {
    pub(crate) end: Result<CaptureEnd, &'static str>,
    pub(crate) elapsed_ms: Option<u64>,
}

impl CaptureOutcome {
    fn before_start(end: Result<CaptureEnd, &'static str>) -> Self {
        Self {
            end,
            elapsed_ms: None,
        }
    }
}

/// Invoke only after native approval. Keep polling through completion after
/// signaling Stop so DXGI/MF's owning worker is joined before reporting cleanup.
/// The result describes the local producer, never viewer receipt or presentation.
#[cfg(windows)]
pub(crate) async fn run(
    sender: ScopedVideoSender,
    stop: watch::Receiver<bool>,
    phase: watch::Receiver<ScopedWssState>,
) -> CaptureOutcome {
    use hub_encode_win::stream::{self, StreamReport};
    use hub_encode_win::EncodeError;
    #[cfg(not(feature = "v2-public-video-60s-test"))]
    use {hub_encode_win::EncoderConfig, std::sync::atomic::AtomicBool};

    run_with_source(sender, stop, phase, CAPTURE_WINDOW, |handoff| {
        let mut handoff_error = None;
        let publish = |_: &StreamReport| handoff.mark_started();
        let sink = |frame| {
            handoff.push(frame).map_err(|code| {
                handoff_error = Some(code);
                EncodeError::Native(code.into())
            })
        };
        #[cfg(not(feature = "v2-public-video-60s-test"))]
        let result = stream::run(
            true,
            EncoderConfig {
                seconds: CAPTURE_WINDOW.as_secs() as u32,
                ..Default::default()
            },
            handoff.cancel.clone(),
            &AtomicBool::new(false),
            publish,
            sink,
        );
        #[cfg(feature = "v2-public-video-60s-test")]
        let result = stream::run_public_video_60s_test(true, handoff.cancel.clone(), publish, sink);
        result
            .map(|_| ())
            .map_err(|_| handoff_error.unwrap_or("PUBLIC_VIDEO_CAPTURE_ENCODER_FAILED"))
    })
    .await
}

async fn run_with_source(
    mut sender: ScopedVideoSender,
    mut stop: watch::Receiver<bool>,
    mut phase: watch::Receiver<ScopedWssState>,
    capture_window: Duration,
    source: impl FnOnce(FrameHandoff) -> Result<(), &'static str> + Send + 'static,
) -> CaptureOutcome {
    tokio::select! {
        biased;
        _ = stop.wait_for(|&stop| stop) => return CaptureOutcome::before_start(Ok(CaptureEnd::LocalStop)),
        phase = timeout(PATH_WAIT, phase.wait_for(|state| verified(state) || *state == ScopedWssState::Closed)) => {
            match phase {
                Ok(Ok(state)) if verified(&state) => (),
                Ok(_) => return CaptureOutcome::before_start(Ok(CaptureEnd::TransportEnded)),
                Err(_) => return CaptureOutcome::before_start(Err(PATH_TIMEOUT)),
            }
        }
    }

    let cancel = CancelOnDrop(StopToken::default());
    let deadline = Instant::now() + capture_window;
    let started = Arc::new(OnceLock::new());
    let (tx, mut rx) = mpsc::channel(1);
    let handoff = FrameHandoff {
        tx,
        cancel: cancel.0.clone(),
        stop: stop.clone(),
        phase: phase.clone(),
        deadline: deadline.into_std(),
        started: started.clone(),
    };
    // The blocking worker owns all COM, MF and DXGI operations. A final
    // stop/phase check on that same thread covers shutdown while it queues.
    let worker = tokio::task::spawn_blocking(move || {
        let result = if handoff.is_live() {
            source(handoff)
        } else {
            Ok(())
        };
        (result, MonotonicInstant::now())
    });

    #[cfg(not(feature = "v2-public-video-60s-test"))]
    let mut packetizer = Packetizer::default();
    #[cfg(feature = "v2-public-video-60s-test")]
    let mut packetizer = Packetizer::public_video_60s_test();
    let end = tokio::select! {
        biased;
        _ = stop.wait_for(|&stop| stop) => Ok(CaptureEnd::LocalStop),
        _ = phase.wait_for(|state| !verified(state)) => Ok(CaptureEnd::TransportEnded),
        _ = tokio::time::sleep_until(deadline) => Ok(CaptureEnd::Limit),
        result = async {
            while let Some(frame) = rx.recv().await {
                let packets = packetizer.packets(frame, 102).map_err(|_| BAD_AU)?;
                timeout(SEND_WAIT, sender.send_access_unit(packets))
                    .await
                    .map_err(|_| BACKPRESSURE)??;
            }
            Ok::<(), &'static str>(())
        } => match result {
            // The actor may revoke the port before broadcasting Closed.
            Err(TRANSPORT_CLOSED) => Ok(CaptureEnd::TransportEnded),
            other => other.map(|()| CaptureEnd::SourceEnded),
        },
    };
    let cutoff = MonotonicInstant::now().min(deadline.into_std());
    // Signal first, unblock any in-progress native handoff, and let the
    // actor close its transport without waiting for native thread cleanup.
    if end != Ok(CaptureEnd::SourceEnded) {
        cancel.0.stop();
    }
    drop(rx);
    drop(sender);
    let (end, finished) = match worker.await {
        Ok((result, finished)) => {
            let end = if end == Ok(CaptureEnd::SourceEnded) {
                match result {
                    // The worker can notice a terminal condition just
                    // before the next poll of the async stop branches.
                    Ok(()) | Err(STOPPED) if finished >= deadline.into_std() => {
                        Ok(CaptureEnd::Limit)
                    }
                    Err(STOPPED) if *stop.borrow() || stop.has_changed().is_err() => {
                        Ok(CaptureEnd::LocalStop)
                    }
                    Err(STOPPED) => Ok(CaptureEnd::TransportEnded),
                    Err(error) => Err(error),
                    Ok(()) => end,
                }
            } else {
                end
            };
            (end, cutoff.min(finished))
        }
        Err(_) => (Err(WORKER_FAILED), cutoff),
    };
    // Monotonic time from actual stream readiness to source exit or
    // revocation, excluding ICE, encoder setup and cleanup/join delay.
    let elapsed_ms = started
        .get()
        .and_then(|began| finished.checked_duration_since(*began))
        .map(|elapsed| elapsed.as_millis() as u64);
    CaptureOutcome { end, elapsed_ms }
}

fn verified(state: &ScopedWssState) -> bool {
    matches!(state, ScopedWssState::HostPathVerified(..))
}

struct CancelOnDrop(StopToken);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

struct FrameHandoff {
    tx: mpsc::Sender<EncodedVideoFrame>,
    cancel: StopToken,
    stop: watch::Receiver<bool>,
    phase: watch::Receiver<ScopedWssState>,
    deadline: MonotonicInstant,
    started: Arc<OnceLock<MonotonicInstant>>,
}

impl FrameHandoff {
    fn is_live(&self) -> bool {
        !self.cancel.is_stopped()
            && std::time::Instant::now() < self.deadline
            && self.stop.has_changed().is_ok()
            && !*self.stop.borrow()
            && self.phase.has_changed().is_ok()
            && verified(&self.phase.borrow())
    }

    fn mark_started(&self) {
        if self.started.get().is_none() && self.is_live() {
            let _ = self.started.set(MonotonicInstant::now());
        }
    }

    fn push(&self, frame: EncodedVideoFrame) -> Result<(), &'static str> {
        if !self.is_live() {
            self.cancel.stop();
            return Err(STOPPED);
        }
        if !(1..=MAX_ANNEX_B_BYTES).contains(&frame.annex_b.len()) {
            return Err(BAD_AU);
        }
        self.tx.blocking_send(frame).map_err(|_| STOPPED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hub_webrtc::{
        direct_policy::{Kind, SelectedRoute},
        public_video_media::ScopedVideoMedia,
    };

    fn path() -> ScopedWssState {
        ScopedWssState::HostPathVerified(SelectedRoute::TurnRelay, Kind::Relay, Kind::Relay)
    }

    fn frame(len: usize, pts: i64) -> EncodedVideoFrame {
        let mut annex_b = vec![
            0, 0, 0, 1, 0x67, 0x42, 0x04, 0x1f, 0, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65,
        ];
        annex_b.resize(len, 0x55);
        EncodedVideoFrame {
            annex_b,
            pts_100ns: pts,
            duration_100ns: 333_333,
            nal_types: vec![7, 8, 5],
            is_idr: true,
            encode_elapsed_ms: 0.0,
        }
    }

    #[test]
    fn capture_window_is_compile_time_only() {
        let expected = if cfg!(feature = "v2-public-video-60s-test") {
            60
        } else {
            15
        };
        assert_eq!(CAPTURE_WINDOW.as_secs(), expected);
    }

    #[tokio::test]
    async fn never_starts_source_before_selected_path_or_after_stop() {
        let (_media, sender) = ScopedVideoMedia::host();
        let (stop, stop_rx) = watch::channel(false);
        let (phase, phase_rx) = watch::channel(ScopedWssState::Established);
        let capture = run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |_| {
            panic!("Established is not a verified path, and Stop must win");
        });
        tokio::pin!(capture);
        assert!(timeout(Duration::from_millis(10), &mut capture)
            .await
            .is_err());
        phase.send_replace(path());
        stop.send_replace(true);
        let outcome = capture.await;
        assert_eq!(outcome.end, Ok(CaptureEnd::LocalStop));
        assert_eq!(outcome.elapsed_ms, None);
    }

    #[tokio::test]
    async fn verified_path_accepts_large_access_unit_but_rejects_oversize() {
        let (_media, sender) = ScopedVideoMedia::host();
        let (_stop, stop_rx) = watch::channel(false);
        let (_phase, phase_rx) = watch::channel(path());
        assert_eq!(
            run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |handoff| {
                handoff.mark_started();
                handoff.push(frame(96 * 1024, 0))
            })
            .await
            .end,
            Ok(CaptureEnd::SourceEnded)
        );

        let (_media, sender) = ScopedVideoMedia::host();
        let (_stop, stop_rx) = watch::channel(false);
        let (_phase, phase_rx) = watch::channel(path());
        assert_eq!(
            run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |handoff| {
                handoff.push(frame(MAX_ANNEX_B_BYTES + 1, 0))
            })
            .await
            .end,
            Err(BAD_AU)
        );
    }

    #[tokio::test]
    async fn stop_and_deadline_both_cancel_the_source_and_join() {
        for explicit in [true, false] {
            let (_media, sender) = ScopedVideoMedia::host();
            let (stop, stop_rx) = watch::channel(false);
            let (_phase, phase_rx) = watch::channel(path());
            let (ready, started) = tokio::sync::oneshot::channel();
            let window = if explicit {
                CAPTURE_WINDOW
            } else {
                Duration::from_millis(120)
            };
            let capture = run_with_source(sender, stop_rx, phase_rx, window, |handoff| {
                handoff.mark_started();
                let _ = ready.send(());
                // Never opens DXGI, and no actor drains the scoped sender.
                while handoff.is_live() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(())
            });
            let observer = async {
                started.await.unwrap();
                if explicit {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    stop.send_replace(true);
                }
            };
            let (outcome, ()) = timeout(Duration::from_secs(2), async {
                tokio::join!(capture, observer)
            })
            .await
            .expect("cleanup must join the worker");
            let expected = if explicit {
                CaptureEnd::LocalStop
            } else {
                CaptureEnd::Limit
            };
            assert_eq!(outcome.end, Ok(expected));
            let elapsed = outcome
                .elapsed_ms
                .expect("only source readiness starts the clock");
            assert!((1..=window.as_millis() as u64).contains(&elapsed));
        }
    }

    #[tokio::test]
    async fn transport_revoke_is_normal_cancellation_before_phase_closes() {
        let (media, sender) = ScopedVideoMedia::host();
        let (_stop, stop_rx) = watch::channel(false);
        let (_phase, phase_rx) = watch::channel(path());
        drop(media);
        assert_eq!(
            run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |handoff| {
                handoff.push(frame(5_000, 0))
            })
            .await
            .end,
            Ok(CaptureEnd::TransportEnded)
        );
    }

    #[tokio::test]
    async fn transport_backpressure_stops_and_joins_the_blocked_handoff() {
        let (_media, sender) = ScopedVideoMedia::host(); // Don't drain the two-slot queue.
        let (_stop, stop_rx) = watch::channel(false);
        let (_phase, phase_rx) = watch::channel(path());
        let result = timeout(
            Duration::from_secs(2),
            run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |handoff| {
                for i in 0..100 {
                    handoff.push(frame(5_000, i * 333_333))?;
                }
                panic!("bounded queues should prevent admitting all frames");
            }),
        )
        .await
        .expect("stopping must unblock and join the capture worker");
        assert_eq!(result.end, Err(BACKPRESSURE));
    }
}
