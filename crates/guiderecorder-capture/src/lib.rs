//! Platform capture backends. See design scratchpad #440.
//!
//! The types every backend shares live in [`frames`], [`session`] and
//! [`keys`]; [`backend`] is the one compiled for this platform. macOS lives
//! in `macos`; every other platform gets `unsupported`, which builds and
//! refuses to record.

pub mod frames;
pub mod keys;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod ocr;
pub mod permissions;
pub mod session;
#[cfg(not(target_os = "macos"))]
pub mod unsupported;
#[cfg(windows)]
mod windows_ocr;

/// The capture backend compiled for this platform: its `frames` module
/// lists displays and windows, its `session` module records.
#[cfg(target_os = "macos")]
pub use macos as backend;
/// The capture backend compiled for this platform: its `frames` module
/// lists displays and windows, its `session` module records.
#[cfg(not(target_os = "macos"))]
pub use unsupported as backend;

pub use permissions::{Permission, PermissionReport, PermissionStatus};

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
