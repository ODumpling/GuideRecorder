//! AI provider abstraction and Claude implementation. See design scratchpad
//! #442 ("Pipeline"); implemented in #361.
//!
//! - [`AiProvider`] is the provider-neutral interface: a per-step pass
//!   ([`AiProvider::generate_step`], which also returns redaction
//!   suggestions), a guide-level pass ([`AiProvider::generate_guide`]) and
//!   step rewrites ([`AiProvider::rewrite`]).
//! - [`claude::ClaudeProvider`] implements it over the Anthropic Messages API.
//! - [`keychain`] stores the API key in the macOS Keychain.
//! - [`concurrency::for_each_bounded`] runs per-step calls in parallel with a
//!   limit; [`retry`] holds the backoff policy.
//! - [`mock::MockProvider`] is a deterministic provider for tests downstream.
//! - [`pipeline::run`] runs both passes over a guide and applies the results.
//!
//! Outputs only carry generated text; callers decide what to apply. Steps
//! whose [`AiStatus`](crate::model::AiStatus) is `edited` must never be
//! overwritten.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

pub use crate::model::RedactionSuggestion;
use crate::model::{Audience, ElementInfo, Rect, StepKind};

mod base64;
pub mod claude;
pub mod concurrency;
pub mod keychain;
pub mod mock;
pub mod pipeline;
pub mod prompts;
pub mod retry;

pub use claude::{ClaudeConfig, ClaudeProvider, Effort, ModelSettings};
pub use keychain::ApiKey;
pub use mock::MockProvider;
pub use pipeline::{CancelToken, ImageSource, PipelineEvent, PipelineOptions, PipelineSummary};

/// Boxed future returned by [`AiProvider`] methods, so the trait stays
/// usable as `dyn AiProvider`.
pub type AiFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AiError>> + Send + 'a>>;

/// A text-generation backend for guides.
pub trait AiProvider: Send + Sync {
    /// Per-step pass: title, description and redaction suggestions for one
    /// step, from its images and metadata.
    fn generate_step<'a>(&'a self, input: &'a StepInput) -> AiFuture<'a, StepOutput>;

    /// Guide-level pass: guide title, intro, prerequisites, summary, tags and
    /// optional consistency edits, from step texts only.
    fn generate_guide<'a>(&'a self, input: &'a GuideInput) -> AiFuture<'a, GuideOutput>;

    /// Rewrite one step's text in the requested style.
    fn rewrite<'a>(&'a self, input: &'a RewriteInput) -> AiFuture<'a, RewriteOutput>;
}

/// Image encoding accepted by the providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageMediaType {
    Png,
    Webp,
}

impl ImageMediaType {
    pub fn mime(self) -> &'static str {
        match self {
            ImageMediaType::Png => "image/png",
            ImageMediaType::Webp => "image/webp",
        }
    }
}

/// Longest image edge, in pixels, the API takes without resizing.
pub const MAX_IMAGE_LONG_EDGE: u32 = 1568;
/// Largest image area, in pixels, the API takes without resizing.
pub const MAX_IMAGE_PIXELS: u64 = 1_150_000;

/// Whether a `width` x `height` image is within the API limits
/// ([`MAX_IMAGE_LONG_EDGE`], [`MAX_IMAGE_PIXELS`]).
pub fn fits_api_limit(width: u32, height: u32) -> bool {
    width.max(height) <= MAX_IMAGE_LONG_EDGE
        && u64::from(width) * u64::from(height) <= MAX_IMAGE_PIXELS
}

/// The size to downscale a `width` x `height` image to before sending, so
/// it is within the API limits. Keeps the aspect ratio; images that already
/// fit are returned unchanged. Never returns a zero dimension.
pub fn fit_for_api(width: u32, height: u32) -> (u32, u32) {
    if fits_api_limit(width, height) {
        return (width, height);
    }
    let (w, h) = (f64::from(width), f64::from(height));
    let by_edge = f64::from(MAX_IMAGE_LONG_EDGE) / w.max(h);
    let by_area = (MAX_IMAGE_PIXELS as f64 / (w * h)).sqrt();
    let scale = by_edge.min(by_area);
    let fit = |n: f64| ((n * scale).floor() as u32).max(1);
    (fit(w), fit(h))
}

/// An encoded image sent to the model.
///
/// Must be within the API size limits (long edge at most
/// [`MAX_IMAGE_LONG_EDGE`] px, area at most [`MAX_IMAGE_PIXELS`] px);
/// downscale to [`fit_for_api`] before encoding. Larger images are resized by
/// the API, costing tokens and detail. Redaction boxes come back as
/// fractions of the image and are mapped through `source_rect`, so they stay
/// correct either way.
#[derive(Clone, PartialEq)]
pub struct ImageData {
    pub media_type: ImageMediaType,
    /// Encoded file bytes (PNG or WebP).
    pub bytes: Vec<u8>,
    /// Pixel size of the encoded image, as sent.
    pub width: u32,
    pub height: u32,
    /// Region of the full screenshot this image shows, in full-screenshot
    /// pixels. Used to map redaction boxes back to screenshot coordinates;
    /// for a downscaled full screenshot this is the whole screenshot.
    pub source_rect: Rect,
}

impl std::fmt::Debug for ImageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageData")
            .field("media_type", &self.media_type)
            .field("bytes", &format_args!("<{} bytes>", self.bytes.len()))
            .field("width", &self.width)
            .field("height", &self.height)
            .field("source_rect", &self.source_rect)
            .finish()
    }
}

/// Input to the per-step pass.
#[derive(Debug, Clone, PartialEq)]
pub struct StepInput {
    pub step_id: Ulid,
    pub kind: StepKind,
    /// Template title the step already has.
    pub template_title: String,
    pub template_description: String,
    pub element: Option<ElementInfo>,
    /// Keystrokes with secure-field input already masked.
    pub keystrokes: Option<String>,
    /// Click crop around the interaction point.
    pub crop: Option<ImageData>,
    /// Downscaled full screenshot, when useful for context.
    pub full: Option<ImageData>,
    /// Template title of the step before this one.
    pub previous_title: Option<String>,
    /// Template title of the step after this one.
    pub next_title: Option<String>,
    /// Current guide title, if any.
    pub guide_title: Option<String>,
    /// BCP 47 output language, e.g. `en`.
    pub language: String,
    pub audience: Audience,
}

/// Result of the per-step pass.
#[derive(Debug, Clone, PartialEq)]
pub struct StepOutput {
    /// Markdown.
    pub title: String,
    /// Markdown; `None` when the title says enough.
    pub description: Option<String>,
    /// Suggestions only; never applied automatically.
    pub redaction_suggestions: Vec<RedactionSuggestion>,
    pub meta: GenerationMeta,
}

/// One step as seen by the guide-level pass.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideStepText {
    pub step_id: Ulid,
    pub kind: StepKind,
    pub title: String,
    pub description: String,
    pub app: Option<String>,
    /// False for user-edited steps; edits proposed for them are dropped.
    pub editable: bool,
}

/// Input to the guide-level pass.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideInput {
    pub current_title: String,
    pub steps: Vec<GuideStepText>,
    pub apps: Vec<String>,
    pub language: String,
    pub audience: Audience,
}

/// Result of the guide-level pass.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideOutput {
    pub title: String,
    /// Markdown.
    pub intro: String,
    /// Markdown.
    pub prerequisites: String,
    pub summary: String,
    pub tags: Vec<String>,
    /// Consistency edits for editable steps only.
    pub step_edits: Vec<StepEdit>,
    pub meta: GenerationMeta,
}

/// A proposed change to one step; `None` fields stay as they are.
#[derive(Debug, Clone, PartialEq)]
pub struct StepEdit {
    pub step_id: Ulid,
    pub title: Option<String>,
    pub description: Option<String>,
}

/// How to rewrite a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteStyle {
    Shorter,
    Beginner,
    Expert,
    Custom(String),
}

/// Input to [`AiProvider::rewrite`].
#[derive(Debug, Clone, PartialEq)]
pub struct RewriteInput {
    pub step_id: Ulid,
    pub kind: StepKind,
    pub title: String,
    pub description: String,
    pub element: Option<ElementInfo>,
    pub style: RewriteStyle,
    pub language: String,
}

/// Result of [`AiProvider::rewrite`].
#[derive(Debug, Clone, PartialEq)]
pub struct RewriteOutput {
    pub title: String,
    pub description: Option<String>,
    pub meta: GenerationMeta,
}

/// Provenance of generated text, recorded in [`AiState`](crate::model::AiState).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationMeta {
    /// Model that produced the text.
    pub model: String,
    /// Prompt file version, e.g. `step.v1`.
    pub prompt_version: String,
    pub usage: Usage,
}

/// Token counts for one call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

impl std::ops::AddAssign for Usage {
    fn add_assign(&mut self, other: Self) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AiError {
    #[error("no API key is set")]
    MissingApiKey,
    /// The key has characters an HTTP header cannot carry.
    #[error("the API key is not valid")]
    InvalidApiKey,
    #[error("keychain error: {0}")]
    Keychain(String),
    /// Connection failure or timeout before a response arrived.
    #[error("network error: {0}")]
    Network(String),
    /// Non-success HTTP status from the API.
    #[error("API error {status} ({kind}): {message}")]
    Api {
        status: u16,
        /// Error `type` from the body, e.g. `rate_limit_error`.
        kind: String,
        message: String,
        /// Server-requested wait before retrying.
        retry_after: Option<std::time::Duration>,
    },
    /// The model declined the request.
    #[error("the model declined the request{}", .category.as_deref().map(|c| format!(" ({c})")).unwrap_or_default())]
    Refusal { category: Option<String> },
    /// The response hit `max_tokens` before the structured output was done.
    #[error("the response was cut off at the token limit")]
    Truncated,
    /// The model replied without calling the expected tool. Retried once
    /// by the Claude provider.
    #[error("the model did not call {tool}")]
    MissingToolCall { tool: String },
    /// The response did not have the expected shape.
    #[error("invalid model output: {0}")]
    InvalidOutput(String),
}

impl AiError {
    /// Whether retrying the same request may succeed: rate limits,
    /// overloads, server errors and network failures.
    pub fn is_retryable(&self) -> bool {
        match self {
            AiError::Network(_) => true,
            AiError::Api { status, kind, .. } => {
                matches!(status, 408 | 409 | 429) || *status >= 500 || kind == "overloaded_error"
            }
            _ => false,
        }
    }

    pub fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            AiError::Api { retry_after, .. } => *retry_after,
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn api(status: u16, kind: &str) -> AiError {
        AiError::Api {
            status,
            kind: kind.into(),
            message: String::new(),
            retry_after: None,
        }
    }

    #[test]
    fn retryable_errors() {
        assert!(api(429, "rate_limit_error").is_retryable());
        assert!(api(500, "api_error").is_retryable());
        assert!(api(529, "overloaded_error").is_retryable());
        assert!(AiError::Network("reset".into()).is_retryable());
        assert!(!api(400, "invalid_request_error").is_retryable());
        assert!(!api(401, "authentication_error").is_retryable());
        assert!(!AiError::Refusal { category: None }.is_retryable());
        assert!(!AiError::MissingApiKey.is_retryable());
        assert!(!AiError::MissingToolCall { tool: "t".into() }.is_retryable());
    }

    #[test]
    fn error_messages() {
        let refusal = AiError::Refusal {
            category: Some("cyber".into()),
        };
        assert_eq!(
            refusal.to_string(),
            "the model declined the request (cyber)"
        );
        let err = AiError::Api {
            status: 429,
            kind: "rate_limit_error".into(),
            message: "slow down".into(),
            retry_after: Some(Duration::from_secs(3)),
        };
        assert_eq!(err.retry_after(), Some(Duration::from_secs(3)));
        assert_eq!(
            err.to_string(),
            "API error 429 (rate_limit_error): slow down"
        );
    }

    #[test]
    fn fits_images_to_api_limit() {
        assert!(fits_api_limit(1568, 700));
        assert!(!fits_api_limit(1569, 10));
        assert!(!fits_api_limit(1440, 900));
        assert_eq!(fit_for_api(800, 600), (800, 600));
        assert_eq!(fit_for_api(0, 0), (0, 0));
        // Area-bound: 2880x1800 -> about 1.15 MP at 16:10.
        let (w, h) = fit_for_api(2880, 1800);
        assert!(fits_api_limit(w, h));
        assert_eq!((w, h), (1356, 847));
        // Edge-bound: a long strip.
        assert_eq!(fit_for_api(4000, 200), (1568, 78));
        assert_eq!(fit_for_api(100_000, 1), (1568, 1));
        for (w, h) in [(5120, 2880), (3024, 1964), (1569, 1569), (1200, 1200)] {
            let (fw, fh) = fit_for_api(w, h);
            assert!(fits_api_limit(fw, fh), "{w}x{h} -> {fw}x{fh}");
            assert!(fw >= 1 && fh >= 1);
        }
    }

    #[test]
    fn image_debug_hides_bytes() {
        let image = ImageData {
            media_type: ImageMediaType::Png,
            bytes: vec![1, 2, 3],
            width: 1,
            height: 1,
            source_rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
        };
        assert!(format!("{image:?}").contains("<3 bytes>"));
    }
}
