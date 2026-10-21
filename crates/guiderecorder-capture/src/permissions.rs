//! OS permissions required for recording (design #440, "Permissions (Q33)").
//!
//! These are macOS's (TCC). Windows asks for none of them: desktop apps
//! may capture the screen, hook input and use UI Automation, so every
//! permission recording needs reports [`PermissionStatus::Granted`] there.
//!
//! Recording needs [`Permission::ALL`]. Narration (#13, see
//! [`crate::narration`]) needs [`Permission::NARRATION`] as well, but only
//! when it is turned on: those never hold a recording back. Where narration
//! is not supported they report [`PermissionStatus::Unknown`]. On Windows
//! they are read from its settings, never prompted for
//! (`windows::permissions`): the microphone from Settings → Privacy &
//! security → Microphone, and speech recognition is granted when a desktop
//! dictation recogniser is installed.
//!
//! [`check`] / [`check_all`] are side-effect free and cheap enough to poll for
//! a live checklist. [`request`] may show a system prompt and must only be
//! called in response to an explicit user action.
//!
//! # Is Input Monitoring needed alongside Accessibility?
//!
//! Short answer: yes for keystrokes, so we track both. Since macOS 10.15
//! (WWDC19 session 701 "Advances in macOS Security"; Apple DTS on the
//! developer forums), TCC splits event taps into two services:
//! a *listen-only* `CGEventTap` (`kCGEventTapOptionListenOnly`) is gated by
//! Input Monitoring (`kTCCServiceListenEvent`, also checked by
//! `CGPreflightListenEventAccess` / `IOHIDCheckAccess(ListenEvent)`), while a
//! modifying tap and event posting are gated by Accessibility. Without Input
//! Monitoring a listen-only tap is still created but keyboard events are
//! silently withheld; mouse events arrive regardless. An Accessibility grant
//! is observed in practice to also satisfy listen-only taps, but that is not
//! documented, so we do not rely on it. We need Accessibility anyway for AX
//! element lookups. Not yet verified at runtime on macOS 26 — confirm in the
//! M0 event-tap spike (#356).

use serde::{Deserialize, Serialize};

/// A TCC permission GuideRecorder needs to record, or to narrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ScreenRecording,
    Accessibility,
    InputMonitoring,
    /// Narration: hearing the microphone.
    Microphone,
    /// Narration: transcribing it (always on the device).
    SpeechRecognition,
}

impl Permission {
    /// What every recording needs.
    pub const ALL: [Permission; 3] = [
        Permission::ScreenRecording,
        Permission::Accessibility,
        Permission::InputMonitoring,
    ];

    /// What narrating a recording needs, on top of [`Permission::ALL`].
    pub const NARRATION: [Permission; 2] = [Permission::Microphone, Permission::SpeechRecognition];
}

/// Current grant state of a [`Permission`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    /// Not granted. For Screen Recording and Accessibility the OS cannot
    /// distinguish "denied" from "never asked", so both report `Denied`.
    Denied,
    /// The user has not been asked yet (Input Monitoring, Microphone and
    /// Speech Recognition only).
    NotDetermined,
    /// Status can't be determined: unsupported platform, or the OS returned
    /// an unexpected value (e.g. an undocumented `IOHIDAccessType`).
    Unknown,
}

impl PermissionStatus {
    pub fn is_granted(self) -> bool {
        self == PermissionStatus::Granted
    }
}

/// Snapshot of the status of every [`Permission`] recording needs
/// ([`Permission::ALL`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionReport {
    pub screen_recording: PermissionStatus,
    pub accessibility: PermissionStatus,
    pub input_monitoring: PermissionStatus,
}

impl PermissionReport {
    /// The status of `permission`; checked live for the narration ones,
    /// which the report does not hold.
    pub fn get(&self, permission: Permission) -> PermissionStatus {
        match permission {
            Permission::ScreenRecording => self.screen_recording,
            Permission::Accessibility => self.accessibility,
            Permission::InputMonitoring => self.input_monitoring,
            Permission::Microphone | Permission::SpeechRecognition => check(permission),
        }
    }

    /// Permissions not currently granted, in [`Permission::ALL`] order.
    pub fn missing(&self) -> Vec<Permission> {
        Permission::ALL
            .into_iter()
            .filter(|p| !self.get(*p).is_granted())
            .collect()
    }

    pub fn all_granted(&self) -> bool {
        self.missing().is_empty()
    }
}

/// System Settings deep link for the pane that controls `permission`. On
/// Windows, the Settings page: Privacy & security → Microphone, Time &
/// language → Speech (where a speech language is added), and Privacy &
/// security for the rest, which Windows never asks about.
pub fn settings_url(permission: Permission) -> &'static str {
    if cfg!(windows) {
        return match permission {
            Permission::Microphone => "ms-settings:privacy-microphone",
            Permission::SpeechRecognition => "ms-settings:speech",
            Permission::ScreenRecording
            | Permission::Accessibility
            | Permission::InputMonitoring => "ms-settings:privacy",
        };
    }
    match permission {
        Permission::ScreenRecording => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        }
        Permission::Accessibility => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        }
        Permission::InputMonitoring => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent"
        }
        Permission::Microphone => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
        }
        Permission::SpeechRecognition => {
            "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition"
        }
    }
}

/// Current status of `permission`. Never prompts.
pub fn check(permission: Permission) -> PermissionStatus {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions as sys;
        match permission {
            Permission::ScreenRecording => sys::check_screen_recording(),
            Permission::Accessibility => sys::check_accessibility(),
            Permission::InputMonitoring => sys::check_input_monitoring(),
            Permission::Microphone => sys::check_microphone(),
            Permission::SpeechRecognition => sys::check_speech_recognition(),
        }
    }
    #[cfg(windows)]
    {
        use crate::windows::permissions as sys;
        match permission {
            Permission::Microphone => sys::check_microphone(),
            Permission::SpeechRecognition => sys::check_speech_recognition(),
            Permission::ScreenRecording
            | Permission::Accessibility
            | Permission::InputMonitoring => PermissionStatus::Granted,
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = permission;
        PermissionStatus::Unknown
    }
}

/// Screen Recording was allowed in System Settings after the app started:
/// [`check`] still says no (macOS keeps that answer until the app
/// restarts) and recording needs a restart to use it. Never prompts.
pub fn screen_recording_needs_restart() -> bool {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions as sys;
        !sys::check_screen_recording().is_granted() && sys::screen_recording_allowed_live()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Status of every permission. Never prompts.
pub fn check_all() -> PermissionReport {
    PermissionReport {
        screen_recording: check(Permission::ScreenRecording),
        accessibility: check(Permission::Accessibility),
        input_monitoring: check(Permission::InputMonitoring),
    }
}

/// Ask the OS to prompt for `permission` and return the resulting status.
///
/// May show a system dialog; call only from an explicit user action. A grant
/// made in System Settings often only takes effect after an app restart
/// (notably Screen Recording), so callers should re-[`check`] later.
/// Windows has no prompt for a desktop app: this only checks.
pub fn request(permission: Permission) -> PermissionStatus {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions as sys;
        match permission {
            Permission::ScreenRecording => sys::request_screen_recording(),
            Permission::Accessibility => sys::request_accessibility(),
            Permission::InputMonitoring => sys::request_input_monitoring(),
            Permission::Microphone => sys::request_microphone(),
            Permission::SpeechRecognition => sys::request_speech_recognition(),
        }
    }
    #[cfg(windows)]
    {
        check(permission)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = permission;
        PermissionStatus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_restart_check_never_prompts_or_fails() {
        // Live, without TCC prompts: an answer either way, no crash.
        let _ = screen_recording_needs_restart();
    }

    #[test]
    fn settings_url_covers_every_permission() {
        let mut seen = std::collections::HashSet::new();
        for p in Permission::ALL.into_iter().chain(Permission::NARRATION) {
            let url = settings_url(p);
            if cfg!(windows) {
                assert!(url.starts_with("ms-settings:"), "{url}");
                // Recording's all open Privacy & security.
                if Permission::ALL.contains(&p) {
                    continue;
                }
            } else {
                assert!(url.starts_with("x-apple.systempreferences:"), "{url}");
                assert!(url.contains("?Privacy_"), "{url}");
            }
            assert!(seen.insert(url), "duplicate url for {p:?}");
        }
        if cfg!(windows) {
            assert_eq!(
                settings_url(Permission::Microphone),
                "ms-settings:privacy-microphone"
            );
            assert_eq!(
                settings_url(Permission::SpeechRecognition),
                "ms-settings:speech"
            );
        }
    }

    #[test]
    fn check_runs_without_panicking() {
        let report = check_all();
        for p in Permission::ALL {
            assert_eq!(report.get(p), check(p));
        }
        #[cfg(windows)]
        assert!(report.all_granted());
        #[cfg(not(any(target_os = "macos", windows)))]
        assert!(Permission::ALL
            .into_iter()
            .all(|p| check(p) == PermissionStatus::Unknown));
        // Narration is never part of what recording needs.
        for p in Permission::NARRATION {
            assert!(!Permission::ALL.contains(&p));
            assert_eq!(report.get(p), check(p));
            // Windows reads its settings: an answer either way.
            #[cfg(windows)]
            assert_ne!(check(p), PermissionStatus::Unknown);
            #[cfg(windows)]
            assert_eq!(request(p), check(p));
            #[cfg(not(any(target_os = "macos", windows)))]
            assert_eq!(check(p), PermissionStatus::Unknown);
        }
    }

    #[test]
    fn report_missing_and_all_granted() {
        let report = PermissionReport {
            screen_recording: PermissionStatus::Granted,
            accessibility: PermissionStatus::Denied,
            input_monitoring: PermissionStatus::NotDetermined,
        };
        assert_eq!(
            report.missing(),
            vec![Permission::Accessibility, Permission::InputMonitoring]
        );
        assert!(!report.all_granted());
    }

    #[test]
    fn serializes_snake_case() {
        let json = serde_json::to_string(&Permission::InputMonitoring).unwrap();
        assert_eq!(json, "\"input_monitoring\"");
        let json = serde_json::to_string(&Permission::SpeechRecognition).unwrap();
        assert_eq!(json, "\"speech_recognition\"");
        let json = serde_json::to_string(&PermissionStatus::NotDetermined).unwrap();
        assert_eq!(json, "\"not_determined\"");
    }
}
