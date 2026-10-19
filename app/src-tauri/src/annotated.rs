//! Annotated guides: pages made from images and PDFs the user chose, each
//! a manual step showing its full image, on which the editor draws
//! hotspots. Images become pages and guides as core's
//! [`guiderecorder_core::pages`] makes them (which the MCP server uses too);
//! PDFs are drawn here. Blocking file IO and decoding; the commands in
//! `commands.rs` run it on the blocking pool after their file dialog.

use std::path::{Path, PathBuf};

use guiderecorder_core::model::Step;
use guiderecorder_core::pages::{
    self, create_annotated_guide, file_name, guide_title, read_page_image, stem_title,
};
use image::RgbaImage;

use crate::editor::{image_step, parse_id, read_file, AppError, ErrorKind, Result, Session};
use crate::pdf::PdfDocument;

/// Files that can become pages, by extension (also the dialog's filter).
pub const PAGE_FILE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "pdf"];

/// Most pages one import adds (all images plus all PDF pages).
pub const MAX_IMPORT_PAGES: usize = pages::MAX_IMPORT_PAGES;

/// A chosen file and how many pages it gives.
struct PageFile {
    path: PathBuf,
    pdf: bool,
    pages: usize,
}

impl Session {
    /// Makes a new annotated guide from `paths` (see [`import`]), titled
    /// after the first file, saves it into the library and indexes it as a
    /// recording is; returns its id. Nothing is left in the library when a
    /// file fails.
    pub fn create_annotated_guide(&self, paths: &[PathBuf]) -> Result<String> {
        let files = count_pages(paths)?;
        let (handle, guide) =
            create_annotated_guide(self.library(), &guide_title(&paths[0]), |handle, steps| {
                import(&files, |title, image| {
                    steps.push(image_step(handle, title, &image)?);
                    Ok(())
                })
            })?;
        self.index_bundle(&handle);
        Ok(guide.id.to_string())
    }

    /// Writes the pages of `paths` (see [`import`]) into an opened,
    /// writable guide's bundle and returns their steps. As with
    /// [`Session::add_image_step`], the guide itself is not changed: the
    /// editor inserts the steps as one undoable edit.
    pub fn add_annotated_pages(&self, guide_id: &str, paths: &[PathBuf]) -> Result<Vec<Step>> {
        let guide_id = parse_id(guide_id)?;
        // Cheap refusals first; reading and drawing are not.
        self.writable_handle(guide_id)?;
        let files = count_pages(paths)?;
        let mut steps = Vec::new();
        // A failure part way leaves the pages written so far in `steps/`,
        // used by no step: `close_guide` sweeps them.
        import(&files, |title, image| {
            let lock = self.guide_lock(guide_id);
            let _guard = lock.lock().unwrap();
            let handle = self.writable_handle(guide_id)?;
            steps.push(image_step(&handle, title, &image)?);
            Ok(())
        })?;
        Ok(steps)
    }
}

/// Checks the chosen files and counts their pages, before any is drawn:
/// PNG, JPEG and WebP images (one page each) and PDFs (opened to count),
/// at most [`MAX_IMPORT_PAGES`] pages in all.
fn count_pages(paths: &[PathBuf]) -> Result<Vec<PageFile>> {
    let too_many = |pages: usize, unit: &str| {
        AppError::new(
            ErrorKind::Invalid,
            format!("{pages} {unit} chosen; at most {MAX_IMPORT_PAGES} pages can be added at once"),
        )
    };
    if paths.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "no files chosen"));
    }
    if paths.len() > MAX_IMPORT_PAGES {
        return Err(too_many(paths.len(), "files"));
    }
    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| PAGE_FILE_EXTENSIONS.contains(&e.as_str()));
        let Some(extension) = extension else {
            return Err(AppError::new(
                ErrorKind::Invalid,
                format!(
                    "{} is not a PNG, JPEG or WebP image or a PDF",
                    file_name(path)
                ),
            ));
        };
        let pdf = extension == "pdf";
        // Each PDF is read twice, here and when drawn, so that only one is
        // in memory at a time.
        let pages = if pdf { open_pdf(path)?.page_count() } else { 1 };
        files.push(PageFile {
            path: path.clone(),
            pdf,
            pages,
        });
    }
    let pages = files.iter().map(|f| f.pages).sum();
    if pages > MAX_IMPORT_PAGES {
        return Err(too_many(pages, "pages"));
    }
    Ok(files)
}

fn open_pdf(path: &Path) -> Result<PdfDocument> {
    let bytes = read_file(path)?;
    PdfDocument::open(&bytes)
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("{}: {e}", file_name(path))))
}

/// Reads and draws each page of `files` in order, and hands it to `page`
/// with its title: an image's or a one-page PDF's file stem, else
/// `Page <n>` (counted within its PDF). Stops at the first error, which
/// names the file.
fn import(files: &[PageFile], mut page: impl FnMut(String, RgbaImage) -> Result<()>) -> Result<()> {
    for file in files {
        let named = |e: AppError| {
            AppError::new(e.kind, format!("{}: {}", file_name(&file.path), e.message))
        };
        if !file.pdf {
            let image = read_page_image(&file.path)?;
            page(stem_title(&file.path), image)?;
            continue;
        }
        let pdf = open_pdf(&file.path)?;
        let count = pdf.page_count();
        // The file may have changed since it was counted.
        if count != file.pages {
            return Err(named(AppError::new(
                ErrorKind::Conflict,
                "the file changed while it was being added",
            )));
        }
        for number in 1..=count {
            let image = pdf
                .render(number)
                .map_err(|e| named(AppError::new(ErrorKind::Invalid, e.to_string())))?;
            let title = if count == 1 {
                stem_title(&file.path)
            } else {
                format!("Page {number}")
            };
            page(title, image)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::tests::{fixture_id, library, FIXTURE};
    use crate::pdf::tests::{pdf, top_left_square, TestPage};
    use guiderecorder_core::library::{read_webp, MAX_WEBP_DIMENSION};
    use guiderecorder_core::model::{GuideKind, ImageShow, StepKind};
    use image::ImageFormat;
    use std::fs;

    /// A `width`×`height` image, red above blue, as `format` at `path`.
    fn image_file(path: &Path, width: u32, height: u32, format: ImageFormat) {
        let image = RgbaImage::from_fn(width, height, |_, y| {
            image::Rgba(if y < height / 2 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 255, 255]
            })
        });
        let image = image::DynamicImage::ImageRgba8(image);
        let image = if format == ImageFormat::Jpeg {
            image::DynamicImage::ImageRgb8(image.to_rgb8())
        } else {
            image
        };
        image.save_with_format(path, format).unwrap();
    }

    /// The bundles in the library, by folder name.
    fn bundles(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".guiderecorder"))
            .collect();
        names.sort();
        names
    }

    fn titles(steps: &[&Step]) -> Vec<String> {
        steps.iter().map(|s| s.title.clone()).collect()
    }

    #[test]
    fn large_images_are_scaled_down_not_refused() {
        // Past both the 40 MP and the per-side limit of a step image.
        let root = library();
        let session = Session::start(&root, Some(root.join("index.sqlite"))).unwrap();
        let path = root.join("plan.png");
        image_file(&path, MAX_WEBP_DIMENSION + 1, 2, ImageFormat::Png);
        let id = session.create_annotated_guide(&[path]).unwrap();
        let opened = session.open_guide(&id).unwrap();
        let page = opened.guide.steps().next().unwrap();
        let image = read_webp(&opened.path.join(&page.image.full)).unwrap();
        assert_eq!(image.dimensions(), (pages::MAX_PAGE_SIDE, 1));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn creates_a_guide_from_images_and_pdfs() {
        let root = library();
        let session = Session::start(&root, Some(root.join("index.sqlite"))).unwrap();
        let files = root.join("files");
        fs::create_dir(&files).unwrap();
        let paths = vec![
            files.join("  Quarterly   report .PNG"),
            files.join("photo.jpeg"),
            files.join("Handbook.pdf"),
            files.join("Single.pdf"),
            files.join("shot.webp"),
        ];
        image_file(&paths[0], 6, 4, ImageFormat::Png);
        image_file(&paths[1], 8, 8, ImageFormat::Jpeg);
        let square = top_left_square(100, 50);
        let pages = [
            TestPage::new(200, 100, &square),
            TestPage::new(100, 300, ""),
        ];
        fs::write(&paths[2], pdf(&pages)).unwrap();
        fs::write(&paths[3], pdf(&pages[..1])).unwrap();
        image_file(&paths[4], 3, 2, ImageFormat::WebP);
        let before = bundles(&root);

        let id = session.create_annotated_guide(&paths).unwrap();
        let opened = session.open_guide(&id).unwrap();
        assert!(!opened.read_only);
        let guide = opened.guide;
        assert_eq!(guide.kind, GuideKind::Annotated);
        assert_eq!(guide.title, "Quarterly report");
        assert!(opened.path.ends_with("Quarterly report.guiderecorder"));
        let steps: Vec<&Step> = guide.steps().collect();
        assert_eq!(
            titles(&steps),
            [
                "Quarterly report",
                "photo",
                "Page 1",
                "Page 2",
                "Single",
                "shot"
            ]
        );
        let sizes: Vec<(u32, u32)> = steps
            .iter()
            .map(|s| {
                read_webp(&opened.path.join(&s.image.full))
                    .unwrap()
                    .dimensions()
            })
            .collect();
        assert_eq!(
            sizes,
            [(6, 4), (8, 8), (400, 200), (200, 600), (400, 200), (3, 2)]
        );
        for step in &steps {
            assert_eq!(step.kind, StepKind::Manual);
            assert_eq!(step.image.full, format!("steps/{}.webp", step.id));
            assert_eq!(step.image.crop, None);
            assert_eq!(step.image.after, None);
            assert_eq!(step.image.show, ImageShow::Full);
            assert!(step.description.is_empty() && step.annotations.is_empty());
            assert!(step.point.is_none() && step.element.is_none());
        }
        // The pixels are the file's: red above blue.
        let png = read_webp(&opened.path.join(&steps[0].image.full)).unwrap();
        assert_eq!(png.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(png.get_pixel(5, 3).0, [0, 0, 255, 255]);
        // The PDF page as drawn: dark top-left, white elsewhere.
        let page = read_webp(&opened.path.join(&steps[2].image.full)).unwrap();
        assert!(page.get_pixel(5, 5).0[..3].iter().all(|&c| c < 40));
        assert_eq!(page.get_pixel(395, 195).0, [255, 255, 255, 255]);

        // Listed (from the index) like a recording.
        let listed = session
            .list_guides(Some("Quarterly report".into()))
            .unwrap();
        assert_eq!(listed.first().map(|g| g.id.as_str()), Some(id.as_str()));
        assert_eq!(listed[0].step_count, 6);
        assert_eq!(bundles(&root).len(), before.len() + 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_bad_file_leaves_no_guide() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let files = root.join("files");
        fs::create_dir(&files).unwrap();
        let good = files.join("good.png");
        image_file(&good, 2, 2, ImageFormat::Png);
        let before = bundles(&root);
        let listed = session.list_guides(None).unwrap().len();

        let broken = files.join("broken.jpg");
        fs::write(&broken, b"not a jpeg").unwrap();
        let notes = files.join("notes.txt");
        fs::write(&notes, b"x").unwrap();
        let bad_pdf = files.join("bad.pdf");
        fs::write(&bad_pdf, b"not a pdf").unwrap();
        let big = files.join("big.png");
        fs::File::create(&big)
            .unwrap()
            .set_len(pages::MAX_FILE_BYTES + 1)
            .unwrap();
        // Not an image despite its name: a PDF.
        let disguised = files.join("disguised.png");
        fs::write(&disguised, pdf(&[TestPage::new(10, 10, "")])).unwrap();
        let cases = [
            (broken, "broken.jpg: not a readable PNG, JPEG or WebP image"),
            (notes, "notes.txt is not a PNG, JPEG or WebP image or a PDF"),
            (bad_pdf, "bad.pdf: not a readable PDF"),
            (big, "is too large (over 64 MB)"),
            (
                disguised,
                "disguised.png: not a readable PNG, JPEG or WebP image",
            ),
            (files.join("missing.png"), "missing.png"),
        ];
        for (bad, message) in cases {
            // The good file is written first, then the bad one fails.
            let err = session
                .create_annotated_guide(&[good.clone(), bad.clone()])
                .unwrap_err();
            assert!(err.message.contains(message), "{message}: {}", err.message);
            assert_eq!(bundles(&root), before, "{}", bad.display());
        }
        assert_eq!(session.list_guides(None).unwrap().len(), listed);
        let none = session.create_annotated_guide(&[]).unwrap_err();
        assert_eq!(none.kind, ErrorKind::Invalid);
        // Nothing half-made is left for recovery either.
        let hidden: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().starts_with('.'))
            .collect();
        assert!(hidden.is_empty(), "{hidden:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn at_most_a_hundred_pages() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let before = bundles(&root);
        let files = root.join("files");
        fs::create_dir(&files).unwrap();
        let page = TestPage::new(10, 10, "");
        let pages: Vec<TestPage<'_>> = (0..MAX_IMPORT_PAGES)
            .map(|_| TestPage::new(page.size.0, page.size.1, page.content))
            .collect();
        let hundred = files.join("hundred.pdf");
        fs::write(&hundred, pdf(&pages)).unwrap();
        let image = files.join("one more.png");
        image_file(&image, 2, 2, ImageFormat::Png);

        let err = session
            .create_annotated_guide(&[image.clone(), hundred.clone()])
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);
        assert_eq!(
            err.message,
            "101 pages chosen; at most 100 pages can be added at once"
        );
        let many: Vec<PathBuf> = vec![image.clone(); MAX_IMPORT_PAGES + 1];
        let err = session.create_annotated_guide(&many).unwrap_err();
        assert_eq!(
            err.message,
            "101 files chosen; at most 100 pages can be added at once"
        );
        assert_eq!(bundles(&root), before);

        // Exactly a hundred is fine.
        let id = session.create_annotated_guide(&[hundred]).unwrap();
        let guide = session.open_guide(&id).unwrap().guide;
        assert_eq!(guide.title, "hundred");
        assert_eq!(guide.steps().count(), MAX_IMPORT_PAGES);
        assert_eq!(
            guide.steps().last().map(|s| s.title.as_str()),
            Some("Page 100")
        );
        // The cap is per import, not per guide.
        assert_eq!(session.add_annotated_pages(&id, &[image]).unwrap().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn adds_pages_to_an_open_guide_without_changing_it() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let bundle = root.join(FIXTURE);
        let files = root.join("files");
        fs::create_dir(&files).unwrap();
        let image = files.join("Map.png");
        image_file(&image, 4, 2, ImageFormat::Png);
        let two = files.join("two.pdf");
        fs::write(
            &two,
            pdf(&[TestPage::new(10, 20, ""), TestPage::new(30, 10, "")]),
        )
        .unwrap();

        // Not open yet.
        let err = session
            .add_annotated_pages(&id, std::slice::from_ref(&image))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotOpen);
        let err = session.add_annotated_pages("nope", &[]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);

        let guide = session.open_guide(&id).unwrap().guide;
        let json = fs::read(bundle.join("guide.json")).unwrap();
        let steps = session
            .add_annotated_pages(&id, &[two.clone(), image.clone()])
            .unwrap();
        assert_eq!(
            steps.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            ["Page 1", "Page 2", "Map"]
        );
        for (step, size) in steps.iter().zip([(20, 40), (60, 20), (4, 2)]) {
            assert_eq!(step.kind, StepKind::Manual);
            assert_eq!(step.image.show, ImageShow::Full);
            let stored = read_webp(&bundle.join(&step.image.full)).unwrap();
            assert_eq!(stored.dimensions(), size);
            // Loadable before the editor saves them.
            assert!(session.step_image(&id, &step.image.full).is_ok());
        }
        // The guide is the editor's to change.
        assert_eq!(fs::read(bundle.join("guide.json")).unwrap(), json);
        assert_eq!(session.open_guide(&id).unwrap().guide, guide);

        // A bad file after a good one: the good one's image is left behind,
        // unused, and the close sweeps it.
        let bad = files.join("bad.webp");
        fs::write(&bad, b"RIFF").unwrap();
        let steps_before: Vec<_> = fs::read_dir(bundle.join("steps")).unwrap().collect();
        let err = session
            .add_annotated_pages(&id, &[image.clone(), bad])
            .unwrap_err();
        assert!(err.message.starts_with("bad.webp: "), "{}", err.message);
        let stray = fs::read_dir(bundle.join("steps")).unwrap().count() - steps_before.len();
        assert_eq!(stray, 1);
        let removed = session.close_guide(&id).unwrap();
        // The three unsaved pages and the stray one.
        assert_eq!(removed.len(), 4);
        assert_eq!(
            fs::read_dir(bundle.join("steps")).unwrap().count(),
            steps_before.len() - 3
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn add_annotated_pages_refuses_read_only_guides() {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let path = root.join(FIXTURE).join("guide.json");
        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        json["schemaVersion"] = (guiderecorder_core::SCHEMA_VERSION + 1).into();
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        session.open_guide(&id).unwrap();
        let image = root.join("page.png");
        image_file(&image, 2, 2, ImageFormat::Png);
        let before = fs::read_dir(root.join(FIXTURE).join("steps"))
            .unwrap()
            .count();
        let err = session.add_annotated_pages(&id, &[image]).unwrap_err();
        assert_eq!(err.kind, ErrorKind::ReadOnly);
        assert_eq!(
            fs::read_dir(root.join(FIXTURE).join("steps"))
                .unwrap()
                .count(),
            before
        );
        fs::remove_dir_all(root).unwrap();
    }
}
