//! Platform capture backends. See design scratchpad #440.
//!
//! The types every backend shares live in [`frames`], [`session`] and
//! [`keys`]; [`backend`] is the one compiled for this platform. macOS lives
//! in `macos` and Windows in `windows`; both record through the same
//! session engine (`recorder`, their `session` module) and frame ring
//! ([`ring`]). Every other platform gets `unsupported`, which builds and
//! refuses to record.

#[cfg(any(target_os = "macos", windows))]
mod cpu;
#[cfg(any(target_os = "macos", windows))]
pub mod element;
pub mod frames;
pub mod keys;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod ocr;
pub mod permissions;
#[cfg(any(target_os = "macos", windows))]
mod recorder;
#[cfg(any(target_os = "macos", windows))]
pub mod ring;
pub mod session;
#[cfg(not(any(target_os = "macos", windows)))]
pub mod unsupported;
#[cfg(windows)]
pub mod windows;

/// The capture backend compiled for this platform: its `frames` module
/// lists displays and windows, its `session` module records.
#[cfg(target_os = "macos")]
pub use macos as backend;
/// The capture backend compiled for this platform: its `frames` module
/// lists displays and windows, its `session` module records.
#[cfg(not(any(target_os = "macos", windows)))]
pub use unsupported as backend;
/// The capture backend compiled for this platform: its `frames` module
/// lists displays and windows, its `session` module records.
#[cfg(windows)]
pub use windows as backend;

pub use permissions::{Permission, PermissionReport, PermissionStatus};

/// Name of the backend compiled for this platform.
pub fn backend_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        "unsupported"
    }
}
