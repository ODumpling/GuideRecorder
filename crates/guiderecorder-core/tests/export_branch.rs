//! Decisions and section audiences in exports (#22).
//!
//! Goldens are under `tests/golden/branch/`; regenerate them with
//! `UPDATE_GOLDEN=1 cargo test -p guiderecorder-core --test export_branch`
//! and review the diff. The fixture guides cover a decision in every
//! format's own goldens; this guide adds sections with audiences and
//! options that lead to sections.

mod support;

use std::path::PathBuf;

use guiderecorder_core::branch::for_audience;
use guiderecorder_core::export::html::{self, HtmlMode, HtmlOptions};
use guiderecorder_core::export::md::{self, Labels};
use guiderecorder_core::export::{RenderGuide, RenderItem, RenderTarget};
use guiderecorder_core::library::GuideHandle;
use guiderecorder_core::model::*;
use guiderecorder_core::Ulid;
use support::assert_golden_text;

fn id(n: u128) -> Ulid {
    Ulid::from_parts(1_792_000_000_000, n)
}

fn step(n: u128, title: &str) -> Item {
    Item::Step(Step {
        id: id(n),
        title: title.into(),
        description: String::new(),
        kind: StepKind::Click,
        point: None,
        end_point: None,
        element: None,
        keystrokes: None,
        url: None,
        image: StepImage {
            full: format!("steps/{}.webp", id(n)),
            crop: None,
            crop_rect: None,
            after: None,
            show: ImageShow::Full,
        },
        annotations: Vec::new(),
        narration: String::new(),
        ai_state: AiState::default(),
    })
}

fn section(n: u128, title: &str, audiences: &[&str]) -> Item {
    Item::Section(Section {
        id: id(n),
        title: title.into(),
        audiences: audiences.iter().map(|a| (*a).to_owned()).collect(),
    })
}

fn option(n: u128, label: &str, target: BranchTarget) -> DecisionOption {
    DecisionOption {
        id: id(n),
        label: label.into(),
        target,
    }
}

/// Step 1, a decision leading to the admins' or users' section or the
/// last step, the two sections, and a shared closing section.
fn guide() -> Guide {
    let mut guide = Guide::new("Reset a password", "en");
    guide.id = id(1);
    guide.items = vec![
        step(2, "Open the **console**"),
        Item::Decision(Decision {
            id: id(3),
            question: "Are you an *administrator*?".into(),
            options: vec![
                option(4, "Yes", BranchTarget::Section { id: id(10) }),
                option(5, "No", BranchTarget::Section { id: id(20) }),
                option(6, "Already reset", BranchTarget::Step { id: id(41) }),
                option(
                    7,
                    "",
                    BranchTarget::Guide {
                        id: id(99),
                        title: "Unlock an account".into(),
                    },
                ),
            ],
        }),
        section(10, "Admins", &["Admin"]),
        step(11, "Open **Users** and choose *Reset*"),
        section(20, "Everyone else", &["User", "Guest"]),
        step(21, "Ask an administrator"),
        section(40, "Finish", &[]),
        step(41, "Sign out"),
    ];
    guide
}

fn handle(guide: &Guide) -> GuideHandle {
    GuideHandle {
        id: guide.id,
        path: PathBuf::from("/nonexistent.guiderecorder"),
    }
}

fn markdown(guide: &Guide) -> String {
    let render = RenderGuide::new(guide, &handle(guide));
    md::render(
        &render,
        &md::image_names(&render),
        &Labels::for_language("en"),
    )
}

fn targets(guide: &Guide) -> Vec<RenderTarget> {
    let render = RenderGuide::new(guide, &handle(guide));
    render
        .decisions()
        .flat_map(|d| d.options.iter().map(|o| o.target.clone()))
        .collect()
}

#[test]
fn markdown_shows_options_and_audiences() {
    assert_golden_text("branch/all.md.golden", &markdown(&guide()));
    // For admins: the users' section and the option leading there go.
    assert_golden_text(
        "branch/admin.md.golden",
        &markdown(&for_audience(&guide(), "admin")),
    );
}

#[test]
fn options_resolve_to_step_numbers() {
    let step = |n, s: Option<&str>| RenderTarget::Step {
        number: Some(n),
        anchor: Some(n),
        section: s.map(str::to_owned),
    };
    let guide_target = RenderTarget::Guide {
        id: id(99),
        title: "Unlock an account".into(),
    };
    assert_eq!(
        targets(&guide()),
        [
            step(2, Some("Admins")),
            step(3, Some("Everyone else")),
            step(4, None),
            guide_target.clone(),
        ]
    );
    assert_eq!(
        targets(&for_audience(&guide(), "User")),
        [step(2, Some("Everyone else")), step(3, None), guide_target]
    );

    // `next` is the step after the decision, a gone target reads as next,
    // and a section with no step after it is the end.
    let mut guide = guide();
    let Item::Decision(decision) = &mut guide.items[1] else {
        panic!("expected the decision");
    };
    decision.options = vec![
        option(4, "a", BranchTarget::Next),
        option(5, "b", BranchTarget::Step { id: id(500) }),
        option(6, "c", BranchTarget::Section { id: id(40) }),
    ];
    guide.items.truncate(7);
    assert_eq!(
        targets(&guide),
        [
            step(2, None),
            step(2, None),
            RenderTarget::Step {
                number: None,
                anchor: None,
                section: Some("Finish".into()),
            },
        ]
    );
}

#[test]
fn labels_say_where_options_lead() {
    let en = Labels::for_language("en");
    let de = Labels::for_language("de");
    let to_section = RenderTarget::Step {
        number: Some(5),
        anchor: Some(5),
        section: Some("Admins".into()),
    };
    let end = RenderTarget::Step {
        number: None,
        anchor: None,
        section: None,
    };
    assert_eq!(
        en.go_to(&to_section, GuideKind::Walkthrough),
        "Go to “Admins” (Step 5)"
    );
    assert_eq!(
        de.go_to(&to_section, GuideKind::Walkthrough),
        "Weiter mit „Admins“ (Schritt 5)"
    );
    assert_eq!(en.go_to(&end, GuideKind::Walkthrough), "Go to the end");
    let page = RenderTarget::Step {
        number: Some(2),
        anchor: Some(2),
        section: None,
    };
    assert_eq!(en.go_to(&page, GuideKind::Annotated), "Go to Page 2");
    let untitled = RenderTarget::Guide {
        id: id(1),
        title: String::new(),
    };
    assert_eq!(
        en.go_to(&untitled, GuideKind::Walkthrough),
        "Go to the guide"
    );
    assert_eq!(
        en.audiences(&["Admin".into(), "User".into()]).unwrap(),
        "For: Admin, User"
    );
    assert_eq!(en.audiences(&[]), None);
}

#[test]
fn html_links_options_to_steps() {
    let guide = guide();
    let render = RenderGuide::new(&guide, &handle(&guide));
    let (page, _) = html::render_string(&render, &handle(&guide), &HtmlOptions::default()).unwrap();
    assert!(
        page.contains(r#"<p class="audience">For: User, Guest</p>"#),
        "{page}"
    );
    assert!(page.contains(
        r##"<li><strong>Yes:</strong> <a href="#step-2">Go to “Admins” (Step 2)</a></li>"##
    ));
    assert!(page.contains(
        r##"<li><strong>Already reset:</strong> <a href="#step-4">Go to Step 4</a></li>"##
    ));
    // Another guide can't be linked from a file.
    assert!(page.contains("<li>Go to the guide “Unlock an account”</li>"));
    assert!(page.contains(r#"<p class="decision-question"><span class="decision-label">Decision:</span> Are you an <em>administrator</em>?</p>"#));
    assert!(page.contains(r#"<article class="step" id="step-4">"#));
}

#[test]
fn walkthrough_options_lead_to_their_slides() {
    let guide = guide();
    let render = RenderGuide::new(&guide, &handle(&guide));
    let opts = HtmlOptions {
        mode: HtmlMode::Walkthrough,
        ..HtmlOptions::default()
    };
    let (page, _) = html::render_string(&render, &handle(&guide), &opts).unwrap();
    // Slides: step 1, the decision, steps 2 to 4, done.
    assert!(page.contains(r#"<section class="wt-slide wt-decision" id="wt-slide-1""#));
    assert!(page.contains(r#"data-status="Decision: Are you an administrator?""#));
    for (href, status) in [
        ("#wt-slide-2", "Step 2 of 4"),
        ("#wt-slide-3", "Step 3 of 4"),
        ("#wt-slide-4", "Step 4 of 4"),
    ] {
        assert!(
            page.contains(&format!(r#"href="{href}" data-branch>"#)),
            "{href}"
        );
        let slide = format!(r#"id="{}""#, &href[1..]);
        let at = page.find(&slide).unwrap();
        assert!(page[at..].contains(&format!(r#"data-status="{status}"#)));
    }
    assert!(page
        .contains(r#"<span class="wt-option-static">Go to the guide “Unlock an account”</span>"#));
    // The decision is listed with the steps.
    assert!(page.contains(
        r##"<a href="#wt-slide-1" data-slide="1"><span class="wt-list-text">Are you an administrator?</span></a>"##
    ));
    let render = RenderGuide::new(&guide, &handle(&guide));
    assert!(matches!(render.items[1], RenderItem::Decision(_)));
}
