//! Exporting a collection (#21): fixture guides as chapters of one
//! document, in Markdown, HTML, Word and PDF.
//!
//! The Markdown golden is `tests/golden/collection/manual.md.golden`;
//! regenerate it with `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core
//! --test export_collection` and review the diff.

mod support;

use std::fs;
use std::io::Read;

use guiderecorder_core::collection::{self, Collection, Entry, Numbering, RenderedCollection};
use guiderecorder_core::export::{
    docx, html, md, pdf, ExportReport, RenderGuide, RenderItem, RenderWarning,
};
use guiderecorder_core::library::{GuideHandle, Library};
use guiderecorder_core::model::Guide;
use guiderecorder_core::Ulid;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

/// A temp copy of the fixture walkthroughs, by title.
struct Fixtures {
    dir: tempfile::TempDir,
    guides: Vec<(Guide, GuideHandle)>,
}

fn fixtures() -> Fixtures {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture_guides_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "guiderecorder") {
            copy_dir(&path, &dir.path().join(path.file_name().unwrap()));
        }
    }
    let library = Library::open(dir.path()).unwrap();
    let guides = library
        .list()
        .unwrap()
        .into_iter()
        .map(|summary| {
            let loaded = library.read(&summary.path).unwrap();
            (loaded.guide, loaded.handle)
        })
        .collect();
    Fixtures { dir, guides }
}

impl Fixtures {
    fn titled(&self, title: &str) -> &Guide {
        &self
            .guides
            .iter()
            .find(|(g, _)| g.title == title)
            .unwrap()
            .0
    }

    /// "Share a calendar event", a text, "Rotate an API key" and a guide
    /// that is gone, as a collection numbered by `numbering`.
    fn manual(&self, numbering: Numbering) -> (Collection, RenderedCollection) {
        let id = |n: u128| Ulid(n);
        let mut collection = Collection::new("Team handbook");
        collection.id = id(1);
        collection.intro = "Everything a new **teammate** needs.".into();
        collection.numbering = numbering;
        collection.entries = vec![
            Entry::Guide {
                id: id(2),
                guide_id: self.titled("Share a calendar event").id,
            },
            Entry::Text {
                id: id(3),
                body: "Next, the *security* basics.".into(),
            },
            Entry::Guide {
                id: id(4),
                guide_id: id(99),
            },
            Entry::Guide {
                id: id(5),
                guide_id: self.titled("Rotate an API key").id,
            },
        ];
        let rendered = collection::render(&collection, self.dir.path(), |guide_id| {
            self.guides.iter().find(|(g, _)| g.id == guide_id).cloned()
        });
        (collection, rendered)
    }
}

/// Asserts that every step image was read: each from its own guide's
/// bundle. (The fixtures' other warnings, such as an unknown crop
/// offset, are theirs.)
fn assert_images_found(report: &ExportReport) {
    let unavailable: Vec<_> = report
        .warnings
        .iter()
        .filter(|(_, w)| matches!(w, RenderWarning::ImageUnavailable { .. }))
        .collect();
    assert!(unavailable.is_empty(), "{unavailable:?}");
}

fn step_count(guide: &Guide) -> usize {
    RenderGuide::new(
        guide,
        &GuideHandle {
            id: guide.id,
            path: "/x".into(),
        },
    )
    .steps()
    .count()
}

#[test]
fn markdown_has_contents_and_chapters() {
    let fixtures = fixtures();
    let (_, rendered) = fixtures.manual(Numbering::Continuous);
    assert_eq!(rendered.missing, [Ulid(99)]);
    let out = tempfile::tempdir().unwrap();
    let dir = out.path().join("manual");
    let report = md::export(
        &rendered.guide,
        &rendered.guide.handle,
        &dir,
        &md::MdOptions::default(),
    )
    .unwrap();
    assert_images_found(&report);
    let markdown = fs::read_to_string(dir.join(md::GUIDE_FILE)).unwrap();
    assert_golden_text("collection/manual.md.golden", &markdown);

    // Every step of both guides has its image, named by its anchor.
    let total = step_count(fixtures.titled("Share a calendar event"))
        + step_count(fixtures.titled("Rotate an API key"));
    let images = fs::read_dir(dir.join("images")).unwrap().count();
    assert_eq!(images, total);
}

#[test]
fn per_chapter_numbers_restart_in_every_format() {
    let fixtures = fixtures();
    let (_, rendered) = fixtures.manual(Numbering::PerChapter);
    let render = &rendered.guide;
    // The first step after each chapter heading.
    let mut first_steps = Vec::new();
    let mut wanted = false;
    for item in &render.items {
        match item {
            RenderItem::Chapter(_) => wanted = true,
            RenderItem::Step(step) if wanted => {
                first_steps.push(step.number);
                wanted = false;
            }
            _ => {}
        }
    }
    assert_eq!(first_steps, [1, 1]);
    let anchors: Vec<u32> = render.steps().map(|s| s.anchor).collect();
    let expected: Vec<u32> = (1..=anchors.len() as u32).collect();
    assert_eq!(anchors, expected);

    let (html, _) =
        html::render_string(render, &render.handle, &html::HtmlOptions::default()).unwrap();
    // Ids stay unique although two steps are numbered 1.
    for anchor in &anchors {
        assert_eq!(
            html.matches(&format!("id=\"step-{anchor}\"")).count(),
            1,
            "step-{anchor}"
        );
    }
}

#[test]
fn html_links_its_contents_to_the_chapters() {
    let fixtures = fixtures();
    let (_, rendered) = fixtures.manual(Numbering::Continuous);
    let render = &rendered.guide;
    let (html, report) =
        html::render_string(render, &render.handle, &html::HtmlOptions::default()).unwrap();
    assert_images_found(&report);
    assert!(html.contains("<nav class=\"contents\" aria-label=\"Contents\">"));
    assert!(html.contains("<a href=\"#chapter-1\">Share a calendar event</a>"));
    assert!(html.contains("<a href=\"#chapter-2\">Rotate an API key</a>"));
    assert!(html.contains("<h2 class=\"chapter\" id=\"chapter-2\">Rotate an API key</h2>"));
    // The text leading into chapter 2 starts its printed page, not the chapter.
    assert!(html.contains("<h2 class=\"chapter page-break\" id=\"chapter-1\">"));
    assert!(html
        .contains("<div class=\"text page-break\">\n<p>Next, the <em>security</em> basics.</p>"));
    // Sections sit a level below the chapters, and steps below them.
    assert!(html.contains("<h3 class=\"section\">Open the event</h3>"));
    assert!(html.contains("<h4 class=\"step-title\">"));
    assert!(!html.contains("<h2 class=\"step-title\">"));
    assert!(html.contains(".chapter { margin-top"));
}

#[test]
fn word_bookmarks_its_chapters() {
    let fixtures = fixtures();
    let (_, rendered) = fixtures.manual(Numbering::Continuous);
    let render = &rendered.guide;
    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("manual.docx");
    let report =
        docx::export(render, &render.handle, &path, &docx::DocxOptions::default()).unwrap();
    assert_images_found(&report);
    let mut zip = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    let part = |zip: &mut zip::ZipArchive<fs::File>, name: &str| {
        let mut text = String::new();
        zip.by_name(name)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        text
    };
    let document = part(&mut zip, "word/document.xml");
    let styles = part(&mut zip, "word/styles.xml");
    for n in 1..=2 {
        assert!(
            document.contains(&format!("w:anchor=\"chapter-{n}\"")),
            "link {n}"
        );
        assert!(
            document.contains(&format!("w:name=\"chapter-{n}\"")),
            "bookmark {n}"
        );
    }
    assert!(document.contains("Contents"));
    assert!(document.contains("<w:pageBreakBefore"));
    assert!(document.contains("w:val=\"Heading3\""));
    assert!(styles.contains("w:styleId=\"Heading3\""));
}

#[test]
fn pdf_lists_chapters_with_pages() {
    let fixtures = fixtures();
    let (_, rendered) = fixtures.manual(Numbering::Continuous);
    let render = &rendered.guide;
    let output = pdf::render(render, &render.handle, &pdf::PdfOptions::default()).unwrap();
    assert_images_found(&output.report);
    // The cover, then the intro and contents.
    let contents = &output.page_text[1];
    assert!(contents.contains("Contents"), "{contents}");
    assert!(contents.contains("Share a calendar event"), "{contents}");
    assert!(contents.contains("Rotate an API key"), "{contents}");
    // Each chapter starts a page; the text leading into the second starts
    // its page. A page's first line is the header (the collection title).
    let openings: Vec<Vec<&str>> = output
        .page_text
        .iter()
        .skip(2)
        .map(|page| page.lines().skip(1).take(2).collect())
        .collect();
    assert!(
        openings.contains(&vec![
            "Share a calendar event",
            "Invite a colleague to an existing Calendar event."
        ]),
        "{openings:?}"
    );
    assert!(
        openings.contains(&vec!["Next, the security basics.", "Rotate an API key"]),
        "{openings:?}"
    );
}

#[test]
fn a_collection_of_nothing_still_exports() {
    let dir = tempfile::tempdir().unwrap();
    let collection = Collection::new("Empty");
    let rendered = collection::render(&collection, dir.path(), |_| None);
    let render = &rendered.guide;
    assert!(!render.has_chapters());
    let out = dir.path().join("empty");
    md::export(render, &render.handle, &out, &md::MdOptions::default()).unwrap();
    assert_eq!(
        fs::read_to_string(out.join(md::GUIDE_FILE)).unwrap(),
        "# Empty\n"
    );
    pdf::render(render, &render.handle, &pdf::PdfOptions::default()).unwrap();
}
