use crate::ProbeState;
use hub_encode_win::{
    h264::EncodedVideoFrame, lifecycle::StopToken, stream, EncodeError, EncoderConfig,
};
use hub_webrtc::{MediaFrame, Session, TransportError, TransportStats};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};
use tauri::{Builder, Context, Manager, State, WebviewWindow, WindowEvent, Wry};
use tokio::sync::{mpsc, oneshot, watch};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
};

const SIGNAL_LIMIT: Duration = Duration::from_secs(12);
const TRANSPORT_LIMIT: Duration = Duration::from_secs(8);
const QUEUE_SIZE: usize = 4;
const STOPPED: &str = "STOPPED";

type Outcome<T> = Result<T, &'static str>;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    seconds: u32,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u32,
}

impl Config {
    fn encoder(self) -> Outcome<EncoderConfig> {
        if !(5..=30).contains(&self.seconds)
            || (self.width, self.height, self.fps, self.bitrate_bps) != (1280, 720, 30, 4_000_000)
        {
            return Err("INVALID_CONFIG");
        }
        Ok(EncoderConfig {
            width: self.width,
            height: self.height,
            fps: self.fps,
            bitrate: self.bitrate_bps,
            seconds: self.seconds,
        })
    }
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    #[default]
    Starting,
    Negotiating,
    Connected,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stats {
    state: Phase,
    frames_encoded: u64,
    encoded_bytes: u64,
    #[serde(flatten)]
    transport: TransportStats,
    first_pts_100ns: Option<i64>,
    last_pts_100ns: Option<i64>,
    idr_frames: u64,
    backpressure_count: u64,
    submitted_frames: Option<u32>,
    missed_ticks: Option<u32>,
    repeated_frames: Option<u32>,
    encoder: Option<String>,
    hardware: Option<bool>,
    source_width: Option<u32>,
    source_height: Option<u32>,
    encoder_setup_ms: Option<f64>,
    elapsed_ms: u64,
    error_code: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartResponse {
    session_id: String,
    offer: String,
}

#[derive(Default)]
pub(crate) struct LoopbackState {
    next: AtomicU64,
    closing: AtomicBool,
    sessions: Mutex<Sessions>,
}

#[derive(Default)]
struct Sessions {
    active: Option<Arc<Run>>,
    previous: Option<(String, Stats)>,
}

struct Run {
    id: String,
    created: Instant,
    stats: Mutex<Stats>,
    stop: StopToken,
    cancel: watch::Sender<bool>,
    done: watch::Sender<bool>,
    answer: Mutex<Option<oneshot::Sender<String>>>,
}

struct CapturePermit(Arc<ProbeState>);

impl Drop for CapturePermit {
    fn drop(&mut self) {
        self.0 .0.store(false, Ordering::Release);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Run {
    fn stop(&self) {
        let mut stats = lock(&self.stats);
        if !matches!(stats.state, Phase::Stopped | Phase::Failed) {
            stats.state = Phase::Stopping;
        }
        self.stop.stop();
        self.cancel.send_replace(true);
    }

    fn phase(&self, phase: Phase) {
        let mut stats = lock(&self.stats);
        if !self.stop.is_stopped() {
            stats.state = phase;
        }
    }

    fn snapshot(&self) -> Stats {
        let mut stats = lock(&self.stats).clone();
        if !matches!(stats.state, Phase::Stopped | Phase::Failed) {
            stats.elapsed_ms = self.created.elapsed().as_millis() as u64;
        }
        stats
    }

    fn transport_stats(&self, stats: TransportStats) {
        lock(&self.stats).transport = stats;
    }
}

fn app_url(url: &tauri::Url) -> bool {
    matches!(
        (url.scheme(), url.host_str()),
        ("https", Some("tauri.localhost")) | ("tauri", Some("localhost"))
    ) && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.path(), "/" | "/index.html")
        && url.query().is_none()
}

fn trusted_window(window: &WebviewWindow) -> Outcome<()> {
    if window.label() == "main" && window.url().is_ok_and(|url| app_url(&url)) {
        Ok(())
    } else {
        Err("WINDOW_NOT_ALLOWED")
    }
}

fn transport_error(error: TransportError) -> &'static str {
    match error {
        TransportError::WebRtc => "WEBRTC_FAILED",
        TransportError::NoLoopbackCandidate => "ICE_LOOPBACK_UNAVAILABLE",
        TransportError::IcePolicy => "ICE_NOT_LOOPBACK",
        TransportError::Sdp => "INVALID_SDP",
        TransportError::Timeout => "ICE_OR_RTP_TIMEOUT",
        TransportError::Disconnected => "ICE_DISCONNECTED",
        TransportError::Profile => "H264_PROFILE_MISMATCH",
        TransportError::AccessUnit => "INVALID_H264_AU",
        TransportError::Timestamp => "INVALID_H264_PTS",
        TransportError::Random => "RANDOM_UNAVAILABLE",
    }
}

async fn native_consent(window: WebviewWindow, run: Arc<Run>, seconds: u32) -> Outcome<()> {
    let (tx, rx) = oneshot::channel();
    let modal = window.clone();
    let prompt_run = run.clone();
    window
        .run_on_main_thread(move || {
            let approved = if prompt_run.stop.is_stopped() || trusted_window(&modal).is_err() {
                false
            } else if let Ok(hwnd) = modal.hwnd() {
                let title: Vec<u16> = "RemoteHub · 本机视频实验\0".encode_utf16().collect();
                let message: Vec<u16> = format!(
                    "允许 RemoteHub 在此电脑上采集主屏并在本窗口播放，最长 {seconds} 秒？\n\n画面不会保存到文件，也不会发送到其他电脑。\0"
                )
                .encode_utf16()
                .collect();
                // The owner is this trusted native window; the strings are NUL-terminated.
                (unsafe {
                    MessageBoxW(hwnd.0, message.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
                }) == IDYES
            } else {
                false
            };
            let _ = tx.send(approved);
        })
        .map_err(|_| "CONSENT_REQUIRED")?;

    // Do not release the permit while a native prompt is still visible; late approval
    // cannot resurrect a cancelled run and a second prompt cannot be opened concurrently.
    if !rx.await.unwrap_or(false) || run.stop.is_stopped() {
        return Err(if run.stop.is_stopped() {
            STOPPED
        } else {
            "CONSENT_REQUIRED"
        });
    }
    Ok(())
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    let _ = cancel.wait_for(|v| *v).await;
}

async fn step<T>(
    cancel: &mut watch::Receiver<bool>,
    limit: Duration,
    future: impl Future<Output = Outcome<T>>,
    timeout_code: &'static str,
) -> Outcome<T> {
    tokio::select! {
        biased;
        () = cancelled(cancel) => Err(STOPPED),
        result = tokio::time::timeout(limit, future) => result.unwrap_or(Err(timeout_code)),
    }
}

#[tauri::command]
pub(crate) async fn p004_start_loopback_probe(
    config: Config,
    window: WebviewWindow,
    state: State<'_, Arc<LoopbackState>>,
    capture: State<'_, Arc<ProbeState>>,
) -> Outcome<StartResponse> {
    trusted_window(&window)?;
    let encoder = config.encoder()?;
    let shared = state.inner().clone();
    let (opening, result) = oneshot::channel();
    let (answer_tx, answer_rx) = oneshot::channel();
    let (run, permit) = {
        let mut sessions = lock(&shared.sessions);
        if shared.closing.load(Ordering::Acquire) {
            return Err("WINDOW_NOT_ALLOWED");
        }
        if sessions.active.is_some() || capture.0.swap(true, Ordering::AcqRel) {
            return Err("BUSY");
        }
        let run = Arc::new(Run {
            id: (shared.next.fetch_add(1, Ordering::Relaxed) + 1).to_string(),
            created: Instant::now(),
            stats: Mutex::new(Stats::default()),
            stop: StopToken::default(),
            cancel: watch::channel(false).0,
            done: watch::channel(false).0,
            answer: Mutex::new(Some(answer_tx)),
        });
        sessions.active = Some(run.clone());
        (run, CapturePermit(capture.inner().clone()))
    };

    tauri::async_runtime::spawn(supervise(
        window, shared, run, permit, encoder, opening, answer_rx,
    ));
    result.await.unwrap_or(Err("SESSION_FAILED"))
}

#[tauri::command]
pub(crate) fn p004_signal(
    session_id: String,
    answer: String,
    window: WebviewWindow,
    state: State<'_, Arc<LoopbackState>>,
) -> Outcome<()> {
    trusted_window(&window)?;
    if answer.is_empty() || answer.len() > 32 * 1024 {
        return Err("INVALID_ANSWER");
    }
    let sessions = lock(&state.sessions);
    let run = sessions
        .active
        .as_ref()
        .filter(|run| run.id == session_id && !run.stop.is_stopped())
        .ok_or("SESSION_NOT_FOUND")?;
    if !matches!(lock(&run.stats).state, Phase::Negotiating) {
        return Err("SESSION_NOT_NEGOTIATING");
    }
    let tx = lock(&run.answer).take().ok_or("ANSWER_ALREADY_SUBMITTED")?;
    tx.send(answer).map_err(|_| "SESSION_NOT_NEGOTIATING")
}

#[tauri::command]
pub(crate) fn p004_get_stats(
    session_id: String,
    window: WebviewWindow,
    state: State<'_, Arc<LoopbackState>>,
) -> Outcome<Stats> {
    trusted_window(&window)?;
    let sessions = lock(&state.sessions);
    if let Some(active) = sessions.active.as_ref().filter(|r| r.id == session_id) {
        Ok(active.snapshot())
    } else {
        sessions
            .previous
            .as_ref()
            .filter(|(id, _)| *id == session_id)
            .map(|(_, stats)| stats.clone())
            .ok_or("SESSION_NOT_FOUND")
    }
}

#[tauri::command]
pub(crate) fn p004_stop_loopback_probe(
    session_id: Option<String>,
    window: WebviewWindow,
    state: State<'_, Arc<LoopbackState>>,
) -> Outcome<()> {
    trusted_window(&window)?;
    if let Some(run) = lock(&state.sessions)
        .active
        .as_ref()
        .filter(|r| session_id.as_deref().is_none_or(|id| id == r.id))
    {
        run.stop();
    }
    Ok(())
}

async fn supervise(
    window: WebviewWindow,
    state: Arc<LoopbackState>,
    run: Arc<Run>,
    permit: CapturePermit,
    config: EncoderConfig,
    opening: oneshot::Sender<Outcome<StartResponse>>,
    answer: oneshot::Receiver<String>,
) {
    let mut opening = Some(opening);
    let mut cancel = run.cancel.subscribe();
    let result = async {
        native_consent(window, run.clone(), config.seconds).await?;
        if opening.as_ref().is_none_or(oneshot::Sender::is_closed) {
            return Err(STOPPED);
        }
        let mut session = step(
            &mut cancel,
            TRANSPORT_LIMIT,
            async { Session::new().await.map_err(transport_error) },
            "PEER_INIT_TIMEOUT",
        )
        .await?;
        let result = negotiate_and_stream(
            &run,
            &mut cancel,
            config,
            &mut opening,
            answer,
            &mut session,
        )
        .await;
        if let Ok(stats) =
            tokio::time::timeout(Duration::from_millis(250), session.refresh_stats()).await
        {
            run.transport_stats(stats);
        }
        let closed = tokio::time::timeout(TRANSPORT_LIMIT, session.close()).await;
        if !matches!(closed, Ok(Ok(()))) && matches!(result, Ok(()) | Err(STOPPED)) {
            return Err("PEER_CLOSE_FAILED");
        }
        result
    }
    .await;

    // Publish the terminal snapshot only after both transport and producer cleanup.
    {
        let mut sessions = lock(&state.sessions);
        let mut stats = lock(&run.stats);
        stats.elapsed_ms = run.created.elapsed().as_millis() as u64;
        match result {
            Ok(()) | Err(STOPPED) => stats.state = Phase::Stopped,
            Err(code) => {
                stats.state = Phase::Failed;
                stats.error_code = Some(code);
            }
        }
        drop(permit);
        sessions.previous = Some((run.id.clone(), stats.clone()));
        sessions.active = None;
    }
    if let Some(reply) = opening {
        let _ = reply.send(Err(result.err().unwrap_or(STOPPED)));
    }
    run.done.send_replace(true);
}

async fn negotiate_and_stream(
    run: &Arc<Run>,
    cancel: &mut watch::Receiver<bool>,
    config: EncoderConfig,
    opening: &mut Option<oneshot::Sender<Outcome<StartResponse>>>,
    answer: oneshot::Receiver<String>,
    session: &mut Session,
) -> Outcome<()> {
    let offer = step(
        cancel,
        TRANSPORT_LIMIT,
        async { session.create_offer().await.map_err(transport_error) },
        "ICE_GATHER_TIMEOUT",
    )
    .await?;
    run.phase(Phase::Negotiating);
    opening
        .take()
        .ok_or(STOPPED)?
        .send(Ok(StartResponse {
            session_id: run.id.clone(),
            offer,
        }))
        .map_err(|_| STOPPED)?;

    let answer = step(
        cancel,
        SIGNAL_LIMIT,
        async { answer.await.map_err(|_| STOPPED) },
        "SIGNAL_TIMEOUT",
    )
    .await?;
    step(
        cancel,
        TRANSPORT_LIMIT,
        async { session.accept_answer(answer).await.map_err(transport_error) },
        "ANSWER_TIMEOUT",
    )
    .await?;
    step(
        cancel,
        SIGNAL_LIMIT,
        async { session.wait_connected().await.map_err(transport_error) },
        "ICE_CONNECT_TIMEOUT",
    )
    .await?;
    run.phase(Phase::Connected);
    run.transport_stats(session.stats());
    stream_video(run, cancel, config, session).await
}

async fn stream_video(
    run: &Arc<Run>,
    cancel: &mut watch::Receiver<bool>,
    config: EncoderConfig,
    session: &mut Session,
) -> Outcome<()> {
    let (tx, mut au_rx) = mpsc::channel::<EncodedVideoFrame>(QUEUE_SIZE);
    let encoder_run = run.clone();
    let producer = tauri::async_runtime::spawn_blocking(move || {
        stream::run(
            true,
            config,
            encoder_run.stop.clone(),
            |info| {
                let mut stats = lock(&encoder_run.stats);
                stats.encoder = Some(info.encoder);
                stats.hardware = Some(info.hardware);
                stats.source_width = Some(info.source_width);
                stats.source_height = Some(info.source_height);
                stats.encoder_setup_ms = Some(info.setup_ms);
            },
            |au| {
                let mut stats = lock(&encoder_run.stats);
                stats.frames_encoded += 1;
                stats.encoded_bytes += au.annex_b.len() as u64;
                stats.first_pts_100ns.get_or_insert(au.pts_100ns);
                stats.last_pts_100ns = Some(au.pts_100ns);
                stats.idr_frames += u64::from(au.is_idr);
                if encoder_run.stop.is_stopped() {
                    return Err(EncodeError::Cancelled);
                }
                match tx.try_send(au) {
                    Ok(()) => Ok(()),
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        stats.backpressure_count += 1;
                        // Do not send any more dependent frames after a queue overflow.
                        encoder_run.stop.stop();
                        encoder_run.cancel.send_replace(true);
                        Err(EncodeError::Backpressure)
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => Err(EncodeError::Cancelled),
                }
            },
        )
    });

    let result = step(
        cancel,
        Duration::from_secs(u64::from(config.seconds) + 15),
        async {
            let mut next_stats = Instant::now() + Duration::from_secs(1);
            loop {
                let au = match tokio::time::timeout(Duration::from_millis(200), au_rx.recv()).await
                {
                    Ok(Some(au)) => au,
                    Ok(None) => break,
                    Err(_) if !session.is_connected() => return Err("ICE_DISCONNECTED"),
                    Err(_) => continue,
                };
                if run.stop.is_stopped() {
                    return Err(STOPPED);
                }
                session
                    .write_frame(MediaFrame {
                        annex_b: au.annex_b,
                        pts_100ns: au.pts_100ns,
                        duration_100ns: au.duration_100ns,
                    })
                    .await
                    .map_err(transport_error)?;
                let stats = if Instant::now() >= next_stats {
                    next_stats = Instant::now() + Duration::from_secs(1);
                    tokio::time::timeout(Duration::from_millis(35), session.refresh_stats())
                        .await
                        .unwrap_or_else(|_| session.stats())
                } else {
                    session.stats()
                };
                run.transport_stats(stats);
            }
            Ok(())
        },
        "MEDIA_TIMEOUT",
    )
    .await;

    if result.is_err() {
        run.stop.stop();
    }
    drop(au_rx);
    match producer.await {
        Ok(Ok(report)) => {
            let mut stats = lock(&run.stats);
            stats.submitted_frames = Some(report.submitted_frames);
            stats.missed_ticks = Some(report.missed_ticks);
            stats.repeated_frames = Some(report.repeated_frames);
            result
        }
        Ok(Err(EncodeError::Backpressure)) => Err("MEDIA_BACKPRESSURE"),
        Ok(Err(EncodeError::Cancelled)) if result.is_err() => result,
        Ok(Err(EncodeError::Busy)) => Err("ENCODER_BUSY"),
        Ok(Err(_)) => Err("ENCODER_FAILED"),
        Err(_) => Err("ENCODER_THREAD_FAILED"),
    }
}

pub(crate) fn configure(
    mut builder: Builder<Wry>,
    mut context: Context<Wry>,
) -> (Builder<Wry>, Context<Wry>) {
    // Create only the embedded main application origin with the experimental WebView2 profile.
    context.config_mut().app.windows[0].create = false;
    builder = builder
        .manage(Arc::new(LoopbackState::default()))
        .setup(|app| {
            let mut config = app.config().app.windows[0].clone();
            config.use_https_scheme = true;
            let profile = app.path().app_local_data_dir()?.join("p004-webview");
            tauri::WebviewWindowBuilder::from_config(app, &config)?
                .data_directory(profile)
                .additional_browser_args(
                    "--allow-loopback-in-peer-connection --disable-features=WebRtcHideLocalIpsWithMdns",
                )
                .on_navigation(app_url)
                .build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            let state = window.state::<Arc<LoopbackState>>();
            if let WindowEvent::CloseRequested { api, .. } = event {
                let sessions = lock(&state.sessions);
                let already_closing = state.closing.swap(true, Ordering::AcqRel);
                if let Some(run) = sessions.active.as_ref() {
                    api.prevent_close();
                    if already_closing {
                        return;
                    }
                    let mut done = run.done.subscribe();
                    run.stop();
                    let window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = done.wait_for(|value| *value).await;
                        let _ = window.destroy();
                    });
                }
            } else if matches!(event, WindowEvent::Destroyed) {
                if let Some(run) = lock(&state.sessions).active.as_ref() {
                    run.stop();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            crate::get_runtime_info,
            crate::run_capture_probe,
            p004_start_loopback_probe,
            p004_signal,
            p004_get_stats,
            p004_stop_loopback_probe
        ]);
    (builder, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_embedded_main_origin_and_media_config_are_accepted() {
        let url = |s| tauri::Url::parse(s).unwrap();
        assert!(app_url(&url("https://tauri.localhost/index.html")));
        assert!(!app_url(&url("http://tauri.localhost/index.html")));
        assert!(!app_url(&url("https://tauri.localhost:1420/index.html")));
        assert!(!app_url(&url("https://tauri.localhost.evil/")));
        assert!(!app_url(&url("https://tauri.localhost/another.html")));
        let config = Config {
            seconds: 12,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_bps: 4_000_000,
        };
        assert!(config.encoder().is_ok());
        assert!(Config {
            seconds: 31,
            ..config
        }
        .encoder()
        .is_err());
        assert!(Config {
            bitrate_bps: 1,
            ..config
        }
        .encoder()
        .is_err());
    }

    #[tokio::test]
    async fn cancellation_preempts_even_ready_work_and_timed_out_work_is_reported() {
        let (please_stop, mut stopped) = watch::channel(false);
        please_stop.send_replace(true);
        assert_eq!(
            step(&mut stopped, TRANSPORT_LIMIT, async { Ok(1) }, "TIMEOUT").await,
            Err(STOPPED)
        );
        let (_keep_open, mut waiting) = watch::channel(false);
        assert_eq!(
            step(
                &mut waiting,
                Duration::from_millis(1),
                std::future::pending::<Outcome<()>>(),
                "TIMEOUT"
            )
            .await,
            Err("TIMEOUT")
        );
    }
}
