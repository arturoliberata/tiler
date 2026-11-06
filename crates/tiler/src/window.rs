//! Thin wrappers over Win32 for inspecting and moving other processes' windows.

use std::ffi::c_void;
use std::mem::size_of;

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
    QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use tiler_core::Rect;

use crate::{monitors, rect_from, Hwnd};

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub title: String,
    pub class: String,
    pub exe: String,
    pub pid: u32,
}

pub fn info(h: Hwnd) -> Info {
    let hwnd = h.raw();
    let title = unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(hwnd, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    };
    let class = unsafe {
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf).max(0) as usize;
        String::from_utf16_lossy(&buf[..n])
    };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    Info {
        title,
        class,
        exe: exe_name(pid).unwrap_or_default(),
        pid,
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open_process(pid: u32) -> Option<OwnedHandle> {
    unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .ok()
        .map(OwnedHandle)
}

fn exe_name(pid: u32) -> Option<String> {
    let p = open_process(pid)?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(p.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
    }
    .ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    Some(path.rsplit('\\').next().unwrap_or(&path).to_string())
}

fn token_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let token = OwnedHandle(token);
    let mut elev = TOKEN_ELEVATION::default();
    let mut ret = 0u32;
    unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            Some(&mut elev as *mut TOKEN_ELEVATION as *mut c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        )
    }
    .ok()?;
    Some(elev.TokenIsElevated != 0)
}

/// Unknown counts as elevated: if we can't even query the token we can't move the window.
pub fn is_elevated(pid: u32) -> bool {
    open_process(pid)
        .and_then(|p| token_elevated(p.0))
        .unwrap_or(true)
}

pub fn self_elevated() -> bool {
    token_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

fn styles(h: Hwnd) -> (u32, u32) {
    unsafe {
        (
            GetWindowLongPtrW(h.raw(), GWL_STYLE) as u32,
            GetWindowLongPtrW(h.raw(), GWL_EXSTYLE) as u32,
        )
    }
}

pub fn is_window(h: Hwnd) -> bool {
    unsafe { IsWindow(Some(h.raw())).as_bool() }
}

pub fn is_zoomed(h: Hwnd) -> bool {
    unsafe { IsZoomed(h.raw()).as_bool() }
}

pub fn is_cloaked(h: Hwnd) -> bool {
    let mut v: u32 = 0;
    unsafe { DwmGetWindowAttribute(h.raw(), DWMWA_CLOAKED, &mut v as *mut u32 as *mut c_void, 4) }
        .is_ok()
        && v != 0
}

pub fn window_rect(h: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(h.raw(), &mut r) }.ok()?;
    Some(rect_from(r))
}

/// The visible frame, excluding the invisible resize borders Windows 10/11 adds.
pub fn frame_rect(h: Hwnd) -> Option<Rect> {
    let mut r = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            h.raw(),
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut r as *mut RECT as *mut c_void,
            size_of::<RECT>() as u32,
        )
    };
    if ok.is_ok() {
        Some(rect_from(r))
    } else {
        window_rect(h)
    }
}

/// Covers its whole monitor (taskbar included) without a caption: games, F11, video players.
pub fn is_fullscreen(h: Hwnd) -> bool {
    let (style, _) = styles(h);
    if style & WS_CAPTION.0 == WS_CAPTION.0 {
        return false;
    }
    match (window_rect(h), monitors::of_window(h)) {
        (Some(w), Some(m)) => {
            w.x <= m.full.x
                && w.y <= m.full.y
                && w.right() >= m.full.right()
                && w.bottom() >= m.full.bottom()
        }
        _ => false,
    }
}

/// Move so that the *visible frame* lands exactly on `target`.
/// Async so that a hung application can't block the tiler.
pub fn place(h: Hwnd, target: Rect) -> bool {
    let (Some(wr), Some(fr)) = (window_rect(h), frame_rect(h)) else {
        return false;
    };
    let (l, t) = (fr.x - wr.x, fr.y - wr.y);
    let (r, b) = (wr.right() - fr.right(), wr.bottom() - fr.bottom());
    unsafe {
        SetWindowPos(
            h.raw(),
            None,
            target.x - l,
            target.y - t,
            target.w + l + r,
            target.h + t + b,
            SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
        )
    }
    .is_ok()
}

pub fn show(h: Hwnd) {
    unsafe {
        let _ = ShowWindow(h.raw(), SW_SHOWNA);
    }
}

/// Bring back a window we hid, *minimised and without activating it*, so it reappears
/// in the taskbar without landing on top of the current layout.
pub fn restore_minimized(h: Hwnd) {
    unsafe {
        let _ = ShowWindow(h.raw(), SW_SHOWMINNOACTIVE);
    }
}

/// The top-level window showing tiler's own console: the Windows Terminal window when
/// hosted by it (the pseudo-console window is owned by it), else the classic console.
pub fn console_host() -> Option<Hwnd> {
    let console = unsafe { windows::Win32::System::Console::GetConsoleWindow() };
    if console.is_invalid() {
        return None;
    }
    let owner = unsafe { GetWindow(console, GW_OWNER) }
        .ok()
        .filter(|o| !o.is_invalid());
    Some(Hwnd::from(owner.unwrap_or(console)))
}

/// Restore a maximised window to its normal size without activating it.
pub fn unmaximize(h: Hwnd) {
    unsafe {
        let _ = ShowWindow(h.raw(), SW_SHOWNOACTIVATE);
    }
}

pub fn hide(h: Hwnd) {
    unsafe {
        let _ = ShowWindow(h.raw(), SW_HIDE);
    }
}

pub fn focus(h: Hwnd) {
    // Allowed because we are handling a hotkey: our process received the last input event.
    if !unsafe { SetForegroundWindow(h.raw()) }.as_bool() {
        tracing::debug!("SetForegroundWindow refused for {h:?}");
    }
}

pub fn close(h: Hwnd) {
    unsafe {
        let _ = PostMessageW(Some(h.raw()), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

unsafe extern "system" fn collect(hwnd: HWND, data: LPARAM) -> BOOL {
    let v = unsafe { &mut *(data.0 as *mut Vec<Hwnd>) };
    v.push(Hwnd::from(hwnd));
    BOOL(1)
}

/// All top-level windows in z-order (topmost first).
pub fn top_level_windows() -> Vec<Hwnd> {
    let mut v: Vec<Hwnd> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut v as *mut _ as isize));
    }
    v
}

// ---------- eligibility ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tile,
    Float,
}

pub struct Ctx {
    pub own_pid: u32,
    pub self_elevated: bool,
}

impl Ctx {
    pub fn new() -> Ctx {
        Ctx {
            own_pid: unsafe { GetCurrentProcessId() },
            self_elevated: self_elevated(),
        }
    }
}

/// Shell and system UI that must never be managed.
const IGNORED_CLASSES: &[&str] = &[
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "TopLevelWindowForOverflowXamlIsland",
    "MultitaskingViewFrame",
    "ForegroundStaging",
    "NotifyIconOverflowWindow",
    "Xaml_WindowedPopupClass",
    "TaskListThumbnailWnd",
    "TilerHiddenWindow",
];

/// Decide whether to manage a window, and how. `Err` carries the reason (shown by `--list`).
pub fn classify(h: Hwnd, info: &Info, ctx: &Ctx) -> Result<Kind, &'static str> {
    let hwnd = h.raw();
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return Err("not visible");
        }
        if IsIconic(hwnd).as_bool() {
            return Err("minimised");
        }
        if GetAncestor(hwnd, GA_ROOT) != hwnd {
            return Err("not top-level");
        }
    }
    if info.pid == ctx.own_pid {
        return Err("own window");
    }
    if is_cloaked(h) {
        return Err("cloaked");
    }
    let (style, ex) = styles(h);
    if style & WS_CHILD.0 != 0 {
        return Err("child window");
    }
    if ex & WS_EX_TOOLWINDOW.0 != 0 {
        return Err("tool window");
    }
    if ex & WS_EX_NOACTIVATE.0 != 0 {
        return Err("no-activate window");
    }
    if info.title.is_empty() {
        return Err("no title");
    }
    if IGNORED_CLASSES.contains(&info.class.as_str()) {
        return Err("shell window");
    }
    let app_window = ex & WS_EX_APPWINDOW.0 != 0;
    if style & WS_CAPTION.0 != WS_CAPTION.0 && !app_window {
        return Err("no caption");
    }
    if is_fullscreen(h) {
        return Err("fullscreen");
    }
    if !ctx.self_elevated && is_elevated(info.pid) {
        return Err("elevated"); // run tiler as admin to manage these
    }
    let owned = unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|o| !o.is_invalid());
    if owned && !app_window {
        return Ok(Kind::Float); // dialogs and other owned windows
    }
    if info.class == "#32770" || style & WS_THICKFRAME.0 == 0 {
        return Ok(Kind::Float); // standard dialog class, or fixed size
    }
    Ok(Kind::Tile)
}
