//! What narration needs on Windows (see [`crate::permissions`]).
//!
//! - Microphone: Windows' privacy settings, which a desktop app can read
//!   but not prompt for. Settings → Privacy & security → Microphone keeps
//!   its switches under `CapabilityAccessManager\ConsentStore\microphone`:
//!   `Value` in HKLM (the device's "Microphone access", set by an
//!   administrator), the same in HKCU (the user's), and HKCU's
//!   `NonPackaged` subkey ("Let desktop apps access your microphone").
//!   Any of them `Deny` refuses; otherwise (`Allow`, or never changed)
//!   the microphone is allowed.
//! - Speech recognition: granted when a desktop dictation recogniser is
//!   installed (a language's speech pack), denied otherwise, as there is
//!   nothing to allow and nothing to recognise with until one is.

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

use super::narration::sapi;
use crate::permissions::PermissionStatus;

const STORE: &str =
    "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone";

pub fn check_microphone() -> PermissionStatus {
    let desktop = format!("{STORE}\\NonPackaged");
    consent([
        sapi::string_value(HKEY_LOCAL_MACHINE, STORE, "Value"),
        sapi::string_value(HKEY_CURRENT_USER, STORE, "Value"),
        sapi::string_value(HKEY_CURRENT_USER, &desktop, "Value"),
    ])
}

/// The microphone's status from its switches: any one `Deny` refuses.
fn consent(switches: [Option<String>; 3]) -> PermissionStatus {
    if switches
        .iter()
        .flatten()
        .any(|v| v.trim().eq_ignore_ascii_case("deny"))
    {
        PermissionStatus::Denied
    } else {
        PermissionStatus::Granted
    }
}

pub fn check_speech_recognition() -> PermissionStatus {
    if sapi::recognizers().is_empty() {
        PermissionStatus::Denied
    } else {
        PermissionStatus::Granted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_switch_off_refuses_the_microphone() {
        let on = || Some("Allow".to_string());
        let off = || Some("Deny".to_string());
        assert_eq!(consent([on(), on(), on()]), PermissionStatus::Granted);
        assert_eq!(consent([None, None, None]), PermissionStatus::Granted);
        assert_eq!(consent([off(), on(), on()]), PermissionStatus::Denied);
        assert_eq!(consent([None, off(), None]), PermissionStatus::Denied);
        // "Let desktop apps access your microphone" off.
        assert_eq!(consent([on(), on(), off()]), PermissionStatus::Denied);
    }

    #[test]
    fn the_live_checks_answer() {
        // Whatever this machine's settings are: an answer, never unknown.
        assert_ne!(check_microphone(), PermissionStatus::Unknown);
        assert_eq!(
            check_speech_recognition().is_granted(),
            !sapi::recognizers().is_empty()
        );
    }
}
