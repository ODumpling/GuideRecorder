//! Shared result type for the format writers (md, html, docx, later pdf).

use std::path::PathBuf;

use ulid::Ulid;

use super::RenderWarning;

/// What a writer produced and anything that degraded along the way.
/// Writers never fail on a bad step image: they render the placeholder
/// and record the warning here.
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    /// Files written, in write order (the main document first).
    pub files: Vec<PathBuf>,
    /// Per-step image warnings, keyed by step id.
    pub warnings: Vec<(Ulid, RenderWarning)>,
}

impl ExportReport {
    /// Record every warning of one rendered step image.
    pub fn add_warnings(&mut self, step: Ulid, warnings: impl IntoIterator<Item = RenderWarning>) {
        self.warnings
            .extend(warnings.into_iter().map(|w| (step, w)));
    }
}
