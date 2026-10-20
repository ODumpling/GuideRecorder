//! The backend of a platform GuideRecorder cannot record on yet: the same
//! API as `macos`, so the app builds, but there are no displays or windows
//! to list and [`session::RecordingSession::start`] always fails with
//! [`session::SessionError::Unsupported`].

pub mod frames {
    //! No displays, no windows, no pointer.

    use guiderecorder_core::model::Point;

    pub use crate::frames::*;

    impl DisplayInfo {
        pub fn main() -> Option<Self> {
            None
        }

        pub fn for_id(_id: u32) -> Option<Self> {
            None
        }

        /// All active displays: none.
        pub fn all() -> Vec<Self> {
            Vec::new()
        }

        pub fn containing(_p: Point) -> Option<Self> {
            None
        }
    }

    /// Always [`FrameError::Unsupported`].
    pub fn list_windows() -> Result<Vec<WindowInfo>, FrameError> {
        Err(FrameError::Unsupported)
    }

    pub fn mouse_location() -> Option<Point> {
        None
    }
}

pub mod session {
    //! A session that never starts: [`RecordingSession`] and [`Recording`]
    //! cannot be built, so their methods are never called.

    use std::convert::Infallible;
    use std::path::Path;

    use guiderecorder_core::library::{GuideHandle, Library, LibraryError};
    use guiderecorder_core::model::Guide;

    pub use crate::session::*;

    use super::frames::FrameError;

    #[derive(Debug, thiserror::Error)]
    pub enum SessionError {
        #[error("frames: {0}")]
        Frames(#[from] FrameError),
        #[error("library: {0}")]
        Library(#[from] LibraryError),
        /// Never returned here; the macOS backend's lost capture.
        #[error("capture stopped: {0}")]
        CaptureLost(String),
        #[error("recording is not supported on this platform yet")]
        Unsupported,
    }

    /// The suggested exclusion list: empty, there is nothing to exclude.
    pub fn default_excluded_bundle_ids() -> Vec<String> {
        Vec::new()
    }

    /// A running recording; never exists here.
    pub struct RecordingSession {
        never: Infallible,
    }

    impl RecordingSession {
        /// Always [`SessionError::Unsupported`].
        pub fn start(_cfg: SessionConfig) -> Result<Self, SessionError> {
            Err(SessionError::Unsupported)
        }

        pub fn session_dir(&self) -> Option<&Path> {
            match self.never {}
        }

        pub fn is_paused(&self) -> bool {
            match self.never {}
        }

        pub fn excluded_app(&self) -> Option<String> {
            match self.never {}
        }

        pub fn is_excluded(&self) -> bool {
            match self.never {}
        }

        pub fn capture_lost(&self) -> Option<String> {
            match self.never {}
        }

        pub fn manual_step(&self) -> bool {
            match self.never {}
        }

        pub fn set_control_hotkeys(&self, _hotkeys: Vec<Hotkey>) {
            match self.never {}
        }

        pub fn pause(&mut self) -> Result<(), SessionError> {
            match self.never {}
        }

        pub fn resume(&mut self) -> Result<(), SessionError> {
            match self.never {}
        }

        pub fn stop(self) -> Result<Recording, SessionError> {
            match self.never {}
        }
    }

    impl Drop for RecordingSession {
        fn drop(&mut self) {
            match self.never {}
        }
    }

    /// What a session recorded; never exists here.
    #[derive(Debug)]
    pub struct Recording {
        /// Changes made to the live feed, applied by
        /// [`Recording::save_guide`].
        pub edits: CaptureEdits,
        never: Infallible,
    }

    /// The result of [`Recording::save_guide`].
    #[derive(Debug, Clone)]
    pub struct SavedGuide {
        pub handle: GuideHandle,
        pub guide: Guide,
        /// Draft steps left out because no snapshot was close enough.
        pub skipped: usize,
    }

    impl Recording {
        pub fn session_dir(&self) -> &Path {
            match self.never {}
        }

        pub fn save_guide(
            &self,
            _library: &Library,
            _title: &str,
        ) -> Result<SavedGuide, SessionError> {
            match self.never {}
        }
    }

    impl Drop for Recording {
        fn drop(&mut self) {
            match self.never {}
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_session_never_starts() {
            assert!(matches!(
                RecordingSession::start(SessionConfig::default()),
                Err(SessionError::Unsupported)
            ));
            assert!(super::super::frames::DisplayInfo::all().is_empty());
            assert!(matches!(
                super::super::frames::list_windows(),
                Err(FrameError::Unsupported)
            ));
        }
    }
}
