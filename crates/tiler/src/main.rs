//! tiler: an automatic BSP tiling window manager for Windows.

mod app;
mod events;
mod journal;
mod monitors;
mod oneshot;
mod window;

use std::fmt;
use std::path::PathBuf;

use tracing::{error, info, warn};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HWND, LPARAM, RECT,
    WPARAM,
};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_HOTKEY,
};

use tiler_core::config::DEFAULT_CONFIG;
use tiler_core::rules::{RuleAction, WindowInfo};
use tiler_core::{Config, Rect};

use crate::app::App;
use crate::events::{Event, Hooks, WM_APP_QUIT};
use crate::journal::Journal;

/// A window handle that is `Send`, `Hash` and printable. Only ever dereferenced through Win32.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hwnd(pub isize);

impl Hwnd {
    pub fn raw(self) -> HWND {
        HWND(self.0 as *mut _)
    }
}

impl From<HWND> for Hwnd {
    fn from(h: HWND) -> Self {
        Hwnd(h.0 as isize)
    }
}

impl fmt::Debug for Hwnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#x}", self.0)
    }
}

pub fn rect_from(r: RECT) -> Rect {
    Rect::from_ltrb(r.left, r.top, r.right, r.bottom)
}

fn dir_from_env(var: &str) -> PathBuf {
    let base = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("tiler");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

const HELP: &str = "tiler: tiling window manager for Windows

USAGE: tiler [OPTION]

  (none)            run the window manager (stays running, reacts to every window)
  --once            arrange the windows open right now, then exit (nothing keeps running)
  --listen [KEY]    start a tiny background listener: KEY (default Win+Y, else Win+J)
                    arranges windows like --once. Returns immediately.
  --stop            stop the background listener
  --autostart [KEY] start the listener now and at every logon
  --no-autostart    remove it from logon
  --status          is the listener running, is it set to start at logon, recent log lines
  --list            show every top-level window and whether it would be managed
  --monitors        show monitors, their device names, and which is the laptop screen
  --restore         show any windows a crashed run left hidden, then exit
  --default-config  print the default configuration
  --verbose         debug logging
  --help            this text

Config: %APPDATA%\\tiler\\config.toml (created on first run)
Log:    %LOCALAPPDATA%\\tiler\\tiler.log";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let value_after = |f: &str| {
        args.iter()
            .position(|a| a == f)
            .and_then(|i| args.get(i + 1))
            .filter(|v| !v.starts_with("--"))
            .cloned()
    };
    if has("--help") || has("-h") {
        println!("{HELP}");
        return;
    }
    if has("--default-config") {
        print!("{DEFAULT_CONFIG}");
        return;
    }

    let data_dir = dir_from_env("LOCALAPPDATA");
    let journal = Journal::new(data_dir.join("hidden.json"));

    // Set up logging before any command runs, so the listener started from the Run
    // key at logon (which has no console anyone sees) still leaves a trace.
    let level = if has("--verbose") {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let (file_writer, _guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(&data_dir, "tiler.log"));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false)
                .with_filter(level),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(level),
        )
        .init();

    // All coordinates are physical pixels on every monitor.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    if has("--restore") {
        // The journal belongs to a running daemon: restoring it would make the daemon
        // jump between workspaces and leave it with no crash record.
        if !claim_daemon_lock() {
            eprintln!("{DAEMON_RUNNING}; nothing to restore (quit it with Alt+Shift+Q to restore its hidden windows)");
            return;
        }
        println!("restored {} window(s)", journal.restore_previous());
        return;
    }
    if has("--list") {
        list_windows();
        return;
    }
    if has("--monitors") {
        list_monitors();
        return;
    }
    if has("--stop") {
        if oneshot::stop_listener() {
            println!("listener stopped");
        } else {
            println!("no listener is running");
        }
        return;
    }
    if has("--status") {
        print_status(&data_dir);
        return;
    }
    if has("--no-autostart") {
        let removed = oneshot::clear_autostart();
        println!(
            "{}",
            if removed {
                "removed from logon"
            } else {
                "was not set to start at logon"
            }
        );
        return;
    }
    if has("--listen") || has("--autostart") {
        let flag = if has("--autostart") {
            "--autostart"
        } else {
            "--listen"
        };
        let requested = value_after(flag);
        let logon = has("--logon");
        if logon {
            info!("started from logon: {}", args.join(" "));
        }
        let key = match oneshot::start_listener(requested.as_deref(), logon) {
            Ok(key) => {
                println!("listening in the background: press {key} to arrange windows");
                println!("stop with `tiler --stop`");
                key
            }
            Err(e) if flag == "--autostart" && e.contains("already running") => {
                println!("{e}");
                match oneshot::pick_key(requested.as_deref()) {
                    Ok(k) => k,
                    Err(_) => requested.unwrap_or_else(|| "win+y".into()),
                }
            }
            Err(e) => {
                error!("{e}");
                return;
            }
        };
        if flag == "--autostart" {
            match oneshot::set_autostart(&key) {
                Ok(()) => {
                    println!("will also start at every logon (undo with `tiler --no-autostart`)")
                }
                Err(e) => eprintln!("{e}"),
            }
        }
        return;
    }

    std::panic::set_hook(Box::new(|p| {
        journal::restore_from_global();
        error!("panic: {p}");
        eprintln!("tiler panicked: {p}\nAll hidden windows were restored.");
    }));

    let config_path = dir_from_env("APPDATA").join("config.toml");
    let config = load_config(&config_path);

    if has("--once") {
        let n = oneshot::arrange(&config);
        println!("arranged {n} window(s)");
        return;
    }
    if let Some(key) = value_after("--listen-child") {
        oneshot::run_listener(&key, &config_path, has("--logon"));
        return;
    }

    // Full (daemon) mode from here on. Only one may run, and only it owns the journal:
    // `--once` and the listener never hide windows, so they must not touch it.
    if !claim_daemon_lock() {
        error!("{DAEMON_RUNNING}; exiting");
        return;
    }
    let restored = journal.restore_previous();
    if restored > 0 {
        warn!("restored {restored} window(s) left hidden by a previous run");
    }

    events::set_main_thread();
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(console_handler), true);
    }

    let msg_window = match events::create_hidden_window() {
        Ok(h) => Hwnd::from(h),
        Err(e) => {
            error!("could not create message window: {e}");
            return;
        }
    };

    let mut app = App::new(config, config_path, journal, msg_window);
    app.register_hotkeys();
    let hooks = Hooks::install();
    app.manage_existing();
    info!(
        "running; Alt+Shift+Q (default) or Ctrl+C to quit\n{}",
        app.dump_state()
    );

    let mut msg = MSG::default();
    loop {
        let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if r.0 <= 0 {
            break;
        }
        match msg.message {
            WM_HOTKEY => events::push(Event::Hotkey(msg.wParam.0 as i32)),
            WM_APP_QUIT => events::push(Event::Quit),
            _ => unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            },
        }
        while let Some(e) = events::pop() {
            if events::quitting() {
                break;
            }
            app.handle(e);
        }
        if app.quit || events::quitting() {
            break;
        }
    }

    drop(hooks);
    app.shutdown();
}

const DAEMON_LOCK: PCWSTR = w!("Local\\tiler-daemon");
const DAEMON_RUNNING: &str = "tiler is already running in full mode";

/// Take the per-session named mutex that marks a running daemon. Returns false if
/// another process holds it. The handle is kept open until this process exits, when
/// Windows releases it (also after a crash).
fn claim_daemon_lock() -> bool {
    let handle = unsafe { CreateMutexW(None, false, DAEMON_LOCK) };
    let last = unsafe { GetLastError() };
    match handle {
        Ok(h) if last == ERROR_ALREADY_EXISTS => {
            unsafe {
                let _ = CloseHandle(h);
            }
            false
        }
        // Held by an elevated daemon: its mutex's default DACL denies us.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => false,
        Ok(_) => true, // deliberately left open for the process lifetime
        Err(e) => {
            warn!(
                "could not create the single-instance mutex ({e}); not checking for another daemon"
            );
            true
        }
    }
}

pub(crate) fn load_config(path: &std::path::Path) -> Config {
    if !path.exists() {
        match std::fs::write(path, DEFAULT_CONFIG) {
            Ok(()) => info!("wrote default config to {}", path.display()),
            Err(e) => warn!("could not write default config: {e}"),
        }
        return Config::default();
    }
    let text = std::fs::read_to_string(path).unwrap_or_default();
    Config::parse(&text).unwrap_or_else(|e| {
        error!("{}: {e}; using built-in defaults", path.display());
        Config::default()
    })
}

/// Runs on a separate thread for Ctrl+C, Ctrl+Break and closing the console.
/// Restore synchronously: for a console close, Windows kills us shortly after this returns.
unsafe extern "system" fn console_handler(_ctrl_type: u32) -> BOOL {
    events::request_quit();
    journal::restore_from_global();
    unsafe {
        let _ = PostThreadMessageW(events::main_thread(), WM_APP_QUIT, WPARAM(0), LPARAM(0));
    }
    BOOL(1)
}

fn list_windows() {
    let ctx = window::Ctx::new();
    // Read the config without creating it, so rules can be checked before first run.
    let path = dir_from_env("APPDATA").join("config.toml");
    let config = match std::fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text).unwrap_or_else(|e| {
            eprintln!(
                "{}: {e}; showing results without your rules",
                path.display()
            );
            Config::default()
        }),
        Err(_) => Config::default(),
    };
    let rules = config.rule_set();
    println!(
        "{:<10} {:<22} {:<24} {:<30} TITLE",
        "HWND", "RESULT", "EXE", "CLASS"
    );
    for h in window::top_level_windows() {
        let info = window::info(h);
        let rule = rules.evaluate(&WindowInfo {
            exe: &info.exe,
            class: &info.class,
            title: &info.title,
        });
        let result = match (window::classify(h, &info, &ctx), rule) {
            (Err(reason), _) => {
                if reason == "not visible" {
                    continue; // hundreds of these; not interesting
                }
                format!("skip: {reason}")
            }
            (Ok(_), Some(RuleAction::Laptop)) => "TILE on laptop (rule)".to_string(),
            (Ok(_), Some(RuleAction::Tile)) => "TILE (rule)".to_string(),
            (Ok(_), Some(RuleAction::Float)) => "FLOAT (rule)".to_string(),
            (Ok(_), Some(RuleAction::Ignore)) => "ignore (rule)".to_string(),
            (Ok(_), Some(RuleAction::Workspace(n))) => format!("TILE ws {} (rule)", n + 1),
            (Ok(window::Kind::Tile), None) => "TILE".to_string(),
            (Ok(window::Kind::Float), None) => "FLOAT".to_string(),
        };
        let title: String = info.title.chars().take(60).collect();
        println!(
            "{:<10} {:<22} {:<24} {:<30} {}",
            format!("{h:?}"),
            result,
            info.exe,
            info.class,
            title
        );
    }
}

fn list_monitors() {
    let internal = monitors::internal_ids();
    for (i, m) in monitors::all().iter().enumerate() {
        let tag = if internal.contains(&m.id) {
            "  built-in (laptop)"
        } else {
            ""
        };
        println!(
            "{i}  {:<14} {}x{} at ({}, {}){tag}",
            m.id, m.full.w, m.full.h, m.full.x, m.full.y
        );
    }
}

/// `tiler --status`: what the owner needs to report when the listener didn't start.
fn print_status(data_dir: &std::path::Path) {
    println!(
        "listener:  {}",
        if oneshot::listener_running() {
            "running"
        } else {
            "not running"
        }
    );
    match oneshot::autostart_entry() {
        None => println!("logon:     not set (run `tiler --autostart`)"),
        Some(cmd) => {
            println!("logon:     {cmd}");
            let exe = cmd.trim_start_matches('"').split('"').next().unwrap_or("");
            if !std::path::Path::new(exe).exists() {
                println!("           that program no longer exists; run `tiler --autostart` again");
            } else if let Ok(me) = std::env::current_exe() {
                if !me.as_os_str().eq_ignore_ascii_case(exe) {
                    println!("           (this is {})", me.display());
                }
            }
            if !cmd.contains("--logon") {
                println!("           written by an older version; run `tiler --autostart` again");
            }
        }
    }
    let log = data_dir.join("tiler.log");
    println!("log:       {}", log.display());
    if let Ok(text) = std::fs::read_to_string(&log) {
        let lines: Vec<&str> = text.lines().collect();
        for l in &lines[lines.len().saturating_sub(15)..] {
            println!("  {l}");
        }
    }
}
