//! Opt-in native-only public viewing. The webview can choose a role, pull one
//! admitted AU, or stop; it cannot supply endpoints, keys, captures or input.
#[cfg(feature = "v2-public-control-desktop")]
use crate::public_control_desktop::{self, InputState, Timing as ControlTiming};
use crate::{
    public_video_capture::{self, CaptureEnd, CaptureOutcome, CaptureTiming},
    public_video_launch::LaunchMode,
    public_video_profile::PublicVideoProfile,
};
use hub_webrtc::{
    ice_policy::IceMode,
    public_video_media::{ScopedVideoMedia, ScopedVideoReceiver, VerifiedVideoFrame},
    public_video_wss::{PublicVideoWss, Role, ScopedWssEnd, ScopedWssState},
};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tauri::{ipc::Response, Manager, WebviewWindow};
#[cfg(feature = "v2-public-control-desktop")]
use tokio::sync::mpsc;
use tokio::sync::{watch, Mutex as AsyncMutex};
use windows::{
    core::HSTRING,
    Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNO},
    },
};

#[cfg(feature = "v2-session-window")]
pub(crate) mod session_window;

const CANCELLED: &str = "PUBLIC_VIDEO_CANCELLED";
const UNEXPECTED_CLOSE: &str = "PUBLIC_VIDEO_CLOSED_UNEXPECTEDLY";
const PULL_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_FRAME_BYTES: usize = 256 * 1024;

pub(crate) struct DesktopVideoState {
    launch: LaunchMode,
    #[cfg(feature = "v2-control-invitations")]
    invitations: Mutex<crate::control_invitations::Invitations>,
    active: Mutex<Option<Arc<Session>>>,
    #[cfg(feature = "v2-session-window")]
    remote_window: Mutex<Option<session_window::Slot>>,
    used: AtomicBool,
    closing: AtomicBool,
}

impl DesktopVideoState {
    pub(crate) fn new() -> Self {
        Self {
            launch: LaunchMode::parse(std::env::args_os().skip(1)),
            #[cfg(feature = "v2-control-invitations")]
            invitations: Mutex::default(),
            active: Mutex::new(None),
            #[cfg(feature = "v2-session-window")]
            remote_window: Mutex::new(None),
            used: AtomicBool::new(false),
            closing: AtomicBool::new(false),
        }
    }
    pub(crate) fn available(&self) -> bool {
        self.launch.is_public_video()
            && (!self.launch.is_control()
                || cfg!(all(
                    feature = "v2-public-control-desktop",
                    not(feature = "v2-public-video-60s-test")
                )))
    }
    #[cfg(feature = "v2-public-control-desktop")]
    pub(crate) fn control_available(&self) -> bool {
        self.available() && self.launch.is_control()
    }
    #[cfg(feature = "v2-control-invitations")]
    pub(crate) fn picker_available(&self) -> bool {
        self.available() && matches!(self.launch, LaunchMode::Invitations)
    }
    pub(crate) fn profile_consumed(&self) -> bool {
        self.used.load(Ordering::Acquire)
    }
}

pub(crate) fn require_lan(window: &WebviewWindow) -> Result<(), String> {
    window
        .try_state::<Arc<DesktopVideoState>>()
        .ok_or("PUBLIC_VIDEO_STATE")?
        .launch
        .require_lan()
        .map_err(str::to_owned)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Starting,
    Ready,
    Stopping,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum EndReason {
    LocalStop,
    PeerLeft,
    CaptureLimit,
    CaptureEnded,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConnectionPath {
    Direct,
    Relay,
}

#[derive(Clone, Serialize)]
pub(crate) struct Status {
    phase: Phase,
    error: Option<&'static str>,
    profile_consumed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_path: Option<ConnectionPath>,
    #[cfg(feature = "v2-public-control-desktop")]
    #[serde(skip_serializing_if = "Option::is_none")]
    input_state: Option<InputState>,
    #[cfg(feature = "v2-public-control-desktop")]
    #[serde(skip_serializing_if = "Option::is_none")]
    round_seconds: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    end_reason: Option<EndReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<u64>,
}

type Terminal = Result<EndReason, &'static str>;

struct Outcome {
    terminal: Terminal,
    elapsed_ms: Option<u64>,
}

impl Status {
    fn observe(&mut self, role: Role, state: ScopedWssState) -> bool {
        if !matches!(self.phase, Phase::Starting | Phase::Ready) {
            return false;
        }
        let path = match (role, state) {
            (Role::Host, ScopedWssState::HostPathVerified(route, ..))
            | (Role::Viewer, ScopedWssState::ViewerPathVerified(route, ..)) => Some(
                if route == hub_webrtc::direct_policy::SelectedRoute::TurnRelay {
                    ConnectionPath::Relay
                } else {
                    ConnectionPath::Direct
                },
            ),
            _ => None,
        };
        let ready =
            path.is_some() || matches!((role, state), (Role::Viewer, ScopedWssState::Established));
        if !ready {
            return false;
        }
        let changed =
            self.phase != Phase::Ready || (path.is_some() && self.connection_path != path);
        self.phase = Phase::Ready;
        if path.is_some() {
            self.connection_path = path;
        }
        changed
    }

    fn closed(result: Result<Outcome, &'static str>) -> Self {
        let Outcome {
            terminal,
            elapsed_ms,
        } = result.unwrap_or_else(|error| Outcome {
            terminal: Err(error),
            elapsed_ms: None,
        });
        let (reason, error) = match terminal {
            Ok(reason) => (reason, None),
            Err(CANCELLED) => (EndReason::LocalStop, None),
            Err(error) => (EndReason::Failed, Some(error)),
        };
        Self {
            phase: Phase::Closed,
            error,
            profile_consumed: false,
            connection_path: None,
            #[cfg(feature = "v2-public-control-desktop")]
            input_state: None,
            #[cfg(feature = "v2-public-control-desktop")]
            round_seconds: None,
            end_reason: Some(reason),
            elapsed_ms,
        }
    }
}

fn actor_terminal(actor: Result<ScopedWssEnd, &'static str>) -> Terminal {
    actor.map(|end| match end {
        ScopedWssEnd::Stopped => EndReason::LocalStop,
        ScopedWssEnd::PeerLeft => EndReason::PeerLeft,
    })
}

fn host_outcome(
    actor: Result<ScopedWssEnd, &'static str>,
    capture: CaptureOutcome,
    locally_stopped: bool,
) -> Outcome {
    let terminal = match (actor, capture.end) {
        (Err(error), _) | (_, Err(error)) => Err(error),
        (Ok(ScopedWssEnd::PeerLeft), _) => Ok(EndReason::PeerLeft),
        (_, Ok(CaptureEnd::LocalStop)) => Ok(EndReason::LocalStop),
        (_, Ok(CaptureEnd::Limit)) => Ok(EndReason::CaptureLimit),
        (_, Ok(CaptureEnd::SourceEnded)) => Ok(EndReason::CaptureEnded),
        (_, Ok(CaptureEnd::TransportEnded)) if locally_stopped => Ok(EndReason::LocalStop),
        (_, Ok(CaptureEnd::TransportEnded)) => Err(UNEXPECTED_CLOSE),
    };
    Outcome {
        terminal,
        elapsed_ms: capture.elapsed_ms,
    }
}

pub(crate) struct Session {
    id: String,
    owner: String,
    profile_consumed: AtomicBool,
    role: Role,
    stop: watch::Sender<bool>,
    status: watch::Sender<Status>,
    receiver: AsyncMutex<Option<ScopedVideoReceiver>>,
    #[cfg(feature = "v2-public-control-desktop")]
    control: bool,
    #[cfg(feature = "v2-public-control-desktop")]
    input_state: watch::Sender<InputState>,
    #[cfg(feature = "v2-public-control-desktop")]
    round_deadline: watch::Sender<Option<std::time::Instant>>,
    #[cfg(feature = "v2-public-control-desktop")]
    round_seconds: std::sync::OnceLock<u16>,
    #[cfg(feature = "v2-public-control-desktop")]
    input_events: mpsc::Sender<String>,
    #[cfg(feature = "v2-public-control-desktop")]
    input_receiver: Mutex<Option<mpsc::Receiver<String>>>,
}

impl Session {
    #[cfg(test)]
    fn new(role: Role, control_launch: bool) -> Self {
        Self::owned(role, control_launch, "main")
    }

    fn owned(role: Role, control_launch: bool, owner: &str) -> Self {
        #[cfg(feature = "v2-public-control-desktop")]
        let (input_events, input_receiver) = mpsc::channel(24);
        #[cfg(not(feature = "v2-public-control-desktop"))]
        let _ = control_launch;
        Self {
            id: uuid::Uuid::new_v4().simple().to_string(),
            owner: owner.to_owned(),
            profile_consumed: AtomicBool::new(false),
            role,
            stop: watch::channel(false).0,
            status: watch::channel(Status {
                phase: Phase::Starting,
                error: None,
                profile_consumed: false,
                connection_path: None,
                #[cfg(feature = "v2-public-control-desktop")]
                input_state: None,
                #[cfg(feature = "v2-public-control-desktop")]
                round_seconds: None,
                end_reason: None,
                elapsed_ms: None,
            })
            .0,
            receiver: AsyncMutex::new(None),
            #[cfg(feature = "v2-public-control-desktop")]
            control: control_launch,
            #[cfg(feature = "v2-public-control-desktop")]
            input_state: watch::channel(InputState::ViewOnly).0,
            #[cfg(feature = "v2-public-control-desktop")]
            round_deadline: watch::channel(None).0,
            #[cfg(feature = "v2-public-control-desktop")]
            round_seconds: std::sync::OnceLock::new(),
            #[cfg(feature = "v2-public-control-desktop")]
            input_events,
            #[cfg(feature = "v2-public-control-desktop")]
            input_receiver: Mutex::new(Some(input_receiver)),
        }
    }

    #[cfg(feature = "v2-public-control-desktop")]
    fn round_elapsed(&self) -> bool {
        self.round_deadline
            .borrow()
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
    }

    fn request_stop(&self) {
        self.stop.send_replace(true);
        self.status.send_if_modified(|status| {
            if matches!(status.phase, Phase::Closed | Phase::Stopping) {
                return false;
            }
            status.phase = Phase::Stopping;
            status.connection_path = None;
            true
        });
    }

    pub(crate) async fn wait_finished(&self) -> bool {
        self.wait_finished_for(Duration::from_secs(5)).await
    }
    async fn wait_finished_for(&self, deadline: Duration) -> bool {
        let mut state = self.status.subscribe();
        tokio::time::timeout(deadline, state.wait_for(|s| s.phase == Phase::Closed))
            .await
            .is_ok_and(|r| r.is_ok())
    }
    fn snapshot(&self) -> Status {
        let mut status = self.status.borrow().clone();
        status.profile_consumed = self.profile_consumed.load(Ordering::Acquire);
        #[cfg(feature = "v2-public-control-desktop")]
        if self.control {
            status.round_seconds = self.round_seconds.get().copied();
            status.input_state = Some(if status.phase == Phase::Closed {
                InputState::Closed
            } else {
                *self.input_state.borrow()
            });
        }
        status
    }
}

struct CaptureLease(Arc<AtomicBool>);
impl Drop for CaptureLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct CloseSession(Arc<Session>);
impl Drop for CloseSession {
    fn drop(&mut self) {
        self.0.stop.send_replace(true);
        self.0.status.send_if_modified(|status| {
            if status.phase == Phase::Closed {
                return false;
            }
            *status = Status::closed(Err(UNEXPECTED_CLOSE));
            true
        });
    }
}

fn valid_generation(generation: &str) -> bool {
    generation.len() == 32
        && generation
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn session_origin(window: &WebviewWindow, owner: &str) -> Result<(), String> {
    #[cfg(feature = "v2-session-window")]
    return crate::experiment_origin::origin_for(window, owner);
    #[cfg(not(feature = "v2-session-window"))]
    {
        let _ = owner;
        crate::experiment_origin::origin(window)
    }
}

fn find(window: &WebviewWindow, generation: &str) -> Result<Arc<Session>, String> {
    if !valid_generation(generation) {
        return Err("PUBLIC_VIDEO_GENERATION".into());
    }
    let state = window.state::<Arc<DesktopVideoState>>();
    let session = state
        .active
        .lock()
        .map_err(|_| "PUBLIC_VIDEO_STATE")?
        .as_ref()
        .filter(|s| s.id == generation && s.owner == window.label())
        .cloned()
        .ok_or("PUBLIC_VIDEO_GENERATION")?;
    session_origin(window, &session.owner)?;
    Ok(session)
}

enum ProfileSource {
    Launch(PathBuf),
    #[cfg(feature = "v2-control-invitations")]
    Selected(Box<PublicVideoProfile>),
}

#[cfg(feature = "v2-control-invitations")]
#[tauri::command]
pub(crate) async fn select_control_invitation(
    window: WebviewWindow,
) -> Result<Option<Vec<crate::control_invitations::Preview>>, String> {
    crate::experiment_origin::origin(&window)?;
    let state = window.state::<Arc<DesktopVideoState>>().inner().clone();
    if !state.picker_available() || state.closing.load(Ordering::Acquire) {
        return Err("PUBLIC_VIDEO_PROFILE_UNAVAILABLE".into());
    }
    #[cfg(feature = "v2-session-window")]
    if session_window::reservation_busy(&state) {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let gate = window.state::<Arc<crate::ProbeState>>().0.clone();
    if gate.swap(true, Ordering::AcqRel) {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let _lease = CaptureLease(gate);
    if state
        .active
        .lock()
        .map_err(|_| "PUBLIC_VIDEO_STATE")?
        .as_ref()
        .is_some_and(|s| s.status.borrow().phase != Phase::Closed)
    {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let hwnd = window.hwnd().map_err(|_| "PUBLIC_VIDEO_NATIVE_WINDOW")?.0 as usize;
    let selected = tauri::async_runtime::spawn_blocking(move || {
        crate::control_invitations::choose(hwnd)?
            .map(|path| crate::control_invitations::load(&path, now()?))
            .transpose()
    })
    .await
    .map_err(|_| "PUBLIC_VIDEO_PROFILE_FILE")??;
    crate::experiment_origin::origin(&window)?;
    if state.closing.load(Ordering::Acquire) {
        return Err(CANCELLED.into());
    }
    let mut invitations = state.invitations.lock().map_err(|_| "PUBLIC_VIDEO_STATE")?;
    selected
        .map(|profile| invitations.select(profile, now()?))
        .transpose()
        .map_err(str::to_owned)
}

#[tauri::command]
pub(crate) fn public_video_start(
    window: WebviewWindow,
    role: String,
    invitation_id: Option<String>,
) -> Result<String, String> {
    crate::experiment_origin::origin(&window)?;
    #[cfg(feature = "v2-session-window")]
    if window.state::<Arc<DesktopVideoState>>().picker_available() {
        return Err("PUBLIC_SESSION_WINDOW_REQUIRED".into());
    }
    start_session(window, role, invitation_id)
}

fn start_session(
    window: WebviewWindow,
    role: String,
    invitation_id: Option<String>,
) -> Result<String, String> {
    session_origin(&window, window.label())?;
    let role = match role.as_str() {
        "host" => Role::Host,
        "viewer" => Role::Viewer,
        _ => return Err("PUBLIC_VIDEO_ROLE".into()),
    };
    let state = window.state::<Arc<DesktopVideoState>>().inner().clone();
    if !state.available() {
        return Err("PUBLIC_VIDEO_PROFILE_REQUIRED".into());
    }
    let mut active = state.active.lock().map_err(|_| "PUBLIC_VIDEO_STATE")?;
    #[cfg(feature = "v2-session-window")]
    if state.picker_available() && !session_window::still_open(&state, window.label()) {
        return Err("PUBLIC_SESSION_WINDOW_REQUIRED".into());
    }
    if state.closing.load(Ordering::Acquire) || state.used.load(Ordering::Acquire) {
        return Err("PUBLIC_VIDEO_PROFILE_UNAVAILABLE".into());
    }
    if active
        .as_ref()
        .is_some_and(|s| s.status.borrow().phase != Phase::Closed)
    {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let gate = window.state::<Arc<crate::ProbeState>>().0.clone();
    if gate.swap(true, Ordering::AcqRel) {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let lease = CaptureLease(gate);
    if window.state::<Arc<crate::p004::P004State>>().busy()
        || window.state::<Arc<crate::p004_lan::LanState>>().busy()
    {
        return Err("PUBLIC_VIDEO_BUSY".into());
    }
    let profile = match invitation_id {
        #[cfg(feature = "v2-control-invitations")]
        Some(id) if state.picker_available() => {
            let profile = state
                .invitations
                .lock()
                .map_err(|_| "PUBLIC_VIDEO_STATE")?
                .take(&id, role, now()?)?;
            ProfileSource::Selected(Box::new(profile))
        }
        Some(_) => return Err("PUBLIC_VIDEO_PROFILE_UNAVAILABLE".into()),
        None => ProfileSource::Launch(
            state
                .launch
                .profile()
                .ok_or("PUBLIC_VIDEO_PROFILE_UNAVAILABLE")?
                .to_path_buf(),
        ),
    };
    let session = Arc::new(Session::owned(
        role,
        state.launch.is_control(),
        window.label(),
    ));
    let id = session.id.clone();
    *active = Some(session.clone());
    drop(active);
    tauri::async_runtime::spawn(async move {
        let cleanup = CloseSession(session.clone());
        let result = run(window, state, profile, session.clone()).await;
        session.stop.send_replace(true);
        *session.receiver.lock().await = None;
        drop(lease);
        session.status.send_replace(Status::closed(result));
        drop(cleanup);
    });
    Ok(id)
}

#[tauri::command]
pub(crate) fn public_video_status(
    window: WebviewWindow,
    generation: String,
) -> Result<Status, String> {
    let session = find(&window, &generation)?;
    Ok(session.snapshot())
}

#[cfg(feature = "v2-public-control-desktop")]
#[tauri::command]
pub(crate) fn public_control_send_input(
    window: WebviewWindow,
    generation: String,
    event: String,
) -> Result<(), String> {
    let session = find(&window, &generation)?;
    if !session.control
        || session.role != Role::Viewer
        || *session.stop.borrow()
        || session.round_elapsed()
        || *session.input_state.borrow() != InputState::Allowed
        || session.status.borrow().phase != Phase::Ready
    {
        return Err("PUBLIC_CONTROL_NOT_ALLOWED".into());
    }
    use hub_webrtc::remote_input::Event;
    if event.is_empty() || event.len() > 256 {
        session.request_stop();
        return Err("PUBLIC_CONTROL_EVENT_REJECTED".into());
    }
    let parsed = serde_json::from_str::<Event>(&event);
    if !matches!(
        &parsed,
        Ok(Event::Move { .. } | Event::Button { .. } | Event::Key { .. } | Event::Release { .. })
    ) {
        session.request_stop();
        return Err("PUBLIC_CONTROL_EVENT_REJECTED".into());
    }
    #[cfg(feature = "v2-session-window")]
    if session.owner != "main"
        && (!window.is_focused().unwrap_or(false) || window.is_minimized().unwrap_or(true))
        && !matches!(parsed, Ok(Event::Release { .. }))
    {
        // This event did not leave the native window. Reporting the pause
        // lets the viewer clear pending events and send release even if it
        // has not yet received the separate native focus notification.
        return Err("PUBLIC_CONTROL_WINDOW_PAUSED".into());
    }
    session.input_events.try_send(event).map_err(|_| {
        session.request_stop();
        "PUBLIC_CONTROL_BACKPRESSURE".to_owned()
    })
}

#[tauri::command]
pub(crate) async fn public_video_stop(
    window: WebviewWindow,
    generation: String,
) -> Result<(), String> {
    let session = find(&window, &generation)?;
    session.request_stop();
    if !session.wait_finished_for(Duration::from_secs(3)).await {
        return Err("PUBLIC_VIDEO_STOP_UNCONFIRMED".into());
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn public_video_next_frame(
    window: WebviewWindow,
    generation: String,
) -> Result<Response, String> {
    let session = find(&window, &generation)?;
    if session.role != Role::Viewer {
        return Err("PUBLIC_VIDEO_ROLE".into());
    }
    let mut stop = session.stop.subscribe();
    if *stop.borrow() {
        return Ok(Response::new(Vec::<u8>::new()));
    }
    if session.status.borrow().phase != Phase::Ready {
        return Ok(Response::new(Vec::<u8>::new()));
    }
    #[cfg(feature = "v2-public-control-desktop")]
    if session.round_elapsed() {
        return Ok(Response::new(Vec::<u8>::new()));
    }
    let mut guard = session
        .receiver
        .try_lock()
        .map_err(|_| "PUBLIC_VIDEO_PULL_BUSY")?;
    let Some(receiver) = guard.as_mut() else {
        // The renderer normally waits for ready; empty is safe while pending.
        return Ok(Response::new(Vec::<u8>::new()));
    };
    let result = tokio::select! {
        biased;
        _ = stopped(&mut stop) => return Ok(Response::new(Vec::<u8>::new())),
        result = tokio::time::timeout(PULL_TIMEOUT, receiver.next_frame()) => result,
    };
    session_origin(&window, &session.owner)?;
    if *stop.borrow() {
        return Ok(Response::new(Vec::<u8>::new()));
    }
    match result {
        Ok(Ok(frame)) => {
            let bytes = frame_bytes(frame)?;
            if *stop.borrow() {
                return Ok(Response::new(Vec::<u8>::new()));
            }
            #[cfg(feature = "v2-public-control-desktop")]
            if session.round_elapsed() {
                return Ok(Response::new(Vec::<u8>::new()));
            }
            Ok(Response::new(bytes))
        }
        Ok(Err(_)) | Err(_) => Ok(Response::new(Vec::<u8>::new())),
    }
}

fn frame_bytes(frame: VerifiedVideoFrame) -> Result<Vec<u8>, &'static str> {
    let len = frame.annex_b.len();
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err("PUBLIC_VIDEO_FRAME_SIZE");
    }
    let mut bytes = Vec::with_capacity(24 + len);
    bytes.extend_from_slice(&frame.sequence.to_le_bytes());
    bytes.extend_from_slice(&frame.timestamp_us.to_le_bytes());
    bytes.extend_from_slice(&(len as u32).to_le_bytes());
    bytes.extend_from_slice(&[u8::from(frame.keyframe), 0, 0, 0]);
    bytes.extend_from_slice(&frame.annex_b);
    Ok(bytes)
}

async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|value| *value).await;
}
fn now() -> Result<u64, &'static str> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| "PUBLIC_VIDEO_CLOCK")
}

async fn run(
    window: WebviewWindow,
    state: Arc<DesktopVideoState>,
    source: ProfileSource,
    session: Arc<Session>,
) -> Result<Outcome, &'static str> {
    let mut stop = session.stop.subscribe();
    if *stop.borrow() {
        return Err(CANCELLED);
    }
    let peer = window
        .state::<crate::v2_pairing::PairingState>()
        .current()
        .map_err(|_| "PUBLIC_VIDEO_VERIFY_PEER_FIRST")?;
    let control_launch = state.launch.is_control();
    let (mut profile, identity) = tauri::async_runtime::spawn_blocking(move || {
        let profile = match source {
            #[cfg(feature = "v2-control-invitations")]
            ProfileSource::Selected(profile) => *profile,
            #[cfg(feature = "v2-public-control-desktop")]
            ProfileSource::Launch(path) if control_launch => {
                PublicVideoProfile::load_control(&path, now()?)?
            }
            ProfileSource::Launch(path) => PublicVideoProfile::load(&path, now()?)?,
        };
        let identity =
            hub_identity::open_existing_default().map_err(|_| "PUBLIC_VIDEO_IDENTITY")?;
        Ok::<_, &'static str>((profile, identity))
    })
    .await
    .map_err(|_| "PUBLIC_VIDEO_PROFILE")??;
    let approval = profile.approval();
    if approval.role != session.role
        || (!control_launch && approval.scope.mode != IceMode::RelayOnly)
    {
        return Err("PUBLIC_VIDEO_ROLE_OR_ROUTE");
    }
    if profile.expected_peer_fingerprint() != peer.fingerprint() {
        return Err("PUBLIC_VIDEO_PEER_MISMATCH");
    }
    if *stop.borrow() {
        return Err(CANCELLED);
    }
    #[cfg(feature = "v2-public-control-desktop")]
    let input_timing = if control_launch && profile.is_control_batch() {
        ControlTiming::Batch
    } else {
        ControlTiming::Legacy
    };
    let approved_window = public_video_capture::CAPTURE_WINDOW;
    #[cfg(feature = "v2-public-control-desktop")]
    let approved_window = if control_launch {
        let window = input_timing.window();
        let _ = session.round_seconds.set(window.as_secs() as u16);
        window
    } else {
        approved_window
    };
    let hwnd = window.hwnd().map_err(|_| "PUBLIC_VIDEO_NATIVE_WINDOW")?.0 as usize;
    let action = if session.role == Role::Host {
        "共享这台电脑的主屏，不含声音"
    } else {
        "观看对方主屏，不含声音"
    };
    let input_notice = if control_launch {
        "这次连接不批准输入。共享端随后需要独立批准当前会话的键鼠操作。拒绝时仍可仅观看。"
    } else {
        "不允许键鼠或无人值守访问。"
    };
    let route_notice = if approval.scope.mode == IceMode::DirectPreferred {
        "优先公网直连，不通时自动使用 TURN 中继。"
    } else {
        "只允许中继。"
    };
    let message = format!(
        "本次仅{action}，共享最多 {} 秒。{route_notice}{input_notice}点「停止」或关窗可结束。\n\n固定 WSS：{}\n固定 TURN：{}\n已核验对方指纹：\n{}\n\n是否同意本次连接？",
        approved_window.as_secs(),
        approval.scope.wss, approval.scope.turn, peer.fingerprint(),
    );
    let title = if control_launch {
        "RemoteHub · 公网短时会话：画面授权"
    } else if cfg!(feature = "v2-public-video-60s-test") {
        "RemoteHub · 公网仅视频（60 秒测试版）"
    } else {
        "RemoteHub · 公网短时仅视频"
    };
    let confirmed = tauri::async_runtime::spawn_blocking(move || unsafe {
        MessageBoxW(
            Some(HWND(hwnd as *mut _)),
            &HSTRING::from(message),
            &HSTRING::from(title),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        ) == IDYES
    })
    .await
    .map_err(|_| "PUBLIC_VIDEO_CONSENT_DIALOG")?;
    if !confirmed {
        return Err("NATIVE_CONSENT_DENIED");
    }
    if *stop.borrow() || state.closing.load(Ordering::Acquire) {
        return Err(CANCELLED);
    }
    session_origin(&window, &session.owner).map_err(|_| "PUBLIC_VIDEO_ORIGIN")?;
    let (phase_tx, mut phase_rx) = watch::channel(ScopedWssState::Negotiating);
    let watch_session = session.clone();
    let monitor = async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(200)) => {
                    if session_origin(&window, &watch_session.owner).is_err() {
                        watch_session.request_stop();
                        return;
                    }
                }
                event = phase_rx.changed() => {
                    if event.is_err() { return; }
                    let phase = *phase_rx.borrow_and_update();
                    watch_session.status.send_if_modified(|s| s.observe(watch_session.role, phase));
                }
            }
        }
    };
    let worker = async {
        let connect = async {
            #[cfg(feature = "v2-public-control-desktop")]
            if control_launch {
                let config = profile.take_control_config(&identity, &peer, now()?, true)?;
                if matches!(state.launch, LaunchMode::Invitations) {
                    #[cfg(feature = "v2-control-invitations")]
                    state
                        .invitations
                        .lock()
                        .map_err(|_| "PUBLIC_VIDEO_STATE")?
                        .consume(approval.room, approval.scope.expires_at)?;
                } else {
                    state.used.store(true, Ordering::Release);
                }
                session.profile_consumed.store(true, Ordering::Release);
                return PublicVideoWss::connect_public_control_wss(config).await;
            }
            let config = profile.take_config(&identity, &peer, now()?, true)?;
            state.used.store(true, Ordering::Release);
            session.profile_consumed.store(true, Ordering::Release);
            PublicVideoWss::connect_public_wss(config).await
        };
        let pending = tokio::select! {
            biased;
            _ = stopped(&mut stop) => return Err(CANCELLED),
            pending = connect => pending?,
        };
        let actor = tokio::select! {
            biased;
            _ = stopped(&mut stop) => return Err(CANCELLED),
            actor = pending.establish() => actor?,
        };
        #[cfg(feature = "v2-public-control-desktop")]
        let (first_video_sent, first_video) = watch::channel(false);
        #[cfg(feature = "v2-public-control-desktop")]
        let run_input = {
            let owner = session.clone();
            let fingerprint = peer.fingerprint().to_owned();
            move |transport, input_stop| async move {
                if owner.role == Role::Host {
                    public_control_desktop::run_host(
                        transport,
                        input_stop,
                        hwnd,
                        fingerprint,
                        first_video,
                        input_timing,
                        owner.input_state.clone(),
                    )
                    .await
                } else {
                    let input_receiver = owner
                        .input_receiver
                        .lock()
                        .map_err(|_| "PUBLIC_CONTROL_STATE")?
                        .take()
                        .ok_or("PUBLIC_CONTROL_STATE")?;
                    public_control_desktop::run_viewer(
                        transport,
                        input_stop,
                        input_receiver,
                        input_timing,
                        owner.input_state.clone(),
                    )
                    .await
                }
            }
        };
        if session.role == Role::Host {
            let (media, sender) = ScopedVideoMedia::host();
            #[cfg(feature = "v2-public-control-desktop")]
            let media = {
                let mut media = media;
                if control_launch {
                    media.notify_first_host_video(first_video_sent);
                }
                media
            };
            let phase = phase_tx.subscribe();
            let actor_stop = stop.clone();
            let capture_timing = CaptureTiming::VideoOnly(public_video_capture::CAPTURE_WINDOW);
            #[cfg(feature = "v2-public-control-desktop")]
            let capture_timing = if control_launch && matches!(input_timing, ControlTiming::Batch) {
                CaptureTiming::Control(session.round_deadline.subscribe())
            } else {
                capture_timing
            };
            let run_actor = async {
                #[cfg(feature = "v2-public-control-desktop")]
                if control_launch {
                    if matches!(input_timing, ControlTiming::Batch) {
                        return actor
                            .run_with_media_and_control_batch(
                                actor_stop,
                                phase_tx,
                                media,
                                session.round_deadline.clone(),
                                run_input,
                            )
                            .await;
                    }
                    return actor
                        .run_with_media_and_control(actor_stop, phase_tx, media, run_input)
                        .await;
                }
                actor.run_with_media(actor_stop, phase_tx, media).await
            };
            let (actor_result, capture_result) = tokio::join!(
                run_actor,
                public_video_capture::run(sender, stop, phase, capture_timing)
            );
            let locally_stopped = *session.stop.borrow();
            Ok(host_outcome(actor_result, capture_result, locally_stopped))
        } else {
            let (media, receiver) = ScopedVideoMedia::viewer();
            *session.receiver.lock().await = Some(receiver);
            let actor = async {
                #[cfg(feature = "v2-public-control-desktop")]
                if control_launch {
                    if matches!(input_timing, ControlTiming::Batch) {
                        return actor
                            .run_with_media_and_control_batch(
                                stop,
                                phase_tx,
                                media,
                                session.round_deadline.clone(),
                                run_input,
                            )
                            .await;
                    }
                    return actor
                        .run_with_media_and_control(stop, phase_tx, media, run_input)
                        .await;
                }
                actor.run_with_media(stop, phase_tx, media).await
            }
            .await;
            Ok(Outcome {
                terminal: actor_terminal(actor),
                elapsed_ms: None,
            })
        }
    };
    let mut worker = std::pin::pin!(worker);
    tokio::select! {
        result = &mut worker => result,
        () = monitor => worker.await,
    }
}

pub(crate) fn close_window(window: &tauri::Window) -> Option<Arc<Session>> {
    let state = window.state::<Arc<DesktopVideoState>>();
    state.closing.store(true, Ordering::Release);
    let session = state.active.lock().ok()?.clone();
    if let Some(session) = &session {
        session.request_stop();
    }
    session
}

pub(crate) fn allow_close_retry(window: &tauri::Window) {
    window
        .state::<Arc<DesktopVideoState>>()
        .closing
        .store(false, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_terminal_reasons_distinguish_limit_source_stop_and_peer() {
        for (actor, capture, expected) in [
            (ScopedWssEnd::Stopped, CaptureEnd::Limit, "capture_limit"),
            (
                ScopedWssEnd::Stopped,
                CaptureEnd::SourceEnded,
                "capture_ended",
            ),
            (ScopedWssEnd::Stopped, CaptureEnd::LocalStop, "local_stop"),
            (
                ScopedWssEnd::PeerLeft,
                CaptureEnd::TransportEnded,
                "peer_left",
            ),
        ] {
            let status = Status::closed(Ok(host_outcome(
                Ok(actor),
                CaptureOutcome {
                    end: Ok(capture),
                    elapsed_ms: Some(1234),
                },
                false,
            )));
            let json = serde_json::to_value(status).unwrap();
            assert_eq!(json["phase"], "closed");
            assert_eq!(json["end_reason"], expected);
            assert_eq!(json["elapsed_ms"], 1234);
            assert!(json["error"].is_null());
        }
    }

    #[test]
    fn route_status_requires_verified_local_transport_and_clears_on_stop() {
        use hub_webrtc::direct_policy::{Kind, SelectedRoute};
        for role in [Role::Host, Role::Viewer] {
            let session = Session::new(role, true);
            let mut status = session.status.borrow().clone();
            status.observe(role, ScopedWssState::Established);
            assert!(status.connection_path.is_none());
            for route in [
                SelectedRoute::ServerReflexiveDirect,
                SelectedRoute::TurnRelay,
            ] {
                let event = if role == Role::Host {
                    ScopedWssState::HostPathVerified(route, Kind::Srflx, Kind::Relay)
                } else {
                    ScopedWssState::ViewerPathVerified(route, Kind::Srflx, Kind::Relay)
                };
                assert!(status.observe(role, event));
                let value = serde_json::to_value(&status).unwrap();
                assert_eq!(
                    value["connection_path"],
                    if route == SelectedRoute::TurnRelay {
                        "relay"
                    } else {
                        "direct"
                    }
                );
                session.status.send_replace(status.clone());
                session.request_stop();
                let mut stopped = session.status.borrow().clone();
                assert!(stopped.connection_path.is_none());
                assert!(!stopped.observe(role, event));
            }
        }
    }

    #[test]
    fn viewer_has_no_capture_timer_and_errors_remain_failures() {
        let viewer = Status::closed(Ok(Outcome {
            terminal: actor_terminal(Ok(ScopedWssEnd::PeerLeft)),
            elapsed_ms: None,
        }));
        let json = serde_json::to_value(viewer).unwrap();
        assert_eq!(json["end_reason"], "peer_left");
        assert!(json.get("elapsed_ms").is_none());

        let initial =
            serde_json::to_value(&*Session::new(Role::Host, false).status.borrow()).unwrap();
        assert!(initial.get("end_reason").is_none());
        assert!(initial.get("elapsed_ms").is_none());

        let stopped_early = serde_json::to_value(Status::closed(Err(CANCELLED))).unwrap();
        assert_eq!(stopped_early["end_reason"], "local_stop");
        assert!(stopped_early.get("elapsed_ms").is_none());
        assert!(stopped_early["error"].is_null());

        let failed = Status::closed(Ok(host_outcome(
            Ok(ScopedWssEnd::PeerLeft),
            CaptureOutcome {
                end: Err("PUBLIC_VIDEO_CAPTURE_INVALID_AU"),
                elapsed_ms: None,
            },
            false,
        )));
        let json = serde_json::to_value(failed).unwrap();
        assert_eq!(json["end_reason"], "failed");
        assert_eq!(json["error"], "PUBLIC_VIDEO_CAPTURE_INVALID_AU");
    }

    #[test]
    fn frame_has_fixed_little_endian_header_and_exact_bytes() {
        let bytes = frame_bytes(VerifiedVideoFrame {
            sequence: 7,
            timestamp_us: 42,
            keyframe: true,
            annex_b: vec![0, 0, 0, 1, 0x65],
        })
        .unwrap();
        assert_eq!(&bytes[..8], &7u64.to_le_bytes());
        assert_eq!(&bytes[8..16], &42u64.to_le_bytes());
        assert_eq!(&bytes[16..24], &[5, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&bytes[24..], &[0, 0, 0, 1, 0x65]);
    }
}
