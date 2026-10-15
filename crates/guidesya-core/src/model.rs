//! Serde types for `guide.json`. See design scratchpad #444 ("guide.json").
//!
//! JSON is camelCase. Ids are ULIDs serialized as strings. Rich-text fields
//! (`title`/`description` on steps, `intro`, `prerequisites`, note `body`)
//! hold the Markdown subset handled by [`crate::markdown`]. Image paths are
//! relative to the guide bundle. All geometry is in image pixels of the full
//! screenshot (annotations: of the image they are drawn on).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::SCHEMA_VERSION;

/// A whole guide: the contents of `guide.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Guide {
    pub schema_version: u32,
    pub id: Ulid,
    pub title: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Markdown.
    #[serde(default)]
    pub intro: String,
    /// Markdown.
    #[serde(default)]
    pub prerequisites: String,
    #[serde(default)]
    pub audience: Audience,
    /// BCP 47 language tag of the guide text, e.g. `en`.
    pub language: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Absent for guides built by hand rather than recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<RecordingInfo>,
    #[serde(default)]
    pub items: Vec<Item>,
    /// Provenance of the guide-level fields (`title`, `intro`,
    /// `prerequisites`, `summary`, `tags`). The editor sets `edited` when the
    /// user changes any of them; the AI guide pass then leaves them alone.
    #[serde(default, skip_serializing_if = "is_default")]
    pub ai_state: AiState,
}

impl Guide {
    /// An empty guide at the current schema version with a fresh id.
    pub fn new(title: impl Into<String>, language: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            schema_version: SCHEMA_VERSION,
            id: Ulid::new(),
            title: title.into(),
            summary: String::new(),
            tags: Vec::new(),
            intro: String::new(),
            prerequisites: String::new(),
            audience: Audience::default(),
            language: language.into(),
            created_at: now,
            updated_at: now,
            recording: None,
            items: Vec::new(),
            ai_state: AiState::default(),
        }
    }

    /// Steps in order, skipping sections and notes.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        self.items.iter().filter_map(|item| match item {
            Item::Step(step) => Some(step),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Audience {
    #[default]
    Beginner,
    Expert,
}

/// Environment the guide was recorded in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingInfo {
    /// GuidesYa version that made the recording.
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    /// Applications seen during the recording.
    #[serde(default)]
    pub apps: Vec<AppInfo>,
    #[serde(default)]
    pub displays: Vec<DisplayInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    pub id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Size in physical pixels.
    pub width: u32,
    pub height: u32,
    /// Backing scale factor (physical / logical pixels).
    pub scale: f64,
}

/// One entry of [`Guide::items`], tagged by `type`.
// Most items are steps, so boxing them would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Item {
    Step(Step),
    Section(Section),
    Note(Note),
}

impl Item {
    pub fn id(&self) -> Ulid {
        match self {
            Item::Step(step) => step.id,
            Item::Section(section) => section.id,
            Item::Note(note) => note.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub id: Ulid,
    /// Markdown.
    pub title: String,
    /// Markdown.
    #[serde(default)]
    pub description: String,
    pub kind: StepKind,
    /// Interaction point in full-screenshot pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<Point>,
    /// Where a `drag` step ends, in full-screenshot pixels; `point` is its start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_point: Option<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementInfo>,
    /// Keystrokes with secure-field input already masked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keystrokes: Option<String>,
    pub image: StepImage,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
    #[serde(default)]
    pub ai_state: AiState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepKind {
    Click,
    DoubleClick,
    RightClick,
    Type,
    Shortcut,
    Drag,
    Manual,
}

/// Accessibility info for the element a step acted on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}

/// Bundle-relative image paths for a step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepImage {
    /// Full screenshot, e.g. `steps/<id>.webp`.
    pub full: String,
    /// Click crop, e.g. `steps/<id>.crop.webp`. Absent for manual steps
    /// inserted from an image file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<String>,
    /// Where the crop sits inside the full screenshot, in full-screenshot
    /// pixels. Annotation geometry is in full-screenshot pixels, so this is
    /// what maps it into the crop. Absent for older guides or no crop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop_rect: Option<Rect>,
    /// Optional frame captured after the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    #[serde(default)]
    pub show: ImageShow,
}

/// Which image the step displays.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageShow {
    #[default]
    Crop,
    Full,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiState {
    #[serde(default)]
    pub status: AiStatus,
    /// Model that produced the current text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Prompt file version, e.g. `step.v1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    /// Short message from the last failed AI pass; cleared when a later
    /// pass succeeds. The text and `status` are left as they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Blur regions the AI suggests for the step's screenshot. Suggestions
    /// only (Q9): never applied as annotations without the user. Replaced by
    /// each successful per-step pass.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_suggestions: Vec<RedactionSuggestion>,
}

/// A suggested blur region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactionSuggestion {
    /// In full-screenshot pixels.
    pub rect: Rect,
    /// Why it looks sensitive, e.g. `email address`.
    pub reason: String,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiStatus {
    /// Template text only; no AI pass yet.
    #[default]
    Template,
    Generating,
    /// Text from the AI pass.
    Ai,
    /// Edited by the user; never overwritten by AI.
    Edited,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub id: Ulid,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: Ulid,
    pub kind: NoteKind,
    /// Markdown.
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoteKind {
    Tip,
    Warning,
    Note,
}

/// An SVG-compatible overlay shape; never baked into source images.
///
/// Serializes as `{ id, type, geometry, style }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    pub id: Ulid,
    #[serde(flatten)]
    pub shape: Shape,
    #[serde(default)]
    pub style: AnnotationStyle,
}

/// Annotation type and its geometry in image pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "geometry", rename_all = "camelCase")]
pub enum Shape {
    Arrow(ArrowGeometry),
    Box(Rect),
    Highlight(Rect),
    Callout(CalloutGeometry),
    Blur(Rect),
    Crop(Rect),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ArrowGeometry {
    pub from: Point,
    pub to: Point,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CalloutGeometry {
    /// Box holding the text.
    #[serde(flatten)]
    pub rect: Rect,
    /// Point the callout's pointer targets, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Point>,
    /// Plain text.
    pub text: String,
}

/// Visual style; unset fields use the app's defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotationStyle {
    /// CSS colour, e.g. `#ff3b30`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// Blur annotations only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_mode: Option<BlurMode>,
    /// Blur radius or pixel block size, in image pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blur_amount: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlurMode {
    Blur,
    Pixelate,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn sample() -> Value {
        serde_json::from_str(include_str!("../testdata/guide.json")).unwrap()
    }

    #[test]
    fn guide_round_trips() {
        let value = sample();
        let guide: Guide = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(guide.items.len(), 5);
        assert_eq!(guide.steps().count(), 3);
        assert_eq!(serde_json::to_value(&guide).unwrap(), value);

        let text = serde_json::to_string_pretty(&guide).unwrap();
        let reparsed: Guide = serde_json::from_str(&text).unwrap();
        assert_eq!(reparsed, guide);
    }

    #[test]
    fn decodes_typed_fields() {
        let guide: Guide = serde_json::from_value(sample()).unwrap();
        assert_eq!(guide.audience, Audience::Expert);
        let Item::Step(step) = &guide.items[1] else {
            panic!("expected step");
        };
        assert_eq!(step.kind, StepKind::DoubleClick);
        assert_eq!(step.image.show, ImageShow::Full);
        assert_eq!(step.ai_state.status, AiStatus::Ai);
        assert!(matches!(step.annotations[0].shape, Shape::Arrow(_)));
        let Shape::Callout(callout) = &step.annotations[3].shape else {
            panic!("expected callout");
        };
        assert_eq!(callout.text, "Here");
        assert_eq!(callout.rect.width, 100.0);
        assert!(step.end_point.is_none());
        let Item::Step(drag) = &guide.items[3] else {
            panic!("expected drag step");
        };
        assert_eq!(drag.kind, StepKind::Drag);
        assert_eq!(drag.point, Some(Point { x: 200.0, y: 150.0 }));
        assert_eq!(drag.end_point, Some(Point { x: 640.0, y: 420.0 }));
        assert_eq!(
            guide.items[4].id().to_string(),
            "01J9ZQ3K8V4N6T2W5X7Y8Z9A0N"
        );
    }

    #[test]
    fn optional_fields_default() {
        let guide: Guide = serde_json::from_value(json!({
            "schemaVersion": 1,
            "id": "01J9ZQ3K8V4N6T2W5X7Y8Z9A0B",
            "title": "Minimal",
            "language": "en",
            "createdAt": "2026-10-14T12:00:00Z",
            "updatedAt": "2026-10-14T12:00:00Z",
            "items": [{
                "type": "step",
                "id": "01J9ZQ3K8V4N6T2W5X7Y8Z9A0D",
                "title": "Click OK",
                "kind": "click",
                "image": { "full": "steps/a.webp" }
            }]
        }))
        .unwrap();
        assert_eq!(guide.audience, Audience::Beginner);
        assert!(guide.recording.is_none());
        let step = guide.steps().next().unwrap();
        assert_eq!(step.image.show, ImageShow::Crop);
        assert_eq!(step.ai_state, AiState::default());
        assert!(step.annotations.is_empty());
    }

    #[test]
    fn old_guide_json_is_unchanged_by_new_ai_fields() {
        // Written before `ai_state.error`, `ai_state.redactionSuggestions`
        // and `Guide.aiState` existed.
        let text = include_str!("../testdata/guide.json");
        let guide: Guide = serde_json::from_str(text).unwrap();
        assert_eq!(guide.ai_state, AiState::default());
        for step in guide.steps() {
            assert_eq!(step.ai_state.error, None);
            assert!(step.ai_state.redaction_suggestions.is_empty());
        }
        let out = serde_json::to_value(&guide).unwrap();
        assert_eq!(out, serde_json::from_str::<Value>(text).unwrap());
        assert!(out.get("aiState").is_none());
        let step_states: Vec<&Value> = out["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.get("aiState"))
            .collect();
        assert_eq!(step_states.len(), 3);
        for state in step_states {
            assert!(state.get("error").is_none());
            assert!(state.get("redactionSuggestions").is_none());
        }
    }

    #[test]
    fn new_ai_fields_round_trip() {
        let mut guide = Guide::new("Untitled", "en");
        guide.ai_state = AiState {
            status: AiStatus::Edited,
            model: None,
            prompt_version: Some("guide.v1".into()),
            error: Some("network error: reset".into()),
            redaction_suggestions: vec![RedactionSuggestion {
                rect: Rect {
                    x: 1.0,
                    y: 2.0,
                    width: 3.0,
                    height: 4.0,
                },
                reason: "email address".into(),
            }],
        };
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(
            json["aiState"],
            json!({
                "status": "edited",
                "promptVersion": "guide.v1",
                "error": "network error: reset",
                "redactionSuggestions": [{
                    "rect": { "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0 },
                    "reason": "email address"
                }]
            })
        );
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);
    }

    #[test]
    fn new_guide_uses_current_schema() {
        let guide = Guide::new("Untitled", "en");
        assert_eq!(guide.schema_version, SCHEMA_VERSION);
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(json["schemaVersion"], SCHEMA_VERSION);
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);
    }
}
