//! Agent skill export (#15).
//!
//! Goldens are under `tests/golden/skill/`, `SKILL.md` and `steps.json`
//! per fixture guide; regenerate them with
//! `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core --test export_skill`
//! and review the diff.

mod support;

use std::fs;

use guiderecorder_core::export::skill::{self, SkillOptions, REPLAY_FILE, SKILL_FILE, STEPS_FILE};
use guiderecorder_core::export::RenderGuide;
use guiderecorder_core::library::Library;
use support::{assert_golden_text, copy_dir, fixture_guides_dir};

#[test]
fn fixture_goldens() {
    let dir = tempfile::tempdir().unwrap();
    for entry in fs::read_dir(fixture_guides_dir()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "guiderecorder") {
            copy_dir(&path, &dir.path().join(path.file_name().unwrap()));
        }
    }
    let library = Library::open(dir.path()).unwrap();
    let summaries = library.list().unwrap();
    assert!(!summaries.is_empty());
    for summary in summaries {
        let loaded = library.read(&summary.path).unwrap();
        let render = RenderGuide::new(&loaded.guide, &loaded.handle);
        let out = tempfile::tempdir().unwrap();
        let opts = SkillOptions {
            replay: true,
            overwrite: true,
            ..SkillOptions::default()
        };
        let report = skill::export(&render, &loaded.handle, out.path(), &opts).unwrap();
        assert_eq!(report.files[0], out.path().join(SKILL_FILE));
        let name = skill::skill_name(&loaded.guide.title);
        let skill_md = fs::read_to_string(out.path().join(SKILL_FILE)).unwrap();
        assert_golden_text(&format!("skill/{name}/SKILL.md.golden"), &skill_md);
        let steps = fs::read_to_string(out.path().join(STEPS_FILE)).unwrap();
        assert_golden_text(&format!("skill/{name}/steps.json.golden"), &steps);

        // Every image SKILL.md links was written.
        for line in skill_md.lines().filter(|l| l.starts_with("![")) {
            let link = line.split("](").nth(1).unwrap().trim_end_matches(')');
            assert!(out.path().join(link).is_file(), "{link}");
        }
        // The script carries the same steps.
        let script = fs::read_to_string(out.path().join(REPLAY_FILE)).unwrap();
        let data: serde_json::Value = serde_json::from_str(&steps).unwrap();
        let embedded = script
            .lines()
            .find_map(|l| l.strip_prefix("const GUIDE = "))
            .and_then(|l| l.strip_suffix(';'))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(embedded).unwrap(),
            data
        );
    }
}
