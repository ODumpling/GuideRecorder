//! Read-only stdio MCP server over a GuidesYa library. See design
//! scratchpad #441 and the crate README.
//!
//! ```text
//! guidesya-mcp --library <dir> [--index <path>]
//! ```
//!
//! The library comes from `--library` or `GUIDESYA_LIBRARY`; the index from
//! `--index`, defaulting to the app's ([`Index::default_path`]). A missing
//! or stale index falls back to scanning the library. Nothing is ever
//! written to either. stdout carries the protocol; messages go to stderr.

mod markdown;
mod server;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context as _};
use guidesya_core::index::Index;
use guidesya_core::library::Library;
use rmcp::ServiceExt;

use server::{Config, GuidesYaMcp};

const LIBRARY_ENV: &str = "GUIDESYA_LIBRARY";

const USAGE: &str = "\
Usage: guidesya-mcp --library <dir> [--index <path>]

Read-only MCP server (stdio) over a GuidesYa library.

Options:
  --library <dir>   Library folder (default: $GUIDESYA_LIBRARY)
  --index <path>    Search index (default: the GuidesYa app's index)
  -h, --help        Show this help
  -V, --version     Show the version

Setup:
  claude mcp add guidesya -- /path/to/guidesya-mcp --library <dir>";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve {
        library: PathBuf,
        index: Option<PathBuf>,
    },
    Help,
    Version,
}

fn parse_args(
    args: impl IntoIterator<Item = OsString>,
    env_library: Option<OsString>,
) -> anyhow::Result<Command> {
    let mut library = None;
    let mut index = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();
        let (flag, inline) = match text.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag.to_owned(), Some(value.into())),
            _ => (text.clone(), None),
        };
        let slot = match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--library" => &mut library,
            "--index" => &mut index,
            _ => bail!("unexpected argument {text:?}\n\n{USAGE}"),
        };
        let value = match inline {
            Some(value) => value,
            None => args
                .next()
                .with_context(|| format!("{flag} needs a value\n\n{USAGE}"))?,
        };
        if value.is_empty() {
            bail!("{flag} needs a value\n\n{USAGE}");
        }
        *slot = Some(PathBuf::from(value));
    }
    let library = library
        .or_else(|| env_library.filter(|v| !v.is_empty()).map(PathBuf::from))
        .with_context(|| {
            format!("no library given: pass --library <dir> or set {LIBRARY_ENV}\n\n{USAGE}")
        })?;
    Ok(Command::Serve {
        library,
        index: index.or_else(Index::default_path),
    })
}

/// Opens the library without creating it: a reader must not write, and a
/// typo should fail loudly rather than serve an empty library.
fn open_library(root: &std::path::Path) -> anyhow::Result<Library> {
    if !root.is_dir() {
        bail!("library {} is not a folder", root.display());
    }
    Library::open(root).with_context(|| format!("cannot open library {}", root.display()))
}

async fn serve(config: Config) -> anyhow::Result<()> {
    let service = GuidesYaMcp::new(config)
        .serve(rmcp::transport::stdio())
        .await
        .context("MCP initialization failed")?;
    service.waiting().await.context("MCP server failed")?;
    Ok(())
}

fn main() -> ExitCode {
    let command = match parse_args(std::env::args_os().skip(1), std::env::var_os(LIBRARY_ENV)) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("guidesya-mcp: {e:#}");
            return ExitCode::from(2);
        }
    };
    let (library, index) = match command {
        Command::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Command::Version => {
            println!(
                "guidesya-mcp {} (guide schema v{})",
                env!("CARGO_PKG_VERSION"),
                guidesya_core::SCHEMA_VERSION
            );
            return ExitCode::SUCCESS;
        }
        Command::Serve { library, index } => (library, index),
    };
    let result = open_library(&library).and_then(|library| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("cannot start the async runtime")?
            .block_on(serve(Config { library, index }))
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("guidesya-mcp: {e:#}");
            ExitCode::FAILURE
        }
    }
}
