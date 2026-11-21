use crate::{h264::EncodedVideoFrame, lifecycle::StopToken, EncodeError};

#[derive(Default)]
pub(crate) struct Output {
    last_pts: Option<i64>,
    pub(crate) frames: u32,
    pub(crate) bytes: u64,
}

impl Output {
    pub(crate) fn publish(
        &mut self,
        frames: Vec<EncodedVideoFrame>,
        stop: &StopToken,
        on_frame: &mut impl FnMut(EncodedVideoFrame) -> Result<(), EncodeError>,
    ) -> Result<(), EncodeError> {
        for frame in frames {
            if stop.is_stopped() {
                break;
            }
            if frame.pts_100ns < 0
                || frame.duration_100ns <= 0
                || self.last_pts.is_some_and(|last| frame.pts_100ns <= last)
            {
                return Err(EncodeError::Native(
                    "invalid or non-monotonic MFT output time".into(),
                ));
            }
            // Starting with a predictive frame would leave a fresh decoder without references.
            if self.frames == 0
                && !(frame.is_idr && frame.nal_types.contains(&7) && frame.nal_types.contains(&8))
            {
                return Err(EncodeError::Bitstream("first live AU lacks SPS/PPS/IDR"));
            }
            let pts = frame.pts_100ns;
            let bytes = frame.annex_b.len() as u64;
            // No waits or drop-and-continue: a sink error terminates this entire run.
            on_frame(frame)?;
            self.last_pts = Some(pts);
            self.frames += 1;
            self.bytes += bytes;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(pts: i64, idr: bool) -> EncodedVideoFrame {
        EncodedVideoFrame {
            annex_b: vec![0, 0, 0, 1, 0x65, 0x80],
            pts_100ns: pts,
            duration_100ns: 333_333,
            nal_types: if idr { vec![7, 8, 5] } else { vec![1] },
            is_idr: idr,
            encode_elapsed_ms: 1.0,
        }
    }

    #[test]
    fn output_aborts_on_backpressure_without_delivering_later_aus() {
        let mut output = Output::default();
        let stop = StopToken::default();
        let mut visited = Vec::new();
        let result = output.publish(
            vec![frame(0, true), frame(333_333, false), frame(666_666, false)],
            &stop,
            &mut |au| {
                visited.push(au.pts_100ns);
                if visited.len() == 2 {
                    return Err(EncodeError::Backpressure);
                }
                Ok(())
            },
        );
        assert!(matches!(result, Err(EncodeError::Backpressure)));
        assert_eq!(visited, [0, 333_333]);
        assert_eq!(output.frames, 1);
    }

    #[test]
    fn output_rejects_non_idr_start_and_reordered_timestamps() {
        let mut output = Output::default();
        let stop = StopToken::default();
        assert!(matches!(
            output.publish(vec![frame(0, false)], &stop, &mut |_| Ok(())),
            Err(EncodeError::Bitstream(_))
        ));
        assert!(matches!(
            output.publish(
                vec![frame(5, true), frame(4, false)],
                &stop,
                &mut |_| Ok(())
            ),
            Err(EncodeError::Native(_))
        ));
        assert_eq!(output.frames, 1);
    }

    #[test]
    fn output_publishes_nothing_after_stop() {
        let mut output = Output::default();
        let stop = StopToken::default();
        output
            .publish(
                vec![frame(0, true), frame(333_333, false)],
                &stop,
                &mut |_| {
                    stop.stop();
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(output.frames, 1);
    }
}
