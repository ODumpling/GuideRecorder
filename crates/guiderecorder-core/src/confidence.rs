//! Element confidence: whether a step's recorded element is identified
//! clearly enough to find again, by an agent reading an exported skill or
//! by the replay script (see [`export::skill`](crate::export::skill)).
//!
//! A pure function of the step: [`element_issues`] lists what is missing
//! or vague, and a step with none is clear. The editor shows the same
//! rules (`app/src/editor/element.ts`, kept in step with this file) so
//! the author can fix a step before exporting.
//!
//! # Which steps need an element
//!
//! Clicks (single, double, right), drags and typing act on one element.
//! Shortcuts go to the frontmost app, and manual steps are done by hand,
//! so neither needs one and they are never flagged.
//!
//! # What counts as unclear
//!
//! - no element at all;
//! - no app (neither name nor bundle id);
//! - no window, except for elements that live outside windows (Dock
//!   items, menu bar items and menu items);
//! - no label (title or description), after trimming;
//! - no role, or a generic container role ([`GENERIC_ROLES`]) that says
//!   nothing about what the element is.

use serde::{Deserialize, Serialize};

use crate::model::{ElementInfo, Step, StepKind};

/// Roles that name a container or an unknown element rather than a
/// control: a step whose element has one can't be found by role. Roles are
/// AX role names on both platforms (the Windows backend maps UI Automation
/// control types onto them).
pub const GENERIC_ROLES: &[&str] = &[
    "AXGroup",
    "AXUnknown",
    "AXScrollArea",
    "AXSplitGroup",
    "AXLayoutArea",
    "AXLayoutItem",
    "AXWebArea",
    "AXWindow",
    "AXApplication",
];

/// Roles of elements that are not inside a window, so a missing window is
/// expected.
pub const WINDOWLESS_ROLES: &[&str] = &["AXDockItem", "AXMenuBarItem", "AXMenuItem"];

/// One reason a step's element is not identified clearly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ElementIssue {
    /// The step acts on an element, but none was recorded.
    NoElement,
    /// No app name or bundle id.
    NoApp,
    /// No window title.
    NoWindow,
    /// No title or description.
    NoLabel,
    /// No role.
    NoRole,
    /// A container or unknown role ([`GENERIC_ROLES`]).
    GenericRole,
}

impl ElementIssue {
    /// The name in `steps.json`, e.g. `noLabel`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoElement => "noElement",
            Self::NoApp => "noApp",
            Self::NoWindow => "noWindow",
            Self::NoLabel => "noLabel",
            Self::NoRole => "noRole",
            Self::GenericRole => "genericRole",
        }
    }

    /// A short phrase for people: "no label", "a generic role".
    pub fn describe(self) -> &'static str {
        match self {
            Self::NoElement => "no element was recorded",
            Self::NoApp => "no app",
            Self::NoWindow => "no window",
            Self::NoLabel => "no label",
            Self::NoRole => "no role",
            Self::GenericRole => "a generic role",
        }
    }
}

/// Whether a step of `kind` acts on one element (see the module docs).
pub fn needs_element(kind: StepKind) -> bool {
    matches!(
        kind,
        StepKind::Click
            | StepKind::DoubleClick
            | StepKind::RightClick
            | StepKind::Drag
            | StepKind::Type
    )
}

/// Why `step`'s element is not identified clearly, in a fixed order; empty
/// when it is, or when the step needs no element.
pub fn element_issues(step: &Step) -> Vec<ElementIssue> {
    if !needs_element(step.kind) {
        return Vec::new();
    }
    match &step.element {
        None => vec![ElementIssue::NoElement],
        Some(element) => locator_issues(element),
    }
}

/// [`element_issues`] for a recorded element, whatever the step kind.
pub fn locator_issues(element: &ElementInfo) -> Vec<ElementIssue> {
    let set = |value: &Option<String>| value.as_deref().is_some_and(|v| !v.trim().is_empty());
    let role = element
        .role
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty());
    let mut issues = Vec::new();
    if !set(&element.app) && !set(&element.bundle_id) {
        issues.push(ElementIssue::NoApp);
    }
    if !set(&element.window) && !role.is_some_and(|r| WINDOWLESS_ROLES.contains(&r)) {
        issues.push(ElementIssue::NoWindow);
    }
    if !set(&element.label) {
        issues.push(ElementIssue::NoLabel);
    }
    match role {
        None => issues.push(ElementIssue::NoRole),
        Some(role) if GENERIC_ROLES.contains(&role) => issues.push(ElementIssue::GenericRole),
        Some(_) => {}
    }
    issues
}

/// `issues` as one phrase: "no label and a generic role".
pub fn describe_issues(issues: &[ElementIssue]) -> String {
    let words: Vec<&str> = issues.iter().map(|i| i.describe()).collect();
    match words.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ImageShow, StepImage};
    use crate::Ulid;
    use ElementIssue::*;

    fn step(kind: StepKind, element: Option<ElementInfo>) -> Step {
        Step {
            id: Ulid::new(),
            title: "T".into(),
            description: String::new(),
            kind,
            point: None,
            end_point: None,
            element,
            keystrokes: None,
            image: StepImage {
                full: "steps/a.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: ImageShow::Full,
            },
            annotations: Vec::new(),
            narration: String::new(),
            ai_state: Default::default(),
        }
    }

    fn element(
        role: Option<&str>,
        label: Option<&str>,
        app: Option<&str>,
        window: Option<&str>,
    ) -> ElementInfo {
        ElementInfo {
            role: role.map(Into::into),
            label: label.map(Into::into),
            app: app.map(Into::into),
            bundle_id: None,
            window: window.map(Into::into),
        }
    }

    #[test]
    fn a_labelled_control_in_a_window_is_clear() {
        let el = element(
            Some("AXButton"),
            Some("Save"),
            Some("TextEdit"),
            Some("Untitled"),
        );
        for kind in [
            StepKind::Click,
            StepKind::DoubleClick,
            StepKind::RightClick,
            StepKind::Drag,
            StepKind::Type,
        ] {
            assert!(
                element_issues(&step(kind, Some(el.clone()))).is_empty(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn shortcuts_and_manual_steps_need_no_element() {
        assert!(element_issues(&step(StepKind::Shortcut, None)).is_empty());
        assert!(element_issues(&step(StepKind::Manual, None)).is_empty());
        assert!(element_issues(&step(StepKind::Shortcut, Some(ElementInfo::default()))).is_empty());
        assert!(!needs_element(StepKind::Shortcut));
        assert!(needs_element(StepKind::Type));
    }

    #[test]
    fn flags_a_missing_element_alone() {
        assert_eq!(element_issues(&step(StepKind::Click, None)), [NoElement]);
    }

    #[test]
    fn flags_each_missing_or_vague_field_in_order() {
        assert_eq!(
            element_issues(&step(StepKind::Click, Some(ElementInfo::default()))),
            [NoApp, NoWindow, NoLabel, NoRole]
        );
        let el = element(Some("AXGroup"), Some("  "), Some("Notes"), Some("Notes"));
        assert_eq!(
            element_issues(&step(StepKind::Click, Some(el))),
            [NoLabel, GenericRole]
        );
        let el = element(Some("AXUnknown"), Some("Canvas"), None, Some("Doc"));
        assert_eq!(
            element_issues(&step(StepKind::Drag, Some(el))),
            [NoApp, GenericRole]
        );
    }

    #[test]
    fn a_bundle_id_stands_in_for_the_app_name() {
        let mut el = element(Some("AXButton"), Some("OK"), None, Some("Alert"));
        el.bundle_id = Some("com.apple.finder".into());
        assert!(locator_issues(&el).is_empty());
    }

    #[test]
    fn dock_and_menu_items_need_no_window() {
        for role in WINDOWLESS_ROLES {
            let el = element(Some(role), Some("Calendar"), Some("Dock"), None);
            assert!(locator_issues(&el).is_empty(), "{role}");
        }
        let el = element(Some("AXButton"), Some("Calendar"), Some("Dock"), None);
        assert_eq!(locator_issues(&el), [NoWindow]);
    }

    #[test]
    fn describes_issues_for_people_and_names_them_for_json() {
        assert_eq!(describe_issues(&[]), "");
        assert_eq!(describe_issues(&[NoLabel]), "no label");
        assert_eq!(
            describe_issues(&[NoLabel, GenericRole]),
            "no label and a generic role"
        );
        assert_eq!(
            describe_issues(&[NoApp, NoWindow, NoLabel]),
            "no app, no window and no label"
        );
        for issue in [NoElement, NoApp, NoWindow, NoLabel, NoRole, GenericRole] {
            assert_eq!(serde_json::to_value(issue).unwrap(), issue.as_str());
        }
    }
}
