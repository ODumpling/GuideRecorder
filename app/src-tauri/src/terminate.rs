//! Routes NSApp `terminate:` (Dock → Quit, AppleScript `quit`, logout and
//! shutdown) through the [`QuitGate`](crate::commands::QuitGate).
//!
//! tao's app delegate has no `applicationShouldTerminate:`, so AppKit goes
//! straight to `applicationWillTerminate:`, which tao turns into
//! `RunEvent::Exit` without a `RunEvent::ExitRequested`: the editor would not
//! get to save. [`install`] adds the method to tao's delegate class. It
//! answers `NSTerminateLater` and starts the same quit as Cmd+Q; once the
//! frontend has saved and called `quit_app` (or the gate's timeouts fire),
//! [`reply`] tells AppKit whether to go on. A logout or shutdown waits for
//! that reply instead of being cancelled.
//!
//! While AppKit waits for that reply, a further Apple Event quit (Dock →
//! Quit or AppleScript `quit` again, a logout) is held by AppKit itself and
//! answered with the one reply. A further `terminate:` message, though,
//! makes AppKit terminate at once, without asking the delegate and whatever
//! the reply later says, so the save would be lost. [`install`] therefore
//! also overrides `terminate:` on tao's `NSApplication` subclass to ignore
//! it while a reply is owed ([`AppKitWait`]).
//!
//! `AppHandle::exit` stops the event loop with NSApp `stop:`, not
//! `terminate:`, so it never comes back here.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// What a quit request is waiting on: `id` of the `terminate:` that AppKit
/// holds with `NSTerminateLater`, if any. Ids keep a reply deadline from
/// answering a later request.
#[derive(Default)]
pub struct PendingReply {
    state: Mutex<(u64, Option<u64>)>,
}

impl PendingReply {
    /// Records a held `terminate:` and returns its id, or `None` if one
    /// is already held: AppKit keeps a single terminate wait, so the reply
    /// owed to the held one answers it.
    pub fn begin(&self) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if state.1.is_some() {
            return None;
        }
        state.0 += 1;
        state.1 = Some(state.0);
        state.1
    }

    /// Takes the held `terminate:`, if any, so exactly one caller replies.
    pub fn take(&self) -> bool {
        self.state.lock().unwrap().1.take().is_some()
    }

    /// Takes the held `terminate:` only if it is still request `id`.
    pub fn take_if(&self, id: u64) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.1 == Some(id) {
            state.1 = None;
            true
        } else {
            false
        }
    }
}

/// Whether AppKit is waiting for `replyToApplicationShouldTerminate:`, as
/// seen from the main thread: set when the delegate answers
/// `NSTerminateLater`, cleared just before the reply is sent. It follows
/// AppKit, not [`PendingReply`]: that is taken off the main thread before
/// the reply gets there, and a `terminate:` in between would still end
/// AppKit's wait.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Default)]
pub struct AppKitWait(AtomicBool);

impl AppKitWait {
    /// The delegate answered `reply`.
    pub fn answered(&self, reply: TerminateReply) {
        if reply == TerminateReply::Later {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// The reply is about to be sent. Cleared first: AppKit sends
    /// `terminate:` again itself when the reply is YES, and that one must
    /// go through.
    pub fn replying(&self) {
        self.0.store(false, Ordering::SeqCst);
    }

    /// Whether a `terminate:` message must be ignored.
    pub fn holds(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// `NSApplicationTerminateReply` (an `NSUInteger`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum TerminateReply {
    Now = 1,
    Later = 2,
}

#[cfg(target_os = "macos")]
pub use macos::{install, reply};

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_char, c_void, CStr};
    use std::panic::{catch_unwind, AssertUnwindSafe};
    use std::sync::{Arc, OnceLock};

    use tauri::{AppHandle, Manager, Runtime};

    use super::{AppKitWait, TerminateReply};
    use crate::commands::QuitGate;

    /// tao's app delegate class (see tao's `app_delegate.rs`).
    const TAO_DELEGATE_CLASS: &CStr = c"TaoAppDelegateParent";
    /// tao's `NSApplication` subclass (see tao's `app.rs`); it only
    /// overrides `sendEvent:`.
    const TAO_APP_CLASS: &CStr = c"TaoApp";

    /// Objective-C `BOOL`: `bool` on Apple Silicon, `signed char` on Intel.
    #[cfg(target_arch = "aarch64")]
    type ObjcBool = bool;
    #[cfg(not(target_arch = "aarch64"))]
    type ObjcBool = i8;

    #[link(name = "objc", kind = "dylib")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn object_getClass(obj: *mut c_void) -> *mut c_void;
        fn class_getName(class: *mut c_void) -> *const c_char;
        fn class_addMethod(
            class: *mut c_void,
            sel: *const c_void,
            imp: *const c_void,
            types: *const c_char,
        ) -> ObjcBool;
        fn class_getMethodImplementation(class: *mut c_void, sel: *const c_void) -> *const c_void;
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }

    /// Runs `QuitGate::on_terminate` for the app; set once by [`install`].
    type Handler = Box<dyn Fn() -> TerminateReply + Send + Sync>;
    static HANDLER: OnceLock<Handler> = OnceLock::new();

    /// `-[NSApplication terminate:]`: (self, _cmd, sender) → void.
    type TerminateImp = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void);
    /// NSApplication's own `terminate:`, which [`terminate`] calls on.
    static SUPER_TERMINATE: OnceLock<TerminateImp> = OnceLock::new();
    static WAIT: AppKitWait = AppKitWait(std::sync::atomic::AtomicBool::new(false));

    /// `NSApp`. Main thread only.
    ///
    /// SAFETY (caller): main thread. `+[NSApplication sharedApplication]`
    /// takes no argument and returns an object.
    unsafe fn ns_app() -> *mut c_void {
        let send: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        send(
            objc_getClass(c"NSApplication".as_ptr()),
            sel_registerName(c"sharedApplication".as_ptr()),
        )
    }

    /// Adds `applicationShouldTerminate:` to tao's app delegate. Main thread
    /// only (`setup`), after the event loop has set the delegate. Leaves the
    /// delegate as it is, and says so, if it is not tao's or already
    /// answers the selector.
    pub fn install<R: Runtime>(app: &AppHandle<R>) {
        let handle = app.clone();
        let handler: Handler =
            Box::new(move || handle.state::<Arc<QuitGate>>().on_terminate(&handle));
        if HANDLER.set(handler).is_err() {
            return;
        }
        // SAFETY: called on the main thread. `-delegate` takes no argument
        // and returns an object; the class and selector names are
        // NUL-terminated literals. `should_terminate` has the C signature of
        // `-(NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)`
        // (self, _cmd, sender → NSUInteger), which "Q@:@" encodes on 64-bit.
        // `-setDelegate:` takes the (live, retained by NSApp) delegate and
        // returns nothing.
        unsafe {
            let send: unsafe extern "C" fn(*mut c_void, *const c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let delegate = send(ns_app(), sel_registerName(c"delegate".as_ptr()));
            if delegate.is_null() {
                eprintln!(
                    "guiderecorder: no app delegate; Dock Quit and logout will not save first"
                );
                return;
            }
            let class = object_getClass(delegate);
            if CStr::from_ptr(class_getName(class)) != TAO_DELEGATE_CLASS {
                eprintln!(
                    "guiderecorder: unknown app delegate; Dock Quit and logout will not save first"
                );
                return;
            }
            let added = class_addMethod(
                class,
                sel_registerName(c"applicationShouldTerminate:".as_ptr()),
                should_terminate as *const c_void,
                c"Q@:@".as_ptr(),
            );
            if added == ObjcBool::default() {
                eprintln!(
                    "guiderecorder: the app delegate already handles applicationShouldTerminate:"
                );
                return;
            }
            // NSApp may cache which optional methods the delegate answers
            // when it is set; setting it again makes AppKit look anew.
            let set: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            set(
                ns_app(),
                sel_registerName(c"setDelegate:".as_ptr()),
                delegate,
            );
            guard_terminate();
        }
    }

    /// Overrides `terminate:` on tao's `NSApplication` subclass with
    /// [`terminate`]. Leaves it, and says so, if NSApp is not tao's or the
    /// subclass already has its own `terminate:`. NSApp's own class may be
    /// a key-value-observing subclass of tao's, so the method goes on
    /// tao's class, which that one inherits from.
    ///
    /// SAFETY (caller): main thread. `-isKindOfClass:` takes a class and
    /// returns a `BOOL`. `-[NSApplication terminate:]` has the C signature
    /// (self, _cmd, sender) → void, which "v@:@" encodes and
    /// [`TerminateImp`] matches. The class is not NSApplication itself, so
    /// adding the method overrides it for this subclass only, and the
    /// implementation looked up before it is NSApplication's.
    unsafe fn guard_terminate() {
        let class = objc_getClass(TAO_APP_CLASS.as_ptr());
        let is_kind: unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> ObjcBool =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        if class.is_null()
            || is_kind(
                ns_app(),
                sel_registerName(c"isKindOfClass:".as_ptr()),
                class,
            ) == ObjcBool::default()
        {
            eprintln!(
                "guiderecorder: unknown NSApplication class; a second quit may skip the save"
            );
            return;
        }
        let sel = sel_registerName(c"terminate:".as_ptr());
        let inherited = class_getMethodImplementation(class, sel);
        if inherited.is_null() {
            return;
        }
        let inherited: TerminateImp = std::mem::transmute(inherited);
        if SUPER_TERMINATE.set(inherited).is_err() {
            return;
        }
        let added = class_addMethod(class, sel, terminate as *const c_void, c"v@:@".as_ptr());
        if added == ObjcBool::default() {
            eprintln!(
                "guiderecorder: NSApp already overrides terminate:; a second quit may skip the save"
            );
        }
    }

    /// `-terminate:` on NSApp; AppKit and menu items send it on the main
    /// thread. Ignored while AppKit waits for the editor to save, where
    /// AppKit would terminate at once; otherwise NSApplication's.
    extern "C" fn terminate(this: *mut c_void, cmd: *const c_void, sender: *mut c_void) {
        if WAIT.holds() {
            eprintln!("guiderecorder: a quit is already waiting for the save; ignoring terminate:");
            return;
        }
        if let Some(inherited) = SUPER_TERMINATE.get() {
            // SAFETY: the main thread, with the arguments AppKit gave us,
            // into the implementation this override replaced.
            unsafe { inherited(this, cmd, sender) }
        }
    }

    /// `-applicationShouldTerminate:`; AppKit calls it on the main thread.
    /// A panic must not unwind into AppKit, and quits as before.
    extern "C" fn should_terminate(
        _this: *mut c_void,
        _cmd: *const c_void,
        _sender: *mut c_void,
    ) -> usize {
        let reply = catch_unwind(AssertUnwindSafe(|| {
            HANDLER
                .get()
                .map_or(TerminateReply::Now, |handler| handler())
        }))
        .unwrap_or(TerminateReply::Now);
        WAIT.answered(reply);
        reply as usize
    }

    /// Answers the `terminate:` held with `NSTerminateLater`: `quit` goes on
    /// to `applicationWillTerminate:` (tao's `RunEvent::Exit`), otherwise
    /// the app stays open and a logout is cancelled. Returns false if the
    /// reply could not be sent to the main thread.
    pub fn reply<R: Runtime>(app: &AppHandle<R>, quit: bool) -> bool {
        app.run_on_main_thread(move || {
            WAIT.replying();
            // SAFETY: on the main thread (`run_on_main_thread`).
            // `-replyToApplicationShouldTerminate:(BOOL)` returns nothing.
            unsafe {
                let send: unsafe extern "C" fn(*mut c_void, *const c_void, ObjcBool) =
                    std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
                send(
                    ns_app(),
                    sel_registerName(c"replyToApplicationShouldTerminate:".as_ptr()),
                    ObjcBool::from(quit),
                );
            }
        })
        .is_ok()
    }
}

#[cfg(not(target_os = "macos"))]
pub fn install<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) {}

/// Never called: no `terminate:` is held off macOS.
#[cfg(not(target_os = "macos"))]
pub fn reply<R: tauri::Runtime>(_app: &tauri::AppHandle<R>, _quit: bool) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_reply_is_taken_once() {
        let pending = PendingReply::default();
        assert!(!pending.take(), "nothing held yet");
        assert!(pending.begin().is_some());
        assert!(pending.take());
        assert!(!pending.take(), "only one caller replies");
    }

    #[test]
    fn begin_while_held_joins_the_held_terminate() {
        let pending = PendingReply::default();
        let id = pending.begin().unwrap();
        assert_eq!(pending.begin(), None, "no second hold");
        assert!(pending.take_if(id), "the held one still gets its reply");
        assert!(!pending.take(), "and only that one");
    }

    #[test]
    fn stale_deadline_does_not_answer_a_later_request() {
        let pending = PendingReply::default();
        let first = pending.begin().unwrap();
        assert!(pending.take(), "the first quit was answered");
        let second = pending.begin().unwrap();
        assert_ne!(first, second);
        assert!(!pending.take_if(first), "the first deadline is stale");
        assert!(pending.take_if(second));
        assert!(!pending.take(), "answered by the deadline");
    }

    #[test]
    fn terminate_is_ignored_only_while_appkit_waits() {
        let wait = AppKitWait::default();
        assert!(!wait.holds(), "a first terminate: goes through");
        wait.answered(TerminateReply::Now);
        assert!(!wait.holds(), "nothing to wait for");
        wait.answered(TerminateReply::Later);
        assert!(wait.holds(), "a second terminate: is ignored");
        wait.answered(TerminateReply::Later);
        assert!(wait.holds(), "a joined terminate keeps the wait");
        wait.replying();
        assert!(
            !wait.holds(),
            "AppKit's own terminate: after YES goes through"
        );
    }

    #[test]
    fn deadline_loses_to_the_quit() {
        let pending = PendingReply::default();
        let id = pending.begin().unwrap();
        assert!(pending.take());
        assert!(!pending.take_if(id));
    }
}
