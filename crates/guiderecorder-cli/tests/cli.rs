//! Runs the built `guiderecorder` binary against a temp copy of
//! `fixtures/guides/`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const ROTATE: &str = "Rotate an API key.guiderecorder";
const CALENDAR: &str = "Share a calendar event.guiderecorder";
const DRUCKER: &str = "Kurzanleitung Drucker.guiderecorder";
/// The annotation guide: pages with hotspots.
const FLOOR_PLAN: &str = "Office floor plan.guiderecorder";

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

    /// Runs `guiderecorder --library <lib> --index <idx> <args>`.
    fn run(&self, args: &[&str]) -> Output {
        let mut cmd = guiderecorder();
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

fn guiderecorder() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_guiderecorder"));
    cmd.env_remove("GUIDERECORDER_LIBRARY");
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
    let out = guiderecorder().arg("--version").output().unwrap();
    assert_success(&out);
    assert_eq!(
        stdout(&out).trim(),
        format!("guiderecorder {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn usage_errors_exit_2() {
    let out = guiderecorder().output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = guiderecorder().arg("frobnicate").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = guiderecorder().arg("validate").output().unwrap();
    assert_eq!(out.status.code(), Some(2), "validate needs a bundle");
}

#[test]
fn missing_library_is_an_error() {
    let out = guiderecorder().arg("list").output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("GUIDERECORDER_LIBRARY"),
        "{}",
        stderr(&out)
    );

    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("nope");
    let out = guiderecorder()
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
    let out = guiderecorder()
        .env("GUIDERECORDER_LIBRARY", fx.library())
        .arg("--index")
        .arg(fx.index())
        .args(["list", "--json"])
        .output()
        .unwrap();
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["guides"].as_array().unwrap().len(), 4);
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
            "Office floor plan",
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
            "createdAt",
            "duplicateOf",
            "folder",
            "id",
            "lastReviewed",
            "owner",
            "path",
            "readOnly",
            "reviewDue",
            "reviewIntervalMonths",
            "reviewOverdue",
            "status",
            "stepCount",
            "summary",
            "tags",
            "title",
            "updatedAt",
            "version"
        ]
    );
    assert_eq!(rotate["owner"], Value::Null);
    assert_eq!(rotate["reviewOverdue"], false);
    assert_eq!(rotate["id"], "01M4WZCE2000000000000002R0");
    assert_eq!(
        rotate["tags"],
        serde_json::json!(["security", "api", "console"])
    );
    assert_eq!(rotate["readOnly"], false);
    assert_eq!(rotate["duplicateOf"], Value::Null);
    assert!(rotate["stepCount"].as_u64().unwrap() > 0);
    assert!(rotate["path"].as_str().unwrap().ends_with(ROTATE));
    assert_eq!(rotate["folder"], "");
}

/// Sets top-level `guide.json` fields of a fixture bundle.
fn set_fields(bundle: &Path, fields: Value) {
    let path = bundle.join("guide.json");
    let mut guide: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for (key, value) in fields.as_object().unwrap() {
        guide[key] = value.clone();
    }
    fs::write(&path, serde_json::to_vec_pretty(&guide).unwrap()).unwrap();
}

#[test]
fn list_shows_review_fields_and_filters_overdue() {
    let fx = Fixture::new();
    set_fields(
        &fx.bundle(ROTATE),
        serde_json::json!({
            "owner": "Security team",
            "status": "needsUpdate",
            "version": "2.1",
            "lastReviewed": "2000-01-31",
            "reviewIntervalMonths": 1,
        }),
    );
    // Reviewed "in the future": never overdue whatever today is.
    set_fields(
        &fx.bundle(CALENDAR),
        serde_json::json!({ "lastReviewed": "2999-01-01", "reviewIntervalMonths": 12 }),
    );
    for _ in 0..2 {
        let value = fx.json(&["list", "--overdue", "--json"]);
        assert_eq!(titles(&value["guides"]), ["Rotate an API key"]);
        let rotate = &value["guides"][0];
        assert_eq!(rotate["owner"], "Security team");
        assert_eq!(rotate["status"], "needsUpdate");
        assert_eq!(rotate["version"], "2.1");
        assert_eq!(rotate["lastReviewed"], "2000-01-31");
        assert_eq!(rotate["reviewIntervalMonths"], 1);
        assert_eq!(rotate["reviewDue"], "2000-02-29");
        assert_eq!(rotate["reviewOverdue"], true);

        let text = stdout(&fx.run(&["list", "--overdue"]));
        assert!(
            text.contains("[needs update]  [review overdue since 2000-02-29]"),
            "{text}"
        );
        let all = fx.json(&["list", "--json"]);
        assert_eq!(all["guides"].as_array().unwrap().len(), 4);
        // Then again through the index.
        assert_success(&fx.run(&["reindex"]));
    }
}

#[test]
fn export_review_details_flag() {
    let fx = Fixture::new();
    set_fields(
        &fx.bundle(ROTATE),
        serde_json::json!({ "owner": "IT", "version": "3" }),
    );
    let plain = fx.out("plain");
    let detailed = fx.out("detailed");
    fx.export_json(ROTATE, "md", &plain, &[]);
    fx.export_json(ROTATE, "md", &detailed, &["--review-details"]);
    let read = |dir: &Path| fs::read_to_string(dir.join("guide.md")).unwrap();
    assert!(!read(&plain).contains("Owner: IT"));
    assert!(read(&detailed).contains("*Version: 3 · Owner: IT*"));
}

#[test]
fn list_and_search_find_guides_in_folders() {
    let fx = Fixture::new();
    let folder = fx.library().join("Work").join("Security");
    fs::create_dir_all(&folder).unwrap();
    fs::rename(fx.bundle(ROTATE), folder.join(ROTATE)).unwrap();
    for _ in 0..2 {
        let value = fx.json(&["list", "--tag", "security", "--json"]);
        assert_eq!(titles(&value["guides"]), ["Rotate an API key"]);
        assert_eq!(value["guides"][0]["folder"], "Work/Security");
        let hits = fx.json(&["search", "password", "--json"]);
        assert!(!hits["hits"].as_array().unwrap().is_empty(), "{hits:#}");
        // Then again through the index.
        assert_success(&fx.run(&["reindex"]));
    }
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
    assert_eq!(added, [DRUCKER, FLOOR_PLAN, ROTATE, CALENDAR]);
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
    assert_eq!(value["added"].as_array().unwrap().len(), 3);
}

#[test]
fn reindex_human_output() {
    let fx = Fixture::new();
    let out = fx.run(&["reindex"]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.starts_with("Index rebuilt: "), "{text}");
    assert!(text.contains("added: 4"), "{text}");
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
    let bundles = [ROTATE, CALENDAR, DRUCKER, FLOOR_PLAN].map(|b| fx.bundle(b));
    let mut cmd = guiderecorder();
    cmd.arg("validate").args(&bundles).arg("--json");
    let out = cmd.output().unwrap();
    assert_success(&out);
    let value: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["ok"], true);
    let reports = value["bundles"].as_array().unwrap();
    assert_eq!(reports.len(), 4);
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
    // The annotation guide's pages and hotspots render cleanly.
    assert_eq!(reports[3]["title"], "Office floor plan");
    assert_eq!(reports[3]["stepCount"], 2);
    assert_eq!(reports[3]["warnings"], serde_json::json!([]));

    let out = fx.run(&["validate", fx.bundle(ROTATE).to_str().unwrap()]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(text.contains(": ok"), "{text}");
    assert!(
        text.contains("Rotate an API key: schema 2, writable"),
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
    let missing = fx.library().join("Missing.guiderecorder");
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
    let out_dir = fx.dir.path().join("exports");
    let logo = fx.dir.path().join("logo.png");
    fs::write(&logo, PNG_1X1).unwrap();
    for (i, format) in ["md", "html", "docx", "pdf"].into_iter().enumerate() {
        let target = out_dir.join(format!("{i}.{format}"));
        let args = [
            "export",
            bundles[0].as_str(),
            "--format",
            format,
            "--out",
            target.to_str().unwrap(),
            "--logo",
            logo.to_str().unwrap(),
            "--accent",
            "#c0ffee",
        ];
        assert_success(&fx.run(&args));
        assert_eq!(fx.snapshot(), before, "{args:?} changed the library");
    }
    assert!(!fx.index().exists(), "readers never create the index");
}

// ---------------------------------------------------------------------------
// export

/// A valid 1×1 RGBA PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

impl Fixture {
    /// A path under the temp dir, outside the library.
    fn out(&self, name: &str) -> PathBuf {
        self.dir.path().join("out").join(name)
    }

    /// `export <bundle> --format <format> --out <out> <extra>`.
    fn export(&self, bundle: &str, format: &str, out: &Path, extra: &[&str]) -> Output {
        let bundle = self.bundle(bundle);
        let mut args = vec![
            "export",
            bundle.to_str().unwrap(),
            "--format",
            format,
            "--out",
            out.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        self.run(&args)
    }

    fn export_json(&self, bundle: &str, format: &str, out: &Path, extra: &[&str]) -> Value {
        let mut extra = extra.to_vec();
        extra.push("--json");
        let out = self.export(bundle, format, out, &extra);
        assert_success(&out);
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn logo(&self) -> PathBuf {
        let path = self.dir.path().join("logo.png");
        fs::write(&path, PNG_1X1).unwrap();
        path
    }
}

fn files(report: &Value) -> Vec<PathBuf> {
    report["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| PathBuf::from(f.as_str().unwrap()))
        .collect()
}

#[test]
fn export_each_format() {
    let fx = Fixture::new();
    let before = fx.snapshot();
    let cases = [
        ("md", "md", "guide.md"),
        ("html", "html", "guide.html"),
        ("docx", "drucker.docx", ""),
        ("pdf", "drucker.pdf", ""),
    ];
    for (format, out_name, main) in cases {
        let out = fx.out(out_name);
        let report = fx.export_json(DRUCKER, format, &out, &[]);
        assert_eq!(
            keys(&report),
            ["files", "format", "id", "notes", "title", "warnings"]
        );
        assert_eq!(report["format"], format);
        assert_eq!(report["title"], "Kurzanleitung Drucker");
        assert_eq!(report["warnings"], serde_json::json!([]));
        let files = files(&report);
        let expected_main = if main.is_empty() {
            out.clone()
        } else {
            out.join(main)
        };
        assert_eq!(files[0], expected_main, "{format}: main document first");
        for file in &files {
            assert!(file.is_file(), "{format}: {} missing", file.display());
        }
        if format == "md" {
            assert!(files.len() > 1, "md writes step images");
        }
    }
    let pdf = fs::read(fx.out("drucker.pdf")).unwrap();
    assert!(pdf.starts_with(b"%PDF-"));
    let docx = fs::read(fx.out("drucker.docx")).unwrap();
    assert!(docx.starts_with(b"PK"));
    assert_eq!(fx.snapshot(), before, "export changed the library");
}

#[test]
fn export_by_id_and_human_output() {
    let fx = Fixture::new();
    let list = fx.json(&["list", "--json"]);
    let guide = list["guides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["title"] == "Kurzanleitung Drucker")
        .unwrap();
    let id = guide["id"].as_str().unwrap();
    let target = fx.out("by-id");
    let out = fx.run(&[
        "export",
        id,
        "--format",
        "html",
        "--out",
        target.to_str().unwrap(),
    ]);
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.starts_with(&format!(
            "Exported Kurzanleitung Drucker ({id}) as html:
"
        )),
        "{text}"
    );
    assert!(text.contains(&target.join("guide.html").display().to_string()));
    assert!(target.join("guide.html").is_file());

    let out = fx.run(&[
        "export",
        "01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "--format",
        "md",
        "--out",
        fx.out("x").to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no guide with this id"),
        "{}",
        stderr(&out)
    );
    let out = fx.run(&["export", "nope", "--format", "md", "--out", "x"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no such bundle folder or guide id"));
    assert!(!fx.out("x").exists());
}

#[test]
fn export_usage_errors_exit_2() {
    let fx = Fixture::new();
    let out = fx.export(DRUCKER, "rtf", &fx.out("x"), &[]);
    assert_eq!(out.status.code(), Some(2));
    let bundle = fx.bundle(DRUCKER);
    let out = fx.run(&["export", bundle.to_str().unwrap(), "--format", "md"]);
    assert_eq!(out.status.code(), Some(2), "--out is required");
}

#[test]
fn export_refuses_to_overwrite_unless_asked() {
    let fx = Fixture::new();
    for (format, name) in [
        ("md", "md"),
        ("html", "html"),
        ("docx", "a.docx"),
        ("pdf", "a.pdf"),
    ] {
        let out = fx.out(name);
        fx.export_json(DRUCKER, format, &out, &[]);
        let again = fx.export(DRUCKER, format, &out, &[]);
        assert_eq!(again.status.code(), Some(1), "{format}");
        assert!(
            stderr(&again).contains("not empty") || stderr(&again).contains("already exists"),
            "{format}: {}",
            stderr(&again)
        );
        fx.export_json(DRUCKER, format, &out, &["--overwrite"]);
    }
}

#[test]
fn export_dates_come_from_the_guide() {
    // DOCX and PDF take `updated_at`, so two runs give the same bytes.
    let fx = Fixture::new();
    for format in ["docx", "pdf"] {
        let a = fx.out(&format!("a.{format}"));
        let b = fx.out(&format!("b.{format}"));
        fx.export_json(DRUCKER, format, &a, &[]);
        fx.export_json(DRUCKER, format, &b, &[]);
        assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap(), "{format}");
    }
}

#[test]
fn export_branding_flags() {
    let fx = Fixture::new();
    let logo = fx.logo();
    let logo = logo.to_str().unwrap();

    let out = fx.out("md");
    let report = fx.export_json(
        DRUCKER,
        "md",
        &out,
        &["--logo", logo, "--accent", "#c0ffee"],
    );
    assert_eq!(files(&report)[1], out.join("images/logo.png"));
    // Re-encoded (metadata dropped), still a 1×1 PNG: IHDR width and
    // height are bytes 16..24.
    let logo_png = fs::read(out.join("images/logo.png")).unwrap();
    assert!(logo_png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(logo_png[16..24], [0, 0, 0, 1, 0, 0, 0, 1]);
    let markdown = fs::read_to_string(out.join("guide.md")).unwrap();
    assert!(
        markdown.starts_with("# Kurzanleitung Drucker\n\n![](images/logo.png)\n"),
        "{markdown}"
    );

    let out = fx.out("html");
    fx.export_json(DRUCKER, "html", &out, &["--logo", logo, "--accent", "#C0F"]);
    let html = fs::read_to_string(out.join("guide.html")).unwrap();
    assert!(html.contains("<img class=\"logo\" src=\"data:image/png;base64,"));
    assert!(html.contains(":root { --accent: #cc00ff; }"));

    for format in ["docx", "pdf"] {
        let plain = fx.out(&format!("plain.{format}"));
        let branded = fx.out(&format!("branded.{format}"));
        fx.export_json(DRUCKER, format, &plain, &[]);
        fx.export_json(
            DRUCKER,
            format,
            &branded,
            &["--logo", logo, "--accent", "#aa1122"],
        );
        assert_ne!(fs::read(&plain).unwrap(), fs::read(&branded).unwrap());
    }
    let docx = fs::read(fx.out("branded.docx")).unwrap();
    assert!(docx.windows(6).any(|w| w == b"AA1122"), "accent in DOCX");
}

#[test]
fn export_paper_flag() {
    let fx = Fixture::new();
    let a4 = fx.out("a4.pdf");
    let letter = fx.out("letter.pdf");
    fx.export_json(DRUCKER, "pdf", &a4, &[]);
    fx.export_json(DRUCKER, "pdf", &letter, &["--paper", "letter"]);
    let a4 = fs::read(a4).unwrap();
    let letter = fs::read(letter).unwrap();
    let has = |pdf: &[u8], needle: &str| String::from_utf8_lossy(pdf).contains(needle);
    assert!(
        has(&a4, "/MediaBox[0 0 595.2756 841.8898]"),
        "A4 by default"
    );
    assert!(has(&letter, "/MediaBox[0 0 612 792]"), "Letter");

    let a4 = fx.out("a4.docx");
    let letter = fx.out("letter.docx");
    fx.export_json(DRUCKER, "docx", &a4, &["--paper", "a4"]);
    fx.export_json(DRUCKER, "docx", &letter, &["--paper", "letter"]);
    assert_ne!(fs::read(a4).unwrap(), fs::read(letter).unwrap());

    let out = fx.export(DRUCKER, "pdf", &fx.out("x.pdf"), &["--paper", "legal"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!fx.out("x.pdf").exists());
}

#[test]
fn export_refuses_bad_branding_before_writing() {
    let fx = Fixture::new();
    let svg = fx.dir.path().join("logo.svg");
    fs::write(&svg, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    let big = fx.dir.path().join("big.png");
    let mut bytes = PNG_1X1.to_vec();
    bytes.resize(2 * 1024 * 1024 + 1, 0);
    fs::write(&big, bytes).unwrap();
    let cases: [(&[&str], &str); 4] = [
        (&["--accent", "red"], "not a hex colour"),
        (&["--logo", svg.to_str().unwrap()], "SVG"),
        (&["--logo", big.to_str().unwrap()], "2 MB"),
        (&["--logo", "/nonexistent/logo.png"], "--logo"),
    ];
    for format in ["md", "html", "docx", "pdf"] {
        for (extra, message) in cases {
            let out = fx.out(&format!("bad.{format}"));
            let result = fx.export(DRUCKER, format, &out, extra);
            assert_eq!(result.status.code(), Some(1), "{format} {extra:?}");
            assert!(
                stderr(&result).contains(message),
                "{format} {extra:?}: {}",
                stderr(&result)
            );
            assert!(!out.exists(), "{format} {extra:?} wrote output");
        }
    }
}

#[test]
fn export_warnings_and_strict() {
    let fx = Fixture::new();
    // The rotate fixture has a crop without a position: one warning.
    let out = fx.out("rotate.html");
    let report = fx.export_json(ROTATE, "html", &out, &[]);
    let warnings = report["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{report:#}");
    assert_eq!(keys(&warnings[0]), ["message", "step"]);
    assert!(warnings[0]["message"]
        .as_str()
        .unwrap()
        .contains("crop position unknown"));

    let out = fx.out("rotate-text");
    let result = fx.export(ROTATE, "md", &out, &[]);
    assert_success(&result);
    assert!(
        stdout(&result).contains("    warning: step "),
        "{}",
        stdout(&result)
    );

    let strict = fx.out("strict");
    let result = fx.export(ROTATE, "md", &strict, &["--strict"]);
    assert_eq!(result.status.code(), Some(1));
    assert!(strict.join("guide.md").is_file(), "strict still writes");
    assert_success(&fx.export(DRUCKER, "md", &fx.out("clean"), &["--strict"]));
}

#[test]
fn export_refuses_an_output_inside_the_bundle() {
    let fx = Fixture::new();
    let before = fx.snapshot();
    let bundle = fx.bundle(DRUCKER);
    for out in [
        bundle.join("export"),
        bundle.join("assets/../guide.pdf"),
        fx.library().join("../library").join(DRUCKER).join("x.pdf"),
    ] {
        let result = fx.export(DRUCKER, "pdf", &out, &["--overwrite"]);
        assert_eq!(result.status.code(), Some(1), "{}", out.display());
        assert!(
            stderr(&result).contains("inside the guide bundle"),
            "{}",
            stderr(&result)
        );
    }
    assert_eq!(fx.snapshot(), before);
}

#[test]
#[cfg(unix)]
fn export_refuses_a_symlink_dotdot_into_the_bundle() {
    // RX3 #1: `link/..` is the parent of the link's target, so a lexical
    // `..` would have let this overwrite the bundle's guide.json.
    let fx = Fixture::new();
    // Git doesn't keep empty folders, so a fresh checkout's bundle may have
    // no steps/; the link must point at a real folder for this case.
    let steps = fx.bundle(DRUCKER).join("steps");
    std::fs::create_dir_all(&steps).unwrap();
    let before = fx.snapshot();
    let link = fx.dir.path().join("link");
    std::os::unix::fs::symlink(&steps, &link).unwrap();
    for (format, name) in [("pdf", "guide.json"), ("docx", "guide.json"), ("md", "x")] {
        let out = link.join("..").join(name);
        let result = fx.export(DRUCKER, format, &out, &["--overwrite"]);
        assert_eq!(result.status.code(), Some(1), "{format}");
        assert!(
            stderr(&result).contains("inside the guide bundle"),
            "{}",
            stderr(&result)
        );
    }
    assert_eq!(fx.snapshot(), before);
    // The guide still exports.
    fx.export_json(DRUCKER, "pdf", &fx.out("after.pdf"), &[]);
}

#[test]
fn export_refuses_other_bundles_and_the_library() {
    let fx = Fixture::new();
    let before = fx.snapshot();
    let list = fx.json(&["list", "--json"]);
    let id = list["guides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["title"] == "Kurzanleitung Drucker")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cases = [
        // Another guide's bundle, with the guide found by id.
        (
            fx.bundle(ROTATE).join("guide.json"),
            "inside the guide bundle",
        ),
        // Any `.guiderecorder` folder, even outside the library.
        (
            fx.dir.path().join("Elsewhere.guiderecorder/x.pdf"),
            "inside the guide bundle",
        ),
        // Anywhere in the library.
        (fx.library().join("export.pdf"), "inside the library"),
    ];
    for (out, message) in &cases {
        let result = fx.run(&[
            "export",
            &id,
            "--format",
            "pdf",
            "--out",
            out.to_str().unwrap(),
            "--overwrite",
        ]);
        assert_eq!(result.status.code(), Some(1), "{}", out.display());
        assert!(stderr(&result).contains(message), "{}", stderr(&result));
    }
    // The library is refused when given by path too.
    let result = fx.export(DRUCKER, "md", &fx.library().join("md"), &[]);
    assert!(
        stderr(&result).contains("inside the library"),
        "{}",
        stderr(&result)
    );
    assert_eq!(fx.snapshot(), before);
    assert!(!fx.dir.path().join("Elsewhere.guiderecorder").exists());
}

#[test]
fn export_refuses_dotdot_after_a_missing_folder() {
    let fx = Fixture::new();
    let out = fx.out("missing/../x.pdf");
    let result = fx.export(DRUCKER, "pdf", &out, &[]);
    assert_eq!(result.status.code(), Some(1));
    assert!(stderr(&result).contains("`..`"), "{}", stderr(&result));
    assert!(!fx.out("x.pdf").exists());
}

#[test]
#[cfg(unix)]
fn export_logo_must_be_a_regular_file() {
    // RX3 #2: /dev/zero never ends and a FIFO blocks on open; both are
    // refused before they are read.
    let fx = Fixture::new();
    let fifo = fx.dir.path().join("logo.fifo");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    let dir = fx.dir.path().to_path_buf();
    for logo in [Path::new("/dev/zero"), fifo.as_path(), dir.as_path()] {
        let out = fx.out("never.html");
        let result = fx.export(DRUCKER, "html", &out, &["--logo", logo.to_str().unwrap()]);
        assert_eq!(result.status.code(), Some(1), "{}", logo.display());
        assert!(
            stderr(&result).contains("not a regular file"),
            "{}",
            stderr(&result)
        );
        assert!(!out.exists());
    }
}

// ---------------------------------------------------------------------------
// Review fixes

#[test]
fn closed_stdout_is_not_an_error() {
    let fx = Fixture::new();
    for args in [&["list"][..], &["list", "--json"], &["search", "key"]] {
        let mut child = guiderecorder()
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
    let out = guiderecorder()
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
    let out = guiderecorder()
        .env("HOME", home.path())
        .arg("--library")
        .arg(fx.library())
        .args(["reindex", "--app-index", "--json"])
        .output()
        .unwrap();
    assert_success(&out);
    let app_index = home
        .path()
        .join("Library/Application Support/dev.dufflebear.guiderecorder/index.sqlite");
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
    assert_eq!(value["added"].as_array().unwrap().len(), 4);
    assert_eq!(fx.json(&["list", "--json"])["source"], "index");
}
