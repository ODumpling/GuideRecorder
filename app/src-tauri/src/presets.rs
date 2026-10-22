//! Brand presets of the open library (core `export::preset`): listed,
//! saved, deleted and made the library's default from Settings and the
//! Export dialog.
//!
//! A preset's logo never crosses the IPC as a path the webview made up:
//! a new logo is a path the logo dialog returned ([`ChosenPaths`]), read
//! and checked here, then stored in the preset file as the checked PNG.
//! Listing sends each logo as that PNG's data URI, for display only.

use std::path::PathBuf;

use guiderecorder_core::export::md::Branding;
use guiderecorder_core::export::preset::{Preset, PresetError, PresetStore};
use serde::{Deserialize, Serialize};

use crate::editor::{AppError, ErrorKind, Result, Session};
use crate::export::{png_uri, read_logo, ChosenPaths};

/// A preset as Settings and the Export dialog show it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetView {
    pub id: String,
    /// The preset without its logo; `None` when the file can't be read.
    pub preset: Option<Preset>,
    /// The logo as a PNG data URI.
    pub logo: Option<String>,
    /// Why the file can't be read or used.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetList {
    pub presets: Vec<PresetView>,
    /// The library's default preset.
    pub default_id: Option<String>,
}

/// What happens to a preset's logo when it is saved.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LogoChange {
    /// Keep the saved preset's logo (none for a new preset).
    #[default]
    Keep,
    Remove,
    /// A file from the logo dialog.
    File {
        path: PathBuf,
    },
}

fn preset_error(e: PresetError) -> AppError {
    let kind = match e {
        PresetError::NotFound(_) => ErrorKind::NotFound,
        PresetError::Invalid(_) | PresetError::Newer(_) => ErrorKind::Invalid,
        _ => ErrorKind::Io,
    };
    AppError::new(kind, e.to_string())
}

fn store(session: &Session) -> PresetStore {
    PresetStore::new(session.root())
}

/// Every preset of the library, and its default.
pub fn list(session: &Session) -> Result<PresetList> {
    let store = store(session);
    let presets = store
        .list()
        .map_err(preset_error)?
        .into_iter()
        .map(|entry| match entry.preset {
            Ok(preset) => view(entry.id, preset),
            Err(e) => PresetView {
                id: entry.id,
                preset: None,
                logo: None,
                error: Some(e.to_string()),
            },
        })
        .collect();
    Ok(PresetList {
        presets,
        default_id: store.default_id().map_err(preset_error)?,
    })
}

/// The preset for the UI: its logo checked and shown as a data URI, or
/// the reason it can't be used.
fn view(id: String, mut preset: Preset) -> PresetView {
    let checked = preset
        .branding()
        .map_err(|e| e.to_string())
        .and_then(|b| b.check().map_err(|e| e.to_string()));
    preset.logo = None;
    match checked {
        Ok(branding) => PresetView {
            id,
            logo: branding.logo().map(|l| png_uri(l.png())),
            preset: Some(preset),
            error: None,
        },
        Err(error) => PresetView {
            id,
            preset: Some(preset),
            logo: None,
            error: Some(error),
        },
    }
}

/// Saves `preset` as `id` (or as a new preset), with its logo changed as
/// `logo` says; returns the id.
pub fn save(
    session: &Session,
    chosen: &ChosenPaths,
    id: Option<&str>,
    mut preset: Preset,
    logo: &LogoChange,
) -> Result<String> {
    let store = store(session);
    let bytes = match logo {
        LogoChange::Keep => match id {
            Some(id) => store
                .read(id)
                .map_err(preset_error)?
                .logo_bytes()
                .map_err(preset_error)?,
            None => None,
        },
        LogoChange::Remove => None,
        LogoChange::File { path } => {
            chosen.check_logo(path)?;
            let bytes = read_logo(path)?;
            // Stored as the checked, upright PNG without metadata.
            let branding = Branding {
                logo: Some(bytes),
                ..Branding::default()
            }
            .check()
            .map_err(|e| AppError::new(ErrorKind::Invalid, format!("logo: {e}")))?;
            branding.logo().map(|l| l.png().to_vec())
        }
    };
    preset.set_logo(bytes.as_deref());
    store.save(id, &preset).map_err(preset_error)
}

pub fn delete(session: &Session, id: &str) -> Result<()> {
    store(session).delete(id).map_err(preset_error)
}

pub fn set_default(session: &Session, id: Option<&str>) -> Result<()> {
    store(session).set_default(id).map_err(preset_error)
}

/// The branding of the library's preset `id`, unchecked.
pub fn branding(session: &Session, id: &str) -> Result<Branding> {
    store(session)
        .read(id)
        .and_then(|p| p.branding())
        .map_err(preset_error)
}
