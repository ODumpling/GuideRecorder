use std::sync::Arc;

use guiderecorder_core::index::Index;
use serde::Serialize;
use tauri::menu::Menu;
use tauri::{AppHandle, DragDropEvent, Manager, RunEvent, Runtime, WindowEvent};

mod ai;
mod blocking;
mod commands;
mod editor;
mod export;
mod hotkeys;
mod navigation;
mod picker;
mod recording;
mod settings;
mod settings_commands;
mod terminate;
mod toolbar;
mod tray;

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

/// Tauri's default menu with its Quit item swapped for one that goes
/// through the `QuitGate`. The predefined item sends NSApp `terminate:`,
/// which tao handles in `applicationWillTerminate` without a
/// `RunEvent::ExitRequested`, so the editor would not get to save.
fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let menu = Menu::default(app)?;
    #[cfg(target_os = "macos")]
    {
        use tauri::menu::{MenuItem, MenuItemKind};
        if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
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
    tauri::Builder::default()
        .menu(app_menu)
        .on_menu_event(|app, event| {
            if event.id() == QUIT_MENU_ID {
                app.state::<Arc<commands::QuitGate>>().on_quit_menu(app);
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
        .setup(|app| {
            app.manage(Arc::new(commands::QuitGate::default()));
            // Dock Quit, AppleScript `quit` and logout save first too.
            terminate::install(app.handle());
            app.manage(Arc::new(DroppedFiles::default()));
            app.manage(Arc::new(recording::Recorder::default()));
            app.manage(Arc::new(picker::RegionPicker::default()));
            app.manage(Arc::new(ai::AiRuns::default()));
            if let Err(e) = tray::create(app.handle()) {
                eprintln!("guiderecorder: cannot create the tray icon: {e}");
            }
            hotkeys::register(app.handle());
            toolbar::sync(app.handle());
            // A few kB, read once before the library is chosen.
            let store = settings::SettingsStore::load(settings::default_path());
            let stored = store.get();
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
            if let WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) = event {
                window.state::<Arc<DroppedFiles>>().record(paths);
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            commands::library_root,
            commands::list_guides,
            commands::open_guide,
            commands::save_guide,
            commands::close_guide,
            commands::delete_guide,
            commands::step_image,
            commands::add_image_step,
            commands::copy_step_images,
            commands::read_image_file,
            commands::choose_export_folder,
            commands::choose_export_file,
            commands::choose_logo,
            commands::export_guide,
            commands::reveal_export,
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
            settings_commands::permission_status,
            settings_commands::open_permission_settings,
            settings_commands::request_permission,
            settings_commands::recording_readiness,
            settings_commands::mcp_setup,
            recording::start_recording,
            recording::pause_recording,
            recording::resume_recording,
            recording::stop_recording,
            recording::cancel_recording,
            recording::recording_status,
            recording::recording_snapshot,
            recording::add_manual_step,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building GuideRecorder")
        .run(|app, event| {
            if let RunEvent::ExitRequested { code, api, .. } = event {
                let gate = app.state::<Arc<commands::QuitGate>>();
                if gate.on_exit_requested(app, code) {
                    api.prevent_exit();
                }
            }
        });
}
