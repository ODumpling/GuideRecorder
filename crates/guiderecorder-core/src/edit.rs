//! Edits from outside the app's editor: the MCP server, when started with
//! `--writable`. Guide-level fields, step text, notes, item order, tags and
//! the cover, applied to a guide in memory by the functions here and written by
//! [`Library::edit`] through [`Library::save_to`] (one atomic `guide.json`
//! rewrite). New annotated guides are [`crate::pages::create_from_images`].
//!
//! # Provenance
//!
//! An edit here is the user's own, made through their assistant, so it
//! marks text as the editor does (`app/src/editor/commands.ts`): a changed
//! guide-level field joins [`Guide::edited_fields`] and the guide's
//! `ai_state` becomes `edited`; a changed step title or description makes
//! the step's `ai_state` `edited`. AI passes then leave that text alone.
//! Text the user already edited may be changed: they asked for it. Values
//! that do not change mark nothing, and adding notes and moving items mark
//! nothing, as in the editor.
//!
//! # The editor lock
//!
//! The app keeps the guide open in its editor in memory and saves it whole,
//! so a write from outside would be lost at its next save. While a guide is
//! open the app keeps [`EDITOR_LOCK_FILE`] in its bundle, holding the app's
//! process id ([`Library::lock_for_editor`]), and [`Library::edit`] refuses
//! a guide whose lock names a running process. A lock left behind by an app
//! that crashed names a process that is gone, and is ignored.
//!
//! Before each write the guide as it was is saved in its version history
//! ([`crate::history`]), so the user can take an edit back.
//!
//! The lock is checked before the guide is read and again just before the
//! write, and the write is refused if `guide.json` changed in between. An
//! app opening the guide in the few milliseconds of the write itself can
//! still load the old copy; that window is not closed.

use std::collections::HashSet;
use std::fs;
use std::io;

use chrono::Utc;
use ulid::Ulid;

use crate::history::{self, VersionReason};
use crate::library::{GuideHandle, Library, LibraryError};
use crate::model::{AiStatus, CoverOptions, Guide, GuideField, GuideStatus, Item, Note, NoteKind};

/// Marker file in a bundle while the guide is open in the app's editor: the
/// app's process id in decimal, then a newline.
pub const EDITOR_LOCK_FILE: &str = ".editor.lock";

/// Why an edit was not made.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error(transparent)]
    Library(#[from] LibraryError),
    /// The guide is open in the app's editor (process `pid`).
    #[error(
        "guide {id} is open in the GuideRecorder editor; close it there (or switch to another \
         guide) and try again"
    )]
    OpenInEditor { id: Ulid, pid: u32 },
    /// The app would not edit the guide as it is (see [`ReadOnlyReason`]).
    #[error("guide {id} is read-only: {reason}")]
    ReadOnly { id: Ulid, reason: ReadOnlyReason },
    /// A bad argument: an unknown item, an empty title, an incomplete order.
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T, E = EditError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyReason {
    /// Saved by a newer app.
    NewerSchema,
    /// An older format, which the app upgrades (with a backup) when it
    /// next opens the guide.
    OlderSchema,
}

impl std::fmt::Display for ReadOnlyReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NewerSchema => "it was saved by a newer GuideRecorder",
            Self::OlderSchema => {
                "it uses an older format; open it once in GuideRecorder to upgrade it"
            }
        })
    }
}

/// The result of [`Library::edit`].
#[derive(Debug, Clone)]
pub struct Edited<T> {
    pub handle: GuideHandle,
    /// The guide as saved (or as it was, if unchanged).
    pub guide: Guide,
    /// Whether anything changed; nothing is written otherwise.
    pub changed: bool,
    /// What the edit returned.
    pub value: T,
}

impl Library {
    /// Applies `edit` to guide `id` and saves it, unless it is open in the
    /// app's editor or read-only (see the module docs). `edit` works on a
    /// copy; when it fails or changes nothing, nothing is written. A change
    /// sets `updated_at`.
    ///
    /// The guide is read like [`Library::read`], so an older one is not
    /// migrated: it is refused as [`ReadOnlyReason::OlderSchema`]. Edits
    /// through one `Library` run one at a time.
    pub fn edit<T>(
        &self,
        id: Ulid,
        edit: impl FnOnce(&mut Guide) -> Result<T>,
    ) -> Result<Edited<T>> {
        let _edits = self.edits.lock().unwrap_or_else(|e| e.into_inner());
        let handle = self.find(id)?.ok_or(LibraryError::NotFound(id))?;
        self.check_inside(&handle.path)?;
        self.check_not_open(&handle)?;
        let (loaded, original) = self.open_guide(&handle.path)?;
        if loaded.read_only {
            return Err(EditError::ReadOnly {
                id,
                reason: ReadOnlyReason::NewerSchema,
            });
        }
        if loaded.migrated_from.is_some() {
            return Err(EditError::ReadOnly {
                id,
                reason: ReadOnlyReason::OlderSchema,
            });
        }
        let handle = loaded.handle;
        let mut guide = loaded.guide.clone();
        let value = edit(&mut guide)?;
        if guide == loaded.guide {
            return Ok(Edited {
                handle,
                guide,
                changed: false,
                value,
            });
        }
        guide.updated_at = Utc::now();
        self.check_not_open(&handle)?;
        let json = handle.guide_json();
        let current = fs::read(&json).map_err(|source| LibraryError::Io {
            path: json.clone(),
            source,
        })?;
        if current != original {
            return Err(LibraryError::ChangedOnDisk(json).into());
        }
        // The guide as it was goes into its history first, so the user can
        // take the edit back. Best effort: history never blocks an edit.
        if let Err(e) =
            history::snapshot(&handle, &loaded.guide, VersionReason::BeforeMcpEdit, None)
        {
            eprintln!("guiderecorder: cannot save a version of guide {id}: {e}");
        }
        self.save_to(&handle, &guide)?;
        Ok(Edited {
            handle,
            guide,
            changed: true,
            value,
        })
    }

    /// Marks the guide in `handle` open in this process's editor (see the
    /// module docs), replacing any lock there.
    pub fn lock_for_editor(&self, handle: &GuideHandle) -> crate::library::Result<()> {
        self.check_inside(&handle.path)?;
        let path = handle.path.join(EDITOR_LOCK_FILE);
        fs::write(&path, format!("{}\n", std::process::id()))
            .map_err(|source| LibraryError::Io { path, source })
    }

    /// Removes this process's editor lock from `handle`'s bundle; a lock of
    /// another process is left alone. A missing lock is fine.
    pub fn unlock_for_editor(&self, handle: &GuideHandle) -> crate::library::Result<()> {
        self.check_inside(&handle.path)?;
        let path = handle.path.join(EDITOR_LOCK_FILE);
        if read_lock(&handle.path) != Some(std::process::id()) {
            return Ok(());
        }
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(LibraryError::Io { path, source }),
        }
    }

    /// The process holding the editor lock of `handle`'s bundle, if it is
    /// still running.
    pub fn editor_lock_holder(&self, handle: &GuideHandle) -> Option<u32> {
        read_lock(&handle.path).filter(|pid| process_is_running(*pid))
    }

    fn check_not_open(&self, handle: &GuideHandle) -> Result<()> {
        match self.editor_lock_holder(handle) {
            Some(pid) => Err(EditError::OpenInEditor { id: handle.id, pid }),
            None => Ok(()),
        }
    }
}

/// The pid in a bundle's lock file; `None` if there is none or it is not
/// one (a lock being written is a few bytes, so that is a stray file).
fn read_lock(bundle: &std::path::Path) -> Option<u32> {
    let text = fs::read_to_string(bundle.join(EDITOR_LOCK_FILE)).ok()?;
    text.trim().parse().ok().filter(|pid| *pid > 0)
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // Signal 0 only checks: EPERM means it runs as another user.
    // SAFETY: `kill` with signal 0 sends nothing and touches no memory.
    let sent = unsafe { libc::kill(pid, 0) } == 0;
    sent || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use std::ffi::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: i32 = 5;

    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    // SAFETY: plain Win32 calls; the handle is checked, only read through
    // `GetExitCodeProcess` into a local, and closed once.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // Access denied means it runs, as another user or elevated.
            return io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED);
        }
        let mut code = 0;
        let read = GetExitCodeProcess(process, &mut code) != 0;
        CloseHandle(process);
        // An exited process keeps its handle (and pid) while one is open.
        !read || code == STILL_ACTIVE
    }
}

/// Without a way to check, a lock always counts.
#[cfg(not(any(unix, windows)))]
fn process_is_running(_pid: u32) -> bool {
    true
}

// ---------------------------------------------------------------------------
// Edits of a guide in memory

/// New values for guide-level fields; `None` leaves a field as it is.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuideFields {
    pub title: Option<String>,
    pub intro: Option<String>,
    pub prerequisites: Option<String>,
    pub summary: Option<String>,
    pub tags: Option<Vec<String>>,
}

/// Sets guide-level fields and returns the ones that changed, each marked
/// edited (see the module docs). The title is trimmed and may not be empty;
/// tags are cleaned up as [`add_tags`] does.
pub fn set_guide_fields(guide: &mut Guide, fields: GuideFields) -> Result<Vec<GuideField>> {
    let title = match fields.title {
        Some(title) if title.trim().is_empty() => {
            return Err(EditError::Invalid("the guide title cannot be empty".into()))
        }
        title => title.map(|t| t.trim().to_owned()),
    };
    let mut changed = Vec::new();
    let mut set = |field: GuideField, slot: &mut String, value: Option<String>| {
        if let Some(value) = value.filter(|v| v != slot) {
            *slot = value;
            changed.push(field);
        }
    };
    set(GuideField::Title, &mut guide.title, title);
    set(GuideField::Intro, &mut guide.intro, fields.intro);
    set(
        GuideField::Prerequisites,
        &mut guide.prerequisites,
        fields.prerequisites,
    );
    set(GuideField::Summary, &mut guide.summary, fields.summary);
    if let Some(tags) = fields.tags {
        let tags = add_tags(&[], &tags);
        if tags != guide.tags {
            guide.tags = tags;
            changed.push(GuideField::Tags);
        }
    }
    for field in &changed {
        mark_field_edited(guide, *field);
    }
    Ok(changed)
}

/// Marks `field` written by the user, as the editor's `setGuideField`
/// does: it joins the fields already edited (all of them in a guide from
/// before per-field tracking), and the guide's `ai_state` becomes `edited`.
pub fn mark_field_edited(guide: &mut Guide, field: GuideField) {
    let edited: Vec<GuideField> = GuideField::ALL
        .into_iter()
        .filter(|f| *f == field || guide.is_field_edited(*f))
        .collect();
    guide.edited_fields = edited;
    guide.ai_state.status = AiStatus::Edited;
}

/// `tags` with `new` added at the end, as the editor adds typed tags: each
/// entry split on commas and line breaks, trimmed, with runs of whitespace
/// made one space. Empty tags and ones already there (ignoring case) are
/// dropped.
pub fn add_tags(tags: &[String], new: &[String]) -> Vec<String> {
    let mut out = tags.to_vec();
    for part in new.iter().flat_map(|t| t.split([',', '\n'])) {
        let tag = part.split_whitespace().collect::<Vec<_>>().join(" ");
        if !tag.is_empty() && !out.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            out.push(tag);
        }
    }
    out
}

/// Adds tags to the guide (see [`add_tags`]), marking them edited if any
/// was new; returns the ones added.
pub fn add_guide_tags(guide: &mut Guide, new: &[String]) -> Vec<String> {
    let tags = add_tags(&guide.tags, new);
    let added = tags[guide.tags.len()..].to_vec();
    if !added.is_empty() {
        guide.tags = tags;
        mark_field_edited(guide, GuideField::Tags);
    }
    added
}

/// Removes tags from the guide (ignoring case and surrounding whitespace),
/// marking them edited if any was there; returns the ones removed.
pub fn remove_guide_tags(guide: &mut Guide, remove: &[String]) -> Vec<String> {
    let wanted: Vec<String> = remove.iter().map(|t| t.trim().to_lowercase()).collect();
    let (removed, kept): (Vec<String>, Vec<String>) = guide
        .tags
        .drain(..)
        .partition(|t| wanted.contains(&t.to_lowercase()));
    guide.tags = kept;
    if !removed.is_empty() {
        mark_field_edited(guide, GuideField::Tags);
    }
    removed
}

/// Changes the library makes to many guides at once (the app's bulk
/// actions): a status, an owner, tags to add and tags to remove. Unset
/// fields are left as they are.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DetailsEdit {
    pub status: Option<GuideStatus>,
    /// The new owner, trimmed; empty clears it.
    pub owner: Option<String>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
}

/// Applies `edit` to the guide (tags as [`add_guide_tags`] and
/// [`remove_guide_tags`], removals first). Returns whether anything changed.
pub fn apply_details(guide: &mut Guide, edit: &DetailsEdit) -> bool {
    let before = (
        guide.review.status,
        guide.review.owner.clone(),
        guide.tags.clone(),
    );
    if let Some(status) = edit.status {
        guide.review.status = Some(status);
    }
    if let Some(owner) = &edit.owner {
        let owner = owner.trim();
        guide.review.owner = (!owner.is_empty()).then(|| owner.to_owned());
    }
    remove_guide_tags(guide, &edit.remove_tags);
    add_guide_tags(guide, &edit.add_tags);
    before
        != (
            guide.review.status,
            guide.review.owner.clone(),
            guide.tags.clone(),
        )
}

/// Sets a step's title and/or description (Markdown). Returns whether
/// either changed; if so the step is marked `edited`.
pub fn set_step_text(
    guide: &mut Guide,
    step_id: Ulid,
    title: Option<String>,
    description: Option<String>,
) -> Result<bool> {
    let guide_id = guide.id;
    let item = guide
        .items
        .iter_mut()
        .find(|item| item.id() == step_id)
        .ok_or_else(|| EditError::Invalid(format!("guide {guide_id} has no step {step_id}")))?;
    let Item::Step(step) = item else {
        let what = match item {
            Item::Section(_) => "section",
            _ => "note",
        };
        return Err(EditError::Invalid(format!(
            "{step_id} is a {what} of guide {guide_id}, not a step"
        )));
    };
    let mut changed = false;
    for (slot, value) in [
        (&mut step.title, title),
        (&mut step.description, description),
    ] {
        if let Some(value) = value.filter(|v| v != slot) {
            *slot = value;
            changed = true;
        }
    }
    if changed {
        step.ai_state.status = AiStatus::Edited;
    }
    Ok(changed)
}

/// Where a new item goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Start,
    End,
    Before(Ulid),
    After(Ulid),
}

/// Inserts a note with `body` (Markdown, not blank) at `position` and
/// returns its id.
pub fn insert_note(
    guide: &mut Guide,
    kind: NoteKind,
    body: String,
    position: Position,
) -> Result<Ulid> {
    if body.trim().is_empty() {
        return Err(EditError::Invalid("the note text cannot be empty".into()));
    }
    let index_of = |id: Ulid| {
        guide
            .items
            .iter()
            .position(|item| item.id() == id)
            .ok_or_else(|| EditError::Invalid(format!("guide {} has no item {id}", guide.id)))
    };
    let index = match position {
        Position::Start => 0,
        Position::End => guide.items.len(),
        Position::Before(id) => index_of(id)?,
        Position::After(id) => index_of(id)? + 1,
    };
    let id = Ulid::new();
    guide
        .items
        .insert(index, Item::Note(Note { id, kind, body }));
    Ok(id)
}

/// Puts the guide's items in the order of `order`, which must list every
/// item id exactly once.
pub fn reorder_items(guide: &mut Guide, order: &[Ulid]) -> Result<()> {
    let invalid = |why: String| {
        Err(EditError::Invalid(format!(
            "{why}; pass every item id of guide {} exactly once (get_guide shows them as \
             {{#step-…}}, {{#section-…}} and {{#note-…}} anchors)",
            guide.id
        )))
    };
    if order.len() != guide.items.len() {
        return invalid(format!(
            "{} ids given for {} items",
            order.len(),
            guide.items.len()
        ));
    }
    let mut seen = HashSet::new();
    for id in order {
        if !seen.insert(*id) {
            return invalid(format!("{id} is listed twice"));
        }
        if !guide.items.iter().any(|item| item.id() == *id) {
            return invalid(format!("the guide has no item {id}"));
        }
    }
    let mut items = std::mem::take(&mut guide.items);
    guide.items = order
        .iter()
        .map(|id| {
            let i = items
                .iter()
                .position(|item| item.id() == *id)
                .expect("checked above");
            items.swap_remove(i)
        })
        .collect();
    Ok(())
}

/// Makes step `step_id` the guide's cover, or clears the choice (`None`:
/// the first screenshot is the cover again), and sets the cover options
/// given. Returns whether anything changed. The step must have a
/// screenshot.
pub fn set_cover(
    guide: &mut Guide,
    step_id: Option<Ulid>,
    options: Option<CoverOptions>,
) -> Result<bool> {
    if let Some(step_id) = step_id {
        let guide_id = guide.id;
        let item = guide
            .items
            .iter()
            .find(|item| item.id() == step_id)
            .ok_or_else(|| EditError::Invalid(format!("guide {guide_id} has no step {step_id}")))?;
        let problem = match item {
            Item::Step(step) if step.image.full.is_empty() => {
                format!("step {step_id} of guide {guide_id} has no screenshot")
            }
            Item::Step(_) => String::new(),
            Item::Section(_) => format!("{step_id} is a section of guide {guide_id}, not a step"),
            Item::Note(_) => format!("{step_id} is a note of guide {guide_id}, not a step"),
        };
        if !problem.is_empty() {
            return Err(EditError::Invalid(problem));
        }
    }
    let before = (guide.cover_step_id, guide.cover_options);
    guide.cover_step_id = step_id;
    if let Some(options) = options {
        guide.cover_options = options;
    }
    Ok((guide.cover_step_id, guide.cover_options) != before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AiState, ImageShow, Section};
    use crate::SCHEMA_VERSION;

    /// The (version 1) test guide, as a current-version guide.
    fn sample_guide() -> Guide {
        let mut guide: Guide =
            serde_json::from_str(include_str!("../testdata/guide.json")).unwrap();
        guide.schema_version = SCHEMA_VERSION;
        guide
    }

    fn ids(guide: &Guide) -> Vec<Ulid> {
        guide.items.iter().map(Item::id).collect()
    }

    fn first_step(guide: &Guide) -> Ulid {
        guide.steps().next().unwrap().id
    }

    #[test]
    fn guide_fields_mark_only_what_changed() {
        let mut guide = sample_guide();
        guide.ai_state = AiState {
            status: AiStatus::Ai,
            model: Some("m".into()),
            ..AiState::default()
        };
        let summary = guide.summary.clone();
        let changed = set_guide_fields(
            &mut guide,
            GuideFields {
                title: Some("  New title ".into()),
                summary: Some(summary),
                tags: Some(vec!["a, b".into(), "A".into(), "  c  d ".into()]),
                ..GuideFields::default()
            },
        )
        .unwrap();
        assert_eq!(changed, [GuideField::Title, GuideField::Tags]);
        assert_eq!(guide.title, "New title");
        assert_eq!(guide.tags, ["a", "b", "c d"]);
        assert_eq!(guide.edited_fields, [GuideField::Title, GuideField::Tags]);
        assert_eq!(guide.ai_state.status, AiStatus::Edited);
        assert_eq!(guide.ai_state.model.as_deref(), Some("m"));

        // Joins the fields already edited, in field order.
        set_guide_fields(
            &mut guide,
            GuideFields {
                intro: Some("Hi".into()),
                ..GuideFields::default()
            },
        )
        .unwrap();
        assert_eq!(
            guide.edited_fields,
            [GuideField::Title, GuideField::Intro, GuideField::Tags]
        );

        let err = set_guide_fields(
            &mut guide,
            GuideFields {
                title: Some(" ".into()),
                ..GuideFields::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, EditError::Invalid(_)), "{err}");
    }

    #[test]
    fn unchanged_fields_mark_nothing() {
        let mut guide = sample_guide();
        let before = guide.clone();
        let same = GuideFields {
            title: Some(guide.title.clone()),
            tags: Some(guide.tags.clone()),
            ..GuideFields::default()
        };
        let changed = set_guide_fields(&mut guide, same).unwrap();
        assert!(changed.is_empty());
        assert_eq!(guide, before);
        assert!(add_guide_tags(&mut guide, &["PDF".into(), " ".into()]).is_empty());
        assert!(remove_guide_tags(&mut guide, &["nope".into()]).is_empty());
        assert_eq!(guide, before);
    }

    #[test]
    fn a_legacy_edited_guide_keeps_every_field_edited() {
        let mut guide = sample_guide();
        guide.ai_state.status = AiStatus::Edited;
        guide.edited_fields.clear();
        mark_field_edited(&mut guide, GuideField::Summary);
        assert_eq!(guide.edited_fields, GuideField::ALL);
    }

    #[test]
    fn tags_are_added_and_removed_ignoring_case() {
        let mut guide = sample_guide();
        assert_eq!(guide.tags, ["reports", "pdf"]);
        let added = add_guide_tags(&mut guide, &["Finance\nQ3".into(), "REPORTS".into()]);
        assert_eq!(added, ["Finance", "Q3"]);
        assert_eq!(guide.tags, ["reports", "pdf", "Finance", "Q3"]);
        assert_eq!(guide.edited_fields, [GuideField::Tags]);
        let removed = remove_guide_tags(&mut guide, &[" PDF ".into(), "q3".into()]);
        assert_eq!(removed, ["pdf", "Q3"]);
        assert_eq!(guide.tags, ["reports", "Finance"]);
    }

    #[test]
    fn details_set_status_owner_and_tags() {
        let mut guide = sample_guide();
        let before = guide.clone();
        assert!(!apply_details(&mut guide, &DetailsEdit::default()));
        assert_eq!(guide, before);

        let edit = DetailsEdit {
            status: Some(GuideStatus::Published),
            owner: Some("  IT team ".into()),
            add_tags: vec!["Onboarding".into(), "PDF".into()],
            remove_tags: vec!["pdf".into()],
        };
        assert!(apply_details(&mut guide, &edit));
        assert_eq!(guide.review.status, Some(GuideStatus::Published));
        assert_eq!(guide.review.owner.as_deref(), Some("IT team"));
        // Removals go first, so a tag both removed and added stays (as added).
        assert_eq!(guide.tags, ["reports", "Onboarding", "PDF"]);
        assert_eq!(guide.edited_fields, [GuideField::Tags]);
        let same_owner = DetailsEdit {
            owner: Some("IT team".into()),
            ..DetailsEdit::default()
        };
        assert!(!apply_details(&mut guide, &same_owner));

        let clear = DetailsEdit {
            owner: Some(" ".into()),
            ..DetailsEdit::default()
        };
        assert!(apply_details(&mut guide, &clear));
        assert_eq!(guide.review.owner, None);

        let json: DetailsEdit =
            serde_json::from_str(r#"{"status":"needsUpdate","addTags":["a"]}"#).unwrap();
        assert_eq!(json.status, Some(GuideStatus::NeedsUpdate));
        assert_eq!(json.add_tags, ["a"]);
        assert_eq!(json.owner, None);
    }

    #[test]
    fn step_text_marks_the_step_edited() {
        let mut guide = sample_guide();
        let id = first_step(&guide);
        let title = guide.steps().next().unwrap().title.clone();
        assert!(!set_step_text(&mut guide, id, Some(title), None).unwrap());
        assert_ne!(
            guide.steps().next().unwrap().ai_state.status,
            AiStatus::Edited
        );
        assert!(set_step_text(&mut guide, id, None, Some("Then **save**.".into())).unwrap());
        let step = guide.steps().next().unwrap();
        assert_eq!(step.description, "Then **save**.");
        assert_eq!(step.ai_state.status, AiStatus::Edited);
        // The guide-level marker is the guide's, not the step's.
        assert!(guide.edited_fields.is_empty());

        let section = Ulid::new();
        guide.items.push(Item::Section(Section {
            id: section,
            title: "S".into(),
        }));
        let err = set_step_text(&mut guide, section, Some("x".into()), None).unwrap_err();
        assert!(err.to_string().contains("is a section"), "{err}");
        let err = set_step_text(&mut guide, Ulid::nil(), Some("x".into()), None).unwrap_err();
        assert!(err.to_string().contains("has no step"), "{err}");
    }

    #[test]
    fn notes_go_where_asked() {
        let mut guide = sample_guide();
        let before = ids(&guide);
        let first =
            insert_note(&mut guide, NoteKind::Tip, "First".into(), Position::Start).unwrap();
        let last = insert_note(&mut guide, NoteKind::Note, "Last".into(), Position::End).unwrap();
        let after = insert_note(
            &mut guide,
            NoteKind::Warning,
            "After".into(),
            Position::After(before[0]),
        )
        .unwrap();
        let ahead = insert_note(
            &mut guide,
            NoteKind::Tip,
            "Before".into(),
            Position::Before(before[0]),
        )
        .unwrap();
        let mut expected = vec![first, ahead, before[0], after];
        expected.extend(&before[1..]);
        expected.push(last);
        assert_eq!(ids(&guide), expected);
        assert!(matches!(
            &guide.items[3],
            Item::Note(Note { kind: NoteKind::Warning, body, .. }) if body == "After"
        ));
        assert!(insert_note(&mut guide, NoteKind::Tip, " \n".into(), Position::End).is_err());
        assert!(insert_note(
            &mut guide,
            NoteKind::Tip,
            "x".into(),
            Position::After(Ulid::nil())
        )
        .is_err());
        assert_eq!(guide.items.len(), expected.len());
    }

    #[test]
    fn cover_is_set_on_steps_with_a_screenshot_and_cleared() {
        let mut guide = sample_guide();
        let step = guide.steps().nth(1).unwrap().id;
        assert!(set_cover(&mut guide, Some(step), None).unwrap());
        assert_eq!(guide.cover_step_id, Some(step));
        assert_eq!(guide.cover_options, CoverOptions::default());
        // The same again changes nothing.
        assert!(!set_cover(&mut guide, Some(step), None).unwrap());
        let options = CoverOptions {
            image: ImageShow::Crop,
            annotations: true,
        };
        assert!(set_cover(&mut guide, Some(step), Some(options)).unwrap());
        assert_eq!(guide.cover_options, options);
        // Clearing keeps the options for the next choice.
        assert!(set_cover(&mut guide, None, None).unwrap());
        assert_eq!((guide.cover_step_id, guide.cover_options), (None, options));

        let section = guide.items[0].id();
        let err = set_cover(&mut guide, Some(section), None).unwrap_err();
        assert!(err.to_string().contains("is a section"), "{err}");
        let err = set_cover(&mut guide, Some(Ulid::nil()), None).unwrap_err();
        assert!(err.to_string().contains("has no step"), "{err}");
        if let Item::Step(s) = &mut guide.items[1] {
            s.image.full.clear();
        }
        let blank = guide.items[1].id();
        let err = set_cover(&mut guide, Some(blank), None).unwrap_err();
        assert!(err.to_string().contains("no screenshot"), "{err}");
        assert_eq!(guide.cover_step_id, None);
    }

    #[test]
    fn reorder_needs_every_item_once() {
        let mut guide = sample_guide();
        guide.items.push(Item::Section(Section {
            id: Ulid::new(),
            title: "S".into(),
        }));
        let before = ids(&guide);
        assert!(before.len() >= 3);
        let mut reversed = before.clone();
        reversed.reverse();
        reorder_items(&mut guide, &reversed).unwrap();
        assert_eq!(ids(&guide), reversed);
        reorder_items(&mut guide, &before).unwrap();

        let original = guide.clone();
        let mut twice = reversed.clone();
        twice[1] = twice[0];
        let err = reorder_items(&mut guide, &twice).unwrap_err();
        assert!(err.to_string().contains("listed twice"), "{err}");
        assert_eq!(guide, original);
        let mut unknown = reversed.clone();
        unknown[2] = Ulid::nil();
        let err = reorder_items(&mut guide, &unknown).unwrap_err();
        assert!(err.to_string().contains("no item"), "{err}");
        assert_eq!(guide, original);
        let err = reorder_items(&mut guide, &reversed[1..]).unwrap_err();
        assert!(err.to_string().contains("ids given"), "{err}");
        assert_eq!(guide, original);
    }

    fn library_with_sample() -> (tempfile::TempDir, Library, Guide) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::open(dir.path().join("Library")).unwrap();
        let guide = sample_guide();
        library.create(&guide).unwrap();
        (dir, library, guide)
    }

    #[test]
    fn edit_saves_changes_and_skips_no_ops() {
        let (_dir, library, guide) = library_with_sample();
        let handle = library.find(guide.id).unwrap().unwrap();
        let bytes = fs::read(handle.guide_json()).unwrap();
        let edited = library
            .edit(guide.id, |g| {
                Ok(add_guide_tags(g, &["reports".into()]).is_empty())
            })
            .unwrap();
        assert!(!edited.changed && edited.value);
        assert_eq!(fs::read(handle.guide_json()).unwrap(), bytes);

        let edited = library
            .edit(guide.id, |g| Ok(add_guide_tags(g, &["new".into()])))
            .unwrap();
        assert!(edited.changed);
        assert!(edited.guide.updated_at > guide.updated_at);
        let saved = library.read(&handle.path).unwrap().guide;
        assert_eq!(saved, edited.guide);
        assert_eq!(saved.tags, ["reports", "pdf", "new"]);
        // Only the change saved a version: the guide as it was before it.
        let versions = history::list(&handle).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].reason, VersionReason::BeforeMcpEdit);
        let before = history::read(&library, &handle, versions[0].id).unwrap();
        assert_eq!(before.guide.tags, ["reports", "pdf"]);

        // A failed edit writes nothing.
        let bytes = fs::read(handle.guide_json()).unwrap();
        let err = library
            .edit(guide.id, |g| {
                g.title = "Changed".into();
                reorder_items(g, &[])
            })
            .unwrap_err();
        assert!(matches!(err, EditError::Invalid(_)));
        assert_eq!(fs::read(handle.guide_json()).unwrap(), bytes);

        let err = library.edit(Ulid::nil(), |_| Ok(())).unwrap_err();
        assert!(matches!(err, EditError::Library(LibraryError::NotFound(_))));
    }

    #[test]
    fn edit_refuses_read_only_guides() {
        let (_dir, library, guide) = library_with_sample();
        let handle = library.find(guide.id).unwrap().unwrap();
        let json = handle.guide_json();
        let original = fs::read(&json).unwrap();
        for (version, reason) in [
            (SCHEMA_VERSION + 1, ReadOnlyReason::NewerSchema),
            (1, ReadOnlyReason::OlderSchema),
        ] {
            let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
            value["schemaVersion"] = version.into();
            let bytes = serde_json::to_vec_pretty(&value).unwrap();
            fs::write(&json, &bytes).unwrap();
            let err = library
                .edit(guide.id, |g| Ok(add_guide_tags(g, &["x".into()])))
                .unwrap_err();
            assert!(
                matches!(err, EditError::ReadOnly { reason: r, .. } if r == reason),
                "{err}"
            );
            assert_eq!(fs::read(&json).unwrap(), bytes);
        }
    }

    #[test]
    fn the_editor_lock_blocks_edits_while_its_process_runs() {
        let (_dir, library, guide) = library_with_sample();
        let handle = library.find(guide.id).unwrap().unwrap();
        let lock = handle.path.join(EDITOR_LOCK_FILE);
        library.lock_for_editor(&handle).unwrap();
        assert_eq!(
            fs::read_to_string(&lock).unwrap(),
            format!("{}\n", std::process::id())
        );
        assert_eq!(
            library.editor_lock_holder(&handle),
            Some(std::process::id())
        );
        let bytes = fs::read(handle.guide_json()).unwrap();
        let err = library
            .edit(guide.id, |g| Ok(add_guide_tags(g, &["x".into()])))
            .unwrap_err();
        assert!(
            matches!(err, EditError::OpenInEditor { pid, .. } if pid == std::process::id()),
            "{err}"
        );
        assert!(err.to_string().contains("open in the GuideRecorder editor"));
        assert_eq!(fs::read(handle.guide_json()).unwrap(), bytes);
        // The lock stays out of listings.
        assert_eq!(library.list().unwrap().len(), 1);

        library.unlock_for_editor(&handle).unwrap();
        assert!(!lock.exists());
        library.unlock_for_editor(&handle).unwrap();
        assert!(
            library
                .edit(guide.id, |g| Ok(add_guide_tags(g, &["x".into()])))
                .unwrap()
                .changed
        );
    }

    #[test]
    fn stale_and_foreign_locks() {
        let (_dir, library, guide) = library_with_sample();
        let handle = library.find(guide.id).unwrap().unwrap();
        let lock = handle.path.join(EDITOR_LOCK_FILE);
        // A process that has exited: its pid is free.
        #[cfg(unix)]
        let mut child = std::process::Command::new("true").spawn().unwrap();
        #[cfg(windows)]
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .unwrap();
        let dead = child.id();
        child.wait().unwrap();
        for stale in [format!("{dead}\n"), "garbage".into(), "0\n".into()] {
            fs::write(&lock, stale).unwrap();
            assert_eq!(library.editor_lock_holder(&handle), None);
        }
        assert!(
            library
                .edit(guide.id, |g| Ok(add_guide_tags(g, &["x".into()])))
                .unwrap()
                .changed
        );
        // Another process's lock is not ours to remove.
        fs::write(&lock, format!("{dead}\n")).unwrap();
        library.unlock_for_editor(&handle).unwrap();
        assert!(lock.exists());
    }
}
