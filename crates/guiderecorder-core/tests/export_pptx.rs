//! PPTX export (#8): package parts, slide order, speaker notes.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use guiderecorder_core::export::pptx::{self, PptxError, PptxOptions};
use guiderecorder_core::export::RenderGuide;
use guiderecorder_core::library::Library;
use support::{copy_dir, fixture_guides_dir};

/// Every part of the package by name.
fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut file = zip.by_index(i).unwrap();
            let mut data = Vec::new();
            file.read_to_end(&mut data).unwrap();
            (file.name().to_owned(), data)
        })
        .collect()
}

/// The text of every `<a:t>` in an XML part, joined with spaces.
fn text(xml: &[u8]) -> String {
    let xml = std::str::from_utf8(xml).unwrap();
    let doc = roxmltree::Document::parse(xml).unwrap();
    doc.descendants()
        .filter(|n| n.has_tag_name("t"))
        .filter_map(|n| n.text())
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn slides_follow_the_guide_with_notes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("library");
    copy_dir(&fixture_guides_dir(), &root);
    let library = Library::open(&root).unwrap();
    let loaded = library
        .read(root.join("Share a calendar event.guiderecorder"))
        .unwrap();
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let out = dir.path().join("share.pptx");
    let opts = PptxOptions {
        timestamp: Some(loaded.guide.updated_at),
        ..PptxOptions::default()
    };
    let report = pptx::export(&render, &out, &opts).unwrap();
    assert_eq!(report.files, std::slice::from_ref(&out));

    let parts = parts(&out);
    for (name, data) in &parts {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            let xml = std::str::from_utf8(data).unwrap();
            roxmltree::Document::parse(xml).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
    // Title slide, then every item: 5 steps, 2 sections, 2 notes.
    let slides = parts
        .keys()
        .filter(|n| n.starts_with("ppt/slides/slide"))
        .count();
    assert_eq!(slides, 1 + 9);
    let images = parts.keys().filter(|n| n.starts_with("ppt/media/")).count();
    assert_eq!(images, 5);
    assert!(text(&parts["ppt/slides/slide1.xml"]).contains("Share a calendar event"));
    assert_eq!(text(&parts["ppt/slides/slide2.xml"]), "Open the event");
    let step = text(&parts["ppt/slides/slide3.xml"]);
    assert!(
        step.starts_with("1.") && step.contains("Calendar"),
        "{step}"
    );
    // Every step slide with a description has it in its speaker notes too.
    for (name, data) in &parts {
        let Some(n) = name
            .strip_prefix("ppt/notesSlides/notesSlide")
            .and_then(|n| n.strip_suffix(".xml"))
        else {
            continue;
        };
        let slide = text(&parts[&format!("ppt/slides/slide{n}.xml")]);
        let notes = text(data);
        assert!(!notes.is_empty());
        if n != "1" {
            assert!(
                slide.contains(&notes),
                "slide {n}: {notes:?} not in {slide:?}"
            );
        }
    }

    // Same guide, same bytes; no overwrite unless asked.
    let again = dir.path().join("again.pptx");
    pptx::export(&render, &again, &opts).unwrap();
    assert_eq!(fs::read(&out).unwrap(), fs::read(&again).unwrap());
    assert!(matches!(
        pptx::export(&render, &out, &opts),
        Err(PptxError::Exists(p)) if p == out
    ));
}
