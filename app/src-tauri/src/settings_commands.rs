//! Tauri commands for the Settings window: stored settings, the library
//! folder, launch at login, the API key, permissions and the MCP setup
//! command. Disk, Keychain and dialog work runs on the blocking pool.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use guiderecorder_capture::permissions::{self, Permission, PermissionStatus};
use guiderecorder_core::ai::keychain::{self, ApiKey};
use guiderecorder_core::index::Index;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State, WebviewWindow};
use tauri_plugin_autostart::ManagerExt as _;
use tauri_plugin_dialog::DialogExt;

use crate::blocking::blocking;
use crate::editor::{self, AppError, ErrorKind, Result, Session, SessionCell};
use crate::export::ChosenPaths;
use crate::hotkeys::{self, HotkeyErrors, HotkeyState};
use crate::settings::{self, Settings, SettingsStore, SettingsView};

/// The library folder the app opened (or tried to open) last.
#[derive(Default)]
pub struct LibraryTarget(Mutex<Option<PathBuf>>);

impl LibraryTarget {
    pub fn set(&self, path: Option<PathBuf>) {
        *self.0.lock().unwrap() = path;
    }

    pub fn get(&self) -> Option<PathBuf> {
        self.0.lock().unwrap().clone()
    }
}

/// The library folder at startup: `$GUIDERECORDER_LIBRARY`, else the one in
/// settings, else `<documents>/GuideRecorder`.
pub fn startup_library(
    env: Option<std::ffi::OsString>,
    configured: Option<PathBuf>,
    documents: Option<PathBuf>,
) -> Option<PathBuf> {
    let env = env.filter(|v| !v.is_empty());
    if env.is_some() {
        return editor::library_root(env, documents);
    }
    configured.or_else(|| editor::library_root(None, documents))
}

/// Opens the library at startup. A folder chosen in Settings that is gone
/// (an unplugged disk, a moved folder) is reported as missing rather than
/// created again, so first-run setup can ask for it. The default folder and
/// `$GUIDERECORDER_LIBRARY` are created as needed.
pub fn open_startup_library(
    root: Option<&Path>,
    configured: bool,
    index: Option<PathBuf>,
) -> Result<Session> {
    let root = root.ok_or_else(|| {
        AppError::new(
            ErrorKind::Io,
            format!("no library folder: set {}", editor::LIBRARY_ENV),
        )
    })?;
    if configured && !root.is_dir() {
        return Err(AppError::new(
            ErrorKind::NotFound,
            format!(
                "the library folder {} is missing (is its disk connected?)",
                root.display()
            ),
        ));
    }
    Session::start(root, index)
}

#[tauri::command]
pub fn get_settings(store: State<'_, Arc<SettingsStore>>) -> SettingsView {
    store.view()
}

/// Saves the settings the UI edited (validated; paths only as chosen in a
/// dialog) and returns them as stored. Changed record or pause shortcuts
/// are registered at once (see [`hotkey_errors`]).
#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, Arc<SettingsStore>>,
    chosen: State<'_, Arc<ChosenPaths>>,
    settings: Settings,
) -> Result<Settings> {
    let (store, chosen) = (Arc::clone(&store), Arc::clone(&chosen));
    blocking(move || {
        let before = store.get().recording.hotkeys;
        let saved = store.update(settings, &chosen)?;
        if saved.recording.hotkeys != before {
            hotkeys::apply(&app, &saved.recording.hotkeys);
        }
        Ok(saved)
    })
    .await
}

/// Why the record or pause shortcut could not be registered (e.g. taken
/// by another app or macOS), as of the last change.
#[tauri::command]
pub fn hotkey_errors(state: State<'_, HotkeyState>) -> HotkeyErrors {
    state.errors()
}

/// The library as Settings shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryInfo {
    /// The folder in use, or the one that failed to open.
    pub path: Option<PathBuf>,
    /// Why it could not be opened; `None` once open (or still starting).
    pub error: Option<String>,
    pub starting: bool,
    /// `$GUIDERECORDER_LIBRARY` decides the folder at startup.
    pub from_env: bool,
    /// The folder used when none is set.
    pub default_path: Option<PathBuf>,
}

fn default_library<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    editor::library_root(None, app.path().document_dir().ok())
}

#[tauri::command]
pub fn library_info<R: Runtime>(
    app: AppHandle<R>,
    cell: State<'_, Arc<SessionCell>>,
    target: State<'_, Arc<LibraryTarget>>,
) -> LibraryInfo {
    let (starting, error) = match cell.current() {
        None => (true, None),
        Some(Ok(_)) => (false, None),
        Some(Err(e)) => (false, Some(e.message)),
    };
    LibraryInfo {
        path: target.get(),
        error,
        starting,
        from_env: std::env::var_os(editor::LIBRARY_ENV).is_some_and(|v| !v.is_empty()),
        default_path: default_library(&app),
    }
}

/// Asks for a library folder; `None` if cancelled. Only folders chosen here
/// can be switched to.
#[tauri::command]
pub async fn choose_library_folder<R: Runtime>(
    window: WebviewWindow<R>,
    chosen: State<'_, Arc<ChosenPaths>>,
) -> Result<Option<PathBuf>> {
    let chosen = Arc::clone(&chosen);
    blocking(move || {
        let Some(path) = window
            .dialog()
            .file()
            .set_parent(&window)
            .set_title("Choose the library folder")
            .set_can_create_directories(true)
            .blocking_pick_folder()
        else {
            return Ok(None);
        };
        let path = path
            .into_path()
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("not a folder path: {e}")))?;
        chosen.add_library(path.clone());
        Ok(Some(path))
    })
    .await
}

/// Opens the library at `path` (a folder from [`choose_library_folder`];
/// `None`: the default folder) in place of the current one, and stores the
/// choice. Refused while a guide is open or the library is still starting.
/// On failure the current library stays.
#[tauri::command]
pub async fn switch_library<R: Runtime>(
    app: AppHandle<R>,
    cell: State<'_, Arc<SessionCell>>,
    store: State<'_, Arc<SettingsStore>>,
    chosen: State<'_, Arc<ChosenPaths>>,
    target: State<'_, Arc<LibraryTarget>>,
    path: Option<PathBuf>,
) -> Result<PathBuf> {
    let root = match &path {
        Some(path) => {
            if !chosen.is_library(path) {
                return Err(AppError::new(
                    ErrorKind::Invalid,
                    format!("{} was not chosen as the library", path.display()),
                ));
            }
            path.clone()
        }
        None => default_library(&app)
            .ok_or_else(|| AppError::new(ErrorKind::Io, "no Documents folder"))?,
    };
    let (cell, store, target) = (Arc::clone(&cell), Arc::clone(&store), Arc::clone(&target));
    blocking(move || {
        switch_to(&cell, &store, &target, root, path, |root| {
            Session::start(root, Index::default_path())
        })
    })
    .await
}

/// [`switch_library`]'s work, with the session start passed in. The choice
/// is stored first and the session swapped only once the new library has
/// opened; if it fails to open, the stored choice goes back, so the stored
/// and the live library never disagree.
pub fn switch_to(
    cell: &SessionCell,
    store: &SettingsStore,
    target: &LibraryTarget,
    root: PathBuf,
    path: Option<PathBuf>,
    start: impl FnOnce(&Path) -> Result<Session>,
) -> Result<PathBuf> {
    match cell.current() {
        None => {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "the library is still opening; try again in a moment",
            ))
        }
        Some(Ok(session)) if session.has_open_guides() => {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "close the open guide before switching libraries",
            ))
        }
        Some(_) => {}
    }
    let previous = store.get().general.library;
    store.modify(|s| s.general.library = path)?;
    match start(&root) {
        Ok(session) => {
            cell.set(Ok(session));
            target.set(Some(root.clone()));
            Ok(root)
        }
        Err(e) => {
            if let Err(revert) = store.modify(|s| s.general.library = previous) {
                eprintln!(
                    "guiderecorder: settings: cannot restore the library choice: {}",
                    revert.message
                );
            }
            Err(e)
        }
    }
}

/// Turns the login item on or off and returns its actual state.
#[tauri::command]
pub async fn set_launch_at_login<R: Runtime>(
    app: AppHandle<R>,
    store: State<'_, Arc<SettingsStore>>,
    enabled: bool,
) -> Result<bool> {
    let store = Arc::clone(&store);
    blocking(move || {
        apply_launch_at_login(&store, enabled, |enabled| {
            let launcher = app.autolaunch();
            let login = |e| AppError::new(ErrorKind::Io, format!("login item: {e}"));
            if enabled {
                launcher.enable().map_err(login)?;
            } else {
                launcher.disable().map_err(login)?;
            }
            launcher.is_enabled().map_err(login)
        })
    })
    .await
}

/// [`set_launch_at_login`]'s work, with the login item passed in as
/// `toggle` (which returns the item's actual state). The setting is stored
/// first; the stored value then follows what the item really did, and goes
/// back if changing the item failed.
pub fn apply_launch_at_login(
    store: &SettingsStore,
    enabled: bool,
    toggle: impl FnOnce(bool) -> Result<bool>,
) -> Result<bool> {
    let previous = store.get().general.launch_at_login;
    store.modify(|s| s.general.launch_at_login = enabled)?;
    match toggle(enabled) {
        Ok(actual) => {
            if actual != enabled {
                store.modify(|s| s.general.launch_at_login = actual)?;
            }
            Ok(actual)
        }
        Err(e) => {
            if let Err(revert) = store.modify(|s| s.general.launch_at_login = previous) {
                eprintln!(
                    "guiderecorder: settings: cannot restore launch at login: {}",
                    revert.message
                );
            }
            Err(e)
        }
    }
}

/// Whether the login item is on.
#[tauri::command]
pub async fn launch_at_login<R: Runtime>(app: AppHandle<R>) -> Result<bool> {
    blocking(move || {
        app.autolaunch()
            .is_enabled()
            .map_err(|e| AppError::new(ErrorKind::Io, format!("login item: {e}")))
    })
    .await
}

fn key_error(e: guiderecorder_core::ai::AiError) -> AppError {
    AppError::new(ErrorKind::Io, e.to_string())
}

/// Whether an API key is stored. The key itself never leaves Rust.
#[tauri::command]
pub async fn api_key_status() -> Result<bool> {
    blocking(|| Ok(keychain::get_api_key().map_err(key_error)?.is_some())).await
}

/// Stores the API key in the Keychain, replacing any earlier one.
#[tauri::command]
pub async fn set_api_key(key: String) -> Result<()> {
    blocking(move || {
        let key = ApiKey::new(key).map_err(|e| AppError::new(ErrorKind::Invalid, e.to_string()))?;
        keychain::set_api_key(&key).map_err(key_error)
    })
    .await
}

#[tauri::command]
pub async fn delete_api_key() -> Result<()> {
    blocking(|| keychain::delete_api_key().map_err(key_error)).await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRow {
    pub permission: Permission,
    pub status: PermissionStatus,
    /// Allowed in System Settings since launch, but macOS applies it only
    /// after a restart (Screen Recording).
    pub restart_needed: bool,
}

/// The status of every permission recording needs. Never prompts; cheap
/// enough to poll every second.
#[tauri::command]
pub async fn permission_status() -> Result<Vec<PermissionRow>> {
    blocking(|| {
        let report = permissions::check_all();
        let restart = permissions::screen_recording_needs_restart();
        Ok(Permission::ALL
            .into_iter()
            .map(|permission| PermissionRow {
                permission,
                status: report.get(permission),
                restart_needed: restart && permission == Permission::ScreenRecording,
            })
            .collect())
    })
    .await
}

/// Quits through the quit flow (the editor saves first) and starts the app
/// again, e.g. for a Screen Recording grant macOS applies only at launch.
#[tauri::command]
pub fn restart_app<R: Runtime>(app: AppHandle<R>) {
    app.state::<Arc<crate::commands::QuitGate>>().restart(&app);
}

/// Asks macOS for `permission` (may show the system prompt) and returns
/// its status. Only from an explicit click in setup or Settings; a grant
/// made in System Settings may need a restart (Screen Recording).
#[tauri::command]
pub async fn request_permission(permission: Permission) -> Result<PermissionStatus> {
    blocking(move || Ok(permissions::request(permission))).await
}

/// Whether recording may start, and what is missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingReadiness {
    pub ready: bool,
    pub missing: Vec<Permission>,
}

impl RecordingReadiness {
    pub fn from_report(report: &permissions::PermissionReport) -> Self {
        let missing = report.missing();
        Self {
            ready: missing.is_empty(),
            missing,
        }
    }
}

/// The gate before every recording: checks the permissions live (never
/// prompts). Editing and exporting never go through it. `start_recording`
/// (#341) must call this first.
pub fn ensure_can_record() -> Result<()> {
    let readiness = RecordingReadiness::from_report(&permissions::check_all());
    if readiness.ready {
        return Ok(());
    }
    let names: Vec<&str> = readiness
        .missing
        .iter()
        .map(|p| match p {
            Permission::ScreenRecording => "Screen Recording",
            Permission::Accessibility => "Accessibility",
            Permission::InputMonitoring => "Input Monitoring",
        })
        .collect();
    Err(AppError::new(
        ErrorKind::Invalid,
        format!("recording needs {} permission", names.join(", ")),
    ))
}

#[tauri::command]
pub async fn recording_readiness() -> Result<RecordingReadiness> {
    blocking(|| Ok(RecordingReadiness::from_report(&permissions::check_all()))).await
}

/// Opens the System Settings pane for `permission` (a fixed deep link).
#[tauri::command]
pub async fn open_permission_settings(permission: Permission) -> Result<()> {
    blocking(move || {
        tauri_plugin_opener::open_url(permissions::settings_url(permission), None::<&str>)
            .map_err(|e| AppError::new(ErrorKind::Io, format!("cannot open System Settings: {e}")))
    })
    .await
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSetup {
    pub command: String,
    pub mcp_path: PathBuf,
    /// Whether `guiderecorder-mcp` is next to the app's executable.
    pub found: bool,
    pub library: Option<PathBuf>,
}

/// The `guiderecorder-mcp` binary next to `exe`.
pub fn mcp_binary(exe: &Path) -> PathBuf {
    exe.parent()
        .unwrap_or_else(|| Path::new("/"))
        .join("guiderecorder-mcp")
}

/// The command that registers the MCP server with Claude Code for the
/// current library.
#[tauri::command]
pub fn mcp_setup(target: State<'_, Arc<LibraryTarget>>) -> Result<McpSetup> {
    let exe = std::env::current_exe()
        .map_err(|e| AppError::new(ErrorKind::Io, format!("cannot find the app: {e}")))?;
    let mcp_path = mcp_binary(&exe);
    let library = target.get();
    let command = settings::mcp_add_command(
        &mcp_path,
        library
            .as_deref()
            .unwrap_or(Path::new("~/Documents/GuideRecorder")),
    );
    Ok(McpSetup {
        command,
        found: mcp_path.is_file(),
        mcp_path,
        library,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_library_order() {
        let docs = Some(PathBuf::from("/Users/a/Documents"));
        let configured = Some(PathBuf::from("/Volumes/Team/Guides"));
        assert_eq!(
            startup_library(Some("/env".into()), configured.clone(), docs.clone()),
            Some(PathBuf::from("/env"))
        );
        assert_eq!(
            startup_library(Some("".into()), configured.clone(), docs.clone()),
            configured
        );
        assert_eq!(
            startup_library(None, None, docs),
            Some(PathBuf::from("/Users/a/Documents/GuideRecorder"))
        );
    }

    use crate::editor::tests::library;

    fn store() -> (SettingsStore, PathBuf) {
        let dir = library().join("app");
        let path = dir.join("settings.json");
        (SettingsStore::load(Some(path.clone())), path)
    }

    #[test]
    fn library_switch_stores_only_what_opened() {
        let root = library();
        let (store, path) = store();
        let cell = SessionCell::default();
        cell.set(Session::start(&root, None));
        let live = |cell: &SessionCell| cell.current().unwrap().unwrap().root().to_path_buf();
        let before = live(&cell);
        let target = LibraryTarget::default();
        target.set(Some(root.clone()));

        // The new library fails to open: nothing changes, stored or live.
        let other = root.join("other");
        let err = switch_to(
            &cell,
            &store,
            &target,
            other.clone(),
            Some(other.clone()),
            |_| Err(AppError::new(ErrorKind::Io, "disk on fire")),
        )
        .unwrap_err();
        assert_eq!(err.message, "disk on fire");
        assert_eq!(store.get().general.library, None);
        assert_eq!(
            SettingsStore::load(Some(path.clone()))
                .get()
                .general
                .library,
            None
        );
        assert_eq!(target.get(), Some(root.clone()));
        assert_eq!(live(&cell), before);

        // It opens: stored and live together.
        let opened = switch_to(
            &cell,
            &store,
            &target,
            other.clone(),
            Some(other.clone()),
            |r| Session::start(r, None),
        )
        .unwrap();
        assert_eq!(opened, other);
        assert_eq!(store.get().general.library, Some(other.clone()));
        assert_eq!(
            SettingsStore::load(Some(path)).get().general.library,
            Some(other.clone())
        );
        assert_eq!(target.get(), Some(other.clone()));
        assert_ne!(live(&cell), before);
        assert!(other.is_dir(), "the new library was created");
    }

    #[test]
    fn library_switch_waits_for_open_guides_and_startup() {
        let root = library();
        let (store, _) = store();
        let target = LibraryTarget::default();
        let starting = SessionCell::default();
        let never = |_: &Path| -> Result<Session> { panic!("must not start") };
        assert!(switch_to(&starting, &store, &target, root.clone(), None, never).is_err());

        let cell = SessionCell::default();
        cell.set(Session::start(&root, None));
        let session = cell.current().unwrap().unwrap();
        let id = session.list_guides(None).unwrap()[0].id.clone();
        session.open_guide(&id).unwrap();
        let err = switch_to(&cell, &store, &target, root.clone(), None, never).unwrap_err();
        assert!(err.message.contains("close the open guide"));
        assert_eq!(store.get().general.library, None);
    }

    #[test]
    fn launch_at_login_stores_what_the_login_item_did() {
        let (store, path) = store();
        assert!(apply_launch_at_login(&store, true, Ok).unwrap());
        assert!(store.get().general.launch_at_login);

        // The item refused: back to the stored value before.
        let err = apply_launch_at_login(&store, false, |_| {
            Err(AppError::new(ErrorKind::Io, "login item: denied"))
        })
        .unwrap_err();
        assert!(err.message.contains("denied"));
        assert!(store.get().general.launch_at_login);
        assert!(
            SettingsStore::load(Some(path.clone()))
                .get()
                .general
                .launch_at_login
        );

        // The item ended up in another state than asked: store that.
        assert!(apply_launch_at_login(&store, false, |_| Ok(true)).unwrap());
        assert!(
            SettingsStore::load(Some(path))
                .get()
                .general
                .launch_at_login
        );
    }

    #[test]
    fn a_configured_library_that_is_gone_is_missing_not_recreated() {
        let root = library();
        let gone = root.join("Unplugged/Guides");
        let err = open_startup_library(Some(&gone), true, None).err().unwrap();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(err.message.contains("missing"));
        assert!(!gone.exists(), "not created");
        // The default folder (or $GUIDERECORDER_LIBRARY) is created as needed.
        open_startup_library(Some(&gone), false, None).unwrap();
        assert!(gone.is_dir());
        // A configured folder that exists opens.
        open_startup_library(Some(&root), true, None).unwrap();
        let err = open_startup_library(None, false, None).err().unwrap();
        assert!(err.message.contains("no library folder"));
    }

    #[test]
    fn recording_needs_every_permission() {
        use permissions::PermissionReport;
        let all = PermissionReport {
            screen_recording: PermissionStatus::Granted,
            accessibility: PermissionStatus::Granted,
            input_monitoring: PermissionStatus::Granted,
        };
        assert_eq!(
            RecordingReadiness::from_report(&all),
            RecordingReadiness {
                ready: true,
                missing: vec![]
            }
        );
        let some = PermissionReport {
            accessibility: PermissionStatus::Denied,
            input_monitoring: PermissionStatus::NotDetermined,
            ..all
        };
        let readiness = RecordingReadiness::from_report(&some);
        assert!(!readiness.ready);
        assert_eq!(
            readiness.missing,
            [Permission::Accessibility, Permission::InputMonitoring]
        );
    }

    #[test]
    fn mcp_binary_sits_next_to_the_app() {
        assert_eq!(
            mcp_binary(Path::new(
                "/Applications/GuideRecorder.app/Contents/MacOS/GuideRecorder"
            )),
            Path::new("/Applications/GuideRecorder.app/Contents/MacOS/guiderecorder-mcp")
        );
    }
}
