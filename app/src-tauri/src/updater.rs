//! App updates from GitHub Releases (`tauri-plugin-updater`): the endpoint
//! and public key are in `tauri.conf.json`; release builds are signed with
//! the matching private key in CI (`.github/workflows/release.yml`).
//!
//! Release builds check at startup and every few hours while
//! `general.checkForUpdates` is on; the frontend (or the app menu's Check
//! for Updates…) can check at any time. An update is only downloaded and
//! installed when the user asks, and it takes effect after a restart that
//! goes through the `QuitGate` (`QuitGate::restart`), so the editor saves
//! first.
//!
//! On Windows the updater's `install` starts the installer and exits the
//! process on the spot, past the `QuitGate`. So there an update is only
//! downloaded when the user asks, and installed as the app exits
//! ([`install_on_exit`]), once the editor has saved and a recording has
//! been stopped.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::editor::{AppError, ErrorKind, Result};
use crate::settings::SettingsStore;

/// Emitted with an [`UpdateStatus`] on every change.
pub const STATUS_EVENT: &str = "updater://status";

/// The first automatic check waits this long after startup, out of the way
/// of opening the library.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
/// Then one check per this long.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Where the updater is. `manual`: the user asked for this check, so the
/// UI reports its result even when there is nothing to install.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum UpdateStatus {
    Idle,
    #[serde(rename_all = "camelCase")]
    Checking {
        manual: bool,
    },
    #[serde(rename_all = "camelCase")]
    UpToDate {
        manual: bool,
    },
    #[serde(rename_all = "camelCase")]
    Available {
        version: String,
        notes: Option<String>,
    },
    /// `total` is unknown when the server sends no length.
    #[serde(rename_all = "camelCase")]
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    /// Installed (on Windows: downloaded, and installed as the app exits);
    /// restart to use it.
    #[serde(rename_all = "camelCase")]
    Ready {
        version: String,
    },
    #[serde(rename_all = "camelCase")]
    Failed {
        message: String,
        manual: bool,
    },
}

impl UpdateStatus {
    /// A check or download is running, or an update waits for a restart:
    /// nothing new starts.
    fn busy(&self) -> bool {
        matches!(
            self,
            Self::Checking { .. } | Self::Downloading { .. } | Self::Ready { .. }
        )
    }
}

/// The updater's state (managed).
pub struct Updates {
    state: Mutex<(UpdateStatus, Option<Update>)>,
    /// A downloaded update, installed as the app exits.
    #[cfg(windows)]
    pending: Mutex<Option<(Update, Vec<u8>)>>,
}

impl Default for Updates {
    fn default() -> Self {
        Self {
            state: Mutex::new((UpdateStatus::Idle, None)),
            #[cfg(windows)]
            pending: Mutex::new(None),
        }
    }
}

impl Updates {
    pub fn status(&self) -> UpdateStatus {
        self.state.lock().unwrap().0.clone()
    }

    fn set<R: Runtime>(&self, app: &AppHandle<R>, status: UpdateStatus, update: Option<Update>) {
        let mut state = self.state.lock().unwrap();
        *state = (status.clone(), update);
        drop(state);
        if let Err(e) = app.emit(STATUS_EVENT, &status) {
            eprintln!("guiderecorder: cannot send the update status: {e}");
        }
    }

    /// Moves to `Checking`, unless something is already running. Returns
    /// whether the check should go ahead.
    fn begin_check<R: Runtime>(&self, app: &AppHandle<R>, manual: bool) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.0.busy() {
            return false;
        }
        *state = (UpdateStatus::Checking { manual }, None);
        drop(state);
        let _ = app.emit(STATUS_EVENT, UpdateStatus::Checking { manual });
        true
    }

    /// The update found by the last check, moving to `Downloading`.
    fn begin_install<R: Runtime>(&self, app: &AppHandle<R>) -> Result<Update> {
        let mut state = self.state.lock().unwrap();
        let update = match (&state.0, &state.1) {
            // A failed install keeps its update to try again.
            (UpdateStatus::Available { .. } | UpdateStatus::Failed { .. }, Some(update)) => {
                update.clone()
            }
            _ => {
                return Err(AppError::new(
                    ErrorKind::Invalid,
                    "there is no update to install; check for updates first",
                ))
            }
        };
        let status = UpdateStatus::Downloading {
            version: update.version.clone(),
            downloaded: 0,
            total: None,
        };
        state.0 = status.clone();
        drop(state);
        let _ = app.emit(STATUS_EVENT, status);
        Ok(update)
    }
}

/// Checks for an update and stores the result.
async fn check<R: Runtime>(app: &AppHandle<R>, manual: bool) {
    let updates = app.state::<Arc<Updates>>();
    if !updates.begin_check(app, manual) {
        return;
    }
    let result = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(e) => Err(e),
    };
    match result {
        Ok(Some(update)) => {
            let status = UpdateStatus::Available {
                version: update.version.clone(),
                notes: update.body.clone().filter(|n| !n.trim().is_empty()),
            };
            updates.set(app, status, Some(update));
        }
        Ok(None) => updates.set(app, UpdateStatus::UpToDate { manual }, None),
        Err(e) => {
            eprintln!("guiderecorder: update check failed: {e}");
            let status = UpdateStatus::Failed {
                message: format!("Could not check for updates: {e}"),
                manual,
            };
            updates.set(app, status, None);
        }
    }
}

/// Starts a check the user asked for (the app menu's Check for Updates…).
pub fn check_from_menu<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check(&app, true).await });
}

/// Checks after [`FIRST_CHECK_DELAY`], then every [`CHECK_INTERVAL`],
/// while the setting is on. Release builds only: a dev build is not an
/// installed app the updater could replace.
pub fn start_background_checks<R: Runtime>(app: &AppHandle<R>) {
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            if app
                .state::<Arc<SettingsStore>>()
                .get()
                .general
                .check_for_updates
            {
                check(&app, false).await;
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// Progress as `Downloading` statuses: one per percent (or per MB when the
/// size is unknown), not one per chunk.
struct Progress {
    version: String,
    downloaded: u64,
    total: Option<u64>,
    sent: u64,
}

impl Progress {
    /// Adds a chunk; returns the status to send, if it moved enough.
    fn add(&mut self, chunk: usize, total: Option<u64>) -> Option<UpdateStatus> {
        self.downloaded += chunk as u64;
        self.total = total.filter(|t| *t > 0);
        let step = self.total.map_or(1024 * 1024, |t| (t / 100).max(1));
        if self.downloaded / step == self.sent / step {
            return None;
        }
        self.sent = self.downloaded;
        Some(UpdateStatus::Downloading {
            version: self.version.clone(),
            downloaded: self.downloaded,
            total: self.total,
        })
    }
}

#[tauri::command]
pub fn update_status(updates: State<'_, Arc<Updates>>) -> UpdateStatus {
    updates.status()
}

/// Checks now; the result arrives as [`STATUS_EVENT`]s. Returns at once.
#[tauri::command]
pub fn check_for_update<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move { check(&app, true).await });
}

/// Downloads and installs the update the last check found; progress and
/// the result arrive as [`STATUS_EVENT`]s. Rejects with `invalid` if there
/// is none.
#[tauri::command]
pub fn install_update<R: Runtime>(
    app: AppHandle<R>,
    updates: State<'_, Arc<Updates>>,
) -> Result<()> {
    let update = updates.begin_install(&app)?;
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Arc<Updates>>();
        let version = update.version.clone();
        let mut progress = Progress {
            version: version.clone(),
            downloaded: 0,
            total: None,
            sent: 0,
        };
        let downloaded = update
            .download(
                |chunk, total| {
                    if let Some(status) = progress.add(chunk, total) {
                        updates.set(&app, status, None);
                    }
                },
                || {},
            )
            .await;
        let installed = match downloaded {
            #[cfg(windows)]
            Ok(bytes) => {
                // Installed as the app exits; see `install_on_exit`.
                *updates.pending.lock().unwrap() = Some((update.clone(), bytes));
                Ok(())
            }
            #[cfg(not(windows))]
            Ok(bytes) => {
                let update = update.clone();
                tauri::async_runtime::spawn_blocking(move || update.install(bytes))
                    .await
                    .unwrap_or_else(|e| {
                        Err(tauri_plugin_updater::Error::Io(std::io::Error::other(e)))
                    })
            }
            Err(e) => Err(e),
        };
        match installed {
            Ok(()) => updates.set(&app, UpdateStatus::Ready { version }, None),
            Err(e) => {
                eprintln!("guiderecorder: cannot install the update: {e}");
                let status = UpdateStatus::Failed {
                    message: format!("Could not install GuideRecorder {version}: {e}"),
                    manual: true,
                };
                // Kept, so Install can be tried again.
                updates.set(&app, status, Some(update));
            }
        }
    });
    Ok(())
}

/// Quits through the `QuitGate` (the editor saves, a recording is stopped
/// and saved) and starts the installed update. Rejects with `invalid`
/// unless an update is installed.
#[tauri::command]
pub fn restart_to_update<R: Runtime>(
    app: AppHandle<R>,
    updates: State<'_, Arc<Updates>>,
) -> Result<()> {
    if !matches!(updates.status(), UpdateStatus::Ready { .. }) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "no update is installed yet",
        ));
    }
    app.state::<Arc<crate::commands::QuitGate>>().restart(&app);
    Ok(())
}

/// On Windows, starts the installer for a downloaded update as the app
/// exits (`RunEvent::Exit`, so after the `QuitGate`). It replaces the app
/// and, if `restart`, starts it again; this doesn't return once the
/// installer has started. Elsewhere it does nothing: the update was
/// installed when it was downloaded.
pub fn install_on_exit<R: Runtime>(app: &AppHandle<R>, restart: bool) {
    #[cfg(windows)]
    {
        let pending = app.state::<Arc<Updates>>().pending.lock().unwrap().take();
        if let Some((update, bytes)) = pending {
            // Verifies the signature, starts the installer and exits.
            if let Err(e) = update.restart_after_install(restart).install(bytes) {
                eprintln!("guiderecorder: cannot install the update: {e}");
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (app, restart);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn statuses_serialize_for_the_frontend() {
        let cases = [
            (UpdateStatus::Idle, json!({ "state": "idle" })),
            (
                UpdateStatus::Checking { manual: true },
                json!({ "state": "checking", "manual": true }),
            ),
            (
                UpdateStatus::UpToDate { manual: false },
                json!({ "state": "upToDate", "manual": false }),
            ),
            (
                UpdateStatus::Available {
                    version: "0.2.0".into(),
                    notes: None,
                },
                json!({ "state": "available", "version": "0.2.0", "notes": null }),
            ),
            (
                UpdateStatus::Downloading {
                    version: "0.2.0".into(),
                    downloaded: 5,
                    total: Some(10),
                },
                json!({ "state": "downloading", "version": "0.2.0", "downloaded": 5, "total": 10 }),
            ),
            (
                UpdateStatus::Ready {
                    version: "0.2.0".into(),
                },
                json!({ "state": "ready", "version": "0.2.0" }),
            ),
            (
                UpdateStatus::Failed {
                    message: "offline".into(),
                    manual: true,
                },
                json!({ "state": "failed", "message": "offline", "manual": true }),
            ),
        ];
        for (status, expected) in cases {
            assert_eq!(serde_json::to_value(&status).unwrap(), expected);
        }
    }

    #[test]
    fn only_idle_and_finished_states_start_something_new() {
        let busy = [
            UpdateStatus::Checking { manual: false },
            UpdateStatus::Downloading {
                version: "1".into(),
                downloaded: 0,
                total: None,
            },
            UpdateStatus::Ready {
                version: "1".into(),
            },
        ];
        assert!(busy.iter().all(UpdateStatus::busy));
        let free = [
            UpdateStatus::Idle,
            UpdateStatus::UpToDate { manual: true },
            UpdateStatus::Available {
                version: "1".into(),
                notes: None,
            },
            UpdateStatus::Failed {
                message: String::new(),
                manual: false,
            },
        ];
        assert!(free.iter().all(|s| !s.busy()));
    }

    #[test]
    fn progress_is_sent_per_percent_or_megabyte() {
        let mut progress = Progress {
            version: "1".into(),
            downloaded: 0,
            total: None,
            sent: 0,
        };
        // 1000 bytes: a status per 10 bytes.
        assert_eq!(progress.add(5, Some(1000)), None);
        assert!(progress.add(5, Some(1000)).is_some());
        assert_eq!(progress.add(9, Some(1000)), None);
        assert_eq!(
            progress.add(1, Some(1000)),
            Some(UpdateStatus::Downloading {
                version: "1".into(),
                downloaded: 20,
                total: Some(1000),
            })
        );
        // Unknown size: a status per MB.
        let mut progress = Progress {
            version: "1".into(),
            downloaded: 0,
            total: None,
            sent: 0,
        };
        assert_eq!(progress.add(512 * 1024, None), None);
        assert!(progress.add(512 * 1024, None).is_some());
    }
}
