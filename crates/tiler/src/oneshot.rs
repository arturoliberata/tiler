//! Fire-and-forget modes.
//!
//! * `tiler --once`: arrange the windows that are open right now, then exit.
//!   Nothing keeps running, nothing is hidden, nothing reacts to new windows.
//! * `tiler --listen [KEY]`: start a tiny background process whose only job is to
//!   wait for one global hotkey (default Win+Y) and then do exactly what `--once`
//!   does. It installs no event hooks and never touches a window until the key is
//!   pressed. `tiler --stop` ends it; `tiler --autostart` starts it at logon.
//!
//! A global hotkey is only ever delivered to a process that registered it, which is
//! why the hotkey variant needs that small listener.

use std::path::Path;
use std::time::Duration;

use tracing::{info, warn};
use windows::core::{w, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::FreeConsole;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ,
};
use windows::Win32::System::Threading::{
    CreateProcessW, CREATE_NO_WINDOW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::*;

use tiler_core::rules::{RuleAction, WindowInfo};
use tiler_core::{hotkey, Config, Model, OverflowMode};

use crate::{app, monitors, window, Hwnd};

const LISTENER_CLASS: PCWSTR = w!("TilerListener");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("tiler");
/// Tried in order when no key is given. Windows 11 assigns almost every Win+letter;
/// these two are the least used (Mixed Reality input switch, focus a Windows tip).
const DEFAULT_KEYS: &[&str] = &["win+y", "win+j", "win+shift+t"];

// ---------- arrange once ----------

/// Arrange every currently open, tileable window, using the same rules as the daemon
/// (max tiles, overflow stack on the laptop screen, insertion order, gaps, rules).
/// Returns the number of windows placed.
pub fn arrange(config: &Config) -> usize {
    let ctx = window::Ctx::new();
    // One workspace and stack overflow: in one-shot mode nothing may ever be hidden.
    let mut model: Model<Hwnd> = Model::new(app::monitor_list(), 1, config.gaps);
    model.max_tiles = config.max_tiles;
    model.insert_mode = config.insert;
    model.overflow_mode = OverflowMode::Stack;
    model.stack_monitor = app::resolve_stack_monitor(config);
    model.stack_monitor_always = config.laptop_stack;
    let rules = config.rule_set();

    // EnumWindows is topmost-first; insert bottom-up (oldest first).
    for h in window::top_level_windows().into_iter().rev() {
        let info = window::info(h);
        let Ok(kind) = window::classify(h, &info, &ctx) else {
            continue;
        };
        let rule = rules.evaluate(&WindowInfo {
            exe: &info.exe,
            class: &info.class,
            title: &info.title,
        });
        let tile = match rule {
            Some(RuleAction::Ignore | RuleAction::Float) => false,
            Some(RuleAction::Tile | RuleAction::Workspace(_)) => true,
            None => kind == window::Kind::Tile,
        };
        if !tile {
            continue;
        }
        let monitor = monitors::of_window(h)
            .and_then(|m| model.monitor_index(&m.id))
            .unwrap_or(0);
        model.add(h, monitor, None, false);
    }

    let layout = model.visible_layout();
    let mut crossed = Vec::new();
    for &(w, r) in &layout {
        if window::is_zoomed(w) {
            window::unmaximize(w);
        }
        let before = monitors::of_window(w).map(|m| m.id);
        window::place(w, r);
        let target = model.loc(w).map(|l| model.monitors[l.monitor].id.clone());
        if before != target {
            crossed.push((w, r));
        }
    }
    // Windows moved to a monitor with a different DPI rescale after the first move;
    // give them a moment, then place them again at the intended size.
    if !crossed.is_empty() {
        std::thread::sleep(Duration::from_millis(150));
        for (w, r) in crossed {
            window::place(w, r);
        }
    }
    for (i, m) in model.monitors.iter().enumerate() {
        let n = model.tile_count(i, 0);
        let mode = if model.is_stacked(i, 0) {
            "stacked full screen"
        } else {
            "tiled"
        };
        info!("monitor {i} ({}): {n} window(s) {mode}", m.id);
    }
    layout.len()
}

// ---------- hotkey listener ----------

fn find_listener() -> Option<HWND> {
    unsafe { FindWindowW(LISTENER_CLASS, PCWSTR::null()) }
        .ok()
        .filter(|h| !h.is_invalid())
}

/// Pick the first key that can actually be registered right now.
pub fn pick_key(requested: Option<&str>) -> Result<String, String> {
    let candidates: Vec<&str> = match requested {
        Some(k) => vec![k],
        None => DEFAULT_KEYS.to_vec(),
    };
    for k in &candidates {
        let hk = hotkey::parse(k)?;
        if unsafe { RegisterHotKey(None, 99, HOT_KEY_MODIFIERS(hk.mods), hk.vk) }.is_ok() {
            unsafe {
                let _ = UnregisterHotKey(None, 99);
            }
            return Ok(k.to_string());
        }
        warn!("{k} is already taken by Windows or another app");
    }
    Err(format!(
        "none of {candidates:?} is free. Pick another, e.g. `tiler --listen win+shift+t` or `tiler --listen ctrl+alt+t`."
    ))
}

/// `tiler --listen [KEY]`: check the key, start a detached copy of ourselves, return.
pub fn start_listener(requested: Option<&str>) -> Result<String, String> {
    if find_listener().is_some() {
        return Err("a listener is already running (stop it with `tiler --stop`)".into());
    }
    let key = pick_key(requested)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd: Vec<u16> = format!("\"{}\" --listen-child {key}", exe.display())
        .encode_utf16()
        .chain([0])
        .collect();
    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            DETACHED_PROCESS | CREATE_NO_WINDOW,
            None,
            PCWSTR::null(),
            &si,
            &mut pi,
        )
    }
    .map_err(|e| format!("could not start the listener: {e}"))?;
    unsafe {
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
    }
    Ok(key)
}

unsafe extern "system" fn listener_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The background process itself (`--listen-child KEY`). Re-reads the config on every
/// key press, so edits take effect without restarting.
pub fn run_listener(key: &str, config_path: &Path) {
    unsafe {
        let _ = FreeConsole(); // in case we were started with a console (e.g. from the Run key)
    }
    if find_listener().is_some() {
        warn!("listener already running; exiting");
        return;
    }
    let hk = match hotkey::parse(key) {
        Ok(hk) => hk,
        Err(e) => {
            warn!("{e}");
            return;
        }
    };
    if let Err(e) = unsafe { RegisterHotKey(None, 1, HOT_KEY_MODIFIERS(hk.mods), hk.vk) } {
        warn!("could not register {key}: {e}");
        return;
    }
    // A hidden window with a known class, so `tiler --stop` can find and close us.
    let created = unsafe {
        GetModuleHandleW(None).and_then(|instance| {
            let wc = WNDCLASSW {
                lpfnWndProc: Some(listener_proc),
                hInstance: instance.into(),
                lpszClassName: LISTENER_CLASS,
                ..Default::default()
            };
            RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                LISTENER_CLASS,
                w!("tiler listener"),
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
        })
    };
    if let Err(e) = created {
        warn!("could not create listener window: {e}");
        return;
    }
    info!("listener started: {key} arranges windows");

    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_HOTKEY {
            let config = crate::load_config(config_path);
            let n = arrange(&config);
            info!("{key}: arranged {n} window(s)");
        } else {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    info!("listener stopped");
}

/// `tiler --stop`
pub fn stop_listener() -> bool {
    match find_listener() {
        Some(h) => unsafe { PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0)) }.is_ok(),
        None => false,
    }
}

// ---------- autostart ----------

/// `tiler --autostart [KEY]`: start the listener at logon (per-user Run key).
pub fn set_autostart(key: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let value: Vec<u16> = format!("\"{}\" --listen {key}", exe.display())
        .encode_utf16()
        .chain([0])
        .collect();
    let r = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            REG_SZ.0,
            Some(value.as_ptr() as *const _),
            (value.len() * 2) as u32,
        )
    };
    if r.is_ok() {
        Ok(())
    } else {
        Err(format!("could not write the Run registry key: {r:?}"))
    }
}

pub fn clear_autostart() -> bool {
    unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) }.is_ok()
}
