//! Runs the built `guidesya` binary against a temp copy of
//! `fixtures/guides/`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const ROTATE: &str = "Rotate an API key.guidesya";
const CALENDAR: &str = "Share a calendar event.guidesya";
const DRUCKER: &str = "Kurzanleitung Drucker.guidesya";

/// A temp copy of the fixture library, with an index path beside it (never
/// the app's real index).
struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/guides");
        copy_dir(&fixtures, &dir.path().join("library"));
        Self { dir }
    }

    fn library(&self) -> PathBuf {
        self.dir.path().join("library")
    }

    fn index(&self) -> PathBuf {
        self.dir.path().join("index.sqlite")
    }

    fn bundle(&self, name: &str) -> PathBuf {
        self.library().join(name)
    }

    /// Runs `guidesya --library <lib> --index <idx> <args>`.
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = guidesya();
        cmd.arg("--library")
            .arg(self.library())
            .arg("--index")
            .arg(self.index())
            .args(args);
        cmd.output().unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert_success(&out);
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Every file in the library with its bytes.
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        snapshot_into(&self.library(), &mut files);
        files
    }
}

fn guidesya() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_guidesya"));
    cmd.env_remove("GUIDESYA_LIBRARY");
    cmd
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn snapshot_into(dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.insert(path.clone(), Vec::new());
            snapshot_into(&path, files);
        } else {
            files.insert(path.clone(), fs::read(&path).unwrap());
        }
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status,
        stdout(out),
        stderr(out)
    );
}

fn titles(guides: &Value) -> Vec<String> {
    let mut titles: Vec<String> = guides
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["title"].as_str().unwrap().to_owned())
        .collect();
    titles.sort();
    titles
}

/// Object keys, sorted.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort();
    keys
}

fn edit_guide_json(bundle: &Path, edit: impl FnOnce(&mut Value)) {
    let path = bundle.join("guide.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    edit(&mut value);
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}

// ---------------------------------------------------------------------------
// General

#[test]
fn version_prints_package_version() {
    let out = guidesya().arg("--version").output().unwrap();
    assert_success(&out);
    assert_eq!(
        stdout(&out).trim(),
        format!("guidesya {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn usage_errors_exit_2() {
    let out = guidesya().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = guidesya().arg("frobnicate").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = guidesya().arg("validate").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "validate needs a bundle");
}

#[test]
fn missing_library_is_an_error() {
    let out = guidesya().arg("list").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("GUIDESYA_LIBRARY"),
        "{}",
        stderr(&out)
    );

    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope");
    let out = guidesya()
        .arg("--library")
        .arg(&missing)
        .arg("list")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!missing.exists(), "a reader must not create the library");
}

#[test]
fn library_comes_from_the_environment() {
    let fx = Fixture::new();
    let out = guidesya()
        .env("GUIDESYA_LIBRARY", fx.library())
        .arg("--index")
        .arg(fx.index())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["guides"].as_array().unwrap().len(), 3);
}

#[test]
fn export_is_a_placeholder() {
    let fx = Fixture::new();
    let out = fx.run(&["export", "x", "--format", "md"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("not yet implemented"));
}

// ---------------------------------------------------------------------------
// list

#[test]
fn list_json_shape() {
    let fx = Fixture::new();
    let value = fx.json(&["list", "--json"]);
    assert_eq!(value["source"], "scan", "no index yet");
    assert_eq!(
        titles(&value["guides"]),
        [
            "Kurzanleitung Drucker",
            "Rotate an API key",
            "Share a calendar event"
        ]
    );
    let rotate = value["guides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["title"] == "Rotate an API key")
        .unwrap();
    let keys = keys(rotate);
    assert_eq!(
        keys,
        [
            "duplicateOf",
            "id",
            "path",
            "readOnly",
            "stepCount",
            "summary",
            "tags",
            "title",
            "updatedAt"
        ]
    );
    assert_eq!(rotate["id"], "01M4WZCE2000000000000002R0");
    assert_eq!(
        rotate["tags"],
        serde_json::json!(["security", "api", "console"])
    );
    assert_eq!(rotate["readOnly"], false);
    assert_eq!(rotate["duplicateOf"], Value::Null);
    assert!(rotate["stepCount"].as_u64().unwrap() > 0);
    assert!(rotate["path"].as_str().unwrap().ends_with(ROTATE));
}

#[test]
fn list_filters_by_tag_and_query() {
    let fx = Fixture::new();
    let value = fx.json(&["list", "--tag", "SECURITY", "--json"]);
    assert_eq!(titles(&value["guides"]), ["Rotate an API key"]);
    let value = fx.json(&["list", "--query", "calendar", "--json"]);
    assert_eq!(titles(&value["guides"]), ["Share a calendar event"]);
    let value = fx.json(&["list", "--tag", "nonexistent", "--json"]);
    assert_eq!(value["guides"], serde_json::json!([]));
}

#[test]
fn list_human_output() {
    let fx = Fixture::new();
    let out = fx.run(&["list"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains("Rotate an API key"), "{text}");
    assert!(text.contains("security, api, console"), "{text}");
    assert!(text.contains("01M4WZCE2000000000000002R0"), "{text}");
}

// ---------------------------------------------------------------------------
// search

#[test]
fn search_json_shape() {
    let fx = Fixture::new();
    let value = fx.json(&["search", "calendar", "--json"]);
    assert_eq!(value["source"], "scan");
    let hits = value["hits"].as_array().unwrap();
    assert!(!hits.is_empty());
    for hit in hits {
        let keys = keys(hit);
        assert_eq!(
            keys,
            [
                "guideId",
                "guidePath",
                "guideTitle",
                "itemId",
                "kind",
                "snippet",
                "stepIndex",
                "title"
            ]
        );
        assert_eq!(hit["guideTitle"], "Share a calendar event");
    }
    assert!(hits
        .iter()
        .any(|h| h["kind"] == "step" && h["stepIndex"].is_u64() && h["itemId"].is_string()));
}

#[test]
fn search_limit_and_no_matches() {
    let fx = Fixture::new();
    let value = fx.json(&["search", "calendar", "--limit", "1", "--json"]);
    assert_eq!(value["hits"].as_array().unwrap().len(), 1);
    let value = fx.json(&["search", "zzzqqq", "--json"]);
    assert_eq!(value["hits"], serde_json::json!([]));
    let out = fx.run(&["search", "zzzqqq"]);
    assert_success(&out);
    assert_eq!(stdout(&out).trim(), "No matches.");
}

#[test]
fn search_human_output() {
    let fx = Fixture::new();
    let out = fx.run(&["search", "calendar"]);
    assert_success(&out);
    assert!(stdout(&out).contains("Share a calendar event › "));
}

// ---------------------------------------------------------------------------
// reindex

#[test]
fn reindex_builds_then_syncs_and_readers_use_it() {
    let fx = Fixture::new();
    let before = fx.snapshot();

    let value = fx.json(&["reindex", "--json"]);
    assert_eq!(value["rebuilt"], true);
    let mut added: Vec<&str> = value["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    added.sort();
    assert_eq!(added, [DRUCKER, ROTATE, CALENDAR]);
    assert!(fx.index().is_file());
    assert_eq!(fx.snapshot(), before, "reindex writes only the index");

    // A current index serves the readers.
    assert_eq!(fx.json(&["list", "--json"])["source"], "index");
    let value = fx.json(&["search", "calendar", "--json"]);
    assert_eq!(value["source"], "index");
    assert!(!value["hits"].as_array().unwrap().is_empty());

    // Nothing changed: an empty sync.
    let value = fx.json(&["reindex", "--json"]);
    assert_eq!(
        value,
        serde_json::json!({
            "index": fx.index().display().to_string(),
            "rebuilt": false,
            "added": [],
            "updated": [],
            "removed": [],
        })
    );

    // A removed bundle makes the index stale (readers scan) until a sync.
    fs::remove_dir_all(fx.bundle(DRUCKER)).unwrap();
    assert_eq!(fx.json(&["list", "--json"])["source"], "scan");
    let value = fx.json(&["reindex", "--json"]);
    assert_eq!(value["removed"], serde_json::json!([DRUCKER]));

    let value = fx.json(&["reindex", "--full", "--json"]);
    assert_eq!(value["rebuilt"], true);
    assert_eq!(value["added"].as_array().unwrap().len(), 2);
}

#[test]
fn reindex_human_output() {
    let fx = Fixture::new();
    let out = fx.run(&["reindex"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with("Index rebuilt: "), "{text}");
    assert!(text.contains("added: 3"), "{text}");
}

#[test]
fn reindex_refuses_a_foreign_file() {
    let fx = Fixture::new();
    fs::write(fx.index(), b"not a database").unwrap();
    let out = fx.run(&["reindex"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(fs::read(fx.index()).unwrap(), b"not a database");
}

// ---------------------------------------------------------------------------
// validate

#[test]
fn validate_fixtures_ok() {
    let fx = Fixture::new();
    let bundles = [ROTATE, CALENDAR, DRUCKER].map(|b| fx.bundle(b));
    let mut cmd = guidesya();
    cmd.arg("validate").args(&bundles).arg("--json");
    let out = cmd.output().unwrap();
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], true);
    let reports = value["bundles"].as_array().unwrap();
    assert_eq!(reports.len(), 3);
    for report in reports {
        let keys = keys(report);
        assert_eq!(
            keys,
            [
                "currentSchemaVersion",
                "errors",
                "id",
                "needsMigration",
                "ok",
                "path",
                "readOnly",
                "schemaVersion",
                "stepCount",
                "title",
                "warnings"
            ]
        );
        assert_eq!(report["ok"], true);
        assert_eq!(report["errors"], serde_json::json!([]));
        assert_eq!(report["schemaVersion"], report["currentSchemaVersion"]);
        assert_eq!(report["needsMigration"], false);
        assert_eq!(report["readOnly"], false);
    }
    assert_eq!(reports[0]["title"], "Rotate an API key");

    let out = fx.run(&["validate", fx.bundle(ROTATE).to_str().unwrap()]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains(": ok"), "{text}");
    assert!(
        text.contains("Rotate an API key: schema 1, writable"),
        "{text}"
    );
}

#[test]
fn validate_reports_broken_images() {
    let fx = Fixture::new();
    let bundle = fx.bundle(ROTATE);
    let image = bundle.join("steps/01M4WZCE2000000000000002R2.webp");
    fs::write(&image, b"not a webp").unwrap();
    fs::remove_file(bundle.join("steps/01M4WZCE2000000000000002R4.crop.webp")).unwrap();

    let out = fx.run(&["validate", bundle.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], false);
    let report = &value["bundles"][0];
    assert_eq!(report["ok"], false);
    let errors = report["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert_eq!(errors[0]["step"], "01M4WZCE2000000000000002R2");
    assert!(errors[0]["message"]
        .as_str()
        .unwrap()
        .contains("broken image steps/01M4WZCE2000000000000002R2.webp"));
    assert_eq!(errors[1]["step"], "01M4WZCE2000000000000002R4");

    let out = fx.run(&["validate", bundle.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("FAILED"));
    assert!(stdout(&out).contains("error: step 01M4WZCE2000000000000002R2"));
}

#[test]
fn validate_reports_unreadable_and_missing_bundles() {
    let fx = Fixture::new();
    fs::write(fx.bundle(CALENDAR).join("guide.json"), b"{ nope").unwrap();
    let missing = fx.library().join("Missing.guidesya");
    let out = fx.run(&[
        "validate",
        fx.bundle(ROTATE).to_str().unwrap(),
        fx.bundle(CALENDAR).to_str().unwrap(),
        missing.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(1));
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    let ok: Vec<bool> = value["bundles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["ok"].as_bool().unwrap())
        .collect();
    assert_eq!(ok, [true, false, false]);
    assert!(value["bundles"][1]["errors"][0]["message"]
        .as_str()
        .unwrap()
        .contains("invalid guide.json"));
    assert_eq!(value["bundles"][1]["schemaVersion"], Value::Null);
    assert!(!missing.exists());

    let not_bundle = fx.library();
    let out = fx.run(&["validate", not_bundle.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn validate_newer_schema_is_read_only() {
    let fx = Fixture::new();
    let bundle = fx.bundle(CALENDAR);
    edit_guide_json(&bundle, |v| v["schemaVersion"] = 99.into());
    let before = fx.snapshot();
    let value = {
        let out = fx.run(&["validate", bundle.to_str().unwrap(), "--json"]);
        assert_success(&out);
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let report = &value["bundles"][0];
    assert_eq!(report["schemaVersion"], 99);
    assert_eq!(report["readOnly"], true);
    assert_eq!(report["needsMigration"], false);
    assert!(report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["message"].as_str().unwrap().contains("newer")));
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn validate_rejects_an_invalid_schema_version() {
    let fx = Fixture::new();
    let bundle = fx.bundle(CALENDAR);
    edit_guide_json(&bundle, |v| v["schemaVersion"] = 0.into());
    let out = fx.run(&["validate", bundle.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("schemaVersion"), "{}", stdout(&out));
}

// ---------------------------------------------------------------------------
// Readers leave the library alone

#[test]
fn readers_leave_library_bytes_unchanged() {
    let fx = Fixture::new();
    let before = fx.snapshot();
    let bundles: Vec<String> = [ROTATE, CALENDAR, DRUCKER]
        .iter()
        .map(|b| fx.bundle(b).display().to_string())
        .collect();
    let mut validate = vec!["validate"];
    validate.extend(bundles.iter().map(String::as_str));
    for args in [
        vec!["list"],
        vec!["list", "--json", "--query", "key"],
        vec!["search", "key"],
        vec!["search", "key", "--json"],
        validate,
    ] {
        assert_success(&fx.run(&args));
        assert_eq!(fx.snapshot(), before, "{args:?} changed the library");
    }
    assert!(!fx.index().exists(), "readers never create the index");
}

// ---------------------------------------------------------------------------
// Review fixes

#[test]
fn closed_stdout_is_not_an_error() {
    let fx = Fixture::new();
    for args in [&["list"][..], &["list", "--json"], &["search", "key"]] {
        let mut child = guidesya()
            .arg("--library")
            .arg(fx.library())
            .arg("--index")
            .arg(fx.index())
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // The reader goes away before the first write.
        drop(child.stdout.take());
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{args:?}");
    }
}

#[test]
fn search_limit_must_be_positive() {
    let fx = Fixture::new();
    let out = fx.run(&["search", "key", "--limit", "0"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--limit"));
}

#[test]
fn reindex_needs_an_explicit_index() {
    let fx = Fixture::new();
    let home = TempDir::new().unwrap();
    let out = guidesya()
        .env("HOME", home.path())
        .arg("--library")
        .arg(fx.library())
        .arg("reindex")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--index PATH"), "{}", stderr(&out));

    let out = fx.run(&["reindex", "--app-index"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("--app-index cannot be used with --index"));

    assert!(!fx.index().exists());
    assert_eq!(fs::read_dir(home.path()).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn reindex_app_index_writes_the_default_path() {
    let fx = Fixture::new();
    let home = TempDir::new().unwrap();
    let out = guidesya()
        .env("HOME", home.path())
        .arg("--library")
        .arg(fx.library())
        .args(["reindex", "--app-index", "--json"])
        .output()
        .unwrap();
    assert_success(&out);
    let app_index = home
        .path()
        .join("Library/Application Support/dev.guidesya.app/index.sqlite");
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["index"], app_index.display().to_string());
    assert!(app_index.is_file());
}

#[test]
fn reindex_refuses_another_format_without_full() {
    if Command::new("sqlite3").arg("-version").output().is_err() {
        eprintln!("skipped: no sqlite3 binary to rewrite the index format");
        return;
    }
    let fx = Fixture::new();
    assert_success(&fx.run(&["reindex"]));
    let status = Command::new("sqlite3")
        .arg(fx.index())
        .arg("UPDATE meta SET value = '0/0' WHERE key = 'format_version';")
        .status()
        .unwrap();
    assert!(status.success());
    let before = fs::read(fx.index()).unwrap();

    let out = fx.run(&["reindex"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("pass --full"), "{}", stderr(&out));
    assert_eq!(fs::read(fx.index()).unwrap(), before, "index left alone");

    let value = fx.json(&["reindex", "--full", "--json"]);
    assert_eq!(value["rebuilt"], true);
    assert_eq!(value["added"].as_array().unwrap().len(), 3);
    assert_eq!(fx.json(&["list", "--json"])["source"], "index");
}
