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
    mut h: PublicVideoWss<'_>,
    mut v: PublicVideoWss<'_>,
) {
    use hub_webrtc::public_video_lifecycle::{supervise_video, PublicVideoEnd};
    use rtc::rtp::{codec::h264::H264Payloader, packetizer::Payloader};

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
    // This small all-intra fixture fits in one RTP STAP-A; each repeated
    // key picture is independently decodable, including its SPS/PPS.
    let mut stap = vec![0x78];
    for payload in payloads {
        match payload[0] & 31 {
            24 => stap.extend_from_slice(&payload[1..]),
            1..=23 => {
                stap.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                stap.extend_from_slice(&payload);
            }
            _ => panic!("unexpected fragmented test picture"),
        }
    }
    assert!(stap.len() <= 1188);
    let SignedVideoPeers {
        host,
        viewer,
        mut viewer_tracks,
        _nat,
    } = negotiate_signed_video(&mut h, &mut v).await;
    let (source, host_driver) = supervise_video(h, host);
    let (sink, viewer_driver) = supervise_video(v, viewer);
    let staged = async {
        let mut display = sink.into_display_reader(42);
        let picture = |seq: u16| Packet {
            header: Header {
                timestamp: u32::from(seq) * 3000,
                ..synthetic_rtp(seq).header
            },
            payload: stap.clone().into(),
        };
        source.send_rtp(picture(1)).await.unwrap();
        timeout(WAIT, viewer_tracks.recv()).await.unwrap().unwrap();
        source.send_rtp(picture(2)).await.unwrap();
        let frame = display.next_frame().await.unwrap();
        assert_eq!(frame.sequence, 0);
        assert_eq!(frame.timestamp_us, 0);
        assert!(frame.keyframe);
        display.stop();
        assert!(display.next_frame().await.is_err());
        frame.annex_b
    };
    let (host_end, viewer_end, annex_b) = timeout(WAIT, async {
        tokio::join!(host_driver, viewer_driver, staged)
    })
    .await
    .unwrap();
    assert_eq!(viewer_end.reason, PublicVideoEnd::Stopped);
    assert_eq!(viewer_end.cleanup, Ok(()));
    assert_eq!(host_end.reason, PublicVideoEnd::PeerLeft);
    assert_eq!(host_end.cleanup, Ok(()));

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
