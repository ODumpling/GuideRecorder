//! Collections of the open library (core `collection`): manuals made of
//! several guides, listed, saved and deleted from the library, and exported
//! as one document (PDF, Word, HTML or Markdown) with a table of contents.

use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;

use guiderecorder_core::collection::{self, Collection, CollectionError, CollectionStore};
use guiderecorder_core::export::{docx, html, md, pdf};
use guiderecorder_core::Ulid;
use serde::Serialize;

use crate::editor::{AppError, ErrorKind, Result, Session};
use crate::export::{
    check_branding, check_out_path, exists, md_error, BrandingRequest, ChosenPaths, ExportResult,
    Format,
};

/// A collection as the library shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionView {
    pub id: String,
    /// `None` when the file can't be read.
    pub collection: Option<Collection>,
    /// Why the file can't be read.
    pub error: Option<String>,
}

fn collection_error(e: CollectionError) -> AppError {
    let kind = match e {
        CollectionError::NotFound(_) => ErrorKind::NotFound,
        CollectionError::Invalid(_) | CollectionError::Newer(_) => ErrorKind::Invalid,
        _ => ErrorKind::Io,
    };
    AppError::new(kind, e.to_string())
}

fn store(session: &Session) -> CollectionStore {
    CollectionStore::new(session.root())
}

/// Every collection of the library, by title.
pub fn list(session: &Session) -> Result<Vec<CollectionView>> {
    Ok(store(session)
        .list()
        .map_err(collection_error)?
        .into_iter()
        .map(|entry| match entry.collection {
            Ok(collection) => CollectionView {
                id: entry.id.to_string(),
                collection: Some(collection),
                error: None,
            },
            Err(e) => CollectionView {
                id: entry.id.to_string(),
                collection: None,
                error: Some(e.to_string()),
            },
        })
        .collect())
}

/// A new, empty collection titled `title`, saved.
pub fn create(session: &Session, title: &str) -> Result<Collection> {
    store(session)
        .save(&Collection::new(title))
        .map_err(collection_error)
}

/// Saves `collection` (an existing one: a new one comes from [`create`]);
/// returns it as written.
pub fn save(session: &Session, collection: &Collection) -> Result<Collection> {
    let store = store(session);
    // Only replace a file that is there and readable, so a collection that
    // was deleted elsewhere does not come back.
    store.read(collection.id).map_err(collection_error)?;
    store.save(collection).map_err(collection_error)
}

pub fn delete(session: &Session, id: &str) -> Result<()> {
    let id = parse_collection_id(id)?;
    store(session).delete(id).map_err(collection_error)
}

fn parse_collection_id(id: &str) -> Result<Ulid> {
    Ulid::from_str(id)
        .map_err(|_| AppError::new(ErrorKind::Invalid, format!("{id:?} is not a collection id")))
}

/// Exports collection `id` from its guides' saved bundles to `out` (which
/// [`ChosenPaths::check_destination`] accepts) as one document. Guides
/// that are gone are left out, with a note in the result. Each guide's
/// lock is held throughout, as for a single guide's export.
#[allow(clippy::too_many_arguments)]
pub fn export(
    session: &Session,
    chosen: &ChosenPaths,
    id: &str,
    format: Format,
    out: &Path,
    overwrite: bool,
    branding: &BrandingRequest,
    paper: docx::Paper,
) -> Result<ExportResult> {
    if !matches!(
        format,
        Format::Md | Format::Html | Format::Docx | Format::Pdf
    ) {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "a collection exports as PDF, Word, HTML or Markdown",
        ));
    }
    let id = parse_collection_id(id)?;
    chosen.check_destination(format, out)?;
    let branding = check_branding(session, chosen, branding)?;
    let collection = store(session).read(id).map_err(collection_error)?;

    // Locked in id order, so two exports never wait on each other.
    let ids: BTreeSet<Ulid> = collection.guide_ids().collect();
    let locks: Vec<_> = ids.iter().map(|id| session.guide_lock(*id)).collect();
    let _guards: Vec<_> = locks.iter().map(|lock| lock.lock().unwrap()).collect();
    let mut failed = Vec::new();
    let mut bundles = Vec::new();
    let rendered = collection::render(&collection, session.root(), |guide_id| {
        let loaded = session
            .bundle(guide_id)
            .and_then(|handle| Ok(session.library().read(&handle.path)?));
        match loaded {
            Ok(loaded) if loaded.guide.id == guide_id => {
                bundles.push(loaded.handle.path.clone());
                Some((loaded.guide, loaded.handle))
            }
            Ok(_) => None,
            Err(e) if e.kind == ErrorKind::NotFound => None,
            Err(e) => {
                failed.push(format!("{guide_id}: {}", e.message));
                None
            }
        }
    });
    for bundle in &bundles {
        check_out_path(out, bundle, session.root())?;
    }
    let render = &rendered.guide;
    let handle = &render.handle;
    let timestamp = Some(render.updated_at);

    let mut report = match format {
        Format::Md => {
            let opts = md::MdOptions {
                overwrite,
                branding,
                ..md::MdOptions::default()
            };
            md::export(render, handle, out, &opts).map_err(md_error)?
        }
        Format::Html => {
            let opts = html::HtmlOptions {
                overwrite,
                branding,
                ..html::HtmlOptions::default()
            };
            html::export(render, handle, out, &opts).map_err(md_error)?
        }
        Format::Docx => {
            let opts = docx::DocxOptions {
                overwrite,
                timestamp,
                paper,
                branding,
                ..docx::DocxOptions::default()
            };
            docx::export(render, handle, out, &opts).map_err(|e| match e {
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
                ..pdf::PdfOptions::default()
            };
            pdf::export(render, handle, out, &opts).map_err(|e| match e {
                pdf::PdfError::Exists(path) => exists(&path),
                e => AppError::new(ErrorKind::Io, e.to_string()),
            })?
        }
        _ => unreachable!("checked above"),
    };
    let gone = rendered.missing.len() - failed.len();
    if gone > 0 {
        report.notes.push(if gone == 1 {
            "1 guide of the collection is no longer in the library and was left out".to_owned()
        } else {
            format!(
                "{gone} guides of the collection are no longer in the library and were left out"
            )
        });
    }
    report
        .notes
        .extend(failed.into_iter().map(|e| format!("left out {e}")));
    let result = ExportResult::from(report);
    if let Some(main) = result.files.first() {
        chosen.add_exported(main.clone());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guiderecorder_core::collection::Entry;
    use guiderecorder_core::model::Guide;

    fn session() -> (tempfile::TempDir, Session) {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::start(dir.path(), None).unwrap();
        (dir, session)
    }

    #[test]
    fn create_save_list_delete() {
        let (_dir, session) = session();
        let mut created = create(&session, "Onboarding").unwrap();
        created.intro = "Hi".into();
        created.entries.push(Entry::Text {
            id: Ulid::new(),
            body: "Between".into(),
        });
        let saved = save(&session, &created).unwrap();
        assert_eq!(saved.intro, "Hi");
        let listed = list(&session).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].collection.as_ref(), Some(&saved));

        delete(&session, &saved.id.to_string()).unwrap();
        assert!(list(&session).unwrap().is_empty());
        // Saving a deleted collection does not bring it back.
        let err = save(&session, &saved).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert_eq!(
            delete(&session, "nope").unwrap_err().kind,
            ErrorKind::Invalid
        );
    }

    #[test]
    fn exports_only_to_chosen_destinations_and_formats() {
        let (dir, session) = session();
        let guide = Guide::new("Reset a password", "en");
        session.library().create(&guide).unwrap();
        let mut collection = create(&session, "Manual").unwrap();
        collection.entries = vec![
            Entry::Guide {
                id: Ulid::new(),
                guide_id: guide.id,
            },
            Entry::Guide {
                id: Ulid::new(),
                guide_id: Ulid::new(),
            },
        ];
        let collection = save(&session, &collection).unwrap();
        let chosen = ChosenPaths::default();
        let out_dir = tempfile::tempdir().unwrap();
        let out = out_dir.path().join("manual.pdf");
        let id = collection.id.to_string();
        let branding = BrandingRequest::default();
        let run = |format, out: &Path| {
            export(
                &session,
                &chosen,
                &id,
                format,
                out,
                false,
                &branding,
                docx::Paper::A4,
            )
        };
        assert_eq!(run(Format::Pdf, &out).unwrap_err().kind, ErrorKind::Invalid);
        chosen.add_file(out.clone());
        assert_eq!(
            run(Format::Pptx, &out).unwrap_err().kind,
            ErrorKind::Invalid
        );
        let result = run(Format::Pdf, &out).unwrap();
        assert_eq!(result.files, std::slice::from_ref(&out));
        assert!(
            result.notes.iter().any(|n| n.contains("1 guide")),
            "{:?}",
            result.notes
        );
        chosen.check_exported(&out).unwrap();
        assert_eq!(run(Format::Pdf, &out).unwrap_err().kind, ErrorKind::Exists);

        // Never into the library.
        chosen.add_folder(dir.path().to_path_buf());
        let inside = dir.path().join("export");
        assert_eq!(
            run(Format::Md, &inside).unwrap_err().kind,
            ErrorKind::Invalid
        );
    }
}
