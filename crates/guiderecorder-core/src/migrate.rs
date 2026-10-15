//! `guide.json` schema migrations. See design scratchpad #444 ("Versioning").
//!
//! A migration is an ordered list of `vN → vN+1` steps over raw JSON
//! ([`serde_json::Value`]), run before the document is decoded into
//! [`crate::model::Guide`]. Everything here is pure: no IO. Backing up and
//! rewriting the file is [`crate::library::Library::load`]'s job.
//!
//! To change the format: bump [`crate::SCHEMA_VERSION`], add the step that
//! takes the previous version's JSON to the new one to [`MIGRATIONS`], and
//! keep a fixture of the old format with a test. The const assertion below
//! fails the build if the two get out of step.

use serde_json::Value;

use crate::SCHEMA_VERSION;

/// One `vN → vN+1` step. It gets the whole document at version `N` and
/// returns it at `N+1`; it does not need to touch `schemaVersion`, which
/// [`Schema::migrate`] sets after each step. An `Err` is a message for
/// [`MigrationError::Step`].
pub type Step = fn(Value) -> Result<Value, String>;

/// The real migrations: entry `i` takes version `i + 1` to `i + 2`.
/// Empty while [`SCHEMA_VERSION`] is 1.
pub const MIGRATIONS: &[Step] = &[];

const _: () = assert!(MIGRATIONS.len() as u32 + 1 == SCHEMA_VERSION);

/// The current schema: [`SCHEMA_VERSION`] reached through [`MIGRATIONS`].
pub const CURRENT: Schema = Schema {
    version: SCHEMA_VERSION,
    steps: MIGRATIONS,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// Versions start at 1.
    #[error("schema version {0} is not valid")]
    InvalidVersion(u32),
    /// The document is from a newer app; it cannot be migrated down.
    #[error("schema version {found} is newer than supported {supported}")]
    Newer { found: u32, supported: u32 },
    /// No step is registered for `from → from + 1` (a gap in the registry).
    #[error("no migration from schema version {from}")]
    Missing { from: u32 },
    /// A step failed, or returned something other than a JSON object.
    #[error("migrating schema version {from} to {}: {message}", from + 1)]
    Step { from: u32, message: String },
}

pub type Result<T, E = MigrationError> = std::result::Result<T, E>;

/// A target version and the steps that reach it. [`CURRENT`] is the real
/// one; tests build others to exercise the framework.
#[derive(Debug, Clone, Copy)]
pub struct Schema {
    /// The version [`Schema::migrate`] produces.
    pub version: u32,
    /// Entry `i` takes version `i + 1` to `i + 2`.
    pub steps: &'static [Step],
}

impl Schema {
    /// Migrates `value` from version `from` to [`Schema::version`], one
    /// step at a time, setting `schemaVersion` after each. Returns the
    /// migrated value and the versions it was migrated from, in order
    /// (`[1, 2]` for 1 → 3; empty if already current).
    pub fn migrate(&self, mut value: Value, from: u32) -> Result<(Value, Vec<u32>)> {
        if from == 0 {
            return Err(MigrationError::InvalidVersion(from));
        }
        if from > self.version {
            return Err(MigrationError::Newer {
                found: from,
                supported: self.version,
            });
        }
        let mut applied = Vec::new();
        for version in from..self.version {
            let step = self
                .steps
                .get(version as usize - 1)
                .ok_or(MigrationError::Missing { from: version })?;
            let step_err = |message: String| MigrationError::Step {
                from: version,
                message,
            };
            value = step(value).map_err(step_err)?;
            value
                .as_object_mut()
                .ok_or_else(|| step_err("result is not a JSON object".to_owned()))?
                .insert("schemaVersion".to_owned(), (version + 1).into());
            applied.push(version);
        }
        Ok((value, applied))
    }
}

/// Migrates `value` from version `from` to [`SCHEMA_VERSION`] with
/// [`MIGRATIONS`]. See [`Schema::migrate`].
pub fn migrate(value: Value, from: u32) -> Result<(Value, Vec<u32>)> {
    CURRENT.migrate(value, from)
}

/// A three-version test schema shared with the library tests. Version 1
/// called the summary `subtitle` and had no `tags`; version 2 renamed
/// `subtitle` to `summary`; version 3 added `tags`, stored as one
/// comma-separated `keywords` string in version 2.
#[cfg(test)]
pub(crate) mod test_schema {
    use super::*;

    pub const SCHEMA: Schema = Schema {
        version: 3,
        steps: &[v1_to_v2, v2_to_v3],
    };

    fn v1_to_v2(mut value: Value) -> Result<Value, String> {
        let object = value.as_object_mut().ok_or("not an object")?;
        if let Some(subtitle) = object.remove("subtitle") {
            object.insert("summary".to_owned(), subtitle);
        }
        object.insert("keywords".to_owned(), "".into());
        Ok(value)
    }

    fn v2_to_v3(mut value: Value) -> Result<Value, String> {
        let object = value.as_object_mut().ok_or("not an object")?;
        let keywords = match object.remove("keywords") {
            Some(Value::String(s)) => s,
            None => String::new(),
            Some(other) => return Err(format!("keywords is not a string: {other}")),
        };
        let tags: Vec<Value> = keywords
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(Value::from)
            .collect();
        object.insert("tags".to_owned(), tags.into());
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::test_schema::SCHEMA;
    use super::*;
    use serde_json::json;

    #[test]
    fn current_schema_has_nothing_to_migrate() {
        let value = json!({ "schemaVersion": SCHEMA_VERSION, "title": "T" });
        let (out, applied) = migrate(value.clone(), SCHEMA_VERSION).unwrap();
        assert_eq!(out, value);
        assert!(applied.is_empty());
        assert_eq!(
            migrate(value.clone(), SCHEMA_VERSION + 1),
            Err(MigrationError::Newer {
                found: SCHEMA_VERSION + 1,
                supported: SCHEMA_VERSION
            })
        );
        assert_eq!(migrate(value, 0), Err(MigrationError::InvalidVersion(0)));
    }

    #[test]
    fn migrates_through_every_step_in_order() {
        let v1 = json!({ "schemaVersion": 1, "title": "T", "subtitle": "S" });
        let (out, applied) = SCHEMA.migrate(v1, 1).unwrap();
        assert_eq!(applied, [1, 2]);
        assert_eq!(
            out,
            json!({ "schemaVersion": 3, "title": "T", "summary": "S", "tags": [] })
        );

        let v2 = json!({ "schemaVersion": 2, "summary": "S", "keywords": "a, b,,c " });
        let (out, applied) = SCHEMA.migrate(v2, 2).unwrap();
        assert_eq!(applied, [2]);
        assert_eq!(
            out,
            json!({ "schemaVersion": 3, "summary": "S", "tags": ["a", "b", "c"] })
        );

        let v3 = json!({ "schemaVersion": 3, "tags": ["x"] });
        assert_eq!(SCHEMA.migrate(v3.clone(), 3).unwrap(), (v3, vec![]));
    }

    #[test]
    fn step_failures_name_the_version() {
        let bad = json!({ "schemaVersion": 2, "keywords": 5 });
        assert!(matches!(
            SCHEMA.migrate(bad, 2),
            Err(MigrationError::Step { from: 2, .. })
        ));
        assert!(matches!(
            SCHEMA.migrate(json!([1, 2]), 1),
            Err(MigrationError::Step { from: 1, .. })
        ));

        fn to_array(_: Value) -> Result<Value, String> {
            Ok(json!([]))
        }
        let breaks_shape = Schema {
            version: 2,
            steps: &[to_array],
        };
        let err = breaks_shape.migrate(json!({}), 1).unwrap_err();
        assert_eq!(
            err,
            MigrationError::Step {
                from: 1,
                message: "result is not a JSON object".into()
            }
        );
        assert_eq!(
            err.to_string(),
            "migrating schema version 1 to 2: result is not a JSON object"
        );
    }

    #[test]
    fn gaps_in_the_registry_are_errors() {
        let short = Schema {
            version: 4,
            steps: SCHEMA.steps,
        };
        assert_eq!(
            short.migrate(json!({ "subtitle": "S" }), 1),
            Err(MigrationError::Missing { from: 3 })
        );
    }
}
