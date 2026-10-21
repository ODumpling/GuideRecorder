//! Serde types for `guide.json`. See design scratchpad #444 ("guide.json").
//!
//! JSON is camelCase. Ids are ULIDs serialized as strings. Rich-text fields
//! (`title`/`description` on steps, `intro`, `prerequisites`, note `body`)
//! hold the Markdown subset handled by [`crate::markdown`]. Image paths are
//! relative to the guide bundle. All geometry is in image pixels of the full
//! screenshot (annotations: of the image they are drawn on).

use chrono::{DateTime, Local, Months, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The type of review days, re-exported for crates without chrono.
pub use chrono::NaiveDate;

use crate::SCHEMA_VERSION;

/// A whole guide: the contents of `guide.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Guide {
    pub schema_version: u32,
    #[schemars(with = "String")]
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
    /// What the guide is: recorded steps, or uploaded pages with hotspots.
    #[serde(default, skip_serializing_if = "is_default")]
    pub kind: GuideKind,
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
    /// user changes any of them, and keeps it while any is in
    /// [`Guide::edited_fields`].
    #[serde(default, skip_serializing_if = "is_default")]
    pub ai_state: AiState,
    /// The guide-level fields the user wrote by hand; the AI guide pass
    /// leaves them alone and may still write the others. Empty in guides
    /// saved before it existed: there an `edited` [`Guide::ai_state`] means
    /// every field is the user's (see [`Guide::is_field_edited`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited_fields: Vec<GuideField>,
    /// The step the author chose as the cover: the guide's library card,
    /// the cover page of a PDF or Word export and an HTML export's
    /// `og:image`. Unset, or naming a step that is gone or has no
    /// screenshot, the card shows the first screenshot and exports show no
    /// cover image (see [`Guide::cover_step`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub cover_step_id: Option<Ulid>,
    /// How the cover is drawn; left out while default.
    #[serde(default, skip_serializing_if = "is_default")]
    pub cover_options: CoverOptions,
    /// Owner, status, version and review schedule. Flattened: the fields
    /// sit at the top level of `guide.json` and are left out while unset,
    /// so guides without them read and write unchanged.
    #[serde(flatten)]
    pub review: ReviewInfo,
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
            kind: GuideKind::default(),
            language: language.into(),
            created_at: now,
            updated_at: now,
            recording: None,
            items: Vec::new(),
            ai_state: AiState::default(),
            edited_fields: Vec::new(),
            cover_step_id: None,
            cover_options: CoverOptions::default(),
            review: ReviewInfo::default(),
        }
    }

    /// When the next review is due (see [`ReviewInfo::review_due`]).
    pub fn review_due(&self) -> Option<NaiveDate> {
        self.review.review_due(self.created_at.date_naive())
    }

    /// Whether the review is overdue on `today` (see
    /// [`ReviewInfo::is_review_overdue`]).
    pub fn is_review_overdue(&self, today: NaiveDate) -> bool {
        self.review
            .is_review_overdue(self.created_at.date_naive(), today)
    }

    /// Whether the user wrote `field` by hand, so AI must not overwrite it.
    /// A guide with no [`Guide::edited_fields`] but an `edited`
    /// [`Guide::ai_state`] predates per-field tracking: all its fields are.
    pub fn is_field_edited(&self, field: GuideField) -> bool {
        if self.edited_fields.is_empty() {
            self.ai_state.status == AiStatus::Edited
        } else {
            self.edited_fields.contains(&field)
        }
    }

    /// Steps in order, skipping sections and notes.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        self.items.iter().filter_map(|item| match item {
            Item::Step(step) => Some(step),
            _ => None,
        })
    }

    /// The step chosen as the cover ([`Guide::cover_step_id`]), if it is
    /// still in the guide and has a screenshot.
    pub fn chosen_cover_step(&self) -> Option<&Step> {
        let id = self.cover_step_id?;
        self.steps()
            .find(|step| step.id == id)
            .filter(|step| has_screenshot(step))
    }

    /// The guide's cover: the chosen step ([`Guide::chosen_cover_step`]),
    /// else the first step with a screenshot.
    pub fn cover_step(&self) -> Option<&Step> {
        self.chosen_cover_step()
            .or_else(|| self.steps().find(|step| has_screenshot(step)))
    }

    /// Hotspot annotations across all steps.
    pub fn hotspot_count(&self) -> usize {
        self.steps()
            .flat_map(|step| &step.annotations)
            .filter(|annotation| matches!(annotation.shape, Shape::Hotspot(_)))
            .count()
    }
}

fn has_screenshot(step: &Step) -> bool {
    !step.image.full.is_empty()
}

/// How a guide's cover is drawn from its step ([`Guide::cover_step_id`]).
/// Blur and pixelation are burned in either way, and a crop annotation
/// still cuts the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", default)]
pub struct CoverOptions {
    /// The close-up (`crop`) or the full screenshot (`full`, the default).
    /// A step without a close-up shows its full screenshot.
    pub image: ImageShow,
    /// Draw the step's annotations (arrows, boxes, callouts, hotspots) on
    /// it; off by default.
    pub annotations: bool,
}

impl Default for CoverOptions {
    fn default() -> Self {
        Self {
            image: ImageShow::Full,
            annotations: false,
        }
    }
}

/// A guide-level field the AI guide pass writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GuideField {
    Title,
    Intro,
    Prerequisites,
    Summary,
    Tags,
}

impl GuideField {
    pub const ALL: [GuideField; 5] = [
        GuideField::Title,
        GuideField::Intro,
        GuideField::Prerequisites,
        GuideField::Summary,
        GuideField::Tags,
    ];
}

/// Who owns a guide, where it stands, which version it is and how often it
/// is reviewed. Every field is optional and set by hand; see
/// [`Guide::review`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewInfo {
    /// Who keeps the guide current, as free text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<GuideStatus>,
    /// Free text, e.g. `1.2`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The day the guide was last reviewed, `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reviewed: Option<NaiveDate>,
    /// Review every N months; unset or 0: no schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_interval_months: Option<u32>,
}

impl ReviewInfo {
    /// When the next review is due: [`ReviewInfo::last_reviewed`] (or
    /// `created`, the day the guide was made, if it never was) plus the
    /// interval, on the same day of the month or else the month's last day.
    /// `None` without an interval.
    pub fn review_due(&self, created: NaiveDate) -> Option<NaiveDate> {
        let months = self.review_interval_months.filter(|&m| m > 0)?;
        self.last_reviewed
            .unwrap_or(created)
            .checked_add_months(Months::new(months))
    }

    /// Whether the review is overdue on `today`: past its due day (see
    /// [`ReviewInfo::review_due`]). Due today is not overdue yet.
    pub fn is_review_overdue(&self, created: NaiveDate, today: NaiveDate) -> bool {
        self.review_due(created).is_some_and(|due| today > due)
    }
}

/// Where a guide stands in its review cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GuideStatus {
    Draft,
    InReview,
    Published,
    /// Known to be out of date.
    NeedsUpdate,
}

impl GuideStatus {
    /// The name in `guide.json`, e.g. `needsUpdate`.
    pub fn as_str(self) -> &'static str {
        match self {
            GuideStatus::Draft => "draft",
            GuideStatus::InReview => "inReview",
            GuideStatus::Published => "published",
            GuideStatus::NeedsUpdate => "needsUpdate",
        }
    }
}

/// Today in the local time zone: the `today` for
/// [`ReviewInfo::is_review_overdue`] outside tests.
pub fn local_today() -> NaiveDate {
    Local::now().date_naive()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Audience {
    #[default]
    Beginner,
    Expert,
}

/// What a guide is made of.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum GuideKind {
    /// Recorded (or hand-made) steps.
    #[default]
    Walkthrough,
    /// Uploaded pages (images, PDF pages), each a `manual` step showing its
    /// full image, explained by [`Shape::Hotspot`] annotations.
    Annotated,
}

impl GuideKind {
    /// The name in `guide.json`, e.g. `annotated`.
    pub fn as_str(self) -> &'static str {
        match self {
            GuideKind::Walkthrough => "walkthrough",
            GuideKind::Annotated => "annotated",
        }
    }
}

/// Environment the guide was recorded in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecordingInfo {
    /// GuideRecorder version that made the recording.
    pub app_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
    /// Applications seen during the recording.
    #[serde(default)]
    pub apps: Vec<AppInfo>,
    #[serde(default)]
    pub displays: Vec<DisplayInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    #[schemars(with = "String")]
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
    /// What the author said during the step while narrating a recording,
    /// transcribed on the device (see [`crate::narration`]). Plain text,
    /// kept as spoken: it fills an empty description and is context for
    /// the AI step and guide passes. Left out while empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub narration: String,
    #[serde(default)]
    pub ai_state: AiState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ImageShow {
    #[default]
    Crop,
    Full,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    /// Blur regions suggested for the step's screenshot, by the AI or found
    /// on the device ([`crate::redact`]). Suggestions only (Q9): never
    /// applied as annotations without the user. Each successful per-step AI
    /// pass replaces the AI's own; on-device ones stay until reviewed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_suggestions: Vec<RedactionSuggestion>,
    /// Whether the step's screenshot was scanned for sensitive text on this
    /// device ([`crate::redact`]), so the editor can say which steps were
    /// not.
    #[serde(default, skip_serializing_if = "is_default")]
    pub scanned: bool,
}

/// A suggested blur region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RedactionSuggestion {
    /// In full-screenshot pixels.
    pub rect: Rect,
    /// Why it looks sensitive, e.g. `email address`.
    pub reason: String,
    /// Who suggested it. Absent (the AI) in guides saved before on-device
    /// redaction.
    #[serde(default, skip_serializing_if = "is_default")]
    pub source: SuggestionSource,
    /// A masked hint of the text found, e.g. `•••• 4417`, for the editor's
    /// privacy review. Only as much as is safe to keep in `guide.json`
    /// (which syncs): never the text itself. Absent for the AI's and for
    /// kinds with nothing safe to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    /// What the user decided. Absent: not reviewed yet (pending), the only
    /// state blurred into images that leave the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<SuggestionReview>,
}

impl RedactionSuggestion {
    /// Not reviewed yet.
    pub fn is_pending(&self) -> bool {
        self.review.is_none()
    }
}

/// The user's decision on a [`RedactionSuggestion`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionReview {
    /// Accepted: a blur annotation now covers it.
    Blurred,
    /// Left as it is.
    Kept,
}

/// Where a [`RedactionSuggestion`] came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SuggestionSource {
    /// The AI step pass.
    #[default]
    Ai,
    /// Text recognised and matched on this device ([`crate::redact`]).
    OnDevice,
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    #[schemars(with = "String")]
    pub id: Ulid,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    #[schemars(with = "String")]
    pub id: Ulid,
    pub kind: NoteKind,
    /// Markdown.
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum NoteKind {
    Tip,
    Warning,
    Note,
}

/// An SVG-compatible overlay shape; never baked into source images.
///
/// Serializes as `{ id, type, geometry, style }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    #[schemars(with = "String")]
    pub id: Ulid,
    #[serde(flatten)]
    pub shape: Shape,
    #[serde(default)]
    pub style: AnnotationStyle,
}

/// Annotation type and its geometry in image pixels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", content = "geometry", rename_all = "camelCase")]
pub enum Shape {
    Arrow(ArrowGeometry),
    Box(Rect),
    Highlight(Rect),
    Callout(CalloutGeometry),
    Blur(Rect),
    Crop(Rect),
    Hotspot(HotspotGeometry),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ArrowGeometry {
    pub from: Point,
    pub to: Point,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// A region the reader can hover or click to see its title and
/// description. Numbered 1, 2, 3, ... in annotation order among the step's
/// hotspots.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HotspotGeometry {
    #[serde(flatten)]
    pub rect: Rect,
    /// Plain text.
    #[serde(default)]
    pub title: String,
    /// Plain text; may hold line breaks.
    #[serde(default)]
    pub description: String,
}

/// Visual style; unset fields use the app's defaults.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
            redaction_suggestions: vec![
                RedactionSuggestion {
                    rect: Rect {
                        x: 1.0,
                        y: 2.0,
                        width: 3.0,
                        height: 4.0,
                    },
                    reason: "email address".into(),
                    source: SuggestionSource::Ai,
                    preview: None,
                    review: None,
                },
                RedactionSuggestion {
                    rect: Rect {
                        x: 5.0,
                        y: 6.0,
                        width: 7.0,
                        height: 8.0,
                    },
                    reason: "card number".into(),
                    source: SuggestionSource::OnDevice,
                    preview: Some("•••• 4417".into()),
                    review: Some(SuggestionReview::Kept),
                },
            ],
            scanned: true,
        };
        let json = serde_json::to_value(&guide).unwrap();
        // The AI's suggestions are written as before, without a source.
        assert_eq!(
            json["aiState"],
            json!({
                "status": "edited",
                "promptVersion": "guide.v1",
                "error": "network error: reset",
                "redactionSuggestions": [{
                    "rect": { "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0 },
                    "reason": "email address"
                }, {
                    "rect": { "x": 5.0, "y": 6.0, "width": 7.0, "height": 8.0 },
                    "reason": "card number",
                    "source": "onDevice",
                    "preview": "•••• 4417",
                    "review": "kept"
                }],
                "scanned": true
            })
        );
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);
    }

    #[test]
    fn edited_fields_round_trip_and_default_empty() {
        let mut guide = Guide::new("Untitled", "en");
        assert!(serde_json::to_value(&guide)
            .unwrap()
            .get("editedFields")
            .is_none());
        guide.edited_fields = vec![GuideField::Intro, GuideField::Tags];
        guide.ai_state.status = AiStatus::Edited;
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(json["editedFields"], json!(["intro", "tags"]));
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);

        let old: Guide = serde_json::from_str(include_str!("../testdata/guide.json")).unwrap();
        assert!(old.edited_fields.is_empty());
    }

    #[test]
    fn field_edits_are_per_field_with_legacy_edited_meaning_all() {
        let mut guide = Guide::new("Untitled", "en");
        assert!(GuideField::ALL.iter().all(|&f| !guide.is_field_edited(f)));
        // Saved before `editedFields`: `edited` covers every field.
        guide.ai_state.status = AiStatus::Edited;
        assert!(GuideField::ALL.iter().all(|&f| guide.is_field_edited(f)));
        guide.edited_fields = vec![GuideField::Summary];
        assert!(guide.is_field_edited(GuideField::Summary));
        assert!(!guide.is_field_edited(GuideField::Intro));
        assert!(!guide.is_field_edited(GuideField::Title));
    }

    #[test]
    fn hotspots_and_kind_round_trip() {
        let mut guide = Guide::new("Floor plan", "en");
        assert!(serde_json::to_value(&guide).unwrap().get("kind").is_none());
        guide.kind = GuideKind::Annotated;
        let annotation: Annotation = serde_json::from_value(json!({
            "id": "01J9ZQ3K8V4N6T2W5X7Y8Z9A0D",
            "type": "hotspot",
            "geometry": {
                "x": 10.0, "y": 20.0, "width": 30.0, "height": 40.0,
                "title": "Kitchen", "description": "Gas hob\nNorth window"
            }
        }))
        .unwrap();
        let Shape::Hotspot(hotspot) = &annotation.shape else {
            panic!("expected hotspot");
        };
        assert_eq!(hotspot.rect.width, 30.0);
        assert_eq!(hotspot.title, "Kitchen");
        assert_eq!(hotspot.description, "Gas hob\nNorth window");
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(json["kind"], "annotated");
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);
        // Title and description may be missing.
        let bare: Annotation = serde_json::from_value(json!({
            "id": "01J9ZQ3K8V4N6T2W5X7Y8Z9A0D",
            "type": "hotspot",
            "geometry": { "x": 0.0, "y": 0.0, "width": 1.0, "height": 1.0 }
        }))
        .unwrap();
        assert!(
            matches!(&bare.shape, Shape::Hotspot(h) if h.title.is_empty() && h.description.is_empty())
        );
    }

    #[test]
    fn cover_round_trips_and_defaults_unset() {
        let old: Guide = serde_json::from_value(sample()).unwrap();
        assert_eq!(old.cover_step_id, None);
        assert_eq!(old.cover_options, CoverOptions::default());
        let json = serde_json::to_value(&old).unwrap();
        assert!(json.get("coverStepId").is_none());
        assert!(json.get("coverOptions").is_none());

        let mut guide = old.clone();
        let third = guide.steps().nth(2).unwrap().id;
        guide.cover_step_id = Some(third);
        guide.cover_options = CoverOptions {
            image: ImageShow::Crop,
            annotations: true,
        };
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(json["coverStepId"], third.to_string());
        assert_eq!(
            json["coverOptions"],
            json!({ "image": "crop", "annotations": true })
        );
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);

        // Options missing a field take its default.
        let mut json = sample();
        json["coverOptions"] = json!({ "annotations": true });
        let partial: Guide = serde_json::from_value(json).unwrap();
        assert_eq!(partial.cover_options.image, ImageShow::Full);
        assert!(partial.cover_options.annotations);
    }

    #[test]
    fn narration_round_trips_and_is_left_out_while_empty() {
        let old: Guide = serde_json::from_value(sample()).unwrap();
        assert!(old.steps().all(|s| s.narration.is_empty()));
        let json = serde_json::to_value(&old).unwrap();
        assert!(json["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("narration").is_none()));

        let mut guide = old;
        if let Some(Item::Step(step)) = guide.items.iter_mut().find(|i| matches!(i, Item::Step(_)))
        {
            step.narration = "now save it".into();
        }
        let json = serde_json::to_value(&guide).unwrap();
        let narrated: Vec<&Value> = json["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|item| item.get("narration"))
            .collect();
        assert_eq!(narrated, [&json!("now save it")]);
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);
    }

    #[test]
    fn cover_step_falls_back_to_the_first_screenshot() {
        let mut guide: Guide = serde_json::from_value(sample()).unwrap();
        let ids: Vec<Ulid> = guide.steps().map(|s| s.id).collect();
        assert_eq!(guide.chosen_cover_step(), None);
        assert_eq!(guide.cover_step().map(|s| s.id), Some(ids[0]));

        guide.cover_step_id = Some(ids[1]);
        assert_eq!(guide.chosen_cover_step().map(|s| s.id), Some(ids[1]));
        assert_eq!(guide.cover_step().map(|s| s.id), Some(ids[1]));

        // A deleted step, or a section's id, is ignored.
        guide.cover_step_id = Some(Ulid::new());
        assert_eq!(guide.chosen_cover_step(), None);
        assert_eq!(guide.cover_step().map(|s| s.id), Some(ids[0]));
        guide.cover_step_id = Some(guide.items[0].id());
        assert_eq!(guide.chosen_cover_step(), None);

        // Steps without a screenshot are skipped.
        guide.cover_step_id = Some(ids[0]);
        for item in &mut guide.items {
            if let Item::Step(step) = item {
                if step.id == ids[0] {
                    step.image.full.clear();
                }
            }
        }
        assert_eq!(guide.chosen_cover_step(), None);
        assert_eq!(guide.cover_step().map(|s| s.id), Some(ids[1]));
    }

    #[test]
    fn review_fields_round_trip_at_the_top_level_and_default_unset() {
        let old: Guide = serde_json::from_str(include_str!("../testdata/guide.json")).unwrap();
        assert_eq!(old.review, ReviewInfo::default());
        let mut guide = Guide::new("Rotate a key", "en");
        let json = serde_json::to_value(&guide).unwrap();
        for key in [
            "owner",
            "status",
            "version",
            "lastReviewed",
            "reviewIntervalMonths",
        ] {
            assert!(json.get(key).is_none(), "{key}");
        }
        guide.review = ReviewInfo {
            owner: Some("IT Ops".into()),
            status: Some(GuideStatus::NeedsUpdate),
            version: Some("1.2".into()),
            last_reviewed: NaiveDate::from_ymd_opt(2026, 3, 31),
            review_interval_months: Some(6),
        };
        let json = serde_json::to_value(&guide).unwrap();
        assert_eq!(json["owner"], "IT Ops");
        assert_eq!(json["status"], "needsUpdate");
        assert_eq!(json["version"], "1.2");
        assert_eq!(json["lastReviewed"], "2026-03-31");
        assert_eq!(json["reviewIntervalMonths"], 6);
        assert_eq!(serde_json::from_value::<Guide>(json).unwrap(), guide);
        for status in [
            GuideStatus::Draft,
            GuideStatus::InReview,
            GuideStatus::Published,
            GuideStatus::NeedsUpdate,
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
    }

    #[test]
    fn review_is_due_after_the_interval_and_overdue_after_that_day() {
        let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        let created = day(1, 10);
        let mut review = ReviewInfo::default();
        assert_eq!(review.review_due(created), None);
        assert!(!review.is_review_overdue(created, day(12, 31)));
        // Never reviewed: due an interval after the guide was made.
        review.review_interval_months = Some(3);
        assert_eq!(review.review_due(created), Some(day(4, 10)));
        assert!(!review.is_review_overdue(created, day(4, 10)));
        assert!(review.is_review_overdue(created, day(4, 11)));
        // Month ends clamp.
        review.last_reviewed = Some(day(8, 31));
        assert_eq!(review.review_due(created), Some(day(11, 30)));
        assert!(!review.is_review_overdue(created, day(11, 30)));
        assert!(review.is_review_overdue(created, day(12, 1)));
        // 0 months is no schedule.
        review.review_interval_months = Some(0);
        assert_eq!(review.review_due(created), None);

        let mut guide = Guide::new("T", "en");
        guide.created_at = "2025-10-19T23:30:00Z".parse().unwrap();
        guide.review.review_interval_months = Some(12);
        assert_eq!(guide.review_due(), NaiveDate::from_ymd_opt(2026, 10, 19));
        assert!(!guide.is_review_overdue(day(10, 19)));
        assert!(guide.is_review_overdue(day(10, 20)));
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
