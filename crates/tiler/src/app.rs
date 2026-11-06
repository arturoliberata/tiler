//! Event handling. The pattern is always: update the model, then `reconcile()`
//! makes reality (visibility and positions) match it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tracing::{debug, info, warn};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS,
};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use tiler_core::config::Binding;
use tiler_core::rules::{RuleAction, RuleSet, WindowInfo};
use tiler_core::{Command, Config, Model, Rect};

use crate::events::Event;
use crate::journal::Journal;
use crate::window::{self, Ctx, Kind};
use crate::{monitors, Hwnd};

const TIMER_VERIFY: usize = 1;
const TIMER_DISPLAY: usize = 2;
/// Pixels of slack when comparing where a window is with where we put it.
const TOL: i32 = 4;
/// A window that moves itself this many times within the window below gets floated.
const MAX_CORRECTIONS: usize = 4;
const CORRECTION_WINDOW: Duration = Duration::from_secs(5);

pub struct App {
    model: Model<Hwnd>,
    config: Config,
    config_path: PathBuf,
    rules: RuleSet,
    bindings: Vec<Binding>,
    journal: Journal,
    ctx: Ctx,
    msg_window: Hwnd,
    /// Windows we hid because their workspace is not active.
    hidden: HashSet<Hwnd>,
    /// Size of `hidden` when the journal was last written.
    journal_len: usize,
    /// Last frame rect we asked each tiled window to take.
    expected: HashMap<Hwnd, Rect>,
    /// Tiled windows the user maximised or made fullscreen; left alone until restored.
    maximized: HashSet<Hwnd>,
    dragging: Option<Hwnd>,
    corrections: HashMap<Hwnd, Vec<Instant>>,
    pub quit: bool,
    /// The window showing tiler's own console. Closing it means "quit", not "re-tile".
    console_host: Option<Hwnd>,
}

pub(crate) fn monitor_list() -> Vec<(String, Rect)> {
    let v: Vec<_> = monitors::all()
        .into_iter()
        .map(|m| (m.id, m.work))
        .collect();
    if v.is_empty() {
        warn!("no monitors reported; assuming 1920x1080");
        return vec![("fallback".into(), Rect::new(0, 0, 1920, 1080))];
    }
    v
}

impl App {
    pub fn new(config: Config, config_path: PathBuf, journal: Journal, msg_window: Hwnd) -> App {
        let mut model = Model::new(monitor_list(), config.workspaces, config.gaps);
        model.max_tiles = config.max_tiles;
        model.insert_mode = config.insert;
        model.overflow_mode = config.overflow;
        model.stack_monitor = resolve_stack_monitor(&config);
        model.stack_monitor_always = config.laptop_stack;
        model.stack_style = config.stack_style;
        model.cascade_offset = config.cascade_offset;
        for (i, m) in model.monitors.iter().enumerate() {
            info!("monitor {i}: {} work area {:?}", m.id, m.area);
        }
        log_stack_monitor(&model);
        App {
            model,
            rules: config.rule_set(),
            bindings: config.bindings().unwrap_or_default(),
            config,
            config_path,
            journal,
            ctx: Ctx::new(),
            msg_window,
            hidden: HashSet::new(),
            journal_len: 0,
            expected: HashMap::new(),
            maximized: HashSet::new(),
            dragging: None,
            corrections: HashMap::new(),
            quit: false,
            console_host: window::console_host(),
        }
    }

    // ---------- lifecycle ----------

    pub fn register_hotkeys(&self) {
        for (i, b) in self.bindings.iter().enumerate() {
            let r = unsafe {
                RegisterHotKey(
                    None,
                    i as i32 + 1,
                    HOT_KEY_MODIFIERS(b.hotkey.mods),
                    b.hotkey.vk,
                )
            };
            if let Err(e) = r {
                warn!(
                    "could not register hotkey {} ({}): {e}; another app probably owns it",
                    b.key, b.command
                );
            }
        }
    }

    fn unregister_hotkeys(&self) {
        for i in 0..self.bindings.len() {
            unsafe {
                let _ = UnregisterHotKey(None, i as i32 + 1);
            }
        }
    }

    pub fn manage_existing(&mut self) {
        // EnumWindows is topmost-first; insert bottom-up so the active window ends up focused.
        for h in window::top_level_windows().into_iter().rev() {
            self.try_manage(h, false);
        }
        if let Some(fg) = foreground() {
            self.model.set_focus(fg);
        }
        self.reconcile();
    }

    pub fn shutdown(&mut self) {
        self.unregister_hotkeys();
        // Windows on hidden workspaces come back minimised: the visible layout stays
        // exactly as it was, and they are one click away in the taskbar.
        let n = self.hidden.len();
        for h in self.hidden.drain() {
            window::restore_minimized(h);
        }
        self.journal.save(&self.hidden);
        info!(
            "stopped; layout left as is; {n} window(s) from hidden workspaces restored minimised"
        );
    }

    // ---------- events ----------

    pub fn handle(&mut self, e: Event) {
        if let Event::Hide(h) | Event::Destroy(h) = e {
            if Some(h) == self.console_host {
                info!("tiler's console window is closing; quitting without re-tiling");
                self.quit = true;
                return;
            }
        }
        match e {
            Event::Show(h) => {
                if self.hidden.remove(&h) {
                    // We never show a window while it is still in `hidden`, so the app did it
                    // (e.g. a notification click). Follow it: switch to its workspace.
                    if let Some(l) = self.model.loc(h) {
                        info!(
                            "{h:?} showed itself on hidden workspace {}; switching to it",
                            l.workspace + 1
                        );
                        self.model.monitors[l.monitor].active = l.workspace;
                        self.model.set_focus(h);
                    }
                    self.reconcile();
                } else if !self.model.is_managed(h) && self.try_manage(h, true) {
                    self.reconcile();
                }
            }
            Event::Hide(h) => {
                if !self.hidden.contains(&h) && self.unmanage(h) {
                    self.reconcile();
                }
            }
            Event::Destroy(h) => {
                let was_hidden = self.hidden.remove(&h);
                if self.unmanage(h) || was_hidden {
                    self.reconcile();
                }
            }
            Event::Foreground(h) => {
                if !self.model.is_managed(h) && self.try_manage(h, true) {
                    self.reconcile();
                }
                self.model.set_focus(h);
            }
            Event::MinimizeStart(h) => {
                if self.unmanage(h) {
                    self.reconcile();
                }
            }
            Event::MinimizeEnd(h) => {
                if self.try_manage(h, true) {
                    self.reconcile();
                }
            }
            Event::MoveSizeStart(h) => {
                if self.model.is_visible(h) {
                    self.dragging = Some(h);
                }
            }
            Event::MoveSizeEnd(h) => {
                if self.dragging == Some(h) {
                    self.dragging = None;
                    self.end_drag(h);
                }
            }
            Event::Location(h) => self.on_location(h),
            Event::DisplayChange => self.schedule(TIMER_DISPLAY, 300),
            Event::Hotkey(id) => {
                if let Some(b) = self.bindings.get((id - 1) as usize).cloned() {
                    debug!("hotkey {} -> {}", b.key, b.command);
                    self.execute(b.command);
                }
            }
            Event::Timer(TIMER_VERIFY) => {
                self.kill(TIMER_VERIFY);
                self.verify();
            }
            Event::Timer(TIMER_DISPLAY) => {
                self.kill(TIMER_DISPLAY);
                // Lid closed/opened changes which built-in screen (if any) exists.
                self.model.stack_monitor = resolve_stack_monitor(&self.config);
                self.model.set_monitors(monitor_list());
                log_stack_monitor(&self.model);
                info!(
                    "display configuration changed; {} monitor(s)",
                    self.model.monitors.len()
                );
                self.expected.clear();
                self.reconcile();
            }
            Event::Timer(_) => {}
            Event::Quit => self.quit = true,
        }
    }

    /// `follow`: if the window overflowed to a hidden workspace, switch to it
    /// (true for windows the user just opened; false while adopting existing windows at startup).
    fn try_manage(&mut self, h: Hwnd, follow: bool) -> bool {
        if self.model.is_managed(h) {
            return false;
        }
        let info = window::info(h);
        let Ok(kind) = window::classify(h, &info, &self.ctx) else {
            return false;
        };
        let rule = self.rules.evaluate(&WindowInfo {
            exe: &info.exe,
            class: &info.class,
            title: &info.title,
        });
        let (floating, workspace) = match rule {
            Some(RuleAction::Ignore) => return false,
            Some(RuleAction::Float) => (true, None),
            Some(RuleAction::Tile) => (false, None),
            Some(RuleAction::Workspace(n)) => (false, Some(n)),
            None => (kind == Kind::Float, None),
        };
        let monitor = monitors::of_window(h)
            .and_then(|m| self.model.monitor_index(&m.id))
            .unwrap_or(self.model.focused_monitor);
        let added = self.model.add(h, monitor, workspace, floating);
        let loc = added.loc;
        info!(
            "manage {h:?} [{}] \"{}\" ({}) -> monitor {} workspace {}{}",
            info.exe,
            info.title,
            info.class,
            loc.monitor,
            loc.workspace + 1,
            if loc.floating { " floating" } else { "" }
        );
        if added.overflowed {
            if loc.floating {
                info!(
                    "  every workspace is full (max_tiles = {}); floating it",
                    self.model.max_tiles
                );
            } else if follow && !self.model.is_visible(h) {
                info!(
                    "  workspace full; following it to workspace {}",
                    loc.workspace + 1
                );
                self.model.monitors[loc.monitor].active = loc.workspace;
                self.model.set_focus(h);
            } else {
                info!(
                    "  workspace full; placed on monitor {} workspace {}",
                    loc.monitor,
                    loc.workspace + 1
                );
            }
        }
        true
    }

    fn unmanage(&mut self, h: Hwnd) -> bool {
        self.expected.remove(&h);
        self.maximized.remove(&h);
        self.corrections.remove(&h);
        if self.dragging == Some(h) {
            self.dragging = None;
        }
        let removed = self.model.remove(h);
        if removed {
            debug!("unmanage {h:?}");
        }
        removed
    }

    fn on_location(&mut self, h: Hwnd) {
        let Some(l) = self.model.loc(h) else { return };
        if l.floating
            || !self.model.is_visible(h)
            || self.dragging == Some(h)
            || self.hidden.contains(&h)
        {
            return;
        }
        if window::is_zoomed(h) || window::is_fullscreen(h) {
            if self.maximized.insert(h) {
                debug!("{h:?} maximised/fullscreen; leaving it alone");
            }
            return;
        }
        if self.maximized.remove(&h) {
            debug!("{h:?} restored; re-tiling");
            self.expected.remove(&h);
            self.reconcile();
            return;
        }
        // Moves are applied asynchronously, so don't judge a single event.
        // Check everything once things have settled.
        self.schedule(TIMER_VERIFY, 150);
    }

    /// Compare where tiled windows are with where we put them.
    fn verify(&mut self) {
        let mut changed = false;
        for (w, _) in self.model.visible_layout() {
            if self.maximized.contains(&w) || self.dragging == Some(w) {
                continue;
            }
            let (Some(exp), Some(actual)) = (self.expected.get(&w).copied(), window::frame_rect(w))
            else {
                continue;
            };
            if actual.approx_eq(&exp, TOL) {
                continue;
            }
            let too_wide = actual.w > exp.w + TOL;
            let too_tall = actual.h > exp.h + TOL;
            if too_wide || too_tall {
                // Scenario #2: the window refuses to shrink. Learn its minimum and re-layout.
                let min = (
                    if too_wide { actual.w } else { 0 },
                    if too_tall { actual.h } else { 0 },
                );
                if self.model.learn_min_size(w, min) {
                    info!("{w:?} has a minimum size of {min:?}; re-laying out");
                    self.expected.remove(&w);
                    changed = true;
                } else {
                    debug!("{w:?} still larger than its slot (no room); accepting the overlap");
                }
                continue;
            }
            let inside = actual.x >= exp.x - TOL
                && actual.y >= exp.y - TOL
                && actual.right() <= exp.right() + TOL
                && actual.bottom() <= exp.bottom() + TOL;
            if inside {
                continue; // smaller than its slot (e.g. size increments, as in terminals); fine
            }
            // Scenario #4: the app moved itself. Put it back, unless it keeps doing it.
            let now = Instant::now();
            let hist = self.corrections.entry(w).or_default();
            hist.retain(|t| now.duration_since(*t) < CORRECTION_WINDOW);
            hist.push(now);
            if hist.len() >= MAX_CORRECTIONS {
                warn!("{w:?} keeps repositioning itself; floating it");
                self.model.set_floating(w, true);
                self.corrections.remove(&w);
            }
            self.expected.remove(&w);
            changed = true;
        }
        if changed {
            self.reconcile();
        }
    }

    /// Scenario #3: the user finished dragging or resizing a window with the mouse.
    fn end_drag(&mut self, h: Hwnd) {
        let Some(l) = self.model.loc(h) else { return };
        let target_monitor = |m: &Model<Hwnd>, mi: Option<monitors::MonitorInfo>| {
            mi.and_then(|mi| m.monitor_index(&mi.id))
        };

        if l.floating {
            // Floating windows go wherever they are dropped; just track the monitor.
            if let Some(i) = target_monitor(&self.model, monitors::of_window(h)) {
                if i != l.monitor {
                    let ws = self.model.monitors[i].active;
                    self.model.move_to(h, i, ws);
                }
            }
            return;
        }
        let (Some(old), Some(new)) = (self.model.slot(h), window::frame_rect(h)) else {
            return;
        };
        let resized = (new.w - old.w).abs() > 2 * TOL || (new.h - old.h).abs() > 2 * TOL;
        if resized {
            // Resizing moves the matching split(s); neighbours follow.
            self.model.drag_resize(h, new);
        } else {
            // Moving: drop onto a window to swap, onto another monitor to move, else snap back.
            let (cx, cy) = window::cursor_pos();
            let target = self
                .model
                .visible_layout()
                .into_iter()
                .find(|(w, r)| *w != h && r.contains_point(cx, cy))
                .map(|(w, _)| w);
            match target {
                Some(t) => {
                    self.model.swap_windows(h, t);
                }
                None => {
                    if let Some(i) = target_monitor(&self.model, monitors::at_point(cx, cy)) {
                        if i != l.monitor {
                            let ws = self.model.monitors[i].active;
                            self.model.move_to(h, i, ws);
                        }
                    }
                }
            }
        }
        self.expected.remove(&h);
        self.reconcile();
    }

    // ---------- commands ----------

    fn execute(&mut self, cmd: Command) {
        let focused = self.model.focused();
        match cmd {
            Command::Focus(d) => {
                if let Some(w) = self.model.focus_dir(d) {
                    window::focus(w);
                }
            }
            Command::Swap(d) => {
                if self.model.swap_dir(d) {
                    self.reconcile();
                    focused.into_iter().for_each(window::focus);
                }
            }
            Command::Resize(d) => {
                if self.model.move_divider(d, self.config.resize_step) {
                    self.reconcile();
                }
            }
            Command::Workspace(n) => {
                if self.model.switch_workspace(n - 1) {
                    self.reconcile();
                    self.model.focused().into_iter().for_each(window::focus);
                }
            }
            Command::SendToWorkspace(n) => {
                if let Some(f) = focused {
                    if self.model.send_to_workspace(f, n - 1) {
                        self.reconcile();
                        self.model.focused().into_iter().for_each(window::focus);
                    } else if !self.model.has_room(self.model.focused_monitor, n - 1) {
                        info!(
                            "workspace {n} is full (max_tiles = {})",
                            self.model.max_tiles
                        );
                    }
                }
            }
            Command::ToggleFloat => {
                if let Some(f) = focused {
                    if self.model.toggle_float(f) {
                        self.expected.remove(&f);
                        self.reconcile();
                    } else {
                        info!(
                            "can't tile {f:?}: this workspace is full (max_tiles = {})",
                            self.model.max_tiles
                        );
                    }
                }
            }
            Command::Close => focused.into_iter().for_each(window::close),
            Command::Retile => {
                self.expected.clear();
                self.reconcile();
            }
            Command::Reload => self.reload(),
            Command::Quit => self.quit = true,
        }
    }

    fn reload(&mut self) {
        let text = std::fs::read_to_string(&self.config_path).unwrap_or_default();
        match Config::parse(&text) {
            Ok(cfg) => {
                if cfg.workspaces != self.config.workspaces {
                    warn!("changing the number of workspaces needs a restart");
                }
                self.unregister_hotkeys();
                self.rules = cfg.rule_set();
                self.bindings = cfg.bindings().unwrap_or_default();
                self.model.gaps = cfg.gaps;
                // Applies to windows added from now on; existing layouts are not reshuffled.
                self.model.max_tiles = cfg.max_tiles;
                self.model.insert_mode = cfg.insert;
                self.model.overflow_mode = cfg.overflow;
                self.model.stack_monitor = resolve_stack_monitor(&cfg);
                self.model.stack_monitor_always = cfg.laptop_stack;
                self.model.stack_style = cfg.stack_style;
                self.model.cascade_offset = cfg.cascade_offset;
                log_stack_monitor(&self.model);
                self.config = cfg;
                self.register_hotkeys();
                self.expected.clear();
                self.reconcile();
                info!("config reloaded");
            }
            Err(e) => warn!("config not reloaded: {e}"),
        }
    }

    // ---------- reconcile ----------

    /// Make the screen match the model: hide/show for workspaces, then move tiled windows.
    pub fn reconcile(&mut self) {
        let mut hidden_changed = false;
        for w in self.model.windows() {
            let visible = self.model.is_visible(w);
            let hidden = self.hidden.contains(&w);
            if !visible && !hidden {
                self.hidden.insert(w); // insert first, so our own Hide event is ignored
                window::hide(w);
                hidden_changed = true;
            } else if visible && hidden {
                self.hidden.remove(&w); // remove first: a Show while still listed means the app did it
                window::show(w);
                self.expected.remove(&w);
                hidden_changed = true;
            }
        }
        // Destroyed windows are dropped from `hidden` in `handle`; save then too.
        if hidden_changed || self.hidden.len() != self.journal_len {
            self.journal.save(&self.hidden);
            self.journal_len = self.hidden.len();
        }

        let mut failed = false;
        for (w, r) in self.model.visible_layout() {
            if self.maximized.contains(&w)
                || self.dragging == Some(w)
                || self.expected.get(&w) == Some(&r)
            {
                continue;
            }
            let monitor_before = monitors::of_window(w).map(|m| m.id);
            if !window::place(w, r) {
                warn!("could not move {w:?}; floating it");
                self.model.set_floating(w, true);
                failed = true;
                continue;
            }
            // Crossing to a monitor with a different DPI rescales the window after the
            // first move; a second move puts it at the intended size.
            let target_id = self
                .model
                .loc(w)
                .map(|l| self.model.monitors[l.monitor].id.clone());
            if monitor_before != target_id {
                window::place(w, r);
            }
            self.expected.insert(w, r);
        }
        if failed {
            self.reconcile();
        }
    }

    fn schedule(&self, id: usize, ms: u32) {
        unsafe { SetTimer(Some(self.msg_window.raw()), id, ms, None) };
    }

    fn kill(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.msg_window.raw()), id);
        }
    }

    pub fn dump_state(&self) -> String {
        let mut s = String::new();
        for (i, m) in self.model.monitors.iter().enumerate() {
            let stack = if self.model.stack_monitor_index() == Some(i) {
                " [stack monitor]"
            } else {
                ""
            };
            s += &format!("monitor {i} {}{stack} active ws {}\n", m.id, m.active + 1);
            for (wi, ws) in m.workspaces.iter().enumerate() {
                if ws.tree.is_none() && ws.floating.is_empty() {
                    continue;
                }
                let stacked = if self.model.is_stacked(i, wi) {
                    " STACKED"
                } else {
                    ""
                };
                s += &format!(
                    "  ws {}{stacked}: tree {:?} floating {:?} focused {:?}\n",
                    wi + 1,
                    ws.tree,
                    ws.floating,
                    ws.focused
                );
            }
        }
        s
    }
}

fn foreground() -> Option<Hwnd> {
    let h = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    (!h.is_invalid()).then(|| Hwnd::from(h))
}

/// "auto" → the built-in laptop screen if one is connected; otherwise the configured name.
pub(crate) fn resolve_stack_monitor(cfg: &Config) -> Option<String> {
    if cfg.stack_monitor.eq_ignore_ascii_case("auto") {
        monitors::internal_ids().into_iter().next()
    } else {
        Some(cfg.stack_monitor.clone())
    }
}

fn log_stack_monitor(model: &Model<Hwnd>) {
    if model.overflow_mode != tiler_core::OverflowMode::Stack {
        return;
    }
    match (model.stack_monitor.as_deref(), model.stack_monitor_index()) {
        (Some(id), Some(i)) => info!("overflow stacks on monitor {i} ({id})"),
        (Some(id), None) => {
            info!("stack monitor {id} is not connected; each monitor stacks its own overflow")
        }
        (None, _) => info!("no built-in screen found; each monitor stacks its own overflow"),
    }
}
