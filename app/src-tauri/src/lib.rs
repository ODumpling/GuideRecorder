use std::sync::Arc;

use guidesya_core::index::Index;
use serde::Serialize;
use tauri::{DragDropEvent, Manager, RunEvent, WindowEvent};

mod commands;
mod editor;
mod export;
mod navigation;
mod settings;
mod settings_commands;

use editor::{AppError, DroppedFiles, ErrorKind, Session, SessionCell};

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
        schema_version: guidesya_core::SCHEMA_VERSION,
        capture_backend: guidesya_capture::backend_name(),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(navigation::guard())
        // Dialogs and Reveal run from Rust only: no capability grants them
        // to the webview.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            app.manage(Arc::new(commands::QuitGate::default()));
            app.manage(Arc::new(DroppedFiles::default()));
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
            let root = settings_commands::startup_library(
                std::env::var_os(editor::LIBRARY_ENV),
                stored.general.library,
                app.path().document_dir().ok(),
            );
            let target = Arc::new(settings_commands::LibraryTarget::default());
            target.set(root.clone());
            app.manage(target);
            // Recovery and the index sync touch the disk: keep them off the
            // main thread. Commands wait for the session.
            tauri::async_runtime::spawn_blocking(move || {
                let session = match root {
                    Some(root) => Session::start(&root, Index::default_path()),
                    None => Err(AppError::new(
                        ErrorKind::Io,
                        format!("no library folder: set {}", editor::LIBRARY_ENV),
                    )),
                };
                if let Err(e) = &session {
                    eprintln!("guidesya: cannot open library: {}", e.message);
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
            settings_commands::mcp_setup,
            commands::quit_ack,
            commands::quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building GuidesYa")
        .run(|app, event| {
            if let RunEvent::ExitRequested { code, api, .. } = event {
                let gate = app.state::<Arc<commands::QuitGate>>();
                if gate.on_exit_requested(app, code) {
                    api.prevent_exit();
                }
            }
        });
}
