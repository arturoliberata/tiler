//! Event sources: WinEvent hooks, the hidden message window (display changes,
//! timers), hotkeys and Ctrl+C. Everything lands in one thread-local queue that the
//! main loop drains, so app logic never runs re-entrantly inside a callback.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::Hwnd;

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Show(Hwnd),
    Hide(Hwnd),
    Destroy(Hwnd),
    Foreground(Hwnd),
    MinimizeStart(Hwnd),
    MinimizeEnd(Hwnd),
    MoveSizeStart(Hwnd),
    MoveSizeEnd(Hwnd),
    Location(Hwnd),
    DisplayChange,
    Hotkey(i32),
    Timer(usize),
    Quit,
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
}

pub fn push(e: Event) {
    QUEUE.with(|q| q.borrow_mut().push_back(e));
}

pub fn pop() -> Option<Event> {
    QUEUE.with(|q| q.borrow_mut().pop_front())
}

/// Posted to the main thread to request a clean shutdown.
pub const WM_APP_QUIT: u32 = WM_APP + 1;
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Set from the console handler thread: stop reacting to events immediately, so the
/// console window disappearing doesn't trigger a re-tile on the way out.
pub fn request_quit() {
    QUITTING.store(true, Ordering::SeqCst);
}

pub fn quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

pub fn set_main_thread() {
    MAIN_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
}

pub fn main_thread() -> u32 {
    MAIN_THREAD.load(Ordering::SeqCst)
}

// ---------- WinEvent hooks ----------

const OBJID_WINDOW: i32 = 0;
const CHILDID_SELF: i32 = 0;

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if hwnd.is_invalid() || id_object != OBJID_WINDOW || id_child != CHILDID_SELF {
        return;
    }
    let w = Hwnd::from(hwnd);
    let e = match event {
        EVENT_OBJECT_SHOW | EVENT_OBJECT_UNCLOAKED => Event::Show(w),
        EVENT_OBJECT_HIDE | EVENT_OBJECT_CLOAKED => Event::Hide(w),
        EVENT_OBJECT_DESTROY => Event::Destroy(w),
        EVENT_SYSTEM_FOREGROUND => Event::Foreground(w),
        EVENT_SYSTEM_MINIMIZESTART => Event::MinimizeStart(w),
        EVENT_SYSTEM_MINIMIZEEND => Event::MinimizeEnd(w),
        EVENT_SYSTEM_MOVESIZESTART => Event::MoveSizeStart(w),
        EVENT_SYSTEM_MOVESIZEEND => Event::MoveSizeEnd(w),
        EVENT_OBJECT_LOCATIONCHANGE => Event::Location(w),
        _ => return,
    };
    push(e);
}

pub struct Hooks(Vec<HWINEVENTHOOK>);

impl Hooks {
    /// Out-of-context hooks: callbacks are delivered on this thread while it pumps messages.
    pub fn install() -> Hooks {
        let ranges = [
            (EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_MINIMIZEEND), // includes move/size start/end
            (EVENT_OBJECT_DESTROY, EVENT_OBJECT_HIDE),
            (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED),
        ];
        let hooks = ranges
            .iter()
            .filter_map(|&(min, max)| {
                let h = unsafe {
                    SetWinEventHook(
                        min,
                        max,
                        None,
                        Some(win_event_proc),
                        0,
                        0,
                        WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                    )
                };
                if h.is_invalid() {
                    tracing::error!("SetWinEventHook({min:#x}..{max:#x}) failed");
                    None
                } else {
                    Some(h)
                }
            })
            .collect();
        Hooks(hooks)
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        for h in self.0.drain(..) {
            unsafe {
                let _ = UnhookWinEvent(h);
            }
        }
    }
}

// ---------- hidden message window ----------

const SPI_SETWORKAREA: usize = 0x002F;

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_DISPLAYCHANGE | WM_DPICHANGED => push(Event::DisplayChange),
        WM_SETTINGCHANGE if wparam.0 == SPI_SETWORKAREA => push(Event::DisplayChange),
        WM_TIMER => {
            push(Event::Timer(wparam.0));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A never-shown top-level window. Top-level (not message-only) because
/// WM_DISPLAYCHANGE and WM_SETTINGCHANGE are only broadcast to top-level windows.
pub fn create_hidden_window() -> windows::core::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = w!("TilerHiddenWindow");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class,
            w!("tiler"),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}
