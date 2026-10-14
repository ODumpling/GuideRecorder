//! Captures the main display for 3 s and prints frame stats.
//!
//! Needs Screen Recording permission for the terminal running it.
//!
//! ```sh
//! cargo run -p guidesya-capture --example frames [fps]
//! ```

#[cfg(target_os = "macos")]
fn main() {
    use std::time::{Duration, Instant};

    use guidesya_capture::macos::event_tap;
    use guidesya_capture::macos::frames::{DisplayInfo, FrameStream, FrameStreamConfig};

    let fps = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(guidesya_capture::macos::frames::DEFAULT_FPS);

    if let Some(d) = DisplayInfo::main() {
        let (w, h) = d.pixel_size();
        println!(
            "main display {}: {}x{} pt at {:?}, scale {}, {w}x{h} px",
            d.id,
            d.bounds.width,
            d.bounds.height,
            (d.bounds.x, d.bounds.y),
            d.scale
        );
    }

    let cfg = FrameStreamConfig {
        fps,
        ring_capacity: 40,
        ..Default::default()
    };
    let stream = match FrameStream::start(cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not start capture: {e}");
            std::process::exit(1);
        }
    };
    let reader = stream.reader();
    let started = event_tap::now();
    let t0 = Instant::now();
    std::thread::sleep(Duration::from_secs(3));
    let ended = event_tap::now();
    let elapsed = t0.elapsed();

    // Frames seen so far, capped at the ring capacity. 40 frames is ~3.3 s at
    // 12 fps and ~830 MB resident on a 2880x1800 Retina display (20.7 MB each).
    let mut frames = Vec::new();
    let mut cursor = ended;
    while let Some(f) = reader.frame_before(cursor) {
        cursor = f.ts;
        frames.push(f);
    }
    frames.reverse();

    println!(
        "{} frames in {:.2} s (ScreenCaptureKit skips frames while the screen is static)",
        frames.len(),
        elapsed.as_secs_f64()
    );
    let mut prev = None;
    for f in &frames {
        let since_start = f.ts.0 as i128 - started.0 as i128;
        let gap = prev.map_or(String::new(), |p: u64| {
            format!(", +{:.1} ms", (f.ts.0 - p) as f64 / 1e6)
        });
        println!(
            "  ts {} ({:+.1} ms from start{gap}): {}x{} px, scale {}, {} bytes",
            f.ts.0,
            since_start as f64 / 1e6,
            f.width,
            f.height,
            f.scale(),
            f.pixels.len()
        );
        prev = Some(f.ts.0);
    }
    if let Some(e) = reader.last_error() {
        println!("stream error: {e}");
    }
    if let Err(e) = stream.stop() {
        eprintln!("stop failed: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the frames example only runs on macOS");
}
