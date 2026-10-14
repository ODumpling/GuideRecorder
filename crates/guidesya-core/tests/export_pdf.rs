//! PDF export (#359, design #443): typesetting, text, safety, determinism.
//!
//! Text is checked twice: in the PDF bytes themselves, extracted with
//! lopdf ([`pdf_text`]), and on the laid-out pages that [`pdf::render`]
//! returns (`PdfOutput::page_text`), which keep real lines and reading
//! order. The goldens under `tests/golden/pdf/` are the latter, per page:
//!
//! ```text
//! === page 2 ===
//! Share a calendar event
//! 1. Click Calendar in the Dock
//! ```
//!
//! Regenerate with `UPDATE_GOLDEN=1 cargo test -p guidesya-core --test export_pdf`.
//! The PDF bytes themselves are checked for structure (header, trailer,
//! page objects, embedded fonts and images) without a PDF parser.

mod support;

use std::fs;
use std::path::PathBuf;

use guidesya_core::export::docx::Paper;
use guidesya_core::export::md::Labels;
use guidesya_core::export::pdf::{self, Branding, PdfError, PdfOptions, PdfOutput};
use guidesya_core::export::{RenderGuide, RenderWarning};
use guidesya_core::library::{Library, LoadedGuide};
use guidesya_core::model::*;
use guidesya_core::Ulid;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

// ---------------------------------------------------------------------------
// Tests

#[test]
fn fixtures_match_text_goldens() {
    let fixtures = Fixtures::new();
    for (name, loaded) in fixtures.guides() {
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let out = fixtures.out(&format!("{name}.pdf"));
        let opts = fixture_options(&loaded);
        let report = pdf::export(&render, &loaded.handle, &out, &opts).unwrap();
        assert_eq!(report.files, std::slice::from_ref(&out));
        // The fixtures' images all exist (some crops lack an offset, which
        // only warns).
        assert!(
            !report
                .warnings
                .iter()
                .any(|(_, w)| matches!(w, RenderWarning::ImageUnavailable { .. })),
            "{name}: {:?}",
            report.warnings
        );

        let output = pdf::render(&render, &loaded.handle, &opts).unwrap();
        assert!(
            fs::read(&out).unwrap() == output.pdf,
            "{name}: export != render"
        );
        assert!(
            output.report.notes.is_empty(),
            "{name}: {:?}",
            output.report.notes
        );
        // Cover page plus at least one page of steps.
        assert!(
            output.page_count() >= 2,
            "{name}: {} pages",
            output.page_count()
        );
        check_pdf_structure(&output);
        // The PDF's own text layer has every step title.
        let pdf = flat(&pdf_text(&output.pdf).join(" "));
        for step in render.steps() {
            let title = guidesya_core::export::ast::plain_text(&step.title);
            assert!(
                pdf.contains(&flat(&title)),
                "{name}: {title:?} not in\n{pdf}"
            );
        }
        eprintln!(
            "{name}: {} pages, {} bytes, typeset in {:?}",
            output.page_count(),
            output.pdf.len(),
            output.compile_time
        );
        assert_golden_text(&format!("pdf/{}.golden", slug(&name)), &dump(&output));
    }
}

#[test]
fn every_step_title_and_note_is_in_the_text() {
    let fixtures = Fixtures::new();
    for (name, loaded) in fixtures.guides() {
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let output = pdf::render(&render, &loaded.handle, &fixture_options(&loaded)).unwrap();
        let text = flat_text(&output);
        let labels = Labels::for_language(&loaded.guide.language);
        assert!(text.contains(&flat(&loaded.guide.title)), "{name}");
        for step in render.steps() {
            let title = guidesya_core::export::ast::plain_text(&step.title);
            let expected = if title.trim().is_empty() {
                format!("{}. {} {}", step.number, labels.step, step.number)
            } else {
                format!("{}. {}", step.number, flat(&title))
            };
            assert!(
                text.contains(&expected),
                "{name}: {expected:?} not in\n{text}"
            );
        }
        for note in loaded.guide.items.iter().filter_map(|i| match i {
            Item::Note(n) => Some(n),
            _ => None,
        }) {
            assert!(text.contains(labels.note_kind(note.kind)), "{name}");
        }
        // Header with the title on every page after the cover, footer "n / N".
        let pages = output.page_count();
        for (i, page) in output.page_text.iter().enumerate().skip(1) {
            assert!(
                page.starts_with(&loaded.guide.title),
                "{name} page {}",
                i + 1
            );
            assert!(
                page.trim_end().ends_with(&format!("{} / {pages}", i + 1)),
                "{name} page {}: {page}",
                i + 1
            );
        }
        // No header or footer on the cover.
        assert!(!output.page_text[0].contains(&format!("1 / {pages}")));
    }
}

#[test]
fn cover_page_has_title_summary_and_date() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Cover test", "en");
    guide.summary = "What this guide does.".into();
    guide.items = vec![step("Click A")];
    let opts = PdfOptions {
        timestamp: Some("2026-03-04T05:06:07Z".parse().unwrap()),
        ..PdfOptions::default()
    };
    let output = lib.render_with(&guide, opts);
    assert_eq!(
        output.page_text[0],
        "Cover test\nWhat this guide does.\n2026-03-04"
    );
    // No timestamp, no date.
    let output = lib.render(&guide);
    assert_eq!(output.page_text[0], "Cover test\nWhat this guide does.");
}

#[test]
fn hostile_text_is_printed_literally_and_never_runs() {
    let hostile = [
        r#"#panic("x")"#,
        r#"#read("/etc/passwd")"#,
        r#"#{ panic("boom") }"#,
        "$x^2$",
        "@ref",
        "<label>",
        r#"#import "@preview/foo:0.1.0": *"#,
        r#"#json("/guide.json")"#,
        r#"#image("/steps/1.png")"#,
        "*not bold* _not em_ `not raw` = no heading",
        r#"\ ] [ } { ) ( // /* comment */ " ' ~ -- ---"#,
    ];
    let lib = Synthetic::new();
    let mut guide = Guide::new(hostile.join(" | "), "en");
    guide.summary = hostile[0].into();
    // Through the Markdown AST too: backslash-escape Markdown's own syntax so
    // the strings reach the PDF writer as plain text.
    let md: Vec<String> = hostile.iter().map(|h| escape_markdown(h)).collect();
    guide.intro = md.join("\n\n");
    guide.prerequisites = format!("- {}\n- `{}`", md[1], hostile[0]);
    guide.items = vec![
        Item::Section(Section {
            id: Ulid::new(),
            title: hostile[2].into(),
        }),
        step(&format!("Click **{}** and *{}*", md[3], md[4])),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Warning,
            body: md[5].clone(),
        }),
    ];
    let labels = Labels {
        tip: hostile[0].into(),
        warning: hostile[1].into(),
        ..Labels::for_language("en")
    };
    let opts = PdfOptions {
        labels: Some(labels),
        ..PdfOptions::default()
    };
    let output = lib.render_with(&guide, opts);
    assert!(output.report.notes.is_empty(), "{:?}", output.report.notes);
    let text = flat_text(&output);
    for h in hostile {
        assert!(
            text.contains(&flat(h)),
            "{h:?} not printed literally in\n{text}"
        );
    }
    // Labels are text too: the warning note is labelled with hostile[1].
    assert!(text.contains(r#"#read("/etc/passwd"): <label>"#), "{text}");
    assert!(
        text.contains(&format!("1. Click {} and {}", hostile[3], hostile[4])),
        "{text}"
    );
    // Nothing from the file system or the template's own files leaked in.
    assert!(!text.contains("root:"), "{text}");
    assert!(!text.contains("\"kind\""), "{text}");
}

#[test]
fn markdown_inlines_and_lists_are_rendered() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Formatting", "en");
    guide.intro =
        "Some **bold**, *italic*, `code` and [a link](https://example.com/x).\\\nNext line.".into();
    guide.prerequisites = "- One\n- Two\n  1. a\n  2. b\n\n3. Three\n4. Four".into();
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
    let output = lib.render(&guide);
    let page = &output.page_text[1];
    let lines: Vec<&str> = page.lines().collect();
    assert_eq!(
        lines,
        [
            "Formatting",
            "Some bold, italic, code and a link.",
            "Next line.",
            "Prerequisites",
            "• One",
            "• Two",
            "1. a",
            "2. b",
            "3. Three",
            "4. Four",
            "Part",
            "1. Press ⌘S",
            "Tip:",
            "• first",
            "• second",
            "2 / 2",
        ],
        "{page}"
    );
    // The link is a real link annotation to the AST's URL.
    let pdf = String::from_utf8_lossy(&output.pdf);
    assert!(pdf.contains("https://example.com/x"), "no link URI");
}

#[test]
fn step_image_heading_and_text_stay_together() {
    // Many steps with tall placeholder images: every heading shares its page
    // with its image and first description paragraph.
    let lib = Synthetic::new();
    let mut guide = Guide::new("Together", "en");
    guide.items = (1..=9)
        .map(|i| {
            let mut item = step(&format!("Step title {i}"));
            if let Item::Step(s) = &mut item {
                s.description = format!("Description {i}.\n\nSecond paragraph {i}.");
            }
            item
        })
        .collect();
    let opts = PdfOptions {
        image: guidesya_core::export::ImageOptions {
            placeholder_size: (800, 700),
            ..PdfOptions::default().image
        },
        ..PdfOptions::default()
    };
    let output = lib.render_with(&guide, opts);
    assert!(output.page_count() >= 5, "{} pages", output.page_count());
    for i in 1..=9 {
        let heading = format!("{i}. Step title {i}");
        let page = output
            .page_text
            .iter()
            .find(|p| p.lines().any(|l| l == heading))
            .unwrap_or_else(|| panic!("{heading} missing"));
        assert!(page.contains(&format!("Description {i}.")), "{page}");
    }
    // The nine identical placeholders are embedded once (an image plus its
    // alpha mask).
    assert!(count(&output.pdf, b"/Subtype/Image") <= 2);
}

#[test]
fn missing_images_become_placeholders_with_warnings() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Placeholders", "en");
    guide.items = vec![step("No image"), step("No image either")];
    let (output, report) = lib.export_with_report(&guide, PdfOptions::default());

    let ids: Vec<Ulid> = guide.steps().map(|s| s.id).collect();
    let warned: Vec<Ulid> = report.warnings.iter().map(|(id, _)| *id).collect();
    assert_eq!(warned, ids);
    assert!(report
        .warnings
        .iter()
        .all(|(_, w)| matches!(w, RenderWarning::ImageUnavailable { .. })));
    assert_eq!(output.page_count(), 2);
    assert!(count(&output.pdf, b"/Subtype/Image") >= 1);
}

#[test]
fn empty_step_titles_fall_back_and_labels_follow_language() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Sprache", "de-DE");
    guide.prerequisites = "Ein Drucker".into();
    guide.items = vec![
        step("Named"),
        step(""),
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Tip,
            body: "Leicht.".into(),
        }),
    ];
    let text = flat_text(&lib.render(&guide));
    for expected in [
        "Voraussetzungen",
        "1. Named",
        "2. Schritt 2",
        "Tipp: Leicht.",
    ] {
        assert!(text.contains(expected), "{expected:?} not in {text}");
    }
}

#[test]
fn branding_hooks_are_off_by_default_and_work() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Branded", "en");
    guide.items = vec![step("Click A")];
    let plain = lib.render(&guide);
    let images_plain = count(&plain.pdf, b"/Subtype/Image");

    // A placeholder PNG from the renderer makes a fine logo.
    let Item::Step(logo_step) = step("logo") else {
        unreachable!()
    };
    let handle = lib.library.create(&Guide::new("Logo", "en")).unwrap();
    let logo = guidesya_core::export::render_step_image(
        &handle,
        &logo_step,
        &guidesya_core::export::ImageOptions {
            placeholder_size: (120, 40),
            ..Default::default()
        },
    )
    .png;
    let opts = PdfOptions {
        branding: Branding {
            logo: Some(logo),
            accent: Some([0xaa, 0x11, 0x22]),
        },
        ..PdfOptions::default()
    };
    let branded = lib.render_with(&guide, opts);
    assert!(
        branded.report.notes.is_empty(),
        "{:?}",
        branded.report.notes
    );
    assert_eq!(branded.page_count(), plain.page_count());
    assert_ne!(branded.pdf, plain.pdf);
    assert!(count(&branded.pdf, b"/Subtype/Image") > images_plain);
}

#[test]
fn letter_paper_is_supported() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Letter", "en");
    guide.items = vec![step("Click A")];
    let a4 = lib.render(&guide);
    let letter = lib.render_with(
        &guide,
        PdfOptions {
            paper: Paper::Letter,
            ..PdfOptions::default()
        },
    );
    let a4_box = String::from_utf8_lossy(&a4.pdf).contains("/MediaBox[0 0 595.2756 841.8898]");
    let letter_box = String::from_utf8_lossy(&letter.pdf).contains("/MediaBox[0 0 612 792]");
    assert!(a4_box && letter_box, "media boxes");
}

#[test]
fn a_long_first_paragraph_flows_across_pages() {
    // 2,500 numbered words as the step's only description paragraph.
    let words: Vec<String> = (1..=2500).map(|i| format!("w{i:04}")).collect();
    let mut guide = Guide::new("Long paragraph", "en");
    let mut long = step("Read this");
    if let Item::Step(s) = &mut long {
        s.description = words.join(" ");
    }
    guide.items = vec![long, step("After")];
    let output = Synthetic::new().render(&guide);
    assert_flows(&output, &words, "w", 3);
    // The heading and image stay with the start of the text.
    let first = output
        .page_text
        .iter()
        .find(|p| p.contains("1. Read this"))
        .unwrap();
    assert!(first.contains("w0001"), "{first}");
}

#[test]
fn a_long_first_list_flows_across_pages() {
    let items: Vec<String> = (1..=150).map(|i| format!("item{i:03}")).collect();
    let mut guide = Guide::new("Long list", "en");
    let mut long = step("Check these");
    if let Item::Step(s) = &mut long {
        s.description = items.iter().map(|i| format!("- {i}\n")).collect();
    }
    guide.items = vec![long];
    let output = Synthetic::new().render(&guide);
    assert_flows(&output, &items, "item", 3);
}

#[test]
fn long_notes_flow_across_pages() {
    let words: Vec<String> = (1..=2500).map(|i| format!("n{i:04}")).collect();
    let mut guide = Guide::new("Long note", "en");
    guide.items = vec![Item::Note(Note {
        id: Ulid::new(),
        kind: NoteKind::Note,
        body: words.join(" "),
    })];
    let output = Synthetic::new().render(&guide);
    assert_flows(&output, &words, "n", 3);
}

#[test]
fn out_of_range_years_are_clamped_not_fatal() {
    let lib = Synthetic::new();
    let mut guide = Guide::new("Dates", "en");
    guide.items = vec![step("Click A")];
    for (timestamp, cover) in [
        ("+10000-01-01T00:00:00Z", "9999-12-31"),
        ("+262000-06-01T12:00:00Z", "9999-12-31"),
        ("-10000-01-01T00:00:00Z", "-9999-01-01"),
        ("9999-12-31T23:59:59Z", "9999-12-31"),
        ("0000-02-29T00:00:00Z", "0000-02-29"),
    ] {
        let opts = PdfOptions {
            timestamp: Some(timestamp.parse().unwrap()),
            ..PdfOptions::default()
        };
        let output = lib.render_with(&guide, opts);
        assert!(
            output.page_text[0].ends_with(cover),
            "{timestamp}: {}",
            output.page_text[0]
        );
    }
}

#[test]
fn over_long_urls_become_text_with_a_note() {
    let lib = Synthetic::new();
    let long = format!("https://e.x/{}", "a".repeat(9000));
    let mut guide = Guide::new("Links", "en");
    guide.intro = format!("See [the docs]({long}) and <{long}> or [ok](https://e.x/ok).");
    let (output, report) = lib.export_with_report(&guide, PdfOptions::default());
    // The URL's text is kept (wrapped over many lines), the link dropped.
    let text = flat_text(&output);
    assert!(text.contains("See the docs and https://e.x/"));
    assert!(text.matches('a').count() >= 9000, "URL text missing");
    assert!(
        text.ends_with("or ok. 2 / 2"),
        "...{}",
        &text[text.len() - 40..]
    );
    assert_eq!(report.notes.len(), 2, "{:?}", report.notes);
    assert!(report.notes[0].contains("9012 bytes"), "{:?}", report.notes);
    let pdf = String::from_utf8_lossy(&output.pdf);
    assert!(pdf.contains("https://e.x/ok"));
    assert!(!pdf.contains("https://e.x/aaaa"));
    // Exactly at the limit is still a link.
    let mut guide = Guide::new("Limit", "en");
    let limit = format!("https://e.x/{}", "b".repeat(8000 - 12));
    guide.intro = format!("[x]({limit})");
    let (output, report) = lib.export_with_report(&guide, PdfOptions::default());
    assert!(report.notes.is_empty(), "{:?}", report.notes);
    assert!(String::from_utf8_lossy(&output.pdf).contains(&limit));
}

#[test]
fn blank_titles_fall_back_to_untitled() {
    let lib = Synthetic::new();
    for (title, language, expected) in [
        ("", "en", "Untitled"),
        ("  \t", "", "Untitled"),
        ("\u{202E}\u{2028}", "de-AT", "Unbenannt"),
    ] {
        let mut guide = Guide::new(title, language);
        guide.items = vec![step("Click A")];
        let output = lib.render(&guide);
        assert_eq!(output.page_text[0], expected);
        assert!(
            output.page_text[1].starts_with(expected),
            "{}",
            output.page_text[1]
        );
    }
}

#[test]
fn separators_and_bidi_controls_are_cleaned() {
    let mut guide = Guide::new("A\u{2028}B\u{2029}C", "en");
    guide.intro = "x\u{202E}gnp.exe\u{202C} y\u{2066}z\u{2069}".into();
    let output = Synthetic::new().render(&guide);
    assert_eq!(output.page_text[0], "A B C");
    let text = flat_text(&output);
    assert!(text.contains("xgnp.exe yz"), "{text}");
    let pdf = pdf_text(&output.pdf).join(" ");
    for c in ['\u{202E}', '\u{202C}', '\u{2066}', '\u{2069}', '\u{2028}'] {
        assert!(!pdf.contains(c), "{c:?} in PDF text");
    }
}

#[test]
fn symbols_use_bundled_fallback_fonts() {
    let mut guide = Guide::new("Keys", "en");
    guide.items = vec![step("Press `⌘⇧⌥S` or ⌃C")];
    let output = Synthetic::new().render(&guide);
    assert!(output.report.notes.is_empty(), "{:?}", output.report.notes);
    let pdf = String::from_utf8_lossy(&output.pdf);
    assert!(pdf.contains("+Lato-Regular"), "Lato");
    assert!(pdf.contains("+Lato-Bold"), "Lato Bold for headings");
    assert!(pdf.contains("+DejaVuSansMono"), "fallback and code font");
    let text = flat(&pdf_text(&output.pdf).join(""));
    for c in ['⌘', '⇧', '⌥', '⌃'] {
        assert!(text.contains(c), "{c} not in PDF text: {text}");
    }
}

#[test]
fn output_is_deterministic() {
    let fixtures = Fixtures::new();
    let (_, loaded) = fixtures.guide("Share a calendar event");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let opts = fixture_options(&loaded);
    let a = pdf::render(&render, &loaded.handle, &opts).unwrap();
    // Another export in between must not matter.
    let (_, other) = fixtures.guide("Rotate an API key");
    let other_render = RenderGuide::new(&other.guide, &other.handle);
    pdf::render(&other_render, &other.handle, &PdfOptions::default()).unwrap();
    let b = pdf::render(&render, &loaded.handle, &opts).unwrap();
    assert!(a.pdf == b.pdf, "two runs differ");
    // And from another thread.
    let c = std::thread::scope(|s| {
        s.spawn(|| pdf::render(&render, &loaded.handle, &opts).unwrap())
            .join()
            .unwrap()
    });
    assert!(a.pdf == c.pdf, "other thread differs");
    // Dates come from the options, never the clock.
    let pdf_text = String::from_utf8_lossy(&a.pdf);
    let date = loaded.guide.updated_at.format("%Y%m%d%H%M%S").to_string();
    assert!(pdf_text.contains(&date), "no creation date {date}");
}

#[test]
fn refuses_to_overwrite_unless_asked() {
    let fixtures = Fixtures::new();
    let (_, loaded) = fixtures.guide("Kurzanleitung Drucker");
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = fixtures.out("existing.pdf");
    fs::write(&out, b"keep me").unwrap();
    let err = pdf::export(&render, &loaded.handle, &out, &PdfOptions::default()).unwrap_err();
    assert!(matches!(err, PdfError::Exists(ref p) if *p == out), "{err}");
    assert_eq!(fs::read(&out).unwrap(), b"keep me");

    let opts = PdfOptions {
        overwrite: true,
        ..PdfOptions::default()
    };
    pdf::export(&render, &loaded.handle, &out, &opts).unwrap();
    assert!(fs::read(&out).unwrap().starts_with(b"%PDF-"));
    // No temp files left behind.
    let names: Vec<_> = fs::read_dir(out.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["existing.pdf"]);
}

// ---------------------------------------------------------------------------
// Helpers

fn fixture_options(loaded: &LoadedGuide) -> PdfOptions {
    PdfOptions {
        timestamp: Some(loaded.guide.updated_at),
        ..PdfOptions::default()
    }
}

/// The per-page text dump described in the module docs.
fn dump(output: &PdfOutput) -> String {
    let mut out = String::new();
    for (i, page) in output.page_text.iter().enumerate() {
        out += &format!("=== page {} ===\n{page}\n", i + 1);
    }
    out
}

/// All page text on one line, whitespace collapsed, so wrapped lines still
/// match.
fn flat_text(output: &PdfOutput) -> String {
    flat(&output.page_text.join(" "))
}

fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Backslash-escapes every ASCII punctuation character, so Markdown keeps
/// the text as is.
fn escape_markdown(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escape = c.is_ascii_punctuation();
            escape.then_some('\\').into_iter().chain([c])
        })
        .collect()
}

/// Text of each page, extracted from the PDF bytes with lopdf.
fn pdf_text(pdf: &[u8]) -> Vec<String> {
    let doc = lopdf::Document::load_mem(pdf).expect("lopdf parses the PDF");
    doc.get_pages()
        .into_keys()
        .map(|n| doc.extract_text(&[n]).expect("page text"))
        .collect()
}

/// Laid-out lines a page may hold: the A4 text area (about 700 pt) at the
/// 10.5 pt body size with its leading fits about 50.
const MAX_LINES_PER_PAGE: usize = 60;

/// `tokens` (unique words starting with `prefix`) all appear exactly once
/// in the PDF's text, in order, spread over at least `min_pages` pages,
/// and no page holds more than fits on it, so nothing ran past the bottom
/// margin.
fn assert_flows(output: &PdfOutput, tokens: &[String], prefix: &str, min_pages: usize) {
    let pages = pdf_text(&output.pdf);
    assert_eq!(pages.len(), output.page_count());
    let found: Vec<Vec<&str>> = pages
        .iter()
        .map(|p| {
            p.split_whitespace()
                .filter(|w| {
                    w.starts_with(prefix)
                        && w[prefix.len()..].bytes().all(|b| b.is_ascii_digit())
                        && w.len() > prefix.len()
                })
                .collect()
        })
        .collect();
    let all: Vec<&str> = found.iter().flatten().copied().collect();
    assert_eq!(all, tokens, "every token once, in order");
    let used = found.iter().filter(|f| !f.is_empty()).count();
    assert!(used >= min_pages, "only {used} pages used");
    for (i, page) in output.page_text.iter().enumerate() {
        let lines = page.lines().count();
        assert!(
            lines <= MAX_LINES_PER_PAGE,
            "page {} has {lines} lines",
            i + 1
        );
    }
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|w| *w == needle)
        .count()
}

/// Header, trailer, the page tree count and an embedded font.
fn check_pdf_structure(output: &PdfOutput) {
    let pdf = &output.pdf;
    assert!(pdf.starts_with(b"%PDF-1."), "header");
    assert!(pdf.trim_ascii_end().ends_with(b"%%EOF"), "trailer");
    let text = String::from_utf8_lossy(pdf);
    assert!(
        text.contains(&format!("/Type/Pages/Count {}", output.page_count())),
        "page tree count"
    );
    assert_eq!(
        count(pdf, b"/Type/Page/") + count(pdf, b"/Type/Page>"),
        output.page_count()
    );
    assert!(text.contains("+Lato-Regular"), "embedded font subset");
    // No font outside the bundled set.
    for font in text.split("/BaseFont/").skip(1) {
        let name = font.split(['/', '>', ' ']).next().unwrap();
        let family = name.split_once('+').map_or(name, |(_, f)| f);
        assert!(
            ["Lato-", "DejaVuSansMono", "NewCM", "LibertinusSerif"]
                .iter()
                .any(|f| family.starts_with(f)),
            "unexpected font {name}"
        );
    }
}

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

    /// Every fixture by bundle name (without `.guidesya`), sorted.
    fn guides(&self) -> Vec<(String, LoadedGuide)> {
        let mut paths: Vec<PathBuf> = fs::read_dir(self.library.root())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "guidesya"))
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

    fn render(&self, guide: &Guide) -> PdfOutput {
        self.render_with(guide, PdfOptions::default())
    }

    fn render_with(&self, guide: &Guide, opts: PdfOptions) -> PdfOutput {
        self.export_with_report(guide, opts).0
    }

    /// Exports with `opts` (forcing `overwrite`) and also returns the typeset
    /// output, which must match the file.
    fn export_with_report(
        &self,
        guide: &Guide,
        opts: PdfOptions,
    ) -> (PdfOutput, guidesya_core::export::ExportReport) {
        let handle = match self.library.create(guide) {
            Ok(handle) => handle,
            Err(_) => self.library.find(guide.id).unwrap().unwrap(),
        };
        let render = RenderGuide::new(guide, &handle);
        let out = self.dir.path().join("out.pdf");
        let opts = PdfOptions {
            overwrite: true,
            ..opts
        };
        let report = pdf::export(&render, &handle, &out, &opts).unwrap();
        let output = pdf::render(&render, &handle, &opts).unwrap();
        assert!(fs::read(&out).unwrap() == output.pdf, "export != render");
        (output, report)
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
        image: StepImage {
            full: format!("steps/{id}.webp"),
            crop: None,
            crop_rect: None,
            after: None,
            show: ImageShow::Full,
        },
        annotations: Vec::new(),
        ai_state: Default::default(),
    })
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}
