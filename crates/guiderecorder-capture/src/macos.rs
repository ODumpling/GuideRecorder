//! macOS capture backend: ScreenCaptureKit frames, CGEventTap input, AX
//! lookups, NSWorkspace app tracking, Vision text recognition, narration
//! (AVAudioEngine and on-device Speech).

pub mod apps;
pub mod ax;
pub mod event_tap;
pub mod frames;
pub mod narration;
pub mod ocr;
pub mod permissions;
/// The recording session: the shared engine.
pub mod session {
    pub use crate::recorder::*;
}
