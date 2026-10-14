//! GuidesYa core: guide format, library, index, step detection, export and AI.
//!
//! See design scratchpads #444 (format), #443 (export) and #442 (AI).

pub mod ai;
pub mod events;
pub mod library;
pub mod markdown;
pub mod migrate;
pub mod model;
pub mod steps;

pub use ulid::Ulid;

/// Current `guide.json` schema version. Bumped together with a migration.
pub const SCHEMA_VERSION: u32 = 1;

const _: () = assert!(SCHEMA_VERSION >= 1);
