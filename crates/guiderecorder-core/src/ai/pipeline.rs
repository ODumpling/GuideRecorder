//! The AI generation pipeline over a [`Guide`]: a bounded-concurrency
//! per-step pass, then a guide-level pass over the resulting step texts. See
//! design scratchpad #442 ("Pipeline"); implemented in #350.
//!
//! [`run`] is pure orchestration over `dyn AiProvider`: it reads step images
//! through an [`ImageSource`], applies each result to the guide as it lands
//! with [`apply_step_output`] / [`apply_guide_output`], reports it through a
//! callback so the app can stream it into the editor, and returns a
//! [`PipelineSummary`].
//!
//! Rules:
//! - Steps whose [`AiStatus`] is `edited` are skipped entirely: never sent
//!   (so not scanned for redactions either), never overwritten and never
//!   touched by consistency edits. Their `AiState` keeps its own suggestions.
//! - A failed step keeps its text and status and gets
//!   [`AiState::error`]; a later successful pass clears it. A cancelled step
//!   is left as it was. Either way the rest of the run carries on.
//! - Redaction suggestions are stored in [`AiState::redaction_suggestions`]
//!   as suggestions only (Q9), replacing the AI's earlier ones and keeping
//!   those found on the device ([`crate::redact`]); they never become
//!   annotations without the user.
//! - Images are sent with the step's blurs and on-device suggestions burned
//!   in ([`burn_in_redactions`]), so what was found sensitive on the device
//!   never leaves it.
//! - Guide-level fields (title, intro, prerequisites, summary, tags) are
//!   written one by one, each only when the user has not edited it
//!   ([`Guide::is_field_edited`]). The editor marks a field edited when the
//!   user changes it (#366, #5).
//!
//! Contract for the app: [`run`] holds `&mut Guide` for the whole run, so it
//! works on its own copy. If the app keeps a separate live copy that the user
//! can edit while a run is going, it must apply each streamed
//! [`StepEvent`] to the live copy only when that step is not `edited` there,
//! and must never replace the live guide with the pipeline's copy.
//!
//! Once `cancel` is cancelled no further result is applied, even one that
//! was already ready: it is reported as cancelled instead.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use image::codecs::webp::WebPEncoder;
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, RgbaImage};
use ulid::Ulid;

use super::concurrency::{for_each_bounded, DEFAULT_STEP_CONCURRENCY};
use super::{
    fit_for_api, AiError, AiProvider, GuideInput, GuideOutput, GuideStepText, ImageData,
    ImageMediaType, RedactionSuggestion, StepInput, StepOutput, SuggestionSource, Usage,
};
use crate::export::raster::locate_crop;
use crate::library::GuideHandle;
use crate::model::{AiState, AiStatus, Guide, GuideField, Item, Point, Rect, Step};
use crate::redact::burn_in_redactions;

/// Reads a step image by its bundle-relative path (e.g. `steps/<id>.webp`).
pub trait ImageSource: Send + Sync {
    fn load(&self, relative: &str) -> Result<RgbaImage, String>;
}

impl ImageSource for GuideHandle {
    fn load(&self, relative: &str) -> Result<RgbaImage, String> {
        self.read_image(relative).map_err(|e| e.to_string())
    }
}

impl<F> ImageSource for F
where
    F: Fn(&str) -> Result<RgbaImage, String> + Send + Sync,
{
    fn load(&self, relative: &str) -> Result<RgbaImage, String> {
        self(relative)
    }
}

/// Cooperative cancellation for [`run`]. Clones share one flag; cancelling
/// stops new calls and drops in-flight ones.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<CancelInner>);

#[derive(Debug, Default)]
struct CancelInner {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Resolves once [`cancel`](Self::cancel) has been called.
    pub async fn cancelled(&self) {
        loop {
            // Created before the check, so a cancel in between still wakes it.
            let notified = self.0.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineOptions {
    /// Per-step calls in flight at once (0 is treated as 1).
    pub concurrency: usize,
    /// Send the downscaled full screenshot alongside the crop. Steps without
    /// a crop always get the full screenshot.
    pub include_full: bool,
    /// Send step images at all. Off for text-only models: the per-step pass
    /// then gets the element, keystrokes and step kind only, and no image
    /// is read.
    pub images: bool,
    /// Run the guide-level pass after the per-step pass.
    pub guide_pass: bool,
}

impl Default for PipelineOptions {
    fn default() -> Self {
        Self {
            concurrency: DEFAULT_STEP_CONCURRENCY,
            include_full: true,
            images: true,
            guide_pass: true,
        }
    }
}

/// What happened to one step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// Generated text was written to the step.
    Applied,
    /// The step is `edited`; it was left alone.
    SkippedEdited,
    /// The call failed; the step keeps its text.
    Failed(AiError),
    /// The run was cancelled before this step finished.
    Cancelled,
}

/// A step result, reported as soon as it lands.
#[derive(Debug, Clone, PartialEq)]
pub struct StepEvent {
    pub step_id: Ulid,
    /// Position among the guide's steps (sections and notes not counted).
    pub index: usize,
    pub outcome: StepOutcome,
    /// The step after applying, for [`StepOutcome::Applied`] and
    /// [`StepOutcome::Failed`] (with [`AiState::error`] set).
    pub step: Option<Step>,
    /// Also stored in the step's [`AiState::redaction_suggestions`].
    pub redaction_suggestions: Vec<RedactionSuggestion>,
}

/// Result of the guide-level pass.
#[derive(Debug, Clone, PartialEq)]
pub enum GuidePassOutcome {
    /// Not requested, or the run was cancelled first.
    NotRun,
    Applied(GuideApplied),
    Failed(AiError),
    Cancelled,
}

/// Progress reported by [`run`].
// Events are short-lived and mostly steps, so boxing would only add
// indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum PipelineEvent {
    /// The per-step pass is starting for these steps, in guide order.
    Started {
        step_ids: Vec<Ulid>,
    },
    Step(StepEvent),
    /// The guide-level pass finished. Guide fields the user has not edited
    /// and any consistency edits are already applied; a failure is
    /// recorded in [`Guide::ai_state`]'s `error`.
    Guide(GuidePassOutcome),
}

/// What [`apply_guide_output`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GuideApplied {
    /// Guide-level fields written, in [`GuideField::ALL`] order; never one
    /// the user has edited.
    pub fields: Vec<GuideField>,
    /// Steps changed by consistency edits.
    pub edited_steps: Vec<Ulid>,
    /// Consistency edits dropped because the step is `edited` or unknown.
    pub skipped_edits: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepFailure {
    pub step_id: Ulid,
    pub error: AiError,
}

/// A step image that could not be read or encoded; the step ran without it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageWarning {
    pub step_id: Ulid,
    pub path: String,
    pub message: String,
}

/// Redaction suggestions for one step, in full-screenshot pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRedactions {
    pub step_id: Ulid,
    pub suggestions: Vec<RedactionSuggestion>,
}

/// Outcome of a whole [`run`].
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineSummary {
    /// Steps in the guide.
    pub steps_total: usize,
    pub steps_applied: usize,
    pub steps_skipped_edited: usize,
    pub steps_cancelled: usize,
    pub failures: Vec<StepFailure>,
    pub guide: GuidePassOutcome,
    /// Only steps with at least one suggestion; also stored on the steps.
    pub redactions: Vec<StepRedactions>,
    pub image_warnings: Vec<ImageWarning>,
    /// Tokens used by successful calls (the provider reports none for
    /// failed ones).
    pub usage: Usage,
    pub cancelled: bool,
}

impl PipelineSummary {
    fn new(steps_total: usize) -> Self {
        Self {
            steps_total,
            steps_applied: 0,
            steps_skipped_edited: 0,
            steps_cancelled: 0,
            failures: Vec::new(),
            guide: GuidePassOutcome::NotRun,
            redactions: Vec::new(),
            image_warnings: Vec::new(),
            usage: Usage::default(),
            cancelled: false,
        }
    }
}

/// Text-only per-step input for the step at `index` among the guide's
/// steps: template text, element, keystrokes, neighbours and guide context.
/// Images are added by [`attach_images`]. `None` if there is no such step.
pub fn step_input_text(guide: &Guide, index: usize) -> Option<StepInput> {
    let steps: Vec<&Step> = guide.steps().collect();
    let step = steps.get(index)?;
    let title_at = |i: Option<usize>| i.and_then(|i| steps.get(i)).map(|s| s.title.clone());
    Some(StepInput {
        step_id: step.id,
        kind: step.kind,
        template_title: step.title.clone(),
        template_description: step.description.clone(),
        element: step.element.clone(),
        keystrokes: step.keystrokes.clone(),
        crop: None,
        full: None,
        previous_title: title_at(index.checked_sub(1)),
        next_title: title_at(Some(index + 1)),
        guide_title: (!guide.title.trim().is_empty()).then(|| guide.title.clone()),
        language: guide.language.clone(),
        audience: guide.audience,
    })
}

/// Loads `step`'s crop and full screenshot into `input`, with its blurs and
/// on-device blur suggestions burned in, downscaled to the API limits. The
/// full screenshot is attached when `include_full` is set or the step has
/// no crop. Images that fail to load or encode are left out and returned as
/// warnings. CPU-bound: call off the async runtime.
pub fn attach_images(
    input: &mut StepInput,
    step: &Step,
    images: &dyn ImageSource,
    include_full: bool,
) -> Vec<ImageWarning> {
    attach_images_until(input, step, images, include_full, None)
}

/// [`attach_images`] that stops early once `cancel` is cancelled: it checks
/// before decoding and before encoding each image. A decode or encode
/// already in progress finishes.
fn attach_images_until(
    input: &mut StepInput,
    step: &Step,
    images: &dyn ImageSource,
    include_full: bool,
    cancel: Option<&CancelToken>,
) -> Vec<ImageWarning> {
    let cancelled = || cancel.is_some_and(CancelToken::is_cancelled);
    let mut warnings = Vec::new();
    let mut warn = |path: &str, message: String| {
        warnings.push(ImageWarning {
            step_id: step.id,
            path: path.to_owned(),
            message,
        });
    };
    let mut load = |path: &str| {
        if cancelled() {
            return None;
        }
        match images.load(path) {
            Ok(image) => Some(image),
            Err(message) => {
                warn(path, message);
                None
            }
        }
    };
    let mut full = load(&step.image.full);
    let mut crop = step.image.crop.as_deref().and_then(&mut load);

    if let (Some(crop), Some(path)) = (&mut crop, step.image.crop.as_deref()) {
        if cancelled() {
            return warnings;
        }
        // Placed by the full screenshot as it is, before its blurs.
        let rect = crop_source_rect(step, crop, full.as_ref());
        burn_in_redactions(
            crop,
            Point {
                x: rect.x,
                y: rect.y,
            },
            step,
        );
        match encode_for_api(crop, rect) {
            Ok(data) => input.crop = Some(data),
            Err(message) => warn(path, message),
        }
    }
    if let Some(full) = &mut full {
        if include_full || input.crop.is_none() {
            if cancelled() {
                return warnings;
            }
            burn_in_redactions(full, Point { x: 0.0, y: 0.0 }, step);
            let rect = Rect {
                x: 0.0,
                y: 0.0,
                width: f64::from(full.width()),
                height: f64::from(full.height()),
            };
            match encode_for_api(full, rect) {
                Ok(data) => input.full = Some(data),
                Err(message) => warn(&step.image.full, message),
            }
        }
    }
    warnings
}

/// Where the crop sits in the full screenshot: the stored `crop_rect`, else
/// found by pixel match, else centred on the step's point (clamped), else the
/// top-left corner.
fn crop_source_rect(step: &Step, crop: &RgbaImage, full: Option<&RgbaImage>) -> Rect {
    if let Some(rect) = step.image.crop_rect {
        return rect;
    }
    let (cw, ch) = (f64::from(crop.width()), f64::from(crop.height()));
    let origin = full
        .and_then(|full| locate_crop(full, crop, step.point))
        .or_else(|| {
            let p = step.point?;
            let full = full?;
            let clamp =
                |v: f64, c: f64, f: u32| (v - c / 2.0).clamp(0.0, (f64::from(f) - c).max(0.0));
            Some(Point {
                x: clamp(p.x, cw, full.width()),
                y: clamp(p.y, ch, full.height()),
            })
        })
        .unwrap_or(Point { x: 0.0, y: 0.0 });
    Rect {
        x: origin.x,
        y: origin.y,
        width: cw,
        height: ch,
    }
}

/// Downscales `image` to [`fit_for_api`] and encodes it as lossless WebP.
pub fn encode_for_api(image: &RgbaImage, source_rect: Rect) -> Result<ImageData, String> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Err("empty image".into());
    }
    let (fw, fh) = fit_for_api(w, h);
    let resized;
    let image = if (fw, fh) == (w, h) {
        image
    } else {
        resized = imageops::resize(image, fw, fh, FilterType::Triangle);
        &resized
    };
    let mut bytes = Vec::new();
    WebPEncoder::new_lossless(&mut bytes)
        .encode(image.as_raw(), fw, fh, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(ImageData {
        media_type: ImageMediaType::Webp,
        bytes,
        width: fw,
        height: fh,
        source_rect,
    })
}

/// Input to the guide-level pass from the guide's current step texts.
pub fn guide_input(guide: &Guide) -> GuideInput {
    let steps: Vec<GuideStepText> = guide
        .steps()
        .map(|step| GuideStepText {
            step_id: step.id,
            kind: step.kind,
            title: step.title.clone(),
            description: step.description.clone(),
            app: step.element.as_ref().and_then(|e| e.app.clone()),
            editable: step.ai_state.status != AiStatus::Edited,
        })
        .collect();
    let mut apps: Vec<String> = match &guide.recording {
        Some(rec) if !rec.apps.is_empty() => rec.apps.iter().map(|a| a.name.clone()).collect(),
        _ => steps.iter().filter_map(|s| s.app.clone()).collect(),
    };
    let mut seen = std::collections::HashSet::new();
    apps.retain(|app| seen.insert(app.clone()));
    GuideInput {
        current_title: guide.title.clone(),
        steps,
        apps,
        language: guide.language.clone(),
        audience: guide.audience,
    }
}

fn step_mut(guide: &mut Guide, step_id: Ulid) -> Option<&mut Step> {
    guide.items.iter_mut().find_map(|item| match item {
        Item::Step(step) if step.id == step_id => Some(step),
        _ => None,
    })
}

/// Writes a per-step result to step `step_id`: title, description and
/// [`AiState`] (`ai`, model, prompt version, the redaction suggestions; no
/// error). Returns `SkippedEdited`, and changes nothing, for an `edited`
/// step; `None` if there is no such step. Suggestions are stored, not turned
/// into annotations: the step's on-device ones, then the AI's.
pub fn apply_step_output(
    guide: &mut Guide,
    step_id: Ulid,
    output: &StepOutput,
) -> Option<StepOutcome> {
    let step = step_mut(guide, step_id)?;
    if step.ai_state.status == AiStatus::Edited {
        return Some(StepOutcome::SkippedEdited);
    }
    step.title = output.title.clone();
    step.description = output.description.clone().unwrap_or_default();
    let on_device = |s: &&RedactionSuggestion| s.source == SuggestionSource::OnDevice;
    let mut redaction_suggestions: Vec<RedactionSuggestion> = step
        .ai_state
        .redaction_suggestions
        .iter()
        .filter(on_device)
        .cloned()
        .collect();
    redaction_suggestions.extend(
        output
            .redaction_suggestions
            .iter()
            .filter(|s| !on_device(s))
            .cloned(),
    );
    step.ai_state = AiState {
        status: AiStatus::Ai,
        model: Some(output.meta.model.clone()),
        prompt_version: Some(output.meta.prompt_version.clone()),
        error: None,
        redaction_suggestions,
    };
    Some(StepOutcome::Applied)
}

/// Records a failed per-step pass on step `step_id` in
/// [`AiState::error`], leaving its text and status alone. Returns
/// `SkippedEdited`, changing nothing, for an `edited` step; `None` if there
/// is no such step.
pub fn apply_step_error(guide: &mut Guide, step_id: Ulid, error: &AiError) -> Option<StepOutcome> {
    let step = step_mut(guide, step_id)?;
    if step.ai_state.status == AiStatus::Edited {
        return Some(StepOutcome::SkippedEdited);
    }
    step.ai_state.error = Some(error.to_string());
    Some(StepOutcome::Failed(error.clone()))
}

/// Writes a guide-level result. Each of title, intro, prerequisites,
/// summary and tags is written only when the user has not edited it (an
/// empty title is never written). If any was written, [`Guide::ai_state`]
/// gets the model and prompt version, and is `ai`, or still `edited` while
/// the user has edited fields. Consistency edits go to steps that are not
/// `edited`; those steps get the guide prompt's model and version, keeping
/// their suggestions and error.
pub fn apply_guide_output(guide: &mut Guide, output: &GuideOutput) -> GuideApplied {
    let mut applied = GuideApplied::default();
    for field in GuideField::ALL {
        if guide.is_field_edited(field) {
            continue;
        }
        match field {
            GuideField::Title if output.title.trim().is_empty() => continue,
            GuideField::Title => guide.title = output.title.clone(),
            GuideField::Intro => guide.intro = output.intro.clone(),
            GuideField::Prerequisites => guide.prerequisites = output.prerequisites.clone(),
            GuideField::Summary => guide.summary = output.summary.clone(),
            GuideField::Tags => guide.tags = output.tags.clone(),
        }
        applied.fields.push(field);
    }
    if !applied.fields.is_empty() {
        guide.ai_state = AiState {
            status: if guide.edited_fields.is_empty() {
                AiStatus::Ai
            } else {
                AiStatus::Edited
            },
            model: Some(output.meta.model.clone()),
            prompt_version: Some(output.meta.prompt_version.clone()),
            ..AiState::default()
        };
    }

    for edit in &output.step_edits {
        let title = edit.title.as_ref().filter(|t| !t.trim().is_empty());
        if title.is_none() && edit.description.is_none() {
            continue;
        }
        let Some(step) =
            step_mut(guide, edit.step_id).filter(|step| step.ai_state.status != AiStatus::Edited)
        else {
            applied.skipped_edits += 1;
            continue;
        };
        if let Some(title) = title {
            step.title = title.clone();
        }
        if let Some(description) = &edit.description {
            step.description = description.clone();
        }
        step.ai_state.status = AiStatus::Ai;
        step.ai_state.model = Some(output.meta.model.clone());
        step.ai_state.prompt_version = Some(output.meta.prompt_version.clone());
        applied.edited_steps.push(edit.step_id);
    }
    applied
}

enum TaskResult {
    Done(Result<StepOutput, AiError>),
    Cancelled,
}

/// Runs the per-step pass over every step that is not `edited`, then the
/// guide-level pass, applying results to `guide` as they land and reporting
/// each through `on_event`.
///
/// Images are read and encoded on blocking threads, at most
/// `options.concurrency` steps at a time. Cancelling `cancel` drops
/// in-flight calls, reports the remaining steps as cancelled and skips the
/// guide pass. Must be called inside a Tokio runtime.
pub async fn run(
    provider: Arc<dyn AiProvider>,
    guide: &mut Guide,
    images: Arc<dyn ImageSource>,
    options: &PipelineOptions,
    cancel: &CancelToken,
    mut on_event: impl FnMut(&PipelineEvent),
) -> PipelineSummary {
    let steps: Vec<Step> = guide.steps().cloned().collect();
    let mut summary = PipelineSummary::new(steps.len());

    let mut jobs = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        if step.ai_state.status == AiStatus::Edited {
            summary.steps_skipped_edited += 1;
            continue;
        }
        let input = step_input_text(guide, index).expect("index is in range");
        jobs.push((index, step.clone(), input));
    }
    let step_ids: Vec<Ulid> = jobs.iter().map(|(_, step, _)| step.id).collect();
    on_event(&PipelineEvent::Started {
        step_ids: step_ids.clone(),
    });

    let include_full = options.include_full;
    let send_images = options.images;
    for_each_bounded(
        jobs,
        options.concurrency,
        |(_, step, mut input)| {
            let provider = Arc::clone(&provider);
            let images = Arc::clone(&images);
            let cancel = cancel.clone();
            async move {
                if cancel.is_cancelled() {
                    return (TaskResult::Cancelled, Vec::new());
                }
                // Text-only: no image is read or sent.
                let (input, warnings) = if !send_images {
                    (input, Vec::new())
                } else {
                    let prepared = tokio::task::spawn_blocking({
                        let cancel = cancel.clone();
                        move || {
                            let warnings = attach_images_until(
                                &mut input,
                                &step,
                                &*images,
                                include_full,
                                Some(&cancel),
                            );
                            (input, warnings)
                        }
                    });
                    // Biased so a cancel wins over a result that is ready too.
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return (TaskResult::Cancelled, Vec::new()),
                        joined = prepared => match joined {
                            Ok(prepared) => prepared,
                            Err(err) => std::panic::resume_unwind(err.into_panic()),
                        },
                    }
                };
                let result = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => TaskResult::Cancelled,
                    result = provider.generate_step(&input) => TaskResult::Done(result),
                };
                (result, warnings)
            }
        },
        |job, (result, warnings)| {
            // A result that finished before the cancel but lands after it is
            // not applied; the tokens it used still count.
            let result = match result {
                TaskResult::Done(done) if cancel.is_cancelled() => {
                    if let Ok(output) = done {
                        summary.usage += output.meta.usage;
                    }
                    TaskResult::Cancelled
                }
                result => result,
            };
            let step_id = step_ids[job];
            let index = guide.steps().position(|s| s.id == step_id).unwrap_or(job);
            summary.image_warnings.extend(warnings);
            let mut event = StepEvent {
                step_id,
                index,
                outcome: StepOutcome::Cancelled,
                step: None,
                redaction_suggestions: Vec::new(),
            };
            match result {
                TaskResult::Cancelled => summary.steps_cancelled += 1,
                TaskResult::Done(Err(error)) => {
                    summary.failures.push(StepFailure {
                        step_id,
                        error: error.clone(),
                    });
                    if let Some(outcome) = apply_step_error(guide, step_id, &error) {
                        event.step = guide.steps().find(|s| s.id == step_id).cloned();
                        event.outcome = outcome;
                    }
                }
                TaskResult::Done(Ok(output)) => {
                    summary.usage += output.meta.usage;
                    match apply_step_output(guide, step_id, &output) {
                        Some(StepOutcome::Applied) => {
                            summary.steps_applied += 1;
                            event.outcome = StepOutcome::Applied;
                            event.step = guide.steps().find(|s| s.id == step_id).cloned();
                        }
                        _ => {
                            summary.steps_skipped_edited += 1;
                            event.outcome = StepOutcome::SkippedEdited;
                        }
                    }
                    if !output.redaction_suggestions.is_empty() {
                        summary.redactions.push(StepRedactions {
                            step_id,
                            suggestions: output.redaction_suggestions.clone(),
                        });
                    }
                    event.redaction_suggestions = output.redaction_suggestions;
                }
            }
            on_event(&PipelineEvent::Step(event));
        },
    )
    .await;

    summary.cancelled = cancel.is_cancelled();
    if options.guide_pass && !summary.cancelled {
        let input = guide_input(guide);
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = provider.generate_guide(&input) => Some(result),
        };
        let outcome = match result {
            // Checked again: nothing is applied once cancelled.
            Some(Ok(output)) if cancel.is_cancelled() => {
                summary.usage += output.meta.usage;
                GuidePassOutcome::Cancelled
            }
            Some(_) if cancel.is_cancelled() => GuidePassOutcome::Cancelled,
            None => GuidePassOutcome::Cancelled,
            Some(Ok(output)) => {
                summary.usage += output.meta.usage;
                GuidePassOutcome::Applied(apply_guide_output(guide, &output))
            }
            Some(Err(error)) => {
                guide.ai_state.error = Some(error.to_string());
                GuidePassOutcome::Failed(error)
            }
        };
        summary.cancelled = cancel.is_cancelled();
        on_event(&PipelineEvent::Guide(outcome.clone()));
        summary.guide = outcome;
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::mock::{mock_meta, MockProvider};
    use crate::ai::{
        fits_api_limit, prompts, AiFuture, GenerationMeta, RewriteInput, RewriteOutput, StepEdit,
    };
    use crate::model::{ElementInfo, ImageShow, StepImage, StepKind};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::Duration;

    fn step(title: &str, status: AiStatus) -> Step {
        let id = Ulid::new();
        Step {
            id,
            title: title.into(),
            description: String::new(),
            kind: StepKind::Click,
            point: Some(Point { x: 500.0, y: 300.0 }),
            end_point: None,
            element: Some(ElementInfo {
                app: Some("Finder".into()),
                ..ElementInfo::default()
            }),
            keystrokes: None,
            image: StepImage {
                full: format!("steps/{id}.webp"),
                crop: Some(format!("steps/{id}.crop.webp")),
                crop_rect: Some(Rect {
                    x: 300.0,
                    y: 200.0,
                    width: 400.0,
                    height: 200.0,
                }),
                after: None,
                show: ImageShow::Crop,
            },
            annotations: Vec::new(),
            ai_state: AiState {
                status,
                ..AiState::default()
            },
        }
    }

    fn guide(steps: Vec<Step>) -> Guide {
        let mut guide = Guide::new("Untitled", "en");
        guide.items = steps.into_iter().map(Item::Step).collect();
        guide
    }

    /// Full screenshots are 2880x1800, crops 400x200.
    fn images() -> Arc<dyn ImageSource> {
        Arc::new(|path: &str| {
            Ok(if path.ends_with(".crop.webp") {
                RgbaImage::new(400, 200)
            } else {
                RgbaImage::new(2880, 1800)
            })
        })
    }

    fn no_images() -> Arc<dyn ImageSource> {
        Arc::new(|path: &str| Err(format!("missing {path}")))
    }

    fn titles(guide: &Guide) -> Vec<String> {
        guide.steps().map(|s| s.title.clone()).collect()
    }

    fn text_only() -> PipelineOptions {
        PipelineOptions {
            include_full: false,
            ..PipelineOptions::default()
        }
    }

    async fn run_mock(
        mock: Arc<MockProvider>,
        guide: &mut Guide,
        images: Arc<dyn ImageSource>,
        options: &PipelineOptions,
    ) -> (PipelineSummary, Vec<PipelineEvent>) {
        let mut events = Vec::new();
        let summary = run(mock, guide, images, options, &CancelToken::new(), |e| {
            events.push(e.clone())
        })
        .await;
        (summary, events)
    }

    #[tokio::test]
    async fn applies_steps_and_skips_edited() {
        let mut guide = guide(vec![
            step("Click A", AiStatus::Template),
            step("My own words", AiStatus::Edited),
            step("Click C", AiStatus::Ai),
        ]);
        let edited_before = guide.steps().nth(1).unwrap().clone();
        let mock = Arc::new(MockProvider::new());
        let (summary, events) = run_mock(mock.clone(), &mut guide, no_images(), &text_only()).await;

        assert_eq!(mock.step_calls(), 2);
        assert_eq!(
            titles(&guide),
            ["AI: Click A", "My own words", "AI: Click C"]
        );
        assert_eq!(guide.steps().nth(1).unwrap(), &edited_before);
        for s in [0, 2] {
            let state = &guide.steps().nth(s).unwrap().ai_state;
            assert_eq!(state.status, AiStatus::Ai);
            assert_eq!(state.prompt_version.as_deref(), Some(prompts::STEP.version));
            assert_eq!(state.model.as_deref(), Some("mock"));
        }
        assert_eq!(summary.steps_total, 3);
        assert_eq!(summary.steps_applied, 2);
        assert_eq!(summary.steps_skipped_edited, 1);
        assert!(summary.failures.is_empty());
        assert!(!summary.cancelled);
        assert!(matches!(summary.guide, GuidePassOutcome::Applied(_)));
        // Started, two steps, guide.
        assert_eq!(events.len(), 4);
        let PipelineEvent::Started { step_ids } = &events[0] else {
            panic!("expected Started first");
        };
        assert_eq!(step_ids.len(), 2);
        let PipelineEvent::Step(event) = &events[1] else {
            panic!("expected a step event");
        };
        assert_eq!(event.outcome, StepOutcome::Applied);
        assert!(event.step.as_ref().unwrap().title.starts_with("AI: "));
        assert!(matches!(events[3], PipelineEvent::Guide(_)));
    }

    #[test]
    fn apply_functions_never_touch_edited_steps() {
        let mut mine = step("Mine", AiStatus::Edited);
        mine.ai_state.redaction_suggestions = vec![suggestion("kept")];
        let mut g = guide(vec![mine.clone(), step("Template", AiStatus::Template)]);
        let ids: Vec<Ulid> = g.steps().map(|s| s.id).collect();
        let out = StepOutput {
            title: "New".into(),
            description: Some("More".into()),
            redaction_suggestions: vec![suggestion("replaced")],
            meta: mock_meta("step.v1"),
        };
        let refused = AiError::Refusal { category: None };
        assert_eq!(
            apply_step_error(&mut g, ids[0], &refused),
            Some(StepOutcome::SkippedEdited)
        );
        assert_eq!(
            apply_step_output(&mut g, ids[0], &out),
            Some(StepOutcome::SkippedEdited)
        );
        assert_eq!(apply_step_output(&mut g, Ulid::new(), &out), None);
        let guide_out = GuideOutput {
            title: "Export a PDF".into(),
            intro: "Intro".into(),
            prerequisites: "- Numbers".into(),
            summary: "Export a sheet.".into(),
            tags: vec!["numbers".into(), "pdf".into()],
            step_edits: vec![
                StepEdit {
                    step_id: ids[0],
                    title: Some("Overwritten".into()),
                    description: None,
                },
                StepEdit {
                    step_id: ids[1],
                    title: Some("Consistent".into()),
                    description: Some("Why".into()),
                },
                StepEdit {
                    step_id: Ulid::new(),
                    title: Some("Ghost".into()),
                    description: None,
                },
            ],
            meta: mock_meta("guide.v1"),
        };
        let applied = apply_guide_output(&mut g, &guide_out);
        assert_eq!(applied.edited_steps, [ids[1]]);
        assert_eq!(applied.skipped_edits, 2);
        assert_eq!(titles(&g), ["Mine", "Consistent"]);
        let edited = g.steps().next().unwrap();
        assert_eq!(edited, &mine);
        let changed = g.steps().nth(1).unwrap();
        assert_eq!(changed.description, "Why");
        assert_eq!(changed.ai_state.prompt_version.as_deref(), Some("guide.v1"));
        assert_eq!(changed.ai_state.status, AiStatus::Ai);
        assert_eq!(g.title, "Export a PDF");
        assert_eq!(g.intro, "Intro");
        assert_eq!(g.prerequisites, "- Numbers");
        assert_eq!(g.summary, "Export a sheet.");
        assert_eq!(g.tags, ["numbers", "pdf"]);
        assert_eq!(applied.fields, GuideField::ALL);
        assert_eq!(g.ai_state.status, AiStatus::Ai);
        assert_eq!(g.ai_state.prompt_version.as_deref(), Some("guide.v1"));
    }

    fn suggestion(reason: &str) -> RedactionSuggestion {
        RedactionSuggestion {
            rect: Rect {
                x: 10.0,
                y: 20.0,
                width: 120.0,
                height: 16.0,
            },
            reason: reason.into(),
            source: SuggestionSource::Ai,
        }
    }

    #[tokio::test]
    async fn error_is_stored_then_cleared_and_suggestions_replaced() {
        let mut g = guide(vec![step("a", AiStatus::Template)]);
        let failing =
            Arc::new(MockProvider::new().on_step(|_| Err(AiError::Network("reset".into()))));
        let (summary, events) = run_mock(failing, &mut g, no_images(), &text_only()).await;
        let s = g.steps().next().unwrap();
        assert_eq!(s.title, "a");
        assert_eq!(s.ai_state.status, AiStatus::Template);
        assert_eq!(s.ai_state.error.as_deref(), Some("network error: reset"));
        assert_eq!(summary.failures.len(), 1);
        // The failure event carries the step with its error.
        let failed_event = events.iter().find_map(|e| match e {
            PipelineEvent::Step(event) => event.step.clone(),
            _ => None,
        });
        assert_eq!(failed_event.unwrap().ai_state.error, s.ai_state.error);

        // An earlier suggestion is replaced by the next successful pass.
        let id = s.id;
        step_mut(&mut g, id).unwrap().ai_state.redaction_suggestions = vec![suggestion("old")];
        let ok = Arc::new(MockProvider::new().on_step(|input| {
            Ok(StepOutput {
                title: format!("AI: {}", input.template_title),
                description: None,
                redaction_suggestions: vec![suggestion("new")],
                meta: mock_meta(prompts::STEP.version),
            })
        }));
        run_mock(ok, &mut g, no_images(), &text_only()).await;
        let s = g.steps().next().unwrap();
        assert_eq!(s.title, "AI: a");
        assert_eq!(s.ai_state.status, AiStatus::Ai);
        assert_eq!(s.ai_state.error, None);
        assert_eq!(s.ai_state.redaction_suggestions, [suggestion("new")]);
    }

    #[tokio::test]
    async fn edited_guide_keeps_its_fields() {
        let mut g = guide(vec![step("a", AiStatus::Template)]);
        g.title = "My title".into();
        g.intro = "My intro".into();
        g.tags = vec!["mine".into()];
        g.ai_state.status = AiStatus::Edited;
        let id = g.steps().next().unwrap().id;
        let mock = Arc::new(MockProvider::new().on_guide(move |_| {
            Ok(GuideOutput {
                title: "AI title".into(),
                intro: "AI intro".into(),
                prerequisites: "AI prereqs".into(),
                summary: "AI summary".into(),
                tags: vec!["ai".into()],
                step_edits: vec![StepEdit {
                    step_id: id,
                    title: Some("Consistent".into()),
                    description: None,
                }],
                meta: mock_meta(prompts::GUIDE.version),
            })
        }));
        let (summary, _) = run_mock(mock, &mut g, no_images(), &text_only()).await;
        assert_eq!(g.title, "My title");
        assert_eq!(g.intro, "My intro");
        assert_eq!(g.prerequisites, "");
        assert_eq!(g.summary, "");
        assert_eq!(g.tags, ["mine"]);
        assert_eq!(g.ai_state.status, AiStatus::Edited);
        assert_eq!(g.ai_state.prompt_version, None);
        // Consistency edits to editable steps still apply.
        assert_eq!(titles(&g), ["Consistent"]);
        let GuidePassOutcome::Applied(applied) = summary.guide else {
            panic!("guide pass should apply");
        };
        assert!(applied.fields.is_empty());
        assert_eq!(applied.edited_steps, [id]);
    }

    #[test]
    fn guide_fields_edited_by_hand_are_kept_and_the_rest_written() {
        let mut g = guide(vec![step("Click A", AiStatus::Template)]);
        g.intro = "My intro".into();
        g.tags = vec!["mine".into()];
        g.edited_fields = vec![GuideField::Intro, GuideField::Tags];
        g.ai_state.status = AiStatus::Edited;
        let output = GuideOutput {
            title: "AI title".into(),
            intro: "AI intro".into(),
            prerequisites: "- AI prereqs".into(),
            summary: "AI summary".into(),
            tags: vec!["ai".into()],
            step_edits: Vec::new(),
            meta: mock_meta("guide.v1"),
        };
        let applied = apply_guide_output(&mut g, &output);
        assert_eq!(
            applied.fields,
            [
                GuideField::Title,
                GuideField::Prerequisites,
                GuideField::Summary
            ]
        );
        assert_eq!(g.title, "AI title");
        assert_eq!(g.intro, "My intro");
        assert_eq!(g.prerequisites, "- AI prereqs");
        assert_eq!(g.summary, "AI summary");
        assert_eq!(g.tags, ["mine"]);
        assert_eq!(g.edited_fields, [GuideField::Intro, GuideField::Tags]);
        // Still `edited` (older apps then leave every field alone), with
        // the model of the text it did write.
        assert_eq!(g.ai_state.status, AiStatus::Edited);
        assert_eq!(g.ai_state.prompt_version.as_deref(), Some("guide.v1"));

        // Running again keeps them still.
        let again = apply_guide_output(&mut g, &output);
        assert_eq!(again.fields, applied.fields);
        assert_eq!(g.intro, "My intro");
        assert_eq!(g.tags, ["mine"]);

        // Every field edited: nothing written, state left alone.
        g.edited_fields = GuideField::ALL.to_vec();
        g.ai_state = AiState {
            status: AiStatus::Edited,
            ..AiState::default()
        };
        let none = apply_guide_output(&mut g, &output);
        assert!(none.fields.is_empty());
        assert_eq!(g.ai_state.prompt_version, None);
    }

    #[tokio::test]
    async fn guide_pass_sees_step_results_and_applies_fields() {
        let mut g = guide(vec![
            step("Click A", AiStatus::Template),
            step("Mine", AiStatus::Edited),
        ]);
        let seen = Arc::new(Mutex::new(None));
        let seen_in = Arc::clone(&seen);
        let mock = Arc::new(MockProvider::new().on_guide(move |input| {
            *seen_in.lock().unwrap() = Some(input.clone());
            Ok(GuideOutput {
                title: "Open a folder in Finder".into(),
                intro: "You will open a folder.".into(),
                prerequisites: String::new(),
                summary: "Open a folder.".into(),
                tags: vec!["finder".into()],
                step_edits: input
                    .steps
                    .iter()
                    .map(|s| StepEdit {
                        step_id: s.step_id,
                        title: Some(format!("{} (consistent)", s.title)),
                        description: None,
                    })
                    .collect(),
                meta: GenerationMeta {
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 10,
                        ..Usage::default()
                    },
                    ..mock_meta(prompts::GUIDE.version)
                },
            })
        }));
        let (summary, _) = run_mock(mock.clone(), &mut g, no_images(), &text_only()).await;

        let input = seen.lock().unwrap().clone().unwrap();
        assert_eq!(input.steps[0].title, "AI: Click A");
        assert!(input.steps[0].editable);
        assert!(!input.steps[1].editable);
        assert_eq!(input.apps, ["Finder"]);
        assert_eq!(g.title, "Open a folder in Finder");
        assert_eq!(g.summary, "Open a folder.");
        assert_eq!(g.tags, ["finder"]);
        assert_eq!(titles(&g), ["AI: Click A (consistent)", "Mine"]);
        let GuidePassOutcome::Applied(applied) = &summary.guide else {
            panic!("guide pass should apply");
        };
        assert_eq!(applied.skipped_edits, 1);
        assert_eq!(summary.usage.input_tokens, 100);
        assert_eq!(mock.guide_calls(), 1);
    }

    #[tokio::test]
    async fn one_failure_does_not_abort_the_rest() {
        let mut g = guide(vec![
            step("a", AiStatus::Template),
            step("fail", AiStatus::Template),
            step("c", AiStatus::Template),
        ]);
        let mock = Arc::new(MockProvider::new().on_step(|input| {
            if input.template_title == "fail" {
                Err(AiError::Refusal { category: None })
            } else {
                Ok(StepOutput {
                    title: input.template_title.to_uppercase(),
                    description: None,
                    redaction_suggestions: vec![RedactionSuggestion {
                        rect: Rect {
                            x: 1.0,
                            y: 2.0,
                            width: 30.0,
                            height: 10.0,
                        },
                        reason: "email address".into(),
                        source: SuggestionSource::Ai,
                    }],
                    meta: GenerationMeta {
                        usage: Usage {
                            input_tokens: 7,
                            output_tokens: 3,
                            ..Usage::default()
                        },
                        ..mock_meta(prompts::STEP.version)
                    },
                })
            }
        }));
        let (summary, events) = run_mock(mock, &mut g, no_images(), &text_only()).await;

        assert_eq!(titles(&g), ["A", "fail", "C"]);
        let failed = g.steps().nth(1).unwrap();
        assert_eq!(failed.ai_state.status, AiStatus::Template);
        assert_eq!(failed.ai_state.prompt_version, None);
        assert_eq!(
            failed.ai_state.error.as_deref(),
            Some("the model declined the request")
        );
        assert_eq!(summary.steps_applied, 2);
        assert_eq!(summary.failures.len(), 1);
        assert_eq!(summary.failures[0].step_id, failed.id);
        assert_eq!(summary.usage.input_tokens, 14);
        assert_eq!(summary.usage.output_tokens, 6);
        // Suggestions are reported and stored on the step, not applied.
        assert_eq!(summary.redactions.len(), 2);
        assert!(g.steps().all(|s| s.annotations.is_empty()));
        let first = g.steps().next().unwrap();
        assert_eq!(first.ai_state.redaction_suggestions.len(), 1);
        assert_eq!(
            first.ai_state.redaction_suggestions[0].reason,
            "email address"
        );
        assert!(failed.ai_state.redaction_suggestions.is_empty());
        assert!(events.iter().any(|e| matches!(
            e,
            PipelineEvent::Step(StepEvent {
                outcome: StepOutcome::Failed(AiError::Refusal { .. }),
                index: 1,
                ..
            })
        )));
        // The guide pass still runs.
        assert!(matches!(summary.guide, GuidePassOutcome::Applied(_)));
        // Image read failures are warnings, not step failures.
        assert_eq!(summary.image_warnings.len(), 6);
    }

    #[tokio::test]
    async fn builds_inputs_with_neighbours_and_context() {
        let mut first = step("Open **File**", AiStatus::Template);
        first.keystrokes = Some("⌘O".into());
        let mut g = guide(vec![
            first,
            step("Mine", AiStatus::Edited),
            step("Click **Open**", AiStatus::Template),
        ]);
        g.items.insert(
            1,
            Item::Section(crate::model::Section {
                id: Ulid::new(),
                title: "Section".into(),
            }),
        );
        g.title = "Open a file".into();
        g.language = "de".into();
        g.audience = crate::model::Audience::Expert;
        let mock = Arc::new(MockProvider::new());
        run_mock(mock.clone(), &mut g, no_images(), &text_only()).await;

        let mut seen = mock.seen_steps();
        seen.sort_by_key(|s| s.template_title.clone());
        let (open, file) = (&seen[0], &seen[1]);
        assert_eq!(file.template_title, "Open **File**");
        assert_eq!(file.keystrokes.as_deref(), Some("⌘O"));
        assert_eq!(file.previous_title, None);
        assert_eq!(file.next_title.as_deref(), Some("Mine"));
        assert_eq!(open.previous_title.as_deref(), Some("Mine"));
        assert_eq!(open.next_title, None);
        assert_eq!(open.guide_title.as_deref(), Some("Open a file"));
        assert_eq!(open.language, "de");
        assert_eq!(open.audience, crate::model::Audience::Expert);
        assert_eq!(
            open.element.as_ref().unwrap().app.as_deref(),
            Some("Finder")
        );
    }

    #[tokio::test]
    async fn downscales_images_to_api_limits() {
        let mut g = guide(vec![step("Click A", AiStatus::Template)]);
        let crop_rect = g.steps().next().unwrap().image.crop_rect.unwrap();
        let mock = Arc::new(MockProvider::new());
        let (summary, _) =
            run_mock(mock.clone(), &mut g, images(), &PipelineOptions::default()).await;
        assert!(summary.image_warnings.is_empty());

        let input = &mock.seen_steps()[0];
        let full = input.full.as_ref().unwrap();
        assert!(fits_api_limit(full.width, full.height));
        assert_eq!((full.width, full.height), fit_for_api(2880, 1800));
        assert_eq!(full.media_type, ImageMediaType::Webp);
        assert_eq!(
            full.source_rect,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 2880.0,
                height: 1800.0
            }
        );
        let decoded = image::load_from_memory(&full.bytes).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (full.width, full.height)
        );

        let crop = input.crop.as_ref().unwrap();
        assert_eq!((crop.width, crop.height), (400, 200));
        assert_eq!(crop.source_rect, crop_rect);
    }

    #[tokio::test]
    async fn text_only_models_get_no_images() {
        let mut first = step("Type your name", AiStatus::Template);
        first.kind = StepKind::Type;
        first.keystrokes = Some("Ada".into());
        let mut g = guide(vec![first]);
        let read = Arc::new(AtomicUsize::new(0));
        let counted: Arc<dyn ImageSource> = Arc::new({
            let read = Arc::clone(&read);
            move |_: &str| {
                read.fetch_add(1, Ordering::SeqCst);
                Ok(RgbaImage::new(400, 200))
            }
        });
        let mock = Arc::new(MockProvider::new());
        let options = PipelineOptions {
            images: false,
            ..PipelineOptions::default()
        };
        let (summary, _) = run_mock(mock.clone(), &mut g, counted, &options).await;
        assert_eq!(summary.steps_applied, 1);
        assert_eq!(read.load(Ordering::SeqCst), 0);
        let input = &mock.seen_steps()[0];
        assert!(input.crop.is_none() && input.full.is_none());
        // The text the model works from instead.
        assert_eq!(input.kind, StepKind::Type);
        assert_eq!(input.keystrokes.as_deref(), Some("Ada"));
        assert!(input.element.is_some());
    }

    #[test]
    fn full_screenshot_is_kept_without_a_crop_and_crop_is_located() {
        let mut s = step("Click", AiStatus::Template);
        s.image.crop_rect = None;
        let mut input = step_input_text(&guide(vec![s.clone()]), 0).unwrap();
        // No crop_rect: centred on the point (500, 300) for a blank image.
        let warnings = attach_images(&mut input, &s, &*images(), false);
        assert!(warnings.is_empty());
        assert!(input.full.is_none());
        let crop = input.crop.unwrap();
        assert_eq!(
            crop.source_rect,
            Rect {
                x: 300.0,
                y: 200.0,
                width: 400.0,
                height: 200.0
            }
        );

        s.image.crop = None;
        let mut input = step_input_text(&guide(vec![s.clone()]), 0).unwrap();
        attach_images(&mut input, &s, &*images(), false);
        assert!(input.crop.is_none());
        assert!(input.full.is_some());
    }

    #[test]
    fn images_are_sent_with_blurs_and_on_device_suggestions_burned_in() {
        // Vertical stripes, which any blur turns grey.
        let stripes = |w, h| {
            RgbaImage::from_fn(w, h, |x, _| {
                let v = if x % 2 == 0 { 0 } else { 255 };
                image::Rgba([v, v, v, 255])
            })
        };
        let source: Arc<dyn ImageSource> = Arc::new(move |path: &str| {
            Ok(if path.ends_with(".crop.webp") {
                stripes(400, 200)
            } else {
                stripes(800, 600)
            })
        });
        let mut s = step("Click", AiStatus::Template);
        let at = |x, y| Rect {
            x,
            y,
            width: 40.0,
            height: 20.0,
        };
        s.annotations.push(crate::model::Annotation {
            id: Ulid::new(),
            shape: crate::model::Shape::Blur(at(600.0, 500.0)),
            style: Default::default(),
        });
        s.ai_state.redaction_suggestions = vec![
            RedactionSuggestion {
                rect: at(350.0, 250.0),
                reason: "email address".into(),
                source: SuggestionSource::OnDevice,
            },
            RedactionSuggestion {
                rect: at(10.0, 10.0),
                reason: "name".into(),
                source: SuggestionSource::Ai,
            },
        ];
        let mut input = step_input_text(&guide(vec![s.clone()]), 0).unwrap();
        assert!(attach_images(&mut input, &s, &*source, true).is_empty());
        let decode = |data: &ImageData| image::load_from_memory(&data.bytes).unwrap().to_rgba8();
        let grey = |image: &RgbaImage, x, y| !matches!(image.get_pixel(x, y).0[0], 0 | 255);

        let full = decode(input.full.as_ref().unwrap());
        assert_eq!(full.dimensions(), (800, 600));
        assert!(grey(&full, 610, 510), "the blur");
        assert!(grey(&full, 360, 255), "the on-device suggestion");
        assert!(!grey(&full, 20, 15), "the AI's suggestion is not burned in");
        assert!(!grey(&full, 200, 300));
        // The crop sits at (300, 200): the suggestion is at (50, 50) in it.
        let crop = decode(input.crop.as_ref().unwrap());
        assert!(grey(&crop, 60, 55));
        assert!(!grey(&crop, 10, 10));
    }

    #[test]
    fn on_device_suggestions_survive_an_ai_pass() {
        let mut s = step("a", AiStatus::Template);
        let found = RedactionSuggestion {
            source: SuggestionSource::OnDevice,
            ..suggestion("card number")
        };
        s.ai_state.redaction_suggestions = vec![suggestion("old"), found.clone()];
        let id = s.id;
        let mut g = guide(vec![s]);
        let out = StepOutput {
            title: "New".into(),
            description: None,
            redaction_suggestions: vec![suggestion("new")],
            meta: mock_meta("step.v1"),
        };
        assert_eq!(
            apply_step_output(&mut g, id, &out),
            Some(StepOutcome::Applied)
        );
        let kept = &g.steps().next().unwrap().ai_state.redaction_suggestions;
        assert_eq!(kept, &[found, suggestion("new")]);
    }

    /// Delegates to a [`MockProvider`] after a delay, tracking calls in flight.
    struct Slow {
        inner: MockProvider,
        delay: Duration,
        active: AtomicUsize,
        peak: AtomicUsize,
        finished: AtomicUsize,
    }

    impl Slow {
        fn new(delay: Duration) -> Self {
            Self {
                inner: MockProvider::new(),
                delay,
                active: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                finished: AtomicUsize::new(0),
            }
        }
    }

    impl AiProvider for Slow {
        fn generate_step<'a>(&'a self, input: &'a StepInput) -> AiFuture<'a, StepOutput> {
            Box::pin(async move {
                let now = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(self.delay).await;
                self.active.fetch_sub(1, Ordering::SeqCst);
                self.finished.fetch_add(1, Ordering::SeqCst);
                self.inner.generate_step(input).await
            })
        }

        fn generate_guide<'a>(&'a self, input: &'a GuideInput) -> AiFuture<'a, GuideOutput> {
            self.inner.generate_guide(input)
        }

        fn rewrite<'a>(&'a self, input: &'a RewriteInput) -> AiFuture<'a, RewriteOutput> {
            self.inner.rewrite(input)
        }
    }

    #[tokio::test]
    async fn concurrency_is_bounded() {
        let mut g = guide(
            (0..12)
                .map(|i| step(&format!("Step {i}"), AiStatus::Template))
                .collect(),
        );
        let slow = Arc::new(Slow::new(Duration::from_millis(15)));
        let options = PipelineOptions {
            concurrency: 3,
            ..text_only()
        };
        let summary = run(
            slow.clone(),
            &mut g,
            no_images(),
            &options,
            &CancelToken::new(),
            |_| {},
        )
        .await;
        assert_eq!(summary.steps_applied, 12);
        assert_eq!(slow.peak.load(Ordering::SeqCst), 3);
        assert!(g.steps().all(|s| s.title.starts_with("AI: ")));
    }

    #[tokio::test]
    async fn cancellation_stops_steps_and_skips_guide_pass() {
        let mut g = guide(
            (0..10)
                .map(|i| step(&format!("Step {i}"), AiStatus::Template))
                .collect(),
        );
        let slow = Arc::new(Slow::new(Duration::from_millis(20)));
        let cancel = CancelToken::new();
        let options = PipelineOptions {
            concurrency: 2,
            ..text_only()
        };
        let mut events = Vec::new();
        let summary = run(slow.clone(), &mut g, no_images(), &options, &cancel, |e| {
            if matches!(e, PipelineEvent::Step(_)) {
                cancel.cancel();
            }
            events.push(e.clone());
        })
        .await;

        assert!(summary.cancelled);
        // The first result cancels; a peer that finishes in the same instant
        // is not applied.
        assert_eq!(summary.steps_applied, 1);
        assert_eq!(summary.steps_applied + summary.steps_cancelled, 10);
        assert_eq!(summary.guide, GuidePassOutcome::NotRun);
        assert!(slow.finished.load(Ordering::SeqCst) <= 2);
        assert_eq!(slow.inner.guide_calls(), 0);
        let applied = g
            .steps()
            .filter(|s| s.ai_state.status == AiStatus::Ai)
            .count();
        assert_eq!(applied, summary.steps_applied);
        // Started + one event per step; no guide event.
        assert_eq!(events.len(), 11);
        assert!(!events.iter().any(|e| matches!(e, PipelineEvent::Guide(_))));
    }

    #[tokio::test]
    async fn cancel_token_wakes_waiters() {
        let token = CancelToken::new();
        let waiter = tokio::spawn({
            let token = token.clone();
            async move { token.cancelled().await }
        });
        tokio::task::yield_now().await;
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
        // Already cancelled: resolves at once.
        token.cancelled().await;
    }

    #[tokio::test]
    async fn guide_pass_failure_is_reported() {
        let mut g = guide(vec![step("a", AiStatus::Template)]);
        let mock =
            Arc::new(MockProvider::new().on_guide(|_| Err(AiError::Network("reset".into()))));
        let (summary, _) = run_mock(mock, &mut g, no_images(), &text_only()).await;
        assert_eq!(
            summary.guide,
            GuidePassOutcome::Failed(AiError::Network("reset".into()))
        );
        assert_eq!(g.title, "Untitled");
        assert_eq!(g.ai_state.error.as_deref(), Some("network error: reset"));
        assert_eq!(g.ai_state.status, AiStatus::Template);
        assert_eq!(summary.steps_applied, 1);
    }

    fn cancel_on_guide_and_step() -> (CancelToken, MockProvider) {
        let cancel = CancelToken::new();
        let (on_step, on_guide) = (cancel.clone(), cancel.clone());
        let mock = MockProvider::new()
            .on_step(move |input| {
                // The result is ready, and so is the cancel.
                on_step.cancel();
                Ok(StepOutput {
                    title: format!("AI: {}", input.template_title),
                    description: None,
                    redaction_suggestions: vec![suggestion("late")],
                    meta: mock_meta(prompts::STEP.version),
                })
            })
            .on_guide(move |_| {
                on_guide.cancel();
                Ok(GuideOutput {
                    title: "Late title".into(),
                    intro: "Late".into(),
                    prerequisites: String::new(),
                    summary: String::new(),
                    tags: Vec::new(),
                    step_edits: Vec::new(),
                    meta: mock_meta(prompts::GUIDE.version),
                })
            });
        (cancel, mock)
    }

    #[tokio::test]
    async fn ready_step_result_is_not_applied_once_cancelled() {
        let mut g = guide(vec![step("a", AiStatus::Template)]);
        let before = g.clone();
        let (cancel, mock) = cancel_on_guide_and_step();
        let mock = Arc::new(mock);
        let mut events = Vec::new();
        let summary = run(
            mock.clone(),
            &mut g,
            no_images(),
            &text_only(),
            &cancel,
            |e| events.push(e.clone()),
        )
        .await;
        assert_eq!(mock.step_calls(), 1);
        assert_eq!(g, before);
        assert!(summary.cancelled);
        assert_eq!(summary.steps_applied, 0);
        assert_eq!(summary.steps_cancelled, 1);
        assert!(summary.redactions.is_empty());
        assert_eq!(summary.guide, GuidePassOutcome::NotRun);
        assert!(events.iter().any(|e| matches!(
            e,
            PipelineEvent::Step(StepEvent {
                outcome: StepOutcome::Cancelled,
                step: None,
                ..
            })
        )));
    }

    #[tokio::test]
    async fn finished_peer_is_not_applied_after_cancel() {
        let mut g = guide(vec![
            step("a", AiStatus::Template),
            step("b", AiStatus::Template),
        ]);
        let cancel = CancelToken::new();
        let options = PipelineOptions {
            concurrency: 2,
            ..text_only()
        };
        let mock = Arc::new(MockProvider::new());
        let summary = run(mock.clone(), &mut g, no_images(), &options, &cancel, |e| {
            if matches!(e, PipelineEvent::Step(_)) {
                cancel.cancel();
            }
        })
        .await;
        assert_eq!(summary.steps_applied, 1);
        assert_eq!(summary.steps_cancelled, 1);
        let applied = g
            .steps()
            .filter(|s| s.ai_state.status == AiStatus::Ai)
            .count();
        assert_eq!(applied, 1);
    }

    #[tokio::test]
    async fn ready_guide_result_is_not_applied_once_cancelled() {
        let mut g = guide(vec![step("a", AiStatus::Edited)]);
        let (cancel, mock) = cancel_on_guide_and_step();
        let mock = Arc::new(mock);
        let mut events = Vec::new();
        let summary = run(
            mock.clone(),
            &mut g,
            no_images(),
            &text_only(),
            &cancel,
            |e| events.push(e.clone()),
        )
        .await;
        assert_eq!(mock.guide_calls(), 1);
        assert_eq!(summary.guide, GuidePassOutcome::Cancelled);
        assert!(summary.cancelled);
        assert_eq!(g.title, "Untitled");
        assert_eq!(g.intro, "");
        assert_eq!(g.ai_state.status, AiStatus::Template);
        assert!(events.contains(&PipelineEvent::Guide(GuidePassOutcome::Cancelled)));
    }

    #[tokio::test]
    async fn cancel_during_image_prep_stops_early_and_skips_the_call() {
        let mut g = guide(vec![step("a", AiStatus::Template)]);
        let cancel = CancelToken::new();
        let loads = Arc::new(AtomicUsize::new(0));
        let images: Arc<dyn ImageSource> = Arc::new({
            let (cancel, loads) = (cancel.clone(), Arc::clone(&loads));
            move |_: &str| {
                loads.fetch_add(1, Ordering::SeqCst);
                cancel.cancel();
                Ok(RgbaImage::new(2880, 1800))
            }
        });
        let mock = Arc::new(MockProvider::new());
        let summary = run(
            mock.clone(),
            &mut g,
            images,
            &PipelineOptions::default(),
            &cancel,
            |_| {},
        )
        .await;
        // The full screenshot was read; the crop was not, and nothing was
        // encoded or sent.
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(mock.step_calls(), 0);
        assert_eq!(summary.steps_cancelled, 1);
        assert_eq!(titles(&g), ["a"]);
    }

    #[test]
    fn encode_failures_are_warnings() {
        let s = step("Click", AiStatus::Template);
        let mut input = step_input_text(&guide(vec![s.clone()]), 0).unwrap();
        let empty = |_: &str| Ok(RgbaImage::new(0, 0));
        let warnings = attach_images(&mut input, &s, &empty, true);
        assert!(input.crop.is_none());
        assert!(input.full.is_none());
        let paths: Vec<&str> = warnings.iter().map(|w| w.path.as_str()).collect();
        assert_eq!(paths, [s.image.crop.as_deref().unwrap(), &s.image.full]);
        assert!(warnings
            .iter()
            .all(|w| w.step_id == s.id && w.message == "empty image"));
    }
}
