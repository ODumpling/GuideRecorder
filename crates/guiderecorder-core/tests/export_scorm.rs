//! SCORM 1.2 export (#23).
//!
//! Goldens are under `tests/golden/scorm/`: the manifest per fixture guide,
//! and one SCO page with a quiz (base64 image data replaced by `…`), which
//! the app's e2e tests also load to run the SCORM runtime against a mocked
//! LMS. Regenerate them with
//! `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core --test export_scorm`
//! and review the diff.

mod support;

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use guiderecorder_core::export::md::slug;
use guiderecorder_core::export::scorm::{
    self, parse_quiz, Completion, ScormError, ScormOptions, MANIFEST_FILE, SCO_FILE,
};
use guiderecorder_core::export::{walkthrough, RenderGuide};
use guiderecorder_core::library::Library;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

const QUIZ: &str = "Where do you start?\n- [x] The first step\n- [ ] The last step\n\n\
                    Can you go back a step?\n- [ ] No\n- [x] Yes, with Back or ←\n";

/// The zip's entries by name, in order.
fn entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut file = zip.by_index(i).unwrap();
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).unwrap();
            (file.name().to_owned(), bytes)
        })
        .collect()
}

/// Checks a manifest against the package: well-formed XML with the
/// SCORM 1.2 metadata, unique identifiers, references that resolve, and
/// every file in the zip listed (and every listed file in the zip).
fn check_manifest(xml: &str, names: &[&str]) {
    let doc = roxmltree::Document::parse(xml).expect("well-formed manifest");
    let root = doc.root_element();
    assert_eq!(root.tag_name().name(), "manifest");
    assert_eq!(
        root.tag_name().namespace(),
        Some("http://www.imsproject.org/xsd/imscp_rootv1p1p2")
    );
    let text = |name: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name(name))
            .and_then(|n| n.text())
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(text("schema"), "ADL SCORM");
    assert_eq!(text("schemaversion"), "1.2");

    let mut ids = HashSet::new();
    for node in doc.descendants().filter(|n| n.is_element()) {
        if let Some(id) = node.attribute("identifier") {
            // An XML ID (NCName), unique in the document.
            assert!(
                id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')),
                "{id}"
            );
            assert!(ids.insert(id.to_owned()), "duplicate identifier {id}");
        }
    }
    let element = |name: &str| doc.descendants().find(|n| n.has_tag_name(name)).unwrap();
    let organizations = element("organizations");
    let organization = element("organization");
    assert_eq!(
        organizations.attribute("default"),
        organization.attribute("identifier")
    );
    let resources: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("resource"))
        .collect();
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let target = item.attribute("identifierref").unwrap();
        assert!(
            resources
                .iter()
                .any(|r| r.attribute("identifier") == Some(target)),
            "{target}"
        );
    }
    let adlcp = "http://www.adlnet.org/xsd/adlcp_rootv1p2";
    let mut listed = HashSet::new();
    for resource in &resources {
        assert_eq!(resource.attribute((adlcp, "scormtype")), Some("sco"));
        assert_eq!(resource.attribute("type"), Some("webcontent"));
        let href = resource.attribute("href").unwrap();
        assert!(names.contains(&href), "{href}");
        for file in resource.children().filter(|n| n.has_tag_name("file")) {
            let href = file.attribute("href").unwrap();
            assert!(names.contains(&href), "{href} is not in the package");
            listed.insert(href);
        }
    }
    for name in names.iter().filter(|n| **n != MANIFEST_FILE) {
        assert!(listed.contains(name), "{name} is not listed");
    }
}

/// Replaces the payload of every base64 data URI with `…`.
fn strip_data(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find(";base64,") {
        let (head, tail) = rest.split_at(i + ";base64,".len());
        out.push_str(head);
        out.push('…');
        let end = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')))
            .unwrap_or(tail.len());
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

#[test]
fn fixture_packages() {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture_guides_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "guiderecorder") {
            copy_dir(&path, &dir.path().join(path.file_name().unwrap()));
        }
    }
    let library = Library::open(dir.path()).unwrap();
    let summaries = library.list().unwrap();
    assert!(!summaries.is_empty());
    let quiz = parse_quiz(QUIZ).unwrap();
    for summary in summaries {
        let loaded = library.read(&summary.path).unwrap();
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let name = slug(&loaded.guide.title);
        let out = tempfile::tempdir().unwrap();
        let path = out.path().join("course.zip");
        // The quiz rule on the fixture with a quiz; the others complete on
        // the last step.
        let with_quiz = name == "rotate-an-api-key";
        let opts = ScormOptions {
            completion: if with_quiz {
                Completion::QuizPassed
            } else {
                Completion::LastStep
            },
            quiz: if with_quiz { quiz.clone() } else { Vec::new() },
            ..ScormOptions::default()
        };
        let report = scorm::export(&render, &loaded.handle, &path, &opts).unwrap();
        assert_eq!(report.files, std::slice::from_ref(&path));

        let entries = entries(&path);
        let names: Vec<&str> = entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, [MANIFEST_FILE, SCO_FILE]);
        let manifest = String::from_utf8(entries[0].1.clone()).unwrap();
        check_manifest(&manifest, &names);
        assert_golden_text(&format!("scorm/{name}/imsmanifest.xml.golden"), &manifest);
        assert!(manifest.contains(&format!("identifier=\"guiderecorder-{}\"", loaded.guide.id)));
        assert_eq!(manifest.contains("masteryscore"), with_quiz);

        // The SCO is the walkthrough with both scripts allowed by hash.
        let page = String::from_utf8(entries[1].1.clone()).unwrap();
        assert!(page.contains("id=\"scorm\""));
        assert!(page.contains(&format!("<script>{}</script>", scorm::SCRIPT)));
        assert!(page.contains(&format!("<script>{}</script>", walkthrough::SCRIPT)));
        let csp = page
            .split("http-equiv=\"Content-Security-Policy\" content=\"")
            .nth(1)
            .unwrap();
        let csp = &csp[..csp.find('"').unwrap()];
        assert_eq!(csp.matches("sha256-").count(), 2, "{csp}");
        assert_eq!(page.contains("id=\"scorm-quiz\""), with_quiz);
        if with_quiz {
            assert_golden_text(
                &format!("scorm/{name}/guide.html.golden"),
                &strip_data(&page),
            );
        }

        // The same options give the same bytes.
        let again = out.path().join("again.zip");
        scorm::export(&render, &loaded.handle, &again, &opts).unwrap();
        assert_eq!(fs::read(&path).unwrap(), fs::read(&again).unwrap());
    }
}

#[test]
fn options_name_the_course_and_existing_files_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let source = fs::read_dir(fixture_guides_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "guiderecorder"))
        .unwrap();
    copy_dir(&source, &dir.path().join(source.file_name().unwrap()));
    let library = Library::open(dir.path()).unwrap();
    let summary = library.list().unwrap().remove(0);
    let loaded = library.read(&summary.path).unwrap();
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);

    let out = tempfile::tempdir().unwrap();
    let path = out.path().join("course.zip");
    let opts = ScormOptions {
        title: Some("  Onboarding:  API keys ".into()),
        identifier: Some("2026 onboarding/api keys".into()),
        ..ScormOptions::default()
    };
    scorm::export(&render, &loaded.handle, &path, &opts).unwrap();
    let manifest = String::from_utf8(entries(&path).remove(0).1).unwrap();
    assert!(manifest.contains("<manifest identifier=\"course-2026-onboarding-api-keys\""));
    assert!(manifest.contains("<title>Onboarding: API keys</title>"));

    // Not replaced without overwrite.
    let err = scorm::export(&render, &loaded.handle, &path, &opts).unwrap_err();
    assert!(matches!(err, ScormError::Exists(p) if p == path));
    let replace = ScormOptions {
        overwrite: true,
        ..opts
    };
    scorm::export(&render, &loaded.handle, &path, &replace).unwrap();

    // The quiz rule needs a quiz; nothing is written.
    let missing = out.path().join("missing.zip");
    let quiz_rule = ScormOptions {
        completion: Completion::QuizPassed,
        ..ScormOptions::default()
    };
    let err = scorm::export(&render, &loaded.handle, &missing, &quiz_rule).unwrap_err();
    assert!(matches!(err, ScormError::Invalid(_)), "{err}");
    assert!(!missing.exists());
}
