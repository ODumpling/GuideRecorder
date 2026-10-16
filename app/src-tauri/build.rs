//! Builds the app with an app manifest, so each window can invoke only the
//! commands its capability file allows (`capabilities/*.json`).
//!
//! Tauri generates an `allow-<command>` permission (underscores become
//! dashes) for every command in [`COMMANDS`]. A command missing from the
//! manifest is rejected at runtime in every window, which no test that mocks
//! the IPC would notice, so the build fails instead when:
//! - [`COMMANDS`] differs from the `generate_handler!` list in `src/lib.rs`;
//! - `capabilities/main.json` does not apply to `main` or does not allow
//!   every command;
//! - any capability that applies to `main` denies a command.

use std::collections::BTreeSet;
use std::fs;

use serde_json::Value;

/// Every command registered with `generate_handler!` in `src/lib.rs`.
const COMMANDS: &[&str] = &[
    "app_info",
    "library_root",
    "list_guides",
    "open_guide",
    "save_guide",
    "close_guide",
    "step_image",
    "add_image_step",
    "copy_step_images",
    "read_image_file",
    "choose_export_folder",
    "choose_export_file",
    "choose_logo",
    "export_guide",
    "reveal_export",
    "verify_crop",
    "get_settings",
    "update_settings",
    "library_info",
    "choose_library_folder",
    "switch_library",
    "set_launch_at_login",
    "launch_at_login",
    "api_key_status",
    "set_api_key",
    "delete_api_key",
    "permission_status",
    "open_permission_settings",
    "request_permission",
    "recording_readiness",
    "mcp_setup",
    "start_recording",
    "pause_recording",
    "resume_recording",
    "stop_recording",
    "cancel_recording",
    "recording_status",
    "recording_snapshot",
    "add_manual_step",
    "list_capture_windows",
    "pick_region",
    "finish_region_pick",
    "ai_generate",
    "ai_cancel",
    "ai_rewrite_step",
    "quit_ack",
    "quit_app",
    "quit_cancel",
];

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    println!("cargo:rerun-if-changed=capabilities");
    check_commands();
    check_main_capabilities();
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run tauri-build");
}

/// The command names in `generate_handler![...]`, without module paths.
fn registered_commands() -> BTreeSet<String> {
    const MACRO: &str = "generate_handler![";
    let lib = strip_comments(&fs::read_to_string("src/lib.rs").expect("read src/lib.rs"));
    let found = lib.matches(MACRO).count();
    assert!(
        found == 1,
        "src/lib.rs must have exactly one `{MACRO}...]` outside comments (found {found}); \
         build.rs reads the command list from it"
    );
    let start = lib.find(MACRO).unwrap() + MACRO.len();
    let end = start + lib[start..].find(']').expect("unclosed generate_handler!");
    lib[start..end]
        .split(',')
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(|path| path.rsplit("::").next().unwrap_or(path).to_string())
        .collect()
}

/// Rust source with `//` and (nested) `/* */` comments removed. String,
/// raw string and char literals are kept as they are, so `"tauri://"` is
/// not mistaken for a comment.
fn strip_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        let (c, next) = (chars[i], chars.get(i + 1).copied());
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && chars.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            out.push(' ');
        } else if c == 'r' && matches!(next, Some('"' | '#')) {
            // Raw string: r"..." or r#"..."#; otherwise an identifier.
            let hashes = chars[i + 1..].iter().take_while(|&&h| h == '#').count();
            if chars.get(i + 1 + hashes) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            let body = i + 2 + hashes;
            let close: String = std::iter::once('"')
                .chain("#".repeat(hashes).chars())
                .collect();
            let rest: String = chars[body..].iter().collect();
            let len = rest.find(&close).map_or(rest.len(), |at| at + close.len());
            let end = body + rest[..len].chars().count();
            out.extend(&chars[i..end]);
            i = end;
        } else if c == '"' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += if chars[i] == '\\' { 2 } else { 1 };
            }
            i = (i + 1).min(chars.len());
            out.extend(&chars[start..i]);
        } else if c == '\'' {
            // A char literal ('x', '\n', '"'); a lifetime has no closing quote.
            let len = match next {
                Some('\\') => chars[i + 2..]
                    .iter()
                    .position(|&q| q == '\'')
                    .map(|at| at + 3),
                Some(_) if chars.get(i + 2) == Some(&'\'') => Some(3),
                _ => None,
            };
            let end = (i + len.unwrap_or(1)).min(chars.len());
            out.extend(&chars[i..end]);
            i = end;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn check_commands() {
    let registered = registered_commands();
    let manifest: BTreeSet<String> = COMMANDS.iter().map(|c| c.to_string()).collect();
    assert_eq!(
        manifest.len(),
        COMMANDS.len(),
        "build.rs COMMANDS has a duplicate"
    );
    let missing: Vec<_> = registered.difference(&manifest).collect();
    let extra: Vec<_> = manifest.difference(&registered).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "build.rs COMMANDS must match generate_handler! in src/lib.rs \
         (missing: {missing:?}, not registered: {extra:?})"
    );
}

/// A capability file's `windows` and permission identifiers. A permission
/// is a string or an object with an `identifier` (one with scopes).
fn read_capability(path: &str) -> (Vec<String>, BTreeSet<String>) {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let json: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"));
    let strings = |key: &str, field: Option<&str>| -> Vec<String> {
        json[key]
            .as_array()
            .unwrap_or_else(|| panic!("{path}: `{key}` must be an array"))
            .iter()
            .map(|entry| {
                let value = match (entry, field) {
                    (Value::Object(object), Some(field)) => object.get(field),
                    _ => Some(entry),
                };
                value
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("{path}: unexpected `{key}` entry {entry}"))
                    .to_string()
            })
            .collect()
    };
    (
        strings("windows", None),
        strings("permissions", Some("identifier"))
            .into_iter()
            .collect(),
    )
}

/// Whether a capability `windows` entry may match `label`. Tauri reads it as
/// a `glob::Pattern`; this handles `*` and `?` (so also `**`) and, to stay
/// conservative, treats any pattern with other glob syntax (`[...]`, or
/// `{`, `}` and `\` that other glob dialects use) as matching.
fn window_matches(pattern: &str, label: &str) -> bool {
    if pattern.contains(['[', ']', '{', '}', '\\']) {
        return true;
    }
    fn matches(pattern: &[u8], label: &[u8]) -> bool {
        match (pattern.split_first(), label.split_first()) {
            (None, None) => true,
            (Some((b'*', rest)), _) => {
                matches(rest, label) || (!label.is_empty() && matches(pattern, &label[1..]))
            }
            (Some((b'?', rest)), Some((_, label))) => matches(rest, label),
            (Some((p, rest)), Some((l, label))) if p == l => matches(rest, label),
            _ => false,
        }
    }
    matches(pattern.as_bytes(), label.as_bytes())
}

fn check_main_capabilities() {
    let slug = |command: &str| command.replace('_', "-");

    let (windows, granted) = read_capability("capabilities/main.json");
    assert!(
        windows.iter().any(|w| w == "main"),
        "capabilities/main.json must list the window \"main\" (windows: {windows:?})"
    );
    let missing: Vec<String> = COMMANDS
        .iter()
        .map(|c| format!("allow-{}", slug(c)))
        .filter(|p| !granted.contains(p))
        .collect();
    assert!(
        missing.is_empty(),
        "capabilities/main.json must allow every command; missing {missing:?}"
    );

    let mut files: Vec<_> = fs::read_dir("capabilities")
        .expect("read capabilities/")
        .map(|entry| entry.expect("read capabilities/").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    for path in files {
        let path = path.to_string_lossy().into_owned();
        let (windows, permissions) = read_capability(&path);
        if !windows.iter().any(|w| window_matches(w, "main")) {
            continue;
        }
        let denied: Vec<String> = COMMANDS
            .iter()
            .map(|c| format!("deny-{}", slug(c)))
            .filter(|p| permissions.contains(p))
            .collect();
        assert!(
            denied.is_empty(),
            "{path} applies to the main window and denies {denied:?}"
        );
    }
}
