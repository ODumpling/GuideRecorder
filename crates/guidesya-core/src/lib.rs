//! GuidesYa core: guide format, library, index, step detection, export and AI.
//!
//! See design scratchpads #444 (format), #443 (export) and #442 (AI).

/// Current `guide.json` schema version. Bumped together with a migration.
pub const SCHEMA_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_is_positive() {
        assert!(SCHEMA_VERSION >= 1);
    }
}
