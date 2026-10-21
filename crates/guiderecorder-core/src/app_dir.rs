//! The folder the desktop app keeps its own files in (settings, sync
//! state, the search index), which the CLI and the MCP server read too.
//!
//! - macOS: `~/Library/Application Support/dev.dufflebear.guiderecorder`
//!   (from `$HOME`).
//! - Windows: `%LOCALAPPDATA%\dev.dufflebear.guiderecorder`. Local rather
//!   than roaming: the files name folders on this PC (the library) and the
//!   index is a database, neither of which should follow the user to
//!   another one.
//!
//! Elsewhere there is none: the app does not run there.

use std::ffi::OsString;
use std::path::PathBuf;

/// The app's identifier, its folder's name.
pub const APP_ID: &str = "dev.dufflebear.guiderecorder";

/// The app's folder on this platform, from the environment; `None` on
/// other platforms or when the variable is unset or empty.
pub fn app_data_dir() -> Option<PathBuf> {
    app_data_dir_from(|name| std::env::var_os(name))
}

/// [`app_data_dir`] with the environment read through `var`.
fn app_data_dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let var = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        Some(
            var("HOME")?
                .join("Library/Application Support")
                .join(APP_ID),
        )
    } else if cfg!(windows) {
        Some(var("LOCALAPPDATA")?.join(APP_ID))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_platform_folder() {
        let env = |name: &str| match name {
            "HOME" => Some(OsString::from("/Users/me")),
            "LOCALAPPDATA" => Some(OsString::from(r"C:\Users\me\AppData\Local")),
            _ => None,
        };
        let dir = app_data_dir_from(env);
        if cfg!(target_os = "macos") {
            assert_eq!(
                dir,
                Some(PathBuf::from(
                    "/Users/me/Library/Application Support/dev.dufflebear.guiderecorder"
                ))
            );
        } else if cfg!(windows) {
            assert_eq!(
                dir,
                Some(PathBuf::from(
                    r"C:\Users\me\AppData\Local\dev.dufflebear.guiderecorder"
                ))
            );
        } else {
            assert_eq!(dir, None);
        }
    }

    #[test]
    fn none_without_the_variable() {
        assert_eq!(app_data_dir_from(|_| None), None);
        assert_eq!(app_data_dir_from(|_| Some(OsString::new())), None);
    }
}
