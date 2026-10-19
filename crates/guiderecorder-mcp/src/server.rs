//! The MCP handler: tools and resources over a library, read-only.
//!
//! Every read goes through [`Library::find`] / [`Library::read`] and the
//! index's `*_with_fallback` readers, none of which write to the library or
//! the index. Images come only from [`render_step_image`], so annotations,
//! blur and pixelation are burned in; the raw screenshots are never sent.
//!
//! Read-only SQLite opens of the index may create the `-shm` and `-wal`
//! sidecar files next to it (normal WAL behaviour); the index itself is
//! never modified.
//!
//! Failures the caller can act on (unknown id, a guide from a newer app, a
//! bad argument) are tool results with `isError` set and a helpful message;
//! protocol errors are kept for unroutable requests.

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine as _;
use guiderecorder_core::export::{render_step_image, ImageOptions, RenderGuide, RenderStep};
use guiderecorder_core::index::{self, ListFilter};
use guiderecorder_core::library::{Library, LibraryError, LoadedGuide};
use guiderecorder_core::model::{local_today, GuideStatus, ImageShow, Item};
use guiderecorder_core::Ulid;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListResourceTemplatesResult, ListResourcesResult,
    PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::markdown;

pub const SERVER_NAME: &str = "guiderecorder";
/// Resource URI prefix; the guide id follows.
pub const GUIDE_URI_PREFIX: &str = "guiderecorder://guide/";
const MIME_MARKDOWN: &str = "text/markdown";
/// `get_step` images are scaled down (never up) to this width.
pub const IMAGE_MAX_WIDTH: u32 = 1280;
/// `get_guide` images are scaled down to this width.
pub const THUMBNAIL_MAX_WIDTH: u32 = 1024;
/// PNG bytes (before base64) `get_guide` adds at most per response.
pub const IMAGE_BYTE_BUDGET: usize = 5 * 1024 * 1024;
pub const DEFAULT_GUIDE_IMAGES: usize = 10;
pub const MAX_GUIDE_IMAGES: usize = 25;
pub const DEFAULT_LIST_LIMIT: usize = 50;
pub const MAX_LIST_LIMIT: usize = 500;
pub const DEFAULT_SEARCH_LIMIT: usize = 20;
pub const MAX_SEARCH_LIMIT: usize = 100;

const INSTRUCTIONS: &str = "GuideRecorder step-by-step guides recorded on this Mac (read-only). \
Use list_guides or search_steps to find a guide, get_guide for its full Markdown (steps carry \
{#step-<id>} anchors), and get_step for one step with its screenshot. Screenshots are rendered \
with the author's annotations and redactions.";

/// Where to read from.
#[derive(Debug)]
pub struct Config {
    pub library: Library,
    /// The app's search index; `None` always scans the library.
    pub index: Option<PathBuf>,
}

/// The MCP server.
#[derive(Clone)]
pub struct GuideRecorderMcp {
    config: Arc<Config>,
    tool_router: ToolRouter<Self>,
}

/// A failure shown to the caller as a tool error.
#[derive(Debug)]
struct ToolError(String);

impl<E: std::fmt::Display> From<E> for ToolError {
    fn from(e: E) -> Self {
        Self(e.to_string())
    }
}

type ToolResult<T> = Result<T, ToolError>;

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListGuidesArgs {
    /// Words to match in titles, summaries, tags and step text.
    #[serde(default)]
    pub query: Option<String>,
    /// Only guides with this tag (case-insensitive).
    #[serde(default)]
    pub tag: Option<String>,
    /// Maximum number of guides (default 50, at most 500).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetGuideArgs {
    /// Guide id (a ULID, from list_guides or search_steps).
    pub id: String,
    /// Also return step screenshots as images (annotations and redactions
    /// burned in). Default false.
    #[serde(default)]
    pub include_images: bool,
    /// Only these step ids get images, in guide order. Default: every step,
    /// up to max_images.
    #[serde(default)]
    pub image_steps: Option<Vec<String>>,
    /// Maximum number of images (default 10, at most 25).
    #[serde(default)]
    pub max_images: Option<usize>,
}

/// Which screenshot to return.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImageChoice {
    /// The cropped view around the action.
    #[default]
    Crop,
    /// The full screenshot.
    Full,
    /// No image.
    None,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct GetStepArgs {
    /// Guide id.
    pub guide_id: String,
    /// Step id (the `{#step-<id>}` anchor in get_guide, without `step-`).
    pub step_id: String,
    /// "crop" (default), "full" or "none".
    #[serde(default)]
    pub image: ImageChoice,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SearchStepsArgs {
    /// Words to search for; the last word matches as a prefix.
    pub query: String,
    /// Maximum number of hits (default 20, at most 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

impl GuideRecorderMcp {
    pub fn new(config: Config) -> Self {
        Self {
            config: Arc::new(config),
            tool_router: Self::tool_router(),
        }
    }

    /// Runs blocking library work off the async runtime. A `ToolError`
    /// becomes an error result; a panic a protocol error.
    async fn blocking<F>(&self, f: F) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&Config) -> ToolResult<CallToolResult> + Send + 'static,
    {
        let config = self.config.clone();
        let result = tokio::task::spawn_blocking(move || f(&config))
            .await
            .map_err(|e| McpError::internal_error(format!("tool task failed: {e}"), None))?;
        Ok(result.unwrap_or_else(|ToolError(message)| {
            CallToolResult::error(vec![ContentBlock::text(message)])
        }))
    }
}

#[tool_router(router = tool_router)]
impl GuideRecorderMcp {
    #[tool(
        description = "List guides in the GuideRecorder library, most recently updated first. Returns \
                       id, title, summary, tags, step_count, updated_at, read_only and folder for \
                       each. folder is the library subfolder holding the guide (\"Work/Onboarding\"; \
                       empty at the top level). Also owner, status (draft, inReview, published or \
                       needsUpdate), version, last_reviewed, review_interval_months and review_due \
                       (YYYY-MM-DD dates; null when unset) and review_overdue. \
                       read_only is true when the GuideRecorder app will not edit the guide as it is: \
                       it was saved by a newer GuideRecorder, or uses an older format the app \
                       upgrades when it next opens the guide. Read-only guides can still be read \
                       with get_guide and get_step."
    )]
    async fn list_guides(
        &self,
        Parameters(args): Parameters<ListGuidesArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking(move |config| list_guides(config, &args))
            .await
    }

    #[tool(
        description = "Get a guide as Markdown: title, intro, prerequisites, sections, notes and \
                       numbered steps with {#step-<id>} anchors. Set include_images to also get \
                       step screenshots (redacted, max 10 by default, about 5 MB in total)."
    )]
    async fn get_guide(
        &self,
        Parameters(args): Parameters<GetGuideArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking(move |config| get_guide(config, &args)).await
    }

    #[tool(
        description = "Get one step of a guide: its text, the UI element and keys involved, and \
                       its screenshot (\"crop\" by default, \"full\" or \"none\") with \
                       annotations and redactions burned in."
    )]
    async fn get_step(
        &self,
        Parameters(args): Parameters<GetStepArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking(move |config| get_step(config, &args)).await
    }

    #[tool(
        description = "Full-text search across all guides' steps, sections, notes and titles. \
                       Returns hits with guide id and title, kind, item id and a snippet with \
                       matches in **bold**."
    )]
    async fn search_steps(
        &self,
        Parameters(args): Parameters<SearchStepsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.blocking(move |config| search_steps(config, &args))
            .await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for GuideRecorderMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let config = self.config.clone();
        let resources = tokio::task::spawn_blocking(move || guide_resources(&config))
            .await
            .map_err(|e| McpError::internal_error(format!("resource task failed: {e}"), None))?
            .map_err(|ToolError(message)| McpError::internal_error(message, None))?;
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, McpError> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new(format!("{GUIDE_URI_PREFIX}{{id}}"), "guide")
                .with_title("GuideRecorder guide")
                .with_description("A guide as Markdown, by guide id")
                .with_mime_type(MIME_MARKDOWN),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let uri = request.uri;
        let Some(id) = uri.strip_prefix(GUIDE_URI_PREFIX) else {
            return Err(McpError::resource_not_found(
                format!("unknown resource {uri:?}; guides are {GUIDE_URI_PREFIX}<id>"),
                None,
            ));
        };
        let id = id.to_owned();
        let config = self.config.clone();
        let text = tokio::task::spawn_blocking(move || {
            let id = parse_id(&id, "guide")
                .map_err(|ToolError(message)| ReadError::NotFound(message))?;
            Ok::<_, ReadError>(guide_markdown(&read_guide(&config, id)?))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("resource task failed: {e}"), None))?
        .map_err(|e| match e {
            // The guide does not exist.
            ReadError::NotFound(message) => McpError::resource_not_found(message, None),
            // It exists but cannot be read (newer schema, corrupt JSON, IO).
            ReadError::Failed(message) => McpError::internal_error(message, None),
        })?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri).with_mime_type(MIME_MARKDOWN)
        ])
        .into())
    }
}

// ---------------------------------------------------------------------------
// Tools, synchronous

fn list_guides(config: &Config, args: &ListGuidesArgs) -> ToolResult<CallToolResult> {
    let limit = args
        .limit
        .unwrap_or(DEFAULT_LIST_LIMIT)
        .clamp(1, MAX_LIST_LIMIT);
    let filter = ListFilter {
        tag: args.tag.clone().filter(|t| !t.trim().is_empty()),
        query: args.query.clone().filter(|q| !q.trim().is_empty()),
        // Duplicates are dropped below, so the limit is applied after.
        limit: None,
    };
    let guides = list(config, &filter)?;
    let total = guides.len();
    let today = local_today();
    let guides: Vec<_> = guides
        .into_iter()
        .take(limit)
        .map(|g| {
            json!({
                "id": g.id.to_string(),
                "title": g.title,
                "summary": g.summary,
                "tags": g.tags,
                "step_count": g.step_count,
                "updated_at": g.updated_at.to_rfc3339(),
                "read_only": g.read_only,
                "folder": g.folder,
                "owner": g.review.owner,
                "status": g.review.status.map(GuideStatus::as_str),
                "version": g.review.version,
                "last_reviewed": g.review.last_reviewed.map(|d| d.to_string()),
                "review_interval_months": g.review.review_interval_months,
                "review_due": g.review_due().map(|d| d.to_string()),
                "review_overdue": g.is_review_overdue(today),
            })
        })
        .collect();
    Ok(CallToolResult::structured(json!({
        "guides": guides,
        "total": total,
    })))
}

fn get_guide(config: &Config, args: &GetGuideArgs) -> ToolResult<CallToolResult> {
    let id = parse_id(&args.id, "guide")?;
    let loaded = read_guide(config, id)?;
    let mut content = vec![ContentBlock::text(guide_markdown(&loaded))];
    if !args.include_images {
        return Ok(CallToolResult::success(content));
    }

    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let max = args
        .max_images
        .unwrap_or(DEFAULT_GUIDE_IMAGES)
        .min(MAX_GUIDE_IMAGES);
    let wanted: Vec<&RenderStep> = match &args.image_steps {
        Some(ids) => {
            let ids = ids
                .iter()
                .map(|id| parse_id(id.strip_prefix("step-").unwrap_or(id), "step"))
                .collect::<ToolResult<Vec<_>>>()?;
            if let Some(missing) = ids
                .iter()
                .find(|id| !render.steps().any(|s| s.step.id == **id))
            {
                return Err(ToolError(format!(
                    "guide {id} has no step {missing}; step ids are the {{#step-<id>}} anchors"
                )));
            }
            render
                .steps()
                .filter(|s| ids.contains(&s.step.id))
                .collect()
        }
        None => render.steps().collect(),
    };
    let options = thumbnail_options();
    let mut shown = 0;
    let mut bytes = 0;
    let mut over_budget = false;
    for step in wanted.iter().take(max) {
        let image = render.step_image(step, &options);
        if bytes + image.png.len() > IMAGE_BYTE_BUDGET {
            over_budget = true;
            break;
        }
        bytes += image.png.len();
        shown += 1;
        content.push(ContentBlock::text(format!(
            "Image of step {} {{#step-{}}} ({}×{}){}",
            step.number,
            step.step.id,
            image.width,
            image.height,
            warnings_suffix(&image.warnings)
        )));
        content.push(png_block(&image.png));
    }
    if shown < wanted.len() {
        let why = if over_budget {
            format!(
                " (the {} MB response limit was reached)",
                IMAGE_BYTE_BUDGET / (1024 * 1024)
            )
        } else {
            String::new()
        };
        content.push(ContentBlock::text(format!(
            "Showing {shown} of {} step images{why}. Pass image_steps to choose others, or call \
             get_step for one step.",
            wanted.len()
        )));
    }
    Ok(CallToolResult::success(content))
}

fn get_step(config: &Config, args: &GetStepArgs) -> ToolResult<CallToolResult> {
    let guide_id = parse_id(&args.guide_id, "guide")?;
    let step_id = parse_id(
        args.step_id.strip_prefix("step-").unwrap_or(&args.step_id),
        "step",
    )?;
    let loaded = read_guide(config, guide_id)?;
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    let Some(step) = render.steps().find(|s| s.step.id == step_id) else {
        let other = loaded.guide.items.iter().find(|item| item.id() == step_id);
        return Err(ToolError(match other {
            Some(Item::Section(_)) => {
                format!("{step_id} is a section of guide {guide_id}, not a step")
            }
            Some(Item::Note(_)) => format!("{step_id} is a note of guide {guide_id}, not a step"),
            _ => format!(
                "guide {guide_id} has no step {step_id}; use get_guide to see its step ids \
                 (the {{#step-<id>}} anchors)"
            ),
        }));
    };
    let steps = render.steps().count();
    let mut text = format!(
        "Guide: {} {{#guide-{}}}, step {} of {steps}\n\n{}",
        markdown::escape_line(&render.title),
        render.id,
        step.number,
        markdown::step(step, render.kind).trim_end()
    );
    let show = match args.image {
        ImageChoice::Crop => Some(ImageShow::Crop),
        ImageChoice::Full => Some(ImageShow::Full),
        ImageChoice::None => None,
    };
    let Some(show) = show else {
        return Ok(CallToolResult::success(vec![ContentBlock::text(text)]));
    };
    let image = render_step_image(&loaded.handle, &step.step, &image_options(Some(show)));
    text.push_str(&format!(
        "\n\nImage: {}×{} PNG{}",
        image.width,
        image.height,
        warnings_suffix(&image.warnings)
    ));
    Ok(CallToolResult::success(vec![
        ContentBlock::text(text),
        png_block(&image.png),
    ]))
}

fn search_steps(config: &Config, args: &SearchStepsArgs) -> ToolResult<CallToolResult> {
    if args.query.trim().is_empty() {
        return Err(ToolError(
            "query is empty; pass some words to search for".into(),
        ));
    }
    let limit = args
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    let hits = match &config.index {
        Some(path) => index::search_with_fallback(path, &config.library, &args.query, limit)?.0,
        None => index::search_library(&config.library, &args.query, limit)?,
    };
    let hits: Vec<_> = hits
        .into_iter()
        .map(|hit| {
            json!({
                "guide_id": hit.guide_id.to_string(),
                "guide_title": hit.guide_title,
                "kind": hit.kind.as_str(),
                "item_id": hit.item_id.map(|id| id.to_string()),
                "step_number": hit.step_index.map(|i| i + 1),
                "title": hit.title,
                "snippet": hit.snippet,
            })
        })
        .collect();
    Ok(CallToolResult::structured(json!({ "hits": hits })))
}

fn guide_resources(config: &Config) -> ToolResult<Vec<Resource>> {
    Ok(list(config, &ListFilter::default())?
        .into_iter()
        .map(|g| {
            let mut resource =
                Resource::new(format!("{GUIDE_URI_PREFIX}{}", g.id), g.title.clone())
                    .with_title(g.title)
                    .with_mime_type(MIME_MARKDOWN);
            if !g.summary.is_empty() {
                resource = resource.with_description(g.summary);
            }
            resource
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Helpers

/// Canonical guides only (duplicate copies are hidden), index first.
fn list(config: &Config, filter: &ListFilter) -> ToolResult<Vec<index::IndexedGuide>> {
    let guides = match &config.index {
        Some(path) => index::list_with_fallback(path, &config.library, filter)?.0,
        None => index::list_library(&config.library, filter)?,
    };
    Ok(guides
        .into_iter()
        .filter(|g| g.duplicate_of.is_none())
        .collect())
}

fn parse_id(id: &str, what: &str) -> ToolResult<Ulid> {
    Ulid::from_string(id.trim()).map_err(|_| {
        ToolError(format!(
            "{id:?} is not a valid {what} id (expected a 26-character ULID such as \
             01J9ZQ3K8V4N6T2W5X7Y8Z9A0B)"
        ))
    })
}

/// Why [`read_guide`] failed; resources map the two cases to different
/// protocol errors, tools show either message.
#[derive(Debug)]
enum ReadError {
    /// No such guide (or not a valid id).
    NotFound(String),
    /// The guide exists but cannot be read.
    Failed(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

/// Reads the canonical bundle of guide `id` without writing anything.
fn read_guide(config: &Config, id: Ulid) -> Result<LoadedGuide, ReadError> {
    let handle = config
        .library
        .find(id)
        .map_err(|e| ReadError::Failed(format!("cannot search the library for {id}: {e}")))?
        .ok_or_else(|| {
            ReadError::NotFound(format!(
                "no guide with id {id} in the library; use list_guides or search_steps to find \
                 one"
            ))
        })?;
    config.library.read(&handle.path).map_err(|e| {
        ReadError::Failed(match e {
            LibraryError::NewerSchema {
                found, supported, ..
            } => format!(
                "guide {id} was saved by a newer GuideRecorder (schema {found}; this server reads up \
                 to {supported}). Update GuideRecorder and guiderecorder-mcp to read it."
            ),
            e => format!("guide {id} could not be read: {e}"),
        })
    })
}

fn guide_markdown(loaded: &LoadedGuide) -> String {
    let render = RenderGuide::new(&loaded.guide, &loaded.handle);
    markdown::guide(
        &render,
        &markdown::GuideMeta {
            tags: &loaded.guide.tags,
            updated_at: loaded.guide.updated_at.to_rfc3339(),
            read_only: loaded.read_only && loaded.migrated_from.is_none(),
            review_due: loaded.guide.review_due().map(|due| {
                let overdue = loaded.guide.is_review_overdue(local_today());
                format!("{due}{}", if overdue { " (overdue)" } else { "" })
            }),
        },
    )
}

pub fn image_options(show: Option<ImageShow>) -> ImageOptions {
    ImageOptions {
        show,
        max_width: Some(IMAGE_MAX_WIDTH),
        ..ImageOptions::default()
    }
}

/// Options for `get_guide` images: the step's own view, smaller.
pub fn thumbnail_options() -> ImageOptions {
    ImageOptions {
        max_width: Some(THUMBNAIL_MAX_WIDTH),
        ..image_options(None)
    }
}

fn png_block(png: &[u8]) -> ContentBlock {
    ContentBlock::image(
        base64::engine::general_purpose::STANDARD.encode(png),
        "image/png",
    )
}

fn warnings_suffix(warnings: &[guiderecorder_core::export::RenderWarning]) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let list: Vec<String> = warnings.iter().map(ToString::to_string).collect();
    format!(" (warnings: {})", list.join("; "))
}
