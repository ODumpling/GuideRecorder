//! [`AiProvider`] over the Anthropic Messages API (`POST /v1/messages`).
//!
//! Each call sends a versioned system prompt ([`prompts`]), the step images
//! as base64 image blocks, and a JSON context block. Structured output comes
//! back through one strict tool (`record_step`, `record_guide`,
//! `record_rewrite`) whose input is the result. Tool choice is `auto`, with
//! the prompt naming the tool: the guide model (`claude-sonnet-5-5`) rejects
//! forced tool choice, so all calls use `auto` for one code path. A reply
//! without the tool call is retried once.
//!
//! Request building and response parsing are pure functions so they can be
//! tested without the network; [`ClaudeProvider`] adds HTTP and retries.
//! The context blocks, tool schemas and output parsing are shared with the
//! other providers ([`super::openai`]), so every provider sees the same
//! input and is held to the same output.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::keychain::{self, ApiKey};
use super::prompts::{self, Prompt};
use super::retry::{retry_after_from_headers, with_retry, RetryPolicy};
use super::{
    base64, fits_api_limit, AiError, AiFuture, AiProvider, GenerationMeta, GuideInput, GuideOutput,
    ImageData, RedactionSuggestion, RewriteInput, RewriteOutput, RewriteStyle, StepEdit, StepInput,
    StepOutput, SuggestionSource, Usage,
};
use crate::model::Rect;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2023-06-01";
pub const DEFAULT_STEP_MODEL: &str = "claude-haiku-5-5";
pub const DEFAULT_GUIDE_MODEL: &str = "claude-sonnet-5-5";
/// Beta that lets the API retry a declined request on a fallback model.
pub const SERVER_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

pub const STEP_TOOL: &str = "record_step";
pub const GUIDE_TOOL: &str = "record_guide";
pub const REWRITE_TOOL: &str = "record_rewrite";

/// `output_config.effort`: how much the model thinks and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

/// Per-call model configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSettings {
    pub model: String,
    pub max_tokens: u32,
    /// `None` uses the model's default.
    pub effort: Option<Effort>,
    /// Send `fallbacks: "default"` so the API reroutes declined requests.
    /// Not supported by every model (e.g. Haiku), and only sent when
    /// [`ClaudeConfig::base_url`] is the first-party [`DEFAULT_BASE_URL`];
    /// proxies and other platforms reject it.
    pub server_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeConfig {
    pub base_url: String,
    pub api_version: String,
    pub step: ModelSettings,
    pub guide: ModelSettings,
    pub rewrite: ModelSettings,
    pub retry: RetryPolicy,
    /// Per-attempt HTTP timeout.
    pub timeout: Duration,
}

impl ClaudeConfig {
    /// Whether requests made with `settings` carry `fallbacks`: only when
    /// enabled and talking to the first-party API.
    pub fn sends_fallbacks(&self, settings: &ModelSettings) -> bool {
        settings.server_fallback && self.base_url.trim_end_matches('/') == DEFAULT_BASE_URL
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            api_version: API_VERSION.into(),
            step: ModelSettings {
                model: DEFAULT_STEP_MODEL.into(),
                max_tokens: 16_000,
                effort: Some(Effort::Low),
                server_fallback: false,
            },
            guide: ModelSettings {
                model: DEFAULT_GUIDE_MODEL.into(),
                max_tokens: 16_000,
                effort: Some(Effort::Medium),
                server_fallback: true,
            },
            rewrite: ModelSettings {
                model: DEFAULT_STEP_MODEL.into(),
                max_tokens: 8_000,
                effort: Some(Effort::Low),
                server_fallback: false,
            },
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(120),
        }
    }
}

pub struct ClaudeProvider {
    http: reqwest::Client,
    key: ApiKey,
    config: ClaudeConfig,
}

impl ClaudeProvider {
    pub fn new(key: ApiKey, config: ClaudeConfig) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|err| AiError::Network(err.to_string()))?;
        Ok(Self { http, key, config })
    }

    /// Use the key stored in the Keychain; [`AiError::MissingApiKey`] when
    /// none is set.
    pub fn from_keychain(config: ClaudeConfig) -> Result<Self, AiError> {
        let key = keychain::get_api_key()?.ok_or(AiError::MissingApiKey)?;
        Self::new(key, config)
    }

    pub fn config(&self) -> &ClaudeConfig {
        &self.config
    }

    /// Send one Messages request with retries; returns the parsed response.
    /// A reply without the expected tool call is retried once.
    async fn send(
        &self,
        settings: &ModelSettings,
        body: &Value,
        tool: &str,
    ) -> Result<Parsed, AiError> {
        let url = format!("{}/v1/messages", self.config.base_url.trim_end_matches('/'));
        let headers = request_headers(&self.config, settings, &self.key)?;
        retry_missing_tool_call(|| async {
            let value = post_json(&self.http, &url, &headers, body, self.config.retry).await?;
            parse_message(&value, tool)
        })
        .await
    }
}

/// POST `body` as JSON with retries ([`with_retry`]); the JSON body of the
/// success. A non-success status becomes [`AiError::Api`] via
/// [`parse_error`]. Shared by the providers.
pub(crate) async fn post_json(
    http: &reqwest::Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
    retry: RetryPolicy,
) -> Result<Value, AiError> {
    with_retry(retry, || async {
        let response = http
            .post(url)
            .headers(headers.clone())
            .json(body)
            .send()
            .await
            .map_err(|err| AiError::Network(err.without_url().to_string()))?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
        };
        let retry_after = retry_after_from_headers(header("retry-after-ms"), header("retry-after"));
        let text = response
            .text()
            .await
            .map_err(|err| AiError::Network(err.without_url().to_string()))?;
        if !(200..300).contains(&status) {
            return Err(parse_error(status, &text, retry_after));
        }
        serde_json::from_str::<Value>(&text)
            .map_err(|err| AiError::InvalidOutput(format!("response is not JSON: {err}")))
    })
    .await
}

/// Run `op` again, once, if it fails with [`AiError::MissingToolCall`].
pub(crate) async fn retry_missing_tool_call<T, Op, Fut>(mut op: Op) -> Result<T, AiError>
where
    Op: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, AiError>>,
{
    match op().await {
        Err(AiError::MissingToolCall { .. }) => op().await,
        result => result,
    }
}

impl AiProvider for ClaudeProvider {
    fn generate_step<'a>(&'a self, input: &'a StepInput) -> AiFuture<'a, StepOutput> {
        Box::pin(async move {
            let settings = &self.config.step;
            let parsed = self
                .send(settings, &step_request(&self.config, input), STEP_TOOL)
                .await?;
            parse_step_output(input, parsed)
        })
    }

    fn generate_guide<'a>(&'a self, input: &'a GuideInput) -> AiFuture<'a, GuideOutput> {
        Box::pin(async move {
            let settings = &self.config.guide;
            let parsed = self
                .send(settings, &guide_request(&self.config, input), GUIDE_TOOL)
                .await?;
            parse_guide_output(input, parsed)
        })
    }

    fn rewrite<'a>(&'a self, input: &'a RewriteInput) -> AiFuture<'a, RewriteOutput> {
        Box::pin(async move {
            let settings = &self.config.rewrite;
            let parsed = self
                .send(
                    settings,
                    &rewrite_request(&self.config, input),
                    REWRITE_TOOL,
                )
                .await?;
            parse_rewrite_output(parsed)
        })
    }
}

// ---------------------------------------------------------------------------
// Requests

/// HTTP headers for a request. The `x-api-key` value is marked sensitive
/// so it is redacted from `Debug` output; never log these anyway.
pub(crate) fn request_headers(
    config: &ClaudeConfig,
    settings: &ModelSettings,
    key: &ApiKey,
) -> Result<HeaderMap, AiError> {
    let mut api_key = HeaderValue::from_str(key.expose()).map_err(|_| AiError::InvalidApiKey)?;
    api_key.set_sensitive(true);
    let version = HeaderValue::from_str(&config.api_version)
        .map_err(|_| AiError::InvalidOutput("invalid API version header".into()))?;
    let mut headers = HeaderMap::new();
    headers.insert(HeaderName::from_static("x-api-key"), api_key);
    headers.insert(HeaderName::from_static("anthropic-version"), version);
    if config.sends_fallbacks(settings) {
        headers.insert(
            HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_static(SERVER_FALLBACK_BETA),
        );
    }
    Ok(headers)
}

fn build_request(
    config: &ClaudeConfig,
    settings: &ModelSettings,
    prompt: Prompt,
    tool: Value,
    content: Vec<Value>,
) -> Value {
    let mut body = json!({
        "model": settings.model,
        "max_tokens": settings.max_tokens,
        "system": prompt.text,
        "messages": [{ "role": "user", "content": content }],
        "tools": [tool],
        "tool_choice": { "type": "auto", "disable_parallel_tool_use": true },
    });
    if let Some(effort) = settings.effort {
        body["output_config"] = json!({ "effort": effort });
    }
    if config.sends_fallbacks(settings) {
        body["fallbacks"] = json!("default");
    }
    body
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "strict": true,
        "input_schema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    })
}

pub(crate) fn step_tool() -> Value {
    tool(
        STEP_TOOL,
        "Record the generated text and redaction suggestions for this step.",
        json!({
            "title": { "type": "string", "description": "Imperative step title in Markdown, at most about 8 words." },
            "description": { "type": "string", "description": "One short line of context in Markdown, or an empty string." },
            "redactions": {
                "type": "array",
                "description": "Sensitive regions visible in the images; empty when none. Coordinates are fractions (0 to 1) of the named image's width and height.",
                "items": {
                    "type": "object",
                    "properties": {
                        "image": { "type": "string", "enum": ["crop", "full"] },
                        "x": { "type": "number", "description": "Left edge, 0 to 1." },
                        "y": { "type": "number", "description": "Top edge, 0 to 1." },
                        "width": { "type": "number", "description": "0 to 1." },
                        "height": { "type": "number", "description": "0 to 1." },
                        "reason": { "type": "string" },
                    },
                    "required": ["image", "x", "y", "width", "height", "reason"],
                    "additionalProperties": false,
                },
            },
        }),
        &["title", "description", "redactions"],
    )
}

pub(crate) fn guide_tool() -> Value {
    tool(
        GUIDE_TOOL,
        "Record the guide-level fields and any consistency edits.",
        json!({
            "title": { "type": "string" },
            "intro": { "type": "string" },
            "prerequisites": { "type": "string" },
            "summary": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "stepEdits": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "n": { "type": "integer", "description": "1-based step number." },
                        "title": { "type": "string", "description": "New title, or an empty string to keep it." },
                        "description": { "type": "string", "description": "New description, or an empty string to keep it." },
                    },
                    "required": ["n", "title", "description"],
                    "additionalProperties": false,
                },
            },
        }),
        &[
            "title",
            "intro",
            "prerequisites",
            "summary",
            "tags",
            "stepEdits",
        ],
    )
}

pub(crate) fn rewrite_tool() -> Value {
    tool(
        REWRITE_TOOL,
        "Record the rewritten step text.",
        json!({
            "title": { "type": "string" },
            "description": { "type": "string", "description": "One short line, or an empty string." },
        }),
        &["title", "description"],
    )
}

fn image_block(image: &ImageData) -> Value {
    debug_assert!(
        fits_api_limit(image.width, image.height),
        "image {}x{} exceeds the API size limit; downscale with ai::fit_for_api first",
        image.width,
        image.height,
    );
    json!({
        "type": "image",
        "source": {
            "type": "base64",
            "media_type": image.media_type.mime(),
            "data": base64::encode(&image.bytes),
        },
    })
}

pub(crate) fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("model types serialize")
}

/// A context object as the text the model reads.
pub(crate) fn context_text(context: &Value) -> String {
    serde_json::to_string_pretty(context).expect("JSON values serialize")
}

fn text_block(context: &Value) -> Value {
    json!({ "type": "text", "text": context_text(context) })
}

/// The step's images in the order they are sent, with their names.
pub(crate) fn step_images(input: &StepInput) -> Vec<(&'static str, &ImageData)> {
    [("crop", input.crop.as_ref()), ("full", input.full.as_ref())]
        .into_iter()
        .filter_map(|(name, image)| Some((name, image?)))
        .collect()
}

/// The per-step context block: the step's metadata and which image is
/// which. Without images (a text-only model), `images` is empty and the
/// model works from the element, keystrokes and step kind alone.
pub(crate) fn step_context(input: &StepInput) -> Value {
    let images = step_images(input);
    let image_info: Vec<Value> = images
        .iter()
        .enumerate()
        .map(|(i, (name, image))| {
            json!({
                "position": i + 1,
                "image": name,
                "shows": if *name == "crop" { "area around the interaction point" } else { "whole screen, downscaled" },
                "width": image.width,
                "height": image.height,
            })
        })
        .collect();
    json!({
        "kind": to_json(&input.kind),
        "templateTitle": input.template_title,
        "templateDescription": input.template_description,
        "element": input.element.as_ref().map(to_json),
        "keystrokes": input.keystrokes,
        "previousStep": input.previous_title,
        "nextStep": input.next_title,
        "guideTitle": input.guide_title,
        "images": image_info,
        "language": input.language,
        "audience": to_json(&input.audience),
    })
}

pub fn step_request(config: &ClaudeConfig, input: &StepInput) -> Value {
    let settings = &config.step;
    let mut content: Vec<Value> = step_images(input)
        .iter()
        .map(|(_, image)| image_block(image))
        .collect();
    content.push(text_block(&step_context(input)));
    build_request(config, settings, prompts::STEP, step_tool(), content)
}

/// The guide-level context block: the step texts and guide settings.
pub(crate) fn guide_context(input: &GuideInput) -> Value {
    let steps: Vec<Value> = input
        .steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            json!({
                "n": i + 1,
                "kind": to_json(&step.kind),
                "title": step.title,
                "description": step.description,
                "app": step.app,
                "editable": step.editable,
            })
        })
        .collect();
    json!({
        "currentTitle": input.current_title,
        "steps": steps,
        "apps": input.apps,
        "language": input.language,
        "audience": to_json(&input.audience),
    })
}

pub fn guide_request(config: &ClaudeConfig, input: &GuideInput) -> Value {
    build_request(
        config,
        &config.guide,
        prompts::GUIDE,
        guide_tool(),
        vec![text_block(&guide_context(input))],
    )
}

fn rewrite_instruction(style: &RewriteStyle) -> String {
    match style {
        RewriteStyle::Shorter => {
            "Make it shorter. Drop the description unless it is essential.".into()
        }
        RewriteStyle::Beginner => {
            "Rewrite for a beginner: plain words, and say where the control is.".into()
        }
        RewriteStyle::Expert => {
            "Rewrite for an expert: as terse as possible, no description.".into()
        }
        RewriteStyle::Custom(text) => text.clone(),
    }
}

/// The rewrite context block: the step text and the instruction.
pub(crate) fn rewrite_context(input: &RewriteInput) -> Value {
    json!({
        "kind": to_json(&input.kind),
        "title": input.title,
        "description": input.description,
        "element": input.element.as_ref().map(to_json),
        "language": input.language,
        "instruction": rewrite_instruction(&input.style),
    })
}

pub fn rewrite_request(config: &ClaudeConfig, input: &RewriteInput) -> Value {
    build_request(
        config,
        &config.rewrite,
        prompts::REWRITE,
        rewrite_tool(),
        vec![text_block(&rewrite_context(input))],
    )
}

// ---------------------------------------------------------------------------
// Responses

/// The useful parts of a successful Messages response.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    /// Model that answered (may differ from the request after a fallback).
    pub model: String,
    /// Input of the expected tool call.
    pub tool_input: Value,
    pub usage: Usage,
}

/// Extract the expected tool call from a Messages response body.
pub fn parse_message(body: &Value, tool: &str) -> Result<Parsed, AiError> {
    let stop_reason = body["stop_reason"].as_str().unwrap_or_default();
    match stop_reason {
        "refusal" => {
            return Err(AiError::Refusal {
                category: body["stop_details"]["category"].as_str().map(str::to_owned),
            })
        }
        "max_tokens" => return Err(AiError::Truncated),
        _ => {}
    }
    let content = body["content"]
        .as_array()
        .ok_or_else(|| AiError::InvalidOutput("response has no content".into()))?;
    let tool_input = content
        .iter()
        .find(|block| block["type"] == "tool_use" && block["name"] == tool)
        .map(|block| block["input"].clone())
        .ok_or_else(|| AiError::MissingToolCall {
            tool: tool.to_owned(),
        })?;
    if !tool_input.is_object() {
        return Err(AiError::InvalidOutput(format!(
            "{tool} input is not an object"
        )));
    }
    let usage = &body["usage"];
    let count = |field: &str| usage[field].as_u64().unwrap_or(0);
    Ok(Parsed {
        model: body["model"].as_str().unwrap_or_default().to_owned(),
        tool_input,
        usage: Usage {
            input_tokens: count("input_tokens"),
            output_tokens: count("output_tokens"),
            cache_read_input_tokens: count("cache_read_input_tokens"),
            cache_creation_input_tokens: count("cache_creation_input_tokens"),
        },
    })
}

/// Build an [`AiError::Api`] from a non-success response. Reads the
/// Anthropic and OpenAI shape (`error.type`, `error.message`) and the bare
/// `error` string some local servers send.
pub fn parse_error(status: u16, body: &str, retry_after: Option<Duration>) -> AiError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let error = parsed.as_ref().map(|value| &value["error"]);
    let kind = error
        .and_then(|e| e["type"].as_str())
        .unwrap_or("http_error")
        .to_owned();
    let message = match error.and_then(|e| e["message"].as_str().or(e.as_str())) {
        Some(message) => message.to_owned(),
        None => body.chars().take(200).collect(),
    };
    AiError::Api {
        status,
        kind,
        message,
        retry_after,
    }
}

fn field<'v>(input: &'v Value, name: &str) -> Result<&'v str, AiError> {
    input[name]
        .as_str()
        .ok_or_else(|| AiError::InvalidOutput(format!("missing string field `{name}`")))
}

fn non_empty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn required_text(input: &Value, name: &str) -> Result<String, AiError> {
    non_empty(field(input, name)?).ok_or_else(|| AiError::InvalidOutput(format!("empty `{name}`")))
}

fn meta(parsed: &Parsed, prompt: Prompt) -> GenerationMeta {
    GenerationMeta {
        model: parsed.model.clone(),
        prompt_version: prompt.version.into(),
        usage: parsed.usage,
    }
}

/// Map a box given as fractions (0 to 1) of an image to full-screenshot
/// pixels through the image's `source_rect`, clipped to the image.
/// Fractions are independent of any resizing the API applies. `None` when
/// less than a pixel is left.
fn to_screenshot(image: &ImageData, x: f64, y: f64, width: f64, height: f64) -> Option<Rect> {
    let x0 = x.clamp(0.0, 1.0);
    let y0 = y.clamp(0.0, 1.0);
    let x1 = (x + width).clamp(0.0, 1.0);
    let y1 = (y + height).clamp(0.0, 1.0);
    let src = image.source_rect;
    let rect = Rect {
        x: src.x + x0 * src.width,
        y: src.y + y0 * src.height,
        width: (x1 - x0) * src.width,
        height: (y1 - y0) * src.height,
    };
    (rect.width >= 1.0 && rect.height >= 1.0).then_some(rect)
}

pub fn parse_step_output(input: &StepInput, parsed: Parsed) -> Result<StepOutput, AiError> {
    let out = &parsed.tool_input;
    let title = required_text(out, "title")?;
    let description = non_empty(field(out, "description")?);
    let images = step_images(input);
    let redaction_suggestions = out["redactions"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|r| {
            let (_, image) = images.iter().find(|(name, _)| r["image"] == *name)?;
            let bbox = to_screenshot(
                image,
                r["x"].as_f64()?,
                r["y"].as_f64()?,
                r["width"].as_f64()?,
                r["height"].as_f64()?,
            )?;
            Some(RedactionSuggestion {
                rect: bbox,
                reason: r["reason"].as_str().unwrap_or_default().trim().to_owned(),
                source: SuggestionSource::Ai,
                preview: None,
                review: None,
            })
        })
        .collect();
    Ok(StepOutput {
        title,
        description,
        redaction_suggestions,
        meta: meta(&parsed, prompts::STEP),
    })
}

pub fn parse_guide_output(input: &GuideInput, parsed: Parsed) -> Result<GuideOutput, AiError> {
    let out = &parsed.tool_input;
    let mut tags: Vec<String> = Vec::new();
    for tag in out["tags"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        if let Some(tag) = tag.as_str().and_then(non_empty) {
            let tag = tag.to_lowercase();
            if !tags.contains(&tag) {
                tags.push(tag);
            }
        }
    }
    let step_edits = out["stepEdits"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|edit| {
            let n = usize::try_from(edit["n"].as_u64()?).ok()?;
            let step = input.steps.get(n.checked_sub(1)?)?;
            if !step.editable {
                return None;
            }
            let title = edit["title"].as_str().and_then(non_empty);
            let description = edit["description"].as_str().and_then(non_empty);
            (title.is_some() || description.is_some()).then_some(StepEdit {
                step_id: step.step_id,
                title,
                description,
            })
        })
        .collect();
    Ok(GuideOutput {
        title: required_text(out, "title")?,
        intro: field(out, "intro")?.trim().to_owned(),
        prerequisites: field(out, "prerequisites")?.trim().to_owned(),
        summary: field(out, "summary")?.trim().to_owned(),
        tags,
        step_edits,
        meta: meta(&parsed, prompts::GUIDE),
    })
}

pub fn parse_rewrite_output(parsed: Parsed) -> Result<RewriteOutput, AiError> {
    let out = &parsed.tool_input;
    Ok(RewriteOutput {
        title: required_text(out, "title")?,
        description: non_empty(field(out, "description")?),
        meta: meta(&parsed, prompts::REWRITE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{GuideStepText, ImageMediaType};
    use crate::model::{Audience, ElementInfo, StepKind};
    use ulid::Ulid;

    const STEP_ID: &str = "01J9ZQ3K8V4N6T2W5X7Y8Z9A0D";

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn step_input() -> StepInput {
        StepInput {
            step_id: STEP_ID.parse().unwrap(),
            kind: StepKind::Click,
            template_title: "Click **Save**".into(),
            template_description: String::new(),
            element: Some(ElementInfo {
                role: Some("AXButton".into()),
                label: Some("Save".into()),
                app: Some("TextEdit".into()),
                ..Default::default()
            }),
            keystrokes: None,
            // Crop: 200x100 image of the 400x200 region at (1000, 500).
            crop: Some(ImageData {
                media_type: ImageMediaType::Webp,
                bytes: b"foo".to_vec(),
                width: 200,
                height: 100,
                source_rect: rect(1000.0, 500.0, 400.0, 200.0),
            }),
            // Full: 1280x800 downscale of a 2560x1600 screenshot.
            full: Some(ImageData {
                media_type: ImageMediaType::Png,
                bytes: b"foobar".to_vec(),
                width: 1280,
                height: 800,
                source_rect: rect(0.0, 0.0, 2560.0, 1600.0),
            }),
            previous_title: Some("Open **TextEdit**".into()),
            next_title: None,
            guide_title: None,
            language: "en".into(),
            audience: Audience::Beginner,
        }
    }

    #[test]
    fn default_models() {
        let config = ClaudeConfig::default();
        assert_eq!(config.step.model, "claude-haiku-5-5");
        assert_eq!(config.rewrite.model, "claude-haiku-5-5");
        assert_eq!(config.guide.model, "claude-sonnet-5-5");
        assert_eq!(config.api_version, "2023-06-01");
    }

    #[test]
    fn headers() {
        let config = ClaudeConfig::default();
        let key = ApiKey::new("sk-test").unwrap();
        let names = |headers: &HeaderMap| {
            let mut names: Vec<String> = headers.keys().map(|n| n.to_string()).collect();
            names.sort();
            names
        };
        let step = request_headers(&config, &config.step, &key).unwrap();
        assert_eq!(names(&step), ["anthropic-version", "x-api-key"]);
        assert!(step["x-api-key"].is_sensitive());
        assert!(!format!("{step:?}").contains("sk-test"));
        assert_eq!(step["anthropic-version"], "2023-06-01");

        let guide = request_headers(&config, &config.guide, &key).unwrap();
        assert_eq!(
            names(&guide),
            ["anthropic-beta", "anthropic-version", "x-api-key"]
        );
        assert_eq!(guide["anthropic-beta"], SERVER_FALLBACK_BETA);
    }

    #[test]
    fn fallbacks_only_on_first_party_api() {
        let mut config = ClaudeConfig::default();
        assert!(config.sends_fallbacks(&config.guide));
        assert!(!config.sends_fallbacks(&config.step));
        config.base_url = format!("{DEFAULT_BASE_URL}/");
        assert!(config.sends_fallbacks(&config.guide));

        config.base_url = "https://proxy.example.com".into();
        assert!(!config.sends_fallbacks(&config.guide));
        let body = guide_request(&config, &guide_input());
        assert!(body.get("fallbacks").is_none());
        let key = ApiKey::new("sk-test").unwrap();
        let headers = request_headers(&config, &config.guide, &key).unwrap();
        assert!(!headers.contains_key("anthropic-beta"));
    }

    #[test]
    #[should_panic(expected = "exceeds the API size limit")]
    fn oversized_image_is_caught_in_debug() {
        let mut input = step_input();
        if let Some(full) = input.full.as_mut() {
            full.width = 2560;
            full.height = 1600;
        }
        step_request(&ClaudeConfig::default(), &input);
    }

    #[test]
    fn step_request_json() {
        let config = ClaudeConfig::default();
        let body = step_request(&config, &step_input());
        let context = json!({
            "kind": "click",
            "templateTitle": "Click **Save**",
            "templateDescription": "",
            "element": { "role": "AXButton", "label": "Save", "app": "TextEdit" },
            "keystrokes": null,
            "previousStep": "Open **TextEdit**",
            "nextStep": null,
            "guideTitle": null,
            "images": [
                { "position": 1, "image": "crop", "shows": "area around the interaction point", "width": 200, "height": 100 },
                { "position": 2, "image": "full", "shows": "whole screen, downscaled", "width": 1280, "height": 800 },
            ],
            "language": "en",
            "audience": "beginner",
        });
        let expected = json!({
            "model": "claude-haiku-5-5",
            "max_tokens": 16000,
            "system": prompts::STEP.text,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/webp", "data": "Zm9v" } },
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "Zm9vYmFy" } },
                    { "type": "text", "text": serde_json::to_string_pretty(&context).unwrap() },
                ],
            }],
            "tools": [step_tool()],
            "tool_choice": { "type": "auto", "disable_parallel_tool_use": true },
            "output_config": { "effort": "low" },
        });
        assert_eq!(body, expected);
        // Re-parse the context text to check it round-trips.
        let text = body["messages"][0]["content"][2]["text"].as_str().unwrap();
        assert_eq!(serde_json::from_str::<Value>(text).unwrap(), context);
    }

    #[test]
    fn step_tool_is_strict() {
        let tool = step_tool();
        assert_eq!(tool["name"], "record_step");
        assert_eq!(tool["strict"], true);
        assert_eq!(tool["input_schema"]["additionalProperties"], false);
        assert_eq!(
            tool["input_schema"]["required"],
            json!(["title", "description", "redactions"])
        );
        let item = &tool["input_schema"]["properties"]["redactions"]["items"];
        assert_eq!(item["additionalProperties"], false);
        assert_eq!(item["properties"]["image"]["enum"], json!(["crop", "full"]));
    }

    #[test]
    fn step_request_without_images_or_effort() {
        let mut config = ClaudeConfig::default();
        config.step.effort = None;
        let mut input = step_input();
        input.crop = None;
        input.full = None;
        let body = step_request(&config, &input);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert!(body.get("output_config").is_none());
        assert!(body.get("fallbacks").is_none());
    }

    fn guide_input() -> GuideInput {
        GuideInput {
            current_title: "Untitled".into(),
            steps: vec![
                GuideStepText {
                    step_id: Ulid::from_parts(1, 1),
                    kind: StepKind::Click,
                    title: "Click **File**".into(),
                    description: String::new(),
                    app: Some("TextEdit".into()),
                    editable: true,
                },
                GuideStepText {
                    step_id: Ulid::from_parts(2, 2),
                    kind: StepKind::Shortcut,
                    title: "Press **⌘S**".into(),
                    description: "Saves the file.".into(),
                    app: None,
                    editable: false,
                },
            ],
            apps: vec!["TextEdit".into()],
            language: "en".into(),
            audience: Audience::Expert,
        }
    }

    #[test]
    fn guide_request_json() {
        let config = ClaudeConfig::default();
        let body = guide_request(&config, &guide_input());
        let context = json!({
            "currentTitle": "Untitled",
            "steps": [
                { "n": 1, "kind": "click", "title": "Click **File**", "description": "", "app": "TextEdit", "editable": true },
                { "n": 2, "kind": "shortcut", "title": "Press **⌘S**", "description": "Saves the file.", "app": null, "editable": false },
            ],
            "apps": ["TextEdit"],
            "language": "en",
            "audience": "expert",
        });
        let expected = json!({
            "model": "claude-sonnet-5-5",
            "max_tokens": 16000,
            "system": prompts::GUIDE.text,
            "messages": [{
                "role": "user",
                "content": [{ "type": "text", "text": serde_json::to_string_pretty(&context).unwrap() }],
            }],
            "tools": [guide_tool()],
            "tool_choice": { "type": "auto", "disable_parallel_tool_use": true },
            "output_config": { "effort": "medium" },
            "fallbacks": "default",
        });
        assert_eq!(body, expected);
    }

    #[test]
    fn rewrite_request_json() {
        let config = ClaudeConfig::default();
        let input = RewriteInput {
            step_id: Ulid::from_parts(1, 1),
            kind: StepKind::Type,
            title: "Type your name".into(),
            description: String::new(),
            element: None,
            style: RewriteStyle::Custom("Use British English.".into()),
            language: "en-GB".into(),
        };
        let body = rewrite_request(&config, &input);
        assert_eq!(body["model"], "claude-haiku-5-5");
        assert_eq!(body["max_tokens"], 8000);
        assert_eq!(body["tools"][0]["name"], "record_rewrite");
        let text = body["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({
                "kind": "type",
                "title": "Type your name",
                "description": "",
                "element": null,
                "language": "en-GB",
                "instruction": "Use British English.",
            })
        );
        for style in [
            RewriteStyle::Shorter,
            RewriteStyle::Beginner,
            RewriteStyle::Expert,
        ] {
            assert!(!rewrite_instruction(&style).is_empty());
        }
    }

    fn response(content: Value, stop_reason: &str) -> Value {
        json!({
            "id": "msg_01",
            "type": "message",
            "role": "assistant",
            "model": "claude-haiku-5-5",
            "content": content,
            "stop_reason": stop_reason,
            "stop_details": null,
            "usage": {
                "input_tokens": 1200,
                "output_tokens": 85,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
        })
    }

    fn step_tool_use(input: Value) -> Value {
        response(
            json!([
                { "type": "thinking", "thinking": "", "signature": "sig" },
                { "type": "text", "text": "Recording the step." },
                { "type": "tool_use", "id": "toolu_01", "name": "record_step", "input": input },
            ]),
            "tool_use",
        )
    }

    #[test]
    fn parses_step_tool_use() {
        let body = step_tool_use(json!({
            "title": " Click **Save** in the toolbar. ",
            "description": "",
            "redactions": [
                // Fractions of the crop, which covers (1000, 500, 400x200).
                { "image": "crop", "x": 0.25, "y": 0.25, "width": 0.25, "height": 0.125, "reason": "email address" },
                // Fractions of the full image, partly off-image, clipped.
                { "image": "full", "x": 0.9375, "y": 0.9375, "width": 0.125, "height": 0.5, "reason": "name" },
                // Zero-size or wholly outside, dropped.
                { "image": "crop", "x": 0.1, "y": 0.1, "width": 0, "height": 0.1, "reason": "x" },
                { "image": "full", "x": 1.5, "y": 0.1, "width": 0.2, "height": 0.1, "reason": "x" },
                // Unknown image, dropped.
                { "image": "after", "x": 0, "y": 0, "width": 0.5, "height": 0.5, "reason": "x" },
            ],
        }));
        let parsed = parse_message(&body, STEP_TOOL).unwrap();
        assert_eq!(parsed.model, "claude-haiku-5-5");
        assert_eq!(parsed.usage.input_tokens, 1200);
        assert_eq!(parsed.usage.output_tokens, 85);
        let out = parse_step_output(&step_input(), parsed).unwrap();
        assert_eq!(out.title, "Click **Save** in the toolbar.");
        assert_eq!(out.description, None);
        assert_eq!(out.meta.prompt_version, "step.v1");
        assert_eq!(
            out.redaction_suggestions,
            [
                RedactionSuggestion {
                    rect: rect(1100.0, 550.0, 100.0, 25.0),
                    reason: "email address".into(),
                    source: SuggestionSource::Ai,
                    preview: None,
                    review: None,
                },
                RedactionSuggestion {
                    rect: rect(2400.0, 1500.0, 160.0, 100.0),
                    reason: "name".into(),
                    source: SuggestionSource::Ai,
                    preview: None,
                    review: None,
                },
            ]
        );
    }

    #[test]
    fn rejects_missing_or_bad_tool_output() {
        let text_only = response(
            json!([{ "type": "text", "text": "Click Save." }]),
            "end_turn",
        );
        assert_eq!(
            parse_message(&text_only, STEP_TOOL),
            Err(AiError::MissingToolCall {
                tool: "record_step".into()
            })
        );
        let wrong_tool = response(
            json!([{ "type": "tool_use", "id": "t", "name": "other", "input": {} }]),
            "tool_use",
        );
        assert!(matches!(
            parse_message(&wrong_tool, STEP_TOOL),
            Err(AiError::MissingToolCall { .. })
        ));
        let parsed = parse_message(
            &step_tool_use(json!({ "title": "  ", "description": "", "redactions": [] })),
            STEP_TOOL,
        )
        .unwrap();
        assert_eq!(
            parse_step_output(&step_input(), parsed),
            Err(AiError::InvalidOutput("empty `title`".into()))
        );
        let parsed = parse_message(&step_tool_use(json!({ "title": "Click" })), STEP_TOOL).unwrap();
        assert_eq!(
            parse_step_output(&step_input(), parsed),
            Err(AiError::InvalidOutput(
                "missing string field `description`".into()
            ))
        );
        assert!(parse_message(&json!({ "stop_reason": "end_turn" }), STEP_TOOL).is_err());
    }

    #[test]
    fn parses_refusal_and_truncation() {
        let mut refusal = response(json!([]), "refusal");
        refusal["stop_details"] =
            json!({ "type": "refusal", "category": "cyber", "explanation": null });
        assert_eq!(
            parse_message(&refusal, STEP_TOOL),
            Err(AiError::Refusal {
                category: Some("cyber".into())
            })
        );
        let truncated = response(
            json!([{ "type": "tool_use", "id": "t", "name": "record_step", "input": {} }]),
            "max_tokens",
        );
        assert_eq!(
            parse_message(&truncated, STEP_TOOL),
            Err(AiError::Truncated)
        );
    }

    #[test]
    fn parses_api_errors() {
        let err = parse_error(
            429,
            r#"{"type":"error","error":{"type":"rate_limit_error","message":"Rate limited"},"request_id":"req_1"}"#,
            Some(Duration::from_secs(7)),
        );
        assert_eq!(
            err,
            AiError::Api {
                status: 429,
                kind: "rate_limit_error".into(),
                message: "Rate limited".into(),
                retry_after: Some(Duration::from_secs(7)),
            }
        );
        assert!(err.is_retryable());

        let overloaded = parse_error(
            529,
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            None,
        );
        assert!(overloaded.is_retryable());

        let bad = parse_error(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: too large"}}"#,
            None,
        );
        assert!(!bad.is_retryable());

        let html = parse_error(502, "<html>Bad Gateway</html>", None);
        assert_eq!(
            html,
            AiError::Api {
                status: 502,
                kind: "http_error".into(),
                message: "<html>Bad Gateway</html>".into(),
                retry_after: None,
            }
        );
        assert!(html.is_retryable());
    }

    #[test]
    fn parses_guide_output() {
        let body = response(
            json!([{ "type": "tool_use", "id": "t", "name": "record_guide", "input": {
                "title": "Save a document in TextEdit",
                "intro": "Save your work. ",
                "prerequisites": "",
                "summary": "Save a TextEdit document.",
                "tags": ["TextEdit", "files", "textedit", " "],
                "stepEdits": [
                    { "n": 1, "title": "Click **File** in the menu bar", "description": "" },
                    // Not editable: dropped.
                    { "n": 2, "title": "Press **⌘ S**", "description": "" },
                    // Out of range or no change: dropped.
                    { "n": 0, "title": "x", "description": "" },
                    { "n": 9, "title": "x", "description": "" },
                    { "n": 1, "title": "", "description": "" },
                ],
            }}]),
            "tool_use",
        );
        let input = guide_input();
        let out = parse_guide_output(&input, parse_message(&body, GUIDE_TOOL).unwrap()).unwrap();
        assert_eq!(out.title, "Save a document in TextEdit");
        assert_eq!(out.intro, "Save your work.");
        assert_eq!(out.prerequisites, "");
        assert_eq!(out.tags, ["textedit", "files"]);
        assert_eq!(
            out.step_edits,
            [StepEdit {
                step_id: input.steps[0].step_id,
                title: Some("Click **File** in the menu bar".into()),
                description: None,
            }]
        );
        assert_eq!(out.meta.prompt_version, "guide.v1");
    }

    #[test]
    fn parses_rewrite_output() {
        let body = response(
            json!([{ "type": "tool_use", "id": "t", "name": "record_rewrite", "input": {
                "title": "Click **Save**",
                "description": "It is in the toolbar.",
            }}]),
            "tool_use",
        );
        let out = parse_rewrite_output(parse_message(&body, REWRITE_TOOL).unwrap()).unwrap();
        assert_eq!(out.title, "Click **Save**");
        assert_eq!(out.description.as_deref(), Some("It is in the toolbar."));
        assert_eq!(out.meta.prompt_version, "rewrite.v1");
    }

    #[tokio::test]
    async fn missing_tool_call_is_retried_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let missing = || AiError::MissingToolCall {
            tool: STEP_TOOL.into(),
        };

        let calls = AtomicU32::new(0);
        let result = retry_missing_tool_call(|| async {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Err(missing()),
                _ => Ok("ok"),
            }
        })
        .await;
        assert_eq!(result, Ok("ok"));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let calls = AtomicU32::new(0);
        let result: Result<(), _> = retry_missing_tool_call(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(missing())
        })
        .await;
        assert_eq!(result, Err(missing()));
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let calls = AtomicU32::new(0);
        let result: Result<(), _> = retry_missing_tool_call(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(AiError::Truncated)
        })
        .await;
        assert_eq!(result, Err(AiError::Truncated));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn provider_from_key_does_not_touch_network() {
        let provider =
            ClaudeProvider::new(ApiKey::new("sk-test").unwrap(), ClaudeConfig::default());
        assert!(provider.is_ok());
        let _: &dyn AiProvider = &provider.unwrap();
    }
}
