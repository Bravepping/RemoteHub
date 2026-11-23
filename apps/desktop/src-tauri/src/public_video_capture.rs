//! Native-only host source for one explicitly consented scoped video run.
//! No screen bytes are exposed to IPC, persisted, or included in errors.

use hub_encode_win::{h264::EncodedVideoFrame, lifecycle::StopToken};
use hub_webrtc::{
    public_video_media::ScopedVideoSender, public_video_wss::ScopedWssState, video::Packetizer,
};
use std::time::Duration;
use tokio::{
    sync::{mpsc, watch},
    time::{timeout, Instant},
};

pub(crate) const CAPTURE_WINDOW: Duration = Duration::from_secs(15);
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

/// Invoke only after native approval. Keep polling through completion after
/// signaling Stop so DXGI/MF's owning worker is joined before reporting cleanup.
/// The result counts complete AUs admitted locally, not sent or displayed frames.
#[cfg(windows)]
pub(crate) async fn run(
    sender: ScopedVideoSender,
    stop: watch::Receiver<bool>,
    phase: watch::Receiver<ScopedWssState>,
) -> Result<u32, &'static str> {
    use hub_encode_win::{EncodeError, EncoderConfig};
    use std::sync::atomic::AtomicBool;

    run_with_source(sender, stop, phase, CAPTURE_WINDOW, |handoff| {
        let mut handoff_error = None;
        let result = hub_encode_win::stream::run(
            true,
            EncoderConfig {
                seconds: CAPTURE_WINDOW.as_secs() as u32,
                ..Default::default()
            },
            handoff.cancel.clone(),
            &AtomicBool::new(false),
            |_| {},
            |frame| {
                handoff.push(frame).map_err(|code| {
                    handoff_error = Some(code);
                    EncodeError::Native(code.into())
                })
            },
        );
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
) -> Result<u32, &'static str> {
    tokio::select! {
        biased;
        _ = stop.wait_for(|&stop| stop) => return Ok(0),
        phase = timeout(PATH_WAIT, phase.wait_for(|state| verified(state) || *state == ScopedWssState::Closed)) => {
            match phase {
                Ok(Ok(state)) if verified(&state) => (),
                Ok(_) => return Ok(0),
                Err(_) => return Err(PATH_TIMEOUT),
            }
        }
    }

    let cancel = CancelOnDrop(StopToken::default());
    let deadline = Instant::now() + capture_window;
    let (tx, mut rx) = mpsc::channel(1);
    let handoff = FrameHandoff {
        tx,
        cancel: cancel.0.clone(),
        stop: stop.clone(),
        phase: phase.clone(),
        deadline: deadline.into_std(),
    };
    // The blocking worker owns all COM, MF and DXGI operations. A final
    // stop/phase check on that same thread covers shutdown while it queues.
    let worker = tokio::task::spawn_blocking(move || {
        if !handoff.is_live() {
            return Ok(());
        }
        source(handoff)
    });

    let mut admitted = 0;
    let mut packetizer = Packetizer::default();
    let result = tokio::select! {
        biased;
        _ = stop.wait_for(|&stop| stop) => None,
        _ = phase.wait_for(|state| !verified(state)) => None,
        _ = tokio::time::sleep_until(deadline) => None,
        result = async {
            while let Some(frame) = rx.recv().await {
                let packets = packetizer.packets(frame, 102).map_err(|_| BAD_AU)?;
                timeout(SEND_WAIT, sender.send_access_unit(packets))
                    .await
                    .map_err(|_| BACKPRESSURE)??;
                admitted += 1;
            }
            Ok::<(), &'static str>(())
        } => Some(result),
    };
    // Revocation can reach the port before the actor broadcasts Closed.
    // The actor, not the capture adapter, reports the underlying cause.
    let result = result.filter(|result| !matches!(result, Err(TRANSPORT_CLOSED)));
    // Signal first, then unblock any in-progress handoff before joining.
    if !matches!(result, Some(Ok(()))) {
        cancel.0.stop();
    }
    drop(rx);
    let worker_result = worker.await.map_err(|_| WORKER_FAILED)?;
    if let Some(transfer_result) = result {
        transfer_result?;
        match worker_result {
            Err(STOPPED) => (),
            other => other?,
        }
    }
    Ok(admitted)
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
    deadline: std::time::Instant,
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
        assert_eq!(capture.await, Ok(0));
    }

    #[tokio::test]
    async fn verified_path_accepts_large_access_unit_but_rejects_oversize() {
        let (_media, sender) = ScopedVideoMedia::host();
        let (_stop, stop_rx) = watch::channel(false);
        let (_phase, phase_rx) = watch::channel(path());
        assert_eq!(
            run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |handoff| {
                handoff.push(frame(96 * 1024, 0))
            })
            .await,
            Ok(1)
        );

        let (_media, sender) = ScopedVideoMedia::host();
        let (_stop, stop_rx) = watch::channel(false);
        let (_phase, phase_rx) = watch::channel(path());
        assert_eq!(
            run_with_source(sender, stop_rx, phase_rx, CAPTURE_WINDOW, |handoff| {
                handoff.push(frame(MAX_ANNEX_B_BYTES + 1, 0))
            })
            .await,
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
                    stop.send_replace(true);
                }
            };
            let (outcome, ()) = timeout(Duration::from_secs(2), async {
                tokio::join!(capture, observer)
            })
            .await
            .expect("cleanup must join the worker");
            assert_eq!(outcome, Ok(0));
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
            .await,
            Ok(0)
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
        assert_eq!(result, Err(BACKPRESSURE));
    }
}
