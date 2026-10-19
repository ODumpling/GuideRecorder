//! AI writing for an open guide (#350): `ai_generate` runs the core
//! pipeline (`guiderecorder_core::ai::pipeline`) in the background on the guide
//! as last saved and streams each result to the editor as `ai://` events;
//! `ai_cancel` stops a run. `ai_rewrite_step` rewrites one step's text and
//! returns it (no events).
//!
//! The editor is the only writer of an open guide. It applies each result
//! to its live copy, skipping steps that are `edited` there, and autosaves.
//! Nothing here writes the guide.
//!
//! Events, in order, each carrying `runId` and `guideId`:
//! - `ai://started` once, with the steps the per-step pass will write;
//! - `ai://step` for each of them as it lands, then once more for each step
//!   the guide pass changed by a consistency edit;
//! - `ai://guide` once, with `fields` null unless the guide pass wrote
//!   them (not requested, cancelled, failed, or the guide is `edited`);
//! - `ai://finished` once, after the run left the registry, so a new run of
//!   the guide may start as soon as it arrives.
//!
//! A run that breaks off (a panic in the pipeline) still sends `ai://guide`
//! and `ai://finished`, both with `error` set to [`RUN_FAILED`].

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guiderecorder_core::ai::claude::{ClaudeConfig, ClaudeProvider};
use guiderecorder_core::ai::openai::{OpenAiConfig, OpenAiProvider};
use guiderecorder_core::ai::pipeline::{self, GuidePassOutcome, StepOutcome};
use guiderecorder_core::ai::{
    keychain, AiError, AiProvider, ApiKey, CancelToken, ImageSource, PipelineEvent,
    PipelineOptions, PipelineSummary, RedactionSuggestion, RewriteInput, RewriteStyle,
};
use guiderecorder_core::library::GuideHandle;
use guiderecorder_core::model::{AiState, AiStatus, Guide, GuideField, GuideKind, Item, Step};
use guiderecorder_core::Ulid;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::blocking::blocking;
use crate::editor::{
    check_image_path, parse_id, AppError, ErrorKind, Result, Session, SessionCell,
};
use crate::settings::{self, Provider, SettingsStore};

pub const STARTED_EVENT: &str = "ai://started";
pub const STEP_EVENT: &str = "ai://step";
pub const GUIDE_EVENT: &str = "ai://guide";
pub const FINISHED_EVENT: &str = "ai://finished";

pub const AI_OFF: &str = "AI is turned off in Settings.";
pub const NO_KEY: &str = "Add an API key in Settings to write with AI.";
pub const NO_MODEL: &str = "Choose a step model and a guide model in Settings to write with AI.";
pub const NO_BASE_URL: &str = "Add the API's base URL in Settings to write with AI.";
pub const BUSY: &str = "Already writing this guide.";
/// An annotated guide's pages have no clicks for the prompts to describe.
pub const NOT_RECORDED: &str = "AI writing is for recorded guides.";
/// `error` of `ai://guide` and `ai://finished` when a run breaks off.
pub const RUN_FAILED: &str = "Writing stopped because of an internal error.";

/// `ai_generate`'s options. Defaults: every step that is not `edited`, and
/// the guide pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GenerateOptions {
    /// Steps to write; others are left alone (and kept out of the guide
    /// pass's consistency edits). `edited` steps are skipped unless `force`.
    /// Empty: no per-step pass.
    pub step_ids: Option<Vec<String>>,
    pub guide_pass: Option<bool>,
    /// An explicit request from the user: the steps in `step_ids`, and with
    /// the guide pass the guide's fields, are written even when `edited`.
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStarted {
    pub run_id: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartedPayload {
    pub run_id: u64,
    pub guide_id: String,
    pub step_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Applied,
    Failed,
    SkippedEdited,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepPayload {
    pub run_id: u64,
    pub guide_id: String,
    pub step_id: String,
    pub outcome: Outcome,
    /// The step as the pipeline left it: for `applied`, and for `failed`
    /// (its text unchanged, `aiState.error` set).
    pub step: Option<Step>,
    pub redaction_suggestions: Vec<RedactionSuggestion>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuideFields {
    pub title: String,
    pub intro: String,
    pub prerequisites: String,
    pub summary: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GuidePayload {
    pub run_id: u64,
    pub guide_id: String,
    pub fields: Option<GuideFields>,
    /// The fields the pass wrote, when `fields` is set: never one the user
    /// edited (unless forced). The others in `fields` are as they were.
    pub written: Vec<GuideField>,
    /// The guide's `aiState` after the pass: with model and prompt version
    /// when `fields` is set (`ai`, or `edited` while the user has edited
    /// fields), the error when it failed, else as it was.
    pub ai_state: AiState,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishedPayload {
    pub run_id: u64,
    pub guide_id: String,
    pub cancelled: bool,
    /// Per-step pass counts, over the requested steps only.
    pub applied: usize,
    pub failed: usize,
    pub skipped_edited: usize,
    pub usage: TokenUsage,
    /// Set when the run broke off on an internal error ([`RUN_FAILED`]);
    /// the counts are then zero and the results already sent stand.
    pub error: Option<String>,
}

/// One `ai://` event; serializes as its payload.
// Mostly steps and short-lived, as in core's `PipelineEvent`.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum AiEvent {
    Started(StartedPayload),
    Step(StepPayload),
    Guide(GuidePayload),
    Finished(FinishedPayload),
}

impl AiEvent {
    pub fn name(&self) -> &'static str {
        match self {
            AiEvent::Started(_) => STARTED_EVENT,
            AiEvent::Step(_) => STEP_EVENT,
            AiEvent::Guide(_) => GUIDE_EVENT,
            AiEvent::Finished(_) => FINISHED_EVENT,
        }
    }
}

/// Where a run's events go: the webview in the app, a list in the tests.
pub trait EventSink: Send + Sync + 'static {
    fn send(&self, event: AiEvent);
}

impl<R: Runtime> EventSink for AppHandle<R> {
    fn send(&self, event: AiEvent) {
        if let Err(e) = self.emit(event.name(), &event) {
            eprintln!("guiderecorder: cannot send {}: {e}", event.name());
        }
    }
}

/// The runs going on: run id → its guide and cancel token. One run per
/// guide at a time.
#[derive(Default)]
pub struct AiRuns {
    next_id: AtomicU64,
    runs: Mutex<HashMap<u64, Run>>,
}

struct Run {
    guide_id: Ulid,
    cancel: CancelToken,
}

impl AiRuns {
    /// Registers a run of `guide_id`; [`BUSY`] if one is going on.
    fn begin(self: &Arc<Self>, guide_id: Ulid) -> Result<RunGuard> {
        let mut runs = self.runs.lock().unwrap();
        if runs.values().any(|run| run.guide_id == guide_id) {
            return Err(AppError::new(ErrorKind::Invalid, BUSY));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = CancelToken::new();
        runs.insert(
            id,
            Run {
                guide_id,
                cancel: cancel.clone(),
            },
        );
        Ok(RunGuard {
            runs: Arc::clone(self),
            id,
            cancel,
        })
    }

    /// Cancels run `run_id`; nothing if it has finished or never existed.
    pub fn cancel(&self, run_id: u64) {
        if let Some(run) = self.runs.lock().unwrap().get(&run_id) {
            run.cancel.cancel();
        }
    }

    /// Cancels every run; returns how many there were.
    pub fn cancel_all(&self) -> usize {
        let runs = self.runs.lock().unwrap();
        for run in runs.values() {
            run.cancel.cancel();
        }
        runs.len()
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.runs.lock().unwrap().len()
    }
}

/// A registered run; dropping it (also on a panic) removes it.
struct RunGuard {
    runs: Arc<AiRuns>,
    id: u64,
    cancel: CancelToken,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.runs.runs.lock().unwrap().remove(&self.id);
    }
}

/// The API key to write with, checking the settings first: [`AI_OFF`] when
/// AI is off, [`NO_BASE_URL`] or [`NO_MODEL`] for an OpenAI-compatible
/// provider not set up yet (the Keychain is not asked then), [`NO_KEY`]
/// without the Anthropic key. `key` reads the Keychain entry it is given.
/// `None` without a key: local providers never ask, and a custom API's key
/// is optional.
pub fn check_ready(
    ai: &settings::Ai,
    key: impl FnOnce(&str) -> Result<Option<ApiKey>, AiError>,
) -> Result<Option<ApiKey>> {
    if !ai.enabled {
        return Err(AppError::new(ErrorKind::Invalid, AI_OFF));
    }
    if let Some(endpoint) = ai.endpoint() {
        if ai.provider == Provider::OpenAiCompatible && endpoint.base_url.trim().is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, NO_BASE_URL));
        }
        if endpoint.step_model.trim().is_empty() || endpoint.guide_model.trim().is_empty() {
            return Err(AppError::new(ErrorKind::Invalid, NO_MODEL));
        }
    }
    let keychain_error = |e: AiError| AppError::new(ErrorKind::Io, e.to_string());
    match ai.provider {
        Provider::Anthropic => match key(keychain::ACCOUNT) {
            Ok(Some(key)) => Ok(Some(key)),
            Ok(None) | Err(AiError::MissingApiKey) => {
                Err(AppError::new(ErrorKind::Invalid, NO_KEY))
            }
            Err(e) => Err(keychain_error(e)),
        },
        Provider::OpenAiCompatible => match key(keychain::OPENAI_COMPATIBLE_ACCOUNT) {
            Ok(key) => Ok(key),
            Err(AiError::MissingApiKey) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        },
        Provider::Ollama | Provider::LmStudio => Ok(None),
    }
}

/// The provider in Settings, with the key [`check_ready`] returned.
pub fn provider(ai: &settings::Ai, key: Option<ApiKey>) -> Result<Arc<dyn AiProvider>> {
    let io = |e: AiError| AppError::new(ErrorKind::Io, e.to_string());
    Ok(match (openai_config(ai), key) {
        (Some(config), key) => Arc::new(OpenAiProvider::new(key, config).map_err(io)?),
        (None, Some(key)) => Arc::new(ClaudeProvider::new(key, claude_config(ai)).map_err(io)?),
        (None, None) => return Err(AppError::new(ErrorKind::Invalid, NO_KEY)),
    })
}

/// The OpenAI-compatible configuration for the provider in Settings;
/// `None` for Anthropic. An empty base URL is the local preset's.
pub fn openai_config(ai: &settings::Ai) -> Option<OpenAiConfig> {
    let e = ai.endpoint()?;
    let (step, guide) = (e.step_model.trim(), e.guide_model.trim());
    let mut config = match ai.provider {
        Provider::Ollama => OpenAiConfig::ollama(step, guide),
        Provider::LmStudio => OpenAiConfig::lm_studio(step, guide),
        _ => OpenAiConfig::new(e.base_url.trim(), step, guide),
    };
    if !e.base_url.trim().is_empty() {
        config.base_url = e.base_url.trim().to_owned();
    }
    Some(config)
}

/// The Claude configuration for the models in Settings.
pub fn claude_config(ai: &settings::Ai) -> ClaudeConfig {
    let mut config = ClaudeConfig::default();
    if !ai.step_model.trim().is_empty() {
        config.step.model = ai.step_model.clone();
        config.rewrite.model = ai.step_model.clone();
    }
    if !ai.guide_model.trim().is_empty() {
        config.guide.model = ai.guide_model.clone();
    }
    // Haiku rejects `fallbacks` (see `ModelSettings::server_fallback`).
    config.guide.server_fallback &= !config.guide.model.contains("haiku");
    config
}

/// Step images of a guide bundle, by the paths in its `guide.json`: only
/// WebP files under `steps/` or `assets/`, inside the bundle.
pub struct BundleImages(pub GuideHandle);

impl ImageSource for BundleImages {
    fn load(&self, relative: &str) -> Result<RgbaImage, String> {
        check_image_path(relative).map_err(|e| e.message)?;
        self.0.read_image(relative).map_err(|e| e.to_string())
    }
}

/// Registers a run of `guide` (the saved snapshot) and returns its id and
/// the future that runs it and sends its events. Errors, before anything is
/// registered: a step id that is invalid or not in `guide`; [`BUSY`].
pub fn begin(
    runs: &Arc<AiRuns>,
    provider: Arc<dyn AiProvider>,
    mut guide: Guide,
    images: Arc<dyn ImageSource>,
    options: &GenerateOptions,
    ai: &settings::Ai,
    sink: Arc<dyn EventSink>,
) -> Result<(u64, impl Future<Output = ()> + Send + 'static)> {
    let guide_pass = options.guide_pass.unwrap_or(true);
    let (unselected, forced) =
        select_steps(&mut guide, options.step_ids.as_deref(), options.force)?;
    let forced = Forced {
        guide: options.force
            && guide_pass
            && GuideField::ALL
                .into_iter()
                .any(|field| guide.is_field_edited(field)),
        ..forced
    };
    if forced.guide {
        guide.ai_state.status = AiStatus::Template;
        guide.edited_fields.clear();
    }
    if !ai.language.trim().is_empty() {
        guide.language = ai.language.clone();
    }
    guide.audience = ai.audience;
    let guard = runs.begin(guide.id)?;
    let run_id = guard.id;
    let pipeline_options = PipelineOptions {
        guide_pass,
        images: ai.images(),
        ..PipelineOptions::default()
    };
    let task = drive(
        guard,
        provider,
        guide,
        images,
        pipeline_options,
        Selection { unselected, forced },
        sink,
    );
    Ok((run_id, task))
}

/// How the snapshot was changed for a run.
struct Selection {
    /// Steps left out by `stepIds`, marked `edited` for the run.
    unselected: usize,
    forced: Forced,
}

/// What `force` un-marked `edited` in the snapshot. A result that does not
/// replace their text reports them `edited` again, as they are.
#[derive(Default)]
struct Forced {
    steps: HashSet<Ulid>,
    guide: bool,
}

impl Forced {
    fn restore_step(&self, step: &mut Step) {
        if self.steps.contains(&step.id) {
            step.ai_state.status = AiStatus::Edited;
        }
    }

    fn restore_guide(&self, ai_state: &mut AiState) {
        if self.guide {
            ai_state.status = AiStatus::Edited;
        }
    }
}

/// Marks the steps not in `step_ids` `edited` in the snapshot, so the
/// pipeline neither writes them nor edits them for consistency; with
/// `force`, un-marks the ones in it. Returns how many steps were marked, and
/// those un-marked.
fn select_steps(
    guide: &mut Guide,
    step_ids: Option<&[String]>,
    force: bool,
) -> Result<(usize, Forced)> {
    let mut forced = Forced::default();
    let Some(step_ids) = step_ids else {
        return Ok((0, forced));
    };
    let mut wanted = HashSet::new();
    for id in step_ids {
        let id = parse_id(id)?;
        if !guide.steps().any(|step| step.id == id) {
            return Err(AppError::new(
                ErrorKind::Invalid,
                format!("step {id} is not in the saved guide"),
            ));
        }
        wanted.insert(id);
    }
    let mut unselected = 0;
    for item in &mut guide.items {
        if let Item::Step(step) = item {
            if !wanted.contains(&step.id) {
                step.ai_state.status = AiStatus::Edited;
                unselected += 1;
            } else if force && step.ai_state.status == AiStatus::Edited {
                step.ai_state.status = AiStatus::Template;
                forced.steps.insert(step.id);
            }
        }
    }
    Ok((unselected, forced))
}

async fn drive(
    guard: RunGuard,
    provider: Arc<dyn AiProvider>,
    mut guide: Guide,
    images: Arc<dyn ImageSource>,
    options: PipelineOptions,
    selection: Selection,
    sink: Arc<dyn EventSink>,
) {
    let run_id = guard.id;
    let guide_id = guide.id.to_string();
    let cancel = guard.cancel.clone();
    let mut ai_state = guide.ai_state.clone();
    selection.forced.restore_guide(&mut ai_state);
    let selection = Arc::new(selection);
    // On its own task, so a panic in the pipeline still ends the run with
    // `ai://guide` and `ai://finished`.
    let pipeline = tauri::async_runtime::spawn({
        let sink = Arc::clone(&sink);
        let guide_id = guide_id.clone();
        let cancel = cancel.clone();
        let selection = Arc::clone(&selection);
        async move {
            let summary = pipeline::run(provider, &mut guide, images, &options, &cancel, |event| {
                if let Some(event) = pipeline_event(run_id, &guide_id, event, &selection.forced) {
                    sink.send(event);
                }
            })
            .await;
            (guide, summary)
        }
    })
    .await;
    let finished = match pipeline {
        Ok((guide, summary)) => {
            for event in closing_events(run_id, &guide, &summary, &selection.forced) {
                sink.send(event);
            }
            finished(run_id, guide_id, &summary, selection.unselected)
        }
        Err(e) => {
            eprintln!("guiderecorder: AI run {run_id} failed: {e}");
            sink.send(AiEvent::Guide(GuidePayload {
                run_id,
                guide_id: guide_id.clone(),
                fields: None,
                written: Vec::new(),
                ai_state,
                error: Some(RUN_FAILED.into()),
            }));
            FinishedPayload {
                run_id,
                guide_id,
                cancelled: cancel.is_cancelled(),
                applied: 0,
                failed: 0,
                skipped_edited: 0,
                usage: TokenUsage::default(),
                error: Some(RUN_FAILED.into()),
            }
        }
    };
    drop(guard);
    sink.send(AiEvent::Finished(finished));
}

/// The event for a pipeline event; the guide pass is sent after the run
/// (see [`closing_events`]), when the guide can be read.
fn pipeline_event(
    run_id: u64,
    guide_id: &str,
    event: &PipelineEvent,
    forced: &Forced,
) -> Option<AiEvent> {
    match event {
        PipelineEvent::Started { step_ids } => Some(AiEvent::Started(StartedPayload {
            run_id,
            guide_id: guide_id.to_owned(),
            step_ids: step_ids.iter().map(Ulid::to_string).collect(),
        })),
        PipelineEvent::Step(step) => {
            let (outcome, error) = match &step.outcome {
                StepOutcome::Applied => (Outcome::Applied, None),
                StepOutcome::SkippedEdited => (Outcome::SkippedEdited, None),
                StepOutcome::Failed(e) => (Outcome::Failed, Some(e.to_string())),
                StepOutcome::Cancelled => (Outcome::Cancelled, None),
            };
            let mut payload_step = step.step.clone();
            if outcome != Outcome::Applied {
                if let Some(s) = &mut payload_step {
                    forced.restore_step(s);
                }
            }
            Some(AiEvent::Step(StepPayload {
                run_id,
                guide_id: guide_id.to_owned(),
                step_id: step.step_id.to_string(),
                outcome,
                step: payload_step,
                redaction_suggestions: step.redaction_suggestions.clone(),
                error,
            }))
        }
        PipelineEvent::Guide(_) => None,
    }
}

/// After the run: a step event for each consistency edit, then the guide
/// event.
fn closing_events(
    run_id: u64,
    guide: &Guide,
    summary: &PipelineSummary,
    forced: &Forced,
) -> Vec<AiEvent> {
    let guide_id = guide.id.to_string();
    let mut events = Vec::new();
    let (fields, written, error) = match &summary.guide {
        GuidePassOutcome::Applied(applied) => {
            for id in &applied.edited_steps {
                if let Some(step) = guide.steps().find(|step| step.id == *id) {
                    events.push(AiEvent::Step(StepPayload {
                        run_id,
                        guide_id: guide_id.clone(),
                        step_id: id.to_string(),
                        outcome: Outcome::Applied,
                        step: Some(step.clone()),
                        redaction_suggestions: step.ai_state.redaction_suggestions.clone(),
                        error: None,
                    }));
                }
            }
            let fields = (!applied.fields.is_empty()).then(|| GuideFields {
                title: guide.title.clone(),
                intro: guide.intro.clone(),
                prerequisites: guide.prerequisites.clone(),
                summary: guide.summary.clone(),
                tags: guide.tags.clone(),
            });
            (fields, applied.fields.clone(), None)
        }
        GuidePassOutcome::Failed(e) => (None, Vec::new(), Some(e.to_string())),
        GuidePassOutcome::NotRun | GuidePassOutcome::Cancelled => (None, Vec::new(), None),
    };
    let mut ai_state = guide.ai_state.clone();
    if fields.is_none() {
        forced.restore_guide(&mut ai_state);
    }
    events.push(AiEvent::Guide(GuidePayload {
        run_id,
        guide_id,
        fields,
        written,
        ai_state,
        error,
    }));
    events
}

fn finished(
    run_id: u64,
    guide_id: String,
    summary: &PipelineSummary,
    unselected: usize,
) -> FinishedPayload {
    FinishedPayload {
        run_id,
        guide_id,
        cancelled: summary.cancelled,
        applied: summary.steps_applied,
        failed: summary.failures.len(),
        // Steps left out by `stepIds` were marked `edited` for the run.
        skipped_edited: summary.steps_skipped_edited.saturating_sub(unselected),
        usage: TokenUsage {
            input_tokens: summary.usage.input_tokens,
            output_tokens: summary.usage.output_tokens,
        },
        error: None,
    }
}

/// Longest `custom` rewrite instruction, in characters after trimming.
pub const MAX_INSTRUCTION_CHARS: usize = 500;

/// `ai_rewrite_step`'s style, `{ "kind": "shorter" }` etc.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum RewriteStyleArg {
    Shorter,
    Beginner,
    Expert,
    Custom { instruction: String },
}

impl RewriteStyleArg {
    /// The core style; [`ErrorKind::Invalid`] for a `custom` instruction
    /// that is empty or longer than [`MAX_INSTRUCTION_CHARS`] once trimmed.
    fn style(self) -> Result<RewriteStyle> {
        Ok(match self {
            RewriteStyleArg::Shorter => RewriteStyle::Shorter,
            RewriteStyleArg::Beginner => RewriteStyle::Beginner,
            RewriteStyleArg::Expert => RewriteStyle::Expert,
            RewriteStyleArg::Custom { instruction } => {
                let instruction = instruction.trim();
                let len = instruction.chars().count();
                if len == 0 || len > MAX_INSTRUCTION_CHARS {
                    return Err(AppError::new(
                        ErrorKind::Invalid,
                        format!("the instruction must be 1 to {MAX_INSTRUCTION_CHARS} characters"),
                    ));
                }
                RewriteStyle::Custom(instruction.to_owned())
            }
        })
    }
}

/// A rewritten step, for the editor to apply as one change.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rewritten {
    pub title: String,
    pub description: Option<String>,
    /// `ai`, with the model and the rewrite prompt version, no error, and
    /// the step's redaction suggestions.
    pub ai_state: AiState,
}

/// Rewrites step `step_id` of `guide` (the saved snapshot), `edited` or
/// not, in the settings language. Errors: an invalid or unknown step id or
/// instruction ([`ErrorKind::Invalid`]); the provider's error
/// ([`ErrorKind::Io`]).
pub async fn rewrite(
    provider: &dyn AiProvider,
    guide: &Guide,
    step_id: &str,
    style: RewriteStyleArg,
    ai: &settings::Ai,
) -> Result<Rewritten> {
    let id = parse_id(step_id)?;
    let style = style.style()?;
    let step = guide.steps().find(|step| step.id == id).ok_or_else(|| {
        AppError::new(
            ErrorKind::Invalid,
            format!("step {id} is not in the saved guide"),
        )
    })?;
    let language = if ai.language.trim().is_empty() {
        guide.language.clone()
    } else {
        ai.language.clone()
    };
    let input = RewriteInput {
        step_id: id,
        kind: step.kind,
        title: step.title.clone(),
        description: step.description.clone(),
        element: step.element.clone(),
        style,
        language,
    };
    let output = provider
        .rewrite(&input)
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, e.to_string()))?;
    Ok(Rewritten {
        title: output.title,
        description: output.description,
        ai_state: AiState {
            status: AiStatus::Ai,
            model: Some(output.meta.model),
            prompt_version: Some(output.meta.prompt_version),
            error: None,
            redaction_suggestions: step.ai_state.redaction_suggestions.clone(),
        },
    })
}

/// The saved snapshot of an open guide for AI writing (see
/// [`Session::saved_guide`]); annotated guides are refused.
fn ai_guide(session: &Session, id: Ulid) -> Result<(Guide, GuideHandle)> {
    let (guide, handle) = session.saved_guide(id)?;
    if guide.kind == GuideKind::Annotated {
        return Err(AppError::new(ErrorKind::Invalid, NOT_RECORDED));
    }
    Ok((guide, handle))
}

/// Starts writing an open guide with AI, on the guide as last saved (the
/// editor saves first), and returns at once; results arrive as `ai://`
/// events.
#[tauri::command]
pub async fn ai_generate<R: Runtime>(
    app: AppHandle<R>,
    cell: State<'_, Arc<SessionCell>>,
    store: State<'_, Arc<SettingsStore>>,
    runs: State<'_, Arc<AiRuns>>,
    guide_id: String,
    options: Option<GenerateOptions>,
) -> Result<RunStarted> {
    let id = parse_id(&guide_id)?;
    let options = options.unwrap_or_default();
    let ai = store.get().ai;
    let cell = Arc::clone(&cell);
    let (key, guide, handle) = blocking({
        let ai = ai.clone();
        move || {
            let key = check_ready(&ai, keychain::get_key)?;
            let (guide, handle) = ai_guide(&*cell.get()?, id)?;
            Ok((key, guide, handle))
        }
    })
    .await?;
    let (run_id, task) = begin(
        &runs,
        provider(&ai, key)?,
        guide,
        Arc::new(BundleImages(handle)),
        &options,
        &ai,
        Arc::new(app),
    )?;
    tauri::async_runtime::spawn(task);
    Ok(RunStarted { run_id })
}

/// Stops run `run_id`; nothing if it has already finished.
#[tauri::command]
pub fn ai_cancel(runs: State<'_, Arc<AiRuns>>, run_id: u64) {
    runs.cancel(run_id);
}

/// Rewrites one step of an open guide as last saved (the editor saves
/// first) and returns the text; nothing is written. Runs alongside an
/// `ai_generate` run of the same guide.
#[tauri::command]
pub async fn ai_rewrite_step(
    cell: State<'_, Arc<SessionCell>>,
    store: State<'_, Arc<SettingsStore>>,
    guide_id: String,
    step_id: String,
    style: RewriteStyleArg,
) -> Result<Rewritten> {
    let id = parse_id(&guide_id)?;
    let ai = store.get().ai;
    let cell = Arc::clone(&cell);
    let (key, guide) = blocking({
        let ai = ai.clone();
        move || {
            let key = check_ready(&ai, keychain::get_key)?;
            let (guide, _) = ai_guide(&*cell.get()?, id)?;
            Ok((key, guide))
        }
    })
    .await?;
    rewrite(&*provider(&ai, key)?, &guide, &step_id, style, &ai).await
}

/// The quit backstop: stops any run still going (nothing is lost; the
/// editor saved what had arrived).
pub fn cancel_on_quit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(runs) = app.try_state::<Arc<AiRuns>>() {
        let cancelled = runs.cancel_all();
        if cancelled > 0 {
            eprintln!("guiderecorder: cancelled {cancelled} AI run(s) at quit");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guiderecorder_core::ai::mock::{mock_meta, MockProvider};
    use guiderecorder_core::ai::{
        prompts, GuideOutput, StepEdit, StepOutput, SuggestionSource, Usage,
    };
    use guiderecorder_core::model::{Audience, Rect, StepImage, StepKind};
    use serde_json::json;
    use std::sync::atomic::AtomicBool;

    #[derive(Default)]
    struct Events(Mutex<Vec<AiEvent>>);

    impl EventSink for Events {
        fn send(&self, event: AiEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Events {
        fn take(&self) -> Vec<AiEvent> {
            std::mem::take(&mut self.0.lock().unwrap())
        }
    }

    fn step(title: &str) -> Step {
        Step {
            id: Ulid::new(),
            title: title.into(),
            description: String::new(),
            kind: StepKind::Click,
            point: None,
            end_point: None,
            element: None,
            keystrokes: None,
            image: StepImage {
                full: "steps/x.webp".into(),
                crop: None,
                crop_rect: None,
                after: None,
                show: Default::default(),
            },
            annotations: Vec::new(),
            ai_state: AiState::default(),
        }
    }

    /// A guide of three steps; the second is `edited`.
    fn guide() -> Guide {
        let mut guide = Guide::new("Rotate a key", "en");
        let mut edited = step("Mine");
        edited.ai_state.status = AiStatus::Edited;
        guide.items = vec![
            Item::Step(step("Click Keys")),
            Item::Step(edited),
            Item::Step(step("Click Rotate")),
        ];
        guide
    }

    fn step_ids(guide: &Guide) -> Vec<Ulid> {
        guide.steps().map(|s| s.id).collect()
    }

    fn no_images() -> Arc<dyn ImageSource> {
        Arc::new(|_: &str| -> Result<RgbaImage, String> { Err("no image".into()) })
    }

    fn ai() -> settings::Ai {
        settings::Ai::default()
    }

    /// Runs `guide` to the end and returns its events.
    fn run_all(
        provider: MockProvider,
        guide: Guide,
        options: GenerateOptions,
    ) -> (Vec<AiEvent>, Arc<AiRuns>) {
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let (_, task) = begin(
            &runs,
            Arc::new(provider),
            guide,
            no_images(),
            &options,
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        (events.take(), runs)
    }

    fn json_of(event: &AiEvent) -> serde_json::Value {
        serde_json::to_value(event).unwrap()
    }

    #[test]
    fn streams_steps_then_guide_then_finished() {
        let guide = guide();
        let ids = step_ids(&guide);
        let provider = MockProvider::new().on_step(|input| {
            Ok(StepOutput {
                title: format!("AI: {}", input.template_title),
                description: Some("Opens it.".into()),
                redaction_suggestions: vec![RedactionSuggestion {
                    rect: Rect {
                        x: 1.0,
                        y: 2.0,
                        width: 3.0,
                        height: 4.0,
                    },
                    reason: "email address".into(),
                    source: SuggestionSource::Ai,
                }],
                meta: guiderecorder_core::ai::GenerationMeta {
                    usage: Usage {
                        input_tokens: 100,
                        output_tokens: 10,
                        ..Usage::default()
                    },
                    ..mock_meta(prompts::STEP.version)
                },
            })
        });
        let (events, runs) = run_all(provider, guide.clone(), GenerateOptions::default());
        let names: Vec<_> = events.iter().map(AiEvent::name).collect();
        assert_eq!(
            names,
            [
                STARTED_EVENT,
                STEP_EVENT,
                STEP_EVENT,
                GUIDE_EVENT,
                FINISHED_EVENT
            ]
        );
        let run_id = match &events[0] {
            AiEvent::Started(started) => started.run_id,
            other => panic!("{other:?}"),
        };
        let gid = guide.id.to_string();
        assert_eq!(
            json_of(&events[0]),
            json!({
                "runId": run_id,
                "guideId": gid,
                "stepIds": [ids[0].to_string(), ids[2].to_string()],
            })
        );
        // Steps land in any order.
        let first = events[1..3]
            .iter()
            .find(|e| json_of(e)["stepId"] == ids[0].to_string())
            .unwrap();
        assert_eq!(
            json_of(first),
            json!({
                "runId": run_id,
                "guideId": gid,
                "stepId": ids[0].to_string(),
                "outcome": "applied",
                "step": {
                    "id": ids[0].to_string(),
                    "title": "AI: Click Keys",
                    "description": "Opens it.",
                    "kind": "click",
                    "image": { "full": "steps/x.webp", "show": "crop" },
                    "annotations": [],
                    "aiState": {
                        "status": "ai",
                        "model": "mock",
                        "promptVersion": "step.v1",
                        "redactionSuggestions": [{
                            "rect": { "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0 },
                            "reason": "email address",
                        }],
                    },
                },
                "redactionSuggestions": [{
                    "rect": { "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0 },
                    "reason": "email address",
                }],
                "error": null,
            })
        );
        assert_eq!(
            json_of(&events[3]),
            json!({
                "runId": run_id,
                "guideId": gid,
                "fields": {
                    "title": "AI: Rotate a key",
                    "intro": "",
                    "prerequisites": "",
                    "summary": "3 steps",
                    "tags": [],
                },
                "written": ["title", "intro", "prerequisites", "summary", "tags"],
                "aiState": { "status": "ai", "model": "mock", "promptVersion": "guide.v1" },
                "error": null,
            })
        );
        assert_eq!(
            json_of(&events[4]),
            json!({
                "runId": run_id,
                "guideId": gid,
                "cancelled": false,
                "applied": 2,
                "failed": 0,
                "skippedEdited": 1,
                "usage": { "inputTokens": 200, "outputTokens": 20 },
                "error": null,
            })
        );
        assert_eq!(runs.active(), 0);
    }

    #[test]
    fn failed_steps_and_guide_pass_carry_their_errors() {
        let guide = guide();
        let ids = step_ids(&guide);
        let failing = ids[2];
        let provider = MockProvider::new()
            .on_step(move |input| {
                if input.step_id == failing {
                    Err(AiError::Truncated)
                } else {
                    Ok(StepOutput {
                        title: "Done".into(),
                        description: None,
                        redaction_suggestions: Vec::new(),
                        meta: mock_meta(prompts::STEP.version),
                    })
                }
            })
            .on_guide(|_| Err(AiError::Refusal { category: None }));
        let (events, _) = run_all(provider, guide.clone(), GenerateOptions::default());
        let failed = events
            .iter()
            .find_map(|e| match e {
                AiEvent::Step(s) if s.outcome == Outcome::Failed => Some(s),
                _ => None,
            })
            .unwrap();
        assert_eq!(failed.step_id, failing.to_string());
        assert_eq!(
            failed.error.as_deref(),
            Some("the response was cut off at the token limit")
        );
        let step = failed.step.as_ref().unwrap();
        assert_eq!(step.title, "Click Rotate");
        assert_eq!(step.ai_state.error, failed.error);
        let AiEvent::Guide(guide_event) = &events[events.len() - 2] else {
            panic!("{events:?}");
        };
        assert_eq!(guide_event.fields, None);
        assert_eq!(
            guide_event.error.as_deref(),
            Some("the model declined the request")
        );
        assert_eq!(guide_event.ai_state.error, guide_event.error);
        let AiEvent::Finished(finished) = events.last().unwrap() else {
            panic!("{events:?}");
        };
        assert_eq!((finished.applied, finished.failed), (1, 1));
    }

    #[test]
    fn edited_guides_get_no_fields_and_consistency_edits_are_streamed() {
        let mut guide = guide();
        guide.ai_state.status = AiStatus::Edited;
        let ids = step_ids(&guide);
        let target = ids[2];
        let edited_step = ids[1];
        let provider = MockProvider::new().on_guide(move |_| {
            Ok(GuideOutput {
                title: "New".into(),
                intro: String::new(),
                prerequisites: String::new(),
                summary: String::new(),
                tags: Vec::new(),
                step_edits: vec![
                    StepEdit {
                        step_id: target,
                        title: Some("Click **Rotate**".into()),
                        description: None,
                    },
                    StepEdit {
                        step_id: edited_step,
                        title: Some("Not mine".into()),
                        description: None,
                    },
                ],
                meta: mock_meta(prompts::GUIDE.version),
            })
        });
        let (events, _) = run_all(provider, guide, GenerateOptions::default());
        let names: Vec<_> = events.iter().map(AiEvent::name).collect();
        assert_eq!(
            names,
            [
                STARTED_EVENT,
                STEP_EVENT,
                STEP_EVENT,
                STEP_EVENT,
                GUIDE_EVENT,
                FINISHED_EVENT
            ]
        );
        let AiEvent::Step(edit) = &events[3] else {
            panic!("{events:?}");
        };
        assert_eq!(edit.step_id, target.to_string());
        assert_eq!(edit.outcome, Outcome::Applied);
        assert_eq!(edit.step.as_ref().unwrap().title, "Click **Rotate**");
        let AiEvent::Guide(guide_event) = &events[4] else {
            panic!("{events:?}");
        };
        assert_eq!(guide_event.fields, None);
        assert_eq!(guide_event.error, None);
        assert_eq!(guide_event.ai_state.status, AiStatus::Edited);
    }

    #[test]
    fn step_ids_limit_the_run() {
        let guide = guide();
        let ids = step_ids(&guide);
        let provider = Arc::new(MockProvider::new());
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let options = GenerateOptions {
            step_ids: Some(vec![ids[1].to_string(), ids[2].to_string()]),
            guide_pass: Some(false),
            force: false,
        };
        let (_, task) = begin(
            &runs,
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            guide.clone(),
            no_images(),
            &options,
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        let events = events.take();
        let AiEvent::Started(started) = &events[0] else {
            panic!("{events:?}");
        };
        // The edited step is skipped even when asked for.
        assert_eq!(started.step_ids, [ids[2].to_string()]);
        assert_eq!(provider.step_calls(), 1);
        assert_eq!(provider.guide_calls(), 0);
        let AiEvent::Guide(guide_event) = &events[events.len() - 2] else {
            panic!("{events:?}");
        };
        assert_eq!(guide_event.fields, None);
        let AiEvent::Finished(finished) = events.last().unwrap() else {
            panic!("{events:?}");
        };
        assert_eq!(
            (finished.applied, finished.skipped_edited, finished.failed),
            (1, 1, 0)
        );

        for bad in ["nope", &Ulid::new().to_string()] {
            let options = GenerateOptions {
                step_ids: Some(vec![bad.to_string()]),
                ..GenerateOptions::default()
            };
            let err = begin(
                &runs,
                Arc::new(MockProvider::new()),
                guide.clone(),
                no_images(),
                &options,
                &ai(),
                Arc::new(Events::default()),
            )
            .err()
            .unwrap();
            assert_eq!(err.kind, ErrorKind::Invalid);
        }
        assert_eq!(runs.active(), 0);
    }

    #[test]
    fn writes_in_the_settings_language_and_audience() {
        let provider = Arc::new(MockProvider::new());
        let runs = Arc::new(AiRuns::default());
        let settings = settings::Ai {
            language: "de".into(),
            audience: Audience::Expert,
            ..ai()
        };
        let (_, task) = begin(
            &runs,
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            guide(),
            no_images(),
            &GenerateOptions::default(),
            &settings,
            Arc::new(Events::default()),
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        let seen = provider.seen_steps();
        assert!(!seen.is_empty());
        assert!(seen
            .iter()
            .all(|s| s.language == "de" && s.audience == Audience::Expert));
    }

    #[test]
    fn text_only_models_get_no_images() {
        let images: Arc<dyn ImageSource> = Arc::new(|_: &str| Ok(RgbaImage::new(400, 200)));
        let run = |ai: settings::Ai| {
            let provider = Arc::new(MockProvider::new());
            let (_, task) = begin(
                &Arc::new(AiRuns::default()),
                Arc::clone(&provider) as Arc<dyn AiProvider>,
                guide(),
                Arc::clone(&images),
                &GenerateOptions::default(),
                &ai,
                Arc::new(Events::default()),
            )
            .unwrap();
            tauri::async_runtime::block_on(task);
            provider.seen_steps()
        };
        assert!(run(ai()).iter().all(|s| s.full.is_some()));
        let mut text_only = settings::Ai {
            provider: Provider::LmStudio,
            ..ai()
        };
        text_only.lm_studio.images = false;
        let seen = run(text_only);
        assert!(!seen.is_empty());
        assert!(seen.iter().all(|s| s.full.is_none() && s.crop.is_none()));
    }

    #[test]
    fn checks_settings_before_the_key() {
        let off = settings::Ai {
            enabled: false,
            ..ai()
        };
        let asked = AtomicBool::new(false);
        let err = check_ready(&off, |_| {
            asked.store(true, Ordering::SeqCst);
            Ok(None)
        })
        .unwrap_err();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, AI_OFF));
        assert!(!asked.load(Ordering::SeqCst));

        let err = check_ready(&ai(), |_| Ok(None)).unwrap_err();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, NO_KEY));
        let err = check_ready(&ai(), |_| Err(AiError::MissingApiKey)).unwrap_err();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, NO_KEY));
        let err = check_ready(&ai(), |_| Err(AiError::Keychain("locked".into()))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Io);

        let key = check_ready(&ai(), |account| {
            assert_eq!(account, keychain::ACCOUNT);
            Ok(Some(ApiKey::new("sk-1").unwrap()))
        })
        .unwrap();
        assert_eq!(key.unwrap().expose(), "sk-1");
        assert!(provider(&ai(), key_for_test()).is_ok());
        assert!(provider(&ai(), None).is_err());
    }

    fn key_for_test() -> Option<ApiKey> {
        Some(ApiKey::new("sk-1").unwrap())
    }

    #[test]
    fn openai_compatible_providers_need_models_not_keys() {
        let never = |_: &str| -> std::result::Result<Option<ApiKey>, AiError> {
            panic!("local providers never ask the Keychain")
        };
        let mut local = settings::Ai {
            provider: Provider::Ollama,
            ..ai()
        };
        let err = check_ready(&local, never).unwrap_err();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, NO_MODEL));
        local.ollama.step_model = "qwen3-vl:8b".into();
        local.ollama.guide_model = "qwen3:14b".into();
        assert_eq!(check_ready(&local, never).unwrap(), None);
        assert!(provider(&local, None).is_ok());
        let config = openai_config(&local).unwrap();
        assert_eq!(config.url(), "http://localhost:11434/v1/chat/completions");
        assert_eq!(config.step.model, "qwen3-vl:8b");
        assert_eq!(config.rewrite.model, "qwen3-vl:8b");
        assert_eq!(config.guide.model, "qwen3:14b");
        // A changed port.
        local.ollama.base_url = "http://localhost:9999/v1".into();
        assert_eq!(
            openai_config(&local).unwrap().url(),
            "http://localhost:9999/v1/chat/completions"
        );
        let studio = settings::Ai {
            provider: Provider::LmStudio,
            lm_studio: local.ollama.clone(),
            ..ai()
        };
        assert_eq!(
            openai_config(&studio).unwrap().base_url,
            "http://localhost:9999/v1"
        );

        let mut custom = settings::Ai {
            provider: Provider::OpenAiCompatible,
            ..ai()
        };
        custom.open_ai_compatible.step_model = "gpt-5-mini".into();
        custom.open_ai_compatible.guide_model = "gpt-5".into();
        let err = check_ready(&custom, never).unwrap_err();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, NO_BASE_URL));
        custom.open_ai_compatible.base_url = "https://api.openai.com/v1".into();
        // The key is optional, and read from its own entry.
        let key = check_ready(&custom, |account| {
            assert_eq!(account, keychain::OPENAI_COMPATIBLE_ACCOUNT);
            Ok(None)
        })
        .unwrap();
        assert_eq!(key, None);
        let key = check_ready(&custom, |_| Ok(key_for_test())).unwrap();
        assert!(key.is_some());
        let err = check_ready(&custom, |_| Err(AiError::Keychain("locked".into()))).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Io);
        assert!(provider(&custom, None).is_ok());
        assert!(openai_config(&ai()).is_none());
    }

    #[test]
    fn one_run_per_guide() {
        let runs = Arc::new(AiRuns::default());
        let first = guide();
        let start = |guide: Guide| {
            begin(
                &runs,
                Arc::new(MockProvider::new()),
                guide,
                no_images(),
                &GenerateOptions::default(),
                &ai(),
                Arc::new(Events::default()),
            )
        };
        let (id, task) = start(first.clone()).unwrap();
        let err = start(first.clone()).err().unwrap();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, BUSY));
        // Another guide may run meanwhile.
        let (other_id, other) = start(guide()).unwrap();
        assert_ne!(id, other_id);
        assert_eq!(runs.active(), 2);
        tauri::async_runtime::block_on(task);
        tauri::async_runtime::block_on(other);
        assert_eq!(runs.active(), 0);
        // Finished: the guide may run again.
        let (again, _task) = start(first).unwrap();
        assert!(again > other_id);
    }

    #[test]
    fn a_dropped_run_leaves_the_registry() {
        let runs = Arc::new(AiRuns::default());
        let (_, task) = begin(
            &runs,
            Arc::new(MockProvider::new()),
            guide(),
            no_images(),
            &GenerateOptions::default(),
            &ai(),
            Arc::new(Events::default()),
        )
        .unwrap();
        assert_eq!(runs.active(), 1);
        drop(task);
        assert_eq!(runs.active(), 0);
    }

    #[test]
    fn cancel_stops_the_run() {
        let guide = guide();
        let ids = step_ids(&guide);
        let provider = Arc::new(MockProvider::new());
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let (run_id, task) = begin(
            &runs,
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            guide.clone(),
            no_images(),
            &GenerateOptions::default(),
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        // Unknown ids are ignored.
        runs.cancel(run_id + 100);
        runs.cancel(run_id);
        tauri::async_runtime::block_on(task);
        let events = events.take();
        assert_eq!(provider.step_calls(), 0);
        assert_eq!(provider.guide_calls(), 0);
        let mut cancelled: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AiEvent::Step(s) => Some((s.step_id.clone(), s.outcome, s.step.is_none())),
                _ => None,
            })
            .collect();
        // Steps land in any order.
        cancelled.sort_by(|a, b| a.0.cmp(&b.0));
        let mut expected = vec![
            (ids[0].to_string(), Outcome::Cancelled, true),
            (ids[2].to_string(), Outcome::Cancelled, true),
        ];
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(cancelled, expected);
        let AiEvent::Guide(guide_event) = &events[events.len() - 2] else {
            panic!("{events:?}");
        };
        assert_eq!((&guide_event.fields, &guide_event.error), (&None, &None));
        assert_eq!(
            json_of(events.last().unwrap()),
            json!({
                "runId": run_id,
                "guideId": guide.id.to_string(),
                "cancelled": true,
                "applied": 0,
                "failed": 0,
                "skippedEdited": 1,
                "usage": { "inputTokens": 0, "outputTokens": 0 },
                "error": null,
            })
        );
        assert_eq!(runs.active(), 0);
        // Cancelling a finished run does nothing.
        runs.cancel(run_id);
    }

    #[test]
    fn cancel_all_reaches_every_run() {
        let runs = Arc::new(AiRuns::default());
        let mut tasks = Vec::new();
        let mut sinks = Vec::new();
        for _ in 0..2 {
            let events = Arc::new(Events::default());
            let (_, task) = begin(
                &runs,
                Arc::new(MockProvider::new()),
                guide(),
                no_images(),
                &GenerateOptions::default(),
                &ai(),
                Arc::clone(&events) as Arc<dyn EventSink>,
            )
            .unwrap();
            tasks.push(task);
            sinks.push(events);
        }
        assert_eq!(runs.cancel_all(), 2);
        for task in tasks {
            tauri::async_runtime::block_on(task);
        }
        for events in sinks {
            let events = events.take();
            let AiEvent::Finished(finished) = events.last().unwrap() else {
                panic!("{events:?}");
            };
            assert!(finished.cancelled);
        }
        assert_eq!(runs.cancel_all(), 0);
    }

    #[test]
    fn a_broken_run_still_sends_guide_and_finished() {
        let mut guide = guide();
        guide.ai_state.model = Some("earlier".into());
        let ids = step_ids(&guide);
        let first = ids[0];
        // Fails the first step, then panics on the next.
        let provider = MockProvider::new().on_step(move |input| {
            if input.step_id == first {
                Err(AiError::Truncated)
            } else {
                panic!("provider broke")
            }
        });
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let (run_id, task) = begin(
            &runs,
            Arc::new(provider),
            guide.clone(),
            no_images(),
            &GenerateOptions::default(),
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        let events = events.take();
        let names: Vec<_> = events.iter().map(AiEvent::name).collect();
        assert_eq!(names.first(), Some(&STARTED_EVENT));
        assert_eq!(&names[names.len() - 2..], [GUIDE_EVENT, FINISHED_EVENT]);
        assert_eq!(names.iter().filter(|n| **n == GUIDE_EVENT).count(), 1);
        let gid = guide.id.to_string();
        assert_eq!(
            json_of(&events[events.len() - 2]),
            json!({
                "runId": run_id,
                "guideId": gid,
                "fields": null,
                "written": [],
                "aiState": { "status": "template", "model": "earlier" },
                "error": RUN_FAILED,
            })
        );
        assert_eq!(
            json_of(events.last().unwrap()),
            json!({
                "runId": run_id,
                "guideId": gid,
                "cancelled": false,
                "applied": 0,
                "failed": 0,
                "skippedEdited": 0,
                "usage": { "inputTokens": 0, "outputTokens": 0 },
                "error": RUN_FAILED,
            })
        );
        // Cleared before `finished`, as on the normal path.
        assert_eq!(runs.active(), 0);
    }

    #[test]
    fn finished_follows_a_cleared_registry() {
        struct Check(Arc<AiRuns>, AtomicBool);
        impl EventSink for Check {
            fn send(&self, event: AiEvent) {
                if let AiEvent::Finished(_) = event {
                    self.1.store(self.0.active() == 0, Ordering::SeqCst);
                }
            }
        }
        for broken in [false, true] {
            let runs = Arc::new(AiRuns::default());
            let check = Arc::new(Check(Arc::clone(&runs), AtomicBool::new(false)));
            let provider = MockProvider::new().on_step(move |input| {
                if broken {
                    panic!("provider broke")
                }
                Ok(StepOutput {
                    title: input.template_title.clone(),
                    description: None,
                    redaction_suggestions: Vec::new(),
                    meta: mock_meta(prompts::STEP.version),
                })
            });
            let (_, task) = begin(
                &runs,
                Arc::new(provider),
                guide(),
                no_images(),
                &GenerateOptions::default(),
                &ai(),
                Arc::clone(&check) as Arc<dyn EventSink>,
            )
            .unwrap();
            tauri::async_runtime::block_on(task);
            assert!(check.1.load(Ordering::SeqCst), "broken: {broken}");
        }
    }

    #[test]
    fn options_from_json() {
        let options: GenerateOptions =
            serde_json::from_value(json!({ "stepIds": ["a"], "guidePass": false })).unwrap();
        assert_eq!(options.step_ids, Some(vec!["a".to_string()]));
        assert_eq!(options.guide_pass, Some(false));
        assert!(!options.force);
        let options: GenerateOptions =
            serde_json::from_value(json!({ "stepIds": [], "guidePass": true, "force": true }))
                .unwrap();
        assert_eq!(options.step_ids, Some(Vec::new()));
        assert!(options.force);
        let options: GenerateOptions = serde_json::from_value(json!({})).unwrap();
        assert_eq!(options, GenerateOptions::default());
        assert_eq!(
            serde_json::to_value(RunStarted { run_id: 7 }).unwrap(),
            json!({ "runId": 7 })
        );
    }

    #[test]
    fn claude_config_follows_settings() {
        let config = claude_config(&settings::Ai {
            step_model: "claude-sonnet-5-5".into(),
            guide_model: "claude-opus-5-5".into(),
            ..ai()
        });
        assert_eq!(config.step.model, "claude-sonnet-5-5");
        assert_eq!(config.rewrite.model, "claude-sonnet-5-5");
        assert_eq!(config.guide.model, "claude-opus-5-5");
        assert!(config.guide.server_fallback);
        let config = claude_config(&settings::Ai {
            guide_model: "claude-haiku-5-5".into(),
            ..ai()
        });
        assert!(!config.guide.server_fallback);
        let config = claude_config(&settings::Ai {
            step_model: String::new(),
            ..ai()
        });
        assert_eq!(config.step.model, ClaudeConfig::default().step.model);
    }

    fn step_events(events: &[AiEvent]) -> Vec<&StepPayload> {
        events
            .iter()
            .filter_map(|e| match e {
                AiEvent::Step(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    fn guide_event(events: &[AiEvent]) -> &GuidePayload {
        events
            .iter()
            .find_map(|e| match e {
                AiEvent::Guide(g) => Some(g),
                _ => None,
            })
            .unwrap()
    }

    fn finished_event(events: &[AiEvent]) -> &FinishedPayload {
        let AiEvent::Finished(finished) = events.last().unwrap() else {
            panic!("{events:?}");
        };
        finished
    }

    #[test]
    fn force_regenerates_an_edited_step() {
        let guide = guide();
        let ids = step_ids(&guide);
        let (events, _) = run_all(
            MockProvider::new(),
            guide,
            GenerateOptions {
                step_ids: Some(vec![ids[1].to_string()]),
                guide_pass: Some(false),
                force: true,
            },
        );
        let AiEvent::Started(started) = &events[0] else {
            panic!("{events:?}");
        };
        assert_eq!(started.step_ids, [ids[1].to_string()]);
        let steps = step_events(&events);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].outcome, Outcome::Applied);
        let step = steps[0].step.as_ref().unwrap();
        assert_eq!(step.title, "AI: Mine");
        assert_eq!(step.ai_state.status, AiStatus::Ai);
        assert_eq!(guide_event(&events).fields, None);
        let finished = finished_event(&events);
        assert_eq!(
            (finished.applied, finished.skipped_edited, finished.failed),
            (1, 0, 0)
        );
    }

    #[test]
    fn without_force_an_edited_step_is_skipped() {
        let guide = guide();
        let ids = step_ids(&guide);
        let provider = Arc::new(MockProvider::new());
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let options = GenerateOptions {
            step_ids: Some(vec![ids[1].to_string()]),
            guide_pass: Some(false),
            force: false,
        };
        let (_, task) = begin(
            &runs,
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            guide,
            no_images(),
            &options,
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        let events = events.take();
        assert_eq!(provider.step_calls(), 0);
        let finished = finished_event(&events);
        assert_eq!((finished.applied, finished.skipped_edited), (0, 1));
    }

    #[test]
    fn a_forced_step_that_fails_stays_edited() {
        let guide = guide();
        let ids = step_ids(&guide);
        let provider = MockProvider::new().on_step(|_| Err(AiError::Truncated));
        let (events, _) = run_all(
            provider,
            guide,
            GenerateOptions {
                step_ids: Some(vec![ids[1].to_string()]),
                guide_pass: Some(false),
                force: true,
            },
        );
        let steps = step_events(&events);
        assert_eq!(steps[0].outcome, Outcome::Failed);
        let step = steps[0].step.as_ref().unwrap();
        assert_eq!(step.title, "Mine");
        assert_eq!(step.ai_state.status, AiStatus::Edited);
        assert!(step.ai_state.error.is_some());
    }

    #[test]
    fn force_regenerates_edited_guide_fields_only() {
        let mut guide = guide();
        guide.ai_state.status = AiStatus::Edited;
        let ids = step_ids(&guide);
        let edited_step = ids[1];
        let other = ids[0];
        let provider = Arc::new(MockProvider::new().on_guide(move |input| {
            // No step is open to consistency edits: there is no step pass.
            assert!(input.steps.iter().all(|s| !s.editable));
            Ok(GuideOutput {
                title: "New".into(),
                intro: "Intro".into(),
                prerequisites: String::new(),
                summary: String::new(),
                tags: vec!["keys".into()],
                step_edits: vec![
                    StepEdit {
                        step_id: other,
                        title: Some("Changed".into()),
                        description: None,
                    },
                    StepEdit {
                        step_id: edited_step,
                        title: Some("Changed".into()),
                        description: None,
                    },
                ],
                meta: mock_meta(prompts::GUIDE.version),
            })
        }));
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let options = GenerateOptions {
            step_ids: Some(Vec::new()),
            guide_pass: Some(true),
            force: true,
        };
        let (_, task) = begin(
            &runs,
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            guide,
            no_images(),
            &options,
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        let events = events.take();
        assert_eq!(provider.step_calls(), 0);
        assert_eq!(provider.guide_calls(), 1);
        let names: Vec<_> = events.iter().map(AiEvent::name).collect();
        assert_eq!(names, [STARTED_EVENT, GUIDE_EVENT, FINISHED_EVENT]);
        let AiEvent::Started(started) = &events[0] else {
            panic!("{events:?}");
        };
        assert!(started.step_ids.is_empty());
        let guide_event = guide_event(&events);
        let fields = guide_event.fields.as_ref().unwrap();
        assert_eq!(
            (fields.title.as_str(), fields.intro.as_str()),
            ("New", "Intro")
        );
        assert_eq!(guide_event.ai_state.status, AiStatus::Ai);
        assert_eq!(
            guide_event.ai_state.prompt_version.as_deref(),
            Some(prompts::GUIDE.version)
        );
        let finished = finished_event(&events);
        assert_eq!(
            (finished.applied, finished.skipped_edited, finished.failed),
            (0, 0, 0)
        );
    }

    #[test]
    fn guide_fields_edited_by_hand_are_not_written() {
        let mut guide = guide();
        guide.summary = "My summary".into();
        guide.edited_fields = vec![GuideField::Summary];
        guide.ai_state.status = AiStatus::Edited;
        let provider = MockProvider::new().on_guide(|_| {
            Ok(GuideOutput {
                title: "New".into(),
                intro: "Intro".into(),
                prerequisites: "- Access".into(),
                summary: "AI summary".into(),
                tags: vec!["keys".into()],
                step_edits: Vec::new(),
                meta: mock_meta(prompts::GUIDE.version),
            })
        });
        let (events, _) = run_all(provider, guide, GenerateOptions::default());
        let guide_event = guide_event(&events);
        let fields = guide_event.fields.as_ref().unwrap();
        assert_eq!(fields.summary, "My summary");
        assert_eq!(fields.intro, "Intro");
        assert_eq!(
            guide_event.written,
            [
                GuideField::Title,
                GuideField::Intro,
                GuideField::Prerequisites,
                GuideField::Tags
            ]
        );
        assert_eq!(guide_event.ai_state.status, AiStatus::Edited);
        let json = serde_json::to_value(guide_event).unwrap();
        assert_eq!(
            json["written"],
            json!(["title", "intro", "prerequisites", "tags"])
        );
    }

    #[test]
    fn force_regenerates_fields_edited_by_hand() {
        let mut guide = guide();
        guide.edited_fields = vec![GuideField::Intro];
        guide.ai_state.status = AiStatus::Edited;
        let (events, _) = run_all(
            MockProvider::new(),
            guide,
            GenerateOptions {
                step_ids: Some(Vec::new()),
                guide_pass: Some(true),
                force: true,
            },
        );
        let guide_event = guide_event(&events);
        assert!(guide_event.fields.is_some());
        assert_eq!(guide_event.written, GuideField::ALL);
        assert_eq!(guide_event.ai_state.status, AiStatus::Ai);
    }

    #[test]
    fn empty_step_ids_run_only_the_guide_pass() {
        let guide = guide();
        let provider = Arc::new(MockProvider::new());
        let runs = Arc::new(AiRuns::default());
        let events = Arc::new(Events::default());
        let options = GenerateOptions {
            step_ids: Some(Vec::new()),
            guide_pass: Some(true),
            force: false,
        };
        let (_, task) = begin(
            &runs,
            Arc::clone(&provider) as Arc<dyn AiProvider>,
            guide,
            no_images(),
            &options,
            &ai(),
            Arc::clone(&events) as Arc<dyn EventSink>,
        )
        .unwrap();
        tauri::async_runtime::block_on(task);
        let events = events.take();
        assert_eq!(provider.step_calls(), 0);
        assert_eq!(provider.guide_calls(), 1);
        assert!(step_events(&events).is_empty());
        assert!(guide_event(&events).fields.is_some());
        let finished = finished_event(&events);
        assert_eq!(
            (finished.applied, finished.skipped_edited, finished.failed),
            (0, 0, 0)
        );
    }

    #[test]
    fn a_forced_guide_pass_that_fails_stays_edited() {
        let mut guide = guide();
        guide.ai_state.status = AiStatus::Edited;
        let provider = MockProvider::new().on_guide(|_| Err(AiError::Truncated));
        let (events, _) = run_all(
            provider,
            guide,
            GenerateOptions {
                step_ids: Some(Vec::new()),
                guide_pass: Some(true),
                force: true,
            },
        );
        let guide_event = guide_event(&events);
        assert_eq!(guide_event.fields, None);
        assert_eq!(guide_event.ai_state.status, AiStatus::Edited);
        assert!(guide_event.error.is_some());
    }

    fn rewrite_now(
        provider: &MockProvider,
        guide: &Guide,
        step_id: &str,
        style: RewriteStyleArg,
    ) -> Result<Rewritten> {
        tauri::async_runtime::block_on(rewrite(provider, guide, step_id, style, &ai()))
    }

    #[test]
    fn rewrites_an_edited_step() {
        let mut guide = guide();
        let ids = step_ids(&guide);
        let suggestion = RedactionSuggestion {
            rect: Rect {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            },
            reason: "email address".into(),
            source: SuggestionSource::Ai,
        };
        if let Item::Step(step) = &mut guide.items[1] {
            step.description = "Mine too.".into();
            step.ai_state.error = Some("old".into());
            step.ai_state.redaction_suggestions = vec![suggestion.clone()];
        }
        let provider = MockProvider::new();
        let rewritten = rewrite_now(
            &provider,
            &guide,
            &ids[1].to_string(),
            RewriteStyleArg::Shorter,
        )
        .unwrap();
        assert_eq!(provider.rewrite_calls(), 1);
        assert_eq!(
            serde_json::to_value(&rewritten).unwrap(),
            json!({
                "title": "AI: Mine",
                "description": "Mine too.",
                "aiState": {
                    "status": "ai",
                    "model": "mock",
                    "promptVersion": prompts::REWRITE.version,
                    "redactionSuggestions": [{
                        "rect": { "x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0 },
                        "reason": "email address",
                    }],
                },
            })
        );
        assert_eq!(rewritten.ai_state.error, None);
    }

    #[test]
    fn rewrite_passes_the_style_and_language() {
        let guide = guide();
        let ids = step_ids(&guide);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let provider = MockProvider::new().on_rewrite({
            let seen = Arc::clone(&seen);
            move |input| {
                seen.lock()
                    .unwrap()
                    .push((input.style.clone(), input.language.clone()));
                Ok(guiderecorder_core::ai::RewriteOutput {
                    title: "T".into(),
                    description: None,
                    meta: mock_meta(prompts::REWRITE.version),
                })
            }
        });
        let style: RewriteStyleArg =
            serde_json::from_value(json!({ "kind": "custom", "instruction": "  Be formal. " }))
                .unwrap();
        let rewritten = rewrite_now(&provider, &guide, &ids[0].to_string(), style).unwrap();
        assert_eq!(rewritten.description, None);
        for (value, style) in [
            (json!({ "kind": "shorter" }), RewriteStyle::Shorter),
            (json!({ "kind": "beginner" }), RewriteStyle::Beginner),
            (json!({ "kind": "expert" }), RewriteStyle::Expert),
        ] {
            let arg: RewriteStyleArg = serde_json::from_value(value).unwrap();
            assert_eq!(arg.style().unwrap(), style);
        }
        assert_eq!(
            seen.lock().unwrap()[0],
            (RewriteStyle::Custom("Be formal.".into()), "en".to_string())
        );
    }

    #[test]
    fn custom_instructions_are_checked() {
        let guide = guide();
        let id = step_ids(&guide)[0].to_string();
        let provider = MockProvider::new();
        let custom = |s: &str| RewriteStyleArg::Custom {
            instruction: s.into(),
        };
        for bad in ["", "   \n", &"x".repeat(MAX_INSTRUCTION_CHARS + 1)] {
            let err = rewrite_now(&provider, &guide, &id, custom(bad)).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid, "{bad:?}");
        }
        assert_eq!(provider.rewrite_calls(), 0);
        // Counted in characters, after trimming.
        let longest = format!("  {}  ", "é".repeat(MAX_INSTRUCTION_CHARS));
        assert!(rewrite_now(&provider, &guide, &id, custom(&longest)).is_ok());
        assert!(rewrite_now(&provider, &guide, &id, custom("a")).is_ok());
        assert!(serde_json::from_value::<RewriteStyleArg>(json!({ "kind": "custom" })).is_err());
        assert!(serde_json::from_value::<RewriteStyleArg>(json!({ "kind": "louder" })).is_err());
    }

    #[test]
    fn rewrite_errors() {
        let guide = guide();
        let id = step_ids(&guide)[0].to_string();
        let provider = MockProvider::new();
        for bad in ["nope".to_string(), Ulid::new().to_string()] {
            let err = rewrite_now(&provider, &guide, &bad, RewriteStyleArg::Expert).unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid);
        }
        assert_eq!(provider.rewrite_calls(), 0);
        let failing = MockProvider::new().on_rewrite(|_| Err(AiError::Refusal { category: None }));
        let err = rewrite_now(&failing, &guide, &id, RewriteStyleArg::Beginner).unwrap_err();
        assert_eq!(
            err,
            AppError::new(ErrorKind::Io, "the model declined the request")
        );
    }

    #[test]
    fn the_saved_snapshot_needs_an_open_guide() {
        let root = crate::editor::tests::library();
        let session = crate::editor::Session::start(&root, None).unwrap();
        let id = parse_id(&crate::editor::tests::fixture_id(&root)).unwrap();
        // The command reads the guide through `saved_guide`, as `ai_generate`.
        let err = session.saved_guide(id).err().unwrap();
        assert_eq!(err.kind, ErrorKind::NotOpen);
    }

    #[test]
    fn annotated_guides_are_not_written_with_ai() {
        let root = crate::editor::tests::library();
        let session = crate::editor::Session::start(&root, None).unwrap();
        let id = crate::editor::tests::fixture_id(&root);
        let mut guide = session.open_guide(&id).unwrap().guide;
        let id = parse_id(&id).unwrap();
        assert!(ai_guide(&session, id).is_ok());
        guide.kind = GuideKind::Annotated;
        session.save_guide(guide).unwrap();
        let err = ai_guide(&session, id).err().unwrap();
        assert_eq!(err, AppError::new(ErrorKind::Invalid, NOT_RECORDED));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rewrite_runs_alongside_a_run_of_the_guide() {
        let runs = Arc::new(AiRuns::default());
        let guide = guide();
        let id = step_ids(&guide)[0].to_string();
        let (_, task) = begin(
            &runs,
            Arc::new(MockProvider::new()),
            guide.clone(),
            no_images(),
            &GenerateOptions::default(),
            &ai(),
            Arc::new(Events::default()),
        )
        .unwrap();
        assert_eq!(runs.active(), 1);
        let provider = MockProvider::new();
        assert!(rewrite_now(&provider, &guide, &id, RewriteStyleArg::Shorter).is_ok());
        tauri::async_runtime::block_on(task);
    }

    #[test]
    fn bundle_images_serve_step_images_only() {
        let root = crate::editor::tests::library();
        let session = crate::editor::Session::start(&root, None).unwrap();
        let id = crate::editor::tests::fixture_id(&root);
        let opened = session.open_guide(&id).unwrap();
        let (guide, handle) = session.saved_guide(parse_id(&id).unwrap()).unwrap();
        assert_eq!(guide, opened.guide);
        let images = BundleImages(handle);
        let full = &guide.steps().next().unwrap().image.full;
        assert!(images.load(full).is_ok());
        assert!(images.load("guide.json").is_err());
        assert!(images.load("steps/../../x.webp").is_err());
    }
}
