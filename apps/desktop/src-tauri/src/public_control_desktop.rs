//! Native owners for the explicit public-control experiment. The transport
//! has already authenticated the peer, TURN route and data channel. This
//! module still requires a separate local host decision before any input.

#[cfg(feature = "v2-session-window-sustained")]
pub(crate) mod sustained;

use hub_identity::session::SessionRole;
use hub_webrtc::{
    input_approval::{SessionSink, SinkError, State},
    input_queue::Control,
    public_control_input::PublicControlInput,
    public_video::PublicControlTransport,
    remote_input::{Event, Injector},
};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO},
    },
};

pub(crate) const LEGACY_CONTROL_WINDOW: Duration = Duration::from_secs(15);
const BATCH_CONTROL_WINDOW: Duration = crate::public_video_capture::CONTROL_WINDOW;
const BAD_DEADLINE: &str = "PUBLIC_CONTROL_ROUND_DEADLINE";

#[derive(Clone, Copy)]
pub(crate) enum Timing {
    Legacy,
    Batch,
}

impl Timing {
    pub(crate) fn window(self) -> Duration {
        match self {
            Self::Legacy => LEGACY_CONTROL_WINDOW,
            Self::Batch => BATCH_CONTROL_WINDOW,
        }
    }

    fn cutoff(self, transport_deadline: Instant, now: Instant) -> Result<Instant, &'static str> {
        match self {
            Self::Legacy => Ok(transport_deadline.min(now + self.window())),
            // The connection actor supplies the one-shot round cutoff;
            // opening or answering the second prompt never moves it.
            Self::Batch if transport_deadline.saturating_duration_since(now) <= self.window() => {
                Ok(transport_deadline)
            }
            Self::Batch => Err(BAD_DEADLINE),
        }
    }
}
const LOCAL_POLL: Duration = Duration::from_millis(15);
const ACK_WAIT: Duration = Duration::from_secs(1);
const VERSION: u8 = 1;
const EVENT_BYTES: usize = 320;
const INNER_BYTES: usize = 512;
const MAX_SEQUENCE: u64 = 1000;
const ROLE: &str = "PUBLIC_CONTROL_ROLE";
const DIALOG: &str = "PUBLIC_CONTROL_INPUT_DIALOG";
const WIRE: &str = "PUBLIC_CONTROL_INPUT_MESSAGE";
const ACK_TIMEOUT: &str = "PUBLIC_CONTROL_INPUT_ACK_TIMEOUT";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputState {
    #[default]
    ViewOnly,
    Pending,
    Allowed,
    Denied,
    #[cfg(feature = "v2-session-window-sustained")]
    Ended,
    Closed,
}

/// Never instantiated until the approved one-shot queue actually dispatches.
#[derive(Default)]
struct DeferredInjector(Option<Injector>);

impl SessionSink for DeferredInjector {
    type Event = Event;

    fn apply(&mut self, event: &Event) -> Result<(), SinkError> {
        self.0.get_or_insert_with(Injector::default).apply(event)
    }

    fn release_owned(&mut self) -> Result<(), SinkError> {
        match self.0.as_mut() {
            Some(injector) => injector.release_owned(),
            None => Ok(()),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    InputGranted {
        v: u8,
        grant_id: String,
    },
    InputDenied {
        v: u8,
    },
    InputAck {
        v: u8,
        ack_seq: u64,
    },
    #[cfg(feature = "v2-session-window-sustained")]
    InputEnded {
        v: u8,
        grant_id: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InputMessage {
    v: u8,
    grant_id: String,
    event_v1: String,
}

#[derive(Clone, Copy)]
enum Halt {
    Stop,
    Emergency,
    Expired,
}

enum Exit {
    Halt(Halt),
    Failed(&'static str),
}

type Flow<T> = Result<T, Exit>;

impl From<&'static str> for Exit {
    fn from(error: &'static str) -> Self {
        Self::Failed(error)
    }
}

impl Halt {
    fn cleanup(self) -> Control {
        match self {
            Self::Stop => Control::TransportLost,
            Self::Emergency => Control::Revoke,
            Self::Expired => Control::Continue,
        }
    }
}

fn local_halt(stop: &watch::Receiver<bool>, until: Instant, host: bool) -> Option<Halt> {
    if *stop.borrow() || stop.has_changed().is_err() {
        Some(Halt::Stop)
    } else if host && Injector::emergency_pressed() {
        Some(Halt::Emergency)
    } else if Instant::now() >= until {
        Some(Halt::Expired)
    } else {
        None
    }
}

fn require_current(stop: &watch::Receiver<bool>, until: Instant, host: bool) -> Flow<()> {
    match local_halt(stop, until, host) {
        Some(end) => Err(Exit::Halt(end)),
        None => Ok(()),
    }
}

async fn wait_halt(stop: &mut watch::Receiver<bool>, until: Instant, host: bool) -> Halt {
    loop {
        if let Some(end) = local_halt(stop, until, host) {
            return end;
        }
        tokio::select! {
            biased;
            changed = stop.changed() => if changed.is_err() { return Halt::Stop; },
            _ = tokio::time::sleep(until.saturating_duration_since(Instant::now()).min(LOCAL_POLL)) => (),
        }
    }
}

/// Canceling transport I/O intentionally revokes its route, but we never
/// cancel it just to service a poll/tick or a waiting dialog.
async fn live<T>(
    stop: &mut watch::Receiver<bool>,
    until: Instant,
    host: bool,
    work: impl Future<Output = Result<T, &'static str>>,
) -> Flow<T> {
    tokio::select! {
        biased;
        end = wait_halt(stop, until, host) => Err(Exit::Halt(end)),
        result = work => result.map_err(Exit::Failed),
    }
}

fn result(outcome: Flow<()>) -> Result<(), &'static str> {
    match outcome {
        Ok(()) | Err(Exit::Halt(_)) => Ok(()),
        Err(Exit::Failed(error)) => Err(error),
    }
}

fn approval_dialog(
    hwnd: usize,
    fingerprint: String,
    timing: Timing,
) -> Result<oneshot::Receiver<bool>, &'static str> {
    approval_dialog_seconds(hwnd, fingerprint, timing.window().as_secs())
}

fn approval_dialog_seconds(
    hwnd: usize,
    fingerprint: String,
    seconds: u64,
) -> Result<oneshot::Receiver<bool>, &'static str> {
    if seconds == 0
        || hwnd == 0
        || fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(DIALOG);
    }
    let (decision, answer) = oneshot::channel();
    tauri::async_runtime::spawn_blocking(move || {
        let message = format!(
            "此窗口单独批准对方在本台 Windows 操作键鼠，只持续到本轮结束（整轮最长 {} 秒）。不批准则只观看。\n\n已核验对方指纹：\n{fingerprint}\n\n允许后可点「停止」或关窗；也可按 Ctrl+Alt+F12 立即撤销。超时后本次回答失效。\n\n是否允许本次操作？",
            seconds,
        );
        let accepted = unsafe {
            MessageBoxW(
                Some(HWND(hwnd as *mut _)),
                &HSTRING::from(message),
                &HSTRING::from("RemoteHub · 公网键鼠单次许可"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
            ) == IDYES
        };
        let _ = decision.send(accepted);
    });
    Ok(answer)
}

fn encode<T: Serialize>(value: &T) -> Result<String, &'static str> {
    serde_json::to_string(value).map_err(|_| WIRE)
}

fn valid_grant(value: &str) -> bool {
    value.len() == 32
        && Uuid::try_parse(value)
            .is_ok_and(|uuid| uuid.get_version_num() == 4 && value == uuid.simple().to_string())
}

fn approved_message(bytes: &[u8], grant_id: &str) -> Result<InputMessage, &'static str> {
    approved_message_version(bytes, grant_id, VERSION)
}
fn approved_message_version(
    bytes: &[u8],
    grant_id: &str,
    version: u8,
) -> Result<InputMessage, &'static str> {
    let payload: InputMessage = serde_json::from_slice(bytes).map_err(|_| WIRE)?;
    if payload.v != version
        || payload.grant_id != grant_id
        || payload.event_v1.is_empty()
        || payload.event_v1.len() > INNER_BYTES
    {
        return Err(WIRE);
    }
    Ok(payload)
}

pub(crate) async fn run_host(
    mut transport: PublicControlTransport,
    mut stop: watch::Receiver<bool>,
    hwnd: usize,
    peer_fingerprint: String,
    mut first_video: watch::Receiver<bool>,
    timing: Timing,
    status: watch::Sender<InputState>,
) -> Result<(), &'static str> {
    status.send_replace(InputState::ViewOnly);
    let outcome = async {
        if transport.role() != SessionRole::Host {
            return Err(ROLE);
        }
        let scope = transport.scope();
        let until = timing.cutoff(transport.deadline(), Instant::now())?;
        if local_halt(&stop, until, true).is_some() {
            return Ok(());
        }
        // Do not offer control of a desktop we could not capture and send.
        // Until then this owner has no grant, input queue or injector.
        match live(&mut stop, until, true, async {
            Ok(first_video.wait_for(|&sent| sent).await.is_ok())
        })
        .await
        {
            Ok(true) => (),
            Ok(false) => return Ok(()),
            Err(exit) => return result(Err(exit)),
        }
        if local_halt(&stop, until, true).is_some() {
            return Ok(());
        }
        let mut owner = PublicControlInput::view_only(scope, until, DeferredInjector::default())
            .map_err(|error| error.code())?;
        let turn = async {
            let pending = owner
                .begin_local_decision(&scope, Control::Continue)
                .map_err(|error| error.code())?;
            status.send_replace(InputState::Pending);
            let mut answer = approval_dialog(hwnd, peer_fingerprint, timing)?;
            let approved = loop {
                require_current(&stop, until, true)?;
                match answer.try_recv() {
                    Ok(approved) => break approved,
                    Err(oneshot::error::TryRecvError::Closed) => return Err(DIALOG.into()),
                    Err(oneshot::error::TryRecvError::Empty) => (),
                }
                // Pre-consent packets are not parsed, queued or counted.
                let _ = live(&mut stop, until, true, transport.recv_text()).await?;
            };
            // Fresh transport verification before accepting the local answer.
            let _ = live(&mut stop, until, true, transport.recv_text()).await?;
            require_current(&stop, until, true)?;
            owner
                .complete_local_decision(pending, &scope, approved, Control::Continue)
                .map_err(|error| error.code())?;

            if !approved {
                status.send_replace(InputState::Denied);
                let denied = encode(&Reply::InputDenied { v: VERSION })?;
                live(&mut stop, until, true, transport.send_text(&denied)).await?;
                // Declining this separate permission doesn't itself end the view.
                loop {
                    let _ = live(&mut stop, until, true, transport.recv_text()).await?;
                }
            }

            // A pre-approval packet in the transport backlog cannot contain
            // this freshly generated grant ID. It isn't an identity or profile.
            let grant_id = Uuid::new_v4().simple().to_string();
            let grant = encode(&Reply::InputGranted {
                v: VERSION,
                grant_id: grant_id.clone(),
            })?;
            live(&mut stop, until, true, transport.send_text(&grant)).await?;
            status.send_replace(InputState::Allowed);

            loop {
                let Some(bytes) = live(&mut stop, until, true, transport.recv_text()).await? else {
                    continue;
                };
                require_current(&stop, until, true)?;
                let payload = approved_message(&bytes, &grant_id)?;
                owner
                    .enqueue_v1(&scope, payload.event_v1.as_bytes(), true, Control::Continue)
                    .map_err(|error| error.code())?;
                require_current(&stop, until, true)?;
                let Some(ack_seq) = owner
                    .dispatch_next(&scope, Control::Continue)
                    .map_err(|error| error.code())?
                else {
                    if owner.state() != State::Allowed {
                        return Ok(());
                    }
                    return Err(WIRE.into());
                };
                let ack = encode(&Reply::InputAck {
                    v: VERSION,
                    ack_seq,
                })?;
                live(&mut stop, until, true, transport.send_text(&ack)).await?;
            }
        }
        .await;

        let control = match &turn {
            Err(Exit::Halt(end)) => end.cleanup(),
            _ => Control::TransportLost,
        };
        // Deliberately check this explicit result, even after another error.
        owner.advance(control).map_err(|error| error.code())?;
        result(turn)
    }
    .await;
    status.send_replace(InputState::Closed);
    outcome
}

fn discard_pending(events: &mut mpsc::Receiver<String>) -> Flow<()> {
    if events.is_closed() {
        return Err(Exit::Halt(Halt::Stop));
    }
    // The producer also checks Allowed. Snapshot the bounded native queue so
    // an active producer can't turn this into an unbounded pre-consent drain.
    for _ in 0..events.len() {
        match events.try_recv() {
            Ok(_) => (),
            Err(mpsc::error::TryRecvError::Empty) => break,
            Err(mpsc::error::TryRecvError::Disconnected) => return Err(Exit::Halt(Halt::Stop)),
        }
    }
    Ok(())
}

fn wrap_event(raw: &str, room: Uuid, seq: u64, grant_id: &str) -> Result<String, &'static str> {
    wrap_event_version(raw, room, seq, grant_id, VERSION, false)
}

fn wrap_event_version(
    raw: &str,
    room: Uuid,
    seq: u64,
    grant_id: &str,
    version: u8,
    wheel: bool,
) -> Result<String, &'static str> {
    let max_sequence = MAX_SEQUENCE;
    #[cfg(feature = "v2-session-window-sustained")]
    let max_sequence = if wheel {
        hub_webrtc::remote_input::MAX_SUSTAINED_EVENTS
    } else {
        max_sequence
    };
    if raw.is_empty() || raw.len() > EVENT_BYTES || !(1..=max_sequence).contains(&seq) {
        return Err(WIRE);
    }
    let parsed: Event = serde_json::from_str(raw).map_err(|_| WIRE)?;
    if matches!(parsed, Event::Wheel { delta } if !wheel || ![-120, 120].contains(&delta)) {
        return Err(WIRE);
    }
    // Keep the original event JSON; the host's bounded v1 parser validates
    // the complete packet and won't see any fields coerced by a Value map.
    let event_v1 = format!(r#"{{"v":1,"seq":{seq},"session_id":"{room}","event":{raw}}}"#);
    if event_v1.len() > INNER_BYTES {
        return Err(WIRE);
    }
    encode(&InputMessage {
        v: version,
        grant_id: grant_id.to_owned(),
        event_v1,
    })
}

async fn viewer_turns(
    transport: &mut PublicControlTransport,
    stop: &mut watch::Receiver<bool>,
    events: &mut mpsc::Receiver<String>,
    status: &watch::Sender<InputState>,
    until: Instant,
) -> Flow<()> {
    let room = Uuid::from_bytes(transport.scope().room);
    require_current(stop, until, false)?;
    status.send_replace(InputState::Pending);
    let grant_id = loop {
        discard_pending(events)?;
        let Some(frame) = live(stop, until, false, transport.recv_text()).await? else {
            continue;
        };
        let response: Reply = serde_json::from_slice(&frame).map_err(|_| WIRE)?;
        match response {
            Reply::InputGranted {
                v: VERSION,
                grant_id,
            } if valid_grant(&grant_id) => break grant_id,
            Reply::InputDenied { v: VERSION } => {
                status.send_replace(InputState::Denied);
                loop {
                    discard_pending(events)?;
                    if live(stop, until, false, transport.recv_text())
                        .await?
                        .is_some()
                    {
                        return Err(WIRE.into());
                    }
                }
            }
            _ => return Err(WIRE.into()),
        }
    };

    discard_pending(events)?;
    require_current(stop, until, false)?;
    status.send_replace(InputState::Allowed);
    let mut seq = 1;
    loop {
        require_current(stop, until, false)?;
        let raw = match events.try_recv() {
            Ok(raw) => raw,
            Err(mpsc::error::TryRecvError::Empty) => {
                if live(stop, until, false, transport.recv_text())
                    .await?
                    .is_some()
                {
                    return Err(WIRE.into());
                }
                continue;
            }
            Err(mpsc::error::TryRecvError::Disconnected) => return Err(Exit::Halt(Halt::Stop)),
        };
        let packet = wrap_event(&raw, room, seq, &grant_id)?;
        live(stop, until, false, transport.send_text(&packet)).await?;
        let ack_end = until.min(Instant::now() + ACK_WAIT);
        tokio::time::timeout_at(ack_end.into(), async {
            loop {
                if let Some(frame) = live(stop, until, false, transport.recv_text()).await? {
                    let ack: Reply = serde_json::from_slice(&frame).map_err(|_| WIRE)?;
                    match ack {
                        Reply::InputAck {
                            v: VERSION,
                            ack_seq,
                        } if ack_seq == seq => return Ok(()),
                        _ => return Err(Exit::Failed(WIRE)),
                    }
                }
            }
        })
        .await
        .map_err(|_| {
            if Instant::now() >= until {
                Exit::Halt(Halt::Expired)
            } else {
                Exit::Failed(ACK_TIMEOUT)
            }
        })??;
        seq += 1;
    }
}

pub(crate) async fn run_viewer(
    mut transport: PublicControlTransport,
    mut stop: watch::Receiver<bool>,
    mut events: mpsc::Receiver<String>,
    timing: Timing,
    status: watch::Sender<InputState>,
) -> Result<(), &'static str> {
    status.send_replace(InputState::ViewOnly);
    let outcome = async {
        if transport.role() != SessionRole::Viewer {
            return Err(ROLE.into());
        }
        let until = timing.cutoff(transport.deadline(), Instant::now())?;
        viewer_turns(&mut transport, &mut stop, &mut events, &status, until).await
    }
    .await;
    // No "last send" after stop. Closing the verified channel is the fallback
    // that makes the host release. Voluntary pause may send release_all
    // normally while the grant still exists.
    status.send_replace(InputState::Closed);
    result(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_input_uses_the_same_deadline_even_after_a_late_prompt() {
        let now = Instant::now();
        let until = now + Duration::from_secs(60);
        assert_eq!(Timing::Batch.cutoff(until, now), Ok(until));
        assert_eq!(
            Timing::Batch.cutoff(until, now + Duration::from_secs(44)),
            Ok(until)
        );
        assert_eq!(
            Timing::Batch.cutoff(until + Duration::from_secs(1), now),
            Err(BAD_DEADLINE)
        );
        assert_eq!(Timing::Batch.cutoff(until, until), Ok(until));
        assert_eq!(
            Timing::Legacy.cutoff(until, now),
            Ok(now + Duration::from_secs(15))
        );
    }

    #[test]
    fn only_fresh_explicit_grant_permits_an_unchanged_v1_event() {
        let a = Uuid::new_v4().simple().to_string();
        let b = Uuid::new_v4().simple().to_string();
        let room = Uuid::new_v4();
        let raw = r#"{"kind":"release_all"}"#;
        let packet = wrap_event(raw, room, 2, &a).unwrap();
        let payload = approved_message(packet.as_bytes(), &a).unwrap();
        assert_eq!(
            payload.event_v1,
            format!(r#"{{"v":1,"seq":2,"session_id":"{room}","event":{raw}}}"#)
        );
        assert!(approved_message(packet.as_bytes(), &b).is_err());
        assert!(approved_message(packet.as_bytes(), "").is_err());
        assert!(valid_grant(&a));
        assert!(!valid_grant("A2D275C737304319A239F5E9FC0F30A0"));
        assert!(!valid_grant(""));
    }

    #[test]
    fn unknown_or_unbounded_events_and_status_are_not_accepted() {
        let grant = Uuid::new_v4().simple().to_string();
        let room = Uuid::new_v4();
        for raw in [
            r#"{"kind":"key","key":"KeyA","down":true,"extra":true}"#,
            r#"{"kind":"key","key":"KeyA","down":true,"down":false}"#,
            r#"{"kind":"wheel","delta":120}"#,
            r#"{"kind":"run"}"#,
        ] {
            assert!(wrap_event(raw, room, 1, &grant).is_err());
        }
        assert!(wrap_event(r#"{"kind":"release_all"}"#, room, 0, &grant).is_err());
        assert!(wrap_event(r#"{"kind":"release_all"}"#, room, 1001, &grant).is_err());
        assert!(serde_json::from_str::<Reply>(
            r#"{"type":"input_ack","v":1,"ack_seq":1,"extra":1}"#
        )
        .is_err());
        assert_eq!(
            serde_json::to_string(&InputState::ViewOnly).unwrap(),
            r#""view_only""#
        );
    }
}
