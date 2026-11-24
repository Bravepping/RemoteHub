//! The main window requests a visible dedicated view or observes/stops it.
//! Only the exact owned native window can attach (media and input use Session::owner).
use super::*;

const LABEL_PREFIX: &str = "remote-session-";
const MISSING: &str = "PUBLIC_SESSION_WINDOW_MISSING";
const STATE: &str = "PUBLIC_SESSION_WINDOW_STATE";

struct Intent {
    role: String,
    invitation_id: String,
}

pub(super) struct Slot {
    label: String,
    intent: Option<Intent>,
    attaching: bool,
    closing: bool,
}

enum Open {
    Existing(String),
    Reserved {
        label: String,
        retired: Option<Slot>,
    },
}

fn reserve_open(state: &DesktopVideoState, intent: Intent) -> Result<Open, String> {
    // Match start_session's active -> slot order. Pending or in-flight
    // attachment has no terminal state and cannot be replaced.
    let active = state.active.lock().map_err(|_| STATE)?;
    let mut slot = state.remote_window.lock().map_err(|_| STATE)?;
    if let Some(current) = slot.as_ref() {
        let current_running = active
            .as_ref()
            .is_some_and(|s| s.owner == current.label && s.status.borrow().phase != Phase::Closed);
        if current.closing || current.intent.is_some() || current.attaching || current_running {
            return Ok(Open::Existing(current.label.clone()));
        }
    }

    if active
        .as_ref()
        .is_some_and(|s| s.status.borrow().phase != Phase::Closed)
    {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let label = format!("{LABEL_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let retired = slot.replace(Slot {
        label: label.clone(),
        intent: Some(intent),
        attaching: false,
        closing: false,
    });
    Ok(Open::Reserved { label, retired })
}

#[derive(Serialize)]
pub(crate) struct Snapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<Status>,
}

fn verified(window: &WebviewWindow) -> Result<Arc<DesktopVideoState>, String> {
    let state = window.state::<Arc<DesktopVideoState>>().inner().clone();
    if !state.picker_available() {
        return Err(MISSING.into());
    }
    let label = window.label();
    if label == "main" {
        crate::experiment_origin::origin(window)?;
    } else {
        let is_owner = state
            .remote_window
            .lock()
            .map_err(|_| STATE)?
            .as_ref()
            .is_some_and(|slot| slot.label == label);
        if !is_owner {
            return Err(MISSING.into());
        }
        crate::experiment_origin::origin_for(window, label)?;
    }
    Ok(state)
}

fn active_for(state: &DesktopVideoState, label: &str) -> Result<Option<Arc<Session>>, String> {
    Ok(state
        .active
        .lock()
        .map_err(|_| STATE)?
        .as_ref()
        .filter(|s| s.owner == label)
        .cloned())
}

fn current_label(state: &DesktopVideoState) -> Result<Option<String>, String> {
    Ok(state
        .remote_window
        .lock()
        .map_err(|_| STATE)?
        .as_ref()
        .map(|s| s.label.clone()))
}

fn focus(window: &WebviewWindow) -> Result<(), String> {
    window
        .show()
        .and_then(|_| window.unminimize())
        .and_then(|_| window.set_focus())
        .map_err(|_| MISSING.to_owned())
}

fn clear_slot(state: &DesktopVideoState, label: &str) {
    if let Ok(mut slot) = state.remote_window.lock() {
        if slot.as_ref().is_some_and(|entry| entry.label == label) {
            *slot = None;
        }
    }
}

#[tauri::command]
pub(crate) async fn public_session_window_open(
    window: WebviewWindow,
    role: String,
    invitation_id: String,
) -> Result<&'static str, String> {
    crate::experiment_origin::origin(&window)?;
    let state = verified(&window)?;
    if state.closing.load(Ordering::Acquire)
        || !matches!(role.as_str(), "host" | "viewer")
        || !valid_generation(&invitation_id)
    {
        return Err(MISSING.into());
    }

    let label = match reserve_open(
        &state,
        Intent {
            role,
            invitation_id,
        },
    )? {
        Open::Existing(label) => {
            if let Some(existing) = window.get_webview_window(&label) {
                focus(&existing)?;
            }
            return Ok("existing");
        }
        Open::Reserved { label, retired } => {
            if let Some(retired) = retired {
                let removed = window
                    .get_webview_window(&retired.label)
                    .is_none_or(|old| old.destroy().is_ok());
                if !removed {
                    let mut slot = state.remote_window.lock().map_err(|_| STATE)?;
                    if slot.as_ref().is_some_and(|s| s.label == label) {
                        *slot = Some(retired);
                    }
                    return Err("PUBLIC_SESSION_WINDOW_CLOSE_FAILED".into());
                }
            }
            label
        }
    };

    let view = tauri::WebviewWindowBuilder::new(
        window.app_handle(),
        &label,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("RemoteHub · 远控窗口")
    .inner_size(1080.0, 740.0)
    .min_inner_size(600.0, 440.0)
    .build();

    match view {
        Ok(_) => Ok("opened"),
        Err(_) => {
            clear_slot(&state, &label);
            Err("PUBLIC_SESSION_WINDOW_OPEN_FAILED".into())
        }
    }
}

#[tauri::command]
pub(crate) fn public_session_window_attach(window: WebviewWindow) -> Result<Snapshot, String> {
    let state = verified(&window)?;
    let label = window.label().to_owned();
    let intent = {
        let mut slot = state.remote_window.lock().map_err(|_| STATE)?;
        let entry = slot
            .as_mut()
            .filter(|s| s.label == label && !s.closing && !s.attaching)
            .ok_or(MISSING)?;
        let intent = entry.intent.take();
        entry.attaching = intent.is_some();
        intent
    };
    if let Some(intent) = intent {
        let result = start_session(window.clone(), intent.role, Some(intent.invitation_id));
        let mut slot = state.remote_window.lock().map_err(|_| STATE)?;
        if let Some(entry) = slot.as_mut().filter(|s| s.label == label) {
            entry.attaching = false;
        }
        result?;
    }
    let snap = snapshot(&window, &state)?;
    if snap.generation.is_none() {
        return Err(MISSING.into());
    }
    Ok(snap)
}

#[tauri::command]
pub(crate) fn public_session_window_snapshot(window: WebviewWindow) -> Result<Snapshot, String> {
    let state = verified(&window)?;
    snapshot(&window, &state)
}

fn snapshot(window: &WebviewWindow, state: &DesktopVideoState) -> Result<Snapshot, String> {
    let label = current_label(state)?;
    let session = match label.as_deref() {
        Some(owner) => active_for(state, owner)?,
        None => None,
    };
    let reveal_generation = label.as_deref() == Some(window.label()) && window.label() != "main";
    Ok(Snapshot {
        generation: session
            .as_ref()
            .filter(|_| reveal_generation)
            .map(|s| s.id.clone()),
        role: session.as_ref().map(|s| {
            if s.role == Role::Viewer {
                "viewer"
            } else {
                "host"
            }
        }),
        status: session.as_ref().map(|s| s.snapshot()),
    })
}

#[tauri::command]
pub(crate) fn public_session_window_focus(window: WebviewWindow) -> Result<(), String> {
    crate::experiment_origin::origin(&window)?;
    let state = verified(&window)?;
    let label = current_label(&state)?.ok_or(MISSING)?;
    let remote = window.get_webview_window(&label).ok_or(MISSING)?;
    focus(&remote)
}

#[tauri::command]
pub(crate) async fn public_session_window_stop(window: WebviewWindow) -> Result<(), String> {
    crate::experiment_origin::origin(&window)?;
    let state = verified(&window)?;
    let label = current_label(&state)?.ok_or(MISSING)?;
    let session = active_for(&state, &label)?.ok_or(MISSING)?;
    session.request_stop();
    if !session.wait_finished_for(Duration::from_secs(3)).await {
        return Err("PUBLIC_VIDEO_STOP_UNCONFIRMED".into());
    }
    Ok(())
}

pub(super) fn input_visible(window: &WebviewWindow) -> bool {
    window.is_focused().unwrap_or(false)
        && !window.is_minimized().unwrap_or(true)
        && window.is_visible().unwrap_or(false)
}

#[tauri::command]
pub(crate) fn public_session_window_arm_input(
    window: WebviewWindow,
    generation: String,
) -> Result<(), String> {
    // This arms only the viewer's local keyboard forwarding, not host consent.
    let session = find(&window, &generation)?;
    if session.owner == "main" {
        return Err("PUBLIC_CONTROL_NOT_ALLOWED".into());
    }
    let focus_generation = session.lock_window_input()?.focus_generation;
    let visible = input_visible(&window);
    let mut input = session.lock_window_input()?;
    if !visible || !session.input_ready() || input.focus_generation != focus_generation {
        return Err("PUBLIC_CONTROL_NOT_ALLOWED".into());
    }
    input.armed = true;
    Ok(())
}

fn pause_owner_input(window: &tauri::Window, state: &DesktopVideoState, label: &str) {
    if let Ok(Some(session)) = active_for(state, label) {
        if let Ok(mut input) = session.lock_window_input() {
            input.focus_generation = input.focus_generation.wrapping_add(1);
            // A full channel or damaged local lock falls back to session
            // teardown and host-side release rather than leaving input armed.
            let _ = session.pause_window_input(&mut input.armed);
        }
    }
    use tauri::Emitter;
    let _ = window.emit_to(
        tauri::EventTarget::webview_window(label),
        "remotehub-session-input-pause",
        (),
    );
}

pub(crate) fn own_window_event(window: &tauri::Window, event: &tauri::WindowEvent) -> bool {
    let label = window.label();
    if !label.starts_with(LABEL_PREFIX) {
        return false;
    }
    let Some(state) = window
        .try_state::<Arc<DesktopVideoState>>()
        .map(|s| s.inner().clone())
    else {
        return true;
    };
    if !current_label(&state).is_ok_and(|s| s.as_deref() == Some(label)) {
        return true;
    }

    match event {
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            let reserved = state.remote_window.lock().is_ok_and(|mut slot| {
                slot.as_mut()
                    .filter(|s| s.label == label && !s.closing)
                    .is_some_and(|s| {
                        s.closing = true;
                        s.intent = None;
                        true
                    })
            });
            if reserved {
                // Do not touch the LAN or main-window lifecycle for a view close.
                let session = active_for(&state, label).ok().flatten();
                if let Some(session) = &session {
                    session.request_stop();
                }
                let window = window.clone();
                tauri::async_runtime::spawn(async move {
                    let clean = match session {
                        Some(s) => s.wait_finished().await,
                        None => true,
                    };
                    if clean && window.destroy().is_ok() {
                        return;
                    }
                    if let Ok(mut slot) = state.remote_window.lock() {
                        if let Some(slot) = slot.as_mut().filter(|s| s.label == window.label()) {
                            slot.closing = false;
                        }
                    }
                });
            }
        }
        tauri::WindowEvent::Destroyed => {
            if let Ok(Some(session)) = active_for(&state, label) {
                session.request_stop();
            }
            clear_slot(&state, label);
        }
        tauri::WindowEvent::Focused(false) => pause_owner_input(window, &state, label),
        tauri::WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) => {
            pause_owner_input(window, &state, label);
        }
        _ => {}
    }
    true
}

pub(crate) fn destroy_after_main_close(window: &tauri::Window) {
    if let Some(state) = window.try_state::<Arc<DesktopVideoState>>() {
        if let Ok(Some(label)) = current_label(&state) {
            if let Some(remote) = window.get_webview_window(&label) {
                let _ = remote.destroy();
            }
        }
    }
}

pub(super) fn still_open(state: &DesktopVideoState, label: &str) -> bool {
    state.remote_window.lock().is_ok_and(|slot| {
        slot.as_ref()
            .is_some_and(|slot| slot.label == label && !slot.closing)
    })
}

pub(super) fn reservation_busy(state: &DesktopVideoState) -> bool {
    state.remote_window.lock().map_or(true, |slot| {
        slot.as_ref()
            .is_some_and(|entry| entry.closing || entry.attaching || entry.intent.is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_visibility_loss_serializes_one_release_and_clears_local_arm() {
        let owner = "remote-session-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let session = Session::owned(Role::Viewer, true, owner);
        assert!(!session.lock_window_input().unwrap().armed);
        let mut receiver = session.input_receiver.lock().unwrap().take().unwrap();
        session.input_state.send_replace(InputState::Allowed);
        session.status.send_if_modified(|status| {
            status.phase = Phase::Ready;
            true
        });

        {
            let mut input = session.lock_window_input().unwrap();
            input.armed = true;
            let pressed = r#"{"kind":"key","key":"KeyA","down":true}"#;
            session.enqueue_input(pressed.to_owned()).unwrap();
            session.pause_window_input(&mut input.armed).unwrap();
            session.pause_window_input(&mut input.armed).unwrap();
            assert!(!input.armed);
            assert_eq!(receiver.try_recv().unwrap(), pressed);
            assert_eq!(receiver.try_recv().unwrap(), r#"{"kind":"release_all"}"#);
            assert!(receiver.try_recv().is_err());
        }
        assert!(session.input_ready());
        assert!(!*session.stop.borrow());
    }

    #[test]
    fn full_release_channel_fails_closed_instead_of_leaving_input_armed() {
        let session = Session::owned(Role::Viewer, true, "remote-session-synthetic");
        session.input_state.send_replace(InputState::Allowed);
        session.status.send_if_modified(|status| {
            status.phase = Phase::Ready;
            true
        });
        let mut input = session.lock_window_input().unwrap();
        input.armed = true;
        for _ in 0..session.input_events.max_capacity() {
            session.enqueue_input("synthetic".to_owned()).unwrap();
        }
        assert_eq!(
            session.pause_window_input(&mut input.armed),
            Err("PUBLIC_CONTROL_BACKPRESSURE".to_owned())
        );
        assert!(!input.armed);
        assert!(*session.stop.borrow());
    }

    fn next_intent() -> Intent {
        Intent {
            role: "viewer".to_owned(),
            invitation_id: uuid::Uuid::new_v4().simple().to_string(),
        }
    }

    #[test]
    fn explicit_open_reuses_live_slot_and_replaces_only_terminal_generations() {
        let state = DesktopVideoState::new();
        let Open::Reserved {
            mut label,
            retired: None,
        } = reserve_open(&state, next_intent()).unwrap()
        else {
            panic!("first explicit request should reserve its own window");
        };
        for _ in 0..3 {
            assert!(matches!(
                reserve_open(&state, next_intent()).unwrap(),
                Open::Existing(current) if current == label
            ));
            {
                let mut slot = state.remote_window.lock().unwrap();
                let slot = slot.as_mut().unwrap();
                assert!(slot.intent.take().is_some());
                slot.attaching = true;
            }
            assert!(matches!(
                reserve_open(&state, next_intent()).unwrap(),
                Open::Existing(current) if current == label
            ));

            let previous = Arc::new(Session::owned(Role::Viewer, true, &label));
            *state.active.lock().unwrap() = Some(previous.clone());
            state
                .remote_window
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .attaching = false;
            previous.request_stop();
            assert!(matches!(
                reserve_open(&state, next_intent()).unwrap(),
                Open::Existing(current) if current == label
            ));
            previous.status.send_replace(Status::closed(Err(CANCELLED)));
            let Open::Reserved {
                label: new_label,
                retired: Some(retired),
            } = reserve_open(&state, next_intent()).unwrap()
            else {
                panic!("new invite should replace the terminal window");
            };
            assert_eq!(retired.label, label);
            assert_ne!(new_label, label);
            clear_slot(&state, &label); // Late native Destroyed event.
            assert_eq!(
                current_label(&state).unwrap().as_deref(),
                Some(new_label.as_str())
            );
            assert!(active_for(&state, &new_label).unwrap().is_none());
            label = new_label;
        }
    }

    #[test]
    fn replaced_or_closing_label_cannot_use_or_clear_another_session_owner() {
        let state = DesktopVideoState::new();
        let owner = "remote-session-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let stale = "remote-session-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        *state.remote_window.lock().unwrap() = Some(Slot {
            label: owner.to_owned(),
            intent: None,
            attaching: false,
            closing: false,
        });
        let session = Arc::new(Session::owned(Role::Viewer, true, owner));
        *state.active.lock().unwrap() = Some(session.clone());

        assert!(still_open(&state, owner));
        assert!(!still_open(&state, stale));
        assert!(active_for(&state, stale).unwrap().is_none());
        clear_slot(&state, stale);
        assert_eq!(current_label(&state).unwrap().as_deref(), Some(owner));
        assert_eq!(active_for(&state, owner).unwrap().unwrap().id, session.id);

        state
            .remote_window
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .closing = true;
        assert!(!still_open(&state, owner));
        assert_eq!(session.snapshot().phase, Phase::Starting);
        active_for(&state, owner).unwrap().unwrap().request_stop();
        assert_eq!(session.snapshot().phase, Phase::Stopping);
        clear_slot(&state, owner);
        assert!(current_label(&state).unwrap().is_none());
    }
}
