//! Writing an export file atomically: into a temporary file next to it,
//! synced, then renamed into place.

use std::fs::{self, File};
use std::io;
use std::path::Path;

use tempfile::TempPath;

/// The folder `out` is written in.
pub(super) fn parent_dir(out: &Path) -> &Path {
    match out.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    }
}

/// Writes `out` with `write`, through a temporary `.<suffix>.tmp` file in
/// the same folder. Without `overwrite`, an existing `out` is kept and the
/// error is [`io::ErrorKind::AlreadyExists`].
pub(super) fn write_file(
    out: &Path,
    overwrite: bool,
    suffix: &str,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let mut tmp = tempfile::Builder::new()
        .prefix(".guiderecorder-export-")
        .suffix(&format!(".{suffix}.tmp"))
        .tempfile_in(parent_dir(out))?;
    write(tmp.as_file_mut())?;
    tmp.as_file().sync_all()?;
    persist(tmp.into_temp_path(), out, overwrite)
}

/// Moves the finished file at `tmp` to `out`; see [`write_file`].
pub(super) fn persist(tmp: TempPath, out: &Path, overwrite: bool) -> io::Result<()> {
    if overwrite {
        tmp.persist(out).map_err(|e| e.error)?;
    } else {
        tmp.persist_noclobber(out).map_err(|e| e.error)?;
    }
    // Make the rename durable; not all platforms can open a directory.
    if let Ok(dir) = File::open(parent_dir(out)) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Fails with [`io::ErrorKind::AlreadyExists`] if `out` exists and may
/// not be replaced, before any work is done.
pub(super) fn check_free(out: &Path, overwrite: bool) -> io::Result<()> {
    if !overwrite && fs::symlink_metadata(out).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", out.display()),
        ));
    }
    Ok(())
}
