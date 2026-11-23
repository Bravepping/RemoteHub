//! Bounded media ports for the signed scoped WSS actor. These ports neither
//! capture a desktop nor decode/present it, and must not be exposed to IPC.

pub use crate::public_video_frames::VerifiedVideoFrame;
use crate::{
    public_video::PublicVideoScopedIce,
    public_video_frames::{
        PublicVideoFrameAssembler, MAX_VIDEO_AU_BYTES, MAX_VIDEO_AU_PACKETS,
        MAX_VIDEO_RTP_PAYLOAD_BYTES,
    },
    public_video_wss::{wall_seconds, Role, ScopedWssState},
    video::SSRC,
};
use rtc::rtp::packet::Packet;
use std::time::Duration;
use tokio::{
    sync::{mpsc, watch},
    time::{timeout, Instant},
};

const CLOSED: &str = "PUBLIC_VIDEO_MEDIA_CLOSED";
const BAD_AU: &str = "PUBLIC_VIDEO_MEDIA_INVALID_AU";
const BACKPRESSURE: &str = "PUBLIC_VIDEO_MEDIA_BACKPRESSURE";
const UNAVAILABLE: &str = "PUBLIC_VIDEO_ICE_PAIR_UNAVAILABLE";
const START_WAIT: Duration = Duration::from_secs(8);
const IDLE: Duration = Duration::from_millis(100);

enum Port {
    Host(mpsc::Receiver<Vec<Packet>>),
    Viewer(mpsc::Sender<VerifiedVideoFrame>),
}

/// An opaque one-role media endpoint for a single scoped actor.
pub struct ScopedVideoMedia {
    port: Port,
    revoke: watch::Sender<bool>,
}

pub struct ScopedVideoSender {
    queue: mpsc::Sender<Vec<Packet>>,
    closed: watch::Receiver<bool>,
}

pub struct ScopedVideoReceiver {
    queue: mpsc::Receiver<VerifiedVideoFrame>,
    closed: watch::Receiver<bool>,
}

impl ScopedVideoMedia {
    pub fn host() -> (Self, ScopedVideoSender) {
        let (tx, rx) = mpsc::channel(2);
        let (revoke, closed) = watch::channel(false);
        (
            Self {
                port: Port::Host(rx),
                revoke,
            },
            ScopedVideoSender { queue: tx, closed },
        )
    }

    pub fn viewer() -> (Self, ScopedVideoReceiver) {
        let (tx, rx) = mpsc::channel(3);
        let (revoke, closed) = watch::channel(false);
        (
            Self {
                port: Port::Viewer(tx),
                revoke,
            },
            ScopedVideoReceiver { queue: rx, closed },
        )
    }

    pub(crate) fn role(&self) -> Role {
        match self.port {
            Port::Host(_) => Role::Host,
            Port::Viewer(_) => Role::Viewer,
        }
    }

    pub(crate) fn revoke(&self) {
        self.revoke.send_replace(true);
    }

    pub(crate) async fn pump(
        &mut self,
        ice: &mut PublicVideoScopedIce<'_>,
        state: &watch::Sender<ScopedWssState>,
    ) -> Result<(), &'static str> {
        match &mut self.port {
            Port::Host(au) => run_host(ice, state, au).await,
            Port::Viewer(frames) => run_viewer(ice, frames).await,
        }
    }
}

impl Drop for ScopedVideoMedia {
    fn drop(&mut self) {
        self.revoke();
    }
}

impl ScopedVideoSender {
    /// Preflight the entire AU before admitting its first packet. A single
    /// native producer should drive this bounded queue, awaiting backpressure.
    pub async fn send_access_unit(&mut self, packets: Vec<Packet>) -> Result<(), &'static str> {
        if *self.closed.borrow() {
            return Err(CLOSED);
        }
        validate_access_unit(&packets)?;
        tokio::select! {
            biased;
            _ = self.closed.wait_for(|closed| *closed) => Err(CLOSED),
            result = self.queue.send(packets) => result.map_err(|_| CLOSED),
        }
    }
}

impl ScopedVideoReceiver {
    /// A well-formed access unit from an admitted RTP stream, not a decoded
    /// or presented frame. Queued frames are not returned after revoke.
    pub async fn next_frame(&mut self) -> Result<VerifiedVideoFrame, &'static str> {
        let frame = tokio::select! {
            biased;
            _ = self.closed.wait_for(|closed| *closed) => return Err(CLOSED),
            frame = self.queue.recv() => frame.ok_or(CLOSED)?,
        };
        if *self.closed.borrow() {
            return Err(CLOSED);
        }
        Ok(frame)
    }
}

fn validate_access_unit(packets: &[Packet]) -> Result<(), &'static str> {
    let first = packets.first().ok_or(BAD_AU)?;
    if packets.len() > MAX_VIDEO_AU_PACKETS {
        return Err(BAD_AU);
    }
    let mut bytes = 0;
    for (index, packet) in packets.iter().enumerate() {
        let h = &packet.header;
        if h.version != 2
            || h.payload_type != 102
            || h.ssrc != SSRC
            || h.timestamp != first.header.timestamp
            || h.sequence_number != first.header.sequence_number.wrapping_add(index as u16)
            || h.marker != (index + 1 == packets.len())
            || h.extension
            || h.padding
            || !h.csrc.is_empty()
            || !(1..=MAX_VIDEO_RTP_PAYLOAD_BYTES).contains(&packet.payload.len())
        {
            return Err(BAD_AU);
        }
        bytes += packet.payload.len();
    }
    if bytes > MAX_VIDEO_AU_BYTES {
        return Err(BAD_AU);
    }
    Ok(())
}

async fn run_host(
    ice: &mut PublicVideoScopedIce<'_>,
    state: &watch::Sender<ScopedWssState>,
    queue: &mut mpsc::Receiver<Vec<Packet>>,
) -> Result<(), &'static str> {
    let initial_until = Instant::now() + START_WAIT;
    let mut route = None;
    loop {
        let path = match ice.selected_path(wall_seconds()?).await {
            Ok(path) => path,
            Err(UNAVAILABLE) if route.is_none() && Instant::now() < initial_until => {
                tokio::time::sleep(Duration::from_millis(10)).await;
                continue;
            }
            Err(error) => return Err(error),
        };
        let key = (path.route, path.local_type, path.remote_type);
        if route != Some(key) {
            route = Some(key);
            state.send_replace(ScopedWssState::HostPathVerified(key.0, key.1, key.2));
        }
        match timeout(IDLE, queue.recv()).await {
            Ok(Some(packets)) => {
                for (index, packet) in packets.into_iter().enumerate() {
                    // Yield within a large IDR, not only between AUs, so
                    // a continuously ready sender can't starve WSS or Stop.
                    if index % 8 == 0 {
                        tokio::task::yield_now().await;
                    }
                    ice.send_rtp(packet, wall_seconds()?).await?;
                }
            }
            Ok(None) => return Ok(()),
            Err(_) => {}
        }
    }
}

async fn run_viewer(
    ice: &mut PublicVideoScopedIce<'_>,
    frames: &mpsc::Sender<VerifiedVideoFrame>,
) -> Result<(), &'static str> {
    let initial_until = Instant::now() + START_WAIT;
    let mut assembler = PublicVideoFrameAssembler::new(SSRC);
    let mut seen_frame = false;
    loop {
        if frames.is_closed() {
            return Ok(());
        }
        if !seen_frame && Instant::now() >= initial_until {
            return Err(UNAVAILABLE);
        }
        if let Some(packet) = ice.recv_rtp(wall_seconds()?).await? {
            if let Some(frame) = assembler.push(&packet)? {
                frames.try_send(frame).map_err(|_| BACKPRESSURE)?;
                seen_frame = true;
            }
        } else if !seen_frame {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::task::yield_now().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revoked_viewer_does_not_hand_out_an_already_queued_frame() {
        let (media, mut receiver) = ScopedVideoMedia::viewer();
        let Port::Viewer(queue) = &media.port else {
            panic!("viewer queue")
        };
        queue
            .try_send(VerifiedVideoFrame {
                sequence: 0,
                timestamp_us: 0,
                keyframe: true,
                annex_b: vec![0; 16],
            })
            .ok()
            .unwrap();
        media.revoke();
        assert_eq!(receiver.next_frame().await.err(), Some(CLOSED));
    }

    #[tokio::test]
    async fn malformed_au_is_never_partly_enqueued() {
        let (mut media, mut sender) = ScopedVideoMedia::host();
        let mut packet = Packet::default();
        packet.header.version = 2;
        packet.header.payload_type = 102;
        packet.header.ssrc = SSRC;
        packet.payload = vec![0x65, 1].into();
        // Missing terminal marker.
        assert_eq!(sender.send_access_unit(vec![packet]).await, Err(BAD_AU));
        let Port::Host(queue) = &mut media.port else {
            panic!("host queue")
        };
        assert!(queue.try_recv().is_err());
    }
}
