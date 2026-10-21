//! The platform's virtual key codes: what [`Key::code`] and
//! [`Hotkey::code`] hold. macOS uses `kVK_*` codes (Carbon `Events.h`);
//! every other platform uses Windows virtual-key codes (`VK_*`), which the
//! Windows backend's keyboard hook reports.
//!
//! [`Key::code`]: guiderecorder_core::events::Key::code
//! [`Hotkey::code`]: crate::session::Hotkey::code

/// The `S` key.
#[cfg(target_os = "macos")]
pub const S: u16 = 1;
/// The `Z` key.
#[cfg(target_os = "macos")]
pub const Z: u16 = 6;
/// Return.
#[cfg(target_os = "macos")]
pub const RETURN: u16 = 36;
/// The keypad's Enter.
#[cfg(target_os = "macos")]
pub const KEYPAD_ENTER: u16 = 76;
/// Tab.
#[cfg(target_os = "macos")]
pub const TAB: u16 = 48;
/// Escape.
#[cfg(target_os = "macos")]
pub const ESCAPE: u16 = 53;

/// The `S` key.
#[cfg(not(target_os = "macos"))]
pub const S: u16 = 0x53;
/// The `Z` key.
#[cfg(not(target_os = "macos"))]
pub const Z: u16 = 0x5A;
/// Return (`VK_RETURN`).
#[cfg(not(target_os = "macos"))]
pub const RETURN: u16 = 0x0D;
/// The keypad's Enter: the same code as Return.
#[cfg(not(target_os = "macos"))]
pub const KEYPAD_ENTER: u16 = RETURN;
/// Tab (`VK_TAB`).
#[cfg(not(target_os = "macos"))]
pub const TAB: u16 = 0x09;
/// Escape (`VK_ESCAPE`).
#[cfg(not(target_os = "macos"))]
pub const ESCAPE: u16 = 0x1B;

/// The code of every key a shortcut can use, by the name the settings store
/// it under: the ANSI letters and digits, `F1` to `F20`, and the named keys.
#[cfg(target_os = "macos")]
pub const NAMED: &[(&str, u16)] = &[
    ("A", 0),
    ("S", 1),
    ("D", 2),
    ("F", 3),
    ("H", 4),
    ("G", 5),
    ("Z", 6),
    ("X", 7),
    ("C", 8),
    ("V", 9),
    ("B", 11),
    ("Q", 12),
    ("W", 13),
    ("E", 14),
    ("R", 15),
    ("Y", 16),
    ("T", 17),
    ("1", 18),
    ("2", 19),
    ("3", 20),
    ("4", 21),
    ("6", 22),
    ("5", 23),
    ("9", 25),
    ("7", 26),
    ("8", 28),
    ("0", 29),
    ("O", 31),
    ("U", 32),
    ("I", 34),
    ("P", 35),
    ("L", 37),
    ("J", 38),
    ("K", 40),
    ("N", 45),
    ("M", 46),
    ("Enter", 36),
    ("Tab", 48),
    ("Space", 49),
    ("Backspace", 51),
    ("Escape", 53),
    ("F17", 64),
    ("F18", 79),
    ("F19", 80),
    ("F20", 90),
    ("F5", 96),
    ("F6", 97),
    ("F7", 98),
    ("F3", 99),
    ("F8", 100),
    ("F9", 101),
    ("F11", 103),
    ("F13", 105),
    ("F16", 106),
    ("F14", 107),
    ("F10", 109),
    ("F12", 111),
    ("F15", 113),
    ("Delete", 117),
    ("F4", 118),
    ("F2", 120),
    ("F1", 122),
];

/// The code of every key a shortcut can use, by the name the settings store
/// it under: the ANSI letters and digits, `F1` to `F20`, and the named keys.
#[cfg(not(target_os = "macos"))]
pub const NAMED: &[(&str, u16)] = &[
    ("A", 0x41),
    ("B", 0x42),
    ("C", 0x43),
    ("D", 0x44),
    ("E", 0x45),
    ("F", 0x46),
    ("G", 0x47),
    ("H", 0x48),
    ("I", 0x49),
    ("J", 0x4A),
    ("K", 0x4B),
    ("L", 0x4C),
    ("M", 0x4D),
    ("N", 0x4E),
    ("O", 0x4F),
    ("P", 0x50),
    ("Q", 0x51),
    ("R", 0x52),
    ("S", 0x53),
    ("T", 0x54),
    ("U", 0x55),
    ("V", 0x56),
    ("W", 0x57),
    ("X", 0x58),
    ("Y", 0x59),
    ("Z", 0x5A),
    ("0", 0x30),
    ("1", 0x31),
    ("2", 0x32),
    ("3", 0x33),
    ("4", 0x34),
    ("5", 0x35),
    ("6", 0x36),
    ("7", 0x37),
    ("8", 0x38),
    ("9", 0x39),
    ("F1", 0x70),
    ("F2", 0x71),
    ("F3", 0x72),
    ("F4", 0x73),
    ("F5", 0x74),
    ("F6", 0x75),
    ("F7", 0x76),
    ("F8", 0x77),
    ("F9", 0x78),
    ("F10", 0x79),
    ("F11", 0x7A),
    ("F12", 0x7B),
    ("F13", 0x7C),
    ("F14", 0x7D),
    ("F15", 0x7E),
    ("F16", 0x7F),
    ("F17", 0x80),
    ("F18", 0x81),
    ("F19", 0x82),
    ("F20", 0x83),
    ("Enter", 0x0D),
    ("Tab", 0x09),
    ("Space", 0x20),
    ("Backspace", 0x08),
    ("Escape", 0x1B),
    ("Delete", 0x2E),
];

/// The code of the key named `name` in [`NAMED`], ignoring ASCII case.
pub fn code(name: &str) -> Option<u16> {
    NAMED
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|&(_, code)| code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_keys_are_distinct() {
        // Letters, digits, F1-F20 and six named keys.
        assert_eq!(NAMED.len(), 26 + 10 + 20 + 6);
        for (i, (name, code)) in NAMED.iter().enumerate() {
            assert!(
                NAMED[i + 1..].iter().all(|(n, c)| n != name && c != code),
                "{name}"
            );
        }
        assert_eq!(code("s"), Some(S));
        assert_eq!(code("Z"), Some(Z));
        assert_eq!(code("F21"), None);
    }
}
