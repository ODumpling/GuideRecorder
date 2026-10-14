//! Export pipeline: render model, annotation and blur rendering, format
//! writers. See design scratchpad #443.
//!
//! `Guide` → [`RenderGuide`] (resolved items, numbered steps, parsed
//! Markdown [`ast`]) → per-format writer, which renders step images on
//! demand with [`render_step_image`].

mod annotate;
pub mod ast;
pub mod blur;
pub mod docx;
pub mod html;
pub mod md;
pub mod raster;
pub mod render;
pub mod report;

pub use raster::{render_step_image, ImageOptions, RenderWarning, RenderedImage};
pub use render::{RenderGuide, RenderItem, RenderNote, RenderSection, RenderStep};
pub use report::ExportReport;
