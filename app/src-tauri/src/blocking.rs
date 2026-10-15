//! The one way commands run blocking work (disk, Keychain, dialogs,
//! capture) off the async runtime.

use crate::editor::{AppError, ErrorKind, Result};

/// Runs `f` on the blocking pool; a panic in it is an `io` error.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| AppError::new(ErrorKind::Io, format!("task failed: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_result_and_turns_a_panic_into_io() {
        tauri::async_runtime::block_on(async {
            assert_eq!(blocking(|| Ok(7)).await.unwrap(), 7);
            let err = blocking(|| Err::<(), _>(AppError::new(ErrorKind::Invalid, "no")))
                .await
                .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Invalid);
            let err = blocking(|| -> Result<()> { panic!("boom") })
                .await
                .unwrap_err();
            assert_eq!(err.kind, ErrorKind::Io);
            assert!(err.message.starts_with("task failed"), "{}", err.message);
        });
    }
}
