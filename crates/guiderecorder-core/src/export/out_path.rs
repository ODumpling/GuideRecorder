//! The export destination check shared by the CLI and the app: an export
//! must never write into a guide bundle or the library.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::library::{BUNDLE_EXTENSION, LEGACY_BUNDLE_EXTENSION};

/// Why [`check_out_path`] refused an output path. The messages leave out
/// the output path itself, so callers can name it their own way (`--out`
/// in the CLI, the chosen file in the app).
#[derive(Debug, thiserror::Error)]
pub enum OutPathError {
    /// The output is inside this guide's bundle or another `.guiderecorder`
    /// folder (the folder given).
    #[error("inside the guide bundle {}", .0.display())]
    InsideBundle(PathBuf),
    /// The output is inside the library folder (given).
    #[error("inside the library {}", .0.display())]
    InsideLibrary(PathBuf),
    /// `path` (the output path or the bundle, as passed in) could not be
    /// resolved, e.g. a dangling link or an unreadable folder.
    #[error("{}: cannot resolve path", path.display())]
    Unresolvable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// The output path (as passed in) has `..` after a folder that does
    /// not exist, which the OS can't resolve.
    #[error("{}: `..` after a folder that does not exist; give a plain path", .0.display())]
    DotDotAfterMissing(PathBuf),
}

/// Refuses an output path inside this guide's bundle, inside any
/// `.guiderecorder` folder (any case), or inside the library folder when it is
/// known, so an export can never change a library. The check uses the
/// real location `resolve_out` finds, so symlinks and `..` can't get
/// around it; that location is returned. A library that can't be resolved
/// (e.g. it doesn't exist) is skipped.
///
/// The check is time-of-check only: the exporters then write to `out` by
/// path, so someone who can change the folders on the way between the
/// check and the write (swap a folder for a link) could still redirect it.
/// That needs write access to the destination's parent folders, which the
/// user running the export already has, so it is not a privilege boundary;
/// the check guards against mistakes, not a hostile local process.
pub fn check_out_path(
    out: &Path,
    bundle: &Path,
    library: Option<&Path>,
) -> Result<PathBuf, OutPathError> {
    let resolved = resolve_out(out)?;
    let bundle = std::fs::canonicalize(bundle).map_err(|source| OutPathError::Unresolvable {
        path: bundle.to_path_buf(),
        source,
    })?;
    if resolved.starts_with(&bundle) {
        return Err(OutPathError::InsideBundle(bundle));
    }
    let is_bundle = |p: &&Path| {
        p.extension().is_some_and(|e| {
            e.eq_ignore_ascii_case(BUNDLE_EXTENSION)
                || e.eq_ignore_ascii_case(LEGACY_BUNDLE_EXTENSION)
        })
    };
    if let Some(other) = resolved.ancestors().find(is_bundle) {
        return Err(OutPathError::InsideBundle(other.to_path_buf()));
    }
    if let Some(library) = library.and_then(|l| std::fs::canonicalize(l).ok()) {
        if resolved.starts_with(&library) {
            return Err(OutPathError::InsideLibrary(library));
        }
    }
    Ok(resolved)
}

/// The real location `out` names: its deepest part that exists (or is a
/// link) canonicalized, so the OS resolves symlinks and `..` in it, plus
/// the rest, which must be plain names. A dangling link is refused.
fn resolve_out(out: &Path) -> Result<PathBuf, OutPathError> {
    let cannot = |source| OutPathError::Unresolvable {
        path: out.to_path_buf(),
        source,
    };
    let absolute = std::path::absolute(out).map_err(cannot)?;
    let components: Vec<Component> = absolute.components().collect();
    let mut split = components.len();
    while split > 1 {
        let prefix: PathBuf = components[..split].iter().collect();
        if prefix.symlink_metadata().is_ok() {
            break;
        }
        split -= 1;
    }
    let prefix: PathBuf = components[..split].iter().collect();
    let mut resolved = std::fs::canonicalize(&prefix).map_err(cannot)?;
    for component in &components[split..] {
        match component {
            Component::Normal(name) => resolved.push(name),
            _ => return Err(OutPathError::DotDotAfterMissing(out.to_path_buf())),
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A tempdir (canonicalized: macOS temp folders sit behind a link)
    /// holding `Library/Guide.guiderecorder/steps` and `Exports`. Git doesn't
    /// keep empty folders, so every folder a test needs is made here.
    struct Fx {
        _dir: tempfile::TempDir,
        root: PathBuf,
        library: PathBuf,
        bundle: PathBuf,
        exports: PathBuf,
    }

    impl Fx {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = fs::canonicalize(dir.path()).unwrap();
            let library = root.join("Library");
            let bundle = library.join("Guide.guiderecorder");
            fs::create_dir_all(bundle.join("steps")).unwrap();
            let exports = root.join("Exports");
            fs::create_dir_all(&exports).unwrap();
            Fx {
                _dir: dir,
                root,
                library,
                bundle,
                exports,
            }
        }

        fn check(&self, out: &Path) -> Result<PathBuf, OutPathError> {
            check_out_path(out, &self.bundle, Some(&self.library))
        }
    }

    #[test]
    fn plain_paths_are_allowed_and_resolved() {
        let fx = Fx::new();
        let out = fx.exports.join("Guide.pdf");
        assert_eq!(fx.check(&out).unwrap(), out);
        let nested = fx.exports.join("new/deeper/Guide");
        assert_eq!(fx.check(&nested).unwrap(), nested);
        // `..` through folders that exist is fine when it lands elsewhere.
        let up = fx.exports.join("../Exports/x.pdf");
        assert_eq!(fx.check(&up).unwrap(), fx.exports.join("x.pdf"));
        // Without a library, only bundles are refused.
        let in_library = fx.library.join("x.pdf");
        assert_eq!(
            check_out_path(&in_library, &fx.bundle, None).unwrap(),
            in_library
        );
    }

    #[test]
    fn this_bundle_is_refused() {
        let fx = Fx::new();
        for out in [
            fx.bundle.join("guide.json"),
            fx.bundle.join("steps/x.webp"),
            fx.bundle.clone(),
            fx.exports.join("../Library/Guide.guiderecorder/x"),
        ] {
            let err = fx.check(&out).unwrap_err();
            assert!(
                matches!(&err, OutPathError::InsideBundle(p) if *p == fx.bundle),
                "{}: {err}",
                out.display()
            );
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_dotdot_into_the_bundle_is_refused() {
        // `link/..` is the parent of the link's target (the bundle), so a
        // lexical `..` would have let this overwrite guide.json.
        let fx = Fx::new();
        let link = fx.root.join("link");
        std::os::unix::fs::symlink(fx.bundle.join("steps"), &link).unwrap();
        let err = fx.check(&link.join("..").join("guide.json")).unwrap_err();
        assert!(matches!(err, OutPathError::InsideBundle(p) if p == fx.bundle));
    }

    #[test]
    fn other_bundles_are_refused_in_any_case() {
        let fx = Fx::new();
        for name in [
            "Other.guiderecorder",
            "Other.GUIDERECORDER",
            "Other.GuideRecorder",
            // Bundles from before the rename.
            "Legacy.guidesya",
            "Legacy.GUIDESYA",
        ] {
            let out = fx.root.join(name).join("x.pdf");
            let err = fx.check(&out).unwrap_err();
            assert!(
                matches!(&err, OutPathError::InsideBundle(p) if *p == fx.root.join(name)),
                "{name}: {err}"
            );
            // Nothing is created by the check.
            assert!(!fx.root.join(name).exists());
        }
    }

    #[test]
    fn the_library_is_refused_with_or_without_a_trailing_slash() {
        let fx = Fx::new();
        // The platform's separator: in the `\\?\` paths Windows
        // canonicalizes to, `/` is part of the name.
        let slash = PathBuf::from(format!(
            "{}{}",
            fx.library.display(),
            std::path::MAIN_SEPARATOR
        ));
        for library in [fx.library.clone(), slash] {
            for out in [fx.library.join("x.pdf"), fx.library.join("md/folder")] {
                let err = check_out_path(&out, &fx.bundle, Some(&library)).unwrap_err();
                assert!(
                    matches!(&err, OutPathError::InsideLibrary(p) if *p == fx.library),
                    "{}: {err}",
                    library.display()
                );
            }
        }
        // Also through `..` and through a link to the library.
        assert!(matches!(
            fx.check(&fx.exports.join("../Library/x.pdf")),
            Err(OutPathError::InsideLibrary(_))
        ));
        #[cfg(unix)]
        {
            let link = fx.root.join("lib-link");
            std::os::unix::fs::symlink(&fx.library, &link).unwrap();
            assert!(matches!(
                fx.check(&link.join("x.pdf")),
                Err(OutPathError::InsideLibrary(_))
            ));
        }
    }

    #[test]
    #[cfg(unix)]
    fn dotdot_after_a_missing_folder_is_refused() {
        let fx = Fx::new();
        let out = fx.exports.join("missing/../x.pdf");
        let err = fx.check(&out).unwrap_err();
        assert!(matches!(&err, OutPathError::DotDotAfterMissing(p) if *p == out));
        assert!(err.to_string().contains("`..`"), "{err}");
    }

    /// Windows takes `..` off a path by its text before any folder is
    /// looked up, so `missing/..` names no folder at all.
    #[test]
    #[cfg(windows)]
    fn dotdot_after_a_missing_folder_is_dropped_on_windows() {
        let fx = Fx::new();
        let out = fx.exports.join("missing/../x.pdf");
        assert_eq!(fx.check(&out).unwrap(), fx.exports.join("x.pdf"));
    }

    #[test]
    #[cfg(unix)]
    fn a_dangling_link_is_refused() {
        let fx = Fx::new();
        let link = fx.exports.join("dangling");
        std::os::unix::fs::symlink(fx.root.join("nowhere"), &link).unwrap();
        for out in [link.clone(), link.join("x.pdf")] {
            let err = fx.check(&out).unwrap_err();
            assert!(
                matches!(&err, OutPathError::Unresolvable { path, .. } if *path == out),
                "{}: {err}",
                out.display()
            );
        }
    }

    #[test]
    fn a_missing_bundle_is_unresolvable() {
        let fx = Fx::new();
        let bundle = fx.root.join("Gone.guiderecorder");
        let err = check_out_path(&fx.exports.join("x.pdf"), &bundle, None).unwrap_err();
        assert!(matches!(err, OutPathError::Unresolvable { path, .. } if path == bundle));
    }
}
