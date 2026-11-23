#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use tauri::{webview::NewWindowResponse, WebviewUrl, WebviewWindowBuilder};

    // No product commands, native plugins, app state, or remote destinations.
    tauri::Builder::default()
        .setup(|app| {
            WebviewWindowBuilder::new(app, "display-check", WebviewUrl::App("index.html".into()))
                .title("RemoteHub · WebView2 合成画面测试")
                .inner_size(760.0, 600.0)
                .min_inner_size(440.0, 520.0)
                .disable_drag_drop_handler()
                .use_https_scheme(true)
                .devtools(false)
                .on_new_window(|_, _| NewWindowResponse::Deny)
                .on_navigation(|url| {
                    url.scheme() == "https"
                        && url.host_str() == Some("tauri.localhost")
                        && url.port().is_none()
                        && matches!(url.path(), "/" | "/index.html")
                        && url.query().is_none()
                        && url.fragment().is_none()
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!(
            "tauri.synthetic-display.conf.json"
        ))
        .expect("failed to open the synthetic display check");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This synthetic display diagnostic requires Windows and WebView2.");
}
