//! Exporting the open guide from the app (design #443; the CLI's
//! `guiderecorder export` is the reference), plus the crop check the annotation
//! layer uses before it shows a crop view.
//!
//! The webview never names a file the app reads or writes on its own: the
//! destination and the logo come from native dialogs run here, and only
//! paths those dialogs returned (and, for "Reveal in Finder", files an
//! export wrote) are accepted later. So no dialog or opener permission is
//! granted to the webview.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use guiderecorder_core::export::md::{Branding, MAX_LOGO_BYTES};
use guiderecorder_core::export::{
    docx, html, md, pdf, verify_crop_rect, ExportReport, OutPathError, RenderGuide,
};
use guiderecorder_core::model::StepImage;
use serde::{Deserialize, Serialize};

use crate::editor::{check_image_path, file_error, parse_id, AppError, ErrorKind, Result, Session};

/// The export formats, as the UI and the CLI name them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Md,
    Html,
    Docx,
    #[default]
    Pdf,
}

impl Format {
    /// Markdown and HTML write into a folder; DOCX and PDF write one file.
    pub fn is_folder(self) -> bool {
        matches!(self, Self::Md | Self::Html)
    }

    /// The file extension of a DOCX or PDF export.
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Md | Self::Html => None,
            Self::Docx => Some("docx"),
            Self::Pdf => Some("pdf"),
        }
    }
}

/// Branding as the UI sends it: a logo path the user chose with
/// [`choose_logo`](crate::commands::choose_logo) and an accent colour.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandingRequest {
    pub logo: Option<PathBuf>,
    pub accent: Option<String>,
}

/// A per-step image warning of an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportWarning {
    pub step_id: String,
    pub message: String,
}

/// `export_guide`'s result: the [`ExportReport`] for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    /// Files written, the main document first.
    pub files: Vec<PathBuf>,
    pub warnings: Vec<ExportWarning>,
    pub notes: Vec<String>,
}

impl From<ExportReport> for ExportResult {
    fn from(report: ExportReport) -> Self {
        Self {
            files: report.files,
            warnings: report
                .warnings
                .into_iter()
                .map(|(step, warning)| ExportWarning {
                    step_id: step.to_string(),
                    message: warning.to_string(),
                })
                .collect(),
            notes: report.notes,
        }
    }
}

/// Most paths each list of [`ChosenPaths`] keeps (the oldest go first).
const MAX_CHOSEN: usize = 32;

/// Paths the user picked in a native dialog, and files exports wrote. The
/// only paths `export_guide` writes to, reads a logo from, or reveals.
#[derive(Debug, Default)]
pub struct ChosenPaths {
    inner: Mutex<Chosen>,
}

#[derive(Debug, Default)]
struct Chosen {
    /// Folders a Markdown or HTML export may be written into (as a new or
    /// earlier export folder inside them).
    folders: Vec<PathBuf>,
    /// DOCX and PDF files chosen in a save dialog.
    files: Vec<PathBuf>,
    logos: Vec<PathBuf>,
    /// Library folders chosen in Settings.
    libraries: Vec<PathBuf>,
    /// Files earlier exports wrote, for "Reveal in Finder".
    exported: Vec<PathBuf>,
}

fn remember(list: &mut Vec<PathBuf>, path: PathBuf) {
    list.retain(|p| p != &path);
    list.push(path);
    if list.len() > MAX_CHOSEN {
        list.remove(0);
    }
}

impl ChosenPaths {
    pub fn add_folder(&self, path: PathBuf) {
        remember(&mut self.inner.lock().unwrap().folders, path);
    }

    pub fn add_file(&self, path: PathBuf) {
        remember(&mut self.inner.lock().unwrap().files, path);
    }

    pub fn add_logo(&self, path: PathBuf) {
        remember(&mut self.inner.lock().unwrap().logos, path);
    }

    pub fn add_library(&self, path: PathBuf) {
        remember(&mut self.inner.lock().unwrap().libraries, path);
    }

    pub fn is_folder(&self, path: &Path) -> bool {
        self.inner.lock().unwrap().folders.iter().any(|p| p == path)
    }

    pub fn is_logo(&self, path: &Path) -> bool {
        self.inner.lock().unwrap().logos.iter().any(|p| p == path)
    }

    pub fn is_library(&self, path: &Path) -> bool {
        self.inner
            .lock()
            .unwrap()
            .libraries
            .iter()
            .any(|p| p == path)
    }

    fn add_exported(&self, path: PathBuf) {
        remember(&mut self.inner.lock().unwrap().exported, path);
    }

    /// Checks that the UI's `out` is a destination the user chose: a DOCX
    /// or PDF file from the save dialog, or for Markdown and HTML a plain
    /// folder name inside a folder from the folder dialog.
    pub fn check_destination(&self, format: Format, out: &Path) -> Result<()> {
        let chosen = self.inner.lock().unwrap();
        let ok = if format.is_folder() {
            let name_ok = out.file_name().is_some_and(|name| {
                Path::new(name).components().count() == 1
                    && matches!(
                        Path::new(name).components().next(),
                        Some(Component::Normal(_))
                    )
            });
            name_ok
                && out
                    .parent()
                    .is_some_and(|parent| chosen.folders.iter().any(|f| f == parent))
        } else {
            let ext_ok = out
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| Some(e.to_ascii_lowercase().as_str()) == format.extension());
            ext_ok && chosen.files.iter().any(|f| f == out)
        };
        if ok {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorKind::Invalid,
                format!("{} was not chosen as the export destination", out.display()),
            ))
        }
    }

    fn check_logo(&self, path: &Path) -> Result<()> {
        if self.is_logo(path) {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorKind::Invalid,
                format!("{} was not chosen as the logo", path.display()),
            ))
        }
    }

    /// Checks that `path` is a file an export wrote.
    pub fn check_exported(&self, path: &Path) -> Result<()> {
        if self
            .inner
            .lock()
            .unwrap()
            .exported
            .iter()
            .any(|p| p == path)
        {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorKind::Invalid,
                format!("{} is not an exported file", path.display()),
            ))
        }
    }
}

/// `path` with `extension` added unless it already ends in it (any case):
/// a save dialog may return the name as typed.
pub fn with_extension(path: PathBuf, extension: &str) -> PathBuf {
    let has = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(extension));
    if has {
        return path;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(extension);
    path.with_file_name(name)
}

/// Exports a guide from its saved bundle (the editor flushes its changes
/// first) to `out` on `paper` (DOCX and PDF only), with the review details line when `review_details`, which [`ChosenPaths::check_destination`] has accepted.
/// The guide lock is held throughout, so no save or close sweep changes the
/// bundle while it is read.
#[allow(clippy::too_many_arguments)]
pub fn export_guide(
    session: &Session,
    chosen: &ChosenPaths,
    guide_id: &str,
    format: Format,
    out: &Path,
    overwrite: bool,
    branding: &BrandingRequest,
    paper: docx::Paper,
    review_details: bool,
) -> Result<ExportResult> {
    let id = parse_id(guide_id)?;
    chosen.check_destination(format, out)?;
    // Checked (and the logo decoded) once, before anything is written.
    let logo = match &branding.logo {
        Some(path) => {
            chosen.check_logo(path)?;
            Some(read_logo(path)?)
        }
        None => None,
    };
    let accent = branding
        .accent
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_owned);
    let branding = Branding { logo, accent }
        .check()
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("branding: {e}")))?;

    let lock = session.guide_lock(id);
    let _guard = lock.lock().unwrap();
    let handle = session.bundle(id)?;
    check_out_path(out, &handle.path, session.root())?;
    let loaded = session.library().read(&handle.path)?;
    if loaded.guide.id != id {
        return Err(AppError::new(
            ErrorKind::Conflict,
            format!("{} no longer holds guide {id}", handle.path.display()),
        ));
    }
    let guide = loaded.guide;
    let handle = loaded.handle;
    let render = RenderGuide::new(&guide, &handle);
    let timestamp = Some(guide.updated_at);

    let report = match format {
        Format::Md => {
            let opts = md::MdOptions {
                overwrite,
                branding,
                review_details,
                ..md::MdOptions::default()
            };
            md::export(&render, &handle, out, &opts).map_err(md_error)?
        }
        Format::Html => {
            let opts = html::HtmlOptions {
                overwrite,
                branding,
                review_details,
                ..html::HtmlOptions::default()
            };
            html::export(&render, &handle, out, &opts).map_err(md_error)?
        }
        Format::Docx => {
            let opts = docx::DocxOptions {
                overwrite,
                timestamp,
                paper,
                branding,
                review_details,
                ..docx::DocxOptions::default()
            };
            docx::export(&render, &handle, out, &opts).map_err(|e| match e {
                docx::DocxError::Exists(path) => exists(&path),
                e => AppError::new(ErrorKind::Io, e.to_string()),
            })?
        }
        Format::Pdf => {
            let opts = pdf::PdfOptions {
                overwrite,
                timestamp,
                paper,
                branding,
                review_details,
                ..pdf::PdfOptions::default()
            };
            pdf::export(&render, &handle, out, &opts).map_err(|e| match e {
                pdf::PdfError::Exists(path) => exists(&path),
                e => AppError::new(ErrorKind::Io, e.to_string()),
            })?
        }
    };
    let result = ExportResult::from(report);
    if let Some(main) = result.files.first() {
        chosen.add_exported(main.clone());
    }
    Ok(result)
}

fn exists(path: &Path) -> AppError {
    AppError::new(
        ErrorKind::Exists,
        format!("{} already exists", path.display()),
    )
}

fn md_error(e: md::ExportError) -> AppError {
    match e {
        md::ExportError::NotEmpty(path) => AppError::new(
            ErrorKind::Exists,
            format!("{} already has files in it", path.display()),
        ),
        e => AppError::new(ErrorKind::Io, e.to_string()),
    }
}

/// Reads a logo file: a regular file of at most [`MAX_LOGO_BYTES`].
/// Devices and FIFOs are refused before they are opened, the type is
/// checked again on the open file, and the read stops one byte past the
/// limit. (The same rules as the CLI's `--logo`.)
fn read_logo(path: &Path) -> Result<Vec<u8>> {
    let not_a_file = || {
        AppError::new(
            ErrorKind::Invalid,
            format!("logo {}: not a regular file", path.display()),
        )
    };
    let meta = std::fs::metadata(path).map_err(|e| file_error(path, e))?;
    if !meta.is_file() {
        return Err(not_a_file());
    }
    let file = File::open(path).map_err(|e| file_error(path, e))?;
    if !file.metadata().map_err(|e| file_error(path, e))?.is_file() {
        return Err(not_a_file());
    }
    let mut bytes = Vec::new();
    file.take(MAX_LOGO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| file_error(path, e))?;
    if bytes.len() > MAX_LOGO_BYTES {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!(
                "logo {}: the logo is over {MAX_LOGO_BYTES} bytes (2 MB)",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

/// Refuses an output path inside a guide bundle or the library; see
/// [`guiderecorder_core::export::check_out_path`].
pub fn check_out_path(out: &Path, bundle: &Path, library: &Path) -> Result<()> {
    match guiderecorder_core::export::check_out_path(out, bundle, Some(library)) {
        Ok(_) => Ok(()),
        Err(e @ (OutPathError::InsideBundle(_) | OutPathError::InsideLibrary(_))) => {
            Err(AppError::new(
                ErrorKind::Invalid,
                format!("{} is {e}; export somewhere else", out.display()),
            ))
        }
        Err(OutPathError::Unresolvable { path, source }) if path == bundle => {
            Err(file_error(&path, source))
        }
        Err(OutPathError::Unresolvable { path, source }) => Err(AppError::new(
            ErrorKind::Invalid,
            format!("{}: cannot resolve path: {source}", path.display()),
        )),
        Err(e @ OutPathError::DotDotAfterMissing(_)) => {
            Err(AppError::new(ErrorKind::Invalid, e.to_string()))
        }
    }
}

/// Whether a step's stored `cropRect` places its crop image exactly: the
/// exporter's check (size ±1 px, a whole-pixel origin, and the crop equal
/// to the full screenshot there). False when there is no crop, no
/// `cropRect`, or an image can't be read; the editor then shows the full
/// screenshot, as the exporter falls back to it.
///
/// Takes the editor's copy of the step image, not the saved one, so steps
/// that are not saved yet are checked too.
pub fn verify_crop(session: &Session, guide_id: &str, image: &StepImage) -> Result<bool> {
    let id = parse_id(guide_id)?;
    let (Some(crop), Some(rect)) = (&image.crop, image.crop_rect) else {
        return Ok(false);
    };
    check_image_path(&image.full)?;
    check_image_path(crop)?;
    let handle = session.open_handle(id)?;
    let (Ok(full), Ok(crop)) = (handle.read_image(&image.full), handle.read_image(crop)) else {
        return Ok(false);
    };
    Ok(verify_crop_rect(&full, &crop, rect))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fresh folder under `target/`.
    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/guiderecorder-app-tests")
            .join(format!(
                "export-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn destinations_must_come_from_a_dialog() {
        let chosen = ChosenPaths::default();
        let folder = PathBuf::from("/Users/me/Desktop");
        let file = PathBuf::from("/Users/me/Desktop/Guide.pdf");
        let err = chosen
            .check_destination(Format::Md, &folder.join("Guide"))
            .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Invalid);

        chosen.add_folder(folder.clone());
        chosen.add_file(file.clone());
        chosen
            .check_destination(Format::Md, &folder.join("Guide"))
            .unwrap();
        chosen
            .check_destination(Format::Html, &folder.join("Guide export"))
            .unwrap();
        chosen.check_destination(Format::Pdf, &file).unwrap();
        for (format, out) in [
            // Not a plain name inside the chosen folder.
            (Format::Md, folder.join("a/b")),
            (Format::Md, folder.join("..")),
            (Format::Md, folder.clone()),
            (Format::Md, PathBuf::from("/Users/me/Guide")),
            // A file the dialog did not return, or the wrong format.
            (Format::Pdf, folder.join("Other.pdf")),
            (Format::Docx, file.clone()),
        ] {
            assert!(
                chosen.check_destination(format, &out).is_err(),
                "{format:?} {}",
                out.display()
            );
        }
    }

    #[test]
    fn chosen_lists_are_capped() {
        let chosen = ChosenPaths::default();
        for i in 0..MAX_CHOSEN + 5 {
            chosen.add_logo(PathBuf::from(format!("/logo{i}.png")));
        }
        assert!(chosen.check_logo(Path::new("/logo0.png")).is_err());
        assert!(chosen
            .check_logo(Path::new(&format!("/logo{}.png", MAX_CHOSEN + 4)))
            .is_ok());
        assert!(chosen.check_exported(Path::new("/logo1.png")).is_err());
    }

    #[test]
    fn save_dialog_names_get_their_extension() {
        assert_eq!(
            with_extension(PathBuf::from("/a/Guide"), "pdf"),
            Path::new("/a/Guide.pdf")
        );
        assert_eq!(
            with_extension(PathBuf::from("/a/Guide.PDF"), "pdf"),
            Path::new("/a/Guide.PDF")
        );
        assert_eq!(
            with_extension(PathBuf::from("/a/v1.2"), "docx"),
            Path::new("/a/v1.2.docx")
        );
    }

    #[test]
    fn out_paths_inside_a_library_or_bundle_are_refused() {
        let dir = scratch();
        let library = dir.join("Library");
        let bundle = library.join("Guide.guiderecorder");
        fs::create_dir_all(&bundle).unwrap();
        let elsewhere = dir.join("Exports");
        fs::create_dir_all(&elsewhere).unwrap();

        check_out_path(&elsewhere.join("Guide"), &bundle, &library).unwrap();
        check_out_path(&elsewhere.join("new/Guide.pdf"), &bundle, &library).unwrap();
        for out in [
            bundle.join("export"),
            library.join("Guide"),
            dir.join("Other.guiderecorder/out"),
            elsewhere.join("../Library/Guide"),
        ] {
            let err = check_out_path(&out, &bundle, &library).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{}", out.display());
        }
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&library, &link).unwrap();
            assert!(check_out_path(&link.join("Guide"), &bundle, &library).is_err());
        }
    }

    #[test]
    fn logos_are_capped_regular_files() {
        let dir = scratch();
        let logo = dir.join("logo.png");
        fs::write(&logo, b"png").unwrap();
        assert_eq!(read_logo(&logo).unwrap(), b"png");
        assert_eq!(read_logo(&dir).unwrap_err().kind, ErrorKind::Invalid);
        let big = dir.join("big.png");
        fs::write(&big, vec![0; MAX_LOGO_BYTES + 1]).unwrap();
        assert!(read_logo(&big).unwrap_err().message.contains("2 MB"));
        assert_eq!(
            read_logo(&dir.join("missing.png")).unwrap_err().kind,
            ErrorKind::NotFound
        );
    }

    use crate::editor::tests::{fixture_id, library, FIXTURE};
    use guiderecorder_core::library::{write_webp, PixelBuffer};
    use guiderecorder_core::model::{ImageShow, Rect};

    /// A started session over a copy of the fixtures, the fixture guide's
    /// id, and a folder outside the library the user "chose".
    fn session() -> (Session, String, ChosenPaths, PathBuf) {
        let root = library();
        let session = Session::start(&root, None).unwrap();
        let id = fixture_id(&root);
        let chosen = ChosenPaths::default();
        let out = scratch();
        chosen.add_folder(out.clone());
        (session, id, chosen, out)
    }

    #[test]
    fn exports_every_format_from_the_saved_bundle() {
        let (session, id, chosen, dir) = session();
        let none = BrandingRequest::default();
        let md = export_guide(
            &session,
            &chosen,
            &id,
            Format::Md,
            &dir.join("md"),
            false,
            &none,
            docx::Paper::A4,
            false,
        )
        .unwrap();
        assert_eq!(md.files[0], dir.join("md").join(md::GUIDE_FILE));
        assert!(md.files.iter().all(|f| f.is_file()));
        // Only the main document can be revealed.
        chosen.check_exported(&md.files[0]).unwrap();
        assert!(chosen.check_exported(&md.files[1]).is_err());

        let html = export_guide(
            &session,
            &chosen,
            &id,
            Format::Html,
            &dir.join("html"),
            false,
            &none,
            docx::Paper::A4,
            false,
        )
        .unwrap();
        assert!(html.files[0].ends_with(html::GUIDE_FILE));

        for format in [Format::Docx, Format::Pdf] {
            let out = with_extension(dir.join("Guide"), format.extension().unwrap());
            // Not chosen in a save dialog yet.
            assert!(export_guide(
                &session,
                &chosen,
                &id,
                format,
                &out,
                false,
                &none,
                docx::Paper::A4,
                false,
            )
            .is_err());
            chosen.add_file(out.clone());
            let result = export_guide(
                &session,
                &chosen,
                &id,
                format,
                &out,
                false,
                &none,
                docx::Paper::A4,
                false,
            )
            .unwrap();
            assert_eq!(result.files, std::slice::from_ref(&out));
            assert!(fs::metadata(&out).unwrap().len() > 0);
            // A second export needs overwrite.
            let err = export_guide(
                &session,
                &chosen,
                &id,
                format,
                &out,
                false,
                &none,
                docx::Paper::A4,
                false,
            )
            .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Exists, "{}", err.message);
            export_guide(
                &session,
                &chosen,
                &id,
                format,
                &out,
                true,
                &none,
                docx::Paper::A4,
                false,
            )
            .unwrap();
        }

        let err = export_guide(
            &session,
            &chosen,
            &id,
            Format::Md,
            &dir.join("md"),
            false,
            &none,
            docx::Paper::A4,
            false,
        )
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::Exists);
        export_guide(
            &session,
            &chosen,
            &id,
            Format::Md,
            &dir.join("md"),
            true,
            &none,
            docx::Paper::A4,
            false,
        )
        .unwrap();
    }

    #[test]
    fn exports_on_the_chosen_paper() {
        let (session, id, chosen, dir) = session();
        let none = BrandingRequest::default();
        for (paper, media_box) in [
            (docx::Paper::A4, "/MediaBox[0 0 595.2756 841.8898]"),
            (docx::Paper::Letter, "/MediaBox[0 0 612 792]"),
        ] {
            let out = dir.join(format!("{paper:?}.pdf"));
            chosen.add_file(out.clone());
            export_guide(
                &session,
                &chosen,
                &id,
                Format::Pdf,
                &out,
                false,
                &none,
                paper,
                false,
            )
            .unwrap();
            let pdf = fs::read(&out).unwrap();
            assert!(
                String::from_utf8_lossy(&pdf).contains(media_box),
                "{paper:?}"
            );
        }
        let docx = |paper: docx::Paper| {
            let out = dir.join(format!("{paper:?}.docx"));
            chosen.add_file(out.clone());
            export_guide(
                &session,
                &chosen,
                &id,
                Format::Docx,
                &out,
                false,
                &none,
                paper,
                false,
            )
            .unwrap();
            fs::read(&out).unwrap()
        };
        assert_ne!(docx(docx::Paper::A4), docx(docx::Paper::Letter));
    }

    #[test]
    fn export_refuses_the_library_and_unchosen_logos() {
        let (session, id, chosen, dir) = session();
        let none = BrandingRequest::default();
        chosen.add_folder(session.root().to_path_buf());
        let err = export_guide(
            &session,
            &chosen,
            &id,
            Format::Md,
            &session.root().join("x"),
            false,
            &none,
            docx::Paper::A4,
            false,
        )
        .unwrap_err();
        assert!(
            err.message.contains("inside the library"),
            "{}",
            err.message
        );

        let logo = dir.join("logo.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&logo)
            .unwrap();
        let branding = BrandingRequest {
            logo: Some(logo.clone()),
            accent: Some("#336699".into()),
        };
        let err = export_guide(
            &session,
            &chosen,
            &id,
            Format::Md,
            &dir.join("a"),
            false,
            &branding,
            docx::Paper::A4,
            false,
        )
        .unwrap_err();
        assert!(
            err.message.contains("not chosen as the logo"),
            "{}",
            err.message
        );
        chosen.add_logo(logo);
        let result = export_guide(
            &session,
            &chosen,
            &id,
            Format::Md,
            &dir.join("a"),
            false,
            &branding,
            docx::Paper::A4,
            false,
        )
        .unwrap();
        assert!(result.files.iter().any(|f| f.ends_with(md::LOGO_FILE)));

        let bad = BrandingRequest {
            accent: Some("red".into()),
            ..BrandingRequest::default()
        };
        let err = export_guide(
            &session,
            &chosen,
            &id,
            Format::Html,
            &dir.join("b"),
            false,
            &bad,
            docx::Paper::A4,
            false,
        )
        .unwrap_err();
        assert!(err.message.starts_with("branding:"), "{}", err.message);
    }

    #[test]
    fn verify_crop_applies_the_exporters_check() {
        let (session, id, _, _) = session();
        session.open_guide(&id).unwrap();
        let bundle = session.root().join(FIXTURE);
        let full = image::RgbaImage::from_fn(40, 30, |x, y| {
            image::Rgba([x as u8 * 6, y as u8 * 8, 7, 255])
        });
        let crop = image::imageops::crop_imm(&full, 10, 5, 20, 12).to_image();
        let write = |name: &str, img: &image::RgbaImage| {
            let pixels = PixelBuffer::rgba(img.width(), img.height(), img.as_raw());
            write_webp(&bundle.join("steps").join(name), &pixels).unwrap();
        };
        write("v.webp", &full);
        write("v.crop.webp", &crop);
        let image = |rect: Option<Rect>| StepImage {
            full: "steps/v.webp".into(),
            crop: Some("steps/v.crop.webp".into()),
            after: None,
            show: ImageShow::Crop,
            crop_rect: rect,
        };
        let rect = |x, y, width, height| {
            Some(Rect {
                x,
                y,
                width,
                height,
            })
        };
        assert!(verify_crop(&session, &id, &image(rect(10.0, 5.0, 20.0, 12.0))).unwrap());
        // Size within ±1 px still fits.
        assert!(verify_crop(&session, &id, &image(rect(10.0, 5.0, 21.0, 12.0))).unwrap());
        // Right size, wrong place; wrong size; fractional origin; none.
        assert!(!verify_crop(&session, &id, &image(rect(11.0, 5.0, 20.0, 12.0))).unwrap());
        assert!(!verify_crop(&session, &id, &image(rect(10.0, 5.0, 25.0, 12.0))).unwrap());
        assert!(!verify_crop(&session, &id, &image(rect(10.5, 5.0, 20.0, 12.0))).unwrap());
        assert!(!verify_crop(&session, &id, &image(None)).unwrap());
        // Unreadable images: false, not an error.
        let mut missing = image(rect(10.0, 5.0, 20.0, 12.0));
        missing.full = "steps/missing.webp".into();
        assert!(!verify_crop(&session, &id, &missing).unwrap());
        // Paths outside steps/ and assets/ are refused.
        let mut outside = image(rect(10.0, 5.0, 20.0, 12.0));
        outside.full = "guide.json".into();
        assert_eq!(
            verify_crop(&session, &id, &outside).unwrap_err().kind,
            ErrorKind::Invalid
        );
    }

    #[test]
    fn report_becomes_a_result() {
        let step = guiderecorder_core::Ulid::new();
        let mut report = ExportReport {
            files: vec![PathBuf::from("/out/guide.md")],
            ..ExportReport::default()
        };
        report.add_warnings(
            step,
            [guiderecorder_core::export::RenderWarning::CropOffsetUnknown],
        );
        report.notes.push("a note".into());
        let result = ExportResult::from(report);
        assert_eq!(result.files, [PathBuf::from("/out/guide.md")]);
        assert_eq!(result.warnings[0].step_id, step.to_string());
        assert!(result.warnings[0].message.contains("crop position unknown"));
        assert_eq!(result.notes, ["a note"]);
    }
}
