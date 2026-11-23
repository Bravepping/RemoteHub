#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    use tauri::{webview::NewWindowResponse, WebviewUrl, WebviewWindowBuilder};

    // Tauri resolves config by the directory, not by an arbitrary file name.
    // This standard-named config is deliberately outside the product config directory.
    let context = tauri::generate_context!("display-check/tauri.conf.json");
    assert_eq!(context.config().identifier, "local.remotehub.display-check");
    assert!(context.config().app.windows.is_empty());
    assert!(context.config().build.dev_url.is_none());
    for asset in ["index.html", "display.js", "display.css", "pattern.mp4"] {
        assert!(
            context.assets().get(&asset.into()).is_some(),
            "DISPLAY_PACKAGE_INVALID: missing {asset}"
        );
    }

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
        .run(context)
        .expect("failed to open the synthetic display check");
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This synthetic display diagnostic requires Windows and WebView2.");
}
