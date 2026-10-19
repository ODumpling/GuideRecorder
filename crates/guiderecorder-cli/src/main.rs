//! `guiderecorder` CLI. See design scratchpad #441 ("CLI").
//!
//! `list`, `search`, `validate` and `export` are readers: they go through
//! [`Library::read`] and read-only index opens, and never write to the
//! library or the index (`export` writes only its `--out` target, which may
//! not be inside the bundle). `reindex` is the only command that writes to
//! an index.
//!
//! Exit codes: 0 on success, 1 on an error (or a `validate` that found
//! errors, or an `export --strict` with warnings), 2 on a usage error. A closed stdout (e.g. `guiderecorder list | head`)
//! is not an error: output stops and the exit code is 0.
//!
//! `--json` output is one pretty-printed object per run. Its keys are
//! camelCase (`stepCount`, `guideId`, ...), matching `guide.json`; new keys
//! may be added, existing ones keep their name and type.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Result};
use clap::builder::RangedU64ValueParser;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use guiderecorder_core::export::md::{Branding, MAX_LOGO_BYTES};
use guiderecorder_core::export::{
    docx, html, md, pdf, render_step_image, ExportReport, ImageOptions, OutPathError, RenderGuide,
    RenderWarning,
};
use guiderecorder_core::index::{
    list_library, list_with_fallback, search_library, search_with_fallback, Index, IndexError,
    IndexedGuide, ListFilter, Source, StepHit, SyncReport,
};
use guiderecorder_core::library::{Library, LoadedGuide};
use guiderecorder_core::model::{local_today, GuideStatus, NaiveDate};
use guiderecorder_core::{Ulid, SCHEMA_VERSION};
use serde_json::{json, Value};

/// Environment variable naming the library folder when `--library` is not
/// given.
const LIBRARY_ENV: &str = "GUIDERECORDER_LIBRARY";

#[derive(Debug, Parser)]
#[command(
    name = "guiderecorder",
    version,
    about = "GuideRecorder command-line tool"
)]
struct Cli {
    /// Library folder holding the `.guiderecorder` bundles [env: GUIDERECORDER_LIBRARY]
    #[arg(long, global = true, value_name = "DIR")]
    library: Option<PathBuf>,
    /// Search index file. Readers default to the app's index; `reindex`
    /// needs this or `--app-index`
    #[arg(long, global = true, value_name = "PATH")]
    index: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List the guides in the library.
    List {
        /// Only guides with this tag (case-insensitive).
        #[arg(long, value_name = "T")]
        tag: Option<String>,
        /// Only guides matching these words, best match first.
        #[arg(long, value_name = "Q")]
        query: Option<String>,
        /// Only guides whose review is overdue: past `lastReviewed` (or
        /// creation) plus `reviewIntervalMonths`, in local time.
        #[arg(long)]
        overdue: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Search guides, steps, sections and notes.
    Search {
        query: String,
        /// Maximum number of hits (at least 1).
        #[arg(
            long,
            default_value_t = 20,
            value_name = "N",
            value_parser = RangedU64ValueParser::<usize>::new().range(1..)
        )]
        limit: usize,
        #[command(flatten)]
        output: Output,
    },
    /// Check bundles: schema, images and rendering. Writes nothing.
    Validate {
        /// `.guiderecorder` bundle folders.
        #[arg(required = true, value_name = "BUNDLE")]
        bundles: Vec<PathBuf>,
        #[command(flatten)]
        output: Output,
    },
    /// Bring a search index up to date with the library.
    ///
    /// The only command that writes, and it writes only the index file
    /// given with `--index PATH`. The GuideRecorder app owns its own index and
    /// keeps it current; writing that one needs `--app-index`, and the app
    /// must not be running. An index of another format is refused unless
    /// `--full` rebuilds it.
    Reindex {
        /// Rebuild the whole index instead of syncing changes. Required to
        /// replace an index of another format.
        #[arg(long)]
        full: bool,
        /// Write the app's own index instead of `--index PATH`. Quit
        /// GuideRecorder first: the app owns that index.
        #[arg(long)]
        app_index: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Export a guide as Markdown, HTML, an interactive HTML walkthrough,
    /// DOCX or PDF.
    ///
    /// Reads the guide without changing the library. Markdown and HTML
    /// write into the `--out` folder (`guide.md` plus `images/`, or
    /// `guide.html`; a walkthrough is also one `guide.html`); DOCX and PDF
    /// write the `--out` file. Image problems
    /// become placeholders and warnings, not errors.
    Export(ExportArgs),
}

#[derive(Debug, Args)]
struct ExportArgs {
    /// A `.guiderecorder` bundle folder, or a guide id in the library.
    #[arg(value_name = "BUNDLE|ID")]
    guide: String,
    #[arg(long, value_enum)]
    format: Format,
    /// Output folder (md, html, walkthrough) or file (docx, pdf).
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
    /// Replace an earlier export at `--out`.
    #[arg(long)]
    overwrite: bool,
    /// Logo image (PNG or JPEG, at most 2 MB and 2000 px a side).
    #[arg(long, value_name = "FILE")]
    logo: Option<PathBuf>,
    /// Accent colour, `#rgb` or `#rrggbb` (not used by Markdown).
    #[arg(long, value_name = "#HEX")]
    accent: Option<String>,
    /// Paper size of a DOCX or PDF export.
    #[arg(long, value_enum, default_value_t = Paper::A4)]
    paper: Paper,
    /// Show the guide's version, owner and last review day (those set)
    /// under the summary, or on the PDF cover.
    #[arg(long)]
    review_details: bool,
    /// Exit with 1 when the export has warnings or notes (the export is
    /// still written).
    #[arg(long)]
    strict: bool,
    #[command(flatten)]
    output: Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Md,
    Html,
    /// Interactive click-through HTML, one step at a time.
    Walkthrough,
    Docx,
    Pdf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Paper {
    A4,
    Letter,
}

impl From<Paper> for docx::Paper {
    fn from(paper: Paper) -> Self {
        match paper {
            Paper::A4 => Self::A4,
            Paper::Letter => Self::Letter,
        }
    }
}

impl Format {
    fn as_str(self) -> &'static str {
        match self {
            Self::Md => "md",
            Self::Html => "html",
            Self::Walkthrough => "walkthrough",
            Self::Docx => "docx",
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Debug, Args)]
struct Output {
    /// Print JSON instead of text (camelCase keys, matching guide.json).
    #[arg(long)]
    json: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut out = io::stdout().lock();
    let result = run(cli, &mut out).and_then(|code| {
        out.flush()?;
        Ok(code)
    });
    match result {
        Ok(code) => code,
        // The reader went away (e.g. `| head`): stop quietly.
        Err(e) if is_broken_pipe(&e) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn is_broken_pipe(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|cause| cause.downcast_ref::<io::Error>())
        .any(|e| e.kind() == io::ErrorKind::BrokenPipe)
}

fn run(cli: Cli, out: &mut impl Write) -> Result<ExitCode> {
    match &cli.command {
        Command::List {
            tag,
            query,
            overdue,
            output,
        } => {
            let library = open_library(&cli)?;
            let filter = ListFilter {
                tag: tag.clone(),
                query: query.clone(),
                limit: None,
            };
            let (mut guides, source) = match index_path(&cli) {
                Some(path) => list_with_fallback(path, &library, &filter)?,
                None => (list_library(&library, &filter)?, Source::Scan),
            };
            let today = local_today();
            if *overdue {
                guides.retain(|g| g.is_review_overdue(today));
            }
            print_list(out, &guides, source, today, output.json)?;
        }
        Command::Search {
            query,
            limit,
            output,
        } => {
            let library = open_library(&cli)?;
            let (hits, source) = match index_path(&cli) {
                Some(path) => search_with_fallback(path, &library, query, *limit)?,
                None => (search_library(&library, query, *limit)?, Source::Scan),
            };
            print_search(out, &hits, source, output.json)?;
        }
        Command::Validate { bundles, output } => {
            let reports: Vec<Validation> = bundles.iter().map(|b| validate(b)).collect();
            let ok = reports.iter().all(|r| r.errors.is_empty());
            print_validate(out, &reports, ok, output.json)?;
            if !ok {
                return Ok(ExitCode::FAILURE);
            }
        }
        Command::Reindex {
            full,
            app_index,
            output,
        } => {
            let path = reindex_path(&cli, *app_index);
            let library = open_library(&cli)?;
            if !*full {
                check_format(&path)?;
            }
            let mut index =
                Index::open(&path).with_context(|| format!("opening {}", path.display()))?;
            let report = if *full {
                index.rebuild(&library)?
            } else {
                index.sync(&library)?
            };
            print_reindex(out, &path, &report, output.json)?;
        }
        Command::Export(args) => {
            let export = export(&cli, args)?;
            let failed = args.strict && export.has_warnings();
            print_export(out, &export, args.output.json)?;
            if failed {
                return Ok(ExitCode::FAILURE);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Opens the library from `--library` or [`LIBRARY_ENV`]. Refuses a
/// missing folder rather than let [`Library::open`] create it.
fn open_library(cli: &Cli) -> Result<Library> {
    let root = match &cli.library {
        Some(root) => root.clone(),
        None => match std::env::var_os(LIBRARY_ENV).filter(|v| !v.is_empty()) {
            Some(root) => PathBuf::from(root),
            None => bail!("no library folder; pass --library DIR or set {LIBRARY_ENV}"),
        },
    };
    if !root.is_dir() {
        bail!("{}: library folder not found", root.display());
    }
    Library::open(&root).with_context(|| format!("opening library {}", root.display()))
}

fn index_path(cli: &Cli) -> Option<PathBuf> {
    cli.index.clone().or_else(Index::default_path)
}

/// The index `reindex` writes: `--index PATH`, or the app's own with
/// `--app-index`. Never a silent default. Exits with a usage error (2)
/// otherwise.
fn reindex_path(cli: &Cli, app_index: bool) -> PathBuf {
    if app_index && cli.index.is_some() {
        Cli::command()
            .error(
                ErrorKind::ArgumentConflict,
                "--app-index cannot be used with --index",
            )
            .exit()
    }
    let path = if app_index {
        Index::default_path().ok_or("no app index on this platform; pass --index PATH")
    } else {
        cli.index
            .clone()
            .ok_or("reindex needs --index PATH (or --app-index for the app's own index)")
    };
    path.unwrap_or_else(|msg| {
        Cli::command()
            .error(ErrorKind::MissingRequiredArgument, msg)
            .exit()
    })
}

/// Refuses an existing index of another format (or a damaged one):
/// [`Index::open`] would empty it, so that takes `--full`. A missing or
/// empty file, or a file that is not an index (which `Index::open` refuses
/// itself), passes.
fn check_format(path: &Path) -> Result<()> {
    if !path.metadata().is_ok_and(|m| m.len() > 0) {
        return Ok(());
    }
    let Ok(index) = Index::open_read_only(path) else {
        return Ok(());
    };
    let probe = ListFilter {
        limit: Some(1),
        ..ListFilter::default()
    };
    match index.list(&probe) {
        Err(IndexError::Incompatible { found, expected }) => bail!(
            "{}: index format {} does not match {expected}; pass --full to rebuild it",
            path.display(),
            found.as_deref().unwrap_or("unknown"),
        ),
        _ => Ok(()),
    }
}

fn source_str(source: Source) -> &'static str {
    match source {
        Source::Index => "index",
        Source::Scan => "scan",
    }
}

fn path_str(path: &Path) -> String {
    path.display().to_string()
}

// ---------------------------------------------------------------------------
// list / search

fn guide_json(guide: &IndexedGuide, today: NaiveDate) -> Value {
    let review = &guide.review;
    json!({
        "id": guide.id.to_string(),
        "path": path_str(&guide.path),
        "title": guide.title,
        "summary": guide.summary,
        "tags": guide.tags,
        "stepCount": guide.step_count,
        "createdAt": guide.created_at.to_rfc3339(),
        "updatedAt": guide.updated_at.to_rfc3339(),
        "folder": guide.folder,
        "owner": review.owner,
        "status": review.status.map(GuideStatus::as_str),
        "version": review.version,
        "lastReviewed": review.last_reviewed.map(|d| d.to_string()),
        "reviewIntervalMonths": review.review_interval_months,
        "reviewDue": guide.review_due().map(|d| d.to_string()),
        "reviewOverdue": guide.is_review_overdue(today),
        "readOnly": guide.read_only,
        "duplicateOf": guide.duplicate_of.as_deref().map(path_str),
    })
}

fn print_list(
    out: &mut impl Write,
    guides: &[IndexedGuide],
    source: Source,
    today: NaiveDate,
    as_json: bool,
) -> io::Result<()> {
    if as_json {
        let value = json!({
            "source": source_str(source),
            "guides": guides.iter().map(|g| guide_json(g, today)).collect::<Vec<_>>(),
        });
        writeln!(out, "{value:#}")?;
        return Ok(());
    }
    if guides.is_empty() {
        writeln!(out, "No guides.")?;
        return Ok(());
    }
    for guide in guides {
        let mut line = format!("{}  ({} steps", guide.title, guide.step_count);
        if !guide.tags.is_empty() {
            line.push_str(&format!("; {}", guide.tags.join(", ")));
        }
        line.push(')');
        if guide.read_only {
            line.push_str("  [read-only]");
        }
        if guide.duplicate_of.is_some() {
            line.push_str("  [duplicate]");
        }
        if guide.review.status == Some(GuideStatus::NeedsUpdate) {
            line.push_str("  [needs update]");
        }
        if let Some(due) = guide
            .review_due()
            .filter(|_| guide.is_review_overdue(today))
        {
            line.push_str(&format!("  [review overdue since {due}]"));
        }
        writeln!(out, "{line}")?;
        writeln!(out, "    {}  {}", guide.id, guide.path.display())?;
    }
    Ok(())
}

fn hit_json(hit: &StepHit) -> Value {
    json!({
        "guideId": hit.guide_id.to_string(),
        "guidePath": path_str(&hit.guide_path),
        "guideTitle": hit.guide_title,
        "kind": hit.kind.as_str(),
        "itemId": hit.item_id.map(|id| id.to_string()),
        "stepIndex": hit.step_index,
        "title": hit.title,
        "snippet": hit.snippet,
    })
}

fn print_search(
    out: &mut impl Write,
    hits: &[StepHit],
    source: Source,
    as_json: bool,
) -> io::Result<()> {
    if as_json {
        let value = json!({
            "source": source_str(source),
            "hits": hits.iter().map(hit_json).collect::<Vec<_>>(),
        });
        writeln!(out, "{value:#}")?;
        return Ok(());
    }
    if hits.is_empty() {
        writeln!(out, "No matches.")?;
        return Ok(());
    }
    for hit in hits {
        let place = match hit.step_index {
            Some(i) => format!("step {}", i + 1),
            None => hit.kind.as_str().to_owned(),
        };
        writeln!(out, "{} › {}  ({place})", hit.guide_title, hit.title)?;
        if !hit.snippet.is_empty() {
            writeln!(out, "    {}", hit.snippet)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// validate

/// The result of validating one bundle.
#[derive(Debug, Default)]
struct Validation {
    path: PathBuf,
    id: Option<String>,
    title: Option<String>,
    /// `schemaVersion` as stored on disk.
    schema_version: Option<u32>,
    needs_migration: bool,
    read_only: bool,
    step_count: usize,
    errors: Vec<Problem>,
    warnings: Vec<Problem>,
}

#[derive(Debug)]
struct Problem {
    step: Option<String>,
    message: String,
}

impl Problem {
    fn json(&self) -> Value {
        json!({ "step": self.step, "message": self.message })
    }
}

/// Reads `bundle` through [`Library::read`] (no migration is written),
/// decodes every step image and renders each step, discarding the output.
fn validate(bundle: &Path) -> Validation {
    let mut out = Validation {
        path: bundle.to_path_buf(),
        ..Validation::default()
    };
    let loaded = match read_bundle(bundle) {
        Ok(loaded) => loaded,
        Err(e) => {
            out.errors.push(Problem {
                step: None,
                message: format!("{e:#}"),
            });
            return out;
        }
    };
    let LoadedGuide {
        handle,
        guide,
        read_only,
        migrated_from,
        ..
    } = loaded;
    out.path = handle.path.clone();
    out.id = Some(guide.id.to_string());
    out.title = Some(guide.title.clone());
    out.schema_version = Some(migrated_from.unwrap_or(guide.schema_version));
    out.needs_migration = migrated_from.is_some();
    out.read_only = read_only;
    if guide.schema_version > SCHEMA_VERSION {
        out.warnings.push(Problem {
            step: None,
            message: format!(
                "schema version {} is newer than supported {SCHEMA_VERSION}; read-only",
                guide.schema_version
            ),
        });
    }

    for step in guide.steps() {
        out.step_count += 1;
        let step_id = step.id.to_string();
        let images = std::iter::once(Some(&step.image.full))
            .chain([step.image.crop.as_ref(), step.image.after.as_ref()])
            .flatten();
        let mut broken = BTreeSet::new();
        for relative in images {
            if let Err(e) = handle.read_image(relative) {
                out.errors.push(Problem {
                    step: Some(step_id.clone()),
                    message: format!("broken image {relative}: {e}"),
                });
                broken.insert(relative.as_str());
            }
        }
        let rendered = render_step_image(&handle, step, &ImageOptions::default());
        for warning in rendered.warnings {
            // Already reported as a broken image.
            if matches!(&warning, RenderWarning::ImageUnavailable { path, .. }
                if broken.contains(path.as_str()))
            {
                continue;
            }
            out.warnings.push(Problem {
                step: Some(step_id.clone()),
                message: warning.to_string(),
            });
        }
    }
    out
}

fn read_bundle(bundle: &Path) -> Result<LoadedGuide> {
    if !bundle.is_dir() {
        bail!("{}: no such bundle folder", bundle.display());
    }
    // Any library works for `read`; the bundle's parent exists, so opening
    // it creates nothing.
    let bundle = std::fs::canonicalize(bundle)
        .with_context(|| format!("{}: cannot resolve path", bundle.display()))?;
    let parent = bundle.parent().unwrap_or(Path::new("/"));
    let library = Library::open(parent)?;
    Ok(library.read(&bundle)?)
}

fn print_validate(
    out: &mut impl Write,
    reports: &[Validation],
    ok: bool,
    as_json: bool,
) -> io::Result<()> {
    if as_json {
        let bundles: Vec<Value> = reports
            .iter()
            .map(|r| {
                json!({
                    "path": path_str(&r.path),
                    "ok": r.errors.is_empty(),
                    "id": r.id,
                    "title": r.title,
                    "schemaVersion": r.schema_version,
                    "currentSchemaVersion": SCHEMA_VERSION,
                    "needsMigration": r.needs_migration,
                    "readOnly": r.read_only,
                    "stepCount": r.step_count,
                    "errors": r.errors.iter().map(Problem::json).collect::<Vec<_>>(),
                    "warnings": r.warnings.iter().map(Problem::json).collect::<Vec<_>>(),
                })
            })
            .collect();
        writeln!(out, "{:#}", json!({ "ok": ok, "bundles": bundles }))?;
        return Ok(());
    }
    for r in reports {
        let status = if r.errors.is_empty() { "ok" } else { "FAILED" };
        writeln!(out, "{}: {status}", r.path.display())?;
        if let (Some(title), Some(version)) = (&r.title, r.schema_version) {
            let migration = if r.needs_migration {
                format!(", needs migration to {SCHEMA_VERSION}")
            } else {
                String::new()
            };
            let access = if r.read_only { "read-only" } else { "writable" };
            writeln!(
                out,
                "    {title}: schema {version}{migration}, {access}, {} steps",
                r.step_count
            )?;
        }
        for (label, problems) in [("error", &r.errors), ("warning", &r.warnings)] {
            for p in problems {
                match &p.step {
                    Some(step) => writeln!(out, "    {label}: step {step}: {}", p.message),
                    None => writeln!(out, "    {label}: {}", p.message),
                }?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// export

/// A finished export, for printing.
struct Export {
    id: String,
    title: String,
    format: Format,
    report: ExportReport,
}

impl Export {
    fn has_warnings(&self) -> bool {
        !self.report.warnings.is_empty() || !self.report.notes.is_empty()
    }
}

fn export(cli: &Cli, args: &ExportArgs) -> Result<Export> {
    let (loaded, library_root) = resolve_guide(cli, &args.guide)?;
    // Checked (and the logo decoded) once, before anything is written.
    let branding = Branding {
        logo: args.logo.as_deref().map(read_logo).transpose()?,
        accent: args.accent.clone(),
    }
    .check()
    .context("invalid branding")?;
    check_out_path(&args.out, &loaded.handle.path, library_root.as_deref())?;

    let LoadedGuide { handle, guide, .. } = loaded;
    let render = RenderGuide::new(&guide, &handle);
    let out = &args.out;
    let overwrite = args.overwrite;
    let timestamp = Some(guide.updated_at);
    let paper = args.paper.into();
    let review_details = args.review_details;
    // The md and html writers create their folder; match that for files.
    if matches!(args.format, Format::Docx | Format::Pdf) {
        if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
    }
    let report = match args.format {
        Format::Md => {
            let opts = md::MdOptions {
                overwrite,
                branding,
                review_details,
                ..md::MdOptions::default()
            };
            md::export(&render, &handle, out, &opts)?
        }
        Format::Html | Format::Walkthrough => {
            let mode = if args.format == Format::Walkthrough {
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
            html::export(&render, &handle, out, &opts)?
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
            docx::export(&render, &handle, out, &opts)?
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
            pdf::export(&render, &handle, out, &opts)?
        }
    };
    Ok(Export {
        id: guide.id.to_string(),
        title: guide.title,
        format: args.format,
        report,
    })
}

/// A bundle folder, or else a guide id looked up in the library. Also
/// returns the library folder when it is known: the one the id was found
/// in, or the one from `--library` / [`LIBRARY_ENV`].
fn resolve_guide(cli: &Cli, guide: &str) -> Result<(LoadedGuide, Option<PathBuf>)> {
    let path = Path::new(guide);
    if path.is_dir() {
        return Ok((read_bundle(path)?, library_root(cli)));
    }
    let Ok(id) = guide.parse::<Ulid>() else {
        bail!("{guide}: no such bundle folder or guide id");
    };
    let library = open_library(cli)?;
    match library.find(id)? {
        Some(handle) => Ok((
            library.read(&handle.path)?,
            Some(library.root().to_path_buf()),
        )),
        None => bail!(
            "{guide}: no guide with this id in {}",
            library.root().display()
        ),
    }
}

/// The `--library` / [`LIBRARY_ENV`] folder, if given.
fn library_root(cli: &Cli) -> Option<PathBuf> {
    cli.library.clone().or_else(|| {
        std::env::var_os(LIBRARY_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
}

/// Reads a regular file of at most [`MAX_LOGO_BYTES`]. Devices and FIFOs
/// are refused before they are opened (a FIFO would block, `/dev/zero`
/// never ends), and the read stops one byte past the limit.
fn read_logo(path: &Path) -> Result<Vec<u8>> {
    let context = || format!("--logo {}", path.display());
    let not_a_file = || anyhow!("--logo {}: not a regular file", path.display());
    if !std::fs::metadata(path).with_context(context)?.is_file() {
        return Err(not_a_file());
    }
    let file = File::open(path).with_context(context)?;
    // The path may have been swapped since the check above.
    if !file.metadata().with_context(context)?.is_file() {
        return Err(not_a_file());
    }
    let mut bytes = Vec::new();
    file.take(MAX_LOGO_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(context)?;
    if bytes.len() > MAX_LOGO_BYTES {
        bail!(
            "--logo {}: the logo is over {MAX_LOGO_BYTES} bytes (2 MB)",
            path.display()
        );
    }
    Ok(bytes)
}

/// Refuses an output path inside a guide bundle or the library; see
/// [`guiderecorder_core::export::check_out_path`]. The messages name `--out`.
fn check_out_path(out: &Path, bundle: &Path, library: Option<&Path>) -> Result<()> {
    match guiderecorder_core::export::check_out_path(out, bundle, library) {
        Ok(_) => Ok(()),
        Err(e @ (OutPathError::InsideBundle(_) | OutPathError::InsideLibrary(_))) => {
            bail!("--out {}: {e}; export somewhere else", out.display())
        }
        Err(OutPathError::Unresolvable { path, source }) => {
            let flag = if path == out { "--out " } else { "" };
            Err(anyhow::Error::new(source)
                .context(format!("{flag}{}: cannot resolve path", path.display())))
        }
        Err(e @ OutPathError::DotDotAfterMissing(_)) => bail!("--out {e}"),
    }
}

fn print_export(out: &mut impl Write, export: &Export, as_json: bool) -> io::Result<()> {
    let warnings: Vec<Problem> = export
        .report
        .warnings
        .iter()
        .map(|(step, warning)| Problem {
            step: Some(step.to_string()),
            message: warning.to_string(),
        })
        .collect();
    if as_json {
        let value = json!({
            "id": export.id,
            "title": export.title,
            "format": export.format.as_str(),
            "files": export.report.files.iter().map(|f| path_str(f)).collect::<Vec<_>>(),
            "warnings": warnings.iter().map(Problem::json).collect::<Vec<_>>(),
            "notes": export.report.notes,
        });
        writeln!(out, "{value:#}")?;
        return Ok(());
    }
    writeln!(
        out,
        "Exported {} ({}) as {}:",
        export.title,
        export.id,
        export.format.as_str()
    )?;
    for file in &export.report.files {
        writeln!(out, "    {}", file.display())?;
    }
    for warning in &warnings {
        let step = warning.step.as_deref().unwrap_or_default();
        writeln!(out, "    warning: step {step}: {}", warning.message)?;
    }
    for note in &export.report.notes {
        writeln!(out, "    note: {note}")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// reindex

fn print_reindex(
    out: &mut impl Write,
    path: &Path,
    report: &SyncReport,
    as_json: bool,
) -> io::Result<()> {
    if as_json {
        let value = json!({
            "index": path_str(path),
            "rebuilt": report.rebuilt,
            "added": report.added,
            "updated": report.updated,
            "removed": report.removed,
        });
        writeln!(out, "{value:#}")?;
        return Ok(());
    }
    let how = if report.rebuilt { "rebuilt" } else { "synced" };
    writeln!(out, "Index {how}: {}", path.display())?;
    for (label, bundles) in [
        ("added", &report.added),
        ("updated", &report.updated),
        ("removed", &report.removed),
    ] {
        writeln!(out, "  {label}: {}", bundles.len())?;
        for bundle in bundles {
            writeln!(out, "    {bundle}")?;
        }
    }
    Ok(())
}
