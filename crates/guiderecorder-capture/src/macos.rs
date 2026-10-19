//! macOS capture backend: ScreenCaptureKit frames, CGEventTap input, AX
//! lookups, Vision text recognition.

pub mod ax;
pub mod event_tap;
pub mod frames;
pub mod ocr;
pub mod permissions;
pub mod session;
