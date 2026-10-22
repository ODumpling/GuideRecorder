use std::sync::Arc;

use guiderecorder_core::index::Index;
use serde::Serialize;
#[cfg(target_os = "macos")]
use tauri::{menu::Menu, AppHandle, Runtime};
use tauri::{DragDropEvent, Manager, RunEvent, WindowEvent};

mod ai;
mod annotated;
mod blocking;
mod commands;
mod editor;
mod export;
mod feed;
mod hotkeys;
mod narration;
mod navigation;
mod pdf;
mod picker;
mod points;
mod presets;
mod recapture;
mod recording;
mod redaction;
mod settings;
mod settings_commands;
mod sync;
mod terminate;
mod toolbar;
mod tray;
mod updater;

use editor::{DroppedFiles, SessionCell};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    version: &'static str,
    schema_version: u32,
    capture_backend: &'static str,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        schema_version: guiderecorder_core::SCHEMA_VERSION,
        capture_backend: guiderecorder_capture::backend_name(),
    }
}

/// Id of the app menu's own Quit item; see [`app_menu`].
const QUIT_MENU_ID: &str = "guiderecorder-quit";
/// Id of the app menu's Check for Updates… item, after About.
const CHECK_UPDATES_MENU_ID: &str = "guiderecorder-check-updates";

/// Tauri's default menu with Check for Updates… after About, and its Quit
/// item swapped for one that goes through the `QuitGate`. The predefined
/// item sends NSApp `terminate:`, which tao handles in
/// `applicationWillTerminate` without a `RunEvent::ExitRequested`, so the
/// editor would not get to save.
///
/// macOS only. Windows apps like this one have no menu bar, and Tauri's
/// default one there (File, Edit, Window, Help) would bring a Quit that
/// posts `WM_QUIT` past the `QuitGate`; the tray menu has Check for
/// Updates… and Quit instead.
#[cfg(target_os = "macos")]
fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    {
        use tauri::menu::{MenuItem, MenuItemKind};
        if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
            let check = MenuItem::with_id(
                app,
                CHECK_UPDATES_MENU_ID,
                "Check for Updates…",
                true,
                None::<&str>,
            )?;
            // About is first.
            app_menu.insert(&check, 1)?;
            let items = app_menu.items()?;
            let quit = items.iter().position(|item| match item {
                MenuItemKind::Predefined(item) => item.text().is_ok_and(|t| t.starts_with("Quit")),
                _ => false,
            });
            if let Some(position) = quit {
                app_menu.remove_at(position)?;
                let label = format!("Quit {}", app.package_info().name);
                let item = MenuItem::with_id(app, QUIT_MENU_ID, label, true, Some("CmdOrCtrl+Q"))?;
                app_menu.insert(&item, position)?;
                return Ok(menu);
            }
        }
        eprintln!(
            "guiderecorder: no Quit item in the default menu; Cmd+Q will quit without saving"
        );
    }
    Ok(menu)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.menu(app_menu);
    builder
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ID {
                app.state::<Arc<commands::QuitGate>>().on_quit_menu(app);
            } else if event.id() == CHECK_UPDATES_MENU_ID {
                tray::show_main(app);
                updater::check_from_menu(app);
            }
        })
        .plugin(navigation::guard())
        // Dialogs and Reveal run from Rust only: no capability grants them
        // to the webview.
        .plugin(tauri_plugin_dialog::init())
        .plugin(hotkeys::plugin())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        // Used from Rust only (`updater.rs`); no capability grants it.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            app.manage(Arc::new(commands::QuitGate::default()));
            // Dock Quit, AppleScript `quit` and logout save first too.
            terminate::install(app.handle());
            app.manage(Arc::new(DroppedFiles::default()));
            app.manage(Arc::new(recording::Recorder::default()));
            app.manage(Arc::new(picker::RegionPicker::default()));
            app.manage(Arc::new(ai::AiRuns::default()));
            app.manage(Arc::new(sync::SyncState::default()));
            if let Err(e) = tray::create(app.handle()) {
                eprintln!("guiderecorder: cannot create the tray icon: {e}");
            }
            toolbar::sync(app.handle());
            // A few kB, read once before the library is chosen.
            let store = settings::SettingsStore::load(settings::default_path());
            let stored = store.get();
            app.manage(hotkeys::HotkeyState::default());
            hotkeys::apply(app.handle(), &stored.recording.hotkeys);
            let chosen = export::ChosenPaths::default();
            // Stored paths were chosen in a dialog when they were set.
            if let Some(folder) = stored.export.folder.clone() {
                chosen.add_folder(folder);
            }
            if let Some(logo) = stored.export.logo.clone() {
                chosen.add_logo(logo);
            }
            app.manage(Arc::new(store));
            app.manage(Arc::new(chosen));
            app.manage(Arc::new(updater::Updates::default()));
            updater::start_background_checks(app.handle());
            let cell = Arc::new(SessionCell::default());
            app.manage(Arc::clone(&cell));
            let from_env = std::env::var_os(editor::LIBRARY_ENV).filter(|v| !v.is_empty());
            // Only a folder from Settings must already exist.
            let configured = from_env.is_none() && stored.general.library.is_some();
            let root = settings_commands::startup_library(
                from_env,
                stored.general.library,
                app.path().document_dir().ok(),
            );
            let target = Arc::new(settings_commands::LibraryTarget::default());
            target.set(root.clone());
            app.manage(target);
            // Recovery and the index sync touch the disk: keep them off the
            // main thread. Commands wait for the session.
            tauri::async_runtime::spawn_blocking(move || {
                let session = settings_commands::open_startup_library(
                    root.as_deref(),
                    configured,
                    Index::default_path(),
                );
                if let Err(e) = &session {
                    eprintln!("guiderecorder: cannot open library: {}", e.message);
                }
                cell.set(session);
            });
            Ok(())
        })
        // Files dropped on the window are the only ones `read_image_file`
        // reads.
        .on_window_event(|window, event| {
            match event {
                WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) => {
                    window.state::<Arc<DroppedFiles>>().record(paths);
                }
                // Windows' light or dark mode changed: so may the taskbar's,
                // which the tray icon follows there.
                WindowEvent::ThemeChanged(_) if cfg!(windows) => tray::refresh(window.app_handle()),
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            commands::library_root,
            commands::list_guides,
            commands::open_guide,
            commands::save_guide,
            commands::close_guide,
            commands::list_versions,
            commands::read_version,
            commands::version_image,
            commands::save_version,
            commands::restore_version,
            commands::delete_guide,
            commands::list_folders,
            commands::create_folder,
            commands::duplicate_guide,
            commands::rename_guide,
            commands::mark_guide_reviewed,
            commands::edit_guide_details,
            commands::delete_guides,
            commands::move_guide,
            commands::reveal_in_library,
            commands::step_image,
            commands::guide_cover,
            commands::add_image_step,
            commands::new_annotated_guide,
            commands::add_annotated_pages,
            commands::copy_step_images,
            commands::read_image_file,
            commands::choose_export_folder,
            commands::choose_export_file,
            commands::choose_logo,
            commands::export_guide,
            commands::preview_export,
            commands::logo_preview,
            commands::list_presets,
            commands::save_preset,
            commands::delete_preset,
            commands::set_default_preset,
            commands::reveal_export,
            commands::open_step_url,
            commands::verify_crop,
            settings_commands::get_settings,
            settings_commands::update_settings,
            settings_commands::library_info,
            settings_commands::choose_library_folder,
            settings_commands::switch_library,
            settings_commands::set_launch_at_login,
            settings_commands::launch_at_login,
            settings_commands::api_key_status,
            settings_commands::set_api_key,
            settings_commands::delete_api_key,
            settings_commands::hotkey_errors,
            settings_commands::permission_status,
            settings_commands::open_permission_settings,
            settings_commands::request_permission,
            settings_commands::recording_readiness,
            settings_commands::mcp_setup,
            settings_commands::restart_app,
            settings_commands::narration_permissions,
            recording::start_recording,
            recording::pause_recording,
            recording::resume_recording,
            recording::stop_recording,
            recording::cancel_recording,
            recording::recording_status,
            recording::recording_snapshot,
            recording::add_manual_step,
            narration::narration_state,
            narration::set_narration,
            feed::delete_capture,
            feed::rename_capture,
            toolbar::resize_toolbar,
            toolbar::set_toolbar_typing,
            picker::list_capture_windows,
            picker::list_capture_displays,
            picker::pick_region,
            picker::finish_region_pick,
            ai::ai_generate,
            ai::ai_cancel,
            ai::ai_rewrite_step,
            commands::quit_ack,
            commands::quit_app,
            commands::quit_cancel,
            updater::update_status,
            updater::check_for_update,
            updater::install_update,
            updater::restart_to_update,
            sync::sync_account,
            sync::sync_sign_in_start,
            sync::sync_sign_in_poll,
            sync::sync_sign_out,
            sync::sync_open_web,
            sync::sync_request,
            sync::sync_load_state,
            sync::sync_save_state,
            sync::sync_read_guide,
            sync::sync_prepare_guide,
            sync::sync_upload,
            sync::sync_apply_guide,
        ])
        .build(tauri::generate_context!())
        .expect("error while building GuideRecorder")
        .run(|app, event| match event {
            RunEvent::ExitRequested { code, api, .. } => {
                let gate = app.state::<Arc<commands::QuitGate>>();
                if gate.on_exit_requested(app, code) {
                    api.prevent_exit();
                }
            }
            RunEvent::Exit => {
                // After `QuitGate::restart` (an update, a Screen Recording grant).
                let restart = app.state::<Arc<commands::QuitGate>>().restart_requested();
                // On Windows a downloaded update installs now, and the
                // installer restarts the app; see `updater::install_on_exit`.
                updater::install_on_exit(app, restart);
                if restart {
                    tauri::process::restart(&app.env());
                }
            }
            _ => {}
        });
}
