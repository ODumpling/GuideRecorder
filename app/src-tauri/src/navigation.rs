//! Keeps the webview on the app's own pages.
//!
//! Guide text contains links. The editor never follows them, but if
//! anything did (a read-only editor, a dropped link, a stray `location`
//! change) the whole app window would turn into that web page. Every
//! top-level navigation outside the app's origin is refused.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::{Manager, Runtime, Url};

/// True for the bundled frontend (`tauri://localhost` on macOS and Linux,
/// `http(s)://tauri.localhost` on Windows) and, in development, the dev
/// server's origin.
pub fn is_app_url(url: &Url, dev_url: Option<&Url>) -> bool {
    let bundled = match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => url.host_str() == Some("tauri.localhost"),
        _ => false,
    };
    bundled || dev_url.is_some_and(|dev| dev.origin() == url.origin())
}

/// A plugin that refuses navigations away from the app.
pub fn guard<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("navigation-guard")
        .on_navigation(|webview, url| {
            let config = webview.app_handle().config();
            let dev_url = if tauri::is_dev() {
                config.build.dev_url.as_ref()
            } else {
                None
            };
            let allowed = is_app_url(url, dev_url);
            if !allowed {
                eprintln!("guiderecorder: blocked navigation to {url}");
            }
            allowed
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn allows_the_bundled_frontend() {
        for allowed in [
            "tauri://localhost",
            "tauri://localhost/index.html#step",
            "http://tauri.localhost/",
            "https://tauri.localhost/assets/x.js",
        ] {
            assert!(is_app_url(&url(allowed), None), "{allowed}");
        }
    }

    #[test]
    fn allows_the_dev_server_only_when_given() {
        let dev = url("http://localhost:1430");
        assert!(is_app_url(&url("http://localhost:1430/"), Some(&dev)));
        assert!(!is_app_url(&url("http://localhost:1430/"), None));
        assert!(!is_app_url(&url("http://localhost:1431/"), Some(&dev)));
        assert!(!is_app_url(&url("https://localhost:1430/"), Some(&dev)));
    }

    #[test]
    fn refuses_everything_else() {
        let dev = url("http://localhost:1430");
        for blocked in [
            "https://example.com/",
            "http://localhost/",
            "tauri://evil.example",
            "https://tauri.localhost.evil.example/",
            "mailto:help@example.com",
            "file:///etc/passwd",
            "about:blank",
            "javascript:alert(1)",
        ] {
            assert!(!is_app_url(&url(blocked), Some(&dev)), "{blocked}");
        }
    }
}
