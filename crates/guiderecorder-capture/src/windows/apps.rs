//! The apps around a recording on Windows: who a pid is (its executable
//! and the description in its version resource), which app is in the
//! foreground and when that changes (`EVENT_SYSTEM_FOREGROUND`), the
//! suggested exclusion list and where sessions keep their work files.
//!
//! An app's id, which stands in for a macOS bundle id (exclusion lists,
//! [`AppRef::bundle_id`]), is its executable's file name in lower case,
//! e.g. `1password.exe`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use guiderecorder_core::events::{AppRef, Timestamp};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, GetMessageW, PeekMessageW, PostThreadMessageW,
    TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG, PM_NOREMOVE, WINEVENT_OUTOFCONTEXT, WM_QUIT,
    WM_USER,
};

use super::event_tap::{self, window_pid};

#[cfg(doc)]
use crate::session::SessionConfig;

/// The suggested exclusion list (the app's settings default): 1Password,
/// Bitwarden, KeePass, KeePassXC, Enpass, and the Windows credential
/// prompt (`CredentialUIBroker.exe`, which asks for passwords to network
/// shares and remote desktops). [`SessionConfig::default`] excludes
/// nothing; the caller passes the list it wants in
/// [`SessionConfig::excluded_bundle_ids`].
pub const DEFAULT_EXCLUDED_BUNDLE_IDS: &[&str] = &[
    "1password.exe",
    "bitwarden.exe",
    "keepass.exe",
    "keepassxc.exe",
    "enpass.exe",
    "credentialuibroker.exe",
];

/// [`DEFAULT_EXCLUDED_BUNDLE_IDS`] as owned strings, ready for
/// [`SessionConfig::excluded_bundle_ids`].
pub fn default_excluded_bundle_ids() -> Vec<String> {
    DEFAULT_EXCLUDED_BUNDLE_IDS
        .iter()
        .map(|&id| id.to_owned())
        .collect()
}

/// `%LOCALAPPDATA%\dev.dufflebear.guiderecorder\sessions` (local, never
/// roamed), or the same under the system temp folder without it.
pub fn default_work_dir() -> PathBuf {
    match std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
        Some(local) => PathBuf::from(local).join("dev.dufflebear.guiderecorder/sessions"),
        None => std::env::temp_dir().join("dev.dufflebear.guiderecorder/sessions"),
    }
}

/// Name and id of a running app: its executable's description (as Task
/// Manager shows it) or file stem, and its file name in lower case. Only
/// the pid when the process cannot be queried (it exited, or it is
/// protected).
pub fn app_for_pid(pid: i32) -> AppRef {
    let Some(path) = u32::try_from(pid)
        .ok()
        .filter(|&p| p != 0)
        .and_then(process_image)
    else {
        return AppRef {
            pid,
            ..AppRef::default()
        };
    };
    AppRef {
        pid,
        name: file_description(&path).or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        }),
        bundle_id: app_id(&path),
    }
}

/// The id an app's executable at `path` goes by: its file name in lower
/// case.
pub fn app_id(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
}

/// The full path of process `pid`'s executable.
pub(crate) fn process_image(pid: u32) -> Option<PathBuf> {
    // SAFETY: a plain query; the handle is closed below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: a valid buffer of `len` code units; `len` is updated to the
    // length written.
    let read = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: the handle opened above.
    let _ = unsafe { CloseHandle(process) };
    read.ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(
        &buf[..len as usize],
    )))
}

/// The `FileDescription` in the version resource of the executable at
/// `path`, in its first listed language.
fn file_description(path: &Path) -> Option<String> {
    let wide: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain([0])
        .collect();
    let file = PCWSTR(wide.as_ptr());
    // SAFETY: a NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(file, None) };
    if size == 0 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: a buffer of `size` bytes.
    unsafe { GetFileVersionInfoW(file, None, size, data.as_mut_ptr().cast()) }.ok()?;
    let translation = query_value(&data, "\\VarFileInfo\\Translation")?;
    let [lang, code_page] = match translation {
        [a, b, c, d, ..] => [u16::from_le_bytes([*a, *b]), u16::from_le_bytes([*c, *d])],
        _ => return None,
    };
    let key = format!("\\StringFileInfo\\{lang:04x}{code_page:04x}\\FileDescription");
    let value = query_value(&data, &key)?;
    let units: Vec<u16> = value
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&u| u != 0)
        .collect();
    let description = String::from_utf16_lossy(&units).trim().to_owned();
    (!description.is_empty()).then_some(description)
}

/// The bytes of value `key` in version resource `data`.
fn query_value<'a>(data: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let key: Vec<u16> = key.encode_utf16().chain([0]).collect();
    let mut ptr: *mut c_void = std::ptr::null_mut();
    let mut len = 0u32;
    // SAFETY: `data` is a version resource read by GetFileVersionInfoW and
    // `key` is NUL-terminated; on success `ptr` points into `data`.
    let found = unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut ptr,
            &mut len,
        )
    };
    if !found.as_bool() || ptr.is_null() {
        return None;
    }
    let start = (ptr as usize).checked_sub(data.as_ptr() as usize)?;
    // String values count UTF-16 units, binary values bytes; reading up to
    // the end of `data` at most is safe either way.
    let end = data.len().min(start + (len as usize) * 2);
    data.get(start..end)
}

/// The id of the app in the foreground now.
pub fn frontmost_bundle_id() -> Option<String> {
    // SAFETY: no arguments.
    let pid = window_pid(unsafe { GetForegroundWindow() });
    app_for_pid(pid).bundle_id
}

/// Reports every change of foreground window to another app
/// (`EVENT_SYSTEM_FOREGROUND`, out of context, on its own thread) until
/// dropped, with its time and the app's id. Unlike macOS it needs no main
/// run loop. The frame streams have no content filter to refresh, so
/// nothing is sent to the session's refresh thread, which ends at once.
pub struct FrontmostObserver {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// Posted to the observer thread once its message queue exists.
const WM_READY: u32 = WM_USER + 1;

type ActivationFn = Box<dyn Fn(Timestamp, Option<&str>)>;

struct Observer {
    on_activated: ActivationFn,
    /// The foreground pid last reported, so a change of window within the
    /// same app is not an activation.
    last_pid: Option<i32>,
    /// Ids by pid, looked up once.
    ids: HashMap<i32, Option<String>>,
}

thread_local! {
    static OBSERVER: RefCell<Option<Observer>> = const { RefCell::new(None) };
}

impl FrontmostObserver {
    /// `_filtered` and `_refresh` are for the macOS backend's content
    /// filter; Windows has none.
    pub fn start(
        on_activated: impl Fn(Timestamp, Option<&str>) + Send + Sync + 'static,
        _filtered: Vec<String>,
        _refresh: Sender<i32>,
    ) -> Self {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("guiderecorder-foreground".into())
            .spawn(move || run_observer(Box::new(on_activated), ready_tx));
        let thread_id = thread
            .as_ref()
            .ok()
            .and_then(|_| ready_rx.recv().ok())
            .flatten();
        match (thread, thread_id) {
            (Ok(thread), Some(thread_id)) => Self {
                thread_id,
                thread: Some(thread),
            },
            (thread, _) => {
                // No hook: activations go unseen, as without a run loop on
                // macOS; start and resume still read the foreground app.
                if let Ok(thread) = thread {
                    let _ = thread.join();
                }
                Self {
                    thread_id: 0,
                    thread: None,
                }
            }
        }
    }
}

impl Drop for FrontmostObserver {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // SAFETY: posting to a thread with a message queue.
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = thread.join();
        }
    }
}

fn run_observer(on_activated: ActivationFn, ready: mpsc::SyncSender<Option<u32>>) {
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    let mut msg = MSG::default();
    // SAFETY: creates this thread's message queue; a valid out-pointer.
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_READY, WM_READY, PM_NOREMOVE) };
    OBSERVER.with(|o| {
        *o.borrow_mut() = Some(Observer {
            on_activated,
            last_pid: None,
            ids: HashMap::new(),
        })
    });
    // SAFETY: an out-of-context hook (no module), delivered to this
    // thread's message loop.
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.is_invalid() {
        let _ = ready.send(None);
        return;
    }
    let _ = ready.send(Some(thread_id));
    // SAFETY: a valid out-pointer; ends on WM_QUIT.
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: a message this thread just received.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    // SAFETY: the hook installed above.
    let _ = unsafe { UnhookWinEvent(hook) };
    OBSERVER.with(|o| o.borrow_mut().take());
}

unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let ts = event_tap::now();
    let pid = window_pid(window);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        OBSERVER.with(|o| {
            let mut o = o.borrow_mut();
            let Some(o) = o.as_mut() else {
                return;
            };
            if pid == 0 || o.last_pid == Some(pid) {
                return;
            }
            o.last_pid = Some(pid);
            let id = o
                .ids
                .entry(pid)
                .or_insert_with(|| app_for_pid(pid).bundle_id)
                .clone();
            (o.on_activated)(ts, id.as_deref());
        })
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_is_named_by_its_executable() {
        let pid = i32::try_from(std::process::id()).unwrap();
        let app = app_for_pid(pid);
        assert_eq!(app.pid, pid);
        let id = app.bundle_id.expect("an id");
        assert!(id.ends_with(".exe"), "{id}");
        assert_eq!(id, id.to_lowercase());
        assert!(app.name.is_some_and(|n| !n.is_empty()));
        // Explorer has a description in its version resource.
        let explorer = std::env::var_os("WINDIR")
            .map(|w| PathBuf::from(w).join("explorer.exe"))
            .unwrap();
        assert_eq!(
            file_description(&explorer).as_deref(),
            Some("Windows Explorer")
        );
    }

    #[test]
    fn unknown_pids_are_only_a_pid() {
        let app = app_for_pid(0);
        assert_eq!(app, AppRef::default());
        let app = app_for_pid(-5);
        assert_eq!(app.bundle_id, None);
    }

    #[test]
    fn ids_are_lower_case_file_names() {
        assert_eq!(
            app_id(Path::new(r"C:\Program Files\1Password\app\8\1Password.exe")).as_deref(),
            Some("1password.exe")
        );
        assert!(default_excluded_bundle_ids()
            .iter()
            .all(|id| id.ends_with(".exe") && *id == id.to_lowercase()));
    }

    #[test]
    fn work_dir_is_local() {
        assert!(default_work_dir().ends_with("dev.dufflebear.guiderecorder/sessions"));
    }

    #[test]
    fn observer_starts_and_stops() {
        let (tx, _rx) = mpsc::channel();
        let observer = FrontmostObserver::start(|_, _| {}, Vec::new(), tx);
        assert!(observer.thread.is_some());
        drop(observer);
    }
}
