//! End-to-end tests: a raw JSON-RPC client talks to the server over a tokio
//! duplex, against a copy of `fixtures/guides` and an index built from it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use guiderecorder_core::edit::EDITOR_LOCK_FILE;
use guiderecorder_core::export::{render_step_image, RenderedImage};
use guiderecorder_core::index::Index;
use guiderecorder_core::library::{read_webp, write_webp, Library, PixelBuffer};
use guiderecorder_core::model::{
    AiStatus, Guide, GuideField, ImageShow, Item, NoteKind, Rect, RedactionSuggestion, Shape,
    SuggestionSource,
};
use guiderecorder_core::Ulid;
use rmcp::ServiceExt;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines, ReadHalf};
use tokio::io::{AsyncRead, WriteHalf};

use crate::server::{
    image_options, thumbnail_options, Config, GuideRecorderMcp, IMAGE_BYTE_BUDGET, IMAGE_MAX_WIDTH,
    THUMBNAIL_MAX_WIDTH,
};
use crate::{parse_args, Command};

const ROTATE: &str = "01M4WZCE2000000000000002R0";
const ROTATE_NOTE: &str = "01M4WZCE2000000000000002R1";
const ROTATE_FIRST_STEP: &str = "01M4WZCE2000000000000002R2";
/// Has a blur annotation over the password field.
const ROTATE_PASSWORD_STEP: &str = "01M4WZCE2000000000000002R4";
const CALENDAR: &str = "01M4WT7MM000000000000002G0";
const CALENDAR_SECTION: &str = "01M4WT7MM000000000000002G1";
/// The annotation guide (pages with hotspots).
const FLOOR_PLAN: &str = "01M4Z96G000000000000000380";
const FLOOR_PLAN_FIRST_PAGE: &str = "01M4Z96G000000000000000385";
const FIXTURE_COUNT: usize = 4;
/// JSON-RPC error codes.
const RESOURCE_NOT_FOUND: i64 = -32002;
const INTERNAL_ERROR: i64 = -32603;

// ---------------------------------------------------------------------------
// Harness

/// A temp copy of the fixture library, with an index built from it.
struct Fixture {
    _dir: tempfile::TempDir,
    library: PathBuf,
    index: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("library");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/guides");
        copy_dir(&source, &library);
        let index = dir.path().join("index").join("index.sqlite");
        Self {
            _dir: dir,
            library,
            index,
        }
    }

    /// Builds the index the way the app does; the writer is closed after.
    fn with_index(self) -> Self {
        let library = Library::open(&self.library).unwrap();
        let mut writer = Index::open(&self.index).unwrap();
        writer.rebuild(&library).unwrap();
        drop(writer);
        self
    }

    fn bundle(&self, title: &str) -> PathBuf {
        self.library.join(format!("{title}.guiderecorder"))
    }

    fn config(&self) -> Config {
        Config {
            library: Library::open(&self.library).unwrap(),
            index: Some(self.index.clone()),
            writable: false,
        }
    }

    /// As started with `--writable`.
    fn writable(&self) -> Config {
        Config {
            writable: true,
            ..self.config()
        }
    }

    /// A guide as saved on disk.
    fn saved(&self, id: &str) -> Guide {
        let library = Library::open(&self.library).unwrap();
        let handle = library
            .find(Ulid::from_string(id).unwrap())
            .unwrap()
            .unwrap();
        library.read(&handle.path).unwrap().guide
    }

    /// Every file in the library, and the index database itself.
    ///
    /// Allowed exception: opening the index read-only may create SQLite's
    /// `-shm` and an empty `-wal` sidecar (normal WAL behaviour; documented
    /// in the README). Those are left out here and checked separately by
    /// [`Fixture::assert_sidecars_hold_no_data`].
    fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        collect_files(&self.library, &mut files);
        if let Ok(bytes) = fs::read(&self.index) {
            files.insert(self.index.clone(), bytes);
        }
        files
    }

    fn sidecar(&self, suffix: &str) -> PathBuf {
        let mut name = self.index.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    }

    /// The allowed sidecars: `-shm` may appear, and `-wal` may appear but
    /// must stay empty, so no transaction was ever written.
    fn assert_sidecars_hold_no_data(&self) {
        let wal = fs::read(self.sidecar("-wal")).unwrap_or_default();
        assert!(wal.is_empty(), "a reader wrote {} WAL bytes", wal.len());
        let index_dir: Vec<_> = fs::read_dir(self.index.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        for path in &index_dir {
            assert!(
                *path == self.index
                    || *path == self.sidecar("-wal")
                    || *path == self.sidecar("-shm"),
                "unexpected file next to the index: {}",
                path.display()
            );
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn collect_files(dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.insert(path.clone(), Vec::new());
            collect_files(&path, out);
        } else {
            out.insert(path.clone(), fs::read(&path).unwrap());
        }
    }
}

/// A minimal MCP client: newline-delimited JSON-RPC over the duplex.
struct Client {
    writer: WriteHalf<DuplexStream>,
    lines: Lines<BufReader<ReadHalf<DuplexStream>>>,
    next_id: u64,
    /// The `initialize` result.
    init: Value,
}

impl Client {
    async fn start(config: Config) -> Self {
        let (server_io, client_io) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let service = GuideRecorderMcp::new(config)
                .serve(server_io)
                .await
                .unwrap();
            let _ = service.waiting().await;
        });
        let (reader, writer) = tokio::io::split(client_io);
        let mut client = Self {
            writer,
            lines: BufReader::new(reader).lines(),
            next_id: 1,
            init: Value::Null,
        };
        let init = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" }
                }),
            )
            .await
            .unwrap();
        assert_eq!(init["serverInfo"]["name"], "guiderecorder");
        assert!(init["capabilities"]["tools"].is_object());
        assert!(init["capabilities"]["resources"].is_object());
        client.init = init;
        client
            .send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        client
    }

    async fn send(&mut self, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    /// The `result`, or the JSON-RPC `error`.
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
            .await;
        loop {
            let line = next_line(&mut self.lines).await;
            let message: Value = serde_json::from_str(&line).unwrap();
            if message["id"] != json!(id) {
                continue; // a notification
            }
            return match message.get("error") {
                Some(error) => Err(error.clone()),
                None => Ok(message["result"].clone()),
            };
        }
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": tool, "arguments": arguments }),
        )
        .await
        .unwrap_or_else(|e| panic!("{tool}: protocol error {e}"))
    }

    /// Calls a tool that must succeed.
    async fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let result = self.call(tool, arguments).await;
        assert_ne!(result["isError"], true, "{tool}: {result:#}");
        result
    }

    /// Calls a tool that must fail, and returns its message.
    async fn tool_error(&mut self, tool: &str, arguments: Value) -> String {
        let result = self.call(tool, arguments).await;
        assert_eq!(result["isError"], true, "{tool} succeeded: {result:#}");
        result["content"][0]["text"].as_str().unwrap().to_owned()
    }
}

async fn next_line<R: AsyncRead + Unpin>(lines: &mut Lines<BufReader<R>>) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(30), lines.next_line())
        .await
        .expect("server reply timed out")
        .unwrap()
        .expect("server closed the connection")
}

fn text(result: &Value, i: usize) -> &str {
    assert_eq!(result["content"][i]["type"], "text", "{result:#}");
    result["content"][i]["text"].as_str().unwrap()
}

fn images(result: &Value) -> Vec<Vec<u8>> {
    result["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block["type"] == "image")
        .map(|block| {
            assert_eq!(block["mimeType"], "image/png");
            base64::engine::general_purpose::STANDARD
                .decode(block["data"].as_str().unwrap())
                .unwrap()
        })
        .collect()
}

/// Decodes a PNG (as `image::RgbaImage`, named only by inference).
fn rendered(png: &[u8]) -> RenderedImage {
    RenderedImage {
        png: png.to_vec(),
        width: 0,
        height: 0,
        warnings: Vec::new(),
        hotspots: Vec::new(),
        point: None,
    }
}

fn step_of(
    library: &Library,
    guide: &str,
    step: &str,
) -> (
    guiderecorder_core::library::GuideHandle,
    guiderecorder_core::model::Step,
) {
    let guide = Ulid::from_string(guide).unwrap();
    let step = Ulid::from_string(step).unwrap();
    let handle = library.find(guide).unwrap().unwrap();
    let loaded = library.read(&handle.path).unwrap();
    let step = loaded
        .guide
        .items
        .into_iter()
        .find_map(|item| match item {
            Item::Step(s) if s.id == step => Some(s),
            _ => None,
        })
        .unwrap();
    (loaded.handle, step)
}

// ---------------------------------------------------------------------------
// Tests

#[tokio::test]
async fn lists_the_four_tools_with_schemas() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.config()).await;
    let tools = client.request("tools/list", json!({})).await.unwrap();
    let mut names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["get_guide", "get_step", "list_guides", "search_steps"]
    );
    let get_step = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "get_step")
        .unwrap();
    let required = get_step["inputSchema"]["required"].as_array().unwrap();
    assert!(required.contains(&json!("guide_id")) && required.contains(&json!("step_id")));
}

#[tokio::test]
async fn list_guides_reads_the_index_and_filters() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.config()).await;

    let all = client.ok("list_guides", json!({})).await;
    let guides = all["structuredContent"]["guides"].as_array().unwrap();
    assert_eq!(guides.len(), FIXTURE_COUNT, "{all:#}");
    let rotate = guides.iter().find(|g| g["id"] == ROTATE).unwrap();
    assert_eq!(rotate["title"], "Rotate an API key");
    assert_eq!(rotate["step_count"], 5);
    assert_eq!(rotate["kind"], "walkthrough");
    assert_eq!(rotate["hotspot_count"], 0);
    let floor_plan = guides.iter().find(|g| g["id"] == FLOOR_PLAN).unwrap();
    assert_eq!(floor_plan["kind"], "annotated");
    assert_eq!(floor_plan["step_count"], 2);
    assert_eq!(floor_plan["hotspot_count"], 6);
    assert_eq!(rotate["read_only"], false);
    assert_eq!(rotate["tags"], json!(["security", "api", "console"]));
    assert!(rotate["updated_at"]
        .as_str()
        .unwrap()
        .starts_with("2026-10-14T10:55:00"));

    let tagged = client.ok("list_guides", json!({ "tag": "MACOS" })).await;
    let ids: Vec<&Value> = tagged["structuredContent"]["guides"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| &g["id"])
        .collect();
    assert_eq!(ids, [&json!(CALENDAR)]);

    let queried = client
        .ok("list_guides", json!({ "query": "password" }))
        .await;
    assert_eq!(queried["structuredContent"]["guides"][0]["id"], ROTATE);

    let limited = client.ok("list_guides", json!({ "limit": 1 })).await;
    assert_eq!(
        limited["structuredContent"]["guides"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(limited["structuredContent"]["total"], FIXTURE_COUNT);
}

#[tokio::test]
async fn guides_in_folders_are_listed_and_read_with_and_without_the_index() {
    let fx = Fixture::new();
    let folder = fx.library.join("Work").join("Security");
    fs::create_dir_all(&folder).unwrap();
    fs::rename(
        fx.bundle("Rotate an API key"),
        folder.join("Rotate an API key.guiderecorder"),
    )
    .unwrap();
    let fx = fx.with_index();
    for index in [Some(fx.index.clone()), None] {
        let config = Config {
            index,
            ..fx.config()
        };
        let mut client = Client::start(config).await;
        let all = client.ok("list_guides", json!({ "tag": "security" })).await;
        let guides = all["structuredContent"]["guides"].as_array().unwrap();
        assert_eq!(guides.len(), 1, "{all:#}");
        assert_eq!(guides[0]["id"], ROTATE);
        assert_eq!(guides[0]["folder"], "Work/Security");
        let all = client.ok("list_guides", json!({})).await;
        let calendar = all["structuredContent"]["guides"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"] == CALENDAR)
            .unwrap()
            .clone();
        assert_eq!(calendar["folder"], "");
        let guide = client.ok("get_guide", json!({ "id": ROTATE })).await;
        assert!(text(&guide, 0).contains("Rotate an API key"));
        let hits = client
            .ok("search_steps", json!({ "query": "password" }))
            .await;
        assert!(!hits["structuredContent"]["hits"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn review_fields_are_listed_and_in_the_guide_markdown() {
    let fx = Fixture::new();
    let path = fx.bundle("Rotate an API key").join("guide.json");
    let mut guide: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    guide["owner"] = "Security *team*".into();
    guide["status"] = "needsUpdate".into();
    guide["version"] = "2.1".into();
    guide["lastReviewed"] = "2000-01-31".into();
    guide["reviewIntervalMonths"] = 1.into();
    fs::write(&path, serde_json::to_vec(&guide).unwrap()).unwrap();
    let fx = fx.with_index();
    for index in [Some(fx.index.clone()), None] {
        let config = Config {
            index,
            ..fx.config()
        };
        let mut client = Client::start(config).await;
        let all = client.ok("list_guides", json!({})).await;
        let guides = all["structuredContent"]["guides"].as_array().unwrap();
        let rotate = guides.iter().find(|g| g["id"] == ROTATE).unwrap();
        assert_eq!(rotate["owner"], "Security *team*");
        assert_eq!(rotate["status"], "needsUpdate");
        assert_eq!(rotate["version"], "2.1");
        assert_eq!(rotate["last_reviewed"], "2000-01-31");
        assert_eq!(rotate["review_interval_months"], 1);
        assert_eq!(rotate["review_due"], "2000-02-29");
        assert_eq!(rotate["review_overdue"], true);
        let calendar = guides.iter().find(|g| g["id"] == CALENDAR).unwrap();
        assert_eq!(calendar["owner"], Value::Null);
        assert_eq!(calendar["review_overdue"], false);

        let md = client.ok("get_guide", json!({ "id": ROTATE })).await;
        let md = text(&md, 0);
        for fact in [
            "Owner: Security \\*team\\*",
            "Status: needsUpdate",
            "Version: 2\\.1",
            "Last reviewed: 2000-01-31",
            "Review due: 2000-02-29 (overdue)",
        ] {
            assert!(md.contains(fact), "{fact} in {md}");
        }
    }
}

#[tokio::test]
async fn duplicate_copies_are_hidden_with_and_without_the_index() {
    let fx = Fixture::new();
    copy_dir(
        &fx.bundle("Rotate an API key"),
        &fx.library.join("Rotate an API key copy.guiderecorder"),
    );
    let fx = fx.with_index();
    for index in [Some(fx.index.clone()), None] {
        let config = Config {
            index,
            ..fx.config()
        };
        let mut client = Client::start(config).await;
        let all = client.ok("list_guides", json!({})).await;
        let guides = all["structuredContent"]["guides"].as_array().unwrap();
        assert_eq!(guides.len(), FIXTURE_COUNT, "{all:#}");
        let hits = client
            .ok("search_steps", json!({ "query": "password" }))
            .await;
        let hits = hits["structuredContent"]["hits"].as_array().unwrap();
        assert!(hits.iter().all(|h| h["guide_id"] == ROTATE));
        let paths: Vec<_> = hits.iter().map(|h| &h["item_id"]).collect();
        let mut unique = paths.clone();
        unique.dedup();
        assert_eq!(paths, unique, "a hit from the copy");
        let resources = client.request("resources/list", json!({})).await.unwrap();
        assert_eq!(
            resources["resources"].as_array().unwrap().len(),
            FIXTURE_COUNT
        );
    }
}

#[tokio::test]
async fn get_guide_returns_markdown_with_anchors() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.config()).await;
    let result = client.ok("get_guide", json!({ "id": ROTATE })).await;
    assert_eq!(result["content"].as_array().unwrap().len(), 1);
    let md = text(&result, 0);
    assert!(
        md.starts_with(&format!("# Rotate an API key {{#guide-{ROTATE}}}\n\n")),
        "{md}"
    );
    assert!(md.contains("Replace a leaked or expiring API key without downtime."));
    assert!(md.contains("Tags: security, api, console"));
    assert!(
        md.contains("## Introduction\n\nRotate keys every *90 days*, or right away if one leaks.")
    );
    assert!(md.contains("## Prerequisites\n\nAdmin access to the console."));
    assert!(md.contains(&format!(
        "### 1. Open **Settings → API keys** {{#step-{ROTATE_FIRST_STEP}}}\n"
    )));
    assert!(md.contains(&format!(
        "> **Warning** {{#note-{ROTATE_NOTE}}}\n>\n> Creating a new key **revokes**"
    )));
    assert!(
        md.contains("> 1. The CI secret\n> 2. Local `.env` files"),
        "{md}"
    );
    assert!(md.contains("### 5. "));
    // Keystrokes stay masked, and the element is described.
    assert!(md.contains("Keys: `••••••••⏎`"), "{md}");
    assert!(md.contains("Element: **Password** (AXSecureTextField) in Safari"));
    assert!(md.contains("[Managing keys](<https://example.com/docs/keys>)"));

    let calendar = client.ok("get_guide", json!({ "id": CALENDAR })).await;
    let md = text(&calendar, 0);
    assert!(
        md.contains(&format!(
            "## Open the event {{#section-{CALENDAR_SECTION}}}"
        )),
        "{md}"
    );
    assert!(md.contains("## Invite people {#section-"));
}

#[tokio::test]
async fn annotation_guides_list_their_hotspots() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.config()).await;
    let result = client.ok("get_guide", json!({ "id": FLOOR_PLAN })).await;
    let md = text(&result, 0);
    assert!(md.contains("Kind: annotation guide"), "{md}");
    assert!(
        md.contains(&format!(
            "### 1. Third floor {{#step-{FLOOR_PLAN_FIRST_PAGE}}}\n\n\
             The lifts open onto the corridor in the middle.\n\n\
             Hotspots:\n\n\
             1. **Reception**\\\n   Sign in here when you arrive.\\\n   \
             Visitor badges are in the top drawer.\n\
             2. **Kitchen \\& café**\\\n"
        )),
        "{md}"
    );
    // User text is escaped; pages have no recorded action.
    assert!(md.contains("press \\<Input\\> on the remote"), "{md}");
    assert!(!md.contains("Action:"), "{md}");

    let step = client
        .ok(
            "get_step",
            json!({ "guide_id": FLOOR_PLAN, "step_id": FLOOR_PLAN_FIRST_PAGE }),
        )
        .await;
    let body = text(&step, 0);
    assert!(
        body.contains("4. **Quiet zone**\\\n   No calls or meetings."),
        "{body}"
    );
    assert!(!body.contains("Action:"), "{body}");

    // Walkthroughs say nothing about a kind.
    let rotate = client.ok("get_guide", json!({ "id": ROTATE })).await;
    assert!(!text(&rotate, 0).contains("Kind:"));
    assert!(!text(&rotate, 0).contains("Hotspots:"));
}

#[tokio::test]
async fn get_guide_includes_capped_rendered_images() {
    let fx = Fixture::new().with_index();
    let library = Library::open(&fx.library).unwrap();
    let mut client = Client::start(fx.config()).await;

    let result = client
        .ok("get_guide", json!({ "id": ROTATE, "include_images": true }))
        .await;
    let pngs = images(&result);
    assert_eq!(pngs.len(), 5);
    let (handle, step) = step_of(&library, ROTATE, ROTATE_PASSWORD_STEP);
    let expected = render_step_image(&handle, &step, &thumbnail_options());
    assert_eq!(pngs[1], expected.png);
    assert!(
        text(&result, 3).starts_with(&format!("Image of step 2 {{#step-{ROTATE_PASSWORD_STEP}}}"))
    );

    let capped = client
        .ok(
            "get_guide",
            json!({ "id": ROTATE, "include_images": true, "max_images": 2 }),
        )
        .await;
    assert_eq!(images(&capped).len(), 2);
    let last = capped["content"].as_array().unwrap().last().unwrap();
    assert!(last["text"]
        .as_str()
        .unwrap()
        .starts_with("Showing 2 of 5 step images"));

    let chosen = client
        .ok(
            "get_guide",
            json!({
                "id": ROTATE,
                "include_images": true,
                "image_steps": [format!("step-{ROTATE_PASSWORD_STEP}")]
            }),
        )
        .await;
    assert_eq!(images(&chosen), [expected.png]);

    let message = client
        .tool_error(
            "get_guide",
            json!({ "id": ROTATE, "include_images": true, "image_steps": [CALENDAR_SECTION] }),
        )
        .await;
    assert!(message.contains("has no step"), "{message}");
}

#[tokio::test]
async fn get_step_returns_text_and_the_rendered_crop() {
    let fx = Fixture::new().with_index();
    let library = Library::open(&fx.library).unwrap();
    let mut client = Client::start(fx.config()).await;

    let result = client
        .ok(
            "get_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_PASSWORD_STEP }),
        )
        .await;
    let body = text(&result, 0);
    assert!(body.starts_with(&format!(
        "Guide: Rotate an API key {{#guide-{ROTATE}}}, step 2 of 5\n\n### 2. Enter your password to confirm {{#step-{ROTATE_PASSWORD_STEP}}}"
    )), "{body}");
    assert!(body.contains("Action: type"));
    assert!(body.contains("Keys: `••••••••⏎`"));

    let (handle, step) = step_of(&library, ROTATE, ROTATE_PASSWORD_STEP);
    let expected = render_step_image(&handle, &step, &image_options(Some(ImageShow::Crop)));
    // Render warnings reach the caller (the fixture crop has no cropRect,
    // so the renderer shows the full screenshot to keep the blur in place).
    for warning in &expected.warnings {
        assert!(body.contains(&warning.to_string()), "{body}");
    }
    let pngs = images(&result);
    assert_eq!(pngs, std::slice::from_ref(&expected.png));
    let decoded = rendered(&pngs[0]).to_rgba().expect("a PNG");
    assert_eq!(decoded.dimensions(), (expected.width, expected.height));
    assert!(expected.width <= IMAGE_MAX_WIDTH);

    let none = client
        .ok(
            "get_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_PASSWORD_STEP, "image": "none" }),
        )
        .await;
    assert!(images(&none).is_empty());
    assert_eq!(none["content"].as_array().unwrap().len(), 1);
}

/// The raw screenshot must never leave the server: the blurred region of the
/// returned image differs from the source pixels, and everything else is
/// the source.
#[tokio::test]
async fn get_step_burns_in_the_blur() {
    let fx = Fixture::new();
    let library = Library::open(&fx.library).unwrap();
    let (handle, step) = step_of(&library, ROTATE, ROTATE_PASSWORD_STEP);
    // The fixture images are solid colours, which blur to themselves; give
    // the full screenshot detail to hide.
    let full = handle.resolve(&step.image.full).unwrap();
    let (w, h) = read_webp(&full).unwrap().dimensions();
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let on = (x + y) % 2 == 0;
            pixels.extend_from_slice(if on {
                &[255, 255, 255, 255]
            } else {
                &[0, 0, 0, 255]
            });
        }
    }
    write_webp(&full, &PixelBuffer::rgba(w, h, &pixels)).unwrap();
    let raw = read_webp(&full).unwrap();
    let blur: Rect = step
        .annotations
        .iter()
        .find_map(|a| match a.shape {
            Shape::Blur(rect) => Some(rect),
            _ => None,
        })
        .unwrap();
    let fx = fx.with_index();
    let mut client = Client::start(fx.config()).await;

    let result = client
        .ok(
            "get_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_PASSWORD_STEP, "image": "full" }),
        )
        .await;
    let pngs = images(&result);
    let expected = render_step_image(&handle, &step, &image_options(Some(ImageShow::Full)));
    assert!(expected.warnings.is_empty(), "{:?}", expected.warnings);
    assert_eq!(pngs, [expected.png]);
    let sent = rendered(&pngs[0]).to_rgba().expect("a PNG");
    assert_eq!(sent.dimensions(), (w, h));

    let inside = |x: u32, y: u32| {
        let (x, y) = (f64::from(x), f64::from(y));
        x >= blur.x && x < blur.x + blur.width && y >= blur.y && y < blur.y + blur.height
    };
    let mut changed = 0;
    let mut kept = 0;
    for y in 0..h {
        for x in 0..w {
            let same = sent.get_pixel(x, y).0 == raw.get_pixel(x, y).0;
            if inside(x, y) {
                changed += usize::from(!same);
            } else if !same {
                // Only the blur may touch pixels beyond its edge (by its
                // radius); far away the source is untouched.
                let near = |v: f64, lo: f64, len: f64| v >= lo - 16.0 && v < lo + len + 16.0;
                assert!(
                    near(f64::from(x), blur.x, blur.width)
                        && near(f64::from(y), blur.y, blur.height),
                    "pixel {x},{y} changed outside the blur"
                );
            } else {
                kept += 1;
            }
        }
    }
    let area = (blur.width * blur.height) as usize;
    assert!(
        changed * 10 >= area * 9,
        "only {changed} of {area} blurred pixels changed"
    );
    assert!(kept > 0);
}

/// Text found sensitive on the device is never sent, reviewed or not: an
/// on-device blur suggestion is blurred in the image, an AI one is not.
#[tokio::test]
async fn get_step_burns_in_on_device_suggestions() {
    let fx = Fixture::new();
    let library = Library::open(&fx.library).unwrap();
    let (handle, step) = step_of(&library, ROTATE, ROTATE_PASSWORD_STEP);
    let full = handle.resolve(&step.image.full).unwrap();
    let (w, h) = read_webp(&full).unwrap().dimensions();
    let pixels: Vec<u8> = (0..h)
        .flat_map(|y| (0..w).map(move |x| if (x + y) % 2 == 0 { 255 } else { 0 }))
        .flat_map(|v| [v, v, v, 255])
        .collect();
    write_webp(&full, &PixelBuffer::rgba(w, h, &pixels)).unwrap();
    let raw = read_webp(&full).unwrap();
    // The 64 × 40 screenshot's corners, away from its blur at (4, 8).
    assert_eq!((w, h), (64, 40));
    let on_device = Rect {
        x: 44.0,
        y: 24.0,
        width: 16.0,
        height: 12.0,
    };
    let ai = Rect {
        x: 0.0,
        y: 32.0,
        width: 12.0,
        height: 8.0,
    };
    let mut loaded = library.read(&handle.path).unwrap();
    for item in &mut loaded.guide.items {
        if let Item::Step(step) = item {
            if step.id.to_string() == ROTATE_PASSWORD_STEP {
                step.ai_state.redaction_suggestions = vec![
                    RedactionSuggestion {
                        rect: on_device,
                        reason: "API key".into(),
                        source: SuggestionSource::OnDevice,
                        preview: None,
                        review: None,
                    },
                    RedactionSuggestion {
                        rect: ai,
                        reason: "name".into(),
                        source: SuggestionSource::Ai,
                        preview: None,
                        review: None,
                    },
                ];
            }
        }
    }
    library.save_to(&handle, &loaded.guide).unwrap();
    let fx = fx.with_index();
    let mut client = Client::start(fx.config()).await;

    let result = client
        .ok(
            "get_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_PASSWORD_STEP, "image": "full" }),
        )
        .await;
    let pngs = images(&result);
    assert_eq!(pngs.len(), 1);
    let sent = rendered(&pngs[0]).to_rgba().expect("a PNG");
    assert_eq!(sent.dimensions(), (w, h));
    let changed = |r: &Rect| {
        (r.y as u32..(r.y + r.height) as u32)
            .flat_map(|y| (r.x as u32..(r.x + r.width) as u32).map(move |x| (x, y)))
            .filter(|&(x, y)| sent.get_pixel(x, y).0 != raw.get_pixel(x, y).0)
            .count()
    };
    let area = (on_device.width * on_device.height) as usize;
    assert!(
        changed(&on_device) * 10 >= area * 9,
        "on-device suggestion not blurred"
    );
    assert_eq!(changed(&ai), 0, "AI suggestion blurred");
}

#[tokio::test]
async fn search_steps_returns_hits_with_context() {
    let fx = Fixture::new().with_index();
    for index in [Some(fx.index.clone()), None] {
        let config = Config {
            index,
            ..fx.config()
        };
        let mut client = Client::start(config).await;
        let result = client
            .ok("search_steps", json!({ "query": "password" }))
            .await;
        let hits = result["structuredContent"]["hits"].as_array().unwrap();
        let step = hits
            .iter()
            .find(|h| h["item_id"] == ROTATE_PASSWORD_STEP)
            .unwrap_or_else(|| panic!("{result:#}"));
        assert_eq!(step["guide_id"], ROTATE);
        assert_eq!(step["guide_title"], "Rotate an API key");
        assert_eq!(step["kind"], "step");
        assert_eq!(step["step_number"], 2);
        assert!(step["snippet"].as_str().unwrap().contains("**"), "{step}");

        let limited = client
            .ok("search_steps", json!({ "query": "the", "limit": 1 }))
            .await;
        assert_eq!(
            limited["structuredContent"]["hits"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn guide_resources_list_and_read() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.config()).await;
    let list = client.request("resources/list", json!({})).await.unwrap();
    let resources = list["resources"].as_array().unwrap();
    assert_eq!(resources.len(), FIXTURE_COUNT);
    let uri = format!("guiderecorder://guide/{ROTATE}");
    let rotate = resources.iter().find(|r| r["uri"] == uri.as_str()).unwrap();
    assert_eq!(rotate["name"], "Rotate an API key");
    assert_eq!(rotate["mimeType"], "text/markdown");

    let templates = client
        .request("resources/templates/list", json!({}))
        .await
        .unwrap();
    assert_eq!(
        templates["resourceTemplates"][0]["uriTemplate"],
        "guiderecorder://guide/{id}"
    );

    let read = client
        .request("resources/read", json!({ "uri": uri }))
        .await
        .unwrap();
    let guide = client.ok("get_guide", json!({ "id": ROTATE })).await;
    assert_eq!(read["contents"][0]["text"], guide["content"][0]["text"]);
    assert_eq!(read["contents"][0]["mimeType"], "text/markdown");

    for bad in [
        "guiderecorder://guide/nope".to_owned(),
        format!("guiderecorder://guide/{}", Ulid::nil()),
        "file:///etc/passwd".to_owned(),
    ] {
        let error = client
            .request("resources/read", json!({ "uri": bad }))
            .await
            .unwrap_err();
        assert_eq!(error["code"], RESOURCE_NOT_FOUND, "{error}");
        assert!(error["message"].as_str().unwrap().len() > 10, "{error}");
    }
}

#[tokio::test]
async fn errors_are_tool_errors_with_helpful_text() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.config()).await;

    let message = client
        .tool_error("get_guide", json!({ "id": "nope" }))
        .await;
    assert!(message.contains("not a valid guide id"), "{message}");

    let unknown = Ulid::nil().to_string();
    let message = client
        .tool_error("get_guide", json!({ "id": unknown }))
        .await;
    assert!(
        message.contains("no guide with id") && message.contains("list_guides"),
        "{message}"
    );

    let message = client
        .tool_error(
            "get_step",
            json!({ "guide_id": CALENDAR, "step_id": CALENDAR_SECTION }),
        )
        .await;
    assert!(message.contains("is a section"), "{message}");

    let message = client
        .tool_error(
            "get_step",
            json!({ "guide_id": CALENDAR, "step_id": ROTATE_FIRST_STEP }),
        )
        .await;
    assert!(
        message.contains("has no step") && message.contains("get_guide"),
        "{message}"
    );

    let message = client
        .tool_error("search_steps", json!({ "query": "  " }))
        .await;
    assert!(message.contains("query is empty"), "{message}");

    // A bad argument shape is a protocol error from the router.
    let error = client
        .request(
            "tools/call",
            json!({ "name": "get_step", "arguments": { "guide_id": ROTATE, "step_id": ROTATE_FIRST_STEP, "image": "huge" } }),
        )
        .await;
    let rejected = match error {
        Err(_) => true,
        Ok(result) => result["isError"] == true,
    };
    assert!(rejected);
}

#[tokio::test]
async fn newer_schema_guides_are_read_only_or_explained() {
    let fx = Fixture::new();
    // Decodable: shown read-only.
    let path = fx.bundle("Rotate an API key").join("guide.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schemaVersion"] = json!(guiderecorder_core::SCHEMA_VERSION + 1);
    value["futureField"] = json!(true);
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    // Not decodable: an item kind this version does not know.
    let path = fx.bundle("Share a calendar event").join("guide.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schemaVersion"] = json!(guiderecorder_core::SCHEMA_VERSION + 1);
    value["items"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "type": "video", "id": Ulid::new().to_string() }));
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let fx = fx.with_index();
    let mut client = Client::start(fx.config()).await;

    let all = client.ok("list_guides", json!({})).await;
    let rotate = all["structuredContent"]["guides"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == ROTATE)
        .unwrap()
        .clone();
    assert_eq!(rotate["read_only"], true);
    let guide = client.ok("get_guide", json!({ "id": ROTATE })).await;
    assert!(text(&guide, 0).contains("Saved by a newer GuideRecorder"));

    let message = client
        .tool_error("get_guide", json!({ "id": CALENDAR }))
        .await;
    assert!(
        message.contains("newer GuideRecorder") && message.contains("Update"),
        "{message}"
    );

    // As a resource it exists but cannot be read: not "not found".
    let error = client
        .request(
            "resources/read",
            json!({ "uri": format!("guiderecorder://guide/{CALENDAR}") }),
        )
        .await
        .unwrap_err();
    assert_eq!(error["code"], INTERNAL_ERROR, "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("newer GuideRecorder"),
        "{error}"
    );
}

/// Real screenshots are large: `get_guide` scales them to thumbnails and
/// stops adding images at the byte budget, saying so.
#[tokio::test]
async fn get_guide_images_stay_within_the_byte_budget() {
    let fx = Fixture::new();
    let library = Library::open(&fx.library).unwrap();
    // Noise does not compress: about 2.6 MB of PNG per 1024×640 thumbnail.
    let (w, h) = (1600, 1000);
    let mut state: u32 = 0x9E37_79B9;
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        for _ in 0..3 {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            pixels.push(state as u8);
        }
        pixels.push(255);
    }
    let loaded = library.read(fx.bundle("Rotate an API key")).unwrap();
    for step in loaded.guide.steps() {
        let full = loaded.handle.resolve(&step.image.full).unwrap();
        write_webp(&full, &PixelBuffer::rgba(w, h, &pixels)).unwrap();
    }
    // Show the full screenshots, so every thumbnail is the large noise.
    let json = loaded.handle.guide_json();
    let mut value: Value = serde_json::from_slice(&fs::read(&json).unwrap()).unwrap();
    for item in value["items"].as_array_mut().unwrap() {
        if item["type"] == "step" {
            item["image"]["show"] = json!("full");
            item["annotations"] = json!([]);
        }
    }
    fs::write(&json, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let mut client = Client::start(fx.config()).await;

    let result = client
        .ok("get_guide", json!({ "id": ROTATE, "include_images": true }))
        .await;
    let pngs = images(&result);
    let total: usize = pngs.iter().map(Vec::len).sum();
    assert!(!pngs.is_empty() && pngs.len() < 5, "{} images", pngs.len());
    assert!(total <= IMAGE_BYTE_BUDGET, "{total} bytes");
    for png in &pngs {
        let decoded = rendered(png).to_rgba().expect("a PNG");
        assert_eq!(decoded.width(), THUMBNAIL_MAX_WIDTH);
    }
    let last = result["content"].as_array().unwrap().last().unwrap();
    let notice = last["text"].as_str().unwrap();
    assert!(
        notice.starts_with(&format!("Showing {} of 5 step images", pngs.len()))
            && notice.contains("response limit"),
        "{notice}"
    );

    // get_step keeps the larger size for a single image.
    let step = client
        .ok(
            "get_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_FIRST_STEP, "image": "full" }),
        )
        .await;
    let png = &images(&step)[0];
    assert_eq!(rendered(png).to_rgba().unwrap().width(), IMAGE_MAX_WIDTH);
}

#[tokio::test]
async fn no_call_writes_to_the_library_or_index() {
    let fx = Fixture::new().with_index();
    let before = fx.snapshot();
    assert!(before.contains_key(&fx.index));
    let mut client = Client::start(fx.config()).await;
    client
        .ok("list_guides", json!({ "query": "key", "tag": "security" }))
        .await;
    client.ok("list_guides", json!({})).await;
    client
        .ok("get_guide", json!({ "id": ROTATE, "include_images": true }))
        .await;
    client
        .ok(
            "get_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_PASSWORD_STEP, "image": "full" }),
        )
        .await;
    client
        .ok("search_steps", json!({ "query": "calendar" }))
        .await;
    client.request("resources/list", json!({})).await.unwrap();
    client
        .request(
            "resources/read",
            json!({ "uri": format!("guiderecorder://guide/{CALENDAR}") }),
        )
        .await
        .unwrap();
    client
        .tool_error("get_guide", json!({ "id": Ulid::nil().to_string() }))
        .await;
    assert_eq!(fx.snapshot(), before);
    fx.assert_sidecars_hold_no_data();
}

// ---------------------------------------------------------------------------
// Write tools (`--writable`)

const WRITE_TOOLS: [&str; 8] = [
    "add_note",
    "add_tags",
    "create_guide_from_images",
    "remove_tags",
    "reorder_items",
    "set_cover",
    "update_guide_fields",
    "update_step",
];

async fn tool_names(client: &mut Client) -> Vec<String> {
    let tools = client.request("tools/list", json!({})).await.unwrap();
    let mut names: Vec<String> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn write_tools_only_with_writable() {
    let fx = Fixture::new().with_index();
    let before = fx.snapshot();
    let mut client = Client::start(fx.config()).await;
    let names = tool_names(&mut client).await;
    assert!(WRITE_TOOLS.iter().all(|t| !names.iter().any(|n| n == t)));
    assert!(text_of(&client.init["instructions"]).contains("(read-only)"));
    // Not routed either: the call is refused and nothing changes.
    let call = client
        .request(
            "tools/call",
            json!({ "name": "add_tags", "arguments": { "guide_id": ROTATE, "tags": ["x"] } }),
        )
        .await;
    assert!(match call {
        Err(_) => true,
        Ok(result) => result["isError"] == true,
    });
    assert_eq!(fx.snapshot(), before);

    let mut client = Client::start(fx.writable()).await;
    let names = tool_names(&mut client).await;
    for tool in WRITE_TOOLS {
        assert!(names.iter().any(|n| n == tool), "{tool} missing: {names:?}");
    }
    assert_eq!(names.len(), 4 + WRITE_TOOLS.len());
    assert!(text_of(&client.init["instructions"]).contains("Editing is allowed"));
}

fn text_of(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

#[tokio::test]
async fn update_guide_fields_marks_the_fields_edited() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.writable()).await;
    let result = client
        .ok(
            "update_guide_fields",
            json!({
                "guide_id": ROTATE,
                "title": "Rotate an API key safely",
                "summary": "Replace a leaked or expiring API key without downtime.",
                "tags": ["security", "Keys, rotation"],
            }),
        )
        .await;
    let out = &result["structuredContent"];
    assert_eq!(out["guide_id"], ROTATE);
    assert_eq!(out["changed"], true);
    // The summary was already that.
    assert_eq!(out["changed_fields"], json!(["title", "tags"]));

    let saved = fx.saved(ROTATE);
    assert_eq!(saved.title, "Rotate an API key safely");
    assert_eq!(saved.tags, ["security", "Keys", "rotation"]);
    assert_eq!(saved.edited_fields, [GuideField::Title, GuideField::Tags]);
    assert_eq!(saved.ai_state.status, AiStatus::Edited);
    assert_eq!(out["updated_at"], saved.updated_at.to_rfc3339());
    // The folder keeps its name, as when the title is edited in the app.
    assert!(fx.bundle("Rotate an API key").is_dir());

    // Reads see it at once (the stale index falls back to a scan).
    let guide = client.ok("get_guide", json!({ "id": ROTATE })).await;
    assert!(text(&guide, 0).starts_with("# Rotate an API key safely"));
    let listed = client.ok("list_guides", json!({ "tag": "rotation" })).await;
    assert_eq!(listed["structuredContent"]["guides"][0]["id"], ROTATE);

    // The same values again change nothing and write nothing.
    let before = fx.snapshot();
    let again = client
        .ok(
            "update_guide_fields",
            json!({ "guide_id": ROTATE, "title": "Rotate an API key safely" }),
        )
        .await;
    assert_eq!(again["structuredContent"]["changed"], false);
    assert_eq!(again["structuredContent"]["changed_fields"], json!([]));
    assert_eq!(fx.snapshot(), before);

    let message = client
        .tool_error("update_guide_fields", json!({ "guide_id": ROTATE }))
        .await;
    assert!(message.contains("nothing to change"), "{message}");
    let message = client
        .tool_error(
            "update_guide_fields",
            json!({ "guide_id": ROTATE, "title": "  " }),
        )
        .await;
    assert!(message.contains("cannot be empty"), "{message}");
}

#[tokio::test]
async fn update_step_add_note_and_reorder() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.writable()).await;

    let result = client
        .ok(
            "update_step",
            json!({
                "guide_id": ROTATE,
                "step_id": format!("step-{ROTATE_PASSWORD_STEP}"),
                "description": "Type your **console** password.",
            }),
        )
        .await;
    assert_eq!(result["structuredContent"]["step_id"], ROTATE_PASSWORD_STEP);
    assert_eq!(result["structuredContent"]["changed"], true);
    let saved = fx.saved(ROTATE);
    let step = saved
        .steps()
        .find(|s| s.id.to_string() == ROTATE_PASSWORD_STEP)
        .unwrap();
    assert_eq!(step.description, "Type your **console** password.");
    assert_eq!(step.ai_state.status, AiStatus::Edited);
    // A step edit is not a guide-level edit.
    assert!(saved.edited_fields.is_empty());

    let message = client
        .tool_error(
            "update_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_NOTE, "title": "x" }),
        )
        .await;
    assert!(message.contains("is a note"), "{message}");

    let result = client
        .ok(
            "add_note",
            json!({
                "guide_id": ROTATE,
                "kind": "tip",
                "body": "Keep the old key until CI passes.",
                "after_id": format!("step-{ROTATE_FIRST_STEP}"),
            }),
        )
        .await;
    let note = result["structuredContent"]["note_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let saved = fx.saved(ROTATE);
    let ids: Vec<String> = saved.items.iter().map(|i| i.id().to_string()).collect();
    assert_eq!(ids[1], ROTATE_FIRST_STEP);
    assert_eq!(ids[2], note);
    assert!(matches!(
        &saved.items[2],
        Item::Note(n) if n.kind == NoteKind::Tip && n.body == "Keep the old key until CI passes."
    ));
    let guide = client.ok("get_guide", json!({ "id": ROTATE })).await;
    assert!(text(&guide, 0).contains(&format!("{{#note-{note}}}")));

    let message = client
        .tool_error(
            "add_note",
            json!({
                "guide_id": ROTATE,
                "body": "x",
                "after_id": ROTATE_NOTE,
                "before_id": ROTATE_NOTE,
            }),
        )
        .await;
    assert!(message.contains("not both"), "{message}");

    // Reverse every item, prefixed ids and bare ones mixed.
    let mut reversed: Vec<String> = ids.iter().rev().cloned().collect();
    reversed[0] = format!("step-{}", reversed[0]);
    let result = client
        .ok(
            "reorder_items",
            json!({ "guide_id": ROTATE, "item_ids": reversed }),
        )
        .await;
    assert_eq!(result["structuredContent"]["changed"], true);
    let order: Vec<String> = fx
        .saved(ROTATE)
        .items
        .iter()
        .map(|i| i.id().to_string())
        .collect();
    assert_eq!(order, ids.iter().rev().cloned().collect::<Vec<_>>());

    let message = client
        .tool_error(
            "reorder_items",
            json!({ "guide_id": ROTATE, "item_ids": [ROTATE_NOTE] }),
        )
        .await;
    assert!(message.contains("exactly once"), "{message}");
}

#[tokio::test]
async fn add_and_remove_tags() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.writable()).await;
    let result = client
        .ok(
            "add_tags",
            json!({ "guide_id": CALENDAR, "tags": ["Sharing", "MACOS"] }),
        )
        .await;
    let out = &result["structuredContent"];
    assert_eq!(out["added"], json!(["Sharing"]));
    assert!(out["tags"].as_array().unwrap().contains(&json!("Sharing")));
    assert_eq!(fx.saved(CALENDAR).edited_fields, [GuideField::Tags]);

    let result = client
        .ok(
            "remove_tags",
            json!({ "guide_id": CALENDAR, "tags": ["sharing", "nope"] }),
        )
        .await;
    assert_eq!(result["structuredContent"]["removed"], json!(["Sharing"]));
    assert!(!fx.saved(CALENDAR).tags.contains(&"Sharing".to_owned()));

    let before = fx.snapshot();
    let result = client
        .ok(
            "remove_tags",
            json!({ "guide_id": CALENDAR, "tags": ["nope"] }),
        )
        .await;
    assert_eq!(result["structuredContent"]["changed"], false);
    assert_eq!(fx.snapshot(), before);
}

#[tokio::test]
async fn set_cover_chooses_changes_and_clears_the_cover() {
    let fx = Fixture::new().with_index();
    let mut client = Client::start(fx.writable()).await;
    let markdown = text(&client.ok("get_guide", json!({ "id": ROTATE })).await, 0).to_owned();
    assert!(!markdown.contains("Cover:"), "{markdown}");

    let result = client
        .ok(
            "set_cover",
            json!({ "guide_id": ROTATE, "step_id": format!("step-{ROTATE_PASSWORD_STEP}") }),
        )
        .await;
    let out = &result["structuredContent"];
    assert_eq!(out["changed"], true);
    assert_eq!(out["cover_step_id"], ROTATE_PASSWORD_STEP);
    assert_eq!(
        (&out["cover_image"], &out["cover_annotations"]),
        (&json!("full"), &json!(false))
    );
    let saved = fx.saved(ROTATE);
    assert_eq!(
        saved.cover_step_id.map(|id| id.to_string()).as_deref(),
        Some(ROTATE_PASSWORD_STEP)
    );
    let markdown = text(&client.ok("get_guide", json!({ "id": ROTATE })).await, 0).to_owned();
    assert!(
        markdown.contains(&format!(
            "{{#step-{ROTATE_PASSWORD_STEP}}} (full screenshot, without annotations)"
        )),
        "{markdown}"
    );

    // Options alone keep the chosen step.
    let result = client
        .ok(
            "set_cover",
            json!({ "guide_id": ROTATE, "image": "crop", "annotations": true }),
        )
        .await;
    let out = &result["structuredContent"];
    assert_eq!(out["cover_step_id"], ROTATE_PASSWORD_STEP);
    assert_eq!(
        (&out["cover_image"], &out["cover_annotations"]),
        (&json!("crop"), &json!(true))
    );
    assert_eq!(fx.saved(ROTATE).cover_options.image, ImageShow::Crop);

    let result = client
        .ok("set_cover", json!({ "guide_id": ROTATE, "clear": true }))
        .await;
    assert_eq!(result["structuredContent"]["cover_step_id"], Value::Null);
    assert_eq!(fx.saved(ROTATE).cover_step_id, None);

    let before = fx.snapshot();
    let error = client
        .tool_error(
            "set_cover",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_NOTE }),
        )
        .await;
    assert!(error.contains("is a note"), "{error}");
    let error = client
        .tool_error("set_cover", json!({ "guide_id": ROTATE }))
        .await;
    assert!(error.contains("nothing to change"), "{error}");
    let error = client
        .tool_error(
            "set_cover",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_FIRST_STEP, "clear": true }),
        )
        .await;
    assert!(error.contains("not both"), "{error}");
    assert_eq!(fx.snapshot(), before);
}

#[tokio::test]
async fn writes_refuse_open_read_only_and_unknown_guides() {
    let fx = Fixture::new();
    // Open in the editor of a running app (this test process).
    let lock = fx.bundle("Rotate an API key").join(EDITOR_LOCK_FILE);
    fs::write(&lock, format!("{}\n", std::process::id())).unwrap();
    // Saved by a newer app.
    let path = fx.bundle("Share a calendar event").join("guide.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schemaVersion"] = json!(guiderecorder_core::SCHEMA_VERSION + 1);
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let fx = fx.with_index();
    let before = fx.snapshot();
    let mut client = Client::start(fx.writable()).await;

    let message = client
        .tool_error("add_tags", json!({ "guide_id": ROTATE, "tags": ["x"] }))
        .await;
    assert!(
        message.contains("open in the GuideRecorder editor"),
        "{message}"
    );
    let message = client
        .tool_error(
            "update_step",
            json!({ "guide_id": ROTATE, "step_id": ROTATE_FIRST_STEP, "title": "x" }),
        )
        .await;
    assert!(
        message.contains("open in the GuideRecorder editor"),
        "{message}"
    );
    let message = client
        .tool_error("add_tags", json!({ "guide_id": CALENDAR, "tags": ["x"] }))
        .await;
    assert!(
        message.contains("read-only") && message.contains("newer GuideRecorder"),
        "{message}"
    );
    let message = client
        .tool_error(
            "add_tags",
            json!({ "guide_id": Ulid::nil().to_string(), "tags": ["x"] }),
        )
        .await;
    assert!(message.contains("no guide with id"), "{message}");
    let message = client
        .tool_error("add_tags", json!({ "guide_id": "nope", "tags": ["x"] }))
        .await;
    assert!(message.contains("not a valid guide id"), "{message}");
    assert_eq!(fx.snapshot(), before);

    // A lock left by an app that is gone does not count.
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let gone = child.id();
    child.wait().unwrap();
    fs::write(&lock, format!("{gone}\n")).unwrap();
    client
        .ok("add_tags", json!({ "guide_id": ROTATE, "tags": ["x"] }))
        .await;
    assert!(fx.saved(ROTATE).tags.contains(&"x".to_owned()));
}

#[tokio::test]
async fn older_guides_are_refused_until_the_app_upgrades_them() {
    let fx = Fixture::new();
    let path = fx.bundle("Rotate an API key").join("guide.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["schemaVersion"] = json!(1);
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let before = fx.snapshot();
    let mut client = Client::start(fx.writable()).await;
    let message = client
        .tool_error("add_tags", json!({ "guide_id": ROTATE, "tags": ["x"] }))
        .await;
    assert!(
        message.contains("read-only") && message.contains("open it once in GuideRecorder"),
        "{message}"
    );
    assert_eq!(fx.snapshot(), before);
}

#[tokio::test]
async fn create_guide_from_images_makes_an_annotation_guide() {
    let fx = Fixture::new().with_index();
    let files = tempfile::tempdir().unwrap();
    let pixels = [200u8, 10, 10, 255].repeat(6 * 4);
    let plan = files.path().join("Ground floor.webp");
    write_webp(&plan, &PixelBuffer::rgba(6, 4, &pixels)).unwrap();
    let roof = files.path().join("Roof.webp");
    write_webp(&roof, &PixelBuffer::rgba(6, 4, &pixels)).unwrap();
    let mut client = Client::start(fx.writable()).await;

    let result = client
        .ok(
            "create_guide_from_images",
            json!({ "paths": [plan, roof], "title": "Building" }),
        )
        .await;
    let out = &result["structuredContent"];
    assert_eq!(out["title"], "Building");
    assert_eq!(out["step_count"], 2);
    assert_eq!(out["folder"], "");
    let id = out["guide_id"].as_str().unwrap().to_owned();
    assert!(fx.bundle("Building").is_dir());

    let guide = client.ok("get_guide", json!({ "id": id })).await;
    let md = text(&guide, 0);
    assert!(md.contains("Kind: annotation guide"), "{md}");
    assert!(md.contains("### 1. Ground floor {#step-"), "{md}");
    assert!(md.contains("### 2. Roof {#step-"), "{md}");
    let step = out["step_ids"][0].as_str().unwrap();
    let page = client
        .ok(
            "get_step",
            json!({ "guide_id": id, "step_id": step, "image": "full" }),
        )
        .await;
    assert_eq!(images(&page).len(), 1);
    let all = client.ok("list_guides", json!({})).await;
    assert_eq!(
        all["structuredContent"]["guides"].as_array().unwrap().len(),
        FIXTURE_COUNT + 1
    );

    // Bad input leaves no guide.
    let before = fx.snapshot();
    let message = client
        .tool_error(
            "create_guide_from_images",
            json!({ "paths": ["relative.png"] }),
        )
        .await;
    assert!(message.contains("not an absolute path"), "{message}");
    let pdf = files.path().join("Manual.pdf");
    fs::write(&pdf, b"%PDF-1.4").unwrap();
    let message = client
        .tool_error("create_guide_from_images", json!({ "paths": [plan, pdf] }))
        .await;
    assert!(
        message.contains("Manual.pdf is not a PNG, JPEG or WebP image")
            && message.contains("GuideRecorder app"),
        "{message}"
    );
    let broken = files.path().join("broken.png");
    fs::write(&broken, b"not a png").unwrap();
    let message = client
        .tool_error(
            "create_guide_from_images",
            json!({ "paths": [plan, broken] }),
        )
        .await;
    assert!(message.contains("broken.png: not a readable"), "{message}");
    let message = client
        .tool_error("create_guide_from_images", json!({ "paths": [] }))
        .await;
    assert!(message.contains("no files"), "{message}");
    assert_eq!(fx.snapshot(), before);
}

#[test]
fn parses_arguments() {
    let args = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
    assert_eq!(
        parse_args(args(&["--library", "/lib", "--index=/x.sqlite"]), None).unwrap(),
        Command::Serve {
            library: "/lib".into(),
            index: Some("/x.sqlite".into()),
            writable: false,
        }
    );
    match parse_args(args(&["--writable", "--library", "/lib"]), None).unwrap() {
        Command::Serve { writable, .. } => assert!(writable),
        other => panic!("{other:?}"),
    }
    assert!(parse_args(args(&["--writable=yes"]), Some("/env".into())).is_err());
    match parse_args(args(&[]), Some("/env".into())).unwrap() {
        Command::Serve { library, .. } => assert_eq!(library, PathBuf::from("/env")),
        other => panic!("{other:?}"),
    }
    match parse_args(args(&["--library=/flag"]), Some("/env".into())).unwrap() {
        Command::Serve { library, .. } => assert_eq!(library, PathBuf::from("/flag")),
        other => panic!("{other:?}"),
    }
    assert_eq!(parse_args(args(&["-h"]), None).unwrap(), Command::Help);
    assert_eq!(
        parse_args(args(&["--version"]), None).unwrap(),
        Command::Version
    );
    let error = parse_args(args(&[]), None).unwrap_err().to_string();
    assert!(error.contains("GUIDERECORDER_LIBRARY"), "{error}");
    assert!(parse_args(args(&["--library"]), None).is_err());
    assert!(parse_args(args(&["--bogus"]), Some("/env".into())).is_err());
}

#[test]
fn a_missing_library_is_not_created() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    assert!(crate::open_library(&missing).is_err());
    assert!(!missing.exists());
}
