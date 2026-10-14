//! Prints the AX element under the mouse and the focused element once a
//! second for 10 seconds. Needs Accessibility permission for the terminal.
//!
//! `cargo run -p guidesya-capture --example ax`

#[cfg(target_os = "macos")]
fn main() {
    use std::time::{Duration, Instant};

    use guidesya_capture::macos::ax;
    use guidesya_capture::{permissions, Permission};
    use guidesya_core::model::Point;
    use objc2_core_graphics::CGEvent;

    let status = permissions::check(Permission::Accessibility);
    if !status.is_granted() {
        eprintln!("Accessibility is {status:?}; grant it to this terminal first.");
    }

    // AX lookups must not run on the main thread (see ax.rs `# Threading`).
    let worker = std::thread::spawn(|| {
        for tick in 1..=10 {
            let loc = CGEvent::location(CGEvent::new(None).as_deref());
            let point = Point { x: loc.x, y: loc.y };

            let started = Instant::now();
            let under = ax::element_at(point);
            let focused = ax::focused_element();
            let took = started.elapsed();

            println!(
                "[{tick:2}] mouse ({:.0}, {:.0}) lookups took {took:?}",
                point.x, point.y
            );
            println!("     under:   {under:?}");
            println!(
                "     focused: {focused:?} (mask keys: {})",
                ax::must_mask_keys(&focused)
            );
            std::thread::sleep(Duration::from_secs(1).saturating_sub(took));
        }
    });
    worker.join().expect("ax worker panicked");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the ax example only runs on macOS");
}
