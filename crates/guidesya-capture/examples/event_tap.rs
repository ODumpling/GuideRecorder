//! Prints input events from the listen-only event tap as JSON lines for 10 s,
//! each with the `targetPid` of the app it was routed to.
//!
//! `cargo run -p guidesya-capture --example event_tap`
//!
//! Click and type in another app's window: `targetPid` should be that app's
//! pid, not 0 (checks the annotated session tap). Events targeting this
//! process are skipped.
//!
//! Needs Input Monitoring for keyboard events (and Accessibility in some
//! macOS versions); grant them to the terminal app running this.

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use guidesya_capture::macos::event_tap::EventTap;
    use guidesya_capture::permissions::{self, Permission};

    for p in [Permission::InputMonitoring, Permission::Accessibility] {
        let status = permissions::check(p);
        if !status.is_granted() {
            eprintln!(
                "warning: {p:?} is {status:?}; some events may be missing ({})",
                permissions::settings_url(p)
            );
        }
    }

    let (tx, rx) = mpsc::channel();
    // Started on the main thread, so the keyboard layout for
    // `charactersIgnoringModifiers` loads synchronously.
    let mut tap = match EventTap::start_with_target_pid(tx) {
        Ok(tap) => tap,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("listening for 10 s (own pid {})...", std::process::id());
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ev) => println!("{}", serde_json::to_string(&ev).expect("serialize")),
            Err(mpsc::RecvTimeoutError::Timeout) => break,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    tap.stop();
    eprintln!("stopped");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the event tap example only runs on macOS");
}
