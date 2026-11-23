#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
mod app {
    use serde::Serialize;
    use std::{sync::Mutex, time::Duration};
    use tauri::{
        webview::NewWindowResponse, Manager, State, WebviewUrl, WebviewWindow,
        WebviewWindowBuilder, WindowEvent,
    };
    use tokio::{sync::watch, time::timeout};

    const LABEL: &str = "stream-display-check";
    const ID: &str = "local.remotehub.stream-display-check";
    const STOPPED: &str = "STREAM_NATIVE_STOPPED";

    struct ActiveRun {
        cancel: watch::Sender<bool>,
        done: watch::Receiver<Option<bool>>,
    }

    #[derive(Default)]
    struct RunState(Mutex<Option<ActiveRun>>);

    impl RunState {
        fn begin(&self) -> Result<(RunGuard<'_>, watch::Receiver<bool>), &'static str> {
            let mut active = self.0.lock().map_err(|_| "STREAM_NATIVE_STATE")?;
            if active.is_some() {
                return Err("STREAM_NATIVE_BUSY");
            }
            let (cancel, stopped) = watch::channel(false);
            let (done, completed) = watch::channel(None);
            *active = Some(ActiveRun {
                cancel,
                done: completed,
            });
            Ok((
                RunGuard {
                    state: self,
                    done,
                    cleanup_confirmed: false,
                },
                stopped,
            ))
        }

        fn stop(&self) -> Result<Option<watch::Receiver<Option<bool>>>, &'static str> {
            let active = self.0.lock().map_err(|_| "STREAM_NATIVE_STATE")?;
            Ok(active.as_ref().map(|run| {
                run.cancel.send_replace(true);
                run.done.clone()
            }))
        }
    }

    struct RunGuard<'a> {
        state: &'a RunState,
        done: watch::Sender<Option<bool>>,
        cleanup_confirmed: bool,
    }

    impl Drop for RunGuard<'_> {
        fn drop(&mut self) {
            if let Ok(mut active) = self.state.0.lock() {
                *active = None;
                self.done.send_replace(Some(self.cleanup_confirmed));
            }
        }
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct DisplayFrame {
        sequence: u64,
        timestamp_us: u64,
        keyframe: bool,
        annex_b: Vec<u8>,
    }

    #[derive(Serialize)]
    struct ResultFrames {
        frames: Vec<DisplayFrame>,
    }

    fn is_page(url: &tauri::Url) -> bool {
        url.scheme() == "https"
            && url.host_str() == Some("tauri.localhost")
            && url.port().is_none()
            && matches!(url.path(), "/" | "/index.html")
            && url.query().is_none()
            && url.fragment().is_none()
    }

    fn check_window(window: &WebviewWindow) -> Result<(), &'static str> {
        if window.label() != LABEL || !window.url().is_ok_and(|url| is_page(&url)) {
            return Err("STREAM_NATIVE_ORIGIN");
        }
        Ok(())
    }

    #[tauri::command]
    async fn run_signed_stream(
        window: WebviewWindow,
        state: State<'_, RunState>,
    ) -> Result<ResultFrames, &'static str> {
        check_window(&window)?;
        let (mut run, stopped) = state.begin()?;
        // Relay state is confined to one executor; Tauri commands are Send,
        // but this loopback fixture need not make its socket handles Sync.
        let signal = stopped.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| "STREAM_NATIVE_RUNTIME")?;
            executor.block_on(hub_rendezvous_probe::verified_synthetic_frames(signal))
        })
        .await
        .map_err(|_| "STREAM_NATIVE_RUNTIME")?;
        run.cleanup_confirmed =
            result.is_ok() || result.as_ref().err().copied() == Some("VERIFIED_SYNTHETIC_STOPPED");
        let frames = result?;
        if *stopped.borrow() {
            return Err(STOPPED);
        }
        if frames.is_empty()
            || frames.len() > 8
            || frames.iter().enumerate().any(|(index, frame)| {
                frame.sequence != index as u64
                    || frame.timestamp_us > 10_000_000
                    || frame.annex_b.is_empty()
                    || frame.annex_b.len() > 16_384
            })
        {
            return Err("STREAM_NATIVE_INVALID_FRAMES");
        }
        Ok(ResultFrames {
            frames: frames
                .into_iter()
                .map(|frame| DisplayFrame {
                    sequence: frame.sequence,
                    timestamp_us: frame.timestamp_us,
                    keyframe: frame.keyframe,
                    annex_b: frame.annex_b,
                })
                .collect(),
        })
    }

    #[tauri::command]
    async fn stop_signed_stream(
        window: WebviewWindow,
        state: State<'_, RunState>,
    ) -> Result<(), &'static str> {
        check_window(&window)?;
        if let Some(mut done) = state.stop()? {
            let completed = timeout(Duration::from_secs(3), done.wait_for(Option::is_some))
                .await
                .map_err(|_| "STREAM_NATIVE_STOP_TIMEOUT")?
                .map_err(|_| "STREAM_NATIVE_STATE")?;
            if *completed != Some(true) {
                return Err("STREAM_NATIVE_CLEANUP_UNCONFIRMED");
            }
        }
        Ok(())
    }

    pub(super) fn run() {
        let context = tauri::generate_context!("stream-display-check/tauri.conf.json");
        assert_eq!(context.config().identifier, ID);
        assert!(context.config().app.windows.is_empty());
        assert!(context.config().build.dev_url.is_none());
        assert!(context.assets().get(&"index.html".into()).is_some());

        tauri::Builder::default()
            .manage(RunState::default())
            .invoke_handler(tauri::generate_handler![
                run_signed_stream,
                stop_signed_stream
            ])
            .on_window_event(|window, event| {
                if window.label() == LABEL
                    && matches!(
                        event,
                        WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
                    )
                {
                    let _ = window.state::<RunState>().stop();
                }
            })
            .setup(|app| {
                WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
                    .title("RemoteHub · 已验签串流显示测试")
                    .inner_size(760.0, 620.0)
                    .min_inner_size(460.0, 560.0)
                    .disable_drag_drop_handler()
                    .use_https_scheme(true)
                    .devtools(false)
                    .on_new_window(|_, _| NewWindowResponse::Deny)
                    .on_navigation(is_page)
                    .build()?;
                Ok(())
            })
            .run(context)
            .expect("failed to open the signed loopback stream display check");
    }
}

#[cfg(windows)]
fn main() {
    app::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This signed stream display diagnostic requires Windows and WebView2.");
}
