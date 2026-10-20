//! Low-level mouse and keyboard hooks (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`)
//! producing [`guiderecorder_core::events`] events: the Windows counterpart
//! of the macOS event tap, with the same API.
//!
//! The hooks run on a dedicated thread with its own message loop, as
//! low-level hooks must. Each event is translated in the hook callback and
//! sent over an `mpsc` channel; the callback must return quickly or Windows
//! skips it (`LowLevelHooksTimeout`) and may remove the hook. `KeyDown::secure`
//! is always `false` here; the recording session fills it in from UI
//! Automation.
//!
//! # Target
//!
//! A mouse event is aimed at the top-level window under the pointer, a key
//! at the foreground window; their process is the event's target pid, which
//! is how the session skips events aimed at GuideRecorder's own windows. A
//! drag keeps the target of its press.
//!
//! # Keys
//!
//! [`Key::code`] is the virtual-key code (`VK_*`), with the left and right
//! modifier keys told apart as the hook reports them. The keypad's Enter is
//! `VK_RETURN`, like Return.
//!
//! - Repeats: the hook reports auto-repeat as more key-downs; a key-down
//!   for a key already down is a repeat.
//! - AltGr: Windows sends it as a fake left Control (scan code `0x21D`)
//!   plus right Alt. The fake Control is ignored, so AltGr reads as Alt
//!   (option) alone, which the step detector treats as typing when it types
//!   a different character, as with ⌥ on a Mac.
//! - Characters come from `ToUnicodeEx` with the foreground window's
//!   keyboard layout and the modifiers as tracked here, with flag `0x4`
//!   so the call leaves the keyboard state (pending dead keys) alone. A
//!   dead key gives no characters. `characters_ignoring_modifiers` keeps
//!   Shift only, as on macOS.
//! - Click counts are not reported by Windows; a press counts as the next
//!   click of a multi-click when it is the same button, within the
//!   double-click time and the double-click rectangle of the last press.
//!
//! # Timestamps
//!
//! Events are stamped when the callback runs, with [`now`]: the
//! performance counter in nanoseconds, the clock Windows.Graphics.Capture
//! stamps frames with (`SystemRelativeTime`), so frames and events compare
//! directly.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::thread::{self, JoinHandle};

use guiderecorder_core::events::{
    CaptureEvent, Key, Modifiers, MouseButton, TimedEvent, Timestamp,
};
use guiderecorder_core::model::Point;
use serde::Serialize;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, GetKeyboardLayout, ToUnicodeEx, VK_CAPITAL, VK_CONTROL,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
    VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetAncestor, GetForegroundWindow, GetMessageW,
    GetSystemMetrics, GetWindowThreadProcessId, PeekMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HHOOK,
    KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, SM_CXDOUBLECLK, SM_CYDOUBLECLK,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN,
    WM_MBUTTONUP, WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
    WM_RBUTTONUP, WM_SYSKEYDOWN, WM_USER, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

use super::PerMonitorAware;

/// The scan code of the fake left Control that AltGr sends.
const ALT_GR_CONTROL_SCAN: u32 = 0x21D;
/// `ToUnicodeEx` flag: do not change the keyboard state.
const KEEP_KEYBOARD_STATE: u32 = 0x4;
/// Posted to the hook thread once its message queue exists.
const WM_READY: u32 = WM_USER + 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not install the input hooks: {0}")]
    Hook(String),
    #[error("could not spawn the input hook thread: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("input hook thread exited before it was ready")]
    ThreadDied,
}

/// A [`TimedEvent`] with the pid of the application it was aimed at (0 if
/// unknown).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetedEvent {
    pub target_pid: i32,
    #[serde(flatten)]
    pub event: TimedEvent,
}

/// Running input hooks. Stop on [`EventTap::stop`] or drop.
pub struct EventTap {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

/// Receives each translated event and its target pid, on the hook thread.
type Sink = Box<dyn Fn(TimedEvent, i32)>;

impl EventTap {
    /// Installs the hooks on a new thread and starts sending events to
    /// `sender`. Returns once the hooks are installed. If the receiver is
    /// dropped, events are discarded until the hooks are stopped.
    pub fn start(sender: Sender<TimedEvent>) -> Result<EventTap, Error> {
        Self::start_with_sink(move || {
            Box::new(move |event, _| {
                let _ = sender.send(event);
            })
        })
    }

    /// Like [`EventTap::start`], but also reports each event's target pid.
    pub fn start_with_target_pid(sender: Sender<TargetedEvent>) -> Result<EventTap, Error> {
        Self::start_with_sink(move || {
            Box::new(move |event, target_pid| {
                let _ = sender.send(TargetedEvent { target_pid, event });
            })
        })
    }

    fn start_with_sink(
        make_sink: impl FnOnce() -> Sink + Send + 'static,
    ) -> Result<EventTap, Error> {
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("guiderecorder-input-hooks".into())
            .spawn(move || run_hooks(make_sink(), ready_tx))?;
        match ready_rx.recv() {
            Ok(Ok(thread_id)) => Ok(EventTap {
                thread_id,
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

    /// Removes the hooks, ends the hook thread's message loop and joins it.
    /// Idempotent.
    pub fn stop(&mut self) {
        if let Some(thread) = self.thread.take() {
            // SAFETY: posting to a thread that has a message queue (it was
            // created before the thread said it was ready).
            let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            let _ = thread.join();
        }
    }
}

impl Drop for EventTap {
    fn drop(&mut self) {
        self.stop();
    }
}

thread_local! {
    /// The hook thread's state: hook callbacks get no user data, and run on
    /// the thread that installed them.
    static HOOK: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

fn run_hooks(sink: Sink, ready: mpsc::SyncSender<Result<u32, Error>>) {
    let _dpi = PerMonitorAware::enter();
    // SAFETY: no arguments.
    let thread_id = unsafe { GetCurrentThreadId() };
    // Creates this thread's message queue, so a stop posted before the loop
    // starts is not lost.
    let mut msg = MSG::default();
    // SAFETY: a valid out-pointer; peeks without removing.
    let _ = unsafe { PeekMessageW(&mut msg, None, WM_READY, WM_READY, PM_NOREMOVE) };
    HOOK.with(|h| *h.borrow_mut() = Some(HookState::new(sink)));
    // SAFETY: valid callbacks for these hook types, installed for every
    // thread (no module handle needed for low-level hooks).
    let hooks = unsafe {
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0);
        (mouse, keyboard)
    };
    let (mouse, keyboard) = match hooks {
        (Ok(m), Ok(k)) => (m, k),
        (m, k) => {
            let error = [m.as_ref().err(), k.as_ref().err()]
                .into_iter()
                .flatten()
                .map(|e| e.message())
                .collect::<Vec<_>>()
                .join("; ");
            unhook([m.ok(), k.ok()]);
            HOOK.with(|h| h.borrow_mut().take());
            let _ = ready.send(Err(Error::Hook(error)));
            return;
        }
    };
    let _ = ready.send(Ok(thread_id));
    // SAFETY: a valid out-pointer; ends on WM_QUIT (0) or an error (-1).
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        // SAFETY: a message this thread just received.
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    unhook([Some(mouse), Some(keyboard)]);
    HOOK.with(|h| h.borrow_mut().take());
}

fn unhook(hooks: [Option<HHOOK>; 2]) {
    for hook in hooks.into_iter().flatten() {
        // SAFETY: a hook this thread installed.
        let _ = unsafe { UnhookWindowsHookEx(hook) };
    }
}

/// The modifier keys down, by side, and Caps Lock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ModifierKeys {
    left_shift: bool,
    right_shift: bool,
    left_control: bool,
    right_control: bool,
    left_alt: bool,
    right_alt: bool,
    left_win: bool,
    right_win: bool,
    caps_lock: bool,
    /// Right Alt is AltGr (a fake left Control came before it).
    alt_gr: bool,
}

impl ModifierKeys {
    fn modifiers(&self) -> Modifiers {
        Modifiers {
            shift: self.left_shift || self.right_shift,
            control: self.left_control || self.right_control,
            option: self.left_alt || self.right_alt,
            command: self.left_win || self.right_win,
            function: false,
            caps_lock: self.caps_lock,
        }
    }

    /// Notes a modifier key going down or up; false if `vk` is not one.
    fn set(&mut self, vk: u16, down: bool) -> bool {
        let key = match vk {
            v if v == VK_LSHIFT.0 => &mut self.left_shift,
            v if v == VK_RSHIFT.0 => &mut self.right_shift,
            v if v == VK_LCONTROL.0 => &mut self.left_control,
            v if v == VK_RCONTROL.0 => &mut self.right_control,
            v if v == VK_LMENU.0 => &mut self.left_alt,
            v if v == VK_RMENU.0 => &mut self.right_alt,
            v if v == VK_LWIN.0 => &mut self.left_win,
            v if v == VK_RWIN.0 => &mut self.right_win,
            _ => return false,
        };
        *key = down;
        if vk == VK_RMENU.0 && !down {
            self.alt_gr = false;
        }
        true
    }

    /// The keyboard state `ToUnicodeEx` reads: the modifiers as tracked,
    /// AltGr as Control and Alt (which is how layouts define it), and
    /// Shift only when `shift_only`.
    fn keyboard_state(&self, shift_only: bool) -> [u8; 256] {
        let mut state = [0u8; 256];
        let mut set = |vk: u16| state[usize::from(vk)] = 0x80;
        if self.left_shift || self.right_shift {
            set(VK_SHIFT.0);
            set(if self.left_shift {
                VK_LSHIFT.0
            } else {
                VK_RSHIFT.0
            });
        }
        if !shift_only {
            if self.left_control || self.right_control || self.alt_gr {
                set(VK_CONTROL.0);
                set(if self.right_control {
                    VK_RCONTROL.0
                } else {
                    VK_LCONTROL.0
                });
            }
            if self.left_alt || self.right_alt {
                set(VK_MENU.0);
                set(if self.right_alt {
                    VK_RMENU.0
                } else {
                    VK_LMENU.0
                });
            }
        }
        if self.caps_lock {
            state[usize::from(VK_CAPITAL.0)] = 0x01;
        }
        state
    }
}

/// The last press, for counting multi-clicks.
#[derive(Debug, Clone, Copy)]
struct LastPress {
    button: MouseButton,
    at_ms: u32,
    point: POINT,
    count: u32,
}

struct HookState {
    sink: Sink,
    keys: ModifierKeys,
    /// Non-modifier keys down, to tell repeats apart.
    down: HashSet<u16>,
    /// A fake left Control was just seen: the right Alt that follows is
    /// AltGr.
    alt_gr_pending: bool,
    /// Buttons held, with the press's target pid and click count.
    held: Vec<(MouseButton, i32, u32)>,
    last_press: Option<LastPress>,
}

impl HookState {
    fn new(sink: Sink) -> Self {
        // SAFETY: no arguments; the toggle bit of the thread's view of Caps
        // Lock, a starting point the hook keeps up to date.
        let caps_lock = unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) } & 1 != 0;
        Self {
            sink,
            keys: ModifierKeys {
                caps_lock,
                ..ModifierKeys::default()
            },
            down: HashSet::new(),
            alt_gr_pending: false,
            held: Vec::new(),
            last_press: None,
        }
    }

    fn emit(&self, event: CaptureEvent, target_pid: i32) {
        (self.sink)(TimedEvent { ts: now(), event }, target_pid);
    }

    fn mouse(&mut self, msg: u32, info: &MSLLHOOKSTRUCT) {
        let point = Point {
            x: f64::from(info.pt.x),
            y: f64::from(info.pt.y),
        };
        let modifiers = self.keys.modifiers();
        match msg {
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN => {
                let button = button_for(msg);
                let click_count = self.click_count(button, info);
                let pid = window_pid_at(info.pt);
                self.held.retain(|(b, ..)| *b != button);
                self.held.push((button, pid, click_count));
                self.emit(
                    CaptureEvent::MouseDown {
                        point,
                        button,
                        click_count,
                        modifiers,
                        element: None,
                    },
                    pid,
                );
            }
            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP | WM_XBUTTONUP => {
                let button = button_for(msg);
                let held = self.held.iter().position(|(b, ..)| *b == button);
                let (pid, click_count) = match held {
                    Some(i) => {
                        let (_, pid, count) = self.held.remove(i);
                        (pid, count)
                    }
                    None => (window_pid_at(info.pt), 1),
                };
                self.emit(
                    CaptureEvent::MouseUp {
                        point,
                        button,
                        click_count,
                        modifiers,
                    },
                    pid,
                );
            }
            WM_MOUSEMOVE => {
                if let Some(&(button, pid, _)) = self.held.first() {
                    self.emit(CaptureEvent::MouseDragged { point, button }, pid);
                }
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                // Notches of 120, up and right positive.
                let delta = f64::from((info.mouseData >> 16) as u16 as i16) / 120.0;
                let (delta_x, delta_y) = if msg == WM_MOUSEWHEEL {
                    (0.0, delta)
                } else {
                    (delta, 0.0)
                };
                self.emit(
                    CaptureEvent::Scroll {
                        point,
                        delta_x,
                        delta_y,
                    },
                    window_pid_at(info.pt),
                );
            }
            _ => {}
        }
    }

    /// The click count of a press of `button`: one more than the last
    /// press's when it is the same button, within the double-click time and
    /// rectangle; else 1.
    fn click_count(&mut self, button: MouseButton, info: &MSLLHOOKSTRUCT) -> u32 {
        let (time, (width, height)) = double_click_limits();
        let count = match self.last_press {
            Some(last)
                if last.button == button
                    && info.time.wrapping_sub(last.at_ms) <= time
                    && (info.pt.x - last.point.x).abs() <= width / 2
                    && (info.pt.y - last.point.y).abs() <= height / 2 =>
            {
                last.count + 1
            }
            _ => 1,
        };
        self.last_press = Some(LastPress {
            button,
            at_ms: info.time,
            point: info.pt,
            count,
        });
        count
    }

    fn keyboard(&mut self, msg: u32, info: &KBDLLHOOKSTRUCT) {
        let down = matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN);
        let Ok(vk) = u16::try_from(info.vkCode) else {
            return;
        };
        if vk == VK_LCONTROL.0 && info.scanCode == ALT_GR_CONTROL_SCAN {
            // AltGr's fake Control: never a modifier of its own.
            self.alt_gr_pending = down;
            return;
        }
        if vk == VK_RMENU.0 && down && self.alt_gr_pending {
            self.keys.alt_gr = true;
        }
        self.alt_gr_pending = false;
        let before = self.keys;
        if self.keys.set(vk, down) {
            if self.keys.modifiers() != before.modifiers() {
                self.emit(
                    CaptureEvent::FlagsChanged {
                        modifiers: self.keys.modifiers(),
                    },
                    foreground_pid(),
                );
            }
            return;
        }
        if !down {
            self.down.remove(&vk);
            return;
        }
        if vk == VK_CAPITAL.0 && !self.down.contains(&vk) {
            self.keys.caps_lock = !self.keys.caps_lock;
        }
        let is_repeat = !self.down.insert(vk);
        let foreground = foreground_window();
        let layout = keyboard_layout(foreground);
        let key = Key {
            code: vk,
            characters: key_characters(vk, info.scanCode, &self.keys.keyboard_state(false), layout),
            characters_ignoring_modifiers: key_characters(
                vk,
                info.scanCode,
                &self.keys.keyboard_state(true),
                layout,
            ),
        };
        self.emit(
            CaptureEvent::KeyDown {
                key,
                modifiers: self.keys.modifiers(),
                is_repeat,
                secure: false,
            },
            window_pid(foreground),
        );
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for HC_ACTION, `lparam` points to an MSLLHOOKSTRUCT for
        // the duration of the call.
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            HOOK.with(|h| {
                if let Some(state) = h.borrow_mut().as_mut() {
                    state.mouse(msg, info);
                }
            })
        }));
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: for HC_ACTION, `lparam` points to a KBDLLHOOKSTRUCT for
        // the duration of the call.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wparam.0 as u32;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            HOOK.with(|h| {
                if let Some(state) = h.borrow_mut().as_mut() {
                    state.keyboard(msg, info);
                }
            })
        }));
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn button_for(msg: u32) -> MouseButton {
    match msg {
        WM_LBUTTONDOWN | WM_LBUTTONUP => MouseButton::Left,
        WM_RBUTTONDOWN | WM_RBUTTONUP => MouseButton::Right,
        _ => MouseButton::Other,
    }
}

/// The double-click time (ms) and rectangle (pixels).
fn double_click_limits() -> (u32, (i32, i32)) {
    // SAFETY: plain queries.
    unsafe {
        (
            GetDoubleClickTime(),
            (
                GetSystemMetrics(SM_CXDOUBLECLK),
                GetSystemMetrics(SM_CYDOUBLECLK),
            ),
        )
    }
}

/// What a key types with `state`, or `None` for a key that types nothing
/// printable (or a dead key).
fn key_characters(
    vk: u16,
    scan: u32,
    state: &[u8; 256],
    layout: windows::Win32::UI::Input::KeyboardAndMouse::HKL,
) -> Option<String> {
    let mut buf = [0u16; 8];
    // SAFETY: valid buffers; flag 0x4 leaves the keyboard state alone.
    let n = unsafe {
        ToUnicodeEx(
            u32::from(vk),
            scan,
            state,
            &mut buf,
            KEEP_KEYBOARD_STATE,
            Some(layout),
        )
    };
    let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
    let text = String::from_utf16(&buf[..n.min(buf.len())]).ok()?;
    printable(&text)
}

/// `Some(s)` if `s` contains only printing characters.
pub(crate) fn printable(s: &str) -> Option<String> {
    (!s.is_empty() && s.chars().all(|c| !c.is_control())).then(|| s.to_owned())
}

fn foreground_window() -> HWND {
    // SAFETY: no arguments.
    unsafe { GetForegroundWindow() }
}

/// The keyboard layout of `window`'s thread (the active layout is per
/// thread), or this thread's.
fn keyboard_layout(window: HWND) -> windows::Win32::UI::Input::KeyboardAndMouse::HKL {
    // SAFETY: a window handle (possibly null, which gives thread 0, this
    // thread's layout).
    unsafe { GetKeyboardLayout(GetWindowThreadProcessId(window, None)) }
}

/// The process owning `window`, 0 if none.
pub(crate) fn window_pid(window: HWND) -> i32 {
    if window.is_invalid() {
        return 0;
    }
    let mut pid = 0u32;
    // SAFETY: a valid out-pointer.
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    i32::try_from(pid).unwrap_or(0)
}

fn foreground_pid() -> i32 {
    window_pid(foreground_window())
}

/// The process of the top-level window under `p` (physical pixels).
fn window_pid_at(p: POINT) -> i32 {
    // SAFETY: plain queries; the thread is per-monitor aware, so `p` is in
    // the right space.
    let root = unsafe { GetAncestor(WindowFromPoint(p), GA_ROOT) };
    window_pid(root)
}

fn frequency() -> u64 {
    static FREQUENCY: OnceLock<u64> = OnceLock::new();
    *FREQUENCY.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: a valid out-pointer; never fails on Windows XP and later.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        u64::try_from(f).ok().filter(|&f| f > 0).unwrap_or(1)
    })
}

/// Performance counter ticks to nanoseconds.
pub(crate) fn ticks_to_timestamp(ticks: u64) -> Timestamp {
    let f = u128::from(frequency());
    Timestamp((u128::from(ticks) * 1_000_000_000 / f) as u64)
}

/// Current time on the clock events and frames are stamped with:
/// performance counter nanoseconds.
pub fn now() -> Timestamp {
    let mut ticks = 0i64;
    // SAFETY: a valid out-pointer; never fails on Windows XP and later.
    let _ = unsafe { QueryPerformanceCounter(&mut ticks) };
    ticks_to_timestamp(u64::try_from(ticks).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_monotonic_ns() {
        let a = now();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = now();
        assert!(b > a);
        let gap = b.0 - a.0;
        assert!((4_000_000..1_000_000_000).contains(&gap), "{gap}");
    }

    #[test]
    fn modifiers_by_side_and_alt_gr() {
        let mut keys = ModifierKeys::default();
        assert!(keys.set(VK_RSHIFT.0, true));
        assert!(keys.set(VK_LCONTROL.0, true));
        assert!(!keys.set(0x41, true));
        let m = keys.modifiers();
        assert!(m.shift && m.control && !m.option && !m.command);
        let state = keys.keyboard_state(false);
        assert_eq!(state[usize::from(VK_SHIFT.0)], 0x80);
        assert_eq!(state[usize::from(VK_CONTROL.0)], 0x80);
        // Shift only.
        let state = keys.keyboard_state(true);
        assert_eq!(state[usize::from(VK_CONTROL.0)], 0);
        assert_eq!(state[usize::from(VK_SHIFT.0)], 0x80);

        // AltGr: Alt alone to the detector, Control and Alt to the layout.
        let mut keys = ModifierKeys {
            alt_gr: true,
            ..ModifierKeys::default()
        };
        keys.set(VK_RMENU.0, true);
        keys.alt_gr = true;
        let m = keys.modifiers();
        assert!(m.option && !m.control);
        let state = keys.keyboard_state(false);
        assert_eq!(state[usize::from(VK_CONTROL.0)], 0x80);
        assert_eq!(state[usize::from(VK_MENU.0)], 0x80);
        keys.set(VK_RMENU.0, false);
        assert!(!keys.alt_gr);
    }

    #[test]
    fn printable_drops_control_characters() {
        assert_eq!(printable("a").as_deref(), Some("a"));
        assert_eq!(printable("\u{13}"), None);
        assert_eq!(printable("\r"), None);
        assert_eq!(printable(""), None);
    }

    #[test]
    fn hooks_install_and_stop() {
        let (tx, _rx) = mpsc::channel();
        let mut tap = EventTap::start_with_target_pid(tx).expect("hooks");
        tap.stop();
        tap.stop();
    }
}
