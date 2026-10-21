//! Listen-only `CGEventTap` producing [`guiderecorder_core::events`] events.
//!
//! The tap runs on a dedicated thread with its own `CFRunLoop`. Events are
//! translated in the tap callback and sent over an `mpsc` channel as
//! [`TimedEvent`]s. `KeyDown::secure` is always `false` here; the recording
//! session fills it in from the AX focused element.
//!
//! # Tap location
//!
//! We tap at `kCGAnnotatedSessionEventTap`. Events there have been annotated
//! with their target application, so `kCGEventTargetUnixProcessID` is set,
//! which is how we skip events aimed at GuideRecorder's own toolbar and windows.
//! (At the HID and plain session taps the event is not yet routed and the
//! target pid is 0.) The session level also sees events posted by other
//! processes at session level (remote desktop tools, accessibility software).
//! Events with a target pid of 0 are kept; [`EventTap::start_with_target_pid`]
//! exposes the pid so this can be checked at runtime (see the example).
//!
//! # Key characters
//!
//! The tap thread must not touch TSM/TIS (`NSEvent.characters*`,
//! `TISCopyCurrent…`): since macOS 14 those assert they run on the main
//! queue and abort the process. `characters` comes from
//! `CGEventKeyboardGetUnicodeString` (a field read). For
//! `characters_ignoring_modifiers` we call `UCKeyTranslate` on the tap thread
//! with a copy of the keyboard layout data that is fetched on the main thread
//! at [`EventTap::start`] and refreshed on
//! `kTISNotifySelectedKeyboardInputSourceChanged`. Both the initial fetch
//! (when `start` is called off the main thread) and the refresh need the main
//! queue to be serviced; until the layout is available the field is None.
//!
//! # Timestamps
//!
//! `CGEventGetTimestamp` is documented as nanoseconds since boot, but on
//! Apple silicon some events carry `mach_absolute_time` ticks instead (24 MHz
//! ticks; identical to ns on Intel). Observed here: synthetic events carry ns.
//! [`event_timestamp`] reads the value both ways and keeps the one nearer the
//! current time, then reports host monotonic ns as described in `events.rs`.

use std::ffi::{c_ulong, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread::{self, JoinHandle};

use dispatch2::DispatchQueue;
use guiderecorder_core::events::{
    CaptureEvent, Key, Modifiers, MouseButton, TimedEvent, Timestamp,
};
use guiderecorder_core::model::Point;
use objc2_core_foundation::{
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFMachPort, CFRetained, CFRunLoop,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventMask, CGEventTapLocation, CGEventTapOptions,
    CGEventTapPlacement, CGEventTapProxy, CGEventType,
};
use serde::Serialize;

/// How long one `CFRunLoopRunInMode` slice lasts before the stop flag is
/// re-checked. Only matters if `stop` races the run loop starting.
const RUN_SLICE_SECS: f64 = 0.25;

/// Event types the tap listens for. Tap-disabled notifications are
/// delivered regardless of the mask.
const EVENT_TYPES: [CGEventType; 12] = [
    CGEventType::LeftMouseDown,
    CGEventType::LeftMouseUp,
    CGEventType::RightMouseDown,
    CGEventType::RightMouseUp,
    CGEventType::OtherMouseDown,
    CGEventType::OtherMouseUp,
    CGEventType::LeftMouseDragged,
    CGEventType::RightMouseDragged,
    CGEventType::OtherMouseDragged,
    CGEventType::ScrollWheel,
    CGEventType::KeyDown,
    CGEventType::FlagsChanged,
];

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `CGEventTapCreate` returned NULL. Usually missing Input Monitoring
    /// or Accessibility permission.
    #[error("CGEventTapCreate failed (check Input Monitoring / Accessibility permission)")]
    TapCreate,
    #[error("could not create a run loop source for the event tap")]
    RunLoopSource,
    #[error("could not spawn the event tap thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("event tap thread exited before it was ready")]
    ThreadDied,
}

/// A [`TimedEvent`] with the pid of the application it was routed to
/// (0 if unknown). For diagnostics; the session uses plain [`TimedEvent`]s.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetedEvent {
    pub target_pid: i32,
    #[serde(flatten)]
    pub event: TimedEvent,
}

/// A running event tap. Stops on [`EventTap::stop`] or drop.
pub struct EventTap {
    stop: Arc<AtomicBool>,
    run_loop: Option<RunLoopHandle>,
    thread: Option<JoinHandle<()>>,
}

impl EventTap {
    /// Creates the tap on a new thread and starts sending events to `sender`.
    ///
    /// Returns once the tap is installed and enabled. Events whose target is
    /// this process are skipped. If the receiver is dropped, events are
    /// discarded until the tap is stopped.
    pub fn start(sender: Sender<TimedEvent>) -> Result<EventTap, Error> {
        Self::start_with_sink(Box::new(move |event, _| {
            let _ = sender.send(event);
        }))
    }

    /// Like [`EventTap::start`], but also reports each event's target pid.
    pub fn start_with_target_pid(sender: Sender<TargetedEvent>) -> Result<EventTap, Error> {
        Self::start_with_sink(Box::new(move |event, target_pid| {
            let _ = sender.send(TargetedEvent { target_pid, event });
        }))
    }

    fn start_with_sink(sink: Sink) -> Result<EventTap, Error> {
        layout::ensure_tracking();
        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let ctx = Box::new(TapContext {
            sink,
            own_pid: i64::from(std::process::id()),
            port: AtomicPtr::new(ptr::null_mut()),
        });
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("guiderecorder-event-tap".into())
            .spawn(move || run_tap(ctx, thread_stop, ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(run_loop)) => Ok(EventTap {
                stop,
                run_loop: Some(run_loop),
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                let _ = thread.join();
                Err(Error::ThreadDied)
            }
        }
    }

    /// Disables the tap, stops its run loop and joins the thread.
    /// Idempotent.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(run_loop) = self.run_loop.take() {
            run_loop.0.stop();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The tap thread's run loop, held so another thread can stop it.
struct RunLoopHandle(CFRetained<CFRunLoop>);

// SAFETY: CFRunLoopStop and CFRelease are thread-safe; the handle is only
// used for those.
unsafe impl Send for RunLoopHandle {}

/// Receives each translated event and its target pid, on the tap thread.
type Sink = Box<dyn Fn(TimedEvent, i32) + Send>;

/// State shared with the C callback through `user_info`. Owned by the tap
/// thread and dropped only after the tap is invalidated.
struct TapContext {
    sink: Sink,
    own_pid: i64,
    /// The tap's mach port, for re-enabling after it is disabled.
    port: AtomicPtr<CFMachPort>,
}

fn run_tap(
    ctx: Box<TapContext>,
    stop: Arc<AtomicBool>,
    ready: mpsc::SyncSender<Result<RunLoopHandle, Error>>,
) {
    let ctx_ptr: *const TapContext = &*ctx;
    // SAFETY: the callback matches `CGEventTapCallBack` and only reads
    // `user_info` as `&TapContext`; `ctx` outlives the tap (it is dropped
    // at the end of this function, after the port is invalidated).
    let port = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::AnnotatedSessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            event_mask(&EVENT_TYPES),
            Some(tap_callback),
            ctx_ptr as *mut c_void,
        )
    };
    let Some(port) = port else {
        let _ = ready.send(Err(Error::TapCreate));
        return;
    };
    ctx.port
        .store(CFRetained::as_ptr(&port).as_ptr(), Ordering::Release);

    let Some(source) = CFMachPort::new_run_loop_source(None, Some(&port), 0) else {
        port.invalidate();
        let _ = ready.send(Err(Error::RunLoopSource));
        return;
    };
    let Some(run_loop) = CFRunLoop::current() else {
        port.invalidate();
        let _ = ready.send(Err(Error::RunLoopSource));
        return;
    };
    // SAFETY: reading immutable CF constant strings.
    let (common, default) = unsafe { (kCFRunLoopCommonModes, kCFRunLoopDefaultMode) };
    run_loop.add_source(Some(&source), common);
    CGEvent::tap_enable(&port, true);

    if ready.send(Ok(RunLoopHandle(run_loop.clone()))).is_ok() {
        while !stop.load(Ordering::SeqCst) {
            CFRunLoop::run_in_mode(default, RUN_SLICE_SECS, false);
        }
    }

    CGEvent::tap_enable(&port, false);
    run_loop.remove_source(Some(&source), common);
    port.invalidate();
    ctx.port.store(ptr::null_mut(), Ordering::Release);
    drop(source);
    drop(port);
    drop(ctx);
}

unsafe extern "C-unwind" fn tap_callback(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    user_info: *mut c_void,
) -> *mut CGEvent {
    // Listen-only: the return value is ignored, but pass the event through.
    let passthrough = event.as_ptr();
    if user_info.is_null() {
        return passthrough;
    }
    // SAFETY: `user_info` is the `TapContext` from `run_tap`, alive for as
    // long as the tap exists. `event` is valid for the callback's duration.
    let ctx = unsafe { &*(user_info as *const TapContext) };
    let event = unsafe { event.as_ref() };
    let _ = catch_unwind(AssertUnwindSafe(|| handle_event(ctx, kind, event)));
    passthrough
}

fn handle_event(ctx: &TapContext, kind: CGEventType, event: &CGEvent) {
    if kind == CGEventType::TapDisabledByTimeout || kind == CGEventType::TapDisabledByUserInput {
        let port = ctx.port.load(Ordering::Acquire);
        // SAFETY: non-null only while the port is alive (cleared before it
        // is released, on this same thread).
        if let Some(port) = unsafe { port.as_ref() } {
            CGEvent::tap_enable(port, true);
        }
        return;
    }
    let event = Some(event);
    let target_pid = CGEvent::integer_value_field(event, CGEventField::EventTargetUnixProcessID);
    if target_pid == ctx.own_pid {
        return;
    }
    let Some(captured) = translate(kind, event) else {
        return;
    };
    let ts = event_timestamp(CGEvent::timestamp(event));
    let event = TimedEvent {
        ts,
        event: captured,
    };
    (ctx.sink)(event, i32::try_from(target_pid).unwrap_or(0));
}

fn translate(kind: CGEventType, event: Option<&CGEvent>) -> Option<CaptureEvent> {
    let modifiers = || modifiers_from_flags(CGEvent::flags(event));
    let point = || {
        let p = CGEvent::location(event);
        Point { x: p.x, y: p.y }
    };
    let click_count = || {
        let n = CGEvent::integer_value_field(event, CGEventField::MouseEventClickState);
        u32::try_from(n).unwrap_or(0)
    };
    let button = button_for(kind);
    let ev = match kind {
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            CaptureEvent::MouseDown {
                point: point(),
                button: button?,
                click_count: click_count(),
                modifiers: modifiers(),
                // Filled from AX by the recording session.
                element: None,
            }
        }
        CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp => {
            CaptureEvent::MouseUp {
                point: point(),
                button: button?,
                click_count: click_count(),
                modifiers: modifiers(),
            }
        }
        CGEventType::LeftMouseDragged
        | CGEventType::RightMouseDragged
        | CGEventType::OtherMouseDragged => CaptureEvent::MouseDragged {
            point: point(),
            button: button?,
        },
        CGEventType::ScrollWheel => CaptureEvent::Scroll {
            point: point(),
            // Point deltas as reported by CG, read as doubles so fractional
            // trackpad deltas survive (sign follows natural scrolling).
            delta_x: CGEvent::double_value_field(
                event,
                CGEventField::ScrollWheelEventPointDeltaAxis2,
            ),
            delta_y: CGEvent::double_value_field(
                event,
                CGEventField::ScrollWheelEventPointDeltaAxis1,
            ),
            // Looked up by the recording session.
            region: None,
        },
        CGEventType::KeyDown => {
            let code = CGEvent::integer_value_field(event, CGEventField::KeyboardEventKeycode);
            let code = u16::try_from(code).unwrap_or(u16::MAX);
            let is_repeat =
                CGEvent::integer_value_field(event, CGEventField::KeyboardEventAutorepeat) != 0;
            let modifiers = key_modifiers(code, CGEvent::flags(event));
            CaptureEvent::KeyDown {
                key: Key {
                    code,
                    characters: event_characters(event),
                    characters_ignoring_modifiers: layout::translate(code, modifiers.shift),
                },
                modifiers,
                is_repeat,
                secure: false,
            }
        }
        CGEventType::FlagsChanged => CaptureEvent::FlagsChanged {
            modifiers: modifiers(),
        },
        _ => return None,
    };
    Some(ev)
}

/// What the key produced with current modifiers, from the event's own
/// unicode string (no TSM access). Non-printing results become None.
fn event_characters(event: Option<&CGEvent>) -> Option<String> {
    let mut buf = [0u16; 16];
    let mut len: c_ulong = 0;
    // SAFETY: `len` and `buf` are valid out-pointers; CG writes at most
    // `buf.len()` code units.
    unsafe {
        CGEvent::keyboard_get_unicode_string(
            event,
            buf.len() as c_ulong,
            &mut len,
            buf.as_mut_ptr(),
        )
    };
    let len = (len as usize).min(buf.len());
    String::from_utf16(&buf[..len])
        .ok()
        .and_then(|s| printable(&s))
}

/// `Some(s)` if `s` contains a printing character. Control characters
/// (Return, Tab, Escape, Delete) and AppKit function-key code points
/// (U+F700–U+F8FF: arrows, F-keys, Home/End) are non-printing.
pub(crate) fn printable(s: &str) -> Option<String> {
    let is_printing = |c: char| !c.is_control() && !('\u{F700}'..='\u{F8FF}').contains(&c);
    if !s.is_empty() && s.chars().all(is_printing) {
        Some(s.to_owned())
    } else {
        None
    }
}

/// Maps CG modifier flags. macOS also sets the Fn flag for arrow,
/// function and navigation keys; [`key_modifiers`] corrects for that.
pub(crate) fn modifiers_from_flags(flags: CGEventFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(CGEventFlags::MaskShift),
        control: flags.contains(CGEventFlags::MaskControl),
        option: flags.contains(CGEventFlags::MaskAlternate),
        command: flags.contains(CGEventFlags::MaskCommand),
        function: flags.contains(CGEventFlags::MaskSecondaryFn),
        caps_lock: flags.contains(CGEventFlags::MaskAlphaShift),
    }
}

/// Modifiers for a key press: as [`modifiers_from_flags`], but with
/// `function` cleared for keys where macOS sets the Fn flag implicitly.
/// (A genuinely held Fn on those keys is indistinguishable and is dropped.)
pub(crate) fn key_modifiers(code: u16, flags: CGEventFlags) -> Modifiers {
    let mut m = modifiers_from_flags(flags);
    if implies_fn_flag(code) {
        m.function = false;
    }
    m
}

/// Virtual key codes (Carbon `kVK_*`) for which macOS sets
/// `kCGEventFlagMaskSecondaryFn` without Fn being held: arrows, navigation
/// keys and F1–F20.
pub(crate) fn implies_fn_flag(code: u16) -> bool {
    matches!(
        code,
        // Arrows: left, right, down, up.
        123..=126
        // Help, Home, Page Up, Forward Delete, End, Page Down.
        | 114 | 115 | 116 | 117 | 119 | 121
        // F1–F12.
        | 122 | 120 | 99 | 118 | 96 | 97 | 98 | 100 | 101 | 109 | 103 | 111
        // F13–F20.
        | 105 | 107 | 113 | 106 | 64 | 79 | 80 | 90
    )
}

/// Button for a mouse event type; None for non-mouse types.
pub(crate) fn button_for(kind: CGEventType) -> Option<MouseButton> {
    match kind {
        CGEventType::LeftMouseDown | CGEventType::LeftMouseUp | CGEventType::LeftMouseDragged => {
            Some(MouseButton::Left)
        }
        CGEventType::RightMouseDown
        | CGEventType::RightMouseUp
        | CGEventType::RightMouseDragged => Some(MouseButton::Right),
        CGEventType::OtherMouseDown
        | CGEventType::OtherMouseUp
        | CGEventType::OtherMouseDragged => Some(MouseButton::Other),
        _ => None,
    }
}

fn event_mask(types: &[CGEventType]) -> CGEventMask {
    types.iter().fold(0, |mask, t| mask | (1 << t.0))
}

/// Converts mach ticks to ns with a `mach_timebase_info` ratio.
pub(crate) fn ticks_to_ns(ticks: u64, numer: u32, denom: u32) -> u64 {
    if numer == denom || denom == 0 {
        return ticks;
    }
    let ns = u128::from(ticks) * u128::from(numer) / u128::from(denom);
    u64::try_from(ns).unwrap_or(u64::MAX)
}

/// Converts a mach-absolute-time value (e.g. a `CGEventTimestamp`) to a
/// host monotonic [`Timestamp`].
pub fn mach_ticks_to_timestamp(ticks: u64) -> Timestamp {
    let (numer, denom) = timebase();
    Timestamp(ticks_to_ns(ticks, numer, denom))
}

/// Converts a `CGEventTimestamp` to host monotonic ns.
///
/// The unit of `CGEventGetTimestamp` is inconsistent: the header says ns,
/// and synthetic events (`CGEventCreate` + post) carry ns on Apple silicon,
/// while hardware events are reported to carry mach ticks there. Both are
/// the same uptime clock, so we read `raw` both ways and keep whichever is
/// closer to now. The two readings differ by ~41x on Apple silicon (24 MHz
/// ticks), so for any recent event the choice is unambiguous; on Intel the
/// readings are identical.
pub fn event_timestamp(raw: u64) -> Timestamp {
    let (numer, denom) = timebase();
    Timestamp(pick_event_ns(raw, now().0, numer, denom))
}

/// `raw` interpreted as mach ticks or as ns, whichever is nearer `now_ns`.
pub(crate) fn pick_event_ns(raw: u64, now_ns: u64, numer: u32, denom: u32) -> u64 {
    let as_ticks = ticks_to_ns(raw, numer, denom);
    if as_ticks.abs_diff(now_ns) <= raw.abs_diff(now_ns) {
        as_ticks
    } else {
        raw
    }
}

/// Current host monotonic time on the same clock as event timestamps.
pub fn now() -> Timestamp {
    // SAFETY: no arguments.
    mach_ticks_to_timestamp(unsafe { mach_absolute_time() })
}

fn timebase() -> (u32, u32) {
    static TIMEBASE: OnceLock<(u32, u32)> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid out-pointer.
        let kr = unsafe { mach_timebase_info(&mut info) };
        if kr == 0 && info.denom != 0 {
            (info.numer, info.denom)
        } else {
            (1, 1)
        }
    })
}

/// Keyboard layout cache for `UCKeyTranslate` on the tap thread.
mod layout {
    use super::*;

    type CFTypeRef = *const c_void;

    /// `shiftKey` (1 << 9) in `UCKeyTranslate`'s `modifierKeyState`, which
    /// takes the Carbon modifier bits shifted right by 8.
    const SHIFT_STATE: u32 = (1 << 9) >> 8;
    const KEY_ACTION_DOWN: u16 = 0;
    /// `1 << kUCKeyTranslateNoDeadKeysBit`: produce the dead key's own
    /// character instead of entering a dead-key state.
    const NO_DEAD_KEYS_MASK: u32 = 1;
    const SUSPENSION_DELIVER_IMMEDIATELY: isize = 4;

    /// A retained `CFData` holding `UCKeyboardLayout` bytes.
    struct Layout {
        data: CFTypeRef,
        kbd_type: u32,
    }

    // SAFETY: CFData is immutable and CFRelease is thread-safe; access is
    // serialized by `LAYOUT`'s mutex.
    unsafe impl Send for Layout {}

    impl Drop for Layout {
        fn drop(&mut self) {
            // SAFETY: `data` was retained when the Layout was built.
            unsafe { CFRelease(self.data) };
        }
    }

    static LAYOUT: Mutex<Option<Layout>> = Mutex::new(None);
    static OBSERVER: Once = Once::new();

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CFDataGetBytePtr(data: CFTypeRef) -> *const u8;
        fn CFNotificationCenterGetDistributedCenter() -> CFTypeRef;
        fn CFNotificationCenterAddObserver(
            center: CFTypeRef,
            observer: *const c_void,
            callback: extern "C" fn(CFTypeRef, *mut c_void, CFTypeRef, *const c_void, CFTypeRef),
            name: CFTypeRef,
            object: *const c_void,
            suspension_behavior: isize,
        );
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        static kTISPropertyUnicodeKeyLayoutData: CFTypeRef;
        static kTISNotifySelectedKeyboardInputSourceChanged: CFTypeRef;
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        fn LMGetKbdType() -> u8;
        fn UCKeyTranslate(
            layout: *const c_void,
            virtual_key_code: u16,
            key_action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            key_translate_options: u32,
            dead_key_state: *mut u32,
            max_string_length: c_ulong,
            actual_string_length: *mut c_ulong,
            unicode_string: *mut u16,
        ) -> i32;
    }

    extern "C" {
        fn pthread_main_np() -> i32;
    }

    /// Loads the current layout and subscribes to input-source changes, on
    /// the main thread (directly if we are on it, else queued on the main
    /// queue without waiting, so a caller blocking main cannot deadlock).
    pub(super) fn ensure_tracking() {
        fn work() {
            refresh();
            OBSERVER.call_once(|| {
                // SAFETY: valid static CF objects; the callback is a plain
                // fn and the observer never needs removing (process-wide).
                unsafe {
                    CFNotificationCenterAddObserver(
                        CFNotificationCenterGetDistributedCenter(),
                        ptr::null(),
                        on_input_source_changed,
                        kTISNotifySelectedKeyboardInputSourceChanged,
                        ptr::null(),
                        SUSPENSION_DELIVER_IMMEDIATELY,
                    )
                };
            });
        }
        // SAFETY: no arguments.
        if unsafe { pthread_main_np() } != 0 {
            work();
        } else {
            DispatchQueue::main().exec_async(work);
        }
    }

    extern "C" fn on_input_source_changed(
        _center: CFTypeRef,
        _observer: *mut c_void,
        _name: CFTypeRef,
        _object: *const c_void,
        _user_info: CFTypeRef,
    ) {
        refresh();
    }

    /// Main thread only: TIS asserts on the main queue.
    fn refresh() {
        // SAFETY: TIS calls on the main thread; Copy-rule sources are
        // released, the Get-rule layout data is retained before use.
        let layout = unsafe {
            let mut data = ptr::null();
            for copy in [
                TISCopyCurrentKeyboardLayoutInputSource,
                TISCopyCurrentASCIICapableKeyboardLayoutInputSource,
            ] {
                let source = copy();
                if source.is_null() {
                    continue;
                }
                let d = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData);
                if !d.is_null() {
                    data = CFRetain(d);
                }
                CFRelease(source);
                if !data.is_null() {
                    break;
                }
            }
            (!data.is_null()).then(|| Layout {
                data,
                kbd_type: u32::from(LMGetKbdType()),
            })
        };
        let old = match LAYOUT.lock() {
            Ok(mut guard) => std::mem::replace(&mut *guard, layout),
            Err(_) => None,
        };
        drop(old);
    }

    /// Characters for `code` with no modifiers except Shift (matching
    /// `NSEvent.charactersIgnoringModifiers`). Safe on any thread.
    pub(super) fn translate(code: u16, shift: bool) -> Option<String> {
        let guard = LAYOUT.lock().ok()?;
        let layout = guard.as_ref()?;
        let mut dead_keys = 0u32;
        let mut buf = [0u16; 8];
        let mut len: c_ulong = 0;
        // SAFETY: `layout.data` is a live CFData with UCKeyboardLayout bytes;
        // out-pointers are valid and `buf.len()` bounds the write.
        let status = unsafe {
            UCKeyTranslate(
                CFDataGetBytePtr(layout.data).cast(),
                code,
                KEY_ACTION_DOWN,
                if shift { SHIFT_STATE } else { 0 },
                layout.kbd_type,
                NO_DEAD_KEYS_MASK,
                &mut dead_keys,
                buf.len() as c_ulong,
                &mut len,
                buf.as_mut_ptr(),
            )
        };
        if status != 0 {
            return None;
        }
        let len = (len as usize).min(buf.len());
        String::from_utf16(&buf[..len])
            .ok()
            .and_then(|s| printable(&s))
    }
}

#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

extern "C" {
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    fn mach_absolute_time() -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn modifier_flags_map() {
        assert_eq!(
            modifiers_from_flags(CGEventFlags::empty()),
            Modifiers::default()
        );
        let m = modifiers_from_flags(CGEventFlags::MaskCommand | CGEventFlags::MaskShift);
        assert!(m.command && m.shift);
        assert!(!m.control && !m.option && !m.function && !m.caps_lock);
        assert!(m.is_shortcut());
        let m = modifiers_from_flags(
            CGEventFlags::MaskControl
                | CGEventFlags::MaskAlternate
                | CGEventFlags::MaskSecondaryFn
                | CGEventFlags::MaskAlphaShift,
        );
        assert!(m.control && m.option && m.function && m.caps_lock);
        assert!(!m.command && !m.shift);
        // Device-dependent low bits are ignored.
        assert_eq!(
            modifiers_from_flags(CGEventFlags(0x100 | 0x8)),
            Modifiers::default()
        );
    }

    #[test]
    fn fn_flag_cleared_for_implied_keys() {
        let flags = CGEventFlags::MaskSecondaryFn | CGEventFlags::MaskShift;
        for code in [123, 126, 115, 119, 116, 121, 117, 122, 111, 90] {
            let m = key_modifiers(code, flags);
            assert!(!m.function, "code {code}");
            assert!(m.shift, "code {code}");
        }
        // Letter "a" (0) and Space (49) keep a real Fn.
        assert!(key_modifiers(0, flags).function);
        assert!(key_modifiers(49, flags).function);
    }

    #[test]
    fn button_map() {
        use CGEventType as T;
        for t in [T::LeftMouseDown, T::LeftMouseUp, T::LeftMouseDragged] {
            assert_eq!(button_for(t), Some(MouseButton::Left));
        }
        for t in [T::RightMouseDown, T::RightMouseUp, T::RightMouseDragged] {
            assert_eq!(button_for(t), Some(MouseButton::Right));
        }
        for t in [T::OtherMouseDown, T::OtherMouseUp, T::OtherMouseDragged] {
            assert_eq!(button_for(t), Some(MouseButton::Other));
        }
        for t in [T::KeyDown, T::ScrollWheel, T::FlagsChanged, T::MouseMoved] {
            assert_eq!(button_for(t), None);
        }
    }

    #[test]
    fn timebase_conversion() {
        // Intel: identity.
        assert_eq!(ticks_to_ns(123_456, 1, 1), 123_456);
        // Apple silicon: 24 MHz ticks, numer/denom = 125/3.
        assert_eq!(ticks_to_ns(24_000_000, 125, 3), 1_000_000_000);
        // No overflow on large inputs.
        assert_eq!(ticks_to_ns(u64::MAX, 125, 3), u64::MAX);
        assert_eq!(ticks_to_ns(u64::MAX / 125, 125, 125), u64::MAX / 125);
        // Defensive: zero denominator passes through.
        assert_eq!(ticks_to_ns(42, 1, 0), 42);
    }

    #[test]
    fn event_timestamp_unit_detection() {
        let now_ns = 3_600_000_000_000; // 1 h uptime
                                        // Event 5 ms ago, reported as 24 MHz ticks.
        let ticks = (now_ns - 5_000_000) * 3 / 125;
        assert_eq!(pick_event_ns(ticks, now_ns, 125, 3), now_ns - 5_000_000);
        // Same event reported as ns.
        assert_eq!(
            pick_event_ns(now_ns - 5_000_000, now_ns, 125, 3),
            now_ns - 5_000_000
        );
        // Intel: identity either way.
        assert_eq!(pick_event_ns(42, now_ns, 1, 1), 42);
    }

    #[test]
    fn now_is_monotonic_ns() {
        let a = now();
        std::thread::sleep(Duration::from_millis(20));
        let b = now();
        let ms = b.millis_since(a);
        assert!((15..1000).contains(&ms), "elapsed {ms} ms");
    }

    #[test]
    fn printable_filters_control_and_function_keys() {
        assert_eq!(printable("a").as_deref(), Some("a"));
        assert_eq!(printable("é").as_deref(), Some("é"));
        assert_eq!(printable(" ").as_deref(), Some(" "));
        assert_eq!(printable(""), None);
        assert_eq!(printable("\r"), None);
        assert_eq!(printable("\t"), None);
        assert_eq!(printable("\u{1b}"), None);
        assert_eq!(printable("\u{7f}"), None);
        assert_eq!(printable("\u{F700}"), None); // up arrow
    }

    #[test]
    fn mask_has_requested_bits() {
        let mask = event_mask(&[CGEventType::LeftMouseDown, CGEventType::KeyDown]);
        assert_eq!(mask, (1 << 1) | (1 << 10));
        let full = event_mask(&EVENT_TYPES);
        assert_eq!(full & (1 << CGEventType::MouseMoved.0), 0);
        assert_ne!(full & (1 << CGEventType::FlagsChanged.0), 0);
    }

    /// Needs Input Monitoring (to receive) and Accessibility (to post).
    /// Posts a no-op flags-changed event and checks it arrives with a
    /// timestamp on the host monotonic ns clock.
    #[test]
    #[ignore = "needs Input Monitoring + Accessibility TCC permissions and a GUI session"]
    fn live_tap_receives_posted_event() {
        let (tx, rx) = mpsc::channel();
        let mut tap = EventTap::start_with_target_pid(tx).expect("tap");
        let before = now();
        let ev = CGEvent::new(None).expect("event");
        CGEvent::set_type(Some(&ev), CGEventType::FlagsChanged);
        CGEvent::set_flags(Some(&ev), CGEventFlags::empty());
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&ev));
        let got = rx.recv_timeout(Duration::from_secs(2)).expect("event");
        tap.stop();
        // The annotated session tap must report where the event was routed.
        eprintln!("target_pid = {}", got.target_pid);
        assert_ne!(got.target_pid, 0, "target pid not annotated");
        let got = got.event;
        assert!(matches!(got.event, CaptureEvent::FlagsChanged { .. }));
        let after = now();
        assert!(
            got.ts.0 + 1_000_000_000 > before.0 && got.ts <= after,
            "{got:?}"
        );
        tap.stop(); // idempotent
    }
}
