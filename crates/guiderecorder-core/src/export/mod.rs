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
pub mod out_path;
pub mod pdf;
pub mod raster;
pub mod render;
pub mod report;
pub mod walkthrough;

pub use md::{Branding, CheckedBranding};
pub use out_path::{check_out_path, OutPathError};
pub use raster::{
    render_step_image, verify_crop_rect, ImageOptions, PlacedHotspot, RenderWarning, RenderedImage,
};
pub use render::{RenderGuide, RenderItem, RenderNote, RenderSection, RenderStep};
pub use report::ExportReport;
