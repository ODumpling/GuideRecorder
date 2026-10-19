//! Animated GIF writer over the motion frames (see the [module
//! docs](super)), with the `image` crate's encoder.
//!
//! Frames are [`FPS`] a second, so every delay is a whole number of the
//! format's hundredths, and a held frame is stored once with a longer
//! delay. Each frame gets its own 256-colour palette; the GIF loops
//! forever. The default frame is [`DEFAULT_SIZE`] pixels on its short side,
//! small enough to paste into a chat or a ticket.

use std::io::Write;
use std::path::Path;

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use resvg::tiny_skia::Pixmap;

use super::{io_error, render, MotionError, MotionOptions, Result};
use crate::export::persist;
use crate::export::render::RenderGuide;
use crate::export::report::ExportReport;

/// Frames per second.
pub const FPS: u32 = 10;
/// Default short side of the frame, in pixels.
pub const DEFAULT_SIZE: u32 = 540;
/// Quantizer speed, 1 (best) to 30 (fastest).
const SPEED: i32 = 10;

/// Writes `guide` as an animated GIF to `out_path`.
pub fn export(
    guide: &RenderGuide,
    out_path: impl AsRef<Path>,
    opts: &MotionOptions,
) -> Result<ExportReport> {
    let out = out_path.as_ref();
    persist::check_free(out, opts.overwrite).map_err(|e| io_error(e, out))?;
    let mut report = ExportReport::default();
    let size = opts.aspect.size(opts.size.unwrap_or(DEFAULT_SIZE));
    // Encoded in memory (a GIF is a few MB), so write errors surface.
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut bytes, SPEED);
        encoder
            .set_repeat(Repeat::Infinite)
            .map_err(|e| MotionError::Encode(e.to_string()))?;
        let mut sink = |pixmap: &Pixmap, ticks: u32| -> Result<()> {
            let rgba = RgbaImage::from_raw(pixmap.width(), pixmap.height(), pixmap.data().to_vec())
                .ok_or_else(|| MotionError::Encode("frame buffer size".into()))?;
            let delay = Delay::from_numer_denom_ms(ticks * 1000 / FPS, 1);
            encoder
                .encode_frame(Frame::from_parts(rgba, 0, 0, delay))
                .map_err(|e| MotionError::Encode(e.to_string()))
        };
        render(guide, opts, size, FPS, &mut report, &mut sink)?;
    }
    persist::write_file(out, opts.overwrite, "gif", |file| file.write_all(&bytes))
        .map_err(|e| io_error(e, out))?;
    report.files.push(out.to_path_buf());
    Ok(report)
}
