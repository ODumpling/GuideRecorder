//! Platform capture backends. See design scratchpad #440.
//!
//! `CaptureBackend` is implemented per platform; macOS lives in `macos`.

#[cfg(target_os = "macos")]
pub mod macos;

/// Name of the backend compiled for this platform.
pub fn backend_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "unsupported"
    }
}
