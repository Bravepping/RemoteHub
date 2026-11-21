pub(crate) fn origin(window: &tauri::WebviewWindow) -> Result<(), String> {
    let url = window.url().map_err(|_| "WINDOW_URL_UNAVAILABLE")?;
    origin_url(window.label(), &url)
}
pub(crate) fn origin_url(label: &str, url: &tauri::Url) -> Result<(), String> {
    let app = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "http" && url.host_str() == Some("tauri.localhost"))
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port() == Some(1420));
    if label != "main" || !app {
        return Err("LOCAL_MAIN_WINDOW_REQUIRED".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn experimental_commands_only_accept_local_main_window() {
        let local = tauri::Url::parse("http://tauri.localhost/").unwrap();
        assert!(origin_url("main", &local).is_ok());
        assert!(origin_url("other", &local).is_err());
        for address in [
            "https://example.com/",
            "http://192.168.1.2:1420/",
            "http://127.0.0.1:1421/",
        ] {
            assert!(origin_url("main", &tauri::Url::parse(address).unwrap()).is_err());
        }
    }
}
