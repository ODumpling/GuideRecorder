//! Version history of a guide (#25): copies of `guide.json` saved in the
//! bundle, so earlier versions outlive the editor's undo.
//!
//! ```text
//! My Guide.guiderecorder/
//!   .history/
//!     versions/   <ulid>.json   one saved version each
//!     objects/    <sha256>      the images versions use, by content hash
//! ```
//!
//! A version file holds its [`VersionInfo`], the whole guide as it was, and
//! the hash of every image the guide used (step screenshots, crops and
//! after frames, by bundle-relative path). Images live once in `objects/`
//! however many versions use them. An object is a hard link to the image
//! when the file system allows it (images are only ever replaced by an
//! atomic rename, never changed in place, so the link keeps the old bytes),
//! else a copy.
//!
//! The app saves a version when a guide closes, before each Write with AI
//! run, before a restore, and when the user asks ("Save version"); the
//! MCP server saves one before each edit it makes. A version identical to
//! the newest one (ignoring `updatedAt`) is not saved again, unless the
//! user asked for it.
//!
//! [`restore`] puts a version's images back and returns its guide; the
//! caller saves it as the guide's current state, after saving the current
//! state as a version, so a restore can itself be undone. [`prune`] applies
//! the user's [`Retention`] and drops images no version uses any more.
//!
//! Like `.backup/`, `.history/` is hidden, never descended into by the
//! library walk and not copied by [`Library::duplicate`].

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use ulid::Ulid;

use crate::library::{
    io_err, upgrade, verify_header, write_atomic, Durability, GuideHandle, Library, LibraryError,
    Result,
};
use crate::model::Guide;

/// The history folder in a bundle.
pub const HISTORY_DIR: &str = ".history";
const VERSIONS_DIR: &str = "versions";
const OBJECTS_DIR: &str = "objects";

/// Longest label kept, in characters.
pub const MAX_LABEL_CHARS: usize = 200;

/// Why a version was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VersionReason {
    /// The guide was closed in the editor.
    Closed,
    /// Before a Write with AI run.
    BeforeAi,
    /// The user saved it ("Save version").
    Manual,
    /// Before an older version was restored over it.
    BeforeRestore,
    /// Before an edit through the MCP server.
    BeforeMcpEdit,
    /// Saved by a newer app for a reason this one does not know.
    #[serde(other)]
    Other,
}

/// A saved version, without its guide: a row of the history list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: Ulid,
    /// When the version was saved.
    pub created_at: DateTime<Utc>,
    pub reason: VersionReason,
    /// The user's name for it; versions with one are never pruned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The guide's title then.
    pub title: String,
    pub step_count: usize,
    /// The guide's `updatedAt` then: when its content last changed.
    pub updated_at: DateTime<Utc>,
}

/// A saved version with its guide.
#[derive(Debug, Clone)]
pub struct Version {
    pub info: VersionInfo,
    /// Migrated to the current schema if it was saved by an older app.
    pub guide: Guide,
}

/// A version file as stored.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionFile {
    #[serde(flatten)]
    info: VersionInfo,
    /// Bundle-relative image path → SHA-256 of its bytes (lowercase hex).
    images: BTreeMap<String, String>,
    guide: Value,
}

/// How many versions to keep. A version goes when it is outside either
/// limit; `None` is no limit. Labelled versions and the newest version are
/// always kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retention {
    /// Keep at most this many unlabelled versions, the newest.
    pub max_versions: Option<usize>,
    /// Keep versions at most this old.
    pub max_age: Option<Duration>,
}

/// The real folders of `bundle`'s history that may hold temp files, for
/// the library's crash recovery.
pub(crate) fn store_dirs(bundle: &Path) -> Vec<PathBuf> {
    let history = bundle.join(HISTORY_DIR);
    if !is_real_dir(&history) {
        return Vec::new();
    }
    vec![history.join(VERSIONS_DIR), history.join(OBJECTS_DIR)]
}

/// Saves `guide` as a new version of the guide in `handle`, with its
/// images. Returns `None` without saving when `reason` is not
/// [`VersionReason::Manual`] and the newest version already holds the same
/// guide and images. An image file that is missing is left out (the
/// version then cannot bring it back); a `label` is trimmed and cut to
/// [`MAX_LABEL_CHARS`].
pub fn snapshot(
    handle: &GuideHandle,
    guide: &Guide,
    reason: VersionReason,
    label: Option<&str>,
) -> Result<Option<VersionInfo>> {
    if guide.id != handle.id {
        return Err(LibraryError::IdMismatch {
            path: handle.path.clone(),
            expected: handle.id,
            found: guide.id,
        });
    }
    verify_header(handle)?;
    let versions = ensure_dir(handle, &format!("{HISTORY_DIR}/{VERSIONS_DIR}"))?;
    let objects = ensure_dir(handle, &format!("{HISTORY_DIR}/{OBJECTS_DIR}"))?;

    let mut images = BTreeMap::new();
    for relative in image_paths(guide) {
        let Ok(source) = handle.resolve(&relative) else {
            continue;
        };
        if !fs::symlink_metadata(&source).is_ok_and(|m| m.is_file()) {
            continue;
        }
        images.insert(relative, store_object(&objects, &source)?);
    }

    let value = serde_json::to_value(guide).map_err(|source| LibraryError::Json {
        path: handle.guide_json(),
        source,
    })?;
    if reason != VersionReason::Manual {
        if let Some(newest) = newest_file(&versions)? {
            if newest.images == images && same_content(&newest.guide, &value) {
                return Ok(None);
            }
        }
    }

    let label = label
        .map(|l| l.trim().chars().take(MAX_LABEL_CHARS).collect::<String>())
        .filter(|l| !l.is_empty());
    let info = VersionInfo {
        id: Ulid::new(),
        created_at: Utc::now(),
        reason,
        label,
        title: guide.title.clone(),
        step_count: guide.steps().count(),
        updated_at: guide.updated_at,
    };
    let file = VersionFile {
        info: info.clone(),
        images,
        guide: value,
    };
    let path = versions.join(format!("{}.json", info.id));
    let json = serde_json::to_vec(&file).map_err(|source| LibraryError::Json {
        path: path.clone(),
        source,
    })?;
    write_atomic(&path, &json, Durability::RenameOnly)?;
    // A prune in another process (the app and the MCP server share the
    // bundle) may have dropped an object between its write and now.
    for (relative, hash) in &file.images {
        if !objects.join(hash).is_file() {
            if let Ok(source) = handle.resolve(relative) {
                store_object(&objects, &source)?;
            }
        }
    }
    Ok(Some(info))
}

/// The guide's versions, newest first. Files that do not decode are left
/// out; a bundle without history has none.
pub fn list(handle: &GuideHandle) -> Result<Vec<VersionInfo>> {
    let Some(dir) = existing_dir(handle, VERSIONS_DIR)? else {
        return Ok(Vec::new());
    };
    let mut out: Vec<VersionInfo> = version_files(&dir)?
        .into_iter()
        .filter_map(|(_, path)| {
            let bytes = fs::read(&path).ok()?;
            serde_json::from_slice(&bytes).ok()
        })
        .collect();
    // Not by id alone: ids made in the same millisecond are in random order.
    out.sort_by_key(|v| std::cmp::Reverse((v.created_at, v.id)));
    Ok(out)
}

/// Version `id` with its guide, migrated like [`Library::read`]. A version
/// saved by a newer app that this one cannot decode is
/// [`LibraryError::NewerSchema`].
pub fn read(library: &Library, handle: &GuideHandle, id: Ulid) -> Result<Version> {
    let (path, file) = read_file(handle, id)?;
    Ok(Version {
        guide: decode_guide(library, &path, file.guide)?.0,
        info: file.info,
    })
}

/// The bytes of image `relative` (a bundle-relative path in the guide) as
/// version `id` saved it.
pub fn image(handle: &GuideHandle, id: Ulid, relative: &str) -> Result<Vec<u8>> {
    let (_, file) = read_file(handle, id)?;
    let hash = file
        .images
        .get(relative)
        .ok_or_else(|| LibraryError::InvalidRelativePath(relative.to_owned()))?;
    read_object(handle, hash)
}

/// Puts version `id`'s images back in the bundle (where they are missing
/// or differ) and returns its guide, with `updated_at` set to now, for the
/// caller to save. Writes nothing else. A version from a newer app is
/// refused with [`LibraryError::NewerSchema`]: restoring it would drop
/// what this app does not know.
pub fn restore(library: &Library, handle: &GuideHandle, id: Ulid) -> Result<Guide> {
    verify_header(handle)?;
    let (path, file) = read_file(handle, id)?;
    let (mut guide, version) = decode_guide(library, &path, file.guide)?;
    if version > library.schema().version {
        return Err(LibraryError::NewerSchema {
            path,
            found: version,
            supported: library.schema().version,
        });
    }
    if guide.id != handle.id {
        return Err(LibraryError::DamagedHistory(path));
    }
    for (relative, hash) in &file.images {
        let target = handle.resolve(relative)?;
        if fs::symlink_metadata(&target).is_ok_and(|m| m.is_file())
            && hash_file(&target).is_ok_and(|h| &h == hash)
        {
            continue;
        }
        let bytes = read_object(handle, hash)?;
        if let Some(parent) = target.parent() {
            match fs::create_dir(parent) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(io_err(parent)(e)),
            }
        }
        write_atomic(&target, &bytes, Durability::RenameOnly)?;
    }
    guide.updated_at = Utc::now();
    Ok(guide)
}

/// Deletes versions outside `retention` (as of `now`), then images no
/// remaining version uses. Returns the deleted versions' ids. Leaves the
/// images alone if a version file cannot be read.
pub fn prune(handle: &GuideHandle, retention: &Retention, now: DateTime<Utc>) -> Result<Vec<Ulid>> {
    let Some(dir) = existing_dir(handle, VERSIONS_DIR)? else {
        return Ok(Vec::new());
    };
    let versions = list(handle)?;
    let mut removed = Vec::new();
    let mut unlabelled = 0usize;
    for (i, info) in versions.iter().enumerate() {
        if info.label.is_some() {
            continue;
        }
        unlabelled += 1;
        let too_many = retention.max_versions.is_some_and(|max| unlabelled > max);
        let too_old = retention
            .max_age
            .is_some_and(|max| now.signed_duration_since(info.created_at) > max);
        if i > 0 && (too_many || too_old) {
            let path = dir.join(format!("{}.json", info.id));
            match fs::remove_file(&path) {
                Ok(()) => removed.push(info.id),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err(&path)(e)),
            }
        }
    }
    collect_garbage(handle, &dir)?;
    Ok(removed)
}

/// Removes objects that no version file mentions.
fn collect_garbage(handle: &GuideHandle, versions: &Path) -> Result<()> {
    let Some(objects) = existing_dir(handle, OBJECTS_DIR)? else {
        return Ok(());
    };
    let mut used = HashSet::new();
    for (_, path) in version_files(versions)? {
        let file = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<VersionFile>(&bytes).ok());
        let Some(file) = file else {
            return Ok(());
        };
        used.extend(file.images.into_values());
    }
    for entry in fs::read_dir(&objects).map_err(io_err(&objects))? {
        let entry = entry.map_err(io_err(&objects))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !is_hash(&name) || used.contains(&name) {
            continue;
        }
        let path = entry.path();
        if let Err(e) = fs::remove_file(&path) {
            if e.kind() != io::ErrorKind::NotFound {
                return Err(io_err(&path)(e));
            }
        }
    }
    Ok(())
}

/// Every image path a guide uses, without repeats.
fn image_paths(guide: &Guide) -> Vec<String> {
    let mut seen = HashSet::new();
    guide
        .steps()
        .flat_map(|step| {
            std::iter::once(step.image.full.as_str())
                .chain(step.image.crop.as_deref())
                .chain(step.image.after.as_deref())
        })
        .filter(|path| !path.is_empty() && seen.insert(*path))
        .map(str::to_owned)
        .collect()
}

/// Puts the image at `source` in `objects` under its hash, unless it is
/// there already, and returns the hash. The image is hard-linked to a temp
/// name first and hashed through that link, so a rename over `source`
/// meanwhile cannot store bytes under the wrong hash.
fn store_object(objects: &Path, source: &Path) -> Result<String> {
    let temp = objects.join(format!(".link-{}.tmp", Ulid::new()));
    if fs::hard_link(source, &temp).is_ok() {
        let stored = hash_file(&temp).and_then(|hash| {
            let target = objects.join(&hash);
            if target.exists() {
                return Ok(hash);
            }
            fs::rename(&temp, &target).map_err(io_err(&target))?;
            Ok(hash)
        });
        let _ = fs::remove_file(&temp);
        return stored;
    }
    // No hard links here (some synced or network folders): copy.
    let bytes = fs::read(source).map_err(io_err(source))?;
    let hash = hex(&Sha256::digest(&bytes));
    let target = objects.join(&hash);
    if !target.exists() {
        write_atomic(&target, &bytes, Durability::RenameOnly)?;
    }
    Ok(hash)
}

fn read_object(handle: &GuideHandle, hash: &str) -> Result<Vec<u8>> {
    if !is_hash(hash) {
        return Err(LibraryError::InvalidRelativePath(hash.to_owned()));
    }
    let path = handle.resolve(&format!("{HISTORY_DIR}/{OBJECTS_DIR}/{hash}"))?;
    let bytes = fs::read(&path).map_err(io_err(&path))?;
    if hex(&Sha256::digest(&bytes)) != hash {
        return Err(LibraryError::DamagedHistory(path));
    }
    Ok(bytes)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(io_err(path))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(io_err(path))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn is_hash(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether two guide values differ in nothing but `updatedAt`.
fn same_content(a: &Value, b: &Value) -> bool {
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(map) = v.as_object_mut() {
            map.remove("updatedAt");
        }
        v
    };
    strip(a) == strip(b)
}

/// Decodes a version's guide, migrating an older one. Returns it and the
/// schema version it was saved at.
fn decode_guide(library: &Library, path: &Path, value: Value) -> Result<(Guide, u32)> {
    let (value, version) = upgrade(path, value, library.schema())?;
    let guide = serde_json::from_value(value).map_err(|source| {
        if version > library.schema().version {
            LibraryError::NewerSchema {
                path: path.to_path_buf(),
                found: version,
                supported: library.schema().version,
            }
        } else {
            LibraryError::Json {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    Ok((guide, version))
}

fn read_file(handle: &GuideHandle, id: Ulid) -> Result<(PathBuf, VersionFile)> {
    let not_found = || LibraryError::VersionNotFound(id);
    let dir = existing_dir(handle, VERSIONS_DIR)?.ok_or_else(not_found)?;
    let path = dir.join(format!("{id}.json"));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(not_found()),
        Err(e) => return Err(io_err(&path)(e)),
    };
    let file = serde_json::from_slice(&bytes).map_err(|source| LibraryError::Json {
        path: path.clone(),
        source,
    })?;
    Ok((path, file))
}

/// The newest decodable version file in `dir`.
fn newest_file(dir: &Path) -> Result<Option<VersionFile>> {
    Ok(version_files(dir)?
        .into_iter()
        .filter_map(|(_, path)| {
            let bytes = fs::read(path).ok()?;
            serde_json::from_slice::<VersionFile>(&bytes).ok()
        })
        .max_by_key(|file| (file.info.created_at, file.info.id)))
}

/// `<ulid>.json` files in `dir`, with their ids.
fn version_files(dir: &Path) -> Result<Vec<(Ulid, PathBuf)>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(io_err(dir))? {
        let entry = entry.map_err(io_err(dir))?;
        let name = entry.file_name();
        let Some(id) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".json"))
            .and_then(|n| n.parse::<Ulid>().ok())
        else {
            continue;
        };
        let path = entry.path();
        if fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            out.push((id, path));
        }
    }
    Ok(out)
}

/// `.history/<name>` if it is a real folder; `None` if it is missing.
/// Symlinks anywhere on the way are refused.
fn existing_dir(handle: &GuideHandle, name: &str) -> Result<Option<PathBuf>> {
    let path = handle.resolve(&format!("{HISTORY_DIR}/{name}"))?;
    match fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_dir() => Ok(Some(path)),
        Ok(_) => Err(LibraryError::DamagedHistory(path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(&path)(e)),
    }
}

/// Makes the bundle folder `relative` (and `.history`) as real folders if
/// missing; refuses symlinks.
fn ensure_dir(handle: &GuideHandle, relative: &str) -> Result<PathBuf> {
    let mut path = handle.path.clone();
    let mut sofar = String::new();
    for part in relative.split('/') {
        if !sofar.is_empty() {
            sofar.push('/');
        }
        sofar.push_str(part);
        path = handle.resolve(&sofar)?;
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if !is_real_dir(&path) {
                    return Err(LibraryError::DamagedHistory(path));
                }
            }
            Err(e) => return Err(io_err(&path)(e)),
        }
    }
    Ok(path)
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{PixelBuffer, StepImageVariant};
    use crate::model::{Item, Step};

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::open(dir.path().join("Library")).unwrap();
        (dir, lib)
    }

    fn step(handle: &GuideHandle, title: &str, shade: u8) -> Step {
        let id = Ulid::new();
        let pixels = vec![shade; 4 * 4 * 4];
        let full = handle
            .write_step_image(
                id,
                StepImageVariant::Full,
                &PixelBuffer::rgba(4, 4, &pixels),
            )
            .unwrap();
        serde_json::from_value(serde_json::json!({
            "id": id,
            "title": title,
            "kind": "click",
            "image": { "full": full },
        }))
        .unwrap()
    }

    /// A saved guide with one step and its image.
    fn guide(lib: &Library) -> (GuideHandle, Guide) {
        let mut guide = Guide::new("Original", "en");
        let handle = lib.create(&guide).unwrap();
        guide
            .items
            .push(Item::Step(step(&handle, "Click Save", 10)));
        lib.save_to(&handle, &guide).unwrap();
        (handle, guide)
    }

    fn object_count(handle: &GuideHandle) -> usize {
        fs::read_dir(handle.path.join(HISTORY_DIR).join(OBJECTS_DIR))
            .unwrap()
            .filter(|e| is_hash(e.as_ref().unwrap().file_name().to_str().unwrap()))
            .count()
    }

    #[test]
    fn snapshots_list_newest_first_and_share_images() {
        let (_tmp, lib) = library();
        let (handle, mut guide) = guide(&lib);
        let first = snapshot(&handle, &guide, VersionReason::Closed, None)
            .unwrap()
            .unwrap();
        guide.title = "Renamed".into();
        let second = snapshot(
            &handle,
            &guide,
            VersionReason::Manual,
            Some("  Before review  "),
        )
        .unwrap()
        .unwrap();
        let listed = list(&handle).unwrap();
        assert_eq!(
            listed.iter().map(|v| v.id).collect::<Vec<_>>(),
            [second.id, first.id]
        );
        assert_eq!(listed[0].label.as_deref(), Some("Before review"));
        assert_eq!(listed[0].title, "Renamed");
        assert_eq!(listed[1].step_count, 1);
        // Both versions use the same screenshot, stored once.
        assert_eq!(object_count(&handle), 1);
    }

    #[test]
    fn an_unchanged_guide_is_not_saved_again_unless_asked() {
        let (_tmp, lib) = library();
        let (handle, mut guide) = guide(&lib);
        assert!(snapshot(&handle, &guide, VersionReason::Closed, None)
            .unwrap()
            .is_some());
        guide.updated_at = Utc::now() + Duration::seconds(5);
        assert!(snapshot(&handle, &guide, VersionReason::BeforeAi, None)
            .unwrap()
            .is_none());
        assert!(snapshot(&handle, &guide, VersionReason::Manual, None)
            .unwrap()
            .is_some());
        assert_eq!(list(&handle).unwrap().len(), 2);
    }

    #[test]
    fn restore_brings_back_text_and_deleted_images() {
        let (_tmp, lib) = library();
        let (handle, guide) = guide(&lib);
        let old = snapshot(&handle, &guide, VersionReason::Closed, None)
            .unwrap()
            .unwrap();
        let Item::Step(old_step) = &guide.items[0] else {
            unreachable!()
        };
        let image = handle.resolve(&old_step.image.full).unwrap();
        let original_bytes = fs::read(&image).unwrap();

        // The step is deleted and its image swept; another replaces it.
        let mut edited = guide.clone();
        edited.items = vec![Item::Step(step(&handle, "Click Open", 200))];
        lib.save_to(&handle, &edited).unwrap();
        fs::remove_file(&image).unwrap();

        let restored = restore(&lib, &handle, old.id).unwrap();
        assert_eq!(restored.items, guide.items);
        assert!(restored.updated_at > guide.updated_at);
        assert_eq!(fs::read(&image).unwrap(), original_bytes);
        assert_eq!(
            read(&lib, &handle, old.id).unwrap().guide.items,
            guide.items
        );
        assert_eq!(
            super::image(&handle, old.id, &old_step.image.full).unwrap(),
            original_bytes
        );
    }

    #[test]
    fn a_replaced_image_keeps_its_old_bytes_in_history() {
        let (_tmp, lib) = library();
        let (handle, guide) = guide(&lib);
        let old = snapshot(&handle, &guide, VersionReason::Closed, None)
            .unwrap()
            .unwrap();
        let Item::Step(s) = &guide.items[0] else {
            unreachable!()
        };
        let path = handle.resolve(&s.image.full).unwrap();
        let before = fs::read(&path).unwrap();
        // Images are replaced by an atomic rename, as the app does.
        write_atomic(&path, b"new bytes", Durability::RenameOnly).unwrap();
        assert_eq!(
            super::image(&handle, old.id, &s.image.full).unwrap(),
            before
        );
        restore(&lib, &handle, old.id).unwrap();
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn damaged_objects_are_refused() {
        let (_tmp, lib) = library();
        let (handle, guide) = guide(&lib);
        let old = snapshot(&handle, &guide, VersionReason::Closed, None)
            .unwrap()
            .unwrap();
        let objects = handle.path.join(HISTORY_DIR).join(OBJECTS_DIR);
        let object = fs::read_dir(&objects)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        // Break the link first, so the step image itself is left alone.
        fs::remove_file(&object).unwrap();
        fs::write(&object, b"garbage").unwrap();
        let Item::Step(s) = &guide.items[0] else {
            unreachable!()
        };
        fs::remove_file(handle.resolve(&s.image.full).unwrap()).unwrap();
        assert!(matches!(
            restore(&lib, &handle, old.id),
            Err(LibraryError::DamagedHistory(_))
        ));
    }

    #[test]
    fn unknown_versions_are_not_found() {
        let (_tmp, lib) = library();
        let (handle, _) = guide(&lib);
        let id = Ulid::new();
        assert!(list(&handle).unwrap().is_empty());
        assert!(matches!(
            read(&lib, &handle, id),
            Err(LibraryError::VersionNotFound(found)) if found == id
        ));
    }

    #[test]
    fn prune_keeps_labelled_and_newest_versions_and_drops_unused_images() {
        let (_tmp, lib) = library();
        let (handle, mut guide) = guide(&lib);
        let mut ids = Vec::new();
        for i in 0..4 {
            guide.title = format!("Take {i}");
            // Each version has its own screenshot.
            guide.items = vec![Item::Step(step(&handle, "Step", i * 40))];
            let label = (i == 0).then_some("Keep me");
            let info = snapshot(&handle, &guide, VersionReason::Manual, label)
                .unwrap()
                .unwrap();
            ids.push(info.id);
        }
        assert_eq!(object_count(&handle), 4);
        let removed = prune(
            &handle,
            &Retention {
                max_versions: Some(2),
                max_age: None,
            },
            Utc::now(),
        )
        .unwrap();
        // Unlabelled, newest first: 3, 2 kept, 1 dropped; 0 is labelled.
        assert_eq!(removed, [ids[1]]);
        let left: Vec<_> = list(&handle).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(left, [ids[3], ids[2], ids[0]]);
        assert_eq!(object_count(&handle), 3);

        // By age, everything but the newest and the labelled one goes.
        let later = Utc::now() + Duration::days(100);
        let removed = prune(
            &handle,
            &Retention {
                max_versions: None,
                max_age: Some(Duration::days(90)),
            },
            later,
        )
        .unwrap();
        assert_eq!(removed, [ids[2]]);
        assert_eq!(object_count(&handle), 2);
    }

    #[test]
    fn duplicates_leave_history_behind() {
        let (_tmp, lib) = library();
        let (handle, guide) = guide(&lib);
        snapshot(&handle, &guide, VersionReason::Closed, None).unwrap();
        let copy = lib.duplicate(&handle, "Copy").unwrap();
        assert!(list(&copy).unwrap().is_empty());
        assert!(!copy.path.join(HISTORY_DIR).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_history_is_refused() {
        let (tmp, lib) = library();
        let (handle, guide) = guide(&lib);
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, handle.path.join(HISTORY_DIR)).unwrap();
        assert!(matches!(
            snapshot(&handle, &guide, VersionReason::Closed, None),
            Err(LibraryError::SymlinkInBundle(_))
        ));
        assert!(fs::read_dir(&elsewhere).unwrap().next().is_none());
    }
}
