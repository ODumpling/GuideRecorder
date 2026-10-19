//! The JSON Schema of `guide.json`, generated from [`model::Guide`] and
//! checked in at `packages/guide-schema/guide.schema.json` for the web stack.
//! Run with `UPDATE_GOLDEN=1` after changing the model, then regenerate the
//! TypeScript (`pnpm --filter @guiderecorder/guide-schema generate`).

mod support;

use guiderecorder_core::model::Guide;
use support::{check_golden_text, crate_dir, update_requested};

#[test]
fn guide_schema_is_current() {
    let schema = schemars::schema_for!(Guide);
    let text = serde_json::to_string_pretty(&schema).unwrap() + "\n";
    let path = crate_dir().join("../../packages/guide-schema/guide.schema.json");
    if let Err(message) = check_golden_text(&path, &text, update_requested()) {
        panic!("{message}");
    }
}
