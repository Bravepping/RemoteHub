use crate::{EncodeError, EncoderConfig};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Default, Clone)]
pub struct StopToken(Arc<AtomicBool>);
impl StopToken {
    pub fn stop(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Default)]
pub struct ProbeGate(AtomicBool);
pub struct ProbeLease<'a>(&'a AtomicBool);
impl ProbeGate {
    pub const fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub fn enter(
        &self,
        consent: bool,
        config: &EncoderConfig,
    ) -> Result<ProbeLease<'_>, EncodeError> {
        config.validate(consent)?;
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| EncodeError::Busy)?;
        Ok(ProbeLease(&self.0))
    }
}
impl Drop for ProbeLease<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn consent_limits_repeated_start_stop_and_cleanup() {
        let gate = ProbeGate::default();
        let config = EncoderConfig::default();
        assert!(matches!(
            gate.enter(false, &config),
            Err(EncodeError::ConsentRequired)
        ));
        {
            let _lease = gate.enter(true, &config).unwrap();
            assert!(matches!(gate.enter(true, &config), Err(EncodeError::Busy)));
        }
        assert!(gate.enter(true, &config).is_ok());
        let stop = StopToken::default();
        let clone = stop.clone();
        clone.stop();
        assert!(stop.is_stopped());
        assert!(EncoderConfig {
            seconds: 31,
            ..config
        }
        .validate(true)
        .is_err());
        assert!(EncoderConfig { width: 1, ..config }.validate(true).is_err());
        assert!(EncoderConfig {
            bitrate: 0,
            ..config
        }
        .validate(true)
        .is_err());
    }
    #[test]
    fn fractional_frame_clock_never_drifts_or_reorders() {
        let c = EncoderConfig::default();
        for i in 0..900 {
            assert!(c.duration(i) > 0);
            assert_eq!(c.pts(i) + c.duration(i), c.pts(i + 1));
        }
        assert_eq!(c.pts(300), 100_000_000);
    }

    #[test]
    fn bounded_pacer_reuses_a_real_frame_for_short_stalls_and_reports_long_stalls() {
        let c = EncoderConfig::default();
        assert_eq!(c.schedule(0, std::time::Duration::ZERO), (0, 0, false));
        assert_eq!(
            c.schedule(0, std::time::Duration::from_millis(50)),
            (0, 0, true)
        );
        assert_eq!(
            c.schedule(0, std::time::Duration::from_millis(100)),
            (3, 3, true)
        );
        assert_eq!(
            c.schedule(3, std::time::Duration::from_millis(100)),
            (3, 0, false)
        );
    }
}
