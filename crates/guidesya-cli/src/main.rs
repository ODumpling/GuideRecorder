//! `guidesya` CLI. See design scratchpad #441 ("CLI").
//!
//! `list`, `search` and `validate` are readers: they go through
//! [`Library::read`] and read-only index opens, and never write to the
//! library or the index. `reindex` is the only writer. `export` lands in
//! #353.
//!
//! Exit codes: 0 on success, 1 on an error (or a `validate` that found
//! errors), 2 on a usage error. A closed stdout (e.g. `guidesya list | head`)
//! is not an error: output stops and the exit code is 0.
//!
//! `--json` output is one pretty-printed object per run. Its keys are
//! camelCase (`stepCount`, `guideId`, ...), matching `guide.json`; new keys
//! may be added, existing ones keep their name and type.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::builder::RangedU64ValueParser;
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use guidesya_core::export::{render_step_image, ImageOptions, RenderWarning};
use guidesya_core::index::{
    list_library, list_with_fallback, search_library, search_with_fallback, Index, IndexError,
    IndexedGuide, ListFilter, Source, StepHit, SyncReport,
};
use guidesya_core::library::{Library, LoadedGuide};
use guidesya_core::SCHEMA_VERSION;
use serde_json::{json, Value};

/// Environment variable naming the library folder when `--library` is not
/// given.
const LIBRARY_ENV: &str = "GUIDESYA_LIBRARY";

#[derive(Debug, Parser)]
#[command(name = "guidesya", version, about = "GuidesYa command-line tool")]
struct Cli {
    /// Library folder holding the `.guidesya` bundles [env: GUIDESYA_LIBRARY]
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
        /// `.guidesya` bundle folders.
        #[arg(required = true, value_name = "BUNDLE")]
        bundles: Vec<PathBuf>,
        #[command(flatten)]
        output: Output,
    },
    /// Bring a search index up to date with the library.
    ///
    /// The only command that writes, and it writes only the index file
    /// given with `--index PATH`. The GuidesYa app owns its own index and
    /// keeps it current; writing that one needs `--app-index`, and the app
    /// must not be running. An index of another format is refused unless
    /// `--full` rebuilds it.
    Reindex {
        /// Rebuild the whole index instead of syncing changes. Required to
        /// replace an index of another format.
        #[arg(long)]
        full: bool,
        /// Write the app's own index instead of `--index PATH`. Quit
        /// GuidesYa first: the app owns that index.
        #[arg(long)]
        app_index: bool,
        #[command(flatten)]
        output: Output,
    },
    /// Export a guide (not yet implemented).
    Export {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        args: Vec<String>,
    },
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
        Command::List { tag, query, output } => {
            let library = open_library(&cli)?;
            let filter = ListFilter {
                tag: tag.clone(),
                query: query.clone(),
                limit: None,
            };
            let (guides, source) = match index_path(&cli) {
                Some(path) => list_with_fallback(path, &library, &filter)?,
                None => (list_library(&library, &filter)?, Source::Scan),
            };
            print_list(out, &guides, source, output.json)?;
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
        Command::Export { .. } => {
            eprintln!("guidesya export: not yet implemented");
            return Ok(ExitCode::FAILURE);
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

fn guide_json(guide: &IndexedGuide) -> Value {
    json!({
        "id": guide.id.to_string(),
        "path": path_str(&guide.path),
        "title": guide.title,
        "summary": guide.summary,
        "tags": guide.tags,
        "stepCount": guide.step_count,
        "updatedAt": guide.updated_at.to_rfc3339(),
        "readOnly": guide.read_only,
        "duplicateOf": guide.duplicate_of.as_deref().map(path_str),
    })
}

fn print_list(
    out: &mut impl Write,
    guides: &[IndexedGuide],
    source: Source,
    as_json: bool,
) -> io::Result<()> {
    if as_json {
        let value = json!({
            "source": source_str(source),
            "guides": guides.iter().map(guide_json).collect::<Vec<_>>(),
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
