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

#[cfg(windows)]
pub(crate) static GATE: ProbeGate = ProbeGate::new();

/// A cross-process lease shared by the CLI and the local video stream that owns the desktop.
#[cfg(windows)]
pub(crate) struct ProcessLease(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl ProcessLease {
    pub(crate) fn acquire() -> Result<Self, EncodeError> {
        use windows::{
            core::w,
            Win32::{
                Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS},
                System::Threading::CreateMutexW,
            },
        };
        unsafe {
            // Preserve the original mutex name for compatibility with an older P0-03 CLI.
            let handle = CreateMutexW(None, true, w!("Local\\RemoteHub.P003.EncodeProbe"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = CloseHandle(handle);
                return Err(EncodeError::Busy);
            }
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessLease {
    fn drop(&mut self) {
        use windows::Win32::{Foundation::CloseHandle, System::Threading::ReleaseMutex};
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
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
    fn live_config_is_narrower_than_the_p003_file_probe() {
        let c = EncoderConfig::default();
        assert!(c.validate_stream(true).is_ok());
        assert!(matches!(
            c.validate_stream(false),
            Err(EncodeError::ConsentRequired)
        ));
        let short = EncoderConfig { seconds: 2, ..c };
        assert!(short.validate(true).is_ok());
        assert!(short.validate_stream(true).is_err());
        assert!(EncoderConfig {
            bitrate: 3_000_000,
            ..c
        }
        .validate_stream(true)
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
