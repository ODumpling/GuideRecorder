//! Web pages in recordings: which apps are browsers, which of their
//! windows are private, and what a browser's address reads as.
//!
//! The capture backends read the frontmost browser window's address (AX on
//! macOS, UI Automation on Windows) and report it as
//! [`CaptureEvent::UrlChanged`](crate::events::CaptureEvent::UrlChanged);
//! the rules here are the pure part they share, so both backends agree on
//! them.
//!
//! - [`browser`]: the browsers whose address is read, by bundle id (macOS)
//!   or executable name (Windows). Any other app has no web address.
//! - [`is_private_window`]: an incognito or private window, recognised by
//!   the words browsers show in its title or toolbar. Nothing is read from
//!   one.
//! - [`page_url`]: the address as a URL: http and https only, without user
//!   name or password. An address bar that hides the scheme
//!   (`example.com/settings`) reads as https.

/// A browser whose web address the recorder reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    Safari,
    Chrome,
    Edge,
    Firefox,
    Arc,
    Brave,
}

impl Browser {
    /// Built on Chromium: Chrome, Edge, Arc and Brave.
    pub fn is_chromium(self) -> bool {
        matches!(
            self,
            Browser::Chrome | Browser::Edge | Browser::Arc | Browser::Brave
        )
    }
}

/// Bundle ids (macOS) and executable names (Windows) of the browsers read,
/// lowercase. Beta and nightly channels count as their browser.
const BROWSERS: &[(&str, Browser)] = &[
    ("com.apple.safari", Browser::Safari),
    ("com.apple.safaritechnologypreview", Browser::Safari),
    ("com.google.chrome", Browser::Chrome),
    ("com.google.chrome.beta", Browser::Chrome),
    ("com.google.chrome.dev", Browser::Chrome),
    ("com.google.chrome.canary", Browser::Chrome),
    ("chrome.exe", Browser::Chrome),
    ("com.microsoft.edgemac", Browser::Edge),
    ("com.microsoft.edgemac.beta", Browser::Edge),
    ("com.microsoft.edgemac.dev", Browser::Edge),
    ("com.microsoft.edgemac.canary", Browser::Edge),
    ("msedge.exe", Browser::Edge),
    ("org.mozilla.firefox", Browser::Firefox),
    ("org.mozilla.firefoxdeveloperedition", Browser::Firefox),
    ("org.mozilla.nightly", Browser::Firefox),
    ("firefox.exe", Browser::Firefox),
    ("company.thebrowser.browser", Browser::Arc),
    ("arc.exe", Browser::Arc),
    ("com.brave.browser", Browser::Brave),
    ("com.brave.browser.beta", Browser::Brave),
    ("com.brave.browser.nightly", Browser::Brave),
    ("brave.exe", Browser::Brave),
];

/// The browser an app is, by its bundle id or (on Windows) executable
/// name, compared without case.
pub fn browser(bundle_id: &str) -> Option<Browser> {
    let id = bundle_id.trim().to_ascii_lowercase();
    BROWSERS
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, browser)| *browser)
}

/// What browsers show in a private window's title or toolbar (lowercase):
/// Chrome's and Brave's "Incognito", Edge's "InPrivate", Firefox's
/// "Private Browsing", Safari's and Arc's "Private" window or tab.
const PRIVATE_MARKERS: &[&str] = &[
    "incognito",
    "inprivate",
    "private browsing",
    "private window",
    "private tab",
];

/// True if `text` (a window title, or a toolbar element's label) marks
/// the window as private. This errs towards private: a page whose title
/// mentions private browsing hides its address too.
pub fn is_private_window(text: &str) -> bool {
    let text = text.to_lowercase();
    PRIVATE_MARKERS.iter().any(|marker| text.contains(marker))
}

/// Longest address kept, in bytes. Longer ones (data stuffed into the
/// query) are dropped rather than cut.
pub const MAX_URL_LEN: usize = 2048;

/// The address a browser shows (`raw`), as an http or https URL; `None`
/// for anything else (a search typed into the address bar, `about:`,
/// `file:` and browser-internal pages). A scheme-less address
/// (`example.com/a`, as Chrome and Safari show them) gets `https://`. Any
/// user name and password are removed.
pub fn page_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > MAX_URL_LEN
        || raw.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let (scheme, rest) = if lower.starts_with("https://") {
        ("https://", &raw["https://".len()..])
    } else if lower.starts_with("http://") {
        ("http://", &raw["http://".len()..])
    } else if raw.contains("://") || has_scheme(raw) {
        return None;
    } else {
        ("https://", raw)
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    // user:password@host
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let name = host.split(':').next().unwrap_or(host);
    let looks_like_host = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '[' | ']'));
    // Without a scheme it must look like a domain (`example.com`) or
    // `localhost`, not a word typed as a search.
    let domain = scheme == "https://" && !lower.starts_with("https://");
    if !looks_like_host || (domain && !name.contains('.') && name != "localhost") {
        return None;
    }
    Some(format!("{scheme}{host}{path}"))
}

/// `mailto:`, `about:blank`, `chrome:newtab` and the like: letters before
/// a colon that is not a port (`localhost:3000`).
fn has_scheme(raw: &str) -> bool {
    let Some((scheme, rest)) = raw.split_once(':') else {
        return false;
    };
    !scheme.is_empty()
        && scheme.chars().all(|c| c.is_ascii_alphabetic())
        && !rest.starts_with(|c: char| c.is_ascii_digit())
}

/// Longest address shown in a title, in characters, before it is cut
/// with "…".
pub const MAX_SHOWN_URL_CHARS: usize = 60;

/// `url` as a reader would say it: without `https://`, `http://` and
/// `www.`, without a lone trailing `/`, cut at [`MAX_SHOWN_URL_CHARS`].
pub fn display_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    let mut shown = url;
    for prefix in ["https://", "http://"] {
        if lower.starts_with(prefix) {
            shown = &url[prefix.len()..];
            break;
        }
    }
    if shown.to_ascii_lowercase().starts_with("www.") {
        shown = &shown[4..];
    }
    // Only the root path: `example.com/` reads as `example.com`.
    if shown.ends_with('/') && shown.matches('/').count() == 1 {
        shown = &shown[..shown.len() - 1];
    }
    if shown.chars().count() > MAX_SHOWN_URL_CHARS {
        let cut: String = shown.chars().take(MAX_SHOWN_URL_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        shown.to_owned()
    }
}

/// True if `text`, typed into a browser, reads as an address rather than
/// a search: [`page_url`] takes it.
pub fn looks_like_address(text: &str) -> bool {
    page_url(text).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsers_by_bundle_id_or_executable() {
        assert_eq!(browser("com.apple.Safari"), Some(Browser::Safari));
        assert_eq!(browser("com.google.Chrome"), Some(Browser::Chrome));
        assert_eq!(browser("company.thebrowser.Browser"), Some(Browser::Arc));
        assert_eq!(browser("msedge.exe"), Some(Browser::Edge));
        assert_eq!(browser("Firefox.EXE"), Some(Browser::Firefox));
        assert_eq!(browser("com.brave.Browser"), Some(Browser::Brave));
        assert_eq!(browser("com.apple.finder"), None);
        assert_eq!(browser("notepad.exe"), None);
        assert!(Browser::Edge.is_chromium());
        assert!(!Browser::Firefox.is_chromium() && !Browser::Safari.is_chromium());
    }

    #[test]
    fn private_windows_by_their_markers() {
        for title in [
            "New Tab - Google Chrome (Incognito)",
            "Bing - [InPrivate] - Microsoft Edge",
            "Mozilla Firefox Private Browsing",
            "Private Window",
            "INCOGNITO",
        ] {
            assert!(is_private_window(title), "{title}");
        }
        for title in ["Inbox - Gmail", "Private messages", ""] {
            assert!(!is_private_window(title), "{title}");
        }
    }

    #[test]
    fn page_urls() {
        assert_eq!(
            page_url("https://example.com/settings?tab=2#top").as_deref(),
            Some("https://example.com/settings?tab=2#top")
        );
        assert_eq!(
            page_url("HTTP://Example.com").as_deref(),
            Some("http://Example.com")
        );
        // Chrome and Safari hide the scheme.
        assert_eq!(
            page_url("example.com/settings").as_deref(),
            Some("https://example.com/settings")
        );
        assert_eq!(
            page_url("localhost:3000/a").as_deref(),
            Some("https://localhost:3000/a")
        );
        assert_eq!(page_url("  docs.rs  ").as_deref(), Some("https://docs.rs"));
        // Credentials never reach a guide.
        assert_eq!(
            page_url("https://bob:hunter2@example.com/x").as_deref(),
            Some("https://example.com/x")
        );
        for raw in [
            "",
            "rust serde",
            "serde",
            "about:blank",
            "chrome://settings",
            "edge://newtab",
            "file:///C:/x.html",
            "javascript:alert(1)",
            "mailto:a@b.c",
            "https://",
            "https://exa mple.com",
        ] {
            assert_eq!(page_url(raw), None, "{raw}");
        }
        assert_eq!(
            page_url(&format!("https://a.b/{}", "x".repeat(MAX_URL_LEN))),
            None
        );
        assert!(looks_like_address("github.com"));
        assert!(!looks_like_address("how to rotate an api key"));
    }

    #[test]
    fn urls_as_shown() {
        assert_eq!(display_url("https://www.example.com/"), "example.com");
        assert_eq!(display_url("https://example.com/a/"), "example.com/a/");
        assert_eq!(display_url("http://localhost:3000"), "localhost:3000");
        let long = format!("https://example.com/{}", "a".repeat(100));
        let shown = display_url(&long);
        assert_eq!(shown.chars().count(), MAX_SHOWN_URL_CHARS);
        assert!(shown.ends_with('…'));
    }
}
