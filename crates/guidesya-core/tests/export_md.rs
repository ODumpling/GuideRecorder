//! Markdown export (#352, design #443).
//!
//! Goldens are under `tests/golden/md/`, one per fixture guide; regenerate
//! them with `UPDATE_GOLDEN=1 cargo test -p guidesya-core --test export_md`
//! and review the diff.

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use guidesya_core::export::ast::{parse_inlines, plain_text};
use guidesya_core::export::md::{self, ExportError, MdOptions};
use guidesya_core::export::{ExportReport, RenderGuide, RenderWarning};
use guidesya_core::library::{GuideHandle, Library, PixelBuffer, StepImageVariant};
use guidesya_core::model::*;
use guidesya_core::Ulid;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
use resvg::tiny_skia::Pixmap;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

// ---------------------------------------------------------------------------
// Helpers

/// A temp copy of the fixture guides, opened read-only.
struct Fixtures {
    _dir: tempfile::TempDir,
    guides: Vec<(GuideHandle, Guide)>,
}

fn fixtures() -> Fixtures {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture_guides_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "guidesya") {
            copy_dir(&path, &dir.path().join(path.file_name().unwrap()));
        }
    }
    let library = Library::open(dir.path()).unwrap();
    let mut guides: Vec<_> = library
        .list()
        .unwrap()
        .into_iter()
        .map(|summary| {
            let loaded = library.read(&summary.path).unwrap();
            (loaded.handle, loaded.guide)
        })
        .collect();
    guides.sort_by(|a, b| a.1.title.cmp(&b.1.title));
    assert!(!guides.is_empty());
    Fixtures { _dir: dir, guides }
}

fn export(guide: &Guide, handle: &GuideHandle, out: &Path, opts: &MdOptions) -> ExportReport {
    let render = RenderGuide::new(guide, handle);
    md::export(&render, handle, out, opts).unwrap()
}

fn relative(out: &Path, path: &Path) -> String {
    path.strip_prefix(out)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

fn decode_png(path: &Path) -> Pixmap {
    Pixmap::decode_png(&fs::read(path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Image destinations in `markdown`.
fn image_urls(markdown: &str) -> Vec<String> {
    Parser::new_ext(markdown, Options::empty())
        .filter_map(|event| match event {
            Event::Start(Tag::Image { dest_url, .. }) => Some(dest_url.into_string()),
            _ => None,
        })
        .collect()
}

/// Text of every heading in `markdown`, in order.
fn headings(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for event in Parser::new_ext(markdown, Options::empty()) {
        match event {
            Event::Start(Tag::Heading { .. }) => current = Some(String::new()),
            Event::End(TagEnd::Heading(_)) => out.extend(current.take()),
            Event::Text(text) | Event::Code(text) => {
                if let Some(current) = &mut current {
                    current.push_str(&text);
                }
            }
            _ => {}
        }
    }
    out
}

/// Plain text of every paragraph in `markdown` (soft breaks as spaces).
fn paragraphs(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for event in Parser::new_ext(markdown, Options::empty()) {
        match event {
            Event::Start(Tag::Paragraph) => current = Some(String::new()),
            Event::End(TagEnd::Paragraph) => out.extend(current.take()),
            Event::Text(text) | Event::Code(text) => {
                if let Some(current) = &mut current {
                    current.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(current) = &mut current {
                    current.push('\n');
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Fixtures

#[test]
fn fixture_goldens() {
    let fixtures = fixtures();
    for (handle, guide) in &fixtures.guides {
        let out = tempfile::tempdir().unwrap();
        let report = export(guide, handle, out.path(), &MdOptions::default());

        let markdown = fs::read_to_string(out.path().join(md::GUIDE_FILE)).unwrap();
        let mut golden = markdown.clone();
        golden.push_str("\n<!-- files -->\n");
        for file in &report.files {
            golden.push_str(&format!("{}\n", relative(out.path(), file)));
        }
        golden.push_str("<!-- warnings -->\n");
        for (id, warning) in &report.warnings {
            golden.push_str(&format!("{id} {warning}\n"));
        }
        let name = format!("md/{}.golden", md::slug(&guide.title));
        assert_golden_text(&name, &golden);
    }
}

#[test]
fn markdown_images_resolve_to_written_files() {
    let fixtures = fixtures();
    for (handle, guide) in &fixtures.guides {
        let out = tempfile::tempdir().unwrap();
        let report = export(guide, handle, out.path(), &MdOptions::default());
        assert_eq!(report.files[0], out.path().join("guide.md"));

        let markdown = fs::read_to_string(&report.files[0]).unwrap();
        let urls = image_urls(&markdown);
        assert_eq!(urls.len(), guide.steps().count(), "{}", guide.title);
        let written: Vec<String> = report.files[1..]
            .iter()
            .map(|f| relative(out.path(), f))
            .collect();
        assert_eq!(urls, written, "{}", guide.title);
        for url in &urls {
            assert!(url.starts_with("images/") && url.ends_with(".png"), "{url}");
            decode_png(&out.path().join(url));
        }
        // Nothing else was written.
        let on_disk = fs::read_dir(out.path().join("images")).unwrap().count();
        assert_eq!(on_disk, urls.len());
    }
}

#[test]
fn output_is_deterministic() {
    let fixtures = fixtures();
    let (handle, guide) = &fixtures.guides[0];
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let ra = export(guide, handle, a.path(), &MdOptions::default());
    let rb = export(guide, handle, b.path(), &MdOptions::default());
    assert_eq!(ra.files.len(), rb.files.len());
    for (fa, fb) in ra.files.iter().zip(&rb.files) {
        assert_eq!(relative(a.path(), fa), relative(b.path(), fb));
        assert_eq!(
            fs::read(fa).unwrap(),
            fs::read(fb).unwrap(),
            "{}",
            fa.display()
        );
    }
}

// ---------------------------------------------------------------------------
// Output folder

#[test]
fn refuses_non_empty_folder_unless_overwrite() {
    let fixtures = fixtures();
    let (handle, guide) = fixtures
        .guides
        .iter()
        .find(|(_, g)| g.steps().count() >= 2)
        .unwrap();
    let render = RenderGuide::new(guide, handle);
    let out = tempfile::tempdir().unwrap();
    fs::write(out.path().join("mine.txt"), "keep").unwrap();

    let err = md::export(&render, handle, out.path(), &MdOptions::default()).unwrap_err();
    assert!(matches!(err, ExportError::NotEmpty(_)), "{err}");
    assert!(!out.path().join("guide.md").exists());

    // An earlier export with an extra step, plus a user file in images/.
    let images = out.path().join("images");
    fs::create_dir_all(&images).unwrap();
    fs::write(images.join("99-old-step.png"), "stale").unwrap();
    fs::write(images.join("logo.png"), "keep").unwrap();

    let opts = MdOptions {
        overwrite: true,
        ..MdOptions::default()
    };
    let report = md::export(&render, handle, out.path(), &opts).unwrap();
    assert!(!images.join("99-old-step.png").exists());
    assert!(images.join("logo.png").exists());
    assert!(out.path().join("mine.txt").exists());
    assert_eq!(report.files.len(), guide.steps().count() + 1);

    // A missing folder is created.
    let nested = out.path().join("a/b");
    md::export(&render, handle, &nested, &MdOptions::default()).unwrap();
    assert!(nested.join("guide.md").is_file());
}

// ---------------------------------------------------------------------------
// Synthetic guides: escaping and placeholders

const HOSTILE: &[&str] = &[
    "<script>alert(1)</script>",
    "\"><img src=x onerror=alert(1)>",
    "# not a heading",
    "1. not a list",
    "- not a bullet",
    "> not a quote",
    "***",
    "===",
    "<https://evil.example/autolink>",
    "&lt;b&gt; &amp; &#60;",
    "*not emphasis* _nor this_ ~~nor this~~",
    "|a|b|",
    "back\\slash \\* \\",
    "[text](https://evil.example) ![img](x.png)",
    "trailing hashes ##",
    "{{ 7*7 }} {% raw %}",
    "</style><style>body{display:none}",
    // `!` before a link must not make an image (a remote load on open).
    "a\\![x](https://evil.example/p.png)",
    "Done&#33;[y](https://evil.example/u.png)",
    "Wow![z](https://evil.example/s.png) *em*![w](https://evil.example/w.png)",
    "costs $5, or $x$ and $$y$$ as math",
];

/// A guide whose every text field is hostile, with steps whose images are
/// missing (placeholders) except one real screenshot.
fn hostile_guide(library: &Library) -> (GuideHandle, Guide) {
    let mut guide = Guide::new(HOSTILE[0], "en");
    guide.summary = HOSTILE[1].into();
    guide.intro = format!("{}\n\n{}", HOSTILE[2], HOSTILE[3]);
    guide.prerequisites = "- [bad](javascript:alert(1))\n- <b>raw</b>".into();
    let handle = library.create(&guide).unwrap();

    let real = Ulid::new();
    let pixels = vec![120u8; 40 * 20 * 4];
    let full = handle
        .write_step_image(
            real,
            StepImageVariant::Full,
            &PixelBuffer::rgba(40, 20, &pixels),
        )
        .unwrap();

    let mut items = Vec::new();
    for (i, text) in HOSTILE.iter().enumerate() {
        if i % 4 == 0 {
            items.push(Item::Section(Section {
                id: Ulid::new(),
                title: (*text).into(),
            }));
        }
        let id = if i == 0 { real } else { Ulid::new() };
        items.push(Item::Step(Step {
            id,
            title: (*text).into(),
            description: format!("{text}\n\nsee [link](https://example.com/a(b)\"c \"t\\\"x\")"),
            kind: StepKind::Click,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full: if i == 0 {
                    full.clone()
                } else {
                    format!("steps/{id}.webp")
                },
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            ai_state: AiState::default(),
        }));
        items.push(Item::Note(Note {
            id: Ulid::new(),
            kind: [NoteKind::Tip, NoteKind::Warning, NoteKind::Note][i % 3],
            body: (*text).into(),
        }));
    }
    guide.items = items;
    library.save_to(&handle, &guide).unwrap();
    (handle, guide)
}

#[test]
fn hostile_text_is_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::open(dir.path().join("lib")).unwrap();
    let (handle, guide) = hostile_guide(&library);
    let out = dir.path().join("out");
    export(&guide, &handle, &out, &MdOptions::default());
    let markdown = fs::read_to_string(out.join("guide.md")).unwrap();

    // No raw HTML, autolinks or unsafe links under CommonMark or GFM.
    for options in [Options::empty(), Options::all()] {
        for event in Parser::new_ext(&markdown, options) {
            match event {
                Event::Html(html) | Event::InlineHtml(html) => panic!("raw html {html:?}"),
                Event::InlineMath(math) | Event::DisplayMath(math) => panic!("math {math:?}"),
                Event::Start(Tag::Image { dest_url, .. }) => {
                    assert!(dest_url.starts_with("images/"), "remote image {dest_url:?}");
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    // Only the source's own http(s) links survive.
                    assert!(
                        dest_url.starts_with("https://"),
                        "unexpected link {dest_url:?}"
                    );
                }
                Event::Start(Tag::Table(_) | Tag::Strikethrough | Tag::BlockQuote(Some(_))) => {
                    panic!("unexpected GFM markup in\n{markdown}")
                }
                _ => {}
            }
        }
    }

    // Plain-text fields round-trip exactly.
    let headings = headings(&markdown);
    assert_eq!(headings[0], HOSTILE[0]);
    let mut step = 0;
    for item in &guide.items {
        match item {
            Item::Section(section) => {
                assert!(headings.contains(&section.title), "{}", section.title)
            }
            Item::Step(s) => {
                step += 1;
                let title = plain_text(&parse_inlines(&s.title));
                // An empty title (e.g. "***", a thematic break) falls back
                // to the label.
                let title = if title.is_empty() {
                    "Step".to_owned()
                } else {
                    title
                };
                let expected = format!("{step}. {title}");
                assert!(
                    headings.contains(&expected),
                    "{expected:?} not in {headings:#?}"
                );
            }
            Item::Note(_) => {}
        }
    }
    let paragraphs = paragraphs(&markdown);
    assert_eq!(paragraphs[0], HOSTILE[1]);
    // Step descriptions keep their text, link included.
    for text in HOSTILE {
        let normalized = plain_text(&parse_inlines(text));
        if normalized.is_empty() {
            continue;
        }
        assert!(
            paragraphs.iter().any(|p| p == &normalized),
            "{normalized:?} not found"
        );
    }
    assert!(paragraphs.iter().any(|p| p == "see link"));
}

#[test]
fn placeholders_and_warnings_land_in_report() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::open(dir.path().join("lib")).unwrap();
    let (handle, guide) = hostile_guide(&library);
    let out = dir.path().join("out");
    let report = export(&guide, &handle, &out, &MdOptions::default());

    let steps: Vec<&Step> = guide.steps().collect();
    // Every step but the first has a missing image.
    assert_eq!(report.warnings.len(), steps.len() - 1);
    for ((id, warning), step) in report.warnings.iter().zip(&steps[1..]) {
        assert_eq!(*id, step.id);
        assert!(
            matches!(warning, RenderWarning::ImageUnavailable { path, .. } if *path == step.image.full),
            "{warning:?}"
        );
    }
    // Placeholders are still written and referenced.
    let images: Vec<PathBuf> = report.files[1..].to_vec();
    assert_eq!(images.len(), steps.len());
    let real = decode_png(&images[0]);
    assert_eq!((real.width(), real.height()), (40, 20));
    let placeholder = decode_png(&images[1]);
    assert_eq!((placeholder.width(), placeholder.height()), (800, 450));
}

#[test]
fn empty_titles_use_no_heading_or_label_only() {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::open(dir.path().join("lib")).unwrap();
    let mut guide = Guide::new("", "en");
    let handle = library.create(&guide).unwrap();
    let id = Ulid::new();
    guide.items = vec![Item::Step(Step {
        id,
        title: "***".into(),
        description: String::new(),
        kind: StepKind::Manual,
        point: None,
        end_point: None,
        element: None,
        keystrokes: None,
        image: StepImage {
            full: format!("steps/{id}.webp"),
            crop: None,
            crop_rect: None,
            after: None,
            show: ImageShow::Full,
        },
        annotations: Vec::new(),
        ai_state: AiState::default(),
    })];
    library.save_to(&handle, &guide).unwrap();
    let out = dir.path().join("out");
    export(&guide, &handle, &out, &MdOptions::default());
    let markdown = fs::read_to_string(out.join("guide.md")).unwrap();
    assert!(markdown.starts_with("## 1. Step\n"), "{markdown}");
    assert_eq!(headings(&markdown), ["1. Step"]);
}
