//! OS permissions required for recording (design #440, "Permissions (Q33)").
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

/// A TCC permission GuidesYa needs to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    ScreenRecording,
    Accessibility,
    InputMonitoring,
}

impl Permission {
    pub const ALL: [Permission; 3] = [
        Permission::ScreenRecording,
        Permission::Accessibility,
        Permission::InputMonitoring,
    ];
}

/// Current grant state of a [`Permission`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionStatus {
    Granted,
    /// Not granted. For Screen Recording and Accessibility the OS cannot
    /// distinguish "denied" from "never asked", so both report `Denied`.
    Denied,
    /// The user has not been asked yet (Input Monitoring only).
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

/// Snapshot of every [`Permission`]'s status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionReport {
    pub screen_recording: PermissionStatus,
    pub accessibility: PermissionStatus,
    pub input_monitoring: PermissionStatus,
}

impl PermissionReport {
    pub fn get(&self, permission: Permission) -> PermissionStatus {
        match permission {
            Permission::ScreenRecording => self.screen_recording,
            Permission::Accessibility => self.accessibility,
            Permission::InputMonitoring => self.input_monitoring,
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

/// System Settings deep link for the pane that controls `permission`.
pub fn settings_url(permission: Permission) -> &'static str {
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
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission;
        PermissionStatus::Unknown
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
pub fn request(permission: Permission) -> PermissionStatus {
    #[cfg(target_os = "macos")]
    {
        use crate::macos::permissions as sys;
        match permission {
            Permission::ScreenRecording => sys::request_screen_recording(),
            Permission::Accessibility => sys::request_accessibility(),
            Permission::InputMonitoring => sys::request_input_monitoring(),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = permission;
        PermissionStatus::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_url_covers_every_permission() {
        let mut seen = std::collections::HashSet::new();
        for p in Permission::ALL {
            let url = settings_url(p);
            assert!(url.starts_with("x-apple.systempreferences:"), "{url}");
            assert!(url.contains("?Privacy_"), "{url}");
            assert!(seen.insert(url), "duplicate url for {p:?}");
        }
    }

    #[test]
    fn check_runs_without_panicking() {
        let report = check_all();
        for p in Permission::ALL {
            assert_eq!(report.get(p), check(p));
        }
        #[cfg(not(target_os = "macos"))]
        assert!(Permission::ALL
            .into_iter()
            .all(|p| check(p) == PermissionStatus::Unknown));
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
        let json = serde_json::to_string(&PermissionStatus::NotDetermined).unwrap();
        assert_eq!(json, "\"not_determined\"");
    }
}
