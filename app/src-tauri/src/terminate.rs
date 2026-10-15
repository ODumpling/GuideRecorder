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
//! `AppHandle::exit` stops the event loop with NSApp `stop:`, not
//! `terminate:`, so it never comes back here.

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

    use super::TerminateReply;
    use crate::commands::QuitGate;

    /// tao's app delegate class (see tao's `app_delegate.rs`).
    const TAO_DELEGATE_CLASS: &CStr = c"TaoAppDelegateParent";

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
        fn sel_registerName(name: *const c_char) -> *const c_void;
        fn objc_msgSend();
    }

    /// Runs `QuitGate::on_terminate` for the app; set once by [`install`].
    type Handler = Box<dyn Fn() -> TerminateReply + Send + Sync>;
    static HANDLER: OnceLock<Handler> = OnceLock::new();

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
                eprintln!("guidesya: no app delegate; Dock Quit and logout will not save first");
                return;
            }
            let class = object_getClass(delegate);
            if CStr::from_ptr(class_getName(class)) != TAO_DELEGATE_CLASS {
                eprintln!(
                    "guidesya: unknown app delegate; Dock Quit and logout will not save first"
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
                eprintln!("guidesya: the app delegate already handles applicationShouldTerminate:");
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
        reply as usize
    }

    /// Answers the `terminate:` held with `NSTerminateLater`: `quit` goes on
    /// to `applicationWillTerminate:` (tao's `RunEvent::Exit`), otherwise
    /// the app stays open and a logout is cancelled. Returns false if the
    /// reply could not be sent to the main thread.
    pub fn reply<R: Runtime>(app: &AppHandle<R>, quit: bool) -> bool {
        app.run_on_main_thread(move || {
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
    fn deadline_loses_to_the_quit() {
        let pending = PendingReply::default();
        let id = pending.begin().unwrap();
        assert!(pending.take());
        assert!(!pending.take_if(id));
    }
}
