//! Records a guide: event tap + frames + AX for a few seconds, then writes a
//! `.guidesya` bundle and prints the steps and the metrics (frame-to-event
//! latency, CPU).
//!
//! ```text
//! cargo run -p guidesya-capture --example record -- \
//!     [--seconds 20] [--library DIR] [--raw DIR] [--title TEXT] \
//!     [--fps 12] [--exclude BUNDLE_ID]...
//! ```
//!
//! - `--library`: library folder; defaults to `target/record/library`.
//! - `--raw`: also write `events.jsonl`, `snapshots.jsonl`, `drafts.json`
//!   and the snapshot frames as WebP (`frames/<source>-<frame ts>.webp`) there.
//! - `--exclude`: bundle id whose events are dropped (repeatable).
//!
//! Snapshot frames are encoded to WebP while recording, so build with
//! `--release`: debug encoding is slow and the encoder falls behind.
//!
//! ⌃⌥S adds a manual step. Needs Screen Recording, Input Monitoring and
//! Accessibility for the terminal running it.

#[cfg(target_os = "macos")]
fn main() {
    use std::path::{Path, PathBuf};
    use std::time::Duration;
    use std::{fs, thread};

    use guidesya_capture::macos::session::{RecordingSession, SessionConfig};
    use guidesya_capture::permissions::{self, Permission};
    use guidesya_core::library::Library;

    let mut seconds = 20u64;
    let mut library: Option<PathBuf> = None;
    let mut raw: Option<PathBuf> = None;
    let mut title = String::from("Recorded guide");
    let mut cfg = SessionConfig::default();
    let mut args = std::env::args().skip(1);
    let usage = "usage: record [--seconds N] [--library DIR] [--raw DIR] [--title TEXT] \
                 [--fps N] [--exclude BUNDLE_ID]...";
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next().unwrap_or_else(|| {
                eprintln!("{arg} needs a value\n{usage}");
                std::process::exit(2);
            })
        };
        match arg.as_str() {
            "--seconds" => seconds = value().parse().expect("--seconds: a number"),
            "--library" => library = Some(value().into()),
            "--raw" => raw = Some(value().into()),
            "--title" => title = value(),
            "--fps" => cfg.frames.fps = value().parse().expect("--fps: a number"),
            "--exclude" => cfg.excluded_bundle_ids.push(value()),
            "-h" | "--help" => {
                println!("{usage}");
                return;
            }
            other => {
                eprintln!("unknown argument {other}\n{usage}");
                std::process::exit(2);
            }
        }
    }
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/record");
    let library = library.unwrap_or_else(|| target.join("library"));

    for p in [
        Permission::ScreenRecording,
        Permission::InputMonitoring,
        Permission::Accessibility,
    ] {
        let status = permissions::check(p);
        if !status.is_granted() {
            eprintln!(
                "warning: {p:?} is {status:?} ({})",
                permissions::settings_url(p)
            );
        }
    }

    let lib = Library::open(&library).expect("open library");
    // On the main thread, so the tap loads the keyboard layout here.
    // Snapshots are encoded into a hidden session folder in the local cache
    // (`session::default_work_dir()`), never in the library.
    let session = match RecordingSession::start(cfg) {
        Ok(session) => session,
        Err(e) => {
            eprintln!("error: could not start recording: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "recording for {seconds} s (own pid {}); ⌃⌥S adds a manual step...",
        std::process::id()
    );
    thread::sleep(Duration::from_secs(seconds));
    let recording = match session.stop() {
        Ok(recording) => recording,
        Err(e) => {
            eprintln!("error: stopping failed: {e}");
            std::process::exit(1);
        }
    };

    let saved = recording.save_guide(&lib, &title).expect("save guide");

    if let Some(raw) = &raw {
        fs::create_dir_all(raw).expect("create raw dir");
        let mut events = String::new();
        for ev in &recording.events {
            events.push_str(&serde_json::to_string(ev).expect("serialize event"));
            events.push('\n');
        }
        fs::write(raw.join("events.jsonl"), events).expect("write events.jsonl");
        let mut snaps = String::new();
        for (event_ts, snap) in &recording.snapshots {
            snaps.push_str(&format!(
                "{{\"eventTs\":{},\"frameTs\":{},\"latencyNs\":{}}}\n",
                event_ts.0,
                snap.frame_ts.0,
                event_ts.0.saturating_sub(snap.frame_ts.0)
            ));
        }
        fs::write(raw.join("snapshots.jsonl"), snaps).expect("write snapshots.jsonl");
        let drafts = serde_json::to_string_pretty(&recording.drafts).expect("serialize drafts");
        fs::write(raw.join("drafts.json"), drafts).expect("write drafts.json");
        let n = recording.write_raw_frames(raw).expect("write frames");
        println!("raw dump: {} ({n} frames)", raw.display());
    }

    let m = &recording.metrics;
    println!();
    println!("guide: {}", saved.handle.path.display());
    println!(
        "steps: {} ({} skipped without a frame)",
        saved.guide.steps().count(),
        saved.skipped
    );
    for (i, step) in saved.guide.steps().enumerate() {
        let crop = if step.image.crop.is_some() {
            "crop"
        } else {
            "full"
        };
        println!("  {:>2}. [{:?}, {crop}] {}", i + 1, step.kind, step.title);
    }
    println!();
    println!(
        "events: {} kept, {} own-pid dropped, {} excluded",
        m.events, m.dropped_own, m.dropped_excluded
    );
    println!(
        "snapshots: {} ({} events found no earlier frame)",
        m.snapshots, m.missing_frames
    );
    match m.latency() {
        Some(l) => println!(
            "frame-to-event latency (event ts - frame ts), ms: n={} min={:.1} median={:.1} \
             p95={:.1} max={:.1} mean={:.1}",
            l.count, l.min_ms, l.median_ms, l.p95_ms, l.max_ms, l.mean_ms
        ),
        None => println!("frame-to-event latency: no samples"),
    }
    let ax_mean = if m.ax_lookups > 0 {
        m.ax_total.as_secs_f64() * 1e3 / m.ax_lookups as f64
    } else {
        0.0
    };
    println!(
        "AX: {} lookups, {} errors, mean {ax_mean:.1} ms, max {:.1} ms",
        m.ax_lookups,
        m.ax_errors,
        m.ax_max.as_secs_f64() * 1e3
    );
    println!(
        "CPU: {:.1}% of one core over {:.1} s (user {:.2} s, system {:.2} s, whole process)",
        m.cpu_percent(),
        m.wall.as_secs_f64(),
        m.cpu_user.as_secs_f64(),
        m.cpu_system.as_secs_f64()
    );
    println!(
        "keys masked because their focus lookup was late: {}",
        m.stale_masked
    );
    let encode_mean = if m.encoded_frames > 0 {
        m.encode_total.as_secs_f64() * 1e3 / m.encoded_frames as f64
    } else {
        0.0
    };
    println!(
        "encoder: {} frames, {} errors, {} snapshots lost, {} dropped (queue full), \
         mean {encode_mean:.0} ms, max {:.0} ms, max backlog {}",
        m.encoded_frames,
        m.encode_errors,
        m.unencoded_snapshots,
        m.snapshots_dropped_queue_full,
        m.encode_max.as_secs_f64() * 1e3,
        m.encoder_max_backlog
    );
    println!(
        "masking: {} keys held by the settle guard, {} scrubbed after the fact; \
         {} focus reads after clicks skipped",
        m.settle_masked, m.retro_scrubbed, m.probes_skipped
    );
    if let Some(e) = &m.frames_error {
        println!("frames stream error: {e}");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the record example only runs on macOS");
}
