use std::sync::Arc;

use guidesya_core::index::Index;
use serde::Serialize;
use tauri::{Manager, RunEvent};

mod commands;
mod editor;

use editor::{AppError, ErrorKind, Session, SessionCell};

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
        .setup(|app| {
            app.manage(Arc::new(commands::QuitGate::default()));
            let cell = Arc::new(SessionCell::default());
            app.manage(Arc::clone(&cell));
            let root = editor::library_root(
                std::env::var_os(editor::LIBRARY_ENV),
                app.path().document_dir().ok(),
            );
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
        .invoke_handler(tauri::generate_handler![
            app_info,
            commands::library_root,
            commands::list_guides,
            commands::open_guide,
            commands::save_guide,
            commands::step_image,
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
