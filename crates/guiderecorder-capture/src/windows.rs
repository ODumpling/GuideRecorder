//! Windows capture backend: Windows.Graphics.Capture frames, low-level
//! input hooks, UI Automation lookups, foreground tracking, Windows OCR.
//!
//! # Coordinates
//!
//! Windows has no single logical coordinate space across displays with
//! different scale factors, so "global display points" here are physical
//! pixels of the virtual screen (the origin at the top left of the primary
//! display), as a per-monitor DPI aware process sees them. Every display
//! then has a scale of 1. The backend makes each thread that reads or
//! compares positions per-monitor aware ([`PerMonitorAware`]), so this
//! holds in a host that is not (a command line tool) as well as in the
//! app.

pub mod apps;
pub mod ax;
pub mod event_tap;
pub mod frames;
pub(crate) mod ocr;

/// The recording session: the shared engine.
pub mod session {
    pub use crate::recorder::*;
}

use std::sync::OnceLock;

use windows::Win32::System::Com::CoIncrementMTAUsage;
use windows::Win32::UI::HiDpi::{
    SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Keeps a multithreaded apartment for the whole process, once, so WinRT
/// works on any thread: one with no apartment joins it implicitly, and one
/// already in an apartment (say the UI thread) keeps its own.
pub(crate) fn ensure_com() {
    static MTA: OnceLock<()> = OnceLock::new();
    MTA.get_or_init(|| {
        // SAFETY: no preconditions. The cookie is never released, so the
        // apartment lasts until the process exits. On failure WinRT calls
        // fail and say why.
        let _ = unsafe { CoIncrementMTAUsage() };
    });
}

/// Makes the current thread per-monitor DPI aware (v2) until dropped, so
/// positions it reads are physical pixels (see `# Coordinates`).
pub(crate) struct PerMonitorAware(DPI_AWARENESS_CONTEXT);

impl PerMonitorAware {
    pub(crate) fn enter() -> Self {
        // SAFETY: a predefined context; returns the previous one (null on
        // failure, which `drop` then leaves alone).
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}

impl Drop for PerMonitorAware {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: the context this thread had before `enter`.
            unsafe { SetThreadDpiAwarenessContext(self.0) };
        }
    }
}
