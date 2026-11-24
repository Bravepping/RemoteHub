//! Explicitly approved, short native input sessions. No arbitrary command/text execution.
use serde::Deserialize;
#[cfg(all(windows, feature = "p3-lan"))]
use std::collections::BTreeSet;
use std::time::{Duration, Instant};
pub const LABEL: &str = "remote-input.v1";
#[derive(Deserialize, Clone)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Event {
    #[serde(rename = "mouse_move")]
    Move { x_norm: f64, y_norm: f64 },
    #[serde(rename = "mouse_button")]
    Button { button: u8, down: bool },
    #[serde(rename = "key")]
    Key { key: String, down: bool },
    #[serde(rename = "wheel")]
    Wheel { delta: i32 },
    #[serde(rename = "release_all")]
    Release {},
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    v: u8,
    seq: u64,
    session_id: String,
    event: Event,
}
pub trait Sink {
    fn apply(&mut self, event: &Event) -> Result<(), String>;
}
pub struct Protocol {
    session: String,
    expires: Instant,
    window: Instant,
    count: u32,
    seq: u64,
}
impl Protocol {
    pub fn new(session: String, seconds: u32) -> Self {
        let now = Instant::now();
        Self::with_deadline(session, now, now + Duration::from_secs(seconds as u64))
    }

    pub(super) fn with_deadline(session: String, now: Instant, expires: Instant) -> Self {
        Self {
            session,
            expires,
            window: now,
            count: 0,
            seq: 0,
        }
    }
    pub fn process(
        &mut self,
        bytes: &[u8],
        text: bool,
        permitted: bool,
        now: Instant,
        sink: &mut impl Sink,
    ) -> Result<u64, String> {
        let (seq, event) = self.validate_event(bytes, text, permitted, now)?;
        sink.apply(&event)?;
        Ok(self.commit_applied(seq))
    }

    pub(super) fn validate_event(
        &mut self,
        bytes: &[u8],
        text: bool,
        permitted: bool,
        now: Instant,
    ) -> Result<(u64, Event), String> {
        if !permitted || now >= self.expires {
            return Err("INPUT_NOT_AUTHORIZED".into());
        }
        if !text || bytes.is_empty() || bytes.len() > 512 {
            return Err("INPUT_MESSAGE_SIZE_OR_TYPE".into());
        }
        if now.duration_since(self.window) >= Duration::from_secs(1) {
            self.window = now;
            self.count = 0;
        }
        self.count += 1;
        if self.count > 120 || self.seq >= 1000 {
            return Err("INPUT_RATE_LIMIT".into());
        }
        let m: Envelope = serde_json::from_slice(bytes).map_err(|_| "INPUT_SCHEMA")?;
        if m.v != 1 || m.session_id != self.session || m.seq != self.seq + 1 {
            return Err("INPUT_SESSION_OR_SEQUENCE".into());
        }
        match &m.event {
            Event::Move { x_norm, y_norm }
                if !x_norm.is_finite()
                    || !y_norm.is_finite()
                    || !(0.0..=1.0).contains(x_norm)
                    || !(0.0..=1.0).contains(y_norm) =>
            {
                return Err("INPUT_COORDINATE".into())
            }
            Event::Button { button, .. } if *button > 2 => return Err("INPUT_BUTTON".into()),
            Event::Key { key, .. } if vk(key).is_none() => {
                return Err("INPUT_KEY_NOT_ALLOWED".into())
            }
            Event::Wheel { delta } if ![-120, 120].contains(delta) => {
                return Err("INPUT_WHEEL".into())
            }
            _ => (),
        }
        Ok((m.seq, m.event))
    }

    /// Only after the exact validated event has been successfully applied.
    pub(super) fn commit_applied(&mut self, seq: u64) -> u64 {
        debug_assert_eq!(seq, self.seq + 1);
        self.seq = seq;
        seq
    }
}
pub fn vk(code: &str) -> Option<u16> {
    let b = code.as_bytes();
    if b.len() == 4 && b.starts_with(b"Key") && b[3].is_ascii_uppercase() {
        return Some(b[3] as u16);
    }
    if b.len() == 6 && b.starts_with(b"Digit") && b[5].is_ascii_digit() {
        return Some(b[5] as u16);
    }
    Some(match code {
        "Space" => 0x20,
        "Enter" => 0x0d,
        "Backspace" => 0x08,
        "Tab" => 0x09,
        "Escape" => 0x1b,
        "Delete" => 0x2e,
        "ArrowLeft" => 0x25,
        "ArrowUp" => 0x26,
        "ArrowRight" => 0x27,
        "ArrowDown" => 0x28,
        "Home" => 0x24,
        "End" => 0x23,
        "ShiftLeft" | "ShiftRight" => 0x10,
        "ControlLeft" | "ControlRight" => 0x11,
        "Period" => 0xbe,
        "Comma" => 0xbc,
        "Slash" => 0xbf,
        "Semicolon" => 0xba,
        "Quote" => 0xde,
        "Minus" => 0xbd,
        "Equal" => 0xbb,
        _ => return None,
    })
}
pub fn map_point(
    x: f64,
    y: f64,
    primary: (i32, i32),
    virtual_rect: (i32, i32, i32, i32),
) -> Result<(i32, i32), String> {
    let (vx, vy, vw, vh) = virtual_rect;
    let (pw, ph) = primary;
    if pw < 2
        || ph < 2
        || vw < 2
        || vh < 2
        || vx > 0
        || vy > 0
        || vx + vw < pw
        || vy + vh < ph
        || !(0.0..=1.0).contains(&x)
        || !(0.0..=1.0).contains(&y)
    {
        return Err("INPUT_DISPLAY_CHANGED".into());
    }
    Ok((
        ((x * (pw - 1) as f64 - vx as f64) * 65535.0 / (vw - 1) as f64).round() as i32,
        ((y * (ph - 1) as f64 - vy as f64) * 65535.0 / (vh - 1) as f64).round() as i32,
    ))
}
#[cfg(all(windows, feature = "p3-lan"))]
#[derive(Default)]
pub struct Injector {
    keys: BTreeSet<u16>,
    buttons: BTreeSet<u8>,
}
#[cfg(all(windows, feature = "p3-lan"))]
impl Injector {
    fn send(input: windows_sys::Win32::UI::Input::KeyboardAndMouse::INPUT) -> Result<(), String> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        if unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) } != 1 {
            return Err("INPUT_SEND_BLOCKED".into());
        }
        Ok(())
    }
    fn key(key: u16, down: bool) -> Result<(), String> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        Self::send(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: key,
                    wScan: 0,
                    dwFlags: (if down { 0 } else { KEYEVENTF_KEYUP })
                        | if (0x21..=0x2e).contains(&key) {
                            KEYEVENTF_EXTENDEDKEY
                        } else {
                            0
                        },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
    }
    fn mouse(flags: u32, data: u32, dx: i32, dy: i32) -> Result<(), String> {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        Self::send(INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        })
    }
    pub fn release(&mut self) -> Result<(), String> {
        let mut failed = false;
        for key in self.keys.clone() {
            if Self::key(key, false).is_ok() {
                self.keys.remove(&key);
            } else {
                failed = true;
            }
        }
        for button in self.buttons.clone() {
            if self
                .apply(&Event::Button {
                    button,
                    down: false,
                })
                .is_err()
            {
                failed = true;
            }
        }
        if failed {
            Err("INPUT_RELEASE_FAILED".into())
        } else {
            Ok(())
        }
    }
    pub fn emergency_pressed() -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        unsafe {
            GetAsyncKeyState(VK_CONTROL as i32) < 0
                && GetAsyncKeyState(VK_MENU as i32) < 0
                && GetAsyncKeyState(VK_F12 as i32) < 0
        }
    }
}
#[cfg(all(windows, feature = "p3-lan"))]
impl Sink for Injector {
    fn apply(&mut self, event: &Event) -> Result<(), String> {
        use windows_sys::Win32::UI::{
            HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*,
        };
        match event {
            Event::Move { x_norm, y_norm } => {
                let old = unsafe {
                    SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
                };
                if old.is_null() {
                    return Err("INPUT_DPI_CONTEXT_FAILED".into());
                }
                let (p, v) = unsafe {
                    (
                        (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)),
                        (
                            GetSystemMetrics(SM_XVIRTUALSCREEN),
                            GetSystemMetrics(SM_YVIRTUALSCREEN),
                            GetSystemMetrics(SM_CXVIRTUALSCREEN),
                            GetSystemMetrics(SM_CYVIRTUALSCREEN),
                        ),
                    )
                };
                let point = map_point(*x_norm, *y_norm, p, v);
                unsafe {
                    SetThreadDpiAwarenessContext(old);
                }
                let (x, y) = point?;
                Self::mouse(
                    MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
                    0,
                    x,
                    y,
                )
            }
            Event::Key { key, down } => {
                let k = vk(key).ok_or("INPUT_KEY_NOT_ALLOWED")?;
                if *down {
                    if self.keys.contains(&k) {
                        return Ok(());
                    }
                    if unsafe { GetAsyncKeyState(k as i32) } < 0 {
                        return Err("INPUT_LOCAL_KEY_HELD".into());
                    }
                    Self::key(k, true)?;
                    self.keys.insert(k);
                } else if self.keys.contains(&k) {
                    Self::key(k, false)?;
                    self.keys.remove(&k);
                }
                Ok(())
            }
            Event::Button { button, down } => {
                let (d, u, k) = match button {
                    0 => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, VK_LBUTTON),
                    1 => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, VK_MBUTTON),
                    2 => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, VK_RBUTTON),
                    _ => return Err("INPUT_BUTTON".into()),
                };
                if *down {
                    if self.buttons.contains(button) {
                        return Ok(());
                    }
                    if unsafe { GetAsyncKeyState(k as i32) } < 0 {
                        return Err("INPUT_LOCAL_BUTTON_HELD".into());
                    }
                    Self::mouse(d, 0, 0, 0)?;
                    self.buttons.insert(*button);
                } else if self.buttons.contains(button) {
                    Self::mouse(u, 0, 0, 0)?;
                    self.buttons.remove(button);
                }
                Ok(())
            }
            Event::Wheel { delta } => Self::mouse(MOUSEEVENTF_WHEEL, *delta as u32, 0, 0),
            Event::Release {} => self.release(),
        }
    }
}
#[cfg(all(
    windows,
    feature = "p3-lan",
    any(feature = "p3-lan-input-queue-test", feature = "v2-public-control")
))]
impl crate::input_approval::SessionSink for Injector {
    type Event = Event;

    fn apply(&mut self, event: &Event) -> Result<(), crate::input_approval::SinkError> {
        Sink::apply(self, event).map_err(|_| crate::input_approval::SinkError)
    }

    fn release_owned(&mut self) -> Result<(), crate::input_approval::SinkError> {
        self.release().map_err(|_| crate::input_approval::SinkError)
    }
}

#[cfg(all(windows, feature = "p3-lan"))]
impl Drop for Injector {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fake(u32);
    impl Sink for Fake {
        fn apply(&mut self, _: &Event) -> Result<(), String> {
            self.0 += 1;
            Ok(())
        }
    }
    fn data(seq: u64, event: &str) -> Vec<u8> {
        format!("{{\"v\":1,\"seq\":{seq},\"session_id\":\"test\",\"event\":{event}}}").into_bytes()
    }
    #[test]
    fn authorization_sequence_size_and_key_limits_precede_sink() {
        let now = Instant::now();
        let mut p = Protocol::new("test".into(), 30);
        let mut f = Fake(0);
        let b = data(1, r#"{"kind":"key","key":"KeyA","down":true}"#);
        assert!(p.process(&b, true, false, now, &mut f).is_err());
        assert_eq!(f.0, 0);
        assert!(p.process(&b, true, true, now, &mut f).is_ok());
        assert!(p.process(&b, true, true, now, &mut f).is_err());
        assert!(p
            .process(
                &data(2, r#"{"kind":"key","key":"MetaLeft","down":true}"#),
                true,
                true,
                now,
                &mut f
            )
            .is_err());
        assert!(p.process(&vec![0; 513], true, true, now, &mut f).is_err());
        assert!(p
            .process(
                &data(2, r#"{"kind":"mouse_move","x_norm":2,"y_norm":0}"#),
                true,
                true,
                now,
                &mut f
            )
            .is_err());
        assert_eq!(f.0, 1);
    }
    #[test]
    fn expired_and_rate_limited_never_reach_sink() {
        let now = Instant::now();
        let mut p = Protocol::new("test".into(), 1);
        let mut f = Fake(0);
        assert!(p
            .process(
                &data(1, r#"{"kind":"release_all"}"#),
                true,
                true,
                now + Duration::from_secs(2),
                &mut f
            )
            .is_err());
        let mut p = Protocol::new("test".into(), 30);
        for n in 1..=120 {
            p.process(
                &data(n, r#"{"kind":"release_all"}"#),
                true,
                true,
                now,
                &mut f,
            )
            .unwrap();
        }
        assert!(p
            .process(
                &data(121, r#"{"kind":"release_all"}"#),
                true,
                true,
                now,
                &mut f
            )
            .is_err());
        assert_eq!(f.0, 120);
    }
    #[test]
    fn physical_primary_mapping_handles_scaled_video_and_negative_secondary() {
        assert_eq!(
            map_point(0.0, 0.0, (2560, 1440), (0, 0, 2560, 1440)).unwrap(),
            (0, 0)
        );
        assert_eq!(
            map_point(1.0, 1.0, (2560, 1440), (0, 0, 2560, 1440)).unwrap(),
            (65535, 65535)
        );
        let (x, _) = map_point(0.0, 0.0, (2560, 1440), (-1920, 0, 4480, 1440)).unwrap();
        assert!(x > 28000 && x < 28200);
        assert!(map_point(f64::NAN, 0.0, (100, 100), (0, 0, 100, 100)).is_err());
    }
    #[test]
    fn wrong_session_binary_extra_fields_and_sink_failure_do_not_advance_sequence() {
        let now = Instant::now();
        let mut p = Protocol::new("test".into(), 30);
        let mut f = Fake(0);
        let valid = data(1, r#"{"kind":"release_all"}"#);
        assert!(p.process(&valid, false, true, now, &mut f).is_err());
        let wrong = String::from_utf8(valid.clone())
            .unwrap()
            .replace("test", "other");
        assert!(p
            .process(wrong.as_bytes(), true, true, now, &mut f)
            .is_err());
        assert!(p
            .process(
                &data(1, r#"{"kind":"release_all","secret":"not-allowed"}"#),
                true,
                true,
                now,
                &mut f
            )
            .is_err());
        struct Blocked;
        impl Sink for Blocked {
            fn apply(&mut self, _: &Event) -> Result<(), String> {
                Err("INPUT_SEND_BLOCKED".into())
            }
        }
        assert!(p.process(&valid, true, true, now, &mut Blocked).is_err());
        assert_eq!(f.0, 0);
        assert_eq!(p.process(&valid, true, true, now, &mut f).unwrap(), 1);
        assert_eq!(f.0, 1);
    }
}
