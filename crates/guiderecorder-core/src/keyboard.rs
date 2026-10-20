//! Keyboard conventions of the platform a trace was recorded on: which
//! virtual key codes [`Key::code`] holds (macOS `kVK_*` codes, Windows
//! `VK_*` codes) and how a shortcut is written ("⌘S" on a Mac, "Ctrl+S"
//! on Windows).
//!
//! Step detection reads keys through a [`KeyPlatform`]
//! ([`StepConfig::keys`]), by default the one it runs on, the same choice
//! `guiderecorder-capture`'s `keys` makes for the codes it records.
//!
//! [`StepConfig::keys`]: crate::steps::StepConfig::keys

use crate::events::{Key, Modifiers};

/// The platform whose key codes and shortcut notation a trace uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyPlatform {
    /// `kVK_*` codes (Carbon `Events.h`); "⌃⌥⇧⌘S".
    Mac,
    /// Windows virtual-key codes; "Win+Ctrl+Alt+Shift+S".
    Windows,
}

/// A key step detection treats specially.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    /// Return, or the keypad's Enter.
    Return,
    Tab,
    Space,
    /// Delete backwards (Backspace; ⌫ on a Mac).
    Backspace,
    Escape,
    /// Delete forwards (⌦ on a Mac).
    ForwardDelete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    /// F1 and up.
    Function(u8),
}

impl Default for KeyPlatform {
    fn default() -> Self {
        Self::CURRENT
    }
}

impl KeyPlatform {
    /// This platform: [`KeyPlatform::Mac`] on macOS, [`KeyPlatform::Windows`]
    /// everywhere else.
    pub const CURRENT: Self = if cfg!(target_os = "macos") {
        Self::Mac
    } else {
        Self::Windows
    };

    /// The special key `code` stands for, if any.
    pub fn named(self, code: u16) -> Option<NamedKey> {
        match self {
            Self::Mac => mac_named(code),
            Self::Windows => windows_named(code),
        }
    }

    /// What Return is called in a title: "Return" on a Mac, "Enter" on
    /// Windows.
    pub fn return_name(self) -> &'static str {
        match self {
            Self::Mac => "Return",
            Self::Windows => "Enter",
        }
    }

    /// The held modifiers before a click's verb: "⌘-" on a Mac, "Ctrl+"
    /// on Windows, or nothing.
    pub fn click_prefix(self, modifiers: Modifiers) -> String {
        let held = self.modifiers(modifiers);
        match self {
            _ if held.is_empty() => held,
            Self::Mac => format!("{held}-"),
            Self::Windows => format!("{held}+"),
        }
    }

    /// Whether control turns a left click into a right click (macOS).
    pub fn control_click_is_right_click(self) -> bool {
        self == Self::Mac
    }

    /// `⌘S`, `⌃⌥⇧⌘↩` (macOS menu order) or `Ctrl+S`, `Win+Shift+S`.
    pub fn shortcut_label(self, key: &Key, modifiers: Modifiers) -> String {
        let name = match self.named(key.code) {
            Some(named) => self.key_name(named),
            None => key
                .characters_ignoring_modifiers
                .as_deref()
                .or(key.characters.as_deref())
                .filter(|s| is_printable(s))
                .map(str::to_uppercase)
                .unwrap_or_else(|| format!("Key {}", key.code)),
        };
        match self {
            Self::Mac => format!("{}{name}", self.modifiers(modifiers)),
            Self::Windows => {
                let held = self.modifiers(modifiers);
                if held.is_empty() {
                    name
                } else {
                    format!("{held}+{name}")
                }
            }
        }
    }

    /// A special key's name in a shortcut label.
    pub fn key_name(self, key: NamedKey) -> String {
        let name = match (self, key) {
            (_, NamedKey::Function(n)) => return format!("F{n}"),
            (Self::Mac, NamedKey::Return) => "↩",
            (Self::Mac, NamedKey::Tab) => "⇥",
            (Self::Mac, NamedKey::Backspace) => "⌫",
            (Self::Mac, NamedKey::Escape) => "⎋",
            (Self::Mac, NamedKey::ForwardDelete) => "⌦",
            (Self::Windows, NamedKey::Return) => "Enter",
            (Self::Windows, NamedKey::Tab) => "Tab",
            (Self::Windows, NamedKey::Backspace) => "Backspace",
            (Self::Windows, NamedKey::Escape) => "Esc",
            (Self::Windows, NamedKey::ForwardDelete) => "Delete",
            (_, NamedKey::Space) => "Space",
            (_, NamedKey::Left) => "←",
            (_, NamedKey::Right) => "→",
            (_, NamedKey::Up) => "↑",
            (_, NamedKey::Down) => "↓",
            (_, NamedKey::Home) => "Home",
            (_, NamedKey::End) => "End",
            (_, NamedKey::PageUp) => "Page Up",
            (_, NamedKey::PageDown) => "Page Down",
        };
        name.to_owned()
    }

    /// The held modifiers: "⌃⌥⇧⌘" on a Mac, "Win+Ctrl+Alt+Shift" (Windows
    /// logo key, Control, Alt, Shift) on Windows.
    fn modifiers(self, m: Modifiers) -> String {
        match self {
            Self::Mac => [
                (m.control, "⌃"),
                (m.option, "⌥"),
                (m.shift, "⇧"),
                (m.command, "⌘"),
            ]
            .into_iter()
            .filter(|(held, _)| *held)
            .map(|(_, symbol)| symbol)
            .collect(),
            Self::Windows => [
                (m.command, "Win"),
                (m.control, "Ctrl"),
                (m.option, "Alt"),
                (m.shift, "Shift"),
            ]
            .into_iter()
            .filter(|(held, _)| *held)
            .map(|(_, name)| name)
            .collect::<Vec<_>>()
            .join("+"),
        }
    }
}

fn mac_named(code: u16) -> Option<NamedKey> {
    Some(match code {
        // Return and the keypad's Enter.
        36 | 76 => NamedKey::Return,
        48 => NamedKey::Tab,
        49 => NamedKey::Space,
        51 => NamedKey::Backspace,
        53 => NamedKey::Escape,
        117 => NamedKey::ForwardDelete,
        123 => NamedKey::Left,
        124 => NamedKey::Right,
        125 => NamedKey::Down,
        126 => NamedKey::Up,
        115 => NamedKey::Home,
        119 => NamedKey::End,
        116 => NamedKey::PageUp,
        121 => NamedKey::PageDown,
        122 => NamedKey::Function(1),
        120 => NamedKey::Function(2),
        99 => NamedKey::Function(3),
        118 => NamedKey::Function(4),
        96 => NamedKey::Function(5),
        97 => NamedKey::Function(6),
        98 => NamedKey::Function(7),
        100 => NamedKey::Function(8),
        101 => NamedKey::Function(9),
        109 => NamedKey::Function(10),
        103 => NamedKey::Function(11),
        111 => NamedKey::Function(12),
        _ => return None,
    })
}

fn windows_named(code: u16) -> Option<NamedKey> {
    Some(match code {
        // VK_RETURN: the keypad's Enter has the same code.
        0x0D => NamedKey::Return,
        0x09 => NamedKey::Tab,
        0x20 => NamedKey::Space,
        0x08 => NamedKey::Backspace,
        0x1B => NamedKey::Escape,
        0x2E => NamedKey::ForwardDelete,
        0x25 => NamedKey::Left,
        0x27 => NamedKey::Right,
        0x26 => NamedKey::Up,
        0x28 => NamedKey::Down,
        0x24 => NamedKey::Home,
        0x23 => NamedKey::End,
        0x21 => NamedKey::PageUp,
        0x22 => NamedKey::PageDown,
        // VK_F1 to VK_F24.
        0x70..=0x87 => NamedKey::Function((code - 0x70 + 1) as u8),
        _ => return None,
    })
}

/// Text a key inserts: no control characters and no AppKit function-key
/// characters (U+F700..U+F8FF).
pub(crate) fn is_printable(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_printable_char)
}

pub(crate) fn is_printable_char(c: char) -> bool {
    !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u16, chars: &str) -> Key {
        Key {
            code,
            characters: Some(chars.into()),
            characters_ignoring_modifiers: Some(chars.to_lowercase()),
        }
    }

    fn mods(command: bool, control: bool, option: bool, shift: bool) -> Modifiers {
        Modifiers {
            command,
            control,
            option,
            shift,
            ..Modifiers::default()
        }
    }

    #[test]
    fn mac_labels() {
        let mac = KeyPlatform::Mac;
        assert_eq!(
            mac.shortcut_label(&key(1, "s"), mods(true, false, false, false)),
            "⌘S"
        );
        assert_eq!(
            mac.shortcut_label(&key(36, "\r"), mods(true, true, true, true)),
            "⌃⌥⇧⌘↩"
        );
        assert_eq!(
            mac.shortcut_label(&key(96, ""), mods(true, false, false, false)),
            "⌘F5"
        );
        assert_eq!(mac.click_prefix(mods(true, false, false, true)), "⇧⌘-");
        assert_eq!(mac.return_name(), "Return");
    }

    #[test]
    fn windows_labels() {
        let win = KeyPlatform::Windows;
        assert_eq!(
            win.shortcut_label(&key(0x53, "s"), mods(false, true, false, false)),
            "Ctrl+S"
        );
        assert_eq!(
            win.shortcut_label(&key(0x53, "s"), mods(true, false, false, true)),
            "Win+Shift+S"
        );
        assert_eq!(
            win.shortcut_label(&key(0x2E, ""), mods(false, true, true, false)),
            "Ctrl+Alt+Delete"
        );
        assert_eq!(
            win.shortcut_label(&key(0x73, ""), mods(false, false, true, false)),
            "Alt+F4"
        );
        assert_eq!(
            win.shortcut_label(&key(0x87, ""), mods(false, true, false, false)),
            "Ctrl+F24"
        );
        assert_eq!(
            win.shortcut_label(&key(0x0D, "\r"), Modifiers::default()),
            "Enter"
        );
        assert_eq!(
            win.click_prefix(mods(false, true, false, true)),
            "Ctrl+Shift+"
        );
        assert_eq!(win.click_prefix(Modifiers::default()), "");
        assert_eq!(win.return_name(), "Enter");
        assert!(!win.control_click_is_right_click());
    }

    #[test]
    fn unknown_keys_fall_back_to_their_code() {
        let unnamed = Key {
            code: 0xFF,
            characters: None,
            characters_ignoring_modifiers: None,
        };
        assert_eq!(
            KeyPlatform::Windows.shortcut_label(&unnamed, mods(false, true, false, false)),
            "Ctrl+Key 255"
        );
    }

    #[test]
    fn the_same_code_means_different_keys() {
        // 36 is Return on a Mac and VK_HOME on Windows.
        assert_eq!(KeyPlatform::Mac.named(36), Some(NamedKey::Return));
        assert_eq!(KeyPlatform::Windows.named(0x24), Some(NamedKey::Home));
        assert_eq!(KeyPlatform::Windows.named(0x0D), Some(NamedKey::Return));
        assert_eq!(KeyPlatform::Mac.named(0x0D), None);
    }
}
