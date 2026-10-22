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

use guiderecorder_core::branch;
use guiderecorder_core::export::md::{Branding, CheckedBranding, MAX_LOGO_BYTES};
use guiderecorder_core::export::{
    docx, html, md, motion, pdf, pptx, scorm, skill, verify_crop_rect, ExportReport, ImageOptions,
    OutPathError, RenderGuide,
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
    /// The interactive click-through HTML (`HtmlMode::Walkthrough`).
    Walkthrough,
    Docx,
    #[default]
    Pdf,
    /// Slides, one per step.
    Pptx,
    /// A video zooming in on each click.
    Mp4,
    /// An animated GIF zooming in on each click.
    Gif,
    /// An agent skill: `SKILL.md`, `steps.json`, images and optionally a
    /// replay script.
    Skill,
    /// A SCORM 1.2 zip: the walkthrough, tracked by an LMS.
    Scorm,
}

impl Format {
    /// Markdown, HTML (either mode) and skills write into a folder; the
    /// other formats write one file.
    pub fn is_folder(self) -> bool {
        matches!(
            self,
            Self::Md | Self::Html | Self::Walkthrough | Self::Skill
        )
    }

    /// The file extension of a one-file export.
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Md | Self::Html | Self::Walkthrough | Self::Skill => None,
            Self::Docx => Some("docx"),
            Self::Pdf => Some("pdf"),
            Self::Pptx => Some("pptx"),
            Self::Mp4 => Some("mp4"),
            Self::Gif => Some("gif"),
            Self::Scorm => Some("zip"),
        }
    }
}

/// MP4 and GIF options as the UI sends them; missing fields take the
/// defaults of [`motion::MotionOptions`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MotionRequest {
    pub seconds_per_step: f64,
    pub aspect: motion::Aspect,
    pub zoom: bool,
    pub captions: bool,
}

impl Default for MotionRequest {
    fn default() -> Self {
        let d = motion::MotionOptions::default();
        Self {
            seconds_per_step: d.seconds_per_step,
            aspect: d.aspect,
            zoom: d.zoom,
            captions: d.captions,
        }
    }
}

/// Skill options as the UI sends them.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SkillRequest {
    /// Also write the replay script (`replay.js`, or `replay.ps1` for a
    /// guide recorded on Windows).
    pub replay: bool,
}

/// SCORM options as the UI sends them; blank title and identifier take
/// the defaults of [`scorm::ScormOptions`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ScormRequest {
    pub title: String,
    pub identifier: String,
    pub completion: scorm::Completion,
    pub mastery_score: u8,
    /// The quiz as typed ([`scorm::parse_quiz`]); blank for none.
    pub quiz: String,
}

impl Default for ScormRequest {
    fn default() -> Self {
        Self {
            title: String::new(),
            identifier: String::new(),
            completion: scorm::Completion::default(),
            mastery_score: scorm::DEFAULT_MASTERY_SCORE,
            quiz: String::new(),
        }
    }
}

impl ScormRequest {
    /// The course options, with the quiz parsed.
    fn options(&self) -> Result<scorm::ScormOptions> {
        let quiz = scorm::parse_quiz(&self.quiz)
            .map_err(|e| AppError::new(ErrorKind::Invalid, e.to_string()))?;
        let given = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
        Ok(scorm::ScormOptions {
            title: given(&self.title),
            identifier: given(&self.identifier),
            completion: self.completion,
            mastery_score: self.mastery_score,
            quiz,
            ..scorm::ScormOptions::default()
        })
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
/// With an `audience`, sections for other audiences are left out (see
/// [`branch::for_audience`]).
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
    motion_request: &MotionRequest,
    skill_request: &SkillRequest,
    scorm_request: &ScormRequest,
    audience: Option<&str>,
) -> Result<ExportResult> {
    let id = parse_id(guide_id)?;
    if !(motion::MIN_SECONDS_PER_STEP..=motion::MAX_SECONDS_PER_STEP)
        .contains(&motion_request.seconds_per_step)
    {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!(
                "seconds per step must be from {} to {}",
                motion::MIN_SECONDS_PER_STEP,
                motion::MAX_SECONDS_PER_STEP
            ),
        ));
    }
    chosen.check_destination(format, out)?;
    // Checked (and the logo decoded) once, before anything is written.
    let branding = check_branding(chosen, branding)?;

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
    let guide = branch::narrow(loaded.guide, audience);
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
        Format::Html | Format::Walkthrough => {
            let mode = if format == Format::Walkthrough {
                html::HtmlMode::Walkthrough
            } else {
                html::HtmlMode::Document
            };
            let opts = html::HtmlOptions {
                mode,
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
        Format::Pptx => {
            let opts = pptx::PptxOptions {
                overwrite,
                timestamp,
                ..pptx::PptxOptions::default()
            };
            pptx::export(&render, out, &opts).map_err(|e| match e {
                pptx::PptxError::Exists(path) => exists(&path),
                e => AppError::new(ErrorKind::Io, e.to_string()),
            })?
        }
        Format::Mp4 | Format::Gif => {
            let opts = motion::MotionOptions {
                overwrite,
                seconds_per_step: motion_request.seconds_per_step,
                aspect: motion_request.aspect,
                zoom: motion_request.zoom,
                captions: motion_request.captions,
                ..motion::MotionOptions::default()
            };
            let result = if format == Format::Mp4 {
                motion::mp4::export(&render, out, &opts)
            } else {
                motion::gif::export(&render, out, &opts)
            };
            result.map_err(|e| match e {
                motion::MotionError::Exists(path) => exists(&path),
                e @ (motion::MotionError::NoSteps | motion::MotionError::Unsupported(_)) => {
                    AppError::new(ErrorKind::Invalid, e.to_string())
                }
                e => AppError::new(ErrorKind::Io, e.to_string()),
            })?
        }
        Format::Skill => {
            let opts = skill::SkillOptions {
                overwrite,
                replay: skill_request.replay,
                ..skill::SkillOptions::default()
            };
            skill::export(&render, &handle, out, &opts).map_err(md_error)?
        }
        Format::Scorm => {
            let opts = scorm::ScormOptions {
                overwrite,
                branding,
                review_details,
                ..scorm_request.options()?
            };
            scorm::export(&render, &handle, out, &opts).map_err(scorm_error)?
        }
    };
    let result = ExportResult::from(report);
    if let Some(main) = result.files.first() {
        chosen.add_exported(main.clone());
    }
    Ok(result)
}

/// The UI's branding, checked: a logo the user chose (read and decoded
/// here) and a valid accent; a blank accent means none.
fn check_branding(chosen: &ChosenPaths, branding: &BrandingRequest) -> Result<CheckedBranding> {
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
    Branding { logo, accent }
        .check()
        .map_err(|e| AppError::new(ErrorKind::Invalid, format!("branding: {e}")))
}

/// Widest step image in a preview, in pixels: the preview pane is about
/// 350 px wide, so this stays sharp on Retina and keeps the IPC small.
const PREVIEW_IMAGE_WIDTH: u32 = 800;

/// Short side of a video or GIF preview frame, in pixels.
const PREVIEW_FRAME_SIZE: u32 = 360;

/// The Export dialog's preview of a guide in one format, made by that
/// format's own exporter (see [`preview_guide`]). Images are PNG data
/// URIs with blur and pixelate burned in, as in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ExportPreview {
    /// The exported page (HTML or interactive walkthrough), self-contained
    /// with its own CSP.
    Html { html: String },
    /// The exported `guide.md` and the images it links, by link.
    Markdown {
        markdown: String,
        images: Vec<PreviewImage>,
    },
    /// The first pages of the PDF, as images. Word is approximated with
    /// the PDF's pages (`approximate`).
    Pages {
        pages: Vec<String>,
        page_count: usize,
        approximate: bool,
    },
    /// Slides: each step's image as the deck shows it, in step order.
    Slides { images: Vec<String> },
    /// Video and GIF: a still of the first scenes (title card, sections,
    /// steps in their close-up).
    Frames {
        frames: Vec<String>,
        scene_count: usize,
    },
}

/// An image a Markdown preview links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewImage {
    /// The link in `guide.md`, e.g. `images/01-open-settings.png`.
    pub link: String,
    pub src: String,
}

fn png_uri(png: &[u8]) -> String {
    html::data_uri("image/png", png)
}

/// Renders a preview of exporting a guide from its saved bundle with these
/// options, writing nothing. The branding and motion options are checked
/// as [`export_guide`] checks them; no destination is needed.
#[allow(clippy::too_many_arguments)]
pub fn preview_guide(
    session: &Session,
    chosen: &ChosenPaths,
    guide_id: &str,
    format: Format,
    branding: &BrandingRequest,
    paper: docx::Paper,
    review_details: bool,
    motion_request: &MotionRequest,
    skill_request: &SkillRequest,
    scorm_request: &ScormRequest,
    audience: Option<&str>,
) -> Result<ExportPreview> {
    let id = parse_id(guide_id)?;
    let branding = check_branding(chosen, branding)?;
    let lock = session.guide_lock(id);
    let _guard = lock.lock().unwrap();
    let handle = session.bundle(id)?;
    let loaded = session.library().read(&handle.path)?;
    if loaded.guide.id != id {
        return Err(AppError::new(
            ErrorKind::Conflict,
            format!("{} no longer holds guide {id}", handle.path.display()),
        ));
    }
    let guide = branch::narrow(loaded.guide, audience);
    let handle = loaded.handle;
    let render = RenderGuide::new(&guide, &handle);
    let image = ImageOptions {
        max_width: Some(PREVIEW_IMAGE_WIDTH),
        ..ImageOptions::default()
    };

    Ok(match format {
        Format::Md => {
            let opts = md::MdOptions {
                image,
                branding,
                review_details,
                ..md::MdOptions::default()
            };
            let preview = md::preview(&render, &handle, &opts);
            ExportPreview::Markdown {
                markdown: preview.markdown,
                images: preview
                    .images
                    .into_iter()
                    .map(|(link, png)| PreviewImage {
                        link,
                        src: png_uri(&png),
                    })
                    .collect(),
            }
        }
        // SKILL.md and its images; steps.json and the script are data.
        Format::Skill => {
            let opts = skill::SkillOptions {
                image,
                replay: skill_request.replay,
                ..skill::SkillOptions::default()
            };
            let preview = skill::preview(&render, &handle, &opts);
            ExportPreview::Markdown {
                markdown: preview.skill_md,
                images: preview
                    .images
                    .into_iter()
                    .map(|(link, png)| PreviewImage {
                        link,
                        src: png_uri(&png),
                    })
                    .collect(),
            }
        }
        Format::Html | Format::Walkthrough => {
            let mode = if format == Format::Walkthrough {
                html::HtmlMode::Walkthrough
            } else {
                html::HtmlMode::Document
            };
            let opts = html::HtmlOptions {
                mode,
                image,
                branding,
                review_details,
                ..html::HtmlOptions::default()
            };
            let (html, _) = html::render_string(&render, &handle, &opts).map_err(md_error)?;
            ExportPreview::Html { html }
        }
        // The SCO page: the walkthrough with the quiz.
        Format::Scorm => {
            let opts = scorm::ScormOptions {
                image,
                branding,
                review_details,
                ..scorm_request.options()?
            };
            let (html, _) = scorm::render_page(&render, &handle, &opts).map_err(scorm_error)?;
            ExportPreview::Html { html }
        }
        Format::Docx | Format::Pdf => {
            let opts = pdf::PdfOptions {
                image,
                timestamp: Some(guide.updated_at),
                paper,
                branding,
                review_details,
                ..pdf::PdfOptions::default()
            };
            let preview = pdf::preview(&render, &handle, &opts)
                .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
            ExportPreview::Pages {
                pages: preview.pages.iter().map(|p| png_uri(p)).collect(),
                page_count: preview.page_count,
                approximate: format == Format::Docx,
            }
        }
        Format::Pptx => {
            let opts = ImageOptions {
                max_width: Some(PREVIEW_IMAGE_WIDTH),
                ..pptx::PptxOptions::default().image
            };
            ExportPreview::Slides {
                images: render
                    .steps()
                    .map(|step| png_uri(&render.step_image(step, &opts).png))
                    .collect(),
            }
        }
        Format::Mp4 | Format::Gif => {
            let opts = motion::MotionOptions {
                seconds_per_step: motion_request
                    .seconds_per_step
                    .clamp(motion::MIN_SECONDS_PER_STEP, motion::MAX_SECONDS_PER_STEP),
                aspect: motion_request.aspect,
                zoom: motion_request.zoom,
                captions: motion_request.captions,
                ..motion::MotionOptions::default()
            };
            let preview =
                motion::preview(&render, &opts, PREVIEW_FRAME_SIZE).map_err(|e| match e {
                    motion::MotionError::NoSteps => {
                        AppError::new(ErrorKind::Invalid, e.to_string())
                    }
                    e => AppError::new(ErrorKind::Io, e.to_string()),
                })?;
            ExportPreview::Frames {
                frames: preview.frames.iter().map(|f| png_uri(f)).collect(),
                scene_count: preview.scene_count,
            }
        }
    })
}

/// The logo the user chose, for the Export dialog to show: a PNG or JPEG
/// of at most [`MAX_LOGO_BYTES`], checked like an export's logo (and
/// turned upright), as a PNG data URI. Only paths from the logo dialog
/// (or the Settings default) are read.
pub fn logo_preview(chosen: &ChosenPaths, path: &Path) -> Result<String> {
    chosen.check_logo(path)?;
    let bytes = read_logo(path)?;
    if !(bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xff, 0xd8, 0xff])) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            format!("logo {}: not a PNG or JPEG image", path.display()),
        ));
    }
    let branding = Branding {
        logo: Some(bytes),
        accent: None,
    }
    .check()
    .map_err(|e| AppError::new(ErrorKind::Invalid, format!("logo {}: {e}", path.display())))?;
    let logo = branding
        .logo()
        .ok_or_else(|| AppError::new(ErrorKind::Invalid, "no logo"))?;
    Ok(png_uri(logo.png()))
}

fn exists(path: &Path) -> AppError {
    AppError::new(
        ErrorKind::Exists,
        format!("{} already exists", path.display()),
    )
}

fn scorm_error(e: scorm::ScormError) -> AppError {
    match e {
        scorm::ScormError::Exists(path) => exists(&path),
        e @ scorm::ScormError::Invalid(_) => AppError::new(ErrorKind::Invalid, e.to_string()),
        scorm::ScormError::Render(e) => md_error(e),
        e => AppError::new(ErrorKind::Io, e.to_string()),
    }
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
        )
        .unwrap();
        assert!(html.files[0].ends_with(html::GUIDE_FILE));
        let walkthrough = export_guide(
            &session,
            &chosen,
            &id,
            Format::Walkthrough,
            &dir.join("walkthrough"),
            false,
            &none,
            docx::Paper::A4,
            false,
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            walkthrough.files,
            [dir.join("walkthrough").join(html::GUIDE_FILE)]
        );
        let skill = export_guide(
            &session,
            &chosen,
            &id,
            Format::Skill,
            &dir.join("skill"),
            false,
            &none,
            docx::Paper::A4,
            false,
            &MotionRequest::default(),
            &SkillRequest { replay: true },
            &ScormRequest::default(),
            None,
        )
        .unwrap();
        assert_eq!(skill.files[0], dir.join("skill").join(skill::SKILL_FILE));
        assert!(skill
            .files
            .contains(&dir.join("skill").join(skill::REPLAY_FILE)));
        assert!(skill.files.iter().all(|f| f.is_file()));
        let page = std::fs::read_to_string(&walkthrough.files[0]).unwrap();
        assert!(page.contains("<script>"));
        assert!(!std::fs::read_to_string(&html.files[0])
            .unwrap()
            .contains("<script>"));

        for format in [Format::Docx, Format::Pdf, Format::Pptx, Format::Scorm] {
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
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
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
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
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
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
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
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
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
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
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
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
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
            &MotionRequest::default(),
            &SkillRequest::default(),
            &ScormRequest::default(),
            None,
        )
        .unwrap_err();
        assert!(err.message.starts_with("branding:"), "{}", err.message);
    }

    #[test]
    fn scorm_options_reach_the_package() {
        let (session, id, chosen, dir) = session();
        let out = dir.join("Course.zip");
        chosen.add_file(out.clone());
        let run = |request: &ScormRequest, preview: bool| {
            let none = BrandingRequest::default();
            let (motion, skill) = (MotionRequest::default(), SkillRequest::default());
            if preview {
                let page = preview_guide(
                    &session,
                    &chosen,
                    &id,
                    Format::Scorm,
                    &none,
                    docx::Paper::A4,
                    false,
                    &motion,
                    &skill,
                    request,
                    None,
                )?;
                let ExportPreview::Html { html } = page else {
                    panic!("{page:?}")
                };
                Ok(html)
            } else {
                export_guide(
                    &session,
                    &chosen,
                    &id,
                    Format::Scorm,
                    &out,
                    true,
                    &none,
                    docx::Paper::A4,
                    false,
                    &motion,
                    &skill,
                    request,
                    None,
                )
                .map(|r| r.files[0].display().to_string())
            }
        };
        let request = ScormRequest {
            title: " Course ".into(),
            identifier: "course-1".into(),
            completion: scorm::Completion::QuizPassed,
            mastery_score: 60,
            quiz: "Ready?\n- [x] Yes\n- [ ] No\n".into(),
        };
        let html = run(&request, true).unwrap();
        assert!(html.contains("data-completion=\"quiz-passed\" data-mastery=\"60\""));
        assert!(html.contains("<legend>1. Ready?</legend>"));
        assert_eq!(run(&request, false).unwrap(), out.display().to_string());
        assert!(fs::read(&out).unwrap().starts_with(b"PK"));

        // A quiz that doesn't parse, or the quiz rule without one, is
        // refused before anything is written.
        fs::remove_file(&out).unwrap();
        for request in [
            ScormRequest {
                quiz: "Ready?\n- [ ] Yes\n".into(),
                ..request.clone()
            },
            ScormRequest {
                quiz: "  ".into(),
                ..request.clone()
            },
        ] {
            for preview in [true, false] {
                let err = run(&request, preview).unwrap_err();
                assert_eq!(err.kind, ErrorKind::Invalid, "{}", err.message);
            }
        }
        assert!(!out.exists());
    }

    #[test]
    fn previews_every_format_without_writing() {
        let (session, id, chosen, dir) = session();
        let none = BrandingRequest::default();
        let steps = {
            let handle = session.bundle(parse_id(&id).unwrap()).unwrap();
            let loaded = session.library().read(&handle.path).unwrap();
            loaded
                .guide
                .items
                .iter()
                .filter(|i| matches!(i, guiderecorder_core::model::Item::Step(_)))
                .count()
        };
        let preview = |format, branding: &BrandingRequest| {
            preview_guide(
                &session,
                &chosen,
                &id,
                format,
                branding,
                docx::Paper::A4,
                true,
                &MotionRequest::default(),
                &SkillRequest::default(),
                &ScormRequest::default(),
                None,
            )
        };
        let is_png = |src: &str| src.starts_with("data:image/png;base64,");

        match preview(Format::Html, &none).unwrap() {
            ExportPreview::Html { html } => {
                assert!(html.contains("<html") && !html.contains("<script>"));
            }
            other => panic!("{other:?}"),
        }
        match preview(Format::Walkthrough, &none).unwrap() {
            ExportPreview::Html { html } => assert!(html.contains("<script>")),
            other => panic!("{other:?}"),
        }
        match preview(Format::Scorm, &none).unwrap() {
            ExportPreview::Html { html } => {
                assert!(html.contains("id=\"scorm\"") && !html.contains("id=\"scorm-quiz\""));
            }
            other => panic!("{other:?}"),
        }
        match preview(Format::Md, &none).unwrap() {
            ExportPreview::Markdown { markdown, images } => {
                assert_eq!(images.len(), steps);
                for image in &images {
                    assert!(markdown.contains(&format!("]({})", image.link)));
                    assert!(is_png(&image.src));
                }
            }
            other => panic!("{other:?}"),
        }
        match preview(Format::Skill, &none).unwrap() {
            ExportPreview::Markdown { markdown, images } => {
                assert!(markdown.starts_with("---\nname: "), "{markdown}");
                assert_eq!(images.len(), steps);
                for image in &images {
                    assert!(markdown.contains(&format!("]({})", image.link)));
                }
            }
            other => panic!("{other:?}"),
        }
        for (format, word) in [(Format::Pdf, false), (Format::Docx, true)] {
            match preview(format, &none).unwrap() {
                ExportPreview::Pages {
                    pages,
                    page_count,
                    approximate,
                } => {
                    assert!(page_count >= 2 && pages.len() == page_count.min(12));
                    assert!(pages.iter().all(|p| is_png(p)));
                    assert_eq!(approximate, word);
                }
                other => panic!("{other:?}"),
            }
        }
        match preview(Format::Pptx, &none).unwrap() {
            ExportPreview::Slides { images } => assert_eq!(images.len(), steps),
            other => panic!("{other:?}"),
        }
        for format in [Format::Mp4, Format::Gif] {
            match preview(format, &none).unwrap() {
                ExportPreview::Frames {
                    frames,
                    scene_count,
                } => {
                    assert!(scene_count >= steps && !frames.is_empty());
                }
                other => panic!("{other:?}"),
            }
        }
        // Nothing was written.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);

        // Branding is checked as for an export.
        let logo = dir.join("logo.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .save(&logo)
            .unwrap();
        let branding = BrandingRequest {
            logo: Some(logo.clone()),
            accent: Some("#336699".into()),
        };
        let err = preview(Format::Html, &branding).unwrap_err();
        assert!(
            err.message.contains("not chosen as the logo"),
            "{}",
            err.message
        );
        chosen.add_logo(logo);
        match preview(Format::Html, &branding).unwrap() {
            ExportPreview::Html { html } => {
                assert!(html.contains("#336699") && html.contains("data:image/png"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn logo_previews_are_chosen_png_or_jpeg_files() {
        let dir = scratch();
        let chosen = ChosenPaths::default();
        let png = dir.join("logo.png");
        image::RgbaImage::from_pixel(3, 2, image::Rgba([9, 8, 7, 255]))
            .save(&png)
            .unwrap();
        let jpeg = dir.join("logo.jpg");
        image::RgbImage::from_pixel(3, 2, image::Rgb([9, 8, 7]))
            .save(&jpeg)
            .unwrap();
        let gif = dir.join("logo.gif");
        fs::write(&gif, b"GIF89a....").unwrap();
        let svg = dir.join("logo.png.svg");
        fs::write(&svg, b"<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        let big = dir.join("big.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.resize(MAX_LOGO_BYTES + 1, 0);
        fs::write(&big, bytes).unwrap();

        // Not chosen: refused before it is read.
        let err = logo_preview(&chosen, &png).unwrap_err();
        assert!(
            err.message.contains("not chosen as the logo"),
            "{}",
            err.message
        );
        for path in [&png, &jpeg, &gif, &svg, &big] {
            chosen.add_logo(path.clone());
        }
        for path in [&png, &jpeg] {
            let uri = logo_preview(&chosen, path).unwrap();
            assert!(
                uri.starts_with("data:image/png;base64,"),
                "{}",
                path.display()
            );
        }
        for path in [&gif, &svg] {
            let err = logo_preview(&chosen, path).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid);
            assert!(err.message.contains("PNG or JPEG"), "{}", err.message);
        }
        assert!(logo_preview(&chosen, &big)
            .unwrap_err()
            .message
            .contains("2 MB"));
    }

    /// The preview shows the walkthrough's own page in a frame, which
    /// keeps the app's CSP, so that CSP allows the page's script by hash.
    #[test]
    fn app_csp_allows_the_walkthrough_script() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = conf["app"]["security"]["csp"].as_str().unwrap();
        let script_src = |csp: &str| {
            csp.split(';')
                .find_map(|d| d.trim().strip_prefix("script-src "))
                .unwrap()
                .split(' ')
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let app = script_src(csp);
        // The walkthrough's script, and the SCORM runtime of its SCORM
        // preview: srcdoc frames run under the app's CSP.
        for page in [guiderecorder_core::export::walkthrough::csp(), scorm::csp()] {
            for hash in script_src(&page) {
                assert!(
                    app.contains(&hash),
                    "tauri.conf.json script-src needs {hash}"
                );
            }
        }
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
