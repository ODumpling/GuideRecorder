//! TCC permission checks and request prompts via plain `extern "C"` FFI.
//!
//! Checks are side-effect free. Request functions may show a system prompt
//! (or register the app in System Settings) and are never called implicitly.

use std::ffi::c_void;

use crate::permissions::PermissionStatus;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFBooleanRef = *const c_void;
type CFIndex = isize;
type IOHIDRequestType = u32;
type IOHIDAccessType = u32;
/// Mac `Boolean` (`unsigned char`), not C `bool`.
type Boolean = u8;

/// Opaque `CFDictionaryKeyCallBacks` / `CFDictionaryValueCallBacks`; only
/// ever used by address.
#[repr(C)]
struct CFDictionaryCallBacks {
    _p: [u8; 0],
}

const IOHID_REQUEST_TYPE_LISTEN_EVENT: IOHIDRequestType = 1;
const IOHID_ACCESS_TYPE_GRANTED: IOHIDAccessType = 0;
const IOHID_ACCESS_TYPE_DENIED: IOHIDAccessType = 1;
const IOHID_ACCESS_TYPE_UNKNOWN: IOHIDAccessType = 2;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFBooleanRef;
    static kCFTypeDictionaryKeyCallBacks: CFDictionaryCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFDictionaryCallBacks;
    fn CFDictionaryCreate(
        allocator: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFRelease(cf: CFTypeRef);
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXIsProcessTrusted() -> Boolean;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> Boolean;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDCheckAccess(request_type: IOHIDRequestType) -> IOHIDAccessType;
    fn IOHIDRequestAccess(request_type: IOHIDRequestType) -> bool;
}

/// The CG/AX preflight APIs return a bare bool, so "never asked" and
/// "denied" are indistinguishable; both map to `Denied`.
fn from_bool(granted: bool) -> PermissionStatus {
    if granted {
        PermissionStatus::Granted
    } else {
        PermissionStatus::Denied
    }
}

pub fn check_screen_recording() -> PermissionStatus {
    // SAFETY: no arguments; documented as safe to call from any thread.
    from_bool(unsafe { CGPreflightScreenCaptureAccess() })
}

pub fn check_accessibility() -> PermissionStatus {
    // SAFETY: no arguments; does not prompt.
    from_bool(unsafe { AXIsProcessTrusted() } != 0)
}

pub fn check_input_monitoring() -> PermissionStatus {
    // SAFETY: plain integer argument; does not prompt.
    match unsafe { IOHIDCheckAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) } {
        IOHID_ACCESS_TYPE_GRANTED => PermissionStatus::Granted,
        IOHID_ACCESS_TYPE_DENIED => PermissionStatus::Denied,
        IOHID_ACCESS_TYPE_UNKNOWN => PermissionStatus::NotDetermined,
        _ => PermissionStatus::Unknown,
    }
}

/// Whether Screen Recording is allowed right now, read live.
/// `CGPreflightScreenCaptureAccess` keeps the answer it had when the app
/// started, so a grant made in System Settings only shows there after a
/// restart. Window titles of other apps are withheld without the
/// permission, and that check is live: any titled window of another
/// process (the Dock's are always titled, so it doesn't count) means it
/// is allowed now.
pub fn screen_recording_allowed_live() -> bool {
    use objc2::runtime::AnyObject;
    use objc2_core_graphics::{CGWindowListCopyWindowInfo, CGWindowListOption};
    use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};

    type Dict = NSDictionary<AnyObject, AnyObject>;
    let Some(list) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return false;
    };
    // SAFETY: a CFArray of CFDictionary is toll-free bridged to NSArray of
    // NSDictionary; `list` outlives the borrow.
    let list: &NSArray<Dict> = unsafe { &*(&*list as *const _ as *const NSArray<Dict>) };
    let get = |d: &Dict, key: &str| {
        let key = NSString::from_str(key);
        let key: &AnyObject = key.as_ref();
        d.objectForKey(key)
    };
    let own_pid = i64::from(std::process::id());
    list.iter().any(|window| {
        let pid = get(&window, "kCGWindowOwnerPID")
            .and_then(|v| v.downcast_ref::<NSNumber>().map(|n| n.longLongValue()));
        let owner = get(&window, "kCGWindowOwnerName")
            .and_then(|v| v.downcast_ref::<NSString>().map(|s| s.to_string()));
        pid.is_some_and(|p| p != own_pid)
            && owner.as_deref() != Some("Dock")
            && owner.as_deref() != Some("Window Server")
            && get(&window, "kCGWindowName").is_some()
    })
}

/// Shows the Screen Recording prompt the first time; afterwards only adds
/// the app to the list in System Settings. Returns the current grant.
pub fn request_screen_recording() -> PermissionStatus {
    // SAFETY: no arguments.
    from_bool(unsafe { CGRequestScreenCaptureAccess() })
}

/// Calls `AXIsProcessTrustedWithOptions({kAXTrustedCheckOptionPrompt: true})`,
/// which shows the "would like to control this computer" prompt if untrusted.
pub fn request_accessibility() -> PermissionStatus {
    // SAFETY: keys/values are valid static CF objects for the duration of the
    // call; the dictionary is created with CF type callbacks (retaining) and
    // released once AX has consumed it.
    let trusted = unsafe {
        let keys = [kAXTrustedCheckOptionPrompt as CFTypeRef];
        let values = [kCFBooleanTrue as CFTypeRef];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            std::ptr::addr_of!(kCFTypeDictionaryKeyCallBacks).cast(),
            std::ptr::addr_of!(kCFTypeDictionaryValueCallBacks).cast(),
        );
        if options.is_null() {
            return check_accessibility();
        }
        let trusted = AXIsProcessTrustedWithOptions(options) != 0;
        CFRelease(options);
        trusted
    };
    from_bool(trusted)
}

/// Shows the Input Monitoring prompt if the status is not yet determined.
pub fn request_input_monitoring() -> PermissionStatus {
    // SAFETY: plain integer argument.
    unsafe { IOHIDRequestAccess(IOHID_REQUEST_TYPE_LISTEN_EVENT) };
    check_input_monitoring()
}
