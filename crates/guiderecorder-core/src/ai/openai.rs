//! [`AiProvider`] over an OpenAI-compatible Chat Completions API
//! (`POST {base_url}/chat/completions`): OpenAI itself, the many hosted and
//! self-hosted services that copy its API, and local servers such as Ollama
//! and LM Studio ([`OpenAiConfig::ollama`], [`OpenAiConfig::lm_studio`]).
//!
//! The system prompts, context blocks, tool schemas and output parsing are
//! the Claude provider's ([`super::claude`]), so every provider gets the
//! same input and is held to the same output. Only the wire format differs:
//! images go as `image_url` data URLs, the tools as `function` tools, and
//! the result comes back as a tool call whose `arguments` is the JSON
//! input. Some local models write that JSON as the reply text instead; it is
//! accepted too. A reply with neither is retried once, as with Claude.
//!
//! The key is optional: local servers take none. Text-only models get no
//! images when the pipeline runs with
//! [`PipelineOptions::images`](super::PipelineOptions) off.

use std::time::Duration;

use serde_json::{json, Value};

use super::claude::{
    context_text, guide_context, guide_tool, parse_guide_output, parse_rewrite_output,
    parse_step_output, post_json, retry_missing_tool_call, rewrite_context, rewrite_tool,
    step_context, step_images, step_tool, Parsed, GUIDE_TOOL, REWRITE_TOOL, STEP_TOOL,
};
use super::keychain::ApiKey;
use super::prompts::{self, Prompt};
use super::retry::RetryPolicy;
use super::{
    base64, fits_api_limit, AiError, AiFuture, AiProvider, GuideInput, GuideOutput, ImageData,
    RewriteInput, RewriteOutput, StepInput, StepOutput, Usage,
};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// Ollama's OpenAI-compatible endpoint on its default port.
pub const OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";
/// LM Studio's local server on its default port.
pub const LM_STUDIO_BASE_URL: &str = "http://localhost:1234/v1";

/// Per-call model configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiModel {
    pub model: String,
    /// `max_tokens`; `None` leaves the limit to the server. Not every
    /// server takes the newer `max_completion_tokens`, so only this is sent.
    pub max_tokens: Option<u32>,
}

impl OpenAiModel {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            max_tokens: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAiConfig {
    /// Up to and including the version segment, e.g.
    /// `https://api.openai.com/v1`; `/chat/completions` is appended.
    pub base_url: String,
    pub step: OpenAiModel,
    pub guide: OpenAiModel,
    pub rewrite: OpenAiModel,
    pub retry: RetryPolicy,
    /// Per-attempt HTTP timeout.
    pub timeout: Duration,
}

impl OpenAiConfig {
    /// `base_url` with `step_model` for steps and rewrites and
    /// `guide_model` for the guide pass.
    pub fn new(base_url: impl Into<String>, step_model: &str, guide_model: &str) -> Self {
        Self {
            base_url: base_url.into(),
            step: OpenAiModel::new(step_model),
            guide: OpenAiModel::new(guide_model),
            rewrite: OpenAiModel::new(step_model),
            retry: RetryPolicy::default(),
            timeout: Duration::from_secs(120),
        }
    }

    /// A local server: a longer timeout, since local models are slower and
    /// may have to load first.
    fn local(base_url: &str, step_model: &str, guide_model: &str) -> Self {
        Self {
            timeout: Duration::from_secs(300),
            ..Self::new(base_url, step_model, guide_model)
        }
    }

    /// Ollama on this Mac ([`OLLAMA_BASE_URL`]).
    pub fn ollama(step_model: &str, guide_model: &str) -> Self {
        Self::local(OLLAMA_BASE_URL, step_model, guide_model)
    }

    /// LM Studio on this Mac ([`LM_STUDIO_BASE_URL`]).
    pub fn lm_studio(step_model: &str, guide_model: &str) -> Self {
        Self::local(LM_STUDIO_BASE_URL, step_model, guide_model)
    }

    pub fn url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

pub struct OpenAiProvider {
    http: reqwest::Client,
    key: Option<ApiKey>,
    config: OpenAiConfig,
}

impl OpenAiProvider {
    /// `key` is sent as a bearer token; `None` for servers without one.
    pub fn new(key: Option<ApiKey>, config: OpenAiConfig) -> Result<Self, AiError> {
        let http = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|err| AiError::Network(err.to_string()))?;
        Ok(Self { http, key, config })
    }

    pub fn config(&self) -> &OpenAiConfig {
        &self.config
    }

    /// Send one request with retries; returns the parsed response. A reply
    /// without the expected tool call is retried once.
    async fn send(
        &self,
        settings: &OpenAiModel,
        body: &Value,
        tool: &str,
    ) -> Result<Parsed, AiError> {
        let url = self.config.url();
        let headers = request_headers(self.key.as_ref())?;
        retry_missing_tool_call(|| async {
            let value = post_json(&self.http, &url, &headers, body, self.config.retry).await?;
            let mut parsed = parse_completion(&value, tool)?;
            if parsed.model.is_empty() {
                parsed.model = settings.model.clone();
            }
            Ok(parsed)
        })
        .await
    }
}

impl AiProvider for OpenAiProvider {
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

/// HTTP headers for a request: a bearer token when there is a key, marked
/// sensitive so it is redacted from `Debug` output.
pub(crate) fn request_headers(key: Option<&ApiKey>) -> Result<HeaderMap, AiError> {
    let mut headers = HeaderMap::new();
    if let Some(key) = key {
        let mut value = HeaderValue::from_str(&format!("Bearer {}", key.expose()))
            .map_err(|_| AiError::InvalidApiKey)?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

/// One of the shared tools ([`step_tool`] etc.) as a `function` tool.
fn function_tool(tool: Value) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool["name"],
            "description": tool["description"],
            "parameters": tool["input_schema"],
            "strict": tool["strict"],
        },
    })
}

fn build_request(settings: &OpenAiModel, prompt: Prompt, tool: Value, content: Value) -> Value {
    let mut body = json!({
        "model": settings.model,
        "messages": [
            { "role": "system", "content": prompt.text },
            { "role": "user", "content": content },
        ],
        "tools": [function_tool(tool)],
        "tool_choice": "auto",
    });
    if let Some(max_tokens) = settings.max_tokens {
        body["max_tokens"] = json!(max_tokens);
    }
    body
}

fn image_part(image: &ImageData) -> Value {
    debug_assert!(
        fits_api_limit(image.width, image.height),
        "image {}x{} exceeds the API size limit; downscale with ai::fit_for_api first",
        image.width,
        image.height,
    );
    let url = format!(
        "data:{};base64,{}",
        image.media_type.mime(),
        base64::encode(&image.bytes)
    );
    json!({ "type": "image_url", "image_url": { "url": url } })
}

/// User message content: the images, then the context text. Plain text
/// when there are no images, which every server takes.
fn user_content(images: &[&ImageData], context: &Value) -> Value {
    if images.is_empty() {
        return json!(context_text(context));
    }
    let mut parts: Vec<Value> = images.iter().map(|image| image_part(image)).collect();
    parts.push(json!({ "type": "text", "text": context_text(context) }));
    Value::Array(parts)
}

pub fn step_request(config: &OpenAiConfig, input: &StepInput) -> Value {
    let images: Vec<&ImageData> = step_images(input).into_iter().map(|(_, i)| i).collect();
    let content = user_content(&images, &step_context(input));
    build_request(&config.step, prompts::STEP, step_tool(), content)
}

pub fn guide_request(config: &OpenAiConfig, input: &GuideInput) -> Value {
    let content = user_content(&[], &guide_context(input));
    build_request(&config.guide, prompts::GUIDE, guide_tool(), content)
}

pub fn rewrite_request(config: &OpenAiConfig, input: &RewriteInput) -> Value {
    let content = user_content(&[], &rewrite_context(input));
    build_request(&config.rewrite, prompts::REWRITE, rewrite_tool(), content)
}

// ---------------------------------------------------------------------------
// Responses

/// Extract the expected tool call from a Chat Completions response body:
/// a `tool_calls` entry named `tool`, else a JSON object in the reply text.
pub fn parse_completion(body: &Value, tool: &str) -> Result<Parsed, AiError> {
    let choice = &body["choices"][0];
    if !choice.is_object() {
        return Err(AiError::InvalidOutput("response has no choices".into()));
    }
    let message = &choice["message"];
    if message["refusal"]
        .as_str()
        .is_some_and(|r| !r.trim().is_empty())
    {
        return Err(AiError::Refusal { category: None });
    }
    match choice["finish_reason"].as_str().unwrap_or_default() {
        "length" => return Err(AiError::Truncated),
        "content_filter" => {
            return Err(AiError::Refusal {
                category: Some("content_filter".into()),
            })
        }
        _ => {}
    }
    let call = message["tool_calls"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .find(|call| call["function"]["name"] == tool);
    let tool_input = match call {
        Some(call) => arguments(&call["function"]["arguments"], tool)?,
        None => message["content"]
            .as_str()
            .and_then(json_in_text)
            .ok_or_else(|| AiError::MissingToolCall {
                tool: tool.to_owned(),
            })?,
    };
    if !tool_input.is_object() {
        return Err(AiError::InvalidOutput(format!(
            "{tool} input is not an object"
        )));
    }
    let usage = &body["usage"];
    let count = |value: &Value| value.as_u64().unwrap_or(0);
    Ok(Parsed {
        model: body["model"].as_str().unwrap_or_default().to_owned(),
        tool_input,
        usage: Usage {
            input_tokens: count(&usage["prompt_tokens"]),
            output_tokens: count(&usage["completion_tokens"]),
            cache_read_input_tokens: count(&usage["prompt_tokens_details"]["cached_tokens"]),
            cache_creation_input_tokens: 0,
        },
    })
}

/// Tool call `arguments`: a JSON string per the API; some servers send the
/// object itself.
fn arguments(value: &Value, tool: &str) -> Result<Value, AiError> {
    match value {
        Value::String(text) => serde_json::from_str(text)
            .map_err(|err| AiError::InvalidOutput(format!("{tool} arguments are not JSON: {err}"))),
        other => Ok(other.clone()),
    }
}

/// A JSON object written as reply text: bare, in a code fence, or after
/// a model's reasoning. From the first `{` to the last `}`.
fn json_in_text(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str::<Value>(text.get(start..=end)?)
        .ok()
        .filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::claude::{self, ClaudeConfig};
    use crate::ai::{GuideStepText, ImageMediaType, RedactionSuggestion, RewriteStyle};
    use crate::model::{Audience, ElementInfo, Rect, StepKind};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use ulid::Ulid;

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
            step_id: Ulid::from_parts(1, 1),
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
            full: None,
            previous_title: None,
            next_title: None,
            guide_title: None,
            language: "en".into(),
            audience: Audience::Beginner,
        }
    }

    fn text_step_input() -> StepInput {
        StepInput {
            kind: StepKind::Type,
            template_title: "Type your name".into(),
            keystrokes: Some("Ada".into()),
            crop: None,
            full: None,
            ..step_input()
        }
    }

    fn guide_input() -> GuideInput {
        GuideInput {
            current_title: "Untitled".into(),
            steps: vec![GuideStepText {
                step_id: Ulid::from_parts(2, 2),
                kind: StepKind::Click,
                title: "Click **File**".into(),
                description: String::new(),
                app: Some("TextEdit".into()),
                editable: true,
            }],
            apps: vec!["TextEdit".into()],
            language: "en".into(),
            audience: Audience::Expert,
        }
    }

    fn rewrite_input() -> RewriteInput {
        RewriteInput {
            step_id: Ulid::from_parts(1, 1),
            kind: StepKind::Click,
            title: "Click **Save**".into(),
            description: String::new(),
            element: None,
            style: RewriteStyle::Shorter,
            language: "en".into(),
        }
    }

    fn config() -> OpenAiConfig {
        OpenAiConfig::new("https://llm.example.com/v1/", "small-vl", "big")
    }

    #[test]
    fn presets() {
        let ollama = OpenAiConfig::ollama("qwen3-vl", "qwen3");
        assert_eq!(ollama.url(), "http://localhost:11434/v1/chat/completions");
        assert_eq!(ollama.step.model, "qwen3-vl");
        assert_eq!(ollama.rewrite.model, "qwen3-vl");
        assert_eq!(ollama.guide.model, "qwen3");
        assert_eq!(ollama.timeout, Duration::from_secs(300));
        let lm_studio = OpenAiConfig::lm_studio("a", "b");
        assert_eq!(lm_studio.url(), "http://localhost:1234/v1/chat/completions");
        assert_eq!(
            config().url(),
            "https://llm.example.com/v1/chat/completions"
        );
        assert_eq!(config().timeout, Duration::from_secs(120));
    }

    #[test]
    fn headers() {
        assert!(request_headers(None).unwrap().is_empty());
        let key = ApiKey::new("sk-test").unwrap();
        let headers = request_headers(Some(&key)).unwrap();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[AUTHORIZATION], "Bearer sk-test");
        assert!(headers[AUTHORIZATION].is_sensitive());
        assert!(!format!("{headers:?}").contains("sk-test"));
    }

    #[test]
    fn step_request_json() {
        let input = step_input();
        let body = step_request(&config(), &input);
        let context = claude::step_context(&input);
        let expected = json!({
            "model": "small-vl",
            "messages": [
                { "role": "system", "content": prompts::STEP.text },
                { "role": "user", "content": [
                    { "type": "image_url", "image_url": { "url": "data:image/webp;base64,Zm9v" } },
                    { "type": "text", "text": serde_json::to_string_pretty(&context).unwrap() },
                ]},
            ],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "record_step",
                    "description": step_tool()["description"],
                    "parameters": step_tool()["input_schema"],
                    "strict": true,
                },
            }],
            "tool_choice": "auto",
        });
        assert_eq!(body, expected);

        let mut capped = config();
        capped.step.max_tokens = Some(4000);
        assert_eq!(step_request(&capped, &input)["max_tokens"], 4000);
    }

    /// The prompt and the context the model reads are the Claude
    /// provider's, byte for byte.
    #[test]
    fn prompts_and_context_match_claude() {
        let claude = ClaudeConfig::default();
        let pairs = [
            (
                step_request(&config(), &step_input()),
                claude::step_request(&claude, &step_input()),
            ),
            (
                step_request(&config(), &text_step_input()),
                claude::step_request(&claude, &text_step_input()),
            ),
            (
                guide_request(&config(), &guide_input()),
                claude::guide_request(&claude, &guide_input()),
            ),
            (
                rewrite_request(&config(), &rewrite_input()),
                claude::rewrite_request(&claude, &rewrite_input()),
            ),
        ];
        for (openai, claude) in pairs {
            assert_eq!(openai["messages"][0]["content"], claude["system"]);
            let claude_content = claude["messages"][0]["content"].as_array().unwrap();
            let context = &claude_content.last().unwrap()["text"];
            let user = &openai["messages"][1]["content"];
            let text = user
                .as_array()
                .map_or(user, |parts| &parts.last().unwrap()["text"]);
            assert_eq!(text, context);
            assert_eq!(
                openai["tools"][0]["function"]["parameters"],
                claude["tools"][0]["input_schema"]
            );
        }
    }

    #[test]
    fn text_only_step_sends_plain_text() {
        let body = step_request(&config(), &text_step_input());
        let content = body["messages"][1]["content"].as_str().unwrap();
        let context: Value = serde_json::from_str(content).unwrap();
        assert_eq!(context["kind"], "type");
        assert_eq!(context["keystrokes"], "Ada");
        assert_eq!(context["element"]["label"], "Save");
        assert_eq!(context["images"], json!([]));

        let body = guide_request(&config(), &guide_input());
        assert_eq!(body["model"], "big");
        assert!(body["messages"][1]["content"].is_string());
        assert_eq!(body["tools"][0]["function"]["name"], "record_guide");
        let body = rewrite_request(&config(), &rewrite_input());
        assert_eq!(body["model"], "small-vl");
        assert_eq!(body["tools"][0]["function"]["name"], "record_rewrite");
    }

    fn completion(message: Value, finish_reason: &str) -> Value {
        json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "small-vl",
            "choices": [{ "index": 0, "message": message, "finish_reason": finish_reason }],
            "usage": {
                "prompt_tokens": 900,
                "completion_tokens": 40,
                "total_tokens": 940,
                "prompt_tokens_details": { "cached_tokens": 128 },
            },
        })
    }

    fn tool_call(name: &str, arguments: Value) -> Value {
        completion(
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": name, "arguments": arguments },
                }],
            }),
            "tool_calls",
        )
    }

    fn text_reply(text: &str) -> Value {
        completion(json!({ "role": "assistant", "content": text }), "stop")
    }

    #[test]
    fn parses_tool_calls() {
        let args = json!({
            "title": "Click **Save**",
            "description": "",
            "redactions": [
                { "image": "crop", "x": 0.25, "y": 0.25, "width": 0.25, "height": 0.125, "reason": "email address" },
                // No full image was sent: dropped.
                { "image": "full", "x": 0, "y": 0, "width": 0.5, "height": 0.5, "reason": "x" },
            ],
        });
        let body = tool_call("record_step", json!(args.to_string()));
        let parsed = parse_completion(&body, STEP_TOOL).unwrap();
        assert_eq!(parsed.model, "small-vl");
        assert_eq!(
            parsed.usage,
            Usage {
                input_tokens: 900,
                output_tokens: 40,
                cache_read_input_tokens: 128,
                cache_creation_input_tokens: 0,
            }
        );
        let out = parse_step_output(&step_input(), parsed).unwrap();
        assert_eq!(out.title, "Click **Save**");
        assert_eq!(out.meta.prompt_version, "step.v1");
        assert_eq!(
            out.redaction_suggestions,
            [RedactionSuggestion {
                rect: rect(1100.0, 550.0, 100.0, 25.0),
                reason: "email address".into(),
            }]
        );

        // Arguments as an object, as some servers send them.
        let body = tool_call("record_step", args.clone());
        assert_eq!(parse_completion(&body, STEP_TOOL).unwrap().tool_input, args);
    }

    #[test]
    fn accepts_json_written_as_text() {
        let json = r#"{"title": "Click **Save**", "description": ""}"#;
        for text in [
            json.to_owned(),
            format!("```json\n{json}\n```"),
            format!("<think>The user wants it short.</think>\n{json}"),
        ] {
            let parsed = parse_completion(&text_reply(&text), REWRITE_TOOL).unwrap();
            let out = parse_rewrite_output(parsed).unwrap();
            assert_eq!(out.title, "Click **Save**");
            assert_eq!(out.description, None);
        }
    }

    #[test]
    fn rejects_missing_or_bad_output() {
        let missing = AiError::MissingToolCall {
            tool: "record_step".into(),
        };
        assert_eq!(
            parse_completion(&text_reply("Click Save."), STEP_TOOL),
            Err(missing.clone())
        );
        assert_eq!(
            parse_completion(&tool_call("other", json!("{}")), STEP_TOOL),
            Err(missing)
        );
        assert!(matches!(
            parse_completion(&tool_call("record_step", json!("{not json")), STEP_TOOL),
            Err(AiError::InvalidOutput(_))
        ));
        assert!(matches!(
            parse_completion(&tool_call("record_step", json!("[1]")), STEP_TOOL),
            Err(AiError::InvalidOutput(_))
        ));
        assert_eq!(
            parse_completion(&json!({ "choices": [] }), STEP_TOOL),
            Err(AiError::InvalidOutput("response has no choices".into()))
        );
    }

    #[test]
    fn parses_truncation_and_refusal() {
        let cut = completion(json!({ "role": "assistant", "content": "{\"ti" }), "length");
        assert_eq!(parse_completion(&cut, STEP_TOOL), Err(AiError::Truncated));
        let filtered = completion(
            json!({ "role": "assistant", "content": "" }),
            "content_filter",
        );
        assert_eq!(
            parse_completion(&filtered, STEP_TOOL),
            Err(AiError::Refusal {
                category: Some("content_filter".into())
            })
        );
        let refused = completion(
            json!({ "role": "assistant", "content": null, "refusal": "I can't help with that." }),
            "stop",
        );
        assert_eq!(
            parse_completion(&refused, STEP_TOOL),
            Err(AiError::Refusal { category: None })
        );
    }

    #[test]
    fn parses_server_errors() {
        let openai = claude::parse_error(
            429,
            r#"{"error":{"message":"Rate limit reached","type":"requests","code":"rate_limit_exceeded"}}"#,
            None,
        );
        assert!(openai.is_retryable());
        assert!(openai.to_string().contains("Rate limit reached"));
        // Ollama sends a bare string.
        let ollama = claude::parse_error(404, r#"{"error":"model \"qwen3\" not found"}"#, None);
        assert_eq!(
            ollama,
            AiError::Api {
                status: 404,
                kind: "http_error".into(),
                message: "model \"qwen3\" not found".into(),
                retry_after: None,
            }
        );
        assert!(!ollama.is_retryable());
    }

    /// A request as the test server received it.
    #[derive(Debug, Clone)]
    struct Received {
        request_line: String,
        /// Header lines, names lowercased.
        headers: Vec<String>,
        body: Value,
    }

    impl Received {
        fn header(&self, name: &str) -> Option<&str> {
            let prefix = format!("{name}: ");
            self.headers.iter().find_map(|h| h.strip_prefix(&prefix))
        }
    }

    /// Serves `responses` (status and JSON body) to one connection each, in
    /// order, on a local port; returns the base URL and the requests.
    fn serve(responses: Vec<(u16, Value)>) -> (String, Arc<Mutex<Vec<Received>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for (status, body) in responses {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(':').unwrap();
                    headers.push(format!("{}: {}", name.to_lowercase(), value.trim()));
                }
                let length: usize = headers
                    .iter()
                    .find_map(|h| h.strip_prefix("content-length: "))
                    .map_or(0, |n| n.parse().unwrap());
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                log.lock().unwrap().push(Received {
                    request_line: request_line.trim_end().to_owned(),
                    headers,
                    body: serde_json::from_slice(&bytes).unwrap(),
                });
                let text = body.to_string();
                let mut stream = reader.into_inner();
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                )
                .unwrap();
            }
        });
        (base_url, received)
    }

    fn fast_retries(mut config: OpenAiConfig) -> OpenAiConfig {
        config.retry = RetryPolicy {
            max_retries: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(1),
            max_retry_after: Duration::from_secs(1),
        };
        config.timeout = Duration::from_secs(10);
        config
    }

    #[tokio::test]
    async fn local_server_without_key() {
        let reply = tool_call(
            "record_step",
            json!(r#"{"title":"Type **Ada**","description":"","redactions":[]}"#),
        );
        // An overload first: retried.
        let (base_url, received) =
            serve(vec![(503, json!({ "error": "server busy" })), (200, reply)]);
        let mut config = fast_retries(OpenAiConfig::ollama("llama3.2", "llama3.2"));
        config.base_url = base_url;
        let provider = OpenAiProvider::new(None, config).unwrap();
        let out = provider.generate_step(&text_step_input()).await.unwrap();
        assert_eq!(out.title, "Type **Ada**");
        assert_eq!(out.meta.model, "small-vl");

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 2);
        let request = &received[1];
        assert_eq!(request.request_line, "POST /v1/chat/completions HTTP/1.1");
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.body["model"], "llama3.2");
        assert!(request.body["messages"][1]["content"].is_string());
    }

    #[tokio::test]
    async fn hosted_api_with_key() {
        let mut reply = tool_call(
            "record_guide",
            json!({
                "title": "Save a document",
                "intro": "",
                "prerequisites": "",
                "summary": "Save it.",
                "tags": ["TextEdit"],
                "stepEdits": [],
            }),
        );
        // No model in the reply: the requested one is recorded.
        reply.as_object_mut().unwrap().remove("model");
        // A text reply first: retried once.
        let (base_url, received) = serve(vec![(200, text_reply("Here you go.")), (200, reply)]);
        let mut config = fast_retries(OpenAiConfig::new(base_url, "small-vl", "big"));
        config.retry.max_retries = 0;
        let provider = OpenAiProvider::new(Some(ApiKey::new("sk-test").unwrap()), config).unwrap();
        let out = provider.generate_guide(&guide_input()).await.unwrap();
        assert_eq!(out.title, "Save a document");
        assert_eq!(out.tags, ["textedit"]);
        assert_eq!(out.meta.model, "big");
        assert_eq!(out.meta.prompt_version, "guide.v1");

        let received = received.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0].header("authorization"), Some("Bearer sk-test"));
        assert_eq!(received[0].body["model"], "big");
    }

    #[tokio::test]
    async fn api_errors_are_reported() {
        let (base_url, _) = serve(vec![(
            401,
            json!({ "error": { "message": "Incorrect API key", "type": "invalid_request_error" } }),
        )]);
        let config = fast_retries(OpenAiConfig::new(base_url, "m", "m"));
        let provider = OpenAiProvider::new(Some(ApiKey::new("sk-bad").unwrap()), config).unwrap();
        let err = provider.rewrite(&rewrite_input()).await.unwrap_err();
        assert_eq!(
            err,
            AiError::Api {
                status: 401,
                kind: "invalid_request_error".into(),
                message: "Incorrect API key".into(),
                retry_after: None,
            }
        );
    }
}
