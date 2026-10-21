//! Conditional content: decisions and section audiences.
//!
//! A [`Decision`](crate::model::Decision) asks the reader a question whose
//! options lead to a step, a section or another guide. A
//! [`Section`](crate::model::Section) may name the audiences it is for
//! (`Admin`, `User`, ...); it covers the items up to the next section.
//! Items before the first section, and untagged sections, are for
//! everyone.
//!
//! [`for_audience`] narrows a guide to one audience before it is exported.
//! Audiences are free text, compared without case and surrounding space.

use std::collections::HashSet;

use ulid::Ulid;

use crate::model::{BranchTarget, Guide, Item};

/// The distinct audiences the guide's sections name, in order of first
/// use, as first written.
pub fn audiences(guide: &Guide) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for item in &guide.items {
        if let Item::Section(section) = item {
            for audience in &section.audiences {
                let audience = audience.trim();
                if !audience.is_empty() && seen.insert(audience.to_lowercase()) {
                    out.push(audience.to_owned());
                }
            }
        }
    }
    out
}

/// Whether a section tagged with `tags` is for `audience`: untagged
/// sections are for everyone.
pub fn is_for(tags: &[String], audience: &str) -> bool {
    let audience = audience.trim();
    tags.iter().all(|tag| tag.trim().is_empty())
        || tags
            .iter()
            .any(|tag| tag.trim().to_lowercase() == audience.to_lowercase())
}

/// `guide` as `audience` reads it: sections tagged for other audiences
/// only are left out with their items, and decision options leading into
/// them are dropped. A decision that loses every option goes too.
pub fn for_audience(guide: &Guide, audience: &str) -> Guide {
    let mut kept = Vec::with_capacity(guide.items.len());
    let mut removed = HashSet::new();
    let mut included = true;
    for item in &guide.items {
        if let Item::Section(section) = item {
            included = is_for(&section.audiences, audience);
        }
        if included {
            kept.push(item.clone());
        } else {
            removed.insert(item.id());
        }
    }
    if !removed.is_empty() {
        kept.retain_mut(|item| {
            let Item::Decision(decision) = item else {
                return true;
            };
            let before = decision.options.len();
            decision
                .options
                .retain(|option| !leads_into(&option.target, &removed));
            before == 0 || !decision.options.is_empty()
        });
    }
    Guide {
        items: kept,
        ..guide.clone()
    }
}

/// [`for_audience`] when an audience is given (and not blank), else the
/// whole guide: what exporters are handed.
pub fn narrow(guide: Guide, audience: Option<&str>) -> Guide {
    match audience.map(str::trim).filter(|a| !a.is_empty()) {
        Some(audience) => for_audience(&guide, audience),
        None => guide,
    }
}

fn leads_into(target: &BranchTarget, ids: &HashSet<Ulid>) -> bool {
    match target {
        BranchTarget::Step { id } | BranchTarget::Section { id } => ids.contains(id),
        BranchTarget::Next | BranchTarget::Guide { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Decision, DecisionOption, Note, NoteKind, Section};

    fn section(title: &str, audiences: &[&str]) -> Item {
        Item::Section(Section {
            id: Ulid::new(),
            title: title.into(),
            audiences: audiences.iter().map(|a| (*a).to_owned()).collect(),
        })
    }

    fn note(body: &str) -> Item {
        Item::Note(Note {
            id: Ulid::new(),
            kind: NoteKind::Note,
            body: body.into(),
        })
    }

    fn decision(targets: Vec<BranchTarget>) -> Item {
        Item::Decision(Decision {
            id: Ulid::new(),
            question: "Who are you?".into(),
            options: targets
                .into_iter()
                .map(|target| DecisionOption {
                    id: Ulid::new(),
                    label: "x".into(),
                    target,
                })
                .collect(),
        })
    }

    fn bodies(guide: &Guide) -> Vec<String> {
        guide
            .items
            .iter()
            .map(|item| match item {
                Item::Section(s) => format!("# {}", s.title),
                Item::Note(n) => n.body.clone(),
                Item::Decision(d) => format!("? {}", d.options.len()),
                Item::Step(s) => s.title.clone(),
            })
            .collect()
    }

    #[test]
    fn lists_audiences_once_in_order() {
        let mut guide = Guide::new("T", "en");
        guide.items = vec![
            section("A", &["Admin", " user "]),
            section("B", &[]),
            section("C", &["ADMIN", "Guest", ""]),
        ];
        assert_eq!(audiences(&guide), ["Admin", "user", "Guest"]);
    }

    #[test]
    fn untagged_sections_are_for_everyone() {
        assert!(is_for(&[], "Admin"));
        assert!(is_for(&[" ".into()], "Admin"));
        assert!(is_for(&["admin".into()], " Admin "));
        assert!(!is_for(&["User".into()], "Admin"));
    }

    #[test]
    fn leaves_out_other_audiences_and_options_leading_there() {
        let mut guide = Guide::new("T", "en");
        let admin = section("Admins", &["Admin"]);
        let user = section("Users", &["User"]);
        let user_note = note("user only");
        let (admin_id, user_id, user_note_id) = (admin.id(), user.id(), user_note.id());
        guide.items = vec![
            note("everyone"),
            decision(vec![
                BranchTarget::Section { id: admin_id },
                BranchTarget::Section { id: user_id },
            ]),
            decision(vec![BranchTarget::Step { id: user_note_id }]),
            decision(Vec::new()),
            admin,
            note("admin only"),
            user,
            user_note,
            section("All", &[]),
            note("everyone again"),
        ];
        let admin_view = for_audience(&guide, "admin");
        assert_eq!(
            bodies(&admin_view),
            [
                "everyone",
                "? 1",
                "? 0",
                "# Admins",
                "admin only",
                "# All",
                "everyone again"
            ]
        );
        let user_view = for_audience(&guide, "User");
        assert_eq!(
            bodies(&user_view),
            [
                "everyone",
                "? 1",
                "? 1",
                "? 0",
                "# Users",
                "user only",
                "# All",
                "everyone again"
            ]
        );
        // Nothing tagged for the audience: only the shared parts.
        assert_eq!(
            bodies(&for_audience(&guide, "Guest")),
            ["everyone", "? 0", "# All", "everyone again"]
        );
    }
}
