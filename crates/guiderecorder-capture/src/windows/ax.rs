//! UI Automation lookups: the element under a point, the focused element,
//! and password-field detection; the Windows counterpart of the macOS AX
//! module, with the same API.
//!
//! Each thread that looks something up gets its own `IUIAutomation` (in
//! the multithreaded COM apartment) and cache request, so every element
//! comes back with the properties read here in one round trip, and short
//! timeouts ([`CONNECTION_TIMEOUT_MS`], [`TRANSACTION_TIMEOUT_MS`]): a hung
//! target app gives [`AxError::Timeout`]. Unlike AX on macOS, a lookup over
//! one of this process's windows does not need the main thread to be free.
//!
//! # Roles
//!
//! UI Automation control types are reported as the AX role names the rest
//! of GuideRecorder uses ([`role_for`]: a button is `AXButton`, an edit
//! `AXTextField`), so the shared ancestor rule ([`crate::element`],
//! re-exported here) applies unchanged.
//!
//! # Secure fields (fail closed)
//!
//! An element is secure when its `IsPassword` property is set, or when that
//! property cannot be read. Keys are masked unless the focused-element
//! lookup returned a definite non-secure element ([`must_mask_keys`]).

use std::cell::RefCell;
use std::collections::HashMap;

use guiderecorder_core::events::{AppRef, RawElement};
use guiderecorder_core::model::{Point, Rect};
use windows::core::{Interface, BSTR};
use windows::Win32::Foundation::{HWND, POINT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, IUIAutomation, IUIAutomation2, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationTreeWalker, TreeScope_Element, UIA_BoundingRectanglePropertyId,
    UIA_ButtonControlTypeId, UIA_CalendarControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId, UIA_CustomControlTypeId,
    UIA_DataGridControlTypeId, UIA_DataItemControlTypeId, UIA_DocumentControlTypeId,
    UIA_EditControlTypeId, UIA_GroupControlTypeId, UIA_HeaderControlTypeId,
    UIA_HeaderItemControlTypeId, UIA_HelpTextPropertyId, UIA_HyperlinkControlTypeId,
    UIA_ImageControlTypeId, UIA_IsPasswordPropertyId, UIA_LabeledByPropertyId,
    UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_MenuBarControlTypeId,
    UIA_MenuControlTypeId, UIA_MenuItemControlTypeId, UIA_NamePropertyId, UIA_PaneControlTypeId,
    UIA_ProcessIdPropertyId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SeparatorControlTypeId, UIA_SliderControlTypeId,
    UIA_SpinnerControlTypeId, UIA_SplitButtonControlTypeId, UIA_StatusBarControlTypeId,
    UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TableControlTypeId, UIA_TextControlTypeId,
    UIA_ThumbControlTypeId, UIA_TitleBarControlTypeId, UIA_ToolBarControlTypeId,
    UIA_ToolTipControlTypeId, UIA_TreeControlTypeId, UIA_TreeItemControlTypeId,
    UIA_WindowControlTypeId, UIA_CONTROLTYPE_ID,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetForegroundWindow, GetWindowTextW, WindowFromPoint, GA_ROOT,
};

use crate::element::role_in;
pub use crate::element::*;

use super::apps;
use super::PerMonitorAware;

/// How long UI Automation waits to connect to a target app.
pub const CONNECTION_TIMEOUT_MS: u32 = 250;
/// How long UI Automation waits for a target app to answer a request.
pub const TRANSACTION_TIMEOUT_MS: u32 = 500;

/// `UIA_E_TIMEOUT`: the target app did not answer in time.
const UIA_E_TIMEOUT: i32 = 0x8013_1505_u32 as i32;
/// `UIA_E_ELEMENTNOTAVAILABLE`: the element is gone.
const UIA_E_ELEMENTNOTAVAILABLE: i32 = 0x8004_0201_u32 as i32;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AxError {
    /// UI Automation could not be started on this thread.
    #[error("UI Automation is not available: {0}")]
    Unavailable(String),
    /// The target app did not answer within [`TRANSACTION_TIMEOUT_MS`]. The
    /// element is unknown, not absent.
    #[error("the target app did not answer the UI Automation request in time")]
    Timeout,
    /// Any other UI Automation failure.
    #[error("UI Automation call {call} failed with error {code:#010x}")]
    Uia { call: &'static str, code: i32 },
}

pub type Result<T> = std::result::Result<T, AxError>;

/// A thread's UI Automation client.
struct Client {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    walker: IUIAutomationTreeWalker,
    /// Apps by pid, looked up once per thread.
    apps: HashMap<i32, AppRef>,
}

thread_local! {
    static CLIENT: RefCell<Option<std::result::Result<Client, AxError>>> =
        const { RefCell::new(None) };
}

impl Client {
    fn new() -> Result<Self> {
        // SAFETY: joins the multithreaded apartment; a thread already in
        // another apartment keeps it (RPC_E_CHANGED_MODE), which UI
        // Automation also works from.
        let joined = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if joined.is_err() && joined != RPC_E_CHANGED_MODE {
            return Err(AxError::Unavailable(joined.message()));
        }
        let unavailable = |e: windows::core::Error| AxError::Unavailable(e.message());
        // SAFETY: the in-process UI Automation client.
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
                .map_err(unavailable)?;
        if let Ok(timeouts) = automation.cast::<IUIAutomation2>() {
            // SAFETY: plain setters; a failure keeps the default timeouts.
            unsafe {
                let _ = timeouts.SetConnectionTimeout(CONNECTION_TIMEOUT_MS);
                let _ = timeouts.SetTransactionTimeout(TRANSACTION_TIMEOUT_MS);
            }
        }
        // SAFETY: building a cache request and getting the control view
        // walker on a live client.
        let (cache, walker) = unsafe {
            let cache = automation.CreateCacheRequest().map_err(unavailable)?;
            for property in [
                UIA_NamePropertyId,
                UIA_ControlTypePropertyId,
                UIA_IsPasswordPropertyId,
                UIA_BoundingRectanglePropertyId,
                UIA_ProcessIdPropertyId,
                UIA_HelpTextPropertyId,
                UIA_LabeledByPropertyId,
            ] {
                cache.AddProperty(property).map_err(unavailable)?;
            }
            cache.SetTreeScope(TreeScope_Element).map_err(unavailable)?;
            (cache, automation.ControlViewWalker().map_err(unavailable)?)
        };
        Ok(Self {
            automation,
            cache,
            walker,
            apps: HashMap::new(),
        })
    }

    fn app(&mut self, pid: i32) -> AppRef {
        if pid <= 0 {
            return AppRef::default();
        }
        self.apps
            .entry(pid)
            .or_insert_with(|| apps::app_for_pid(pid))
            .clone()
    }
}

/// Runs `f` with this thread's client, creating it on first use.
fn with_client<T>(f: impl FnOnce(&mut Client) -> Result<T>) -> Result<T> {
    let _dpi = PerMonitorAware::enter();
    CLIENT.with(|c| {
        let mut c = c.borrow_mut();
        match c.get_or_insert_with(Client::new) {
            Ok(client) => f(client),
            Err(e) => Err(e.clone()),
        }
    })
}

/// Maps a UI Automation error: a timeout aborts the lookup; anything else
/// is an error of `call`.
fn uia_error(call: &'static str) -> impl Fn(windows::core::Error) -> AxError {
    move |e| match e.code().0 {
        UIA_E_TIMEOUT => AxError::Timeout,
        code => AxError::Uia { call, code },
    }
}

/// `Ok(None)` when there is no usable element (none there, which UI
/// Automation reports as success with no element, or it is gone), else
/// the mapped error.
fn absent_or<T>(result: windows::core::Result<T>, call: &'static str) -> Result<Option<T>> {
    match result {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.code().is_ok() || e.code().0 == UIA_E_ELEMENTNOTAVAILABLE => Ok(None),
        Err(e) => Err(uia_error(call)(e)),
    }
}

/// Element under `point` (global display points: physical pixels), after
/// applying the ancestor rule (see [`crate::element`]).
///
/// `Ok(None)` means UI Automation found no element there. A hung app gives
/// `Err(AxError::Timeout)`.
pub fn element_at(point: Point) -> Result<Option<RawElement>> {
    let at = POINT {
        x: point.x.round() as i32,
        y: point.y.round() as i32,
    };
    with_client(|client| {
        // SAFETY: a live client and cache request.
        let hit = unsafe {
            client
                .automation
                .ElementFromPointBuildCache(at, &client.cache)
        };
        let Some(hit) = absent_or(hit, "ElementFromPoint")? else {
            return Ok(None);
        };
        let hit_role = role(&hit);
        let mut chain = vec![(hit, hit_role)];
        if role_in(chain[0].1, PASS_THROUGH_ROLES) {
            while chain.len() <= MAX_PARENT_DEPTH {
                let (last, _) = chain.last().expect("chain is never empty");
                // SAFETY: a live element, walker and cache request.
                let parent = unsafe {
                    client
                        .walker
                        .GetParentElementBuildCache(last, &client.cache)
                };
                let Some(parent) = absent_or(parent, "GetParentElement")? else {
                    break;
                };
                let role = role(&parent);
                let stop = role_in(role, ACTIONABLE_ROLES) || role_in(role, WALK_STOP_ROLES);
                chain.push((parent, role));
                if stop {
                    break;
                }
            }
        }
        let roles: Vec<Option<&str>> = chain.iter().map(|(_, r)| *r).collect();
        let target = choose_target(&roles);
        let (el, _) = &chain[target];
        let mut raw = read_element(client, el, window_title(root_window_at(at)))?;
        if may_inherit_label(raw.role.as_deref(), raw.is_secure) && raw.best_label().is_none() {
            // Nearest label below the target, walking back towards the hit.
            for (child, _) in chain[..target].iter().rev() {
                if let Some(label) = node_label(child) {
                    raw.label = Some(label);
                    break;
                }
            }
        }
        Ok(Some(raw))
    })
}

/// The focused element.
///
/// `Ok(None)` means UI Automation reported no focused element. For
/// keystroke masking, pass the whole result to [`must_mask_keys`].
pub fn focused_element() -> Result<Option<RawElement>> {
    with_client(|client| {
        // SAFETY: a live client and cache request.
        let focused = unsafe { client.automation.GetFocusedElementBuildCache(&client.cache) };
        let Some(el) = absent_or(focused, "GetFocusedElement")? else {
            return Ok(None);
        };
        // SAFETY: no arguments.
        let foreground = unsafe { GetForegroundWindow() };
        read_element(client, &el, window_title(foreground)).map(Some)
    })
}

/// The focused element's identity: its UI Automation runtime id, the
/// identity the recording session caches key verdicts by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusRef(Vec<i32>);

/// The focused element's runtime id (one lookup), or `None` when there is
/// none or the lookup failed.
pub fn focused_ref() -> Option<FocusRef> {
    with_client(|client| {
        // SAFETY: a live client; the runtime id array is destroyed below.
        unsafe {
            let el = client
                .automation
                .GetFocusedElement()
                .map_err(uia_error("GetFocusedElement"))?;
            let ids = el.GetRuntimeId().map_err(uia_error("GetRuntimeId"))?;
            let read = int_array(ids);
            let _ = SafeArrayDestroy(ids);
            Ok(read.map(FocusRef))
        }
    })
    .ok()
    .flatten()
}

/// The `i32`s of a one-dimensional `SAFEARRAY` of `VT_I4`.
///
/// # Safety
/// `array` must be null or a valid one-dimensional array of `i32`.
unsafe fn int_array(array: *mut windows::Win32::System::Com::SAFEARRAY) -> Option<Vec<i32>> {
    if array.is_null() {
        return None;
    }
    // SAFETY: a valid array, as the caller promises.
    unsafe {
        let lower = SafeArrayGetLBound(array, 1).ok()?;
        let upper = SafeArrayGetUBound(array, 1).ok()?;
        let mut out = Vec::new();
        for i in lower..=upper {
            let mut value = 0i32;
            SafeArrayGetElement(array, &i, (&mut value as *mut i32).cast()).ok()?;
            out.push(value);
        }
        Some(out)
    }
}

/// The AX role a control type stands for (see `# Roles`); `AXUnknown` for
/// one without a counterpart.
pub fn role_for(control_type: UIA_CONTROLTYPE_ID) -> &'static str {
    match control_type {
        t if t == UIA_ButtonControlTypeId => "AXButton",
        t if t == UIA_CalendarControlTypeId => "AXGroup",
        t if t == UIA_CheckBoxControlTypeId => "AXCheckBox",
        t if t == UIA_ComboBoxControlTypeId => "AXComboBox",
        t if t == UIA_EditControlTypeId => "AXTextField",
        t if t == UIA_HyperlinkControlTypeId => "AXLink",
        t if t == UIA_ImageControlTypeId => "AXImage",
        t if t == UIA_ListItemControlTypeId => "AXCell",
        t if t == UIA_ListControlTypeId => "AXList",
        t if t == UIA_MenuControlTypeId => "AXMenu",
        t if t == UIA_MenuBarControlTypeId => "AXMenuBar",
        t if t == UIA_MenuItemControlTypeId => "AXMenuItem",
        t if t == UIA_ProgressBarControlTypeId => "AXProgressIndicator",
        t if t == UIA_RadioButtonControlTypeId => "AXRadioButton",
        t if t == UIA_ScrollBarControlTypeId => "AXScrollBar",
        t if t == UIA_SliderControlTypeId => "AXSlider",
        t if t == UIA_SpinnerControlTypeId => "AXIncrementor",
        t if t == UIA_StatusBarControlTypeId => "AXGroup",
        t if t == UIA_TabControlTypeId => "AXTabGroup",
        t if t == UIA_TabItemControlTypeId => "AXTab",
        t if t == UIA_TextControlTypeId => "AXStaticText",
        t if t == UIA_ToolBarControlTypeId => "AXToolbar",
        t if t == UIA_ToolTipControlTypeId => "AXHelpTag",
        t if t == UIA_TreeControlTypeId => "AXOutline",
        t if t == UIA_TreeItemControlTypeId => "AXRow",
        t if t == UIA_GroupControlTypeId => "AXGroup",
        t if t == UIA_ThumbControlTypeId => "AXValueIndicator",
        t if t == UIA_DataGridControlTypeId => "AXTable",
        t if t == UIA_DataItemControlTypeId => "AXCell",
        t if t == UIA_DocumentControlTypeId => "AXWebArea",
        t if t == UIA_SplitButtonControlTypeId => "AXMenuButton",
        t if t == UIA_WindowControlTypeId => "AXWindow",
        // A pane is a container: clicking one is usually about a control
        // around it.
        t if t == UIA_PaneControlTypeId => "AXGroup",
        t if t == UIA_HeaderControlTypeId => "AXGroup",
        t if t == UIA_HeaderItemControlTypeId => "AXButton",
        t if t == UIA_TableControlTypeId => "AXTable",
        t if t == UIA_TitleBarControlTypeId => "AXGroup",
        t if t == UIA_SeparatorControlTypeId => "AXSplitter",
        t if t == UIA_CustomControlTypeId => "AXUnknown",
        _ => "AXUnknown",
    }
}

/// The element's role from its cached control type; `None` if unreadable.
fn role(el: &IUIAutomationElement) -> Option<&'static str> {
    // SAFETY: a live element with the control type cached.
    unsafe { el.CachedControlType() }.ok().map(role_for)
}

/// A cached string property, cleaned with [`clean_text`].
fn cached_text(read: windows::core::Result<BSTR>) -> Option<String> {
    read.ok().and_then(|s| clean_text(&s.to_string()))
}

/// Label from a linked label element (`LabeledBy`), by its name.
fn linked_label(el: &IUIAutomationElement) -> Option<String> {
    // SAFETY: a live element with `LabeledBy` cached; the label's name is
    // read live (one more request).
    unsafe {
        let label = el.CachedLabeledBy().ok()?;
        cached_text(label.CurrentName())
    }
}

/// Best label of a node below the target, for label inheritance: its
/// linked label, its name (a text's name is its visible text), its help
/// text.
fn node_label(el: &IUIAutomationElement) -> Option<String> {
    // SAFETY: a live element with these properties cached.
    unsafe {
        linked_label(el)
            .or_else(|| cached_text(el.CachedName()))
            .or_else(|| cached_text(el.CachedHelpText()))
    }
}

/// Reads the element's cached fields.
fn read_element(
    client: &mut Client,
    el: &IUIAutomationElement,
    window_title: Option<String>,
) -> Result<RawElement> {
    let role = role(el);
    // SAFETY: a live element with these properties cached.
    let (is_password, title, description, rect, pid) = unsafe {
        (
            el.CachedIsPassword().map(|b| b.as_bool()),
            cached_text(el.CachedName()),
            cached_text(el.CachedHelpText()),
            el.CachedBoundingRectangle().ok(),
            el.CachedProcessId().unwrap_or(0),
        )
    };
    // Unreadable is secure (fail closed).
    let is_secure = is_password.unwrap_or(true);
    let frame = rect
        .filter(|r| r.right > r.left && r.bottom > r.top)
        .map(|r| Rect {
            x: f64::from(r.left),
            y: f64::from(r.top),
            width: f64::from(r.right - r.left),
            height: f64::from(r.bottom - r.top),
        });
    Ok(RawElement {
        role: role.map(str::to_owned),
        subrole: is_secure.then(|| SECURE_TEXT_FIELD.to_owned()),
        title,
        description,
        label: linked_label(el),
        window_title,
        app: client.app(pid),
        frame,
        is_secure,
    })
}

/// The top-level window under `p`.
fn root_window_at(p: POINT) -> HWND {
    // SAFETY: plain queries.
    unsafe { GetAncestor(WindowFromPoint(p), GA_ROOT) }
}

/// A window's title (read without messaging the window, so a hung app
/// cannot stall it), cleaned.
fn window_title(window: HWND) -> Option<String> {
    if window.is_invalid() {
        return None;
    }
    let mut buf = [0u16; 512];
    // SAFETY: a valid buffer.
    let n = unsafe { GetWindowTextW(window, &mut buf) };
    let n = usize::try_from(n).ok()?.min(buf.len());
    clean_text(&String::from_utf16_lossy(&buf[..n]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_types_map_to_ax_roles() {
        assert_eq!(role_for(UIA_ButtonControlTypeId), "AXButton");
        assert_eq!(role_for(UIA_EditControlTypeId), "AXTextField");
        assert_eq!(role_for(UIA_TextControlTypeId), "AXStaticText");
        assert_eq!(role_for(UIA_WindowControlTypeId), "AXWindow");
        assert_eq!(role_for(UIA_CONTROLTYPE_ID(1)), "AXUnknown");
        // The shared rule then picks the button around a text.
        let roles = [
            Some(role_for(UIA_TextControlTypeId)),
            Some(role_for(UIA_ButtonControlTypeId)),
        ];
        assert_eq!(choose_target(&roles), 1);
        // An edit never borrows a label from what is typed in it.
        assert!(!may_inherit_label(
            Some(role_for(UIA_EditControlTypeId)),
            false
        ));
    }

    #[test]
    fn masking_only_for_definite_non_secure() {
        let plain = RawElement::default();
        assert!(!must_mask_keys::<AxError>(&Ok(Some(plain))));
        assert!(must_mask_keys::<AxError>(&Ok(None)));
        assert!(must_mask_keys(&Err(AxError::Timeout)));
    }

    #[test]
    fn live_lookups_do_not_error() {
        // The desktop is always there; whatever is focused, a lookup
        // answers (possibly with nothing).
        element_at(Point { x: 1.0, y: 1.0 }).expect("element_at");
        focused_element().expect("focused_element");
        let _ = focused_ref();
    }
}
