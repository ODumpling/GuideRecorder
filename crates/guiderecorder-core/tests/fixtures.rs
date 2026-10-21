//! Fixture guides in `fixtures/guides/`. See design scratchpad #439 ("Testing").
//!
//! The bundles are built by [`build_fixtures`] through
//! [`guiderecorder_core::library`], so they are exactly what the app writes. To
//! change them, edit the builders below and run
//!
//! ```text
//! REGENERATE_FIXTURES=1 cargo test -p guiderecorder-core --test fixtures -- --ignored regenerate_fixtures
//! ```
//!
//! `fixtures_match_builders` fails until the committed bundles are
//! regenerated. Tests never open the committed bundles as a library: they
//! work on a temp copy ([`FixtureLibrary`]). Images are tiny solid-colour stand-ins (64×40 screenshots,
//! 32×20 crops) to keep the repo small; geometry stays inside them. The
//! annotation guide's pages are small drawn floor plans instead
//! ([`PAGE_SIZE`]), so its hotspots have something to point at in the
//! app's sample library.
//!
//! Git does not keep empty folders, so a bundle checked out from the repo may
//! lack an empty `steps/` or `assets/`; the library copes with that.

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use guiderecorder_core::library::{
    step_image_path, write_webp, GuideHandle, Library, LoadedGuide, PixelBuffer, StepImageVariant,
    GUIDE_FILE,
};
use guiderecorder_core::markdown;
use guiderecorder_core::model::*;
use guiderecorder_core::{Ulid, SCHEMA_VERSION};
use image::{Rgba, RgbaImage};
use support::ImageTolerance;

const FULL_SIZE: (u32, u32) = (64, 40);
const CROP_SIZE: (u32, u32) = (32, 20);
/// Size of an annotation guide page.
const PAGE_SIZE: (u32, u32) = (480, 300);
const STEP_MODEL: &str = "claude-sonnet-5-5";

// ---------------------------------------------------------------------------
// Tests

#[test]
fn every_fixture_loads() {
    let fixtures = FixtureLibrary::new();
    let summaries = fixtures.library.list().unwrap();
    assert_eq!(summaries.len(), FIXTURE_COUNT, "{summaries:#?}");
    for summary in &summaries {
        assert_eq!(summary.duplicate_of, None);
        assert_eq!(summary.schema_version, SCHEMA_VERSION);
        let loaded = fixtures.library.load(&summary.path).unwrap();
        assert!(!loaded.read_only);
        assert_eq!(loaded.migrated_from, None);
        let (handle, guide) = (loaded.handle, loaded.guide);
        assert_eq!(handle.id, summary.id);
        assert_eq!(guide.title, summary.title);
        assert_eq!(guide.steps().count(), summary.step_count);
        assert_eq!(guide.kind, summary.kind);
        assert_eq!(guide.hotspot_count(), summary.hotspot_count);
    }
}

#[test]
fn fixtures_round_trip_byte_stably() {
    let fixtures = FixtureLibrary::new();
    for (committed, copy) in fixture_bundles().iter().zip(fixtures.bundles()) {
        let LoadedGuide { handle, guide, .. } = fixtures.library.load(&copy).unwrap();
        fixtures.library.save_to(&handle, &guide).unwrap();
        let original = fs::read(committed.join(GUIDE_FILE)).unwrap();
        let saved = fs::read(handle.guide_json()).unwrap();
        assert!(
            original == saved,
            "{} changed on save:\n{}",
            committed.display(),
            String::from_utf8_lossy(&saved)
        );
        assert_eq!(fixtures.library.load(&copy).unwrap().guide, guide);
    }
}

#[test]
fn every_image_decodes() {
    let fixtures = FixtureLibrary::new();
    for bundle in fixtures.bundles() {
        let LoadedGuide { handle, guide, .. } = fixtures.library.load(&bundle).unwrap();
        let full_size = match guide.kind {
            GuideKind::Walkthrough => FULL_SIZE,
            GuideKind::Annotated => PAGE_SIZE,
        };
        for step in guide.steps() {
            let image = &step.image;
            assert_eq!(
                handle.read_image(&image.full).unwrap().dimensions(),
                full_size
            );
            if let Some(crop) = &image.crop {
                assert_eq!(handle.read_image(crop).unwrap().dimensions(), CROP_SIZE);
            }
            if let Some(after) = &image.after {
                assert_eq!(handle.read_image(after).unwrap().dimensions(), FULL_SIZE);
            }
        }
    }
}

/// Keeps the fixtures honest: together they use every item, step, note,
/// annotation, blur, AI, image and audience variant, so exporters tested
/// against them see every case. A new model variant fails to compile in
/// [`variants`] until it is listed there, and then fails this test until a
/// fixture uses it.
#[test]
fn fixtures_cover_every_variant() {
    let guides = FixtureLibrary::new().guides();
    let items = || guides.iter().flat_map(|g| &g.items);
    let steps = || guides.iter().flat_map(|g| g.steps());
    let annotations = || steps().flat_map(|s| &s.annotations);
    let notes = || {
        items().filter_map(|i| match i {
            Item::Note(n) => Some(n),
            _ => None,
        })
    };

    fn assert_covers(what: &str, used: BTreeSet<String>, all: BTreeSet<String>) {
        assert_eq!(
            used, all,
            "{what}: fixtures use {used:?}, the model has {all:?}"
        );
    }
    assert_covers(
        "items",
        items().map(variants::item).map(str::to_owned).collect(),
        set(variants::ITEMS),
    );
    assert_covers(
        "step kinds",
        steps().map(|s| debug(s.kind)).collect(),
        variants::step_kinds(),
    );
    assert_covers(
        "note kinds",
        notes().map(|n| debug(n.kind)).collect(),
        variants::note_kinds(),
    );
    assert_covers(
        "shapes",
        annotations()
            .map(|a| variants::shape(&a.shape))
            .map(str::to_owned)
            .collect(),
        set(variants::SHAPES),
    );
    assert_covers(
        "blur modes",
        annotations()
            .filter_map(|a| a.style.blur_mode)
            .map(debug)
            .collect(),
        variants::blur_modes(),
    );
    assert_covers(
        "AI states",
        steps().map(|s| debug(s.ai_state.status)).collect(),
        variants::ai_statuses(),
    );
    assert_covers(
        "image show",
        steps().map(|s| debug(s.image.show)).collect(),
        variants::image_shows(),
    );
    assert_covers(
        "audiences",
        guides.iter().map(|g| debug(g.audience)).collect(),
        variants::audiences(),
    );
    assert_covers(
        "guide kinds",
        guides.iter().map(|g| debug(g.kind)).collect(),
        variants::guide_kinds(),
    );

    for step in steps() {
        assert_eq!(step.kind == StepKind::Drag, step.end_point.is_some());
    }
    assert!(steps().any(|s| s.image.after.is_some()));
    assert!(steps().any(|s| s.image.crop.is_none()));
    assert!(steps().any(|s| s.image.full.starts_with("assets/")));
    assert!(guides.iter().any(|g| g.recording.is_none()));
    assert!(guides.iter().any(|g| g.recording.is_some()));
    // Annotation guides are made of full-image manual pages.
    for guide in guides.iter().filter(|g| g.kind == GuideKind::Annotated) {
        for step in guide.steps() {
            assert_eq!(step.kind, StepKind::Manual);
            assert_eq!(step.image.show, ImageShow::Full);
            assert_eq!(step.image.crop, None);
        }
    }
    let hotspots = || {
        annotations().filter_map(|a| match &a.shape {
            Shape::Hotspot(h) => Some(h),
            _ => None,
        })
    };
    assert!(hotspots().any(|h| h.description.contains('\n')));
    assert!(hotspots()
        .any(|h| h.title.contains(['<', '&', '"']) || h.description.contains(['<', '&', '"'])));
}

/// `markdown::normalize` over every rich-text field of every fixture.
#[test]
fn markdown_normalize_golden() {
    let mut out = String::new();
    for guide in FixtureLibrary::new().guides() {
        for (field, text) in rich_text_fields(&guide) {
            let normalized = markdown::normalize(text);
            assert_eq!(markdown::normalize(&normalized), normalized, "{field}");
            out += &format!("=== {} :: {field}\n{normalized}\n", guide.title);
        }
    }
    support::assert_golden_text("markdown_normalize.golden", &out);
}

#[test]
fn fixtures_match_builders() {
    let temp = tempfile::tempdir().unwrap();
    build_fixtures(temp.path());
    let built = bundle_files(temp.path());
    let committed = bundle_files(&support::fixture_guides_dir());
    assert_eq!(
        built.keys().collect::<Vec<_>>(),
        committed.keys().collect::<Vec<_>>(),
        "{REGENERATE}"
    );
    for (relative, built_path) in &built {
        let committed_path = &committed[relative];
        if relative.ends_with(".webp") {
            let want = guiderecorder_core::library::read_webp(built_path).unwrap();
            let got = guiderecorder_core::library::read_webp(committed_path).unwrap();
            if let Some(diff) = support::image_diff(&want, &got, ImageTolerance::EXACT) {
                panic!("{relative}: {diff}\n{REGENERATE}");
            }
        } else {
            let want = fs::read_to_string(built_path).unwrap();
            let got = fs::read_to_string(committed_path).unwrap();
            assert!(want == got, "{relative} differs\n{REGENERATE}");
        }
    }
}

/// Set to `1` (with `--ignored`) to let [`regenerate_fixtures`] rewrite
/// `fixtures/guides`, so a plain `--ignored` run for other tests can't.
const REGENERATE_ENV: &str = "REGENERATE_FIXTURES";

#[test]
#[ignore = "rewrites fixtures/guides; run with REGENERATE_FIXTURES=1 after changing the builders"]
fn regenerate_fixtures() {
    if std::env::var(REGENERATE_ENV).as_deref() != Ok("1") {
        eprintln!("skipped: set {REGENERATE_ENV}=1 to rewrite fixtures/guides");
        return;
    }
    let dir = support::fixture_guides_dir();
    for bundle in fixture_bundles() {
        fs::remove_dir_all(bundle).unwrap();
    }
    build_fixtures(&dir);
}

const REGENERATE: &str = "fixtures/guides is out of date; run \
    `REGENERATE_FIXTURES=1 cargo test -p guiderecorder-core --test fixtures -- --ignored regenerate_fixtures`";

// ---------------------------------------------------------------------------
// Helpers

/// A temp copy of `fixtures/guides` opened as a library. Tests load and save
/// through this, never the committed bundles: `Library::load` migrates and
/// rewrites older schemas in place.
struct FixtureLibrary {
    library: Library,
    // Removed on drop, after `library`.
    _dir: tempfile::TempDir,
}

impl FixtureLibrary {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for bundle in fixture_bundles() {
            support::copy_dir(&bundle, &dir.path().join(bundle.file_name().unwrap()));
        }
        Self {
            library: Library::open(dir.path()).unwrap(),
            _dir: dir,
        }
    }

    /// The copied bundles, sorted like [`fixture_bundles`].
    fn bundles(&self) -> Vec<PathBuf> {
        bundles_in(self.library.root())
    }

    fn guides(&self) -> Vec<Guide> {
        self.bundles()
            .iter()
            .map(|b| self.library.load(b).unwrap().guide)
            .collect()
    }
}

/// `.guiderecorder` folders in the committed `fixtures/guides`, sorted. Read
/// only; open them through [`FixtureLibrary`].
fn fixture_bundles() -> Vec<PathBuf> {
    bundles_in(&support::fixture_guides_dir())
}

fn bundles_in(dir: &Path) -> Vec<PathBuf> {
    let mut bundles: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "guiderecorder") && p.is_dir())
        .collect();
    bundles.sort();
    bundles
}

/// Every file under the bundles of `root`, keyed by root-relative path.
/// Dotfiles and dot-folders (`.DS_Store`, `.backup/`) are skipped: they
/// are local noise, not fixture content.
fn bundle_files(root: &Path) -> BTreeMap<String, PathBuf> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<String, PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(relative.into_owned(), path);
            }
        }
    }
    let mut out = BTreeMap::new();
    for bundle in bundles_in(root) {
        walk(&bundle, root, &mut out);
    }
    out
}

fn debug(value: impl std::fmt::Debug) -> String {
    format!("{value:?}")
}

fn set(labels: &[&str]) -> BTreeSet<String> {
    labels.iter().map(|l| (*l).to_owned()).collect()
}

/// Every variant of the model enums the fixtures must cover. Each list sits
/// next to a match with no wildcard, so adding a variant to the model breaks
/// the build here until the list is updated.
mod variants {
    use std::collections::BTreeSet;

    use guiderecorder_core::model::*;

    /// Lists the unit variants of `$ty`, checked exhaustive at compile time.
    macro_rules! all_variants {
        ($name:ident, $ty:ident: $($variant:ident),+ $(,)?) => {
            pub fn $name() -> BTreeSet<String> {
                #[allow(dead_code)]
                fn exhaustive(value: $ty) {
                    match value {
                        $($ty::$variant)|+ => {}
                    }
                }
                [$($ty::$variant),+].iter().map(|v| format!("{v:?}")).collect()
            }
        };
    }

    all_variants!(step_kinds, StepKind: Click, DoubleClick, RightClick, Type, Shortcut, Drag, Manual);
    all_variants!(note_kinds, NoteKind: Tip, Warning, Note);
    all_variants!(blur_modes, BlurMode: Blur, Pixelate);
    all_variants!(ai_statuses, AiStatus: Template, Generating, Ai, Edited);
    all_variants!(image_shows, ImageShow: Crop, Full);
    all_variants!(audiences, Audience: Beginner, Expert);
    all_variants!(guide_kinds, GuideKind: Walkthrough, Annotated);

    pub const ITEMS: &[&str] = &["step", "section", "note"];

    pub fn item(item: &Item) -> &'static str {
        match item {
            Item::Step(_) => "step",
            Item::Section(_) => "section",
            Item::Note(_) => "note",
        }
    }

    /// Callouts count twice: with and without an anchor point.
    pub const SHAPES: &[&str] = &[
        "arrow",
        "box",
        "highlight",
        "callout",
        "callout+anchor",
        "blur",
        "crop",
        "hotspot",
    ];

    pub fn shape(shape: &Shape) -> &'static str {
        match shape {
            Shape::Arrow(_) => "arrow",
            Shape::Box(_) => "box",
            Shape::Highlight(_) => "highlight",
            Shape::Callout(c) if c.anchor.is_some() => "callout+anchor",
            Shape::Callout(_) => "callout",
            Shape::Blur(_) => "blur",
            Shape::Crop(_) => "crop",
            Shape::Hotspot(_) => "hotspot",
        }
    }
}

/// Markdown fields, labelled by where they sit in the guide.
fn rich_text_fields(guide: &Guide) -> Vec<(String, &str)> {
    let mut fields = vec![
        ("intro".to_owned(), guide.intro.as_str()),
        ("prerequisites".to_owned(), guide.prerequisites.as_str()),
    ];
    for (i, item) in guide.items.iter().enumerate() {
        match item {
            Item::Step(step) => {
                fields.push((format!("items[{i}].title"), &step.title));
                fields.push((format!("items[{i}].description"), &step.description));
            }
            Item::Note(note) => fields.push((format!("items[{i}].body"), &note.body)),
            Item::Section(_) => {}
        }
    }
    fields.retain(|(_, text)| !text.is_empty());
    fields
}

// ---------------------------------------------------------------------------
// Builders

const FIXTURE_COUNT: usize = 4;

/// Writes every fixture bundle into the library at `root`.
fn build_fixtures(root: &Path) {
    let library = Library::open(root).unwrap();
    for fixture in [calendar(), api_key(), printer(), floor_plan()] {
        fixture.write(&library);
    }
    assert_eq!(bundles_in(root).len(), FIXTURE_COUNT);
}

/// A guide plus the images its steps reference: solid colours, or drawn
/// pictures.
struct Fixture {
    guide: Guide,
    /// Bundle-relative path → (size, RGBA colour).
    images: Vec<(String, (u32, u32), [u8; 4])>,
    /// Bundle-relative path → picture.
    pictures: Vec<(String, RgbaImage)>,
}

impl Fixture {
    fn write(&self, library: &Library) {
        let handle = library.create(&self.guide).unwrap();
        for (relative, (width, height), colour) in &self.images {
            let data = colour.repeat(*width as usize * *height as usize);
            let pixels = PixelBuffer::rgba(*width, *height, &data);
            write_image(&handle, relative, &pixels);
        }
        for (relative, picture) in &self.pictures {
            let pixels = PixelBuffer::rgba(picture.width(), picture.height(), picture.as_raw());
            write_image(&handle, relative, &pixels);
        }
    }
}

fn write_image(handle: &GuideHandle, relative: &str, pixels: &PixelBuffer<'_>) {
    let (dir, file) = relative.split_once('/').unwrap();
    if dir == "steps" {
        // Through the step API, so its paths and layout are what we store.
        let (stem, variant) = match file.trim_end_matches(".webp").split_once('.') {
            None => (file.trim_end_matches(".webp"), StepImageVariant::Full),
            Some((stem, "crop")) => (stem, StepImageVariant::Crop),
            Some((stem, "after")) => (stem, StepImageVariant::After),
            Some(other) => panic!("unexpected step image {other:?}"),
        };
        let step: Ulid = stem.parse().unwrap();
        assert_eq!(
            handle.write_step_image(step, variant, pixels).unwrap(),
            relative
        );
    } else {
        write_webp(&handle.resolve(relative).unwrap(), pixels).unwrap();
    }
}

/// Builds a fixture's items with deterministic ids and image colours.
struct Builder {
    created: DateTime<Utc>,
    /// The guide's id; items count up from it.
    id_base: u128,
    next_id: u128,
    items: Vec<Item>,
    images: Vec<(String, (u32, u32), [u8; 4])>,
    pictures: Vec<(String, RgbaImage)>,
}

impl Builder {
    fn new(created: &str, id_base: u128) -> Self {
        Self {
            created: created.parse().unwrap(),
            id_base,
            next_id: id_base,
            items: Vec::new(),
            images: Vec::new(),
            pictures: Vec::new(),
        }
    }

    fn id(&mut self) -> Ulid {
        self.next_id += 1;
        Ulid::from_parts(self.created.timestamp_millis() as u64, self.next_id)
    }

    fn guide(mut self, title: &str, language: &str, edit: impl FnOnce(&mut Guide)) -> Fixture {
        let mut guide = Guide::new(title, language);
        guide.id = Ulid::from_parts(self.created.timestamp_millis() as u64, self.id_base);
        guide.created_at = self.created;
        guide.updated_at = self.created + chrono::Duration::minutes(25);
        guide.items = std::mem::take(&mut self.items);
        edit(&mut guide);
        Fixture {
            guide,
            images: self.images,
            pictures: self.pictures,
        }
    }

    fn section(&mut self, title: &str) {
        let id = self.id();
        self.items.push(Item::Section(Section {
            id,
            title: title.to_owned(),
        }));
    }

    fn note(&mut self, kind: NoteKind, body: &str) {
        let id = self.id();
        self.items.push(Item::Note(Note {
            id,
            kind,
            body: body.to_owned(),
        }));
    }

    /// A recorded step with a full screenshot and a crop.
    fn step(&mut self, kind: StepKind, title: &str, colour: [u8; 4]) -> &mut Step {
        let id = self.id();
        let full = step_image_path(id, StepImageVariant::Full);
        let crop = step_image_path(id, StepImageVariant::Crop);
        self.images.push((full.clone(), FULL_SIZE, colour));
        self.images.push((crop.clone(), CROP_SIZE, darker(colour)));
        self.push_step(Step {
            id,
            title: title.to_owned(),
            description: String::new(),
            kind,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full,
                crop: Some(crop),
                crop_rect: None,
                after: None,
                show: ImageShow::Crop,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: AiState::default(),
        })
    }

    /// A manual step showing only a full image at `full` (no crop).
    fn manual(&mut self, title: &str, full: Option<&str>, colour: [u8; 4]) -> &mut Step {
        let id = self.id();
        let full = full.map_or_else(
            || step_image_path(id, StepImageVariant::Full),
            str::to_owned,
        );
        self.images.push((full.clone(), FULL_SIZE, colour));
        self.push_step(Step {
            id,
            title: title.to_owned(),
            description: String::new(),
            kind: StepKind::Manual,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full,
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: AiState::default(),
        })
    }

    /// A page of an annotation guide: a manual step showing only `picture`.
    fn page(&mut self, title: &str, picture: RgbaImage) -> &mut Step {
        let id = self.id();
        let full = step_image_path(id, StepImageVariant::Full);
        self.pictures.push((full.clone(), picture));
        self.push_step(Step {
            id,
            title: title.to_owned(),
            description: String::new(),
            kind: StepKind::Manual,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full,
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: AiState::default(),
        })
    }

    fn hotspot(&mut self, area: Rect, title: &str, description: &str) -> Annotation {
        self.annotation(
            Shape::Hotspot(HotspotGeometry {
                rect: area,
                title: title.to_owned(),
                description: description.to_owned(),
            }),
            AnnotationStyle::default(),
        )
    }

    fn push_step(&mut self, step: Step) -> &mut Step {
        self.items.push(Item::Step(step));
        match self.items.last_mut() {
            Some(Item::Step(step)) => step,
            _ => unreachable!(),
        }
    }

    /// Adds an after-action frame to the last step.
    fn with_after(&mut self, colour: [u8; 4]) {
        let Some(Item::Step(step)) = self.items.last_mut() else {
            panic!("no step to add an after frame to");
        };
        let after = step_image_path(step.id, StepImageVariant::After);
        step.image.after = Some(after.clone());
        self.images.push((after, FULL_SIZE, colour));
    }

    fn annotation(&mut self, shape: Shape, style: AnnotationStyle) -> Annotation {
        Annotation {
            id: self.id(),
            shape,
            style,
        }
    }
}

fn darker([r, g, b, a]: [u8; 4]) -> [u8; 4] {
    [r / 2, g / 2, b / 2, a]
}

fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn element(
    role: &str,
    label: &str,
    app: &str,
    bundle_id: &str,
    window: Option<&str>,
) -> ElementInfo {
    ElementInfo {
        role: Some(role.to_owned()),
        label: Some(label.to_owned()),
        app: Some(app.to_owned()),
        bundle_id: Some(bundle_id.to_owned()),
        window: window.map(str::to_owned),
    }
}

fn ai(status: AiStatus, prompt: &str) -> AiState {
    AiState {
        status,
        model: Some(STEP_MODEL.to_owned()),
        prompt_version: Some(prompt.to_owned()),
        ..AiState::default()
    }
}

fn red() -> AnnotationStyle {
    AnnotationStyle {
        color: Some("#ff3b30".to_owned()),
        stroke_width: Some(2.0),
        ..Default::default()
    }
}

fn recording(apps: &[(&str, &str, &str)]) -> RecordingInfo {
    RecordingInfo {
        app_version: "0.1.0".to_owned(),
        os_version: Some("macOS 26.1".to_owned()),
        apps: apps
            .iter()
            .map(|(name, bundle_id, version)| AppInfo {
                name: (*name).to_owned(),
                bundle_id: Some((*bundle_id).to_owned()),
                version: Some((*version).to_owned()),
            })
            .collect(),
        displays: vec![DisplayInfo {
            id: 1,
            name: Some("Built-in Retina Display".to_owned()),
            width: 3024,
            height: 1964,
            scale: 2.0,
        }],
    }
}

/// Recorded beginner guide: sections, click / double-click / type /
/// right-click / shortcut steps, an after frame, arrow, box, highlight and
/// callouts (with and without anchor), tip and plain notes, and AI states
/// `ai`, `edited`, `template` and `generating`.
fn calendar() -> Fixture {
    const CAL: &str = "com.apple.iCal";
    let mut b = Builder::new("2026-10-14T09:00:00Z", 0x0A00);
    b.section("Open the event");

    let arrow = b.annotation(
        Shape::Arrow(ArrowGeometry {
            from: point(4.0, 4.0),
            to: point(14.0, 10.0),
        }),
        red(),
    );
    let boxed = b.annotation(Shape::Box(rect(12.0, 6.0, 16.0, 10.0)), red());
    let step = b.step(
        StepKind::Click,
        "Click **Calendar** in the Dock",
        [0x34, 0x7a, 0xf6, 0xff],
    );
    step.point = Some(point(30.0, 36.0));
    step.element = Some(element(
        "AXDockItem",
        "Calendar",
        "Dock",
        "com.apple.dock",
        None,
    ));
    step.annotations = vec![arrow, boxed];
    step.ai_state = ai(AiStatus::Ai, "step.v1");
    b.with_after([0xf2, 0xf2, 0xf7, 0xff]);

    let highlight = b.annotation(
        Shape::Highlight(rect(2.0, 6.0, 28.0, 6.0)),
        AnnotationStyle {
            fill: Some("#ffcc00".to_owned()),
            opacity: Some(0.4),
            ..Default::default()
        },
    );
    let callout = b.annotation(
        Shape::Callout(CalloutGeometry {
            rect: rect(36.0, 2.0, 24.0, 10.0),
            anchor: Some(point(20.0, 12.0)),
            text: "Your event".to_owned(),
        }),
        AnnotationStyle {
            font_size: Some(6.0),
            ..red()
        },
    );
    let step = b.step(
        StepKind::DoubleClick,
        "Double-click the *Team sync* event",
        [0xff, 0x95, 0x00, 0xff],
    );
    step.description = "The event opens in its own window.".to_owned();
    step.point = Some(point(22.0, 14.0));
    step.element = Some(element(
        "AXStaticText",
        "Team sync",
        "Calendar",
        CAL,
        Some("Calendar"),
    ));
    step.annotations = vec![highlight, callout];
    step.image.show = ImageShow::Full;
    step.ai_state = ai(AiStatus::Edited, "step.v1");

    b.note(
        NoteKind::Tip,
        "You can also select the event and press `⌘O` to open it.",
    );
    b.section("Invite people");

    let step = b.step(
        StepKind::Type,
        "Type the guest's email in **Add Invitees**",
        [0x34, 0xc7, 0x59, 0xff],
    );
    step.description =
        "Separate several guests with commas.\n\n- Guests get an email invite\n- You can remove them later\n"
            .to_owned();
    step.point = Some(point(40.0, 20.0));
    step.element = Some(element(
        "AXTextField",
        "Add Invitees",
        "Calendar",
        CAL,
        Some("Team sync"),
    ));
    step.keystrokes = Some("alex@example.com⏎".to_owned());

    let callout = b.annotation(
        Shape::Callout(CalloutGeometry {
            rect: rect(4.0, 26.0, 30.0, 10.0),
            anchor: None,
            text: "Opens a menu".to_owned(),
        }),
        AnnotationStyle::default(),
    );
    let step = b.step(
        StepKind::RightClick,
        "Right-click the guest and choose **Send Email**",
        [0xaf, 0x52, 0xde, 0xff],
    );
    step.point = Some(point(40.0, 24.0));
    step.element = Some(element(
        "AXStaticText",
        "alex@example.com",
        "Calendar",
        CAL,
        Some("Team sync"),
    ));
    step.annotations = vec![callout];
    step.ai_state = ai(AiStatus::Ai, "step.v1");

    let step = b.step(
        StepKind::Shortcut,
        "Press `⌘S` to save",
        [0x8e, 0x8e, 0x93, 0xff],
    );
    step.keystrokes = Some("⌘S".to_owned());
    step.ai_state = AiState {
        status: AiStatus::Generating,
        ..AiState::default()
    };

    b.note(
        NoteKind::Note,
        "Guests see changes after they [refresh their calendar](https://support.apple.com/guide/calendar/welcome/mac).",
    );

    b.guide("Share a calendar event", "en", |g| {
        g.summary = "Invite a colleague to an existing Calendar event.".to_owned();
        g.tags = vec!["calendar".to_owned(), "macos".to_owned()];
        g.intro = "This guide shows how to invite someone to an event you **already** created."
            .to_owned();
        g.prerequisites = "- A Mac signed in to iCloud\n- An existing event\n".to_owned();
        g.recording = Some(recording(&[
            ("Dock", "com.apple.dock", "1.8"),
            ("Calendar", CAL, "15.0"),
        ]));
    })
}

/// Recorded expert guide with secrets: secure typing (masked keystrokes),
/// blur and pixelate, a crop annotation, a drag with an end point, a
/// warning note and a manual step.
fn api_key() -> Fixture {
    const SAFARI: &str = "com.apple.Safari";
    let mut b = Builder::new("2026-10-14T10:30:00Z", 0x0B00);

    b.note(
        NoteKind::Warning,
        "Creating a new key **revokes** the old one immediately. Update these first:\n\n1. The CI secret\n2. Local `.env` files\n",
    );

    let step = b.step(
        StepKind::Click,
        "Open **Settings → API keys**",
        [0x5a, 0xc8, 0xfa, 0xff],
    );
    step.point = Some(point(10.0, 8.0));
    step.element = Some(element(
        "AXLink",
        "API keys",
        "Safari",
        SAFARI,
        Some("Settings – Example Console"),
    ));
    step.ai_state = ai(AiStatus::Ai, "step.v1");

    let blur = b.annotation(
        Shape::Blur(rect(4.0, 8.0, 24.0, 6.0)),
        AnnotationStyle {
            blur_mode: Some(BlurMode::Blur),
            blur_amount: Some(8.0),
            ..Default::default()
        },
    );
    let step = b.step(
        StepKind::Type,
        "Enter your password to confirm",
        [0xff, 0x2d, 0x55, 0xff],
    );
    step.description = "Password input is masked while recording.".to_owned();
    step.point = Some(point(16.0, 10.0));
    step.element = Some(element(
        "AXSecureTextField",
        "Password",
        "Safari",
        SAFARI,
        Some("Settings – Example Console"),
    ));
    step.keystrokes = Some("••••••••⏎".to_owned());
    step.annotations = vec![blur];
    step.ai_state = ai(AiStatus::Ai, "step.v1");

    let pixelate = b.annotation(
        Shape::Blur(rect(8.0, 18.0, 48.0, 6.0)),
        AnnotationStyle {
            blur_mode: Some(BlurMode::Pixelate),
            blur_amount: Some(6.0),
            ..Default::default()
        },
    );
    let crop = b.annotation(
        Shape::Crop(rect(0.0, 10.0, 64.0, 20.0)),
        AnnotationStyle::default(),
    );
    let boxed = b.annotation(Shape::Box(rect(40.0, 28.0, 20.0, 8.0)), red());
    let step = b.step(
        StepKind::Click,
        "Click **Create key** and copy the new key",
        [0x00, 0xc7, 0xbe, 0xff],
    );
    step.description =
        "The key is shown **once**. Store it in your password manager; see [Managing keys](https://example.com/docs/keys)."
            .to_owned();
    step.point = Some(point(50.0, 32.0));
    step.element = Some(element(
        "AXButton",
        "Create key",
        "Safari",
        SAFARI,
        Some("Settings – Example Console"),
    ));
    step.image.show = ImageShow::Full;
    step.annotations = vec![pixelate, crop, boxed];
    step.ai_state = ai(AiStatus::Edited, "step.v1");
    b.with_after([0x00, 0x63, 0x5f, 0xff]);

    let arrow = b.annotation(
        Shape::Arrow(ArrowGeometry {
            from: point(10.0, 10.0),
            to: point(50.0, 30.0),
        }),
        red(),
    );
    let step = b.step(
        StepKind::Drag,
        "Drag the new key to **Production**",
        [0x58, 0x56, 0xd6, 0xff],
    );
    step.point = Some(point(10.0, 10.0));
    step.end_point = Some(point(50.0, 30.0));
    step.element = Some(element(
        "AXRow",
        "New key",
        "Safari",
        SAFARI,
        Some("Settings – Example Console"),
    ));
    step.image.show = ImageShow::Full;
    step.annotations = vec![arrow];

    let step = b.manual("Check that the key works", None, [0x1c, 0x1c, 0x1e, 0xff]);
    step.description =
        "Run this in Terminal:\n\n`curl -H \"x-api-key: $KEY\" https://api.example.com/v1/ping`\n\nIt should print *pong*."
            .to_owned();
    step.ai_state = ai(AiStatus::Ai, "step.v1");

    b.guide("Rotate an API key", "en", |g| {
        g.summary = "Replace a leaked or expiring API key without downtime.".to_owned();
        g.tags = vec![
            "security".to_owned(),
            "api".to_owned(),
            "console".to_owned(),
        ];
        g.audience = Audience::Expert;
        g.intro = "Rotate keys every *90 days*, or right away if one leaks.".to_owned();
        g.prerequisites = "Admin access to the console.".to_owned();
        g.recording = Some(recording(&[("Safari", SAFARI, "26.1")]));
    })
}

/// Hand-built German guide: no recording, no tags, one manual step whose
/// image is an inserted asset, and Markdown that `normalize` rewrites.
fn printer() -> Fixture {
    let mut b = Builder::new("2026-10-13T16:00:00Z", 0x0C00);
    let step = b.manual(
        "Papierfach **2** öffnen",
        Some("assets/drucker-fach.webp"),
        [0xe5, 0xe5, 0xea, 0xff],
    );
    step.description =
        "Fach ganz herausziehen.  \nDann Papier einlegen.\n\n* A4\n* A5\n\n# Hinweis\n\nNicht __überfüllen__."
            .to_owned();
    b.note(
        NoteKind::Note,
        "Mehr dazu im [Handbuch](handbuch.pdf) und unter <https://example.com/drucker>.",
    );
    b.guide("Kurzanleitung Drucker", "de", |g| {
        g.intro = "Für den Drucker im **2. Stock**.".to_owned();
    })
}

/// Annotation guide: two pages of a drawn floor plan explained by
/// hotspots, one with a multi-line description, one with text that needs
/// escaping in every format, one with its own colour, and a tip between the
/// pages.
fn floor_plan() -> Fixture {
    let mut b = Builder::new("2026-10-15T08:00:00Z", 0x0D00);

    let reception = b.hotspot(
        rect(24.0, 24.0, 140.0, 92.0),
        "Reception",
        "Sign in here when you arrive.\nVisitor badges are in the top drawer.",
    );
    let kitchen = b.hotspot(
        rect(176.0, 24.0, 136.0, 92.0),
        "Kitchen & café",
        "Coffee, tea and fruit. Please load the dishwasher before you leave.",
    );
    let meeting = b.hotspot(
        rect(324.0, 24.0, 132.0, 132.0),
        "Meeting room \"Fjord\"",
        "Seats 8. Book it on the panel by the door; to present, press <Input> on the remote.",
    );
    let quiet = b.hotspot(
        rect(24.0, 176.0, 208.0, 100.0),
        "Quiet zone",
        "No calls or meetings.",
    );
    let step = b.page("Third floor", floor_plan_picture(false));
    step.description = "The lifts open onto the corridor in the middle.".to_owned();
    step.annotations = vec![reception, kitchen, meeting, quiet];

    b.note(
        NoteKind::Tip,
        "Need a desk for the day? Ask at **Reception**.",
    );

    let mut stairs = b.hotspot(
        rect(436.0, 176.0, 28.0, 56.0),
        "Stairwell B",
        "Use the stairs in a fire, never the lifts.",
    );
    stairs.style.color = Some("#16a34a".to_owned());
    let assembly = b.hotspot(
        rect(16.0, 236.0, 28.0, 48.0),
        "Stairwell A",
        "Leads to the assembly point in the car park.",
    );
    let step = b.page("Fire exits", floor_plan_picture(true));
    step.annotations = vec![stairs, assembly];

    b.guide("Office floor plan", "en", |g| {
        g.kind = GuideKind::Annotated;
        g.summary = "Find your way around the third floor.".to_owned();
        g.tags = vec!["office".to_owned()];
        g.intro = "Hover over or tap a numbered area to see what is there.".to_owned();
    })
}

/// A simple floor plan, [`PAGE_SIZE`]: rooms on paper-white with dark
/// walls, desks in the open area and, with `exits`, green fire exits.
fn floor_plan_picture(exits: bool) -> RgbaImage {
    const PAPER: [u8; 4] = [0xf8, 0xf7, 0xf4, 0xff];
    const WALL: [u8; 4] = [0x3a, 0x3a, 0x3c, 0xff];
    const DESK: [u8; 4] = [0xcb, 0xd5, 0xe1, 0xff];
    const EXIT: [u8; 4] = [0x16, 0xa3, 0x4a, 0xff];
    // (x, y, width, height, colour), painted in order.
    let mut shapes: Vec<(u32, u32, u32, u32, [u8; 4])> = vec![
        // Outer wall, then the floor inside it.
        (8, 8, 464, 284, WALL),
        (14, 14, 452, 272, PAPER),
        // Rooms: wall, then tinted floor.
        (14, 14, 158, 108, WALL),
        (14, 14, 154, 104, [0xdb, 0xea, 0xfe, 0xff]),
        (168, 14, 152, 108, WALL),
        (172, 14, 144, 104, [0xfe, 0xf3, 0xc7, 0xff]),
        (316, 14, 150, 148, WALL),
        (320, 14, 146, 144, [0xdc, 0xfc, 0xe7, 0xff]),
        (14, 168, 226, 118, WALL),
        (14, 172, 222, 114, [0xed, 0xe9, 0xfe, 0xff]),
        // Doors onto the corridor.
        (70, 118, 36, 4, PAPER),
        (226, 118, 36, 4, PAPER),
        (370, 158, 36, 4, PAPER),
        (110, 168, 36, 4, PAPER),
        // Lifts.
        (250, 132, 24, 28, WALL),
        (278, 132, 24, 28, WALL),
    ];
    // Open-plan desks.
    for row in 0..2 {
        for col in 0..4 {
            shapes.push((260 + col * 46, 186 + row * 50, 34, 22, DESK));
        }
    }
    if exits {
        shapes.push((436, 176, 28, 56, EXIT));
        shapes.push((16, 236, 28, 48, EXIT));
    }
    let (w, h) = PAGE_SIZE;
    let mut image = RgbaImage::from_pixel(w, h, Rgba(PAPER));
    for (x, y, width, height, colour) in shapes {
        for py in y..(y + height).min(h) {
            for px in x..(x + width).min(w) {
                image.put_pixel(px, py, Rgba(colour));
            }
        }
    }
    image
}
