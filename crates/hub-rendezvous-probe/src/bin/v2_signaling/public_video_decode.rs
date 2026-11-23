//! Opt-in local pixel validation for the signed RTP fixture; no product codec dependency.
use super::*;

const FRAME_WIDTH: usize = 32;
const FRAME_HEIGHT: usize = 32;
const FRAME_LUMA: usize = FRAME_WIDTH * FRAME_HEIGHT;

fn four_quadrants_yuv420() -> Vec<u8> {
    let mut frame = vec![128; FRAME_LUMA * 3 / 2];
    let quadrants = [36, 94, 154, 218];
    for y in 0..FRAME_HEIGHT {
        for x in 0..FRAME_WIDTH {
            let row = usize::from(y >= FRAME_HEIGHT / 2);
            let col = usize::from(x >= FRAME_WIDTH / 2);
            frame[y * FRAME_WIDTH + x] = quadrants[row * 2 + col];
        }
    }
    frame
}

fn ffmpeg_fixture(args: &[&str], input: &[u8]) -> Vec<u8> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut child = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("install FFmpeg with libx264 to run the optional local decoder test");
    assert!(input.len() <= 16 * 1024);
    let mut pipe = child.stdin.take().unwrap();
    let written = pipe.write_all(input);
    drop(pipe);
    let output = child.wait_with_output().expect("wait for FFmpeg fixture");
    assert!(
        output.status.success(),
        "FFmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    written.expect("feed complete FFmpeg fixture");
    output.stdout
}

pub(super) async fn decode_pattern_round_trip(
    h: &mut PublicVideoWss<'_>,
    v: &mut PublicVideoWss<'_>,
) {
    use rtc::rtp::{
        codec::h264::{H264Packet, H264Payloader},
        packetizer::{Depacketizer, Payloader},
    };

    let expected = four_quadrants_yuv420();
    let encoded = ffmpeg_fixture(
        &[
            "-f",
            "rawvideo",
            "-pixel_format",
            "yuv420p",
            "-video_size",
            "32x32",
            "-framerate",
            "30",
            "-i",
            "pipe:0",
            "-frames:v",
            "1",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-tune",
            "zerolatency",
            "-profile:v",
            "baseline",
            "-level:v",
            "3.1",
            "-crf",
            "12",
            "-threads",
            "1",
            "-f",
            "h264",
            "pipe:1",
        ],
        &expected,
    );
    let payloads = H264Payloader::default()
        .payload(1188, &encoded.into())
        .unwrap();
    // The stream must include parameter sets plus an IDR, not just dummy transport bytes.
    assert!(payloads.len() >= 2 && payloads.len() <= 8);
    let SignedVideoPeers {
        host,
        viewer,
        mut viewer_tracks,
        _nat,
    } = negotiate_signed_video(h, v).await;
    // This verifies the wrapper's track-open boundary without depending on
    // whether it queues the very first RTP packet before exposing the track.
    h.send_checked_rtp(host.as_ref(), synthetic_rtp(1))
        .await
        .unwrap();
    timeout(WAIT, viewer_tracks.recv()).await.unwrap().unwrap();
    const FIRST_SEQUENCE: u16 = 2;
    const PICTURE_TIMESTAMP: u32 = 12_000;
    let count = payloads.len();
    for (index, payload) in payloads.into_iter().enumerate() {
        let seq = FIRST_SEQUENCE + index as u16;
        h.send_checked_rtp(
            host.as_ref(),
            Packet {
                header: Header {
                    timestamp: PICTURE_TIMESTAMP,
                    marker: index + 1 == count,
                    ..synthetic_rtp(seq).header
                },
                payload,
            },
        )
        .await
        .unwrap();
    }

    let mut annex_b = Vec::new();
    let mut unpacker = H264Packet::default();
    let mut next = FIRST_SEQUENCE;
    // At most one bootstrap packet can be queued in front of the picture.
    for _ in 0..=count {
        let packet = v.recv_checked_rtp(viewer.as_ref()).await.unwrap();
        if packet.header.sequence_number == 1 && next == FIRST_SEQUENCE {
            continue;
        }
        assert_eq!(packet.header.sequence_number, next);
        assert_eq!(packet.header.timestamp, PICTURE_TIMESTAMP);
        assert_eq!(packet.header.payload_type, 102);
        next += 1;
        let final_packet = next == FIRST_SEQUENCE + count as u16;
        assert_eq!(packet.header.marker, final_packet);
        annex_b.extend(unpacker.depacketize(&packet.payload).unwrap());
        if final_packet {
            break;
        }
    }
    assert_eq!(next, FIRST_SEQUENCE + count as u16);
    host.close().await.unwrap();
    viewer.close().await.unwrap();

    let actual = ffmpeg_fixture(
        &[
            "-xerror", "-threads", "1", "-f", "h264", "-i", "pipe:0", "-an", "-pix_fmt", "yuv420p",
            "-f", "rawvideo", "pipe:1",
        ],
        &annex_b,
    );
    assert_eq!(
        actual.len(),
        expected.len(),
        "expect one 32x32 YUV420 frame"
    );
    let largest_difference = expected
        .iter()
        .zip(&actual)
        .map(|(&want, &got)| want.abs_diff(got))
        .max()
        .unwrap();
    assert!(
        largest_difference <= 4,
        "decoded pixel delta: {largest_difference}"
    );
    eprintln!("verified one 32x32 YUV420 frame; maximum sample delta: {largest_difference}");
}

#[tokio::test]
#[ignore = "opt-in local encode/decode test requires FFmpeg with libx264"]
async fn public_video_wss_decodes_synthetic_pattern_pixels() {
    exchange(Scenario::LiveDecodedPattern).await;
}
