//! Prints input events from the listen-only event tap as JSON lines for 10 s,
//! each with the `targetPid` of the app it was routed to.
//!
//! `cargo run -p guiderecorder-capture --example event_tap [-- --raw-ts]`
//!
//! Click and type in another app's window: `targetPid` should be that app's
//! pid, not 0 (checks the annotated session tap). Events targeting this
//! process are skipped.
//!
//! `--raw-ts` instead runs a separate minimal tap that prints, per event,
//! the raw `CGEventGetTimestamp` value, which reading `event_timestamp`
//! picked for it (mach ticks or ns), and the ts-to-now latency under each
//! reading, then a summary. Use it to check the hardware timestamp unit.
//!
//! Needs Input Monitoring for keyboard events (and Accessibility in some
//! macOS versions); grant them to the terminal app running this.

#[cfg(target_os = "macos")]
fn main() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use guiderecorder_capture::macos::event_tap::EventTap;
    use guiderecorder_capture::permissions::{self, Permission};

    for p in [Permission::InputMonitoring, Permission::Accessibility] {
        let status = permissions::check(p);
        if !status.is_granted() {
            eprintln!(
                "warning: {p:?} is {status:?}; some events may be missing ({})",
                permissions::settings_url(p)
            );
        }
    }

    if std::env::args().skip(1).any(|a| a == "--raw-ts") {
        raw_ts::run(Duration::from_secs(10));
        return;
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

/// `--raw-ts`: a minimal listen-only tap on the main run loop that reports
/// raw event timestamps. Separate from `EventTap`, which only exposes the
/// normalised `ts`.
#[cfg(target_os = "macos")]
mod raw_ts {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::io::Write;
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    use guiderecorder_capture::macos::event_tap::{event_timestamp, mach_ticks_to_timestamp, now};
    use objc2_core_foundation::{kCFRunLoopDefaultMode, CFMachPort, CFRetained, CFRunLoop};
    use objc2_core_graphics::{
        CGEvent, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy,
        CGEventType,
    };

    const TYPES: [(CGEventType, &str); 6] = [
        (CGEventType::LeftMouseDown, "leftMouseDown"),
        (CGEventType::LeftMouseUp, "leftMouseUp"),
        (CGEventType::RightMouseDown, "rightMouseDown"),
        (CGEventType::KeyDown, "keyDown"),
        (CGEventType::ScrollWheel, "scroll"),
        (CGEventType::FlagsChanged, "flagsChanged"),
    ];

    #[derive(Default)]
    struct Counts {
        ticks: Cell<u32>,
        ns: Cell<u32>,
        same: Cell<u32>,
        max_latency_us: Cell<i64>,
    }

    thread_local! {
        static COUNTS: Counts = Counts::default();
        static PORT: RefCell<Option<CFRetained<CFMachPort>>> = const { RefCell::new(None) };
    }

    pub fn run(duration: Duration) {
        let mask = TYPES.iter().fold(0u64, |m, (t, _)| m | (1 << t.0));
        // SAFETY: the callback matches `CGEventTapCallBack` and ignores
        // `user_info`.
        let port = unsafe {
            CGEvent::tap_create(
                CGEventTapLocation::AnnotatedSessionEventTap,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                mask,
                Some(callback),
                std::ptr::null_mut(),
            )
        };
        let Some(port) = port else {
            eprintln!(
                "error: CGEventTapCreate failed (check Input Monitoring / Accessibility permission)"
            );
            std::process::exit(1);
        };
        let source = CFMachPort::new_run_loop_source(None, Some(&port), 0)
            .expect("run loop source for the tap");
        let run_loop = CFRunLoop::current().expect("current run loop");
        // SAFETY: reading an immutable CF constant string.
        let mode = unsafe { kCFRunLoopDefaultMode };
        run_loop.add_source(Some(&source), mode);
        CGEvent::tap_enable(&port, true);
        PORT.with(|p| *p.borrow_mut() = Some(port.clone()));

        eprintln!(
            "raw timestamps for {} s; click and type in another app...",
            duration.as_secs()
        );
        println!("type raw reading ts latency_us (as_ns_us / as_ticks_us)");
        let deadline = Instant::now() + duration;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            CFRunLoop::run_in_mode(mode, left.as_secs_f64().min(0.25), false);
        }
        CGEvent::tap_enable(&port, false);
        run_loop.remove_source(Some(&source), mode);
        port.invalidate();
        PORT.with(|p| p.borrow_mut().take());

        COUNTS.with(|c| {
            eprintln!(
                "summary: read as mach ticks {}, as ns {}, both readings equal {} \
                 (Intel or 1:1 timebase); max latency {} us",
                c.ticks.get(),
                c.ns.get(),
                c.same.get(),
                c.max_latency_us.get()
            );
        });
    }

    unsafe extern "C-unwind" fn callback(
        _proxy: CGEventTapProxy,
        kind: CGEventType,
        event: NonNull<CGEvent>,
        _user_info: *mut c_void,
    ) -> *mut CGEvent {
        let passthrough = event.as_ptr();
        // SAFETY: `event` is valid for the duration of the callback.
        let event = unsafe { event.as_ref() };
        // No panic may unwind into CoreGraphics.
        let _ = catch_unwind(AssertUnwindSafe(|| handle(kind, event)));
        passthrough
    }

    fn handle(kind: CGEventType, event: &CGEvent) {
        if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput
        {
            PORT.with(|p| {
                if let Some(port) = p.borrow().as_ref() {
                    CGEvent::tap_enable(port, true);
                }
            });
            return;
        }
        let raw = CGEvent::timestamp(Some(event));
        let now_ns = now().0 as i64;
        let as_ticks = mach_ticks_to_timestamp(raw).0;
        let picked = event_timestamp(raw).0;
        let reading = if as_ticks == raw {
            "same"
        } else if picked == as_ticks {
            "ticks"
        } else {
            "ns"
        };
        let latency_us = |ts: u64| (now_ns - ts as i64) / 1000;
        let name = TYPES
            .iter()
            .find(|(t, _)| *t == kind)
            .map_or("other", |(_, n)| n);
        // Ignore write errors (e.g. a closed pipe with `| head`).
        let _ = writeln!(
            std::io::stdout().lock(),
            "{name} raw={raw} reading={reading} ts={picked} latency_us={} \
             (as_ns_us={} / as_ticks_us={})",
            latency_us(picked),
            latency_us(raw),
            latency_us(as_ticks)
        );
        COUNTS.with(|c| {
            let counter = match reading {
                "ticks" => &c.ticks,
                "ns" => &c.ns,
                _ => &c.same,
            };
            counter.set(counter.get() + 1);
            c.max_latency_us
                .set(c.max_latency_us.get().max(latency_us(picked)));
        });
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("the event tap example only runs on macOS");
}
