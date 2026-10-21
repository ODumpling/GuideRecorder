//! DOCX export (#365, design #443): package structure and text extraction.
//!
//! The text goldens under `tests/golden/docx/` are a dump of
//! `word/document.xml`, one line per paragraph:
//!
//! ```text
//! Heading2 | 1. Click **Calendar** in the Dock
//! Normal shd=E8F5E9 | **Tip: **You can also ... press `⌘O` to open it.
//! Normal num=3/0 | First item
//! ```
//!
//! with the paragraph style, list numbering (`numId/ilvl`) and shading fill,
//! then the text with run formatting marked Markdown-style (`**bold**`,
//! `*italic*`, `` `code` ``), hyperlinks as `[text](target)` resolved through
//! the relationships, line breaks as `↵` and images as `[image WxH]` (the
//! displayed size in EMU). Regenerate with
//! `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core --test export_docx`.
//!
//! docx-rs stores the package uncompressed, so a tiny zip reader suffices.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use guiderecorder_core::export::docx::{
    self, Branding, CheckedBranding, DocxError, DocxOptions, Paper,
};
use guiderecorder_core::export::md::Labels;
use guiderecorder_core::export::{ExportReport, RenderGuide, RenderWarning};
use guiderecorder_core::library::{GuideHandle, Library, LoadedGuide};
use guiderecorder_core::model::*;
use guiderecorder_core::Ulid;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

// ---------------------------------------------------------------------------
// Tests

#[test]
fn fixtures_match_text_goldens() {
    let fixtures = Fixtures::new();
    for (name, loaded) in fixtures.guides() {
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let out = fixtures.out(&format!("{name}.docx"));
        let opts = DocxOptions {
            timestamp: Some(loaded.guide.updated_at),
            ..DocxOptions::default()
        };
        let report = docx::export(&render, &loaded.handle, &out, &opts).unwrap();
        assert_eq!(report.files, std::slice::from_ref(&out));
        let package = Package::open(&out);
        assert_golden_text(
            &format!("docx/{}.golden", slug(&name)),
            &package.text_dump(),
        );
    }
}

/// A valid 1×1 RGBA PNG.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0x60, 0xf8, 0x5f,
    0x0f, 0x00, 0x02, 0x87, 0x01, 0x80, 0xeb, 0x47, 0xba, 0x92, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

fn branding() -> CheckedBranding {
    Branding {
        logo: Some(PNG_1X1.to_vec()),
        accent: Some("#aa1122".into()),
    }
    .check()
    .unwrap()
}

/// A 1900×40 PNG logo, wider than the text width at 96 dpi.
fn wide_logo() -> CheckedBranding {
    let mut wide = Vec::new();
    image::RgbaImage::from_pixel(1900, 40, image::Rgba([0, 0, 0, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut wide),
            image::ImageFormat::Png,
        )
        .unwrap();
    Branding {
        logo: Some(wide),
        accent: Some("#aa1122".into()),
    }
    .check()
    .unwrap()
}

/// The text dump plus the branding-relevant formatting: style colours and
/// the note border colours, in document order.
fn branded_dump(package: &Package) -> String {
    let mut out = package.text_dump();
    out += "=== styles ===\n";
    let styles = package.text("word/styles.xml");
    let mut style = None;
    for (name, tag) in tags(&styles) {
        match name {
            "w:style" => style = attr(tag, "w:styleId"),
            "w:color" => {
                if let (Some(style), Some(colour)) = (&style, attr(tag, "w:val")) {
                    out += &format!("{style} color={colour}\n");
                }
            }
            _ => {}
        }
    }
    out += "=== note borders ===\n";
    let doc = package.text("word/document.xml");
    for (name, tag) in tags(&doc) {
        if name == "w:left" {
            if let Some(colour) = attr(tag, "w:color") {
                out += &format!("{colour}\n");
            }
        }
    }
    out
}

#[test]
fn branded_fixtures_match_text_goldens() {
    let fixtures = Fixtures::new();
    for (name, loaded) in fixtures.guides() {
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let out = fixtures.out(&format!("{name}.branded.docx"));
        let opts = DocxOptions {
            timestamp: Some(loaded.guide.updated_at),
            branding: branding(),
            ..DocxOptions::default()
        };
        docx::export(&render, &loaded.handle, &out, &opts).unwrap();
        let package = Package::open(&out);
        let dump = branded_dump(&package);
        // The logo paragraph comes first, at its 96 dpi size (1 px).
        assert!(
            dump.starts_with("Normal | [image 9525x9525]\nTitle | "),
            "{dump}"
        );
        assert_golden_text(&format!("docx/branded/{}.golden", slug(&name)), &dump);
    }

    // A wide logo is scaled to the text width: A4 text width is 9026 twips
    // = 5731510 EMU, and 40 px of 1900 at that width is 120663 EMU.
    let (name, loaded) = fixtures.guide("Share a calendar event");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out(&format!("{name}.wide.docx"));
    let opts = DocxOptions {
        timestamp: Some(loaded.guide.updated_at),
        branding: wide_logo(),
        ..DocxOptions::default()
    };
    docx::export(&render, &loaded.handle, &out, &opts).unwrap();
    let dump = branded_dump(&Package::open(&out));
    assert!(
        dump.starts_with("Normal | [image 5731510x120663]\nTitle | "),
        "{dump}"
    );
    assert_golden_text("docx/branded/wide-logo.golden", &dump);
}

#[test]
fn branding_is_off_by_default() {
    let synthetic = Synthetic::new();
    let mut guide = Guide::new("Plain", "en");
    guide.items = vec![
        Item::Section(Section {
            id: Ulid::new(),
            title: "Part".into(),
        }),
        step("Click A"),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Note,
            body: "Hello".into(),
        }),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Tip,
            body: "Hi".into(),
        }),
    ];
    let plain = branded_dump(&synthetic.export(&guide));
    assert!(!plain.contains("Heading1 color="), "{plain}");
    assert!(
        plain.ends_with("=== note borders ===\n1A73E8\n2E7D32\n"),
        "{plain}"
    );

    let (branded, _) = Synthetic::new().export_with_options(
        &guide,
        DocxOptions {
            branding: Branding {
                logo: None,
                accent: Some("#Ab1".into()),
            }
            .check()
            .unwrap(),
            ..DocxOptions::default()
        },
    );
    let branded = branded_dump(&branded);
    assert!(
        branded.contains("Heading1 color=AABB11\nHeading2 color=AABB11\n"),
        "{branded}"
    );
    // Neutral notes take the accent; tips keep their green.
    assert!(
        branded.ends_with("=== note borders ===\nAABB11\n2E7D32\n"),
        "{branded}"
    );
    assert!(!branded.contains("[image 9525x9525]"));
}

#[test]
fn wide_logos_fit_the_text_width() {
    let synthetic = Synthetic::new();
    let mut wide = Vec::new();
    image::RgbaImage::from_pixel(1900, 10, image::Rgba([0, 0, 0, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut wide),
            image::ImageFormat::Png,
        )
        .unwrap();
    let (package, _) = synthetic.export_with_options(
        &Guide::new("Wide", "en"),
        DocxOptions {
            branding: Branding {
                logo: Some(wide),
                accent: None,
            }
            .check()
            .unwrap(),
            ..DocxOptions::default()
        },
    );
    // A4 text width is 9026 twips = 5731510 EMU; 10 px high → 30165 EMU.
    let dump = package.text_dump();
    assert!(
        dump.starts_with("Normal | [image 5731510x30165]\n"),
        "{dump}"
    );
}

#[test]
fn package_has_the_expected_parts() {
    let fixtures = Fixtures::new();
    let (_, loaded) = fixtures.guide("Share a calendar event");
    let out = fixtures.out("share.docx");
    let opts = DocxOptions {
        timestamp: Some(loaded.guide.updated_at),
        ..DocxOptions::default()
    };
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    docx::export(&render, &loaded.handle, &out, &opts).unwrap();

    let package = Package::open(&out);
    for part in [
        "[Content_Types].xml",
        "_rels/.rels",
        "docProps/app.xml",
        "docProps/core.xml",
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/styles.xml",
        "word/numbering.xml",
        "word/settings.xml",
        "word/fontTable.xml",
    ] {
        assert!(package.parts.contains_key(part), "missing {part}");
    }
    let content_types = package.text("[Content_Types].xml");
    assert!(content_types.contains("image/png"), "{content_types}");

    // Fixed dates from the guide, not the clock.
    let core = package.text("docProps/core.xml");
    let date = loaded
        .guide
        .updated_at
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string();
    assert_eq!(core.matches(&date).count(), 2, "{core}");

    // Styles used by the document are defined.
    let styles = package.text("word/styles.xml");
    for style in ["Title", "Subtitle", "Heading1", "Heading2"] {
        assert!(
            styles.contains(&format!("w:styleId=\"{style}\"")),
            "{style}"
        );
    }
}

#[test]
fn one_image_per_step_sized_to_the_page() {
    let fixtures = Fixtures::new();
    for (name, loaded) in fixtures.guides() {
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let out = fixtures.out(&format!("{name}.docx"));
        docx::export(&render, &loaded.handle, &out, &DocxOptions::default()).unwrap();
        let package = Package::open(&out);
        let document = package.text("word/document.xml");
        let steps = render.steps().count();
        assert_eq!(document.matches("<w:drawing>").count(), steps, "{name}");

        // Every image relationship points at a PNG in the package. docx-rs
        // stores identical images once.
        let images = package.relationships_of_type("/image");
        assert_eq!(images.len(), steps, "{name}");
        for target in images.values() {
            let png = &package.parts[&format!("word/{target}")];
            assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"), "{target}");
        }
        let media = package
            .parts
            .keys()
            .filter(|p| p.starts_with("word/media/"))
            .count();
        assert!(media >= 1 && media <= steps, "{name}: {media} media");
    }

    // Wide images are shown at the text width, keeping the aspect ratio.
    let (_, loaded) = fixtures.guide("Rotate an API key");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("wide.docx");
    let opts = DocxOptions {
        paper: Paper::Letter,
        image: guiderecorder_core::export::ImageOptions {
            placeholder_size: (2400, 1200),
            ..DocxOptions::default().image
        },
        ..DocxOptions::default()
    };
    // Force placeholders by pointing every step at a missing image.
    let mut guide = loaded.guide.clone();
    for item in &mut guide.items {
        if let Item::Step(step) = item {
            step.image.full = "steps/missing.webp".into();
            step.image.crop = None;
        }
    }
    let render_missing = RenderGuide::new(&guide, &loaded.handle);
    docx::export(&render_missing, &loaded.handle, &out, &opts).unwrap();
    let document = Package::open(&out).text("word/document.xml");
    let text_width = (12240 - 2 * 1440) * 635;
    let extent = format!("<wp:extent cx=\"{text_width}\" cy=\"{}\"", text_width / 2);
    assert_eq!(
        document.matches(&extent).count(),
        render.steps().count(),
        "{document}"
    );
}

#[test]
fn chosen_cover_follows_the_title_on_its_own_page() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Covered", "en");
    guide.summary = "A summary".into();
    guide.intro = "The intro".into();
    guide.items = vec![step("Click A"), step("Click B")];
    let plain = lib.export(&guide).text("word/document.xml");
    assert_eq!(plain.matches("<w:drawing>").count(), 2);
    assert!(!plain.contains("w:type=\"page\""));

    let mut guide = Guide {
        id: Ulid::new(),
        ..guide
    };
    guide.cover_step_id = Some(guide.items[1].id());
    let document = lib.export(&guide).text("word/document.xml");
    assert_eq!(document.matches("<w:drawing>").count(), 3);
    assert_eq!(document.matches("w:type=\"page\"").count(), 1);
    // Title and summary, the cover and its page break, then the intro.
    let at = |needle: &str| document.find(needle).unwrap();
    assert!(at("A summary") < at("<w:drawing>"));
    assert!(at("<w:drawing>") < at("w:type=\"page\""));
    assert!(at("w:type=\"page\"") < at("The intro"));
}

#[test]
fn missing_images_become_placeholders_with_warnings() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Placeholders", "en");
    guide.items = vec![step("No image"), step("No image either")];
    let (package, report) = lib.export_with_report(&guide);

    let ids: Vec<Ulid> = guide.steps().map(|s| s.id).collect();
    let warned: Vec<Ulid> = report.warnings.iter().map(|(id, _)| *id).collect();
    assert_eq!(warned, ids);
    assert!(report
        .warnings
        .iter()
        .all(|(_, w)| matches!(w, RenderWarning::ImageUnavailable { .. })));
    // The placeholders are still embedded, at the text width.
    assert_eq!(
        package
            .text_dump()
            .matches("[image 5731510x3223974]")
            .count(),
        2
    );
}

#[test]
fn xml_special_characters_are_escaped() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Tom & Jerry <script>alert(\"x\")</script> 'q'", "en");
    guide.summary = "a < b && c > d".into();
    guide.intro = "Raw <b>html</b> & `<code>` and [a & b](https://e.x/?a=1&b=<2>)".into();
    guide.items = vec![
        step("Click **<Save> & \"close\"**\u{0} now"),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Warning,
            body: "]]> <!-- not a comment --> \\&amp;".into(),
        }),
    ];
    let package = lib.export(&guide);
    let document = package.text("word/document.xml");

    for raw in ["<script>", "<b>", "<Save>", "<code>", "<!--", "]]>"] {
        assert!(!document.contains(raw), "raw {raw:?} in document.xml");
    }
    assert!(!document.contains('\u{0}'));
    let dump = package.text_dump();
    for text in [
        "Tom & Jerry <script>alert(\"x\")</script> 'q'",
        "a < b && c > d",
        "Raw <b>html</b> & `<code>` and [a & b](https://e.x/?a=1&b=<2>)",
        "1. Click **<Save> & \"close\"** now",
        "**Warning: **]]> <!-- not a comment --> &amp;",
    ] {
        assert!(dump.contains(text), "{text:?} not in\n{dump}");
    }
    // The relationship target is escaped exactly once.
    let rels = package.text("word/_rels/document.xml.rels");
    assert!(
        rels.contains("Target=\"https://e.x/?a=1&amp;b=&lt;2&gt;\""),
        "{rels}"
    );
}

#[test]
fn hyperlinks_are_external_relationships() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Links", "en");
    guide.intro = "See [docs](https://example.com/docs \"Docs\"), <mailto:help@example.com> \
                   and **[bold link](https://example.com/b)**."
        .into();
    guide.items = vec![step("Open [the page](https://example.com/page)")];
    let package = lib.export(&guide);

    let links = package.relationships_of_type("/hyperlink");
    let targets: Vec<&str> = links.values().map(String::as_str).collect();
    assert_eq!(
        targets,
        [
            "https://example.com/docs",
            "mailto:help@example.com",
            "https://example.com/b",
            "https://example.com/page",
        ]
    );
    let rels = package.text("word/_rels/document.xml.rels");
    assert_eq!(rels.matches("TargetMode=\"External\"").count(), 4, "{rels}");
    let document = package.text("word/document.xml");
    for id in links.keys() {
        assert!(
            document.contains(&format!("<w:hyperlink r:id=\"{id}\"")),
            "{id}"
        );
    }
    let dump = package.text_dump();
    assert!(dump.contains("See [docs](https://example.com/docs), [mailto:help@example.com](mailto:help@example.com) and [**bold link**](https://example.com/b)."), "{dump}");
    assert!(
        dump.contains("1. Open [the page](https://example.com/page)"),
        "{dump}"
    );
}

#[test]
fn markdown_maps_to_runs_and_lists() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Formatting", "en");
    guide.prerequisites =
        "- One *two* `three`\n- Nested:\n  1. a\n  2. b\n\n3. Three\n4. Four".into();
    guide.items = vec![
        Item::Section(Section {
            id: Ulid::new(),
            title: "Part".into(),
        }),
        step("Press `⌘S`"),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Tip,
            body: "- first\n- second".into(),
        }),
    ];
    let dump = lib.export(&guide).text_dump();
    let expected = "\
Title | Formatting
Heading1 | Prerequisites
Normal num=2/0 | One *two* `three`
Normal num=2/0 | Nested:
Normal num=3/1 | a
Normal num=3/1 | b
Normal num=4/0 | Three
Normal num=4/0 | Four
Heading1 | Part
Heading2 | 1. Press `⌘S`
Normal | [image 5731510x3223974]
Normal shd=E8F5E9 | **Tip: **
Normal num=5/0 shd=E8F5E9 | first
Normal num=5/0 shd=E8F5E9 | second
";
    assert_eq!(dump, expected);
    let numbering = lib.last_package().text("word/numbering.xml");
    // The numbered list restarts at its own start number.
    assert!(
        numbering.contains("<w:num w:numId=\"4\"><w:abstractNumId w:val=\"3\" /><w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"3\" />"),
        "{numbering}"
    );
}

#[test]
fn image_comes_before_the_description() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Order", "en");
    let mut first = step("Click **A**");
    if let Item::Step(s) = &mut first {
        s.description = "Then wait.".into();
    }
    guide.items = vec![first];
    let dump = lib.export(&guide).text_dump();
    assert_eq!(
        dump,
        "\
Title | Order
Heading2 | 1. Click **A**
Normal | [image 5731510x3223974]
Normal | Then wait.
"
    );
}

#[test]
fn empty_step_titles_fall_back_like_markdown() {
    let mut guide = Guide::new("Empty", "en");
    guide.items = vec![step("Named"), step(""), step("  ")];
    let dump = Synthetic::new().export(&guide).text_dump();
    let headings: Vec<&str> = dump.lines().filter(|l| l.starts_with("Heading2")).collect();
    assert_eq!(
        headings,
        [
            "Heading2 | 1. Named",
            "Heading2 | 2. Step 2",
            "Heading2 | 3. Step 3"
        ]
    );

    guide.language = "de-DE".into();
    let dump = Synthetic::new().export(&guide).text_dump();
    assert!(dump.contains("Heading2 | 2. Schritt 2\n"), "{dump}");
}

#[test]
fn review_details_follow_the_title_when_asked() {
    let mut guide = Guide::new("Audit", "en");
    guide.summary = "Yearly.".into();
    guide.review = ReviewInfo {
        version: Some("1.0".into()),
        last_reviewed: chrono::NaiveDate::from_ymd_opt(2026, 10, 19),
        ..ReviewInfo::default()
    };
    let dump = Synthetic::new().export(&guide).text_dump();
    assert_eq!(dump, "Title | Audit\nSubtitle | Yearly.\n");
    let opts = DocxOptions {
        review_details: true,
        ..DocxOptions::default()
    };
    let (package, _) = Synthetic::new().export_with_options(&guide, opts);
    assert_eq!(
        package.text_dump(),
        "Title | Audit\nSubtitle | Yearly.\nNormal | Version: 1.0 · Last reviewed: 2026-10-19\n"
    );
}

#[test]
fn labels_follow_the_guide_language_or_the_options() {
    let mut guide = Guide::new("Sprache", "de");
    guide.prerequisites = "Ein Drucker".into();
    guide.items = vec![
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Tip,
            body: "Leicht.".into(),
        }),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Warning,
            body: "Heiß.".into(),
        }),
    ];
    let dump = Synthetic::new().export(&guide).text_dump();
    assert_eq!(
        dump,
        "\
Title | Sprache
Heading1 | Voraussetzungen
Normal | Ein Drucker
Normal shd=E8F5E9 | **Tipp: **Leicht.
Normal shd=FFF4E5 | **Warnung: **Heiß.
"
    );

    // Explicit labels win over the language, and are escaped.
    let labels = Labels {
        prerequisites: "Needs <&>".into(),
        tip: "Psst".into(),
        ..Labels::for_language("en")
    };
    let opts = DocxOptions {
        labels: Some(labels),
        ..DocxOptions::default()
    };
    let (package, _) = Synthetic::new().export_with_options(&guide, opts);
    let dump = package.text_dump();
    assert!(dump.contains("Heading1 | Needs <&>\n"), "{dump}");
    assert!(dump.contains("**Psst: **Leicht."), "{dump}");
    assert!(dump.contains("**Warning: **Heiß."), "{dump}");
    assert!(package
        .text("word/document.xml")
        .contains("Needs &lt;&amp;&gt;"));
}

#[test]
fn output_is_deterministic() {
    let fixtures = Fixtures::new();
    let (_, loaded) = fixtures.guide("Share a calendar event");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let opts = DocxOptions {
        timestamp: Some(loaded.guide.updated_at),
        ..DocxOptions::default()
    };
    let a = fixtures.out("a.docx");
    let b = fixtures.out("b.docx");
    docx::export(&render, &loaded.handle, &a, &opts).unwrap();
    // Something else creating docx-rs ids in between must not matter.
    let other = fixtures.out("other.docx");
    docx::export(&render, &loaded.handle, &other, &DocxOptions::default()).unwrap();
    docx::export(&render, &loaded.handle, &b, &opts).unwrap();
    assert!(fs::read(&a).unwrap() == fs::read(&b).unwrap());
    // And from another thread.
    let c = fixtures.out("c.docx");
    std::thread::scope(|s| {
        s.spawn(|| docx::export(&render, &loaded.handle, &c, &opts).unwrap());
    });
    assert!(fs::read(&a).unwrap() == fs::read(&c).unwrap());
}

#[test]
fn refuses_to_overwrite_unless_asked() {
    let fixtures = Fixtures::new();
    let (_, loaded) = fixtures.guide("Kurzanleitung Drucker");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("existing.docx");
    fs::write(&out, b"keep me").unwrap();
    let err = docx::export(&render, &loaded.handle, &out, &DocxOptions::default()).unwrap_err();
    assert!(
        matches!(err, DocxError::Exists(ref p) if *p == out),
        "{err}"
    );
    assert_eq!(fs::read(&out).unwrap(), b"keep me");

    let opts = DocxOptions {
        overwrite: true,
        ..DocxOptions::default()
    };
    docx::export(&render, &loaded.handle, &out, &opts).unwrap();
    Package::open(&out);
    // No temp files left behind.
    let names: Vec<_> = fs::read_dir(out.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["existing.docx"]);
}

// ---------------------------------------------------------------------------
// Helpers

/// A temp copy of the fixture guides plus an output folder.
struct Fixtures {
    _dir: tempfile::TempDir,
    library: Library,
    out_dir: PathBuf,
}

impl Fixtures {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        copy_dir(&fixture_guides_dir(), &root);
        let out_dir = dir.path().join("out");
        fs::create_dir(&out_dir).unwrap();
        Self {
            library: Library::open(&root).unwrap(),
            out_dir,
            _dir: dir,
        }
    }

    /// Every fixture by bundle name (without `.guiderecorder`), sorted.
    fn guides(&self) -> Vec<(String, LoadedGuide)> {
        let mut paths: Vec<PathBuf> = fs::read_dir(self.library.root())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "guiderecorder"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty());
        paths
            .into_iter()
            .map(|p| {
                let name = p.file_stem().unwrap().to_string_lossy().into_owned();
                (name, self.library.read(&p).unwrap())
            })
            .collect()
    }

    fn guide(&self, name: &str) -> (String, LoadedGuide) {
        self.guides()
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("no fixture {name}"))
    }

    fn out(&self, name: &str) -> PathBuf {
        self.out_dir.join(name)
    }
}

/// A synthetic guide in a temp library; steps have no image files, so they
/// render as 800×450 placeholders.
struct Synthetic {
    dir: tempfile::TempDir,
    library: Library,
}

impl Synthetic {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path()).unwrap();
        Self { dir, library }
    }

    fn export(&self, guide: &Guide) -> Package {
        self.export_with_report(guide).0
    }

    fn export_with_report(&self, guide: &Guide) -> (Package, ExportReport) {
        self.export_with_options(guide, DocxOptions::default())
    }

    /// Exports with `opts`, forcing `overwrite` and a fixed timestamp.
    fn export_with_options(&self, guide: &Guide, opts: DocxOptions) -> (Package, ExportReport) {
        let handle: GuideHandle = self.library.create(guide).unwrap();
        let render = RenderGuide::new(guide, &handle);
        let out = self.dir.path().join("out.docx");
        let opts = DocxOptions {
            overwrite: true,
            timestamp: Some(DateTime::<Utc>::UNIX_EPOCH),
            ..opts
        };
        let report = docx::export(&render, &handle, &out, &opts).unwrap();
        (Package::open(&out), report)
    }

    fn last_package(&self) -> Package {
        Package::open(&self.dir.path().join("out.docx"))
    }
}

fn step(title: &str) -> Item {
    let id = Ulid::new();
    Item::Step(Step {
        id,
        title: title.into(),
        description: String::new(),
        kind: StepKind::Click,
        point: None,
        end_point: None,
        element: None,
        keystrokes: None,
        url: None,
        image: StepImage {
            full: format!("steps/{id}.webp"),
            crop: None,
            crop_rect: None,
            after: None,
            show: ImageShow::Full,
        },
        annotations: Vec::new(),
        narration: String::new(),
        ai_state: Default::default(),
    })
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The parts of a DOCX package.
struct Package {
    parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// Reads a zip whose entries are stored (docx-rs never compresses).
    fn open(path: &Path) -> Self {
        let zip = fs::read(path).unwrap();
        let u16_at = |i: usize| u16::from_le_bytes([zip[i], zip[i + 1]]) as usize;
        let u32_at = |i: usize| u32::from_le_bytes(zip[i..i + 4].try_into().unwrap()) as usize;
        let eocd = (0..=zip.len() - 22)
            .rev()
            .find(|&i| zip[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
            .expect("not a zip file");
        let (count, mut at) = (u16_at(eocd + 10), u32_at(eocd + 16));
        let mut parts = BTreeMap::new();
        for _ in 0..count {
            assert_eq!(zip[at..at + 4], [0x50, 0x4b, 0x01, 0x02], "central header");
            let method = u16_at(at + 10);
            let size = u32_at(at + 20);
            let (name_len, extra_len, comment_len) =
                (u16_at(at + 28), u16_at(at + 30), u16_at(at + 32));
            let local = u32_at(at + 42);
            let name = String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).unwrap();
            at += 46 + name_len + extra_len + comment_len;
            if name.ends_with('/') {
                continue;
            }
            assert_eq!(method, 0, "{name} is compressed");
            assert_eq!(
                zip[local..local + 4],
                [0x50, 0x4b, 0x03, 0x04],
                "local header"
            );
            let data = local + 30 + u16_at(local + 26) + u16_at(local + 28);
            parts.insert(name, zip[data..data + size].to_vec());
        }
        Self { parts }
    }

    fn text(&self, part: &str) -> String {
        let bytes = self.parts.get(part).unwrap_or_else(|| panic!("no {part}"));
        String::from_utf8(bytes.clone()).unwrap()
    }

    /// Relationship id → target of `word/document.xml`, for types ending
    /// in `kind`, in document order.
    fn relationships_of_type(&self, kind: &str) -> BTreeMap<String, String> {
        let rels = self.text("word/_rels/document.xml.rels");
        tags(&rels)
            .filter(|(name, _)| *name == "Relationship")
            .filter(|(_, tag)| attr(tag, "Type").is_some_and(|t| t.ends_with(kind)))
            .map(|(_, tag)| (attr(tag, "Id").unwrap(), attr(tag, "Target").unwrap()))
            .collect()
    }

    /// The paragraph dump described in the module docs.
    fn text_dump(&self) -> String {
        let links = self.relationships_of_type("/hyperlink");
        let doc = self.text("word/document.xml");
        let mut out = String::new();
        let mut para = Para::default();
        let mut run = RunFmt::default();
        let (mut in_ppr, mut in_run) = (false, false);
        let mut link_target = None;
        let mut rest = doc.as_str();
        while let Some(start) = rest.find('<') {
            let end = start + rest[start..].find('>').unwrap();
            let tag = &rest[start + 1..end];
            let after = &rest[end + 1..];
            let name = tag
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap();
            let closing = tag.starts_with('/');
            match (name, closing) {
                ("w:p", false) => para = Para::default(),
                ("", true) if tag == "/w:p" => {
                    let mut head = para.style.clone().unwrap_or_else(|| "Normal".into());
                    if let Some(num) = &para.num {
                        head += &format!(" num={num}/{}", para.ilvl.as_deref().unwrap_or("0"));
                    }
                    if let Some(fill) = &para.shd {
                        head += &format!(" shd={fill}");
                    }
                    out += &format!("{head} | {}\n", para.text);
                }
                ("w:pPr", false) => in_ppr = !tag.ends_with('/'),
                ("", true) if tag == "/w:pPr" => in_ppr = false,
                ("w:pStyle", false) if in_ppr => para.style = attr(tag, "w:val"),
                ("w:numId", false) if in_ppr => para.num = attr(tag, "w:val"),
                ("w:ilvl", false) if in_ppr => para.ilvl = attr(tag, "w:val"),
                ("w:shd", false) if in_ppr && !in_run => para.shd = attr(tag, "w:fill"),
                ("w:r", false) => {
                    run = RunFmt::default();
                    in_run = true;
                }
                ("", true) if tag == "/w:r" => in_run = false,
                ("w:b", false) if in_run => {
                    run.bold = attr(tag, "w:val").is_none_or(|v| v != "false")
                }
                ("w:i", false) if in_run => {
                    run.italic = attr(tag, "w:val").is_none_or(|v| v != "false")
                }
                ("w:rFonts", false) if in_run => run.code = tag.contains("Consolas"),
                ("w:t", false) if !tag.ends_with('/') => {
                    let close = after.find("</w:t>").unwrap();
                    para.text += &run.wrap(&unescape(&after[..close]));
                }
                ("w:br", false) => para.text.push('↵'),
                ("wp:extent", false) => {
                    para.text += &format!(
                        "[image {}x{}]",
                        attr(tag, "cx").unwrap(),
                        attr(tag, "cy").unwrap()
                    );
                }
                ("w:hyperlink", false) => {
                    let id = attr(tag, "r:id").expect("external hyperlink");
                    link_target = Some(links.get(&id).cloned().expect("relationship"));
                    para.text.push('[');
                }
                ("", true) if tag == "/w:hyperlink" => {
                    para.text += &format!("]({})", link_target.take().unwrap());
                }
                _ => {}
            }
            rest = after;
        }
        out
    }
}

#[derive(Default)]
struct Para {
    style: Option<String>,
    num: Option<String>,
    ilvl: Option<String>,
    shd: Option<String>,
    text: String,
}

#[derive(Default)]
struct RunFmt {
    bold: bool,
    italic: bool,
    code: bool,
}

impl RunFmt {
    fn wrap(&self, text: &str) -> String {
        let mut text = text.to_owned();
        if self.code {
            text = format!("`{text}`");
        }
        if self.italic {
            text = format!("*{text}*");
        }
        if self.bold {
            text = format!("**{text}**");
        }
        text
    }
}

/// Tags of `xml` as (name, whole tag text without `<>`).
fn tags(xml: &str) -> impl Iterator<Item = (&str, &str)> {
    xml.split('<').skip(1).map(|s| {
        let tag = &s[..s.find('>').unwrap()];
        let name = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap();
        (name, tag)
    })
}

/// Unescaped value of attribute `name` in `tag`.
fn attr(tag: &str, name: &str) -> Option<String> {
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    let len = tag[start..].find('"')?;
    Some(unescape(&tag[start..start + len]))
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#xA;", "\n")
        .replace("&amp;", "&")
}
