//! The menu bar icon: idle, recording or paused (template images), with a
//! menu to start, pause, resume and stop a recording, open the main window
//! and quit. Updated from every recording status sent (see
//! `recording::emit`); its actions go through [`recording::control`].

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime};

use crate::recording::{self, Control, Recorder, RecordingStatus};

const TRAY_ID: &str = "guidesya-tray";

const START_ID: &str = "tray-start";
const PAUSE_ID: &str = "tray-pause";
const RESUME_ID: &str = "tray-resume";
const STOP_ID: &str = "tray-stop";
const OPEN_ID: &str = "tray-open";
const QUIT_ID: &str = "tray-quit";

const IDLE_ICON: &[u8] = include_bytes!("../icons/tray/idle.png");
const RECORDING_ICON: &[u8] = include_bytes!("../icons/tray/recording.png");
const PAUSED_ICON: &[u8] = include_bytes!("../icons/tray/paused.png");

/// What the tray shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Recording,
    Paused,
    /// A stopped recording is being saved: shown as recording, with every
    /// recording item disabled.
    Saving,
}

impl TrayState {
    pub fn of(status: &RecordingStatus) -> Self {
        match status {
            RecordingStatus::Idle => Self::Idle,
            RecordingStatus::Recording { .. } => Self::Recording,
            RecordingStatus::Paused { .. } => Self::Paused,
            RecordingStatus::Saving { .. } => Self::Saving,
        }
    }

    fn icon(self) -> &'static [u8] {
        match self {
            Self::Idle => IDLE_ICON,
            Self::Recording | Self::Saving => RECORDING_ICON,
            Self::Paused => PAUSED_ICON,
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "GuidesYa",
            Self::Recording => "GuidesYa — recording",
            Self::Paused => "GuidesYa — paused",
            Self::Saving => "GuidesYa — saving the recording",
        }
    }
}

/// Which menu items are enabled; Open GuidesYa and Quit always are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enabled {
    pub start: bool,
    pub pause: bool,
    pub resume: bool,
    pub stop: bool,
}

impl Enabled {
    pub fn of(state: TrayState) -> Self {
        Self {
            start: state == TrayState::Idle,
            pause: state == TrayState::Recording,
            resume: state == TrayState::Paused,
            stop: matches!(state, TrayState::Recording | TrayState::Paused),
        }
    }
}

/// The action a menu item stands for; `None` for Open, Quit and ids not
/// ours.
pub fn control_for(id: &str) -> Option<Control> {
    match id {
        START_ID => Some(Control::Start),
        PAUSE_ID => Some(Control::Pause),
        RESUME_ID => Some(Control::Resume),
        STOP_ID => Some(Control::Stop),
        _ => None,
    }
}

/// The menu's recording items, kept to enable them per state.
struct Items<R: Runtime> {
    start: MenuItem<R>,
    pause: MenuItem<R>,
    resume: MenuItem<R>,
    stop: MenuItem<R>,
}

impl<R: Runtime> Items<R> {
    fn apply(&self, enabled: Enabled) -> tauri::Result<()> {
        self.start.set_enabled(enabled.start)?;
        self.pause.set_enabled(enabled.pause)?;
        self.resume.set_enabled(enabled.resume)?;
        self.stop.set_enabled(enabled.stop)
    }
}

/// The tray icon and its items (managed state).
struct Tray<R: Runtime> {
    icon: TrayIcon<R>,
    items: Items<R>,
}

/// Creates the tray icon in its state for the current status.
pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let state = TrayState::of(&app.state::<Arc<Recorder>>().status().status);
    let enabled = Enabled::of(state);
    let item = |id: &str, text: &str, on: bool| MenuItem::with_id(app, id, text, on, None::<&str>);
    let items = Items {
        start: item(START_ID, "Start Recording", enabled.start)?,
        pause: item(PAUSE_ID, "Pause", enabled.pause)?,
        resume: item(RESUME_ID, "Resume", enabled.resume)?,
        stop: item(STOP_ID, "Stop and Save", enabled.stop)?,
    };
    let open = item(OPEN_ID, "Open GuidesYa", true)?;
    let quit = item(QUIT_ID, "Quit GuidesYa", true)?;
    let menu = Menu::with_items(
        app,
        &[
            &items.start,
            &items.pause,
            &items.resume,
            &items.stop,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let icon = TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(state.icon())?)
        .icon_as_template(true)
        .tooltip(state.tooltip())
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(on_menu_event)
        .build(app)?;
    app.manage(Tray { icon, items });
    Ok(())
}

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    if let Some(control) = control_for(id) {
        recording::control(app, control);
    } else if id == OPEN_ID {
        show_main(app);
    } else if id == QUIT_ID {
        // The app menu's Quit path: the editor saves first.
        app.state::<Arc<crate::commands::QuitGate>>()
            .on_quit_menu(app);
    }
}

/// Puts the icon and the menu in line with the stored status. A no-op
/// before the tray exists.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let (Some(tray), Some(recorder)) =
        (app.try_state::<Tray<R>>(), app.try_state::<Arc<Recorder>>())
    else {
        return;
    };
    let state = TrayState::of(&recorder.status().status);
    let result = Image::from_bytes(state.icon())
        .and_then(|icon| tray.icon.set_icon_with_as_template(Some(icon), true))
        .and_then(|()| tray.icon.set_tooltip(Some(state.tooltip())))
        .and_then(|()| tray.items.apply(Enabled::of(state)));
    if let Err(e) = result {
        eprintln!("guidesya: cannot update the tray: {e}");
    }
}

/// Shows, unminimizes and focuses the main window.
pub fn show_main<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_items_are_enabled_per_state() {
        let on = |start, pause, resume, stop| Enabled {
            start,
            pause,
            resume,
            stop,
        };
        assert_eq!(Enabled::of(TrayState::Idle), on(true, false, false, false));
        assert_eq!(
            Enabled::of(TrayState::Recording),
            on(false, true, false, true)
        );
        assert_eq!(Enabled::of(TrayState::Paused), on(false, false, true, true));
        assert_eq!(
            Enabled::of(TrayState::Saving),
            on(false, false, false, false)
        );
    }

    #[test]
    fn tray_state_follows_the_status() {
        assert_eq!(TrayState::of(&RecordingStatus::Idle), TrayState::Idle);
        let recording = RecordingStatus::Recording {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            excluded_app: None,
        };
        assert_eq!(TrayState::of(&recording), TrayState::Recording);
        let paused = RecordingStatus::Paused {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
            capture_area_fallback: false,
            paused_ms: 0,
            paused_at: "2026-10-15T01:01:00.000Z".into(),
            excluded_app: None,
        };
        assert_eq!(TrayState::of(&paused), TrayState::Paused);
        let saving = RecordingStatus::Saving {
            started_at: "2026-10-15T01:00:00.000Z".into(),
            title: None,
        };
        assert_eq!(TrayState::of(&saving), TrayState::Saving);
        // Three distinct icons, each a PNG.
        let icons = [IDLE_ICON, RECORDING_ICON, PAUSED_ICON];
        for (i, icon) in icons.iter().enumerate() {
            assert!(icon.starts_with(b"\x89PNG"));
            assert!(icons[i + 1..].iter().all(|other| other != icon));
        }
        assert_ne!(TrayState::Idle.icon(), TrayState::Paused.icon());
        assert_ne!(TrayState::Recording.icon(), TrayState::Paused.icon());
    }

    #[test]
    fn menu_ids_map_to_controls() {
        assert_eq!(control_for(START_ID), Some(Control::Start));
        assert_eq!(control_for(PAUSE_ID), Some(Control::Pause));
        assert_eq!(control_for(RESUME_ID), Some(Control::Resume));
        assert_eq!(control_for(STOP_ID), Some(Control::Stop));
        for other in [OPEN_ID, QUIT_ID, "guidesya-quit", ""] {
            assert_eq!(control_for(other), None, "{other}");
        }
    }
}
