//! Global hotkeys: ⌃⌥R starts or stops a recording, ⌃⌥P pauses or resumes
//! it, from any app (through [`recording::control`], as the tray does).
//!
//! ⌃⌥S is deliberately not registered: while recording, the capture
//! session's event tap turns the manual-step hotkey into a manual step and
//! drops the key itself.

use guiderecorder_capture::macos::session::Hotkey;
use tauri::plugin::TauriPlugin;
use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{
    Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutEvent, ShortcutState,
};

use crate::recording::{self, Control};

/// Every hotkey: its key (as the settings name it, and as the plugin's
/// code) and its action, all with ⌃⌥. Both the registered shortcuts and the
/// keys the capture session drops ([`capture_hotkeys`]) come from here.
const KEYS: [(&str, Code, Control); 2] = [
    ("R", Code::KeyR, Control::ToggleRecording),
    ("P", Code::KeyP, Control::TogglePause),
];

/// Every hotkey and its action.
fn bindings() -> [(Shortcut, Control); 2] {
    let ctrl_option = Some(Modifiers::CONTROL | Modifiers::ALT);
    KEYS.map(|(_, code, control)| (Shortcut::new(ctrl_option, code), control))
}

/// The hotkeys as the capture session matches keys, for
/// `SessionConfig::control_hotkeys`: a press of one while recording is
/// handled here, so it must not become a step.
pub fn capture_hotkeys() -> Vec<Hotkey> {
    KEYS.iter()
        .filter_map(|(key, _, _)| recording::parse_hotkey(&format!("Ctrl+Alt+{key}")))
        .collect()
}

/// The action for a press of `shortcut`; `None` for a release or a
/// shortcut not ours.
pub fn control_for(shortcut: &Shortcut, state: ShortcutState) -> Option<Control> {
    if state != ShortcutState::Pressed {
        return None;
    }
    bindings()
        .into_iter()
        .find(|(s, _)| s == shortcut)
        .map(|(_, control)| control)
}

/// The plugin, with the handler that runs the actions. The hotkeys are
/// registered by [`register`], so one taken by another app does not stop
/// the app from starting.
pub fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(
            |app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent| {
                if let Some(control) = control_for(shortcut, event.state()) {
                    recording::control(app, control);
                }
            },
        )
        .build()
}

/// Registers every hotkey; one that cannot be registered (e.g. taken by
/// another app) is logged and skipped.
pub fn register<R: Runtime>(app: &AppHandle<R>) {
    for (shortcut, _) in bindings() {
        if let Err(e) = app.global_shortcut().register(shortcut) {
            eprintln!("guiderecorder: cannot register the hotkey {shortcut}: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_map_to_toggles_on_press_only() {
        let ctrl_option = Some(Modifiers::CONTROL | Modifiers::ALT);
        let r = Shortcut::new(ctrl_option, Code::KeyR);
        let p = Shortcut::new(ctrl_option, Code::KeyP);
        assert_eq!(
            control_for(&r, ShortcutState::Pressed),
            Some(Control::ToggleRecording)
        );
        assert_eq!(
            control_for(&p, ShortcutState::Pressed),
            Some(Control::TogglePause)
        );
        assert_eq!(control_for(&r, ShortcutState::Released), None);
        // Same key, other modifiers: not ours.
        let cmd_r = Shortcut::new(Some(Modifiers::SUPER), Code::KeyR);
        assert_eq!(control_for(&cmd_r, ShortcutState::Pressed), None);
    }

    #[test]
    fn capture_hotkeys_are_the_registered_ones() {
        let hotkeys = capture_hotkeys();
        assert_eq!(hotkeys.len(), KEYS.len());
        // kVK_ANSI_R and kVK_ANSI_P with exactly ⌃⌥, as ⌃⌥S has.
        assert_eq!(hotkeys.iter().map(|h| h.code).collect::<Vec<_>>(), [15, 35]);
        assert!(hotkeys
            .iter()
            .all(|h| h.modifiers == Hotkey::MANUAL_STEP.modifiers));
        assert!(!hotkeys.contains(&Hotkey::MANUAL_STEP));
    }

    /// The manual-step hotkey is left to the capture session's event tap.
    #[test]
    fn manual_step_hotkey_is_not_registered() {
        let s = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyS);
        assert!(bindings().iter().all(|(b, _)| *b != s));
        assert_eq!(control_for(&s, ShortcutState::Pressed), None);
        // Parsed the way the settings store shortcuts.
        let parsed: Shortcut = "Control+Alt+R".parse().unwrap();
        assert_eq!(
            control_for(&parsed, ShortcutState::Pressed),
            Some(Control::ToggleRecording)
        );
    }
}
