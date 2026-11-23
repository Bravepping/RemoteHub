//! Own one negotiated loopback video connection and its signaling lifetime.
//! This actor does not negotiate, open sockets, or grant consent.

use crate::public_video_wss::{PublicVideoWss, PublicVideoWssEvent};
use rtc::rtp::packet::Packet;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{timeout, Instant},
};
use webrtc::{peer_connection::PeerConnection, rtp_transceiver::RTCRtpTransceiverDirection};

const QUEUE_SIZE: usize = 4;
const SIGNAL_POLL: Duration = Duration::from_millis(25);
const PAIR_POLL: Duration = Duration::from_millis(100);
const LOCAL_READ_TIMEOUT: Duration = Duration::from_millis(400);
const CLOSE_TIMEOUT: Duration = Duration::from_millis(600);
const CLOSED: &str = "PUBLIC_VIDEO_SESSION_CLOSED";
const BUSY: &str = "PUBLIC_VIDEO_SESSION_BUSY";
const SCOPE_OR_POLL_TIMEOUT: &str = "PUBLIC_VIDEO_WSS_TIMEOUT";

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PublicVideoEnd {
    Stopped,
    PeerLeft,
    ControllerDropped,
    Error(&'static str),
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct PublicVideoLifecycleOutcome {
    pub reason: PublicVideoEnd,
    pub cleanup: Result<(), &'static str>,
}

enum Command {
    Send(Packet, oneshot::Sender<Result<(), &'static str>>),
    Receive(oneshot::Sender<Result<Packet, &'static str>>),
}

pub struct PublicVideoHandle {
    commands: mpsc::Sender<Command>,
    stop: watch::Sender<bool>,
    outcome: watch::Receiver<Option<PublicVideoLifecycleOutcome>>,
}

impl PublicVideoHandle {
    pub async fn send_rtp(&self, packet: Packet) -> Result<(), &'static str> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(Command::Send(packet, tx))?;
        rx.await.map_err(|_| CLOSED)?
    }

    /// Only request a packet once WebRTC has surfaced the receiver track.
    pub async fn recv_rtp(&self) -> Result<Packet, &'static str> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(Command::Receive(tx))?;
        rx.await.map_err(|_| CLOSED)?
    }

    fn enqueue(&self, command: Command) -> Result<(), &'static str> {
        if *self.stop.borrow() {
            return Err(CLOSED);
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => BUSY,
                mpsc::error::TrySendError::Closed(_) => CLOSED,
            })
    }

    /// Independent of the media queue. No further request can start once
    /// stop is observed; a packet already handed to WebRTC cannot be recalled.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    pub async fn wait_closed(&self) -> Result<PublicVideoLifecycleOutcome, &'static str> {
        let mut outcome = self.outcome.clone();
        let value = *outcome
            .wait_for(Option::is_some)
            .await
            .map_err(|_| CLOSED)?;
        value.ok_or(CLOSED)
    }
}

/// Create, but do not detach, a supervised video-only fixture session.
/// Drive the future to completion (e.g. with `join!`); dropping the handle
/// requests cleanup. Do not use a separate clone of the peer for media while
/// this actor owns it. The peer and WSS must already have finished SDP.
pub fn supervise_video<'a>(
    wss: PublicVideoWss<'a>,
    pc: Arc<dyn PeerConnection>,
) -> (
    PublicVideoHandle,
    impl Future<Output = PublicVideoLifecycleOutcome> + 'a,
) {
    let (commands, command_rx) = mpsc::channel(QUEUE_SIZE);
    let (stop, stop_rx) = watch::channel(false);
    let (outcome_tx, outcome) = watch::channel(None);
    let handle = PublicVideoHandle {
        commands,
        stop,
        outcome,
    };
    let task = async move {
        let mut commands = command_rx;
        let mut stop = stop_rx;
        let mut wss = wss;
        let reason = active(&mut wss, &*pc, &mut commands, &mut stop).await;
        // Close command admission before waiting for either cleanup to finish.
        drop(commands);
        let (wss_close, pc_close) = tokio::join!(
            timeout(CLOSE_TIMEOUT, wss.leave()),
            timeout(CLOSE_TIMEOUT, pc.close()),
        );
        let cleanup = match (wss_close, pc_close) {
            (_, Err(_)) => Err("PUBLIC_VIDEO_PC_CLOSE_TIMEOUT"),
            (_, Ok(Err(_))) => Err("PUBLIC_VIDEO_PC_CLOSE_FAILED"),
            (Err(_), _) => Err("PUBLIC_VIDEO_WSS_CLOSE_TIMEOUT"),
            (Ok(Err(_)), _) => Err("PUBLIC_VIDEO_WSS_CLOSE_FAILED"),
            (Ok(Ok(())), Ok(Ok(()))) => Ok(()),
        };
        let result = PublicVideoLifecycleOutcome { reason, cleanup };
        outcome_tx.send_replace(Some(result));
        result
    };
    (handle, task)
}

async fn active(
    wss: &mut PublicVideoWss<'_>,
    pc: &dyn PeerConnection,
    commands: &mut mpsc::Receiver<Command>,
    stop: &mut watch::Receiver<bool>,
) -> PublicVideoEnd {
    let mut checked_track = false;
    let mut next_pair_poll = Instant::now();
    loop {
        // Do not race this read with ordinary commands. A canceled WSS read
        // consumes the relay connection; only terminal stop may cancel it.
        if let Err(end) = signal_poll(wss, stop).await {
            return end;
        }
        if Instant::now() >= next_pair_poll {
            if let Err(end) = until_stop(stop, async {
                if checked_track
                    || timeout(LOCAL_READ_TIMEOUT, has_rtp_track(pc))
                        .await
                        .map_err(|_| "PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE")??
                {
                    wss.check_live_video_pair(pc).await?;
                    checked_track = true;
                }
                Ok(())
            })
            .await
            {
                return end;
            }
            next_pair_poll = Instant::now() + PAIR_POLL;
        }
        match commands.try_recv() {
            Ok(Command::Send(packet, reply)) if !reply.is_closed() => {
                match until_stop(stop, wss.send_checked_rtp(pc, packet)).await {
                    Ok(()) => {
                        checked_track = true;
                        let _ = reply.send(Ok(()));
                    }
                    Err(end) => {
                        let _ = reply.send(Err(command_error(end)));
                        return end;
                    }
                }
            }
            Ok(Command::Receive(reply)) if !reply.is_closed() => {
                match until_stop(stop, wss.recv_checked_rtp(pc)).await {
                    Ok(packet) => {
                        checked_track = true;
                        // A packet read just before signaling loss/stop must
                        // not be handed to a caller that might present it.
                        if let Err(end) = signal_poll(wss, stop).await {
                            let _ = reply.send(Err(command_error(end)));
                            return end;
                        }
                        // WSS intentionally uses one code for a short idle
                        // poll and terminal scope expiry. Rechecking the live
                        // gate also distinguishes those before returning RTP.
                        if let Err(end) = until_stop(stop, async {
                            wss.check_live_video_pair(pc).await.map(|_| ())
                        })
                        .await
                        {
                            let _ = reply.send(Err(command_error(end)));
                            return end;
                        }
                        let _ = reply.send(Ok(packet));
                    }
                    Err(end) => {
                        let _ = reply.send(Err(command_error(end)));
                        return end;
                    }
                }
            }
            Ok(_) | Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return PublicVideoEnd::ControllerDropped
            }
        }
    }
}

async fn signal_poll(
    wss: &mut PublicVideoWss<'_>,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), PublicVideoEnd> {
    let event = until_stop(stop, async {
        match wss.next_event(SIGNAL_POLL).await {
            // The WSS API uses the same code for idle polls and the terminal
            // expiry. A terminal expiry consumes WSS; the next read or media
            // check observes Closed and can never forward a packet.
            Err(SCOPE_OR_POLL_TIMEOUT) => Ok(None),
            Err(error) => Err(error),
            Ok(event) => Ok(Some(event)),
        }
    })
    .await?;
    match event {
        None => Ok(()),
        Some(PublicVideoWssEvent::PeerLeft) => Err(PublicVideoEnd::PeerLeft),
        Some(PublicVideoWssEvent::Signal { .. }) => Err(PublicVideoEnd::Error(
            "PUBLIC_VIDEO_SESSION_UNEXPECTED_SIGNAL",
        )),
    }
}

async fn has_rtp_track(pc: &dyn PeerConnection) -> Result<bool, &'static str> {
    const UNAVAILABLE: &str = "PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE";
    let transceivers = pc.get_transceivers().await;
    let [video] = transceivers.as_slice() else {
        return Err(UNAVAILABLE);
    };
    match video.current_direction().await.map_err(|_| UNAVAILABLE)? {
        RTCRtpTransceiverDirection::Sendonly => Ok(true),
        // This wrapper exposes a viewer receiver on the first incoming RTP.
        RTCRtpTransceiverDirection::Recvonly => {
            Ok(video.receiver().await.map_err(|_| UNAVAILABLE)?.is_some())
        }
        _ => Err(UNAVAILABLE),
    }
}

async fn until_stop<T>(
    stop: &mut watch::Receiver<bool>,
    work: impl Future<Output = Result<T, &'static str>>,
) -> Result<T, PublicVideoEnd> {
    if *stop.borrow() {
        return Err(PublicVideoEnd::Stopped);
    }
    tokio::select! {
        biased;
        changed = stop.changed() => Err(if changed.is_ok() {
            PublicVideoEnd::Stopped
        } else {
            PublicVideoEnd::ControllerDropped
        }),
        value = work => {
            // Be defensive even if the other thread sent stop while work's
            // final poll was executing.
            if *stop.borrow() {
                Err(PublicVideoEnd::Stopped)
            } else {
                value.map_err(PublicVideoEnd::Error)
            }
        }
    }
}

fn command_error(end: PublicVideoEnd) -> &'static str {
    match end {
        PublicVideoEnd::Error(error) => error,
        _ => CLOSED,
    }
}
