#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseAction {
    Hide,
    Exit,
}

#[derive(Default)]
pub(crate) struct Policy {
    pub(crate) tray_available: bool,
    pub(crate) resident: bool,
    pub(crate) exiting: bool,
}

impl Policy {
    pub(crate) fn set_resident(&mut self, enabled: bool, busy: bool) -> Result<(), &'static str> {
        if self.exiting {
            return Err("APPLICATION_EXITING");
        }
        if enabled && (!self.tray_available || busy) {
            return Err("TRAY_RESIDENCY_UNAVAILABLE");
        }
        self.resident = enabled;
        Ok(())
    }

    pub(crate) fn close_action(&self, busy: bool) -> CloseAction {
        if self.resident && self.tray_available && !busy && !self.exiting {
            CloseAction::Hide
        } else {
            CloseAction::Exit
        }
    }

    pub(crate) fn begin_exit(&mut self) -> bool {
        if self.exiting {
            return false;
        }
        self.exiting = true;
        self.resident = false;
        true
    }

    pub(crate) fn allow_capture(&self) -> Result<(), &'static str> {
        if self.exiting || self.resident {
            Err("APPLICATION_NOT_FOREGROUND_ONLY")
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_close_exits_and_residency_is_not_inherited() {
        let mut p = Policy::default();
        assert_eq!(p.close_action(false), CloseAction::Exit);
        assert!(p.set_resident(true, false).is_err());
        p.tray_available = true;
        p.set_resident(true, false).unwrap();
        p.set_resident(true, false).unwrap();
        assert_eq!(p.close_action(false), CloseAction::Hide);
        assert_eq!(Policy::default().close_action(false), CloseAction::Exit);
    }
    #[test]
    fn busy_capture_cannot_enable_or_hide_to_tray() {
        let mut p = Policy {
            tray_available: true,
            ..Policy::default()
        };
        assert!(p.set_resident(true, true).is_err());
        assert!(!p.resident);
        p.set_resident(true, false).unwrap();
        assert_eq!(p.close_action(true), CloseAction::Exit);
        assert!(p.allow_capture().is_err());
    }
    #[test]
    fn repeated_exit_is_idempotent_and_never_resurrects() {
        let mut p = Policy {
            tray_available: true,
            resident: true,
            exiting: false,
        };
        assert!(p.begin_exit());
        assert!(!p.begin_exit());
        assert!(!p.resident);
        assert!(p.set_resident(true, false).is_err());
        assert!(p.allow_capture().is_err());
        assert_eq!(p.close_action(false), CloseAction::Exit);
    }
    #[test]
    fn disabling_residency_restores_foreground_only_policy() {
        let mut p = Policy {
            tray_available: true,
            ..Policy::default()
        };
        p.set_resident(true, false).unwrap();
        p.set_resident(false, false).unwrap();
        p.set_resident(false, false).unwrap();
        assert!(p.allow_capture().is_ok());
        assert_eq!(p.close_action(false), CloseAction::Exit);
        p.tray_available = false;
        assert_eq!(p.close_action(false), CloseAction::Exit);
    }
}
