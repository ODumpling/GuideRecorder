//! The rules every backend applies to the element under a click or with
//! focus: which element a click is about (the ancestor rule, below), when
//! it may borrow a label, when keys must be masked, and how text is
//! cleaned. Roles are AX role names; the Windows backend maps UI
//! Automation control types onto them.
//!
//! # Ancestor rule
//!
//! The hit-tested element is often a child of the control the user meant: a
//! static text or image inside a button, or a group inside a cell. Only
//! when the hit element's role is a [pass-through role](PASS_THROUGH_ROLES)
//! does a backend walk up its parents, up to [`MAX_PARENT_DEPTH`] levels,
//! stopping at a window or the application. It reports the first ancestor
//! with an [actionable role](ACTIONABLE_ROLES) instead of the hit element.
//! If that ancestor has no label of its own, it takes the nearest label
//! found on the way back down (for example a static text's value).
//! Otherwise the hit element is reported as is. The choice is the pure
//! function [`choose_target`]. Secure and text-entry targets never inherit
//! a label ([`may_inherit_label`]).

use guiderecorder_core::events::RawElement;

/// How many parent hops a backend's hit test may take looking for a better
/// target.
pub const MAX_PARENT_DEPTH: usize = 3;

/// Longest label/title kept, in characters, after whitespace is collapsed.
pub const MAX_TEXT_CHARS: usize = 200;

/// Roles that are usually decoration inside a real control.
pub const PASS_THROUGH_ROLES: &[&str] = &["AXStaticText", "AXImage", "AXGroup", "AXUnknown"];

/// Roles a user clicks on purpose; preferred over a pass-through child.
pub const ACTIONABLE_ROLES: &[&str] = &[
    "AXButton",
    "AXMenuButton",
    "AXPopUpButton",
    "AXCheckBox",
    "AXRadioButton",
    "AXMenuItem",
    "AXMenuBarItem",
    "AXLink",
    "AXTab",
    "AXCell",
    "AXRow",
    "AXDisclosureTriangle",
    "AXComboBox",
    "AXTextField",
    "AXTextArea",
    "AXSlider",
    "AXIncrementor",
    "AXColorWell",
    "AXDockItem",
];

/// Roles at which the parent walk stops.
pub const WALK_STOP_ROLES: &[&str] = &["AXWindow", "AXApplication", "AXSheet", "AXDrawer"];

/// The (sub)role of a secure text field.
pub const SECURE_TEXT_FIELD: &str = "AXSecureTextField";

/// True unless `focused` is a definite non-secure element. Timeouts, errors
/// and "nothing focused" all mask (fail closed).
pub fn must_mask_keys<E>(focused: &Result<Option<RawElement>, E>) -> bool {
    !matches!(focused, Ok(Some(el)) if !el.is_secure)
}

/// True for a secure text field, by subrole (the documented signal) or by
/// role (some custom controls report it there).
pub fn is_secure(role: Option<&str>, subrole: Option<&str>) -> bool {
    subrole == Some(SECURE_TEXT_FIELD) || role == Some(SECURE_TEXT_FIELD)
}

/// Trims, collapses runs of whitespace to one space, and caps the length
/// at [`MAX_TEXT_CHARS`]. Blank input gives `None`.
pub fn clean_text(s: &str) -> Option<String> {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    match collapsed.char_indices().nth(MAX_TEXT_CHARS) {
        Some((cut, _)) => Some(format!("{}…", collapsed[..cut].trim_end())),
        None => Some(collapsed),
    }
}

/// Index into `roles` (hit element first, then its ancestors) of the
/// element to report. See the module docs for the rule.
pub fn choose_target(roles: &[Option<&str>]) -> usize {
    let Some(&hit) = roles.first() else {
        return 0;
    };
    if !role_in(hit, PASS_THROUGH_ROLES) {
        return 0;
    }
    for (i, &role) in roles.iter().enumerate().skip(1).take(MAX_PARENT_DEPTH) {
        if role_in(role, ACTIONABLE_ROLES) {
            return i;
        }
        if role_in(role, WALK_STOP_ROLES) {
            break;
        }
    }
    0
}

/// Text-entry roles whose children may hold typed or autofilled content.
pub const NO_INHERIT_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"];

/// Whether a target element may take a label from a descendant. Never for
/// secure elements or text-entry controls: their child text can be user
/// content (typed text, autofill), not a label.
pub fn may_inherit_label(role: Option<&str>, is_secure: bool) -> bool {
    !is_secure && !role_in(role, NO_INHERIT_ROLES)
}

pub(crate) fn role_in(role: Option<&str>, set: &[&str]) -> bool {
    role.is_some_and(|r| set.contains(&r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_collapses_and_caps() {
        assert_eq!(clean_text("  Save \n  As… "), Some("Save As…".into()));
        assert_eq!(clean_text(" \t\n"), None);
        let long = "a".repeat(MAX_TEXT_CHARS + 50);
        let cleaned = clean_text(&long).unwrap();
        assert_eq!(cleaned.chars().count(), MAX_TEXT_CHARS + 1);
        assert!(cleaned.ends_with('…'));
        // Multi-byte input is cut on a char boundary.
        let wide = "é".repeat(MAX_TEXT_CHARS + 1);
        assert_eq!(
            clean_text(&wide).unwrap().chars().count(),
            MAX_TEXT_CHARS + 1
        );
    }

    #[test]
    fn secure_detection() {
        assert!(is_secure(Some("AXTextField"), Some("AXSecureTextField")));
        assert!(is_secure(Some("AXSecureTextField"), None));
        assert!(!is_secure(Some("AXTextField"), Some("AXSearchField")));
        assert!(!is_secure(None, None));
    }

    #[test]
    fn static_text_in_button_targets_button() {
        let roles = [Some("AXStaticText"), Some("AXButton"), Some("AXWindow")];
        assert_eq!(choose_target(&roles), 1);
    }

    #[test]
    fn label_inheritance_blocked_for_secure_and_text_entry() {
        assert!(may_inherit_label(Some("AXButton"), false));
        assert!(may_inherit_label(Some("AXCell"), false));
        assert!(may_inherit_label(None, false));
        assert!(!may_inherit_label(Some("AXButton"), true));
        for role in ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"] {
            assert!(!may_inherit_label(Some(role), false), "{role}");
        }
    }

    #[test]
    fn actionable_hit_is_kept() {
        assert_eq!(choose_target(&[Some("AXButton"), Some("AXCell")]), 0);
        assert_eq!(choose_target(&[None, Some("AXButton")]), 0);
        assert_eq!(choose_target(&[]), 0);
    }

    #[test]
    fn walk_stops_at_window_and_depth() {
        let window_first = [Some("AXStaticText"), Some("AXWindow"), Some("AXButton")];
        assert_eq!(choose_target(&window_first), 0);

        let mut deep = vec![Some("AXStaticText")];
        deep.extend((0..MAX_PARENT_DEPTH).map(|_| Some("AXGroup")));
        deep.push(Some("AXButton"));
        assert_eq!(choose_target(&deep), 0);

        let in_cell = [Some("AXStaticText"), Some("AXGroup"), Some("AXCell")];
        assert_eq!(choose_target(&in_cell), 2);
    }

    #[test]
    fn any_lookup_error_masks() {
        let failed: Result<Option<RawElement>, ()> = Err(());
        assert!(must_mask_keys(&failed));
        assert!(!must_mask_keys::<()>(&Ok(Some(RawElement::default()))));
    }
}
