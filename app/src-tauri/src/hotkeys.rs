//! Global hotkeys: the record shortcut starts or stops a recording, the
//! pause shortcut pauses or resumes it, from any app (through
//! [`recording::control`], as the tray does). Both come from the settings
//! (Settings → Recording) and are registered again whenever they change
//! ([`apply`]).
//!
//! The manual-step shortcut is deliberately not registered: while
//! recording, the capture session's event tap turns it into a manual step
//! and drops the key itself.

use std::sync::Mutex;

use guiderecorder_capture::macos::session::Hotkey;
use serde::Serialize;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::recording::{self, Control};
use crate::settings::Hotkeys;

/// Each registered hotkey's setting and action. Both the registered
/// shortcuts and the keys the capture session drops ([`capture_hotkeys`])
/// come from here.
fn actions(hotkeys: &Hotkeys) -> [(&str, Control); 2] {
    [
        (&hotkeys.record, Control::ToggleRecording),
        (&hotkeys.pause, Control::TogglePause),
    ]
}

/// Every hotkey set (not empty) and its action, or why the stored value is
/// not a shortcut the plugin can register.
fn bindings(hotkeys: &Hotkeys) -> Vec<(Control, Result<Shortcut, String>)> {
    actions(hotkeys)
        .into_iter()
        .filter(|(value, _)| !value.is_empty())
        .map(|(value, control)| {
            let shortcut = value
                .parse::<Shortcut>()
                .map_err(|e| format!("{value} is not a shortcut that can be registered ({e})"));
            (control, shortcut)
        })
        .collect()
}

/// The hotkeys as the capture session matches keys, for
/// `SessionConfig::control_hotkeys`: a press of one while recording is
/// handled here, so it must not become a step.
pub fn capture_hotkeys(hotkeys: &Hotkeys) -> Vec<Hotkey> {
    actions(hotkeys)
        .iter()
        .filter_map(|(value, _)| recording::parse_hotkey(value))
        .collect()
}

/// Why a hotkey could not be registered, for Settings; `None` when it is
/// registered or not set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyErrors {
    pub record: Option<String>,
    pub pause: Option<String>,
}

impl HotkeyErrors {
    fn set(&mut self, control: Control, error: String) {
        match control {
            Control::ToggleRecording => self.record = Some(error),
            Control::TogglePause => self.pause = Some(error),
            // Not a hotkey action (see `actions`).
            _ => {}
        }
    }
}

/// The registered hotkeys and the errors of the last [`apply`].
#[derive(Default)]
pub struct HotkeyState(Mutex<(Vec<(Shortcut, Control)>, HotkeyErrors)>);

impl HotkeyState {
    pub fn errors(&self) -> HotkeyErrors {
        self.0.lock().unwrap().1.clone()
    }

    fn control_for(&self, shortcut: &Shortcut, state: ShortcutState) -> Option<Control> {
        control_for(&self.0.lock().unwrap().0, shortcut, state)
    }
}

/// The action for a press of `shortcut` among `registered`; `None` for a
/// release or a shortcut not ours.
fn control_for(
    registered: &[(Shortcut, Control)],
    shortcut: &Shortcut,
    state: ShortcutState,
) -> Option<Control> {
    if state != ShortcutState::Pressed {
        return None;
    }
    registered
        .iter()
        .find(|(s, _)| s == shortcut)
        .map(|(_, control)| *control)
}

/// The plugin, with the handler that runs the actions. The hotkeys are
/// registered by [`apply`], so one taken by another app does not stop the
/// app from starting.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(
            |app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent| {
                let control = app
                    .try_state::<HotkeyState>()
                    .and_then(|state| state.control_for(shortcut, event.state()));
                if let Some(control) = control {
                    recording::control(app, control);
                }
            },
        )
        .build()
}

/// Registers the hotkeys in `hotkeys` in place of the ones registered
/// before (so an old shortcut stops working at once). One that cannot be
/// registered (e.g. taken by another app or macOS) is skipped and its error
/// kept for Settings ([`HotkeyState::errors`]).
pub fn apply<R: Runtime>(app: &AppHandle<R>, hotkeys: &Hotkeys) {
    let Some(state) = app.try_state::<HotkeyState>() else {
        return;
    };
    let mut state = state.0.lock().unwrap();
    let shortcuts = app.global_shortcut();
    for (shortcut, _) in state.0.drain(..) {
        if let Err(e) = shortcuts.unregister(shortcut) {
            eprintln!("guiderecorder: cannot unregister the hotkey {shortcut}: {e}");
        }
    }
    let mut errors = HotkeyErrors::default();
    for (control, shortcut) in bindings(hotkeys) {
        let registered = shortcut.and_then(|shortcut| {
            shortcuts
                .register(shortcut)
                .map(|()| shortcut)
                .map_err(|e| {
                    format!("This shortcut can't be used: another app or macOS may have it ({e}).")
                })
        });
        match registered {
            Ok(shortcut) => state.0.push((shortcut, control)),
            Err(e) => {
                eprintln!("guiderecorder: hotkey: {e}");
                errors.set(control, e);
            }
        }
    }
    state.1 = errors;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    fn hotkeys(record: &str, pause: &str) -> Hotkeys {
        Hotkeys {
            record: record.into(),
            pause: pause.into(),
            manual_step: "Ctrl+Alt+S".into(),
        }
    }

    fn registered(hotkeys: &Hotkeys) -> Vec<(Shortcut, Control)> {
        bindings(hotkeys)
            .into_iter()
            .map(|(control, shortcut)| (shortcut.unwrap(), control))
            .collect()
    }

    #[test]
    fn hotkeys_map_to_toggles_on_press_only() {
        let bound = registered(&hotkeys("Ctrl+Alt+R", "Ctrl+Alt+P"));
        let ctrl_option = Some(Modifiers::CONTROL | Modifiers::ALT);
        let r = Shortcut::new(ctrl_option, Code::KeyR);
        let p = Shortcut::new(ctrl_option, Code::KeyP);
        assert_eq!(
            control_for(&bound, &r, ShortcutState::Pressed),
            Some(Control::ToggleRecording)
        );
        assert_eq!(
            control_for(&bound, &p, ShortcutState::Pressed),
            Some(Control::TogglePause)
        );
        assert_eq!(control_for(&bound, &r, ShortcutState::Released), None);
        // Same key, other modifiers: not ours.
        let cmd_r = Shortcut::new(Some(Modifiers::SUPER), Code::KeyR);
        assert_eq!(control_for(&bound, &cmd_r, ShortcutState::Pressed), None);
    }

    #[test]
    fn bindings_follow_the_settings() {
        let bound = registered(&hotkeys("CmdOrCtrl+Shift+F5", "Option+Command+9"));
        assert_eq!(
            bound,
            [
                (
                    Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::F5),
                    Control::ToggleRecording
                ),
                (
                    Shortcut::new(Some(Modifiers::ALT | Modifiers::SUPER), Code::Digit9),
                    Control::TogglePause
                ),
            ]
        );
        // The old ⌃⌥R is not ours any more.
        let r = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyR);
        assert_eq!(control_for(&bound, &r, ShortcutState::Pressed), None);
        // An empty setting is no hotkey.
        assert_eq!(
            registered(&hotkeys("", "Ctrl+Alt+P")),
            [(
                Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyP),
                Control::TogglePause
            )]
        );
        assert!(bindings(&hotkeys("", "")).is_empty());
    }

    #[test]
    fn a_value_the_plugin_cannot_parse_is_an_error() {
        let bound = bindings(&hotkeys("Ctrl+Alt+R", "Hyper+P"));
        assert!(bound[0].1.is_ok());
        assert_eq!(bound[1].0, Control::TogglePause);
        assert!(bound[1].1.as_ref().unwrap_err().contains("Hyper+P"));
    }

    /// Every key and modifier `settings::is_hotkey` accepts can be
    /// registered, so a stored shortcut never fails to parse.
    #[test]
    fn every_shortcut_the_settings_accept_parses() {
        let mut keys: Vec<String> = ('A'..='Z').chain('0'..='9').map(String::from).collect();
        keys.extend((1..=20).map(|n| format!("F{n}")));
        keys.extend(
            ["Space", "Enter", "Tab", "Escape", "Backspace", "Delete"]
                .iter()
                .map(|k| k.to_string()),
        );
        let mods = [
            "CmdOrCtrl",
            "CommandOrControl",
            "Cmd",
            "Command",
            "Super",
            "Ctrl",
            "Control",
            "Alt",
            "Option",
        ];
        for key in &keys {
            for m in mods {
                let value = format!("{m}+Shift+{key}");
                assert!(crate::settings::is_hotkey(&value), "{value}");
                assert!(value.parse::<Shortcut>().is_ok(), "{value}");
            }
        }
    }

    #[test]
    fn capture_hotkeys_are_the_registered_ones() {
        let hotkeys = capture_hotkeys(&hotkeys("Ctrl+Alt+R", "Ctrl+Alt+P"));
        // kVK_ANSI_R and kVK_ANSI_P with exactly ⌃⌥, as ⌃⌥S has.
        assert_eq!(hotkeys.iter().map(|h| h.code).collect::<Vec<_>>(), [15, 35]);
        assert!(hotkeys
            .iter()
            .all(|h| h.modifiers == Hotkey::MANUAL_STEP.modifiers));
        assert!(!hotkeys.contains(&Hotkey::MANUAL_STEP));
    }

    #[test]
    fn capture_hotkeys_follow_the_settings() {
        let dropped = capture_hotkeys(&hotkeys("CmdOrCtrl+Shift+F5", ""));
        assert_eq!(dropped, [recording::parse_hotkey("Cmd+Shift+F5").unwrap()]);
        assert!(capture_hotkeys(&hotkeys("", "")).is_empty());
    }

    /// The manual-step hotkey is left to the capture session's event tap.
    #[test]
    fn manual_step_hotkey_is_not_registered() {
        let settings = hotkeys("Ctrl+Alt+R", "Ctrl+Alt+P");
        let s: Shortcut = settings.manual_step.parse().unwrap();
        let bound = registered(&settings);
        assert!(bound.iter().all(|(b, _)| *b != s));
        assert_eq!(control_for(&bound, &s, ShortcutState::Pressed), None);
        assert!(!capture_hotkeys(&settings).contains(&Hotkey::MANUAL_STEP));
    }

    #[test]
    fn errors_are_per_hotkey() {
        let mut errors = HotkeyErrors::default();
        errors.set(Control::TogglePause, "taken".into());
        assert_eq!(
            errors,
            HotkeyErrors {
                record: None,
                pause: Some("taken".into())
            }
        );
        assert_eq!(
            serde_json::to_value(&errors).unwrap(),
            serde_json::json!({ "record": null, "pause": "taken" })
        );
    }
}
