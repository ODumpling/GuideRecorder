//! The apps around a recording on macOS: who a pid is
//! (`NSRunningApplication`), which app is frontmost and when that changes
//! (`NSWorkspace` notifications), the suggested exclusion list and where
//! sessions keep their work files.

use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use block2::RcBlock;
use guiderecorder_core::events::{AppRef, Timestamp};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2_app_kit::{
    NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
    NSWorkspaceDidActivateApplicationNotification, NSWorkspaceDidLaunchApplicationNotification,
};
use objc2_foundation::{NSNotification, NSObjectProtocol};

use super::event_tap;
use super::frames;

#[cfg(doc)]
use crate::session::SessionConfig;

/// The suggested exclusion list (the app's settings default):
/// 1Password 8, 7 and 6 (6 from the App Store and from AgileBits' own
/// store, whose ids differ; with the 7 and 6 mini helpers, whose team-id
/// prefixed ids no `<id>.` rule reaches), Bitwarden, Keychain Access,
/// Passwords (macOS 15+) and Messages. [`SessionConfig::default`] excludes nothing; the caller
/// passes the list it wants in [`SessionConfig::excluded_bundle_ids`].
pub const DEFAULT_EXCLUDED_BUNDLE_IDS: &[&str] = &[
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.agilebits.onepassword-osx",
    "com.agilebits.onepassword4",
    "2BUA8C4S2C.com.agilebits.onepassword7-helper",
    "2BUA8C4S2C.com.agilebits.onepassword-osx-helper",
    "2BUA8C4S2C.com.agilebits.onepassword4-helper",
    "com.bitwarden.desktop",
    "com.apple.keychainaccess",
    "com.apple.Passwords",
    "com.apple.MobileSMS",
];

/// [`DEFAULT_EXCLUDED_BUNDLE_IDS`] as owned strings, ready for
/// [`SessionConfig::excluded_bundle_ids`].
pub fn default_excluded_bundle_ids() -> Vec<String> {
    DEFAULT_EXCLUDED_BUNDLE_IDS
        .iter()
        .map(|&id| id.to_owned())
        .collect()
}

/// `$HOME/Library/Caches/dev.dufflebear.guiderecorder/sessions`, or the same under the
/// system temp folder without `$HOME`.
pub fn default_work_dir() -> PathBuf {
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(home) => {
            PathBuf::from(home).join("Library/Caches/dev.dufflebear.guiderecorder/sessions")
        }
        None => std::env::temp_dir().join("dev.dufflebear.guiderecorder/sessions"),
    }
}

/// Name and bundle id of a running app.
pub fn app_for_pid(pid: i32) -> AppRef {
    objc2::rc::autoreleasepool(|_| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        AppRef {
            pid,
            name: app
                .as_ref()
                .and_then(|a| a.localizedName())
                .map(|s| s.to_string()),
            bundle_id: app
                .as_ref()
                .and_then(|a| a.bundleIdentifier())
                .map(|s| s.to_string()),
        }
    })
}

/// The bundle id of the frontmost app now.
pub fn frontmost_bundle_id() -> Option<String> {
    objc2::rc::autoreleasepool(|_| {
        NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .and_then(|app| app.bundleIdentifier())
            .map(|id| id.to_string())
    })
}

/// Reports every app activation (`NSWorkspaceDidActivateApplicationNotification`)
/// until dropped, with its time and bundle id, and sends the pid of each
/// filtered app that activates or launches
/// (`NSWorkspaceDidLaunchApplicationNotification`) to the session's filter
/// refresh thread, which ends when this is dropped. The notifications are
/// posted, and so handled, on the main thread: they need the main run loop
/// running, as in the app.
pub struct FrontmostObserver {
    tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

// SAFETY: the tokens are only passed to `removeObserver:`, and
// NSNotificationCenter is thread-safe.
unsafe impl Send for FrontmostObserver {}

/// The app a workspace notification is about: its bundle id and pid.
fn notified_app(note: &NSNotification) -> Option<(Option<String>, i32)> {
    objc2::rc::autoreleasepool(|_| {
        note.userInfo()
            // SAFETY: an NSString key, as the dictionary expects.
            .and_then(|info| unsafe { info.objectForKey(NSWorkspaceApplicationKey) })
            .and_then(|app| app.downcast::<NSRunningApplication>().ok())
            .map(|app| {
                (
                    app.bundleIdentifier().map(|id| id.to_string()),
                    app.processIdentifier(),
                )
            })
    })
}

impl FrontmostObserver {
    /// `filtered`: the bundle ids the frame streams leave out (the
    /// session's plus [`SessionConfig::frames`]'), which a launch or
    /// activation is matched against for a refresh.
    pub fn start(
        on_activated: impl Fn(Timestamp, Option<&str>) + Send + Sync + 'static,
        filtered: Vec<String>,
        refresh: Sender<i32>,
    ) -> Self {
        let filtered = Arc::new(filtered);
        let filtered_launch = Arc::clone(&filtered);
        let refresh_launch = refresh.clone();
        let activated = RcBlock::new(move |note: NonNull<NSNotification>| {
            let ts = event_tap::now();
            // SAFETY: the center passes a valid notification for the call.
            let app = notified_app(unsafe { note.as_ref() });
            let bundle_id = app.as_ref().and_then(|(id, _)| id.as_deref());
            on_activated(ts, bundle_id);
            if let Some((Some(id), pid)) = &app {
                if frames::bundle_id_matches(id, &filtered) {
                    let _ = refresh.send(*pid);
                }
            }
        });
        let launched = RcBlock::new(move |note: NonNull<NSNotification>| {
            // SAFETY: the center passes a valid notification for the call.
            if let Some((Some(id), pid)) = notified_app(unsafe { note.as_ref() }) {
                if frames::bundle_id_matches(&id, &filtered_launch) {
                    let _ = refresh_launch.send(pid);
                }
            }
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // SAFETY: no object filter and no queue (handled on the posting
        // thread); the blocks only touch `Send + Sync` state.
        let tokens = unsafe {
            vec![
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceDidActivateApplicationNotification),
                    None,
                    None,
                    &activated,
                ),
                center.addObserverForName_object_queue_usingBlock(
                    Some(NSWorkspaceDidLaunchApplicationNotification),
                    None,
                    None,
                    &launched,
                ),
            ]
        };
        Self { tokens }
    }
}

impl Drop for FrontmostObserver {
    fn drop(&mut self) {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        for token in &self.tokens {
            let token: &AnyObject = token.as_ref();
            // SAFETY: a token this center returned.
            unsafe { center.removeObserver(token) };
        }
    }
}
