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
use std::{collections::HashMap, time::Duration};
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
const VIEWER_QUEUE_WAIT: Duration = Duration::from_millis(150);
const RTP_GAP: &str = "PUBLIC_VIDEO_RTP_GAP";
const REORDER_PACKETS: u16 = 48;
const REORDER_WAIT: Duration = Duration::from_millis(80);

enum Port {
    Host(mpsc::Receiver<Vec<Packet>>),
    Viewer(mpsc::Sender<VerifiedVideoFrame>),
}

/// An opaque one-role media endpoint for a single scoped actor.
pub struct ScopedVideoMedia {
    port: Port,
    revoke: watch::Sender<bool>,
    first_host_video: Option<watch::Sender<bool>>,
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
                first_host_video: None,
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
                first_host_video: None,
            },
            ScopedVideoReceiver { queue: rx, closed },
        )
    }

    /// One-shot notification after a complete host AU has been sent to the
    /// verified transport. This does not attest that the peer displayed it.
    #[cfg(feature = "v2-public-control")]
    pub fn notify_first_host_video(&mut self, sender: watch::Sender<bool>) {
        debug_assert_eq!(self.role(), Role::Host);
        self.first_host_video = Some(sender);
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
            Port::Host(au) => run_host(ice, state, au, &mut self.first_host_video).await,
            Port::Viewer(frames) => run_viewer(ice, state, frames).await,
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
    #[cfg(test)]
    pub(crate) fn fixture_has_queued_frame(&self) -> bool {
        !self.queue.is_empty()
    }

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
    first_video: &mut Option<watch::Sender<bool>>,
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
                if let Some(first_video) = first_video.take() {
                    first_video.send_replace(true);
                }
            }
            Ok(None) => return Ok(()),
            Err(_) => {}
        }
    }
}

async fn run_viewer(
    ice: &mut PublicVideoScopedIce<'_>,
    state: &watch::Sender<ScopedWssState>,
    frames: &mpsc::Sender<VerifiedVideoFrame>,
) -> Result<(), &'static str> {
    let initial_until = Instant::now() + START_WAIT;
    let mut assembler = PublicVideoFrameAssembler::new(SSRC);
    let mut order = RtpOrder::default();
    let mut seen_frame = false;
    let mut route = None;
    loop {
        if frames.is_closed() {
            return Ok(());
        }
        if !seen_frame && Instant::now() >= initial_until {
            return Err(UNAVAILABLE);
        }
        order.check_deadline(Instant::now())?;
        if let Some((packet, path)) = ice.recv_rtp(wall_seconds()?).await? {
            let key = (path.route, path.local_type, path.remote_type);
            if route != Some(key) {
                route = Some(key);
                state.send_replace(ScopedWssState::ViewerPathVerified(key.0, key.1, key.2));
            }
            // recv_rtp has already checked the signed live path, SSRC, PT
            // and datagram size. Do not feed arrival order into the strict AU parser.
            order.insert(packet, Instant::now())?;
            while let Some(packet) = order.pop() {
                if let Some(frame) = assembler.push(&packet)? {
                    if !enqueue_viewer_frame(frames, frame, || ice.check_current(wall_seconds()?))
                        .await?
                    {
                        return Ok(());
                    }
                    seen_frame = true;
                }
            }
        } else if !seen_frame {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::task::yield_now().await;
    }
}

/// Hold at most one additional complete AU while waiting for the existing
/// bounded queue. The owning WSS actor can cancel this wait on stop or leave.
async fn enqueue_viewer_frame(
    frames: &mpsc::Sender<VerifiedVideoFrame>,
    frame: VerifiedVideoFrame,
    check_scope: impl FnOnce() -> Result<(), &'static str>,
) -> Result<bool, &'static str> {
    match timeout(VIEWER_QUEUE_WAIT, frames.reserve()).await {
        Ok(Ok(permit)) => {
            check_scope()?;
            permit.send(frame);
            Ok(true)
        }
        Ok(Err(_)) => Ok(false),
        Err(_) => Err(BACKPRESSURE),
    }
}

/// A short reorder window, not loss recovery. Never skip a missing packet
/// or resync the strict assembler on a partial access unit.
#[derive(Default)]
struct RtpOrder {
    next: Option<u16>,
    pending: HashMap<u16, Packet>,
    gap_since: Option<Instant>,
}

impl RtpOrder {
    fn check_deadline(&self, now: Instant) -> Result<(), &'static str> {
        if self
            .gap_since
            .is_some_and(|since| now.saturating_duration_since(since) >= REORDER_WAIT)
        {
            return Err(RTP_GAP);
        }
        Ok(())
    }

    fn insert(&mut self, packet: Packet, now: Instant) -> Result<(), &'static str> {
        self.check_deadline(now)?;
        let seq = packet.header.sequence_number;
        let expected = *self.next.get_or_insert(seq);
        let ahead = seq.wrapping_sub(expected);
        if ahead > REORDER_PACKETS {
            // Ignore only recent already-consumed sequence numbers. Far or
            // half-space jumps are ambiguous and still terminate the session.
            return if expected.wrapping_sub(seq) <= REORDER_PACKETS {
                Ok(())
            } else {
                Err(RTP_GAP)
            };
        }
        self.pending.entry(seq).or_insert(packet);
        if ahead != 0 {
            self.gap_since.get_or_insert(now);
        }
        Ok(())
    }

    fn pop(&mut self) -> Option<Packet> {
        let seq = self.next?;
        let packet = self.pending.remove(&seq)?;
        self.next = Some(seq.wrapping_add(1));
        if self.pending.is_empty() {
            self.gap_since = None;
        }
        Some(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_frame(sequence: u64) -> VerifiedVideoFrame {
        VerifiedVideoFrame {
            sequence,
            timestamp_us: sequence * 40_000,
            keyframe: sequence == 0,
            annex_b: vec![0; 16],
        }
    }

    #[tokio::test]
    async fn transient_viewer_queue_pressure_preserves_frame_order() {
        let (media, mut receiver) = ScopedVideoMedia::viewer();
        let Port::Viewer(queue) = &media.port else {
            panic!("viewer queue")
        };
        for sequence in 0..3 {
            assert!(
                enqueue_viewer_frame(queue, complete_frame(sequence), || Ok(()))
                    .await
                    .unwrap()
            );
        }
        let waiting = enqueue_viewer_frame(queue, complete_frame(3), || Ok(()));
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        assert_eq!(receiver.next_frame().await.unwrap().sequence, 0);
        assert_eq!(waiting.await, Ok(true));
        for expected in 1..=3 {
            assert_eq!(receiver.next_frame().await.unwrap().sequence, expected);
        }
    }

    #[tokio::test]
    async fn stalled_viewer_queue_times_out_without_dropping_and_resuming() {
        let (tx, mut rx) = mpsc::channel(1);
        assert!(enqueue_viewer_frame(&tx, complete_frame(0), || Ok(()))
            .await
            .unwrap());
        assert_eq!(
            enqueue_viewer_frame(&tx, complete_frame(1), || panic!("no permit")).await,
            Err(BACKPRESSURE)
        );
        assert_eq!(rx.try_recv().unwrap().sequence, 0);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn capacity_return_does_not_bypass_revoked_scope() {
        let (tx, mut rx) = mpsc::channel(1);
        assert!(enqueue_viewer_frame(&tx, complete_frame(0), || Ok(()))
            .await
            .unwrap());
        let waiting = enqueue_viewer_frame(&tx, complete_frame(1), || Err("test scope ended"));
        tokio::pin!(waiting);
        assert!(futures_util::poll!(&mut waiting).is_pending());
        assert_eq!(rx.try_recv().unwrap().sequence, 0);
        assert_eq!(waiting.await, Err("test scope ended"));
        assert!(rx.try_recv().is_err());
    }

    fn packet(seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Packet {
        let mut packet = Packet::default();
        packet.header.version = 2;
        packet.header.payload_type = 102;
        packet.header.ssrc = SSRC;
        packet.header.sequence_number = seq;
        packet.header.timestamp = timestamp;
        packet.header.marker = marker;
        packet.payload = bytes::Bytes::copy_from_slice(payload);
        packet
    }

    #[test]
    fn verified_late_fragment_is_reordered_before_strict_assembly_across_wrap() {
        let now = Instant::now();
        let mut order = RtpOrder::default();
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        let first = packet(
            u16::MAX,
            0,
            true,
            &[0x78, 0, 2, 0x67, 7, 0, 2, 0x68, 8, 0, 2, 0x65, 5],
        );
        order.insert(first.clone(), now).unwrap();
        let frame = assembler.push(&order.pop().unwrap()).unwrap().unwrap();
        assert!(frame.keyframe);
        // This marker arrives before the start fragment. The strict parser
        // would terminate if we delivered it before sequence zero.
        let end = packet(1, 9000, true, &[0x7c, 0x41, 3, 1]);
        assert_eq!(assembler.push(&end).err(), Some(RTP_GAP));
        let mut assembler = PublicVideoFrameAssembler::new(SSRC);
        assembler.push(&first).unwrap().unwrap();
        order.insert(end.clone(), now).unwrap();
        order.insert(end, now + Duration::from_millis(2)).unwrap();
        assert!(order.pop().is_none());
        order.insert(first, now).unwrap(); // Recent duplicate, across wrap.
        assert!(order.pop().is_none());
        order
            .insert(
                packet(0, 9000, false, &[0x7c, 0x81, 3, 0, 0]),
                now + Duration::from_millis(20),
            )
            .unwrap();
        assert!(assembler.push(&order.pop().unwrap()).unwrap().is_none());
        let frame = assembler.push(&order.pop().unwrap()).unwrap().unwrap();
        assert_eq!(frame.sequence, 1);
        assert_eq!(frame.annex_b, [0, 0, 0, 1, 0x61, 3, 0, 0, 3, 1]);
        assert!(order.pop().is_none());
        assert_eq!(order.check_deadline(now + REORDER_WAIT), Ok(()));
    }

    #[test]
    fn missing_fragment_is_not_released_or_allowed_to_extend_its_deadline() {
        let now = Instant::now();
        let mut order = RtpOrder::default();
        order.insert(packet(10, 0, true, &[0x61, 5]), now).unwrap();
        assert_eq!(order.pop().unwrap().header.sequence_number, 10);
        order.insert(packet(12, 0, true, &[0x61, 5]), now).unwrap();
        order
            .insert(
                packet(13, 0, true, &[0x61, 5]),
                now + REORDER_WAIT - Duration::from_millis(1),
            )
            .unwrap();
        assert!(order.pop().is_none());
        assert_eq!(order.check_deadline(now + REORDER_WAIT), Err(RTP_GAP));
        assert_eq!(
            order.insert(packet(11, 0, true, &[0x61, 5]), now + REORDER_WAIT),
            Err(RTP_GAP)
        );
        assert!(order.pop().is_none());
    }

    #[test]
    fn order_window_cannot_buffer_an_unbounded_sequence_jump() {
        let now = Instant::now();
        let mut order = RtpOrder::default();
        order.insert(packet(30, 0, true, &[0x61, 5]), now).unwrap();
        order.pop().unwrap();
        for seq in (32..=31 + REORDER_PACKETS).rev() {
            order.insert(packet(seq, 0, true, &[0x61, 5]), now).unwrap();
        }
        assert_eq!(order.pending.len(), usize::from(REORDER_PACKETS));
        assert_eq!(
            order.insert(packet(32 + REORDER_PACKETS, 0, true, &[0x61, 5]), now),
            Err(RTP_GAP)
        );
        assert!(order.pop().is_none());
    }

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
