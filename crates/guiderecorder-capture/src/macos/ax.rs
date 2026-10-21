//! Accessibility (AX) lookups: the element under a point, the focused
//! element, and secure-field detection. See design scratchpad #440.
//!
//! All calls go through the system-wide AX element with a short messaging
//! timeout ([`MESSAGING_TIMEOUT_SECS`]). The first message that times out
//! (`kAXErrorCannotComplete`) aborts the whole lookup with
//! [`AxError::Timeout`], so a hung target app costs one timeout per lookup,
//! not one per attribute.
//!
//! Ownership: every `AXUIElementCopy*` result is +1 and is wrapped in
//! [`CFRetained`] straight away, so it is released on drop.
//!
//! # Threading
//!
//! Call these functions from a background worker only. Never call them on
//! the main thread: AX requests to our own process are served there, so a
//! lookup over a GuideRecorder window would block until it times out. Never call
//! them on the event tap callback thread either: a slow lookup gets the tap
//! disabled (`kCGEventTapDisabledByTimeout`). Debug builds assert
//! "not main thread". The functions keep no shared state and are safe to
//! call from several threads at once.
//!
//! # Secure fields (fail closed)
//!
//! Keystrokes must be masked unless the focused-element lookup returned a
//! definite non-secure element, i.e. `Ok(Some(el))` with `!el.is_secure`.
//! Timeouts, errors and "no focused element" all mean "mask". Use
//! [`must_mask_keys`]. Inside a lookup, if `AXRole` or `AXSubrole` cannot
//! be read for any reason other than "unsupported" or "no value", the
//! element is marked `is_secure = true` ([`secure_verdict`]).
//!
//! # Ancestor rule
//!
//! [`element_at`] walks `AXParent` from the hit element by the rule every
//! backend shares (see [`crate::element`], re-exported here): the first
//! actionable ancestor of a pass-through hit is reported, and may borrow a
//! label from below ([`choose_target`], [`may_inherit_label`]).
//!
//! # Field values
//!
//! Text field contents are user data. They are read only by
//! [`field_value`], for typed text (input methods, dead keys), and never
//! from a secure field ([`may_read_value`]); labels never come from them.
//!
//! # Web addresses
//!
//! [`browser_page`] reads the focused window of the frontmost app when it
//! is a browser ([`guiderecorder_core::web::browser`]): the `AXURL` of its
//! web area (Safari, and Chromium and Firefox when their web content is
//! exposed), else the `AXValue` of the first text field in its toolbar
//! (the address bar, which Chromium shows without `https://`; without a
//! toolbar, the first text field showing an address). The search reads
//! at most [`MAX_PAGE_NODES`] elements and never goes into the web area.
//! A window is private by its title or a toolbar element named for it
//! ("Incognito", "InPrivate", "Private Browsing"). The address is read
//! again only when the window or its title changes ([`PageCache`], one per
//! thread), and not from an address bar that has the focus (being typed
//! in).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;

use guiderecorder_core::events::{AppRef, RawElement};
use guiderecorder_core::model::{Point, Rect};
use guiderecorder_core::web;
use objc2::MainThreadMarker;
use objc2_app_kit::NSRunningApplication;
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFRetained, CFString, CFType, CGPoint, CGSize, CFURL,
};

use crate::element::role_in;
pub use crate::element::*;

/// Per-message AX timeout. Applied to the system-wide element, which sets
/// the global default for every element.
pub const MESSAGING_TIMEOUT_SECS: f32 = 0.25;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AxError {
    /// Accessibility permission is not granted (`kAXErrorAPIDisabled`).
    #[error("accessibility access is not granted")]
    NotTrusted,
    /// The target app did not answer within [`MESSAGING_TIMEOUT_SECS`]
    /// (`kAXErrorCannotComplete`). The element is unknown, not absent.
    #[error("the target app did not answer the AX request in time")]
    Timeout,
    /// Any other AX failure, e.g. `kAXErrorIllegalArgument`.
    #[error("AX call {call} failed with error {code}")]
    Ax { call: &'static str, code: i32 },
}

pub type Result<T> = std::result::Result<T, AxError>;

/// Element under `point` (global display points, top-left origin), after
/// applying the ancestor rule (see module docs).
///
/// `Ok(None)` means AX reported no usable element there (`NoValue`,
/// `Failure`, `InvalidUIElement`, ...). A hung app gives
/// `Err(AxError::Timeout)`. Background threads only (see `# Threading`).
pub fn element_at(point: Point) -> Result<Option<RawElement>> {
    debug_assert!(
        MainThreadMarker::new().is_none(),
        "AX lookup on main thread"
    );
    let system = system_wide();
    let mut out: *const AXUIElement = std::ptr::null();
    // SAFETY: `out` is a valid out-pointer; on success it holds a +1 element.
    let err = unsafe {
        system.copy_element_at_position(point.x as f32, point.y as f32, NonNull::from(&mut out))
    };
    let Some(hit) = lookup_result(err, out, "AXUIElementCopyElementAtPosition")? else {
        return Ok(None);
    };

    // Chain of (element, role): the hit element, then ancestors only while
    // the rule may still pick one.
    let hit_role = role_attr(&hit, "AXRole")?;
    let mut chain = vec![(hit, hit_role)];
    if role_in(chain[0].1.value(), PASS_THROUGH_ROLES) {
        while chain.len() <= MAX_PARENT_DEPTH {
            let (last, _) = chain.last().expect("chain is never empty");
            let Some(parent) = element_attr(last, "AXParent")? else {
                break;
            };
            let role = role_attr(&parent, "AXRole")?;
            let stop =
                role_in(role.value(), ACTIONABLE_ROLES) || role_in(role.value(), WALK_STOP_ROLES);
            chain.push((parent, role));
            if stop {
                break;
            }
        }
    }
    let roles: Vec<Option<&str>> = chain.iter().map(|(_, r)| r.value()).collect();
    let target = choose_target(&roles);

    let (el, role) = &chain[target];
    let mut raw = read_element(el, role.clone())?;
    // Search fields report AXSearchField as their subrole, so check both.
    let inherit = may_inherit_label(raw.role.as_deref(), raw.is_secure)
        && may_inherit_label(raw.subrole.as_deref(), raw.is_secure);
    if inherit && raw.best_label().is_none() {
        // Nearest label below the target, walking back towards the hit.
        for (child, child_role) in chain[..target].iter().rev() {
            if let Some(label) = node_label(child, child_role.value())? {
                raw.label = Some(label);
                break;
            }
        }
    }
    Ok(Some(raw))
}

/// The system-wide focused element (`AXFocusedUIElement`).
///
/// `Ok(None)` means AX reported no focused element. For keystroke masking,
/// pass the whole result to [`must_mask_keys`]. Background threads only
/// (see `# Threading`).
pub fn focused_element() -> Result<Option<RawElement>> {
    let Some(el) = focused()? else {
        return Ok(None);
    };
    let role = role_attr(&el, "AXRole")?;
    read_element(&el, role).map(Some)
}

/// The system-wide focused element itself.
fn focused() -> Result<Option<CFRetained<AXUIElement>>> {
    debug_assert!(
        MainThreadMarker::new().is_none(),
        "AX lookup on main thread"
    );
    let system = system_wide();
    let name = CFString::from_static_str("AXFocusedUIElement");
    let mut out: *const CFType = std::ptr::null();
    // SAFETY: `out` is a valid out-pointer; on success it holds a +1 value.
    let err = unsafe { system.copy_attribute_value(&name, NonNull::from(&mut out)) };
    let Some(value) = lookup_result(
        err,
        out,
        "AXUIElementCopyAttributeValue(AXFocusedUIElement)",
    )?
    else {
        return Ok(None);
    };
    Ok(value.downcast::<AXUIElement>().ok())
}

/// The text (`AXValue`) of the focused element, when it is a text field or
/// area that is not secure ([`may_read_value`], fail closed) and holds at
/// most `max_chars` characters; `None` otherwise. See `# Field values`.
/// Background threads only (see `# Threading`).
pub fn field_value(max_chars: usize) -> Result<Option<String>> {
    let Some(el) = focused()? else {
        return Ok(None);
    };
    let role = role_attr(&el, "AXRole")?;
    let subrole = role_attr(&el, "AXSubrole")?;
    let secure = secure_verdict(&role, &subrole);
    if !may_read_value(role.value(), subrole.value(), secure) {
        return Ok(None);
    }
    Ok(raw_string_attr(&el, "AXValue")?.and_then(|value| field_text(&value, max_chars)))
}

/// The frame of the scrolled area under `point` (see
/// [`guiderecorder_core::steps`], `# Scrolling`): the nearest scroll area
/// or web area at or around the element there, else its window.
/// Background threads only (see `# Threading`).
pub fn scroll_region_at(point: Point) -> Result<Option<Rect>> {
    debug_assert!(
        MainThreadMarker::new().is_none(),
        "AX lookup on main thread"
    );
    let system = system_wide();
    let mut out: *const AXUIElement = std::ptr::null();
    // SAFETY: `out` is a valid out-pointer; on success it holds a +1 element.
    let err = unsafe {
        system.copy_element_at_position(point.x as f32, point.y as f32, NonNull::from(&mut out))
    };
    let Some(hit) = lookup_result(err, out, "AXUIElementCopyElementAtPosition")? else {
        return Ok(None);
    };
    let mut el = hit.clone();
    for _ in 0..=MAX_SCROLL_DEPTH {
        let role = role_attr(&el, "AXRole")?;
        if role_in(role.value(), SCROLLED_ROLES) {
            if let Some(frame) = frame(&el)? {
                return Ok(Some(frame));
            }
        }
        if role.value() == Some("AXWindow") {
            return frame(&el);
        }
        if role_in(role.value(), WALK_STOP_ROLES) {
            break;
        }
        match element_attr(&el, "AXParent")? {
            Some(parent) => el = parent,
            None => break,
        }
    }
    match element_attr(&hit, "AXWindow")? {
        Some(window) => frame(&window),
        None => Ok(None),
    }
}

thread_local! {
    static PAGES: RefCell<PageCache> = RefCell::new(PageCache::default());
}

/// The frontmost app's focused window's page, if the app is a browser
/// ([`web::browser`]); see `# Web addresses`. Background threads only (see
/// `# Threading`).
pub fn browser_page() -> Result<Option<BrowserPage>> {
    debug_assert!(
        MainThreadMarker::new().is_none(),
        "AX lookup on main thread"
    );
    let system = system_wide();
    let Some(app_el) = element_attr(&system, "AXFocusedApplication")? else {
        return Ok(None);
    };
    let app = app_ref(&app_el);
    if app.bundle_id.as_deref().and_then(web::browser).is_none() {
        return Ok(None);
    }
    let Some(window) = element_attr(&app_el, "AXFocusedWindow")? else {
        return Ok(Some(BrowserPage {
            app,
            ..BrowserPage::default()
        }));
    };
    let title = string_attr(&window, "AXTitle")?;
    if title.as_deref().is_some_and(web::is_private_window) {
        return Ok(Some(BrowserPage::private(app)));
    }
    let pid = app.pid;
    if let Some(page) = PAGES.with(|pages| pages.borrow().get(pid, title.as_deref())) {
        return Ok(Some(page));
    }
    let page = match search_page(&window)? {
        Search::Private => BrowserPage::private(app),
        Search::Typing => return Ok(Some(BrowserPage::typing(app))),
        Search::Url(url) => BrowserPage {
            app,
            url,
            ..BrowserPage::default()
        },
    };
    PAGES.with(|pages| pages.borrow_mut().put(pid, title, page.clone()));
    Ok(Some(page))
}

/// What a browser window's search found.
enum Search {
    Private,
    /// The address bar has the focus: what it shows is being typed.
    Typing,
    Url(Option<String>),
}

/// Searches `window` breadth first, outside web content, for its address
/// (see `# Web addresses`).
fn search_page(window: &AXUIElement) -> Result<Search> {
    let mut queue: VecDeque<(CFRetained<AXUIElement>, bool)> = VecDeque::new();
    queue.extend(children(window)?.into_iter().map(|child| (child, false)));
    // The address bar is the first text field in a toolbar; without a
    // toolbar, the first text field showing an address.
    let mut address_bar: Option<Option<String>> = None;
    let mut fallback = None;
    let mut read = 0;
    while let Some((el, in_toolbar)) = queue.pop_front() {
        read += 1;
        if read > MAX_PAGE_NODES {
            break;
        }
        let role = role_attr(&el, "AXRole")?;
        match role.value() {
            Some("AXWebArea") => {
                if let Some(url) = url_attr(&el, "AXURL")?.as_deref().and_then(web::page_url) {
                    return Ok(Search::Url(Some(url)));
                }
                // Web content is never searched.
                continue;
            }
            Some("AXTextField" | "AXComboBox") if address_bar.is_none() => {
                // Being typed in, it shows what is typed, not the address.
                if bool_attr(&el, "AXFocused")? {
                    return Ok(Search::Typing);
                }
                let url = raw_string_attr(&el, "AXValue")?
                    .as_deref()
                    .and_then(web::page_url);
                if in_toolbar {
                    address_bar = Some(url);
                } else if fallback.is_none() {
                    fallback = url;
                }
            }
            Some(_) if in_toolbar => {
                let named_private = [
                    string_attr(&el, "AXTitle")?,
                    string_attr(&el, "AXDescription")?,
                ]
                .iter()
                .flatten()
                .any(|name| web::is_private_window(name));
                if named_private {
                    return Ok(Search::Private);
                }
            }
            _ => {}
        }
        let in_toolbar = in_toolbar || role.value() == Some("AXToolbar");
        queue.extend(children(&el)?.into_iter().map(|child| (child, in_toolbar)));
    }
    Ok(Search::Url(address_bar.unwrap_or(fallback)))
}

/// `AXChildren`, the elements only.
fn children(el: &AXUIElement) -> Result<Vec<CFRetained<AXUIElement>>> {
    let Some(array) = attr(el, "AXChildren")?
        .ok()
        .and_then(|v| v.downcast::<CFArray>().ok())
    else {
        return Ok(Vec::new());
    };
    // SAFETY: `AXChildren` is an array of CF objects.
    let array = unsafe { CFRetained::cast_unchecked::<CFArray<CFType>>(array) };
    Ok(array
        .iter()
        .filter_map(|child| child.downcast::<AXUIElement>().ok())
        .collect())
}

/// A string attribute as it is (trimmed only, not cut like a label).
fn raw_string_attr(el: &AXUIElement, name: &'static str) -> Result<Option<String>> {
    Ok(attr(el, name)?
        .ok()
        .and_then(|v| v.downcast::<CFString>().ok())
        .map(|s| s.to_string().trim().to_owned()))
}

/// A URL attribute (a `CFURL`, or a string in some apps) as text.
fn url_attr(el: &AXUIElement, name: &'static str) -> Result<Option<String>> {
    let Some(value) = attr(el, name)?.ok() else {
        return Ok(None);
    };
    Ok(match value.downcast::<CFURL>() {
        Ok(url) => Some(url.string().to_string()),
        Err(value) => value.downcast::<CFString>().ok().map(|s| s.to_string()),
    })
}

/// A boolean attribute; `false` when absent.
fn bool_attr(el: &AXUIElement, name: &'static str) -> Result<bool> {
    Ok(attr(el, name)?
        .ok()
        .and_then(|v| v.downcast::<CFBoolean>().ok())
        .is_some_and(|b| b.as_bool()))
}

/// Outcome of reading `AXRole` / `AXSubrole`, kept distinct so a failed
/// read can fail closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrRead {
    Value(String),
    /// Unsupported attribute or no value: a definite "not set".
    Absent,
    /// Any other error, or a value of the wrong type.
    Failed,
}

impl AttrRead {
    pub fn value(&self) -> Option<&str> {
        match self {
            AttrRead::Value(s) => Some(s),
            AttrRead::Absent | AttrRead::Failed => None,
        }
    }
}

/// Secure verdict for an element: secure by role/subrole, or unknown
/// because either read failed (fail closed).
pub fn secure_verdict(role: &AttrRead, subrole: &AttrRead) -> bool {
    *role == AttrRead::Failed
        || *subrole == AttrRead::Failed
        || is_secure(role.value(), subrole.value())
}

/// How an AX error code is handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    Success,
    /// The thing asked for is not there (definitely, or not usefully).
    Absent,
    /// Some other failure; the answer is unknown.
    Failed,
    /// Abort the whole lookup with this error.
    Abort(AxError),
}

/// Classifies an attribute-read error. Only `NoValue` and
/// `AttributeUnsupported` count as a definite absence; timeouts and missing
/// permission abort the lookup.
pub fn classify_attr_error(err: AXError) -> ErrorClass {
    match err {
        AXError::Success => ErrorClass::Success,
        AXError::NoValue | AXError::AttributeUnsupported => ErrorClass::Absent,
        AXError::CannotComplete => ErrorClass::Abort(AxError::Timeout),
        AXError::APIDisabled => ErrorClass::Abort(AxError::NotTrusted),
        _ => ErrorClass::Failed,
    }
}

/// Classifies the error of the lookup itself (hit test or focused element).
/// Errors that mean "no usable element" become [`ErrorClass::Absent`];
/// `IllegalArgument` and unknown codes are errors.
pub fn classify_lookup_error(err: AXError, call: &'static str) -> ErrorClass {
    match err {
        AXError::Success => ErrorClass::Success,
        AXError::NoValue
        | AXError::AttributeUnsupported
        | AXError::NotImplemented
        | AXError::InvalidUIElement
        | AXError::Failure => ErrorClass::Absent,
        AXError::CannotComplete => ErrorClass::Abort(AxError::Timeout),
        AXError::APIDisabled => ErrorClass::Abort(AxError::NotTrusted),
        other => ErrorClass::Abort(AxError::Ax {
            call,
            code: other.0,
        }),
    }
}

fn system_wide() -> CFRetained<AXUIElement> {
    // SAFETY: no arguments; returns a +1 element wrapped by the binding.
    let system = unsafe { AXUIElement::new_system_wide() };
    // SAFETY: valid element. A failure only means the default timeout stays.
    let _ = unsafe { system.set_messaging_timeout(MESSAGING_TIMEOUT_SECS) };
    system
}

/// Takes ownership of a +1 `out` after a successful copy (null → `None`).
///
/// # Safety
/// `out` must be null or a +1 reference handed over by an AX copy call.
unsafe fn adopt<T: objc2_core_foundation::Type>(out: *const T) -> Option<CFRetained<T>> {
    // SAFETY: guaranteed by the caller.
    NonNull::new(out.cast_mut()).map(|p| unsafe { CFRetained::from_raw(p) })
}

fn lookup_result<T: objc2_core_foundation::Type>(
    err: AXError,
    out: *const T,
    call: &'static str,
) -> Result<Option<CFRetained<T>>> {
    match classify_lookup_error(err, call) {
        // SAFETY: success means `out` is a +1 reference (or null).
        ErrorClass::Success => Ok(unsafe { adopt(out) }),
        ErrorClass::Absent | ErrorClass::Failed => Ok(None),
        ErrorClass::Abort(e) => Err(e),
    }
}

/// Raw attribute value. Timeouts and missing permission abort the lookup.
fn attr(el: &AXUIElement, name: &'static str) -> Result<AttrValue> {
    let name = CFString::from_static_str(name);
    let mut out: *const CFType = std::ptr::null();
    // SAFETY: valid element and out-pointer; on success `out` is +1.
    let err = unsafe { el.copy_attribute_value(&name, NonNull::from(&mut out)) };
    match classify_attr_error(err) {
        // SAFETY: success means `out` is a +1 reference (or null).
        ErrorClass::Success => Ok(match unsafe { adopt(out) } {
            Some(v) => AttrValue::Value(v),
            None => AttrValue::Absent,
        }),
        ErrorClass::Absent => Ok(AttrValue::Absent),
        ErrorClass::Failed => Ok(AttrValue::Failed),
        ErrorClass::Abort(e) => Err(e),
    }
}

enum AttrValue {
    Value(CFRetained<CFType>),
    Absent,
    Failed,
}

impl AttrValue {
    fn ok(self) -> Option<CFRetained<CFType>> {
        match self {
            AttrValue::Value(v) => Some(v),
            AttrValue::Absent | AttrValue::Failed => None,
        }
    }
}

/// `AXRole` / `AXSubrole`, keeping read failures distinct (see [`AttrRead`]).
fn role_attr(el: &AXUIElement, name: &'static str) -> Result<AttrRead> {
    Ok(match attr(el, name)? {
        AttrValue::Value(v) => match v.downcast::<CFString>() {
            Ok(s) => clean_text(&s.to_string()).map_or(AttrRead::Absent, AttrRead::Value),
            Err(_) => AttrRead::Failed,
        },
        AttrValue::Absent => AttrRead::Absent,
        AttrValue::Failed => AttrRead::Failed,
    })
}

/// String attribute, cleaned with [`clean_text`]. Non-fatal failures are `None`.
fn string_attr(el: &AXUIElement, name: &'static str) -> Result<Option<String>> {
    Ok(attr(el, name)?
        .ok()
        .and_then(|v| v.downcast::<CFString>().ok())
        .and_then(|s| clean_text(&s.to_string())))
}

fn element_attr(el: &AXUIElement, name: &'static str) -> Result<Option<CFRetained<AXUIElement>>> {
    Ok(attr(el, name)?
        .ok()
        .and_then(|v| v.downcast::<AXUIElement>().ok()))
}

fn value_attr<T: Default>(
    el: &AXUIElement,
    name: &'static str,
    ty: AXValueType,
) -> Result<Option<T>> {
    let Some(value) = attr(el, name)?
        .ok()
        .and_then(|v| v.downcast::<AXValue>().ok())
    else {
        return Ok(None);
    };
    let mut out = T::default();
    // SAFETY: `out` has the C layout AX writes for `ty` (CGPoint / CGSize);
    // AXValueGetValue checks the stored type and returns false on mismatch.
    let ok = unsafe { value.value(ty, NonNull::from(&mut out).cast::<c_void>()) };
    Ok(ok.then_some(out))
}

fn frame(el: &AXUIElement) -> Result<Option<Rect>> {
    let Some(pos) = value_attr::<CGPoint>(el, "AXPosition", AXValueType::CGPoint)? else {
        return Ok(None);
    };
    let Some(size) = value_attr::<CGSize>(el, "AXSize", AXValueType::CGSize)? else {
        return Ok(None);
    };
    Ok(Some(Rect {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
    }))
}

/// Label from a linked title element (its title, else its value) or the
/// placeholder text.
fn linked_label(el: &AXUIElement) -> Result<Option<String>> {
    if let Some(t) = element_attr(el, "AXTitleUIElement")? {
        if let Some(s) = string_attr(&t, "AXTitle")? {
            return Ok(Some(s));
        }
        if let Some(s) = string_attr(&t, "AXValue")? {
            return Ok(Some(s));
        }
    }
    string_attr(el, "AXPlaceholderValue")
}

/// Best label of a node below the target, for label inheritance.
fn node_label(el: &AXUIElement, role: Option<&str>) -> Result<Option<String>> {
    if let Some(s) = linked_label(el)? {
        return Ok(Some(s));
    }
    if let Some(s) = string_attr(el, "AXTitle")? {
        return Ok(Some(s));
    }
    if let Some(s) = string_attr(el, "AXDescription")? {
        return Ok(Some(s));
    }
    // A static text's visible text is its value. Other values (text field
    // contents) are user data and are never read here.
    if role == Some("AXStaticText") {
        return string_attr(el, "AXValue");
    }
    Ok(None)
}

fn window_title(el: &AXUIElement, role: Option<&str>) -> Result<Option<String>> {
    if role == Some("AXWindow") {
        return string_attr(el, "AXTitle");
    }
    match element_attr(el, "AXWindow")? {
        Some(w) => string_attr(&w, "AXTitle"),
        None => Ok(None),
    }
}

fn app_ref(el: &AXUIElement) -> AppRef {
    let mut pid: i32 = 0;
    // SAFETY: valid element and out-pointer. Local call, no IPC.
    let err = unsafe { el.pid(NonNull::from(&mut pid)) };
    if err != AXError::Success || pid <= 0 {
        return AppRef::default();
    }
    objc2::rc::autoreleasepool(|_| {
        let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid);
        AppRef {
            pid,
            name: app
                .as_ref()
                .and_then(|a| a.localizedName())
                .map(|s| s.to_string()),
            bundle_id: app
                .as_ref()
                .and_then(|a| a.bundleIdentifier())
                .map(|s| s.to_string()),
        }
    })
}

/// Reads the element's fields; `role` was already read by the caller.
fn read_element(el: &AXUIElement, role: AttrRead) -> Result<RawElement> {
    let subrole = role_attr(el, "AXSubrole")?;
    let is_secure = secure_verdict(&role, &subrole);
    let role = role.value().map(str::to_owned);
    Ok(RawElement {
        is_secure,
        title: string_attr(el, "AXTitle")?,
        description: string_attr(el, "AXDescription")?,
        label: linked_label(el)?,
        window_title: window_title(el, role.as_deref())?,
        app: app_ref(el),
        frame: frame(el)?,
        role,
        subrole: subrole.value().map(str::to_owned),
    })
}

/// A focused-element reference compared with `CFEqual`: the identity the
/// recording session caches key verdicts by.
#[derive(Clone)]
pub struct FocusRef(CFRetained<AXUIElement>);

impl PartialEq for FocusRef {
    fn eq(&self, other: &Self) -> bool {
        let a: &CFType = &self.0;
        let b: &CFType = &other.0;
        a == b
    }
}

/// The system-wide focused element reference (one AX message), or `None`
/// when there is none or the lookup failed.
pub fn focused_ref() -> Option<FocusRef> {
    // SAFETY: no arguments; returns a +1 element wrapped by the binding.
    let system = unsafe { AXUIElement::new_system_wide() };
    // SAFETY: valid element. A failure only means the default timeout stays.
    let _ = unsafe { system.set_messaging_timeout(MESSAGING_TIMEOUT_SECS) };
    let name = CFString::from_static_str("AXFocusedUIElement");
    let mut out: *const CFType = std::ptr::null();
    // SAFETY: valid element and out-pointer; on success `out` is +1.
    let err = unsafe { system.copy_attribute_value(&name, NonNull::from(&mut out)) };
    let value = NonNull::new(out.cast_mut())
        // SAFETY: a non-null `out` holds a +1 reference we now own (also
        // released this way on failure, which leaves it null in practice).
        .map(|p| unsafe { CFRetained::from_raw(p) })?;
    if err != AXError::Success {
        return None;
    }
    value.downcast::<AXUIElement>().ok().map(FocusRef)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> AttrRead {
        AttrRead::Value(s.into())
    }

    #[test]
    fn secure_verdict_fails_closed() {
        use AttrRead::{Absent, Failed};
        assert!(!secure_verdict(&v("AXTextField"), &Absent));
        assert!(!secure_verdict(&Absent, &Absent));
        assert!(secure_verdict(&v("AXTextField"), &v("AXSecureTextField")));
        assert!(secure_verdict(&Failed, &Absent));
        assert!(secure_verdict(&v("AXTextField"), &Failed));
    }

    #[test]
    fn attr_error_mapping() {
        assert_eq!(classify_attr_error(AXError::Success), ErrorClass::Success);
        assert_eq!(classify_attr_error(AXError::NoValue), ErrorClass::Absent);
        assert_eq!(
            classify_attr_error(AXError::AttributeUnsupported),
            ErrorClass::Absent
        );
        assert_eq!(
            classify_attr_error(AXError::CannotComplete),
            ErrorClass::Abort(AxError::Timeout)
        );
        assert_eq!(
            classify_attr_error(AXError::APIDisabled),
            ErrorClass::Abort(AxError::NotTrusted)
        );
        for err in [
            AXError::Failure,
            AXError::IllegalArgument,
            AXError::InvalidUIElement,
            AXError::NotImplemented,
        ] {
            assert_eq!(classify_attr_error(err), ErrorClass::Failed, "{err:?}");
        }
    }

    #[test]
    fn lookup_error_mapping() {
        let c = |e| classify_lookup_error(e, "test");
        for err in [
            AXError::NoValue,
            AXError::Failure,
            AXError::InvalidUIElement,
            AXError::NotImplemented,
        ] {
            assert_eq!(c(err), ErrorClass::Absent, "{err:?}");
        }
        assert_eq!(
            c(AXError::CannotComplete),
            ErrorClass::Abort(AxError::Timeout)
        );
        assert_eq!(
            c(AXError::APIDisabled),
            ErrorClass::Abort(AxError::NotTrusted)
        );
        assert_eq!(
            c(AXError::IllegalArgument),
            ErrorClass::Abort(AxError::Ax {
                call: "test",
                code: AXError::IllegalArgument.0
            })
        );
    }

    #[test]
    fn masking_only_for_definite_non_secure() {
        let plain = RawElement::default();
        let secure = RawElement {
            is_secure: true,
            ..Default::default()
        };
        assert!(!must_mask_keys::<AxError>(&Ok(Some(plain))));
        assert!(must_mask_keys::<AxError>(&Ok(Some(secure))));
        assert!(must_mask_keys::<AxError>(&Ok(None)));
        assert!(must_mask_keys(&Err(AxError::Timeout)));
        assert!(must_mask_keys(&Err(AxError::NotTrusted)));
    }

    #[test]
    #[ignore = "needs Accessibility permission and a live desktop"]
    fn live_lookups_do_not_error() {
        assert_eq!(
            crate::permissions::check(crate::Permission::Accessibility),
            crate::PermissionStatus::Granted,
            "grant Accessibility to the test runner"
        );
        element_at(Point { x: 10.0, y: 10.0 }).expect("element_at");
        focused_element().expect("focused_element");
    }
}
