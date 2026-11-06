//! The tiler's world model: monitors → workspaces → BSP tree + floating windows.
//!
//! All operations are pure state changes. The platform layer compares
//! the model with reality afterwards (show/hide/move) in one reconcile step.

use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::Hash;

use crate::geom::{Axis, Direction, Rect};
use crate::nav;
use crate::tree::{self, Gaps, MinSizes, Node};

#[derive(Clone, Debug)]
pub struct Workspace<W> {
    pub tree: Option<Node<W>>,
    pub floating: Vec<W>,
    pub focused: Option<W>,
}

impl<W> Default for Workspace<W> {
    fn default() -> Self {
        Self {
            tree: None,
            floating: Vec::new(),
            focused: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Monitor<W> {
    /// Stable identifier (device name such as `\\.\DISPLAY1`).
    pub id: String,
    /// Work area (excludes the taskbar).
    pub area: Rect,
    pub workspaces: Vec<Workspace<W>>,
    pub active: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub monitor: usize,
    pub workspace: usize,
    pub floating: bool,
}

/// Which tile a new window splits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InsertMode {
    /// Split the focused tile (spiral-like; sizes halve each time).
    #[default]
    Focused,
    /// Split the largest tile (balanced; 4 windows form a 2x2 grid).
    Largest,
}

/// What happens to a new tiled window when its workspace already has `max_tiles`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OverflowMode {
    /// Another visible monitor with room, else the next workspace with room, else float.
    #[default]
    NextWorkspace,
    /// Another visible monitor with room, else the *stack monitor* (normally the laptop
    /// screen). A workspace holding more than `max_tiles` shows every window full screen,
    /// stacked, and goes back to tiles when the count drops.
    Stack,
}

/// How a stacked workspace places its windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum StackStyle {
    /// Every window exactly full screen, on top of each other.
    Full,
    /// Classic cascade: each newer window offset down and right, so every title bar shows.
    #[default]
    Cascade,
}

/// Result of `Model::add`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Added {
    pub loc: Loc,
    /// The requested workspace was full, so the window went somewhere else
    /// (another monitor, another workspace, or floating if everything is full).
    pub overflowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target<W> {
    Window(W),
    Monitor(usize),
}

#[derive(Debug)]
pub struct Model<W> {
    pub monitors: Vec<Monitor<W>>,
    pub focused_monitor: usize,
    pub gaps: Gaps,
    /// Maximum tiled windows per workspace; 0 = unlimited.
    pub max_tiles: usize,
    pub insert_mode: InsertMode,
    pub overflow_mode: OverflowMode,
    /// Monitor id that receives overflow in `OverflowMode::Stack`. `None` (or not
    /// connected): each monitor stacks its own overflow.
    pub stack_monitor: Option<String>,
    /// The stack monitor never tiles: any workspace there with 2+ windows is stacked.
    pub stack_monitor_always: bool,
    pub stack_style: StackStyle,
    /// Cascade step in pixels (shrunk automatically when many windows are stacked).
    pub cascade_offset: i32,
    /// Order in which windows were placed in their current workspace; the cascade puts
    /// older windows at the back (top-left) and newer ones at the front.
    seq: HashMap<W, u64>,
    next_seq: u64,
    locs: HashMap<W, Loc>,
    mins: MinSizes<W>,
    ws_count: usize,
}

impl<W: Copy + Eq + Hash + Debug> Model<W> {
    /// `monitors` must not be empty.
    pub fn new(monitors: Vec<(String, Rect)>, ws_count: usize, gaps: Gaps) -> Self {
        assert!(!monitors.is_empty(), "at least one monitor is required");
        let ws_count = ws_count.max(1);
        Model {
            monitors: monitors
                .into_iter()
                .map(|(id, area)| Self::empty_monitor(id, area, ws_count))
                .collect(),
            focused_monitor: 0,
            gaps,
            max_tiles: 0,
            insert_mode: InsertMode::Focused,
            overflow_mode: OverflowMode::NextWorkspace,
            stack_monitor: None,
            stack_monitor_always: false,
            stack_style: StackStyle::Full,
            cascade_offset: 40,
            seq: HashMap::new(),
            next_seq: 0,
            locs: HashMap::new(),
            mins: HashMap::new(),
            ws_count,
        }
    }

    fn empty_monitor(id: String, area: Rect, n: usize) -> Monitor<W> {
        Monitor {
            id,
            area,
            workspaces: (0..n).map(|_| Workspace::default()).collect(),
            active: 0,
        }
    }

    // ---------- queries ----------

    pub fn workspace_count(&self) -> usize {
        self.ws_count
    }

    pub fn loc(&self, w: W) -> Option<Loc> {
        self.locs.get(&w).copied()
    }

    pub fn is_managed(&self, w: W) -> bool {
        self.locs.contains_key(&w)
    }

    pub fn windows(&self) -> Vec<W> {
        self.locs.keys().copied().collect()
    }

    pub fn is_visible(&self, w: W) -> bool {
        self.locs
            .get(&w)
            .is_some_and(|l| self.monitors[l.monitor].active == l.workspace)
    }

    fn ws(&self, l: Loc) -> &Workspace<W> {
        &self.monitors[l.monitor].workspaces[l.workspace]
    }

    fn ws_mut(&mut self, l: Loc) -> &mut Workspace<W> {
        &mut self.monitors[l.monitor].workspaces[l.workspace]
    }

    pub fn ws_layout(&self, monitor: usize, workspace: usize) -> Vec<(W, Rect)> {
        let m = &self.monitors[monitor];
        match &m.workspaces[workspace].tree {
            Some(_) if self.is_stacked(monitor, workspace) => {
                let full = m.area.inset(self.gaps.outer);
                let order = self.stack_order(monitor, workspace);
                if self.stack_style == StackStyle::Full {
                    return order.into_iter().map(|w| (w, full)).collect();
                }
                cascade(full, self.cascade_offset, order.len())
                    .into_iter()
                    .zip(order)
                    .map(|(r, w)| (w, r))
                    .collect()
            }
            Some(t) => tree::layout(t, m.area, self.gaps, &self.mins),
            None => Vec::new(),
        }
    }

    /// Tiled windows of a workspace from back (oldest) to front (newest).
    pub fn stack_order(&self, monitor: usize, workspace: usize) -> Vec<W> {
        let mut v = self.monitors[monitor].workspaces[workspace]
            .tree
            .as_ref()
            .map(|t| t.leaves())
            .unwrap_or_default();
        v.sort_by_key(|w| self.seq.get(w).copied().unwrap_or(0));
        v
    }

    /// More tiled windows than `max_tiles`: every window is shown full screen.
    pub fn is_stacked(&self, monitor: usize, workspace: usize) -> bool {
        let n = self.tile_count(monitor, workspace);
        (self.max_tiles > 0 && n > self.max_tiles)
            || (self.stack_monitor_always && n > 1 && self.is_stack_monitor(monitor))
    }

    /// Index of the monitor that takes overflow in stack mode, if it is connected.
    pub fn stack_monitor_index(&self) -> Option<usize> {
        let id = self.stack_monitor.as_deref()?;
        self.monitor_index(id)
    }

    fn is_stack_monitor(&self, monitor: usize) -> bool {
        self.overflow_mode == OverflowMode::Stack && self.stack_monitor_index() == Some(monitor)
    }

    /// Target rectangles of every tiled window on a visible workspace.
    pub fn visible_layout(&self) -> Vec<(W, Rect)> {
        (0..self.monitors.len())
            .flat_map(|m| self.ws_layout(m, self.monitors[m].active))
            .collect()
    }

    /// Where a tiled window should be. `None` for floating or unknown windows.
    pub fn slot(&self, w: W) -> Option<Rect> {
        let l = self.loc(w)?;
        if l.floating {
            return None;
        }
        self.ws_layout(l.monitor, l.workspace)
            .into_iter()
            .find(|(x, _)| *x == w)
            .map(|(_, r)| r)
    }

    pub fn focused(&self) -> Option<W> {
        let m = &self.monitors[self.focused_monitor];
        m.workspaces[m.active].focused
    }

    pub fn active_workspace(&self) -> usize {
        self.monitors[self.focused_monitor].active
    }

    // ---------- capacity ----------

    pub fn tile_count(&self, monitor: usize, workspace: usize) -> usize {
        self.monitors[monitor].workspaces[workspace]
            .tree
            .as_ref()
            .map_or(0, |t| t.leaves().len())
    }

    /// The stack monitor never runs out of room: it stacks instead.
    pub fn has_room(&self, monitor: usize, workspace: usize) -> bool {
        self.max_tiles == 0
            || self.is_stack_monitor(monitor)
            || self.tile_count(monitor, workspace) < self.max_tiles
    }

    /// Where a new tiled window can go. Order: the requested workspace; then (unless the
    /// workspace was explicitly requested) another monitor's *visible* workspace; then the
    /// first workspace with room on the same monitor; then anywhere. `None` = all full.
    fn find_room(
        &self,
        monitor: usize,
        workspace: usize,
        explicit: bool,
    ) -> Option<(usize, usize)> {
        if self.has_room(monitor, workspace) {
            return Some((monitor, workspace));
        }
        if !explicit {
            // A visible workspace on another (non-stack) monitor that has real room.
            let visible = (0..self.monitors.len())
                .filter(|&m| m != monitor && !self.is_stack_monitor(m))
                .map(|m| (m, self.monitors[m].active))
                .find(|&(m, wi)| self.has_room(m, wi));
            if visible.is_some() {
                return visible;
            }
        }
        if self.overflow_mode == OverflowMode::Stack {
            // The stack monitor, or (if none is connected) stack on this workspace itself.
            return Some(match self.stack_monitor_index() {
                Some(sm) if explicit => (sm, workspace),
                Some(sm) => (sm, self.monitors[sm].active),
                None => (monitor, workspace),
            });
        }
        let same = (0..self.ws_count)
            .map(|wi| (monitor, wi))
            .find(|&(m, wi)| self.has_room(m, wi));
        same.or_else(|| {
            (0..self.monitors.len())
                .flat_map(|m| (0..self.ws_count).map(move |wi| (m, wi)))
                .find(|&(m, wi)| self.has_room(m, wi))
        })
    }

    /// Attach as tiled where there is room (see `find_room`), else floating.
    fn attach_with_overflow(
        &mut self,
        w: W,
        monitor: usize,
        workspace: usize,
        explicit: bool,
    ) -> Added {
        let (loc, overflowed) = match self.find_room(monitor, workspace, explicit) {
            Some((m, wi)) => (
                Loc {
                    monitor: m,
                    workspace: wi,
                    floating: false,
                },
                (m, wi) != (monitor, workspace),
            ),
            None => (
                Loc {
                    monitor,
                    workspace,
                    floating: true,
                },
                true,
            ),
        };
        self.attach(w, loc);
        Added { loc, overflowed }
    }

    // ---------- adding & removing ----------

    /// Start managing `w` on `monitor`. `workspace` defaults to the monitor's active one.
    /// Respects `max_tiles` (see `find_room`).
    pub fn add(&mut self, w: W, monitor: usize, workspace: Option<usize>, floating: bool) -> Added {
        if let Some(loc) = self.loc(w) {
            return Added {
                loc,
                overflowed: false,
            };
        }
        let monitor = monitor.min(self.monitors.len() - 1);
        let requested =
            workspace.map_or(self.monitors[monitor].active, |n| n.min(self.ws_count - 1));
        if floating {
            let loc = Loc {
                monitor,
                workspace: requested,
                floating,
            };
            self.attach(w, loc);
            return Added {
                loc,
                overflowed: false,
            };
        }
        self.attach_with_overflow(w, monitor, requested, workspace.is_some())
    }

    /// Insert into a workspace by splitting a tile chosen by `insert_mode` (or as the root).
    /// Does not check capacity.
    fn attach(&mut self, w: W, loc: Loc) {
        self.locs.insert(w, loc);
        self.next_seq += 1;
        self.seq.insert(w, self.next_seq);
        if loc.floating {
            self.ws_mut(loc).floating.push(w);
        } else {
            self.insert_into_tree(w, loc);
        }
        self.ws_mut(loc).focused = Some(w);
    }

    /// Tree insertion only; no bookkeeping.
    fn insert_into_tree(&mut self, w: W, loc: Loc) {
        {
            let ws = self.ws(loc);
            let target = match self.insert_mode {
                InsertMode::Focused => ws
                    .focused
                    .filter(|f| ws.tree.as_ref().is_some_and(|t| t.contains(*f)))
                    .or_else(|| ws.tree.as_ref().map(|t| t.first_leaf())),
                // Ties go to the last (right/bottom) tile, so the left "main" tile is split last.
                InsertMode::Largest => self
                    .ws_layout(loc.monitor, loc.workspace)
                    .into_iter()
                    .max_by_key(|(_, r)| r.w as i64 * r.h as i64)
                    .map(|(w, _)| w),
            };
            match target {
                None => self.ws_mut(loc).tree = Some(Node::Leaf(w)),
                Some(t) => {
                    let r = self
                        .ws_layout(loc.monitor, loc.workspace)
                        .into_iter()
                        .find(|(x, _)| *x == t)
                        .map_or(self.monitors[loc.monitor].area, |(_, r)| r);
                    let axis = if r.w >= r.h { Axis::X } else { Axis::Y };
                    self.ws_mut(loc)
                        .tree
                        .as_mut()
                        .expect("target exists")
                        .insert_at(t, w, axis);
                }
            }
        }
    }

    /// Re-insert every tile in order, giving a fresh balanced tree. Used when a stack
    /// turns back into tiles: insertions made while stacked don't produce a useful shape.
    fn rebuild_tree(&mut self, monitor: usize, workspace: usize) {
        let loc = Loc {
            monitor,
            workspace,
            floating: false,
        };
        let Some(t) = self.ws_mut(loc).tree.take() else {
            return;
        };
        for w in t.leaves() {
            self.insert_into_tree(w, loc);
        }
    }

    /// Remove from its workspace but keep learned data (used for moves).
    fn detach(&mut self, w: W) -> Option<Loc> {
        let l = self.locs.remove(&w)?;
        let was_stacked = !l.floating && self.is_stacked(l.monitor, l.workspace);
        let ws = self.ws_mut(l);
        if l.floating {
            ws.floating.retain(|x| *x != w);
        } else {
            tree::remove(&mut ws.tree, w);
        }
        if ws.focused == Some(w) {
            ws.focused = ws
                .tree
                .as_ref()
                .map(|t| t.first_leaf())
                .or_else(|| ws.floating.last().copied());
        }
        if was_stacked && !self.is_stacked(l.monitor, l.workspace) {
            self.rebuild_tree(l.monitor, l.workspace);
        }
        Some(l)
    }

    /// Stop managing `w` entirely.
    pub fn remove(&mut self, w: W) -> bool {
        self.mins.remove(&w);
        self.seq.remove(&w);
        self.detach(w).is_some()
    }

    // ---------- focus & navigation ----------

    pub fn set_focus(&mut self, w: W) -> bool {
        let Some(l) = self.loc(w) else { return false };
        self.focused_monitor = l.monitor;
        self.ws_mut(l).focused = Some(w);
        true
    }

    /// The window or monitor that lies in `dir` from the focused window.
    pub fn neighbor(&self, dir: Direction) -> Option<Target<W>> {
        let fm = self.focused_monitor;
        let focused = self.focused();
        let from = focused
            .and_then(|f| self.slot(f))
            .unwrap_or(self.monitors[fm].area);
        let cands: Vec<_> = self
            .nav_candidates()
            .into_iter()
            .filter(|(w, _)| Some(*w) != focused)
            .collect();
        if let Some(w) = nav::neighbor(from, &cands, dir) {
            return Some(Target::Window(w));
        }
        let mons: Vec<_> = self
            .monitors
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != fm)
            .map(|(i, m)| (i, m.area))
            .collect();
        nav::neighbor(self.monitors[fm].area, &mons, dir).map(Target::Monitor)
    }

    /// Like `visible_layout`, but a stacked workspace contributes only its top window.
    fn nav_candidates(&self) -> Vec<(W, Rect)> {
        (0..self.monitors.len())
            .flat_map(|m| {
                let wi = self.monitors[m].active;
                let all = self.ws_layout(m, wi);
                if self.is_stacked(m, wi) {
                    let top = self.monitors[m].workspaces[wi].focused;
                    let pick = all
                        .iter()
                        .find(|(w, _)| Some(*w) == top)
                        .or(all.first())
                        .copied();
                    pick.into_iter().collect()
                } else {
                    all
                }
            })
            .collect()
    }

    /// In a stack, the next (`forward`) or previous window, wrapping around.
    fn cycle_stack(&mut self, forward: bool) -> Option<W> {
        let f = self.focused()?;
        let l = self.loc(f)?;
        if l.floating || !self.is_stacked(l.monitor, l.workspace) {
            return None;
        }
        let leaves = self.stack_order(l.monitor, l.workspace);
        let i = leaves.iter().position(|w| *w == f)?;
        let n = leaves.len();
        let next = leaves[if forward {
            (i + 1) % n
        } else {
            (i + n - 1) % n
        }];
        self.set_focus(next);
        Some(next)
    }

    /// Move focus. Returns the window that should now receive OS focus, if any.
    /// In a stacked workspace, up/down cycle through the stack; left/right leave it.
    pub fn focus_dir(&mut self, dir: Direction) -> Option<W> {
        if dir.axis() == Axis::Y {
            if let Some(w) = self.cycle_stack(dir.is_forward()) {
                return Some(w);
            }
        }
        match self.neighbor(dir)? {
            Target::Window(w) => {
                self.set_focus(w);
                Some(w)
            }
            Target::Monitor(i) => {
                self.focused_monitor = i;
                self.focused()
            }
        }
    }

    // ---------- moving windows ----------

    /// Swap the focused tiled window with its neighbour, or move it to the
    /// neighbouring monitor if there is no window in that direction.
    pub fn swap_dir(&mut self, dir: Direction) -> bool {
        let Some(f) = self.focused() else {
            return false;
        };
        if self.loc(f).is_none_or(|l| l.floating) {
            return false;
        }
        match self.neighbor(dir) {
            Some(Target::Window(t)) => self.swap_windows(f, t),
            Some(Target::Monitor(i)) => {
                let ws = self.monitors[i].active;
                self.move_to(f, i, ws) && self.set_focus(f)
            }
            None => false,
        }
    }

    pub fn swap_windows(&mut self, a: W, b: W) -> bool {
        let (Some(la), Some(lb)) = (self.loc(a), self.loc(b)) else {
            return false;
        };
        if la.floating || lb.floating || a == b {
            return false;
        }
        if (la.monitor, la.workspace) == (lb.monitor, lb.workspace) {
            if let Some(t) = self.ws_mut(la).tree.as_mut() {
                t.swap_leaves(a, b);
            }
        } else {
            if let Some(t) = self.ws_mut(la).tree.as_mut() {
                t.replace_leaf(a, b);
            }
            if let Some(t) = self.ws_mut(lb).tree.as_mut() {
                t.replace_leaf(b, a);
            }
            self.locs.insert(a, lb);
            self.locs.insert(b, la);
            let wa = self.ws_mut(la);
            if wa.focused == Some(a) {
                wa.focused = Some(b);
            }
            self.ws_mut(lb).focused = Some(a);
            self.focused_monitor = lb.monitor;
        }
        true
    }

    /// Move a window to a workspace. Refused (false) if it is tiled and the target is full.
    pub fn move_to(&mut self, w: W, monitor: usize, workspace: usize) -> bool {
        if monitor >= self.monitors.len() || workspace >= self.ws_count {
            return false;
        }
        let Some(l) = self.loc(w) else { return false };
        if !l.floating
            && (l.monitor, l.workspace) != (monitor, workspace)
            && !self.has_room(monitor, workspace)
        {
            return false;
        }
        self.detach(w);
        self.attach(
            w,
            Loc {
                monitor,
                workspace,
                floating: l.floating,
            },
        );
        true
    }

    // ---------- workspaces ----------

    /// `n` is 0-based. Returns false if already active or out of range.
    pub fn switch_workspace(&mut self, n: usize) -> bool {
        let m = &mut self.monitors[self.focused_monitor];
        if n >= self.ws_count || m.active == n {
            return false;
        }
        m.active = n;
        true
    }

    pub fn send_to_workspace(&mut self, w: W, n: usize) -> bool {
        let Some(l) = self.loc(w) else { return false };
        if l.workspace == n {
            return false;
        }
        self.move_to(w, l.monitor, n)
    }

    // ---------- floating ----------

    pub fn toggle_float(&mut self, w: W) -> bool {
        let Some(l) = self.loc(w) else { return false };
        self.set_floating(w, !l.floating)
    }

    pub fn set_floating(&mut self, w: W, floating: bool) -> bool {
        let Some(l) = self.loc(w) else { return false };
        if l.floating == floating || (!floating && !self.has_room(l.monitor, l.workspace)) {
            return false;
        }
        self.detach(w);
        self.attach(w, Loc { floating, ..l });
        true
    }

    // ---------- sizing ----------

    pub fn move_divider(&mut self, dir: Direction, step: f32) -> bool {
        let Some(f) = self.focused() else {
            return false;
        };
        let Some(l) = self.loc(f) else { return false };
        let delta = if dir.is_forward() { step } else { -step };
        self.ws_mut(l)
            .tree
            .as_mut()
            .is_some_and(|t| t.move_divider(f, dir.axis(), delta))
    }

    /// The user resized a tiled window with the mouse; `new` is its new frame.
    pub fn drag_resize(&mut self, w: W, new: Rect) -> bool {
        let Some(old) = self.slot(w) else {
            return false;
        };
        let l = self.loc(w).expect("slot implies loc");
        let area = self.monitors[l.monitor].area.inset(self.gaps.outer);
        let inner = self.gaps.inner;
        let mins = std::mem::take(&mut self.mins);
        let changed = self
            .ws_mut(l)
            .tree
            .as_mut()
            .is_some_and(|t| t.resize_to(w, area, inner, &mins, old, new));
        self.mins = mins;
        changed
    }

    /// Record that `w` refused to be smaller than `size`. Returns true if this is new information.
    pub fn learn_min_size(&mut self, w: W, size: (i32, i32)) -> bool {
        let cur = self.mins.get(&w).copied().unwrap_or((0, 0));
        let next = (cur.0.max(size.0), cur.1.max(size.1));
        if next == cur {
            return false;
        }
        self.mins.insert(w, next);
        true
    }

    pub fn min_size(&self, w: W) -> Option<(i32, i32)> {
        self.mins.get(&w).copied()
    }

    // ---------- monitors ----------

    /// Replace the monitor set (after plug/unplug or resolution change).
    /// Monitors are matched by id. Windows on removed monitors move to the same
    /// workspace number on the first monitor.
    pub fn set_monitors(&mut self, new: Vec<(String, Rect)>) {
        if new.is_empty() {
            return;
        }
        let focused_id = self
            .monitors
            .get(self.focused_monitor)
            .map(|m| m.id.clone());
        let old = std::mem::take(&mut self.monitors);
        let mut mons: Vec<Monitor<W>> = new
            .into_iter()
            .map(|(id, area)| Self::empty_monitor(id, area, self.ws_count))
            .collect();
        let mut orphans = Vec::new();
        for om in old {
            match mons.iter_mut().find(|m| m.id == om.id) {
                Some(nm) => {
                    nm.workspaces = om.workspaces;
                    nm.active = om.active;
                }
                None => orphans.extend(om.workspaces.into_iter().enumerate()),
            }
        }
        self.monitors = mons;
        self.locs.clear();
        for (mi, m) in self.monitors.iter().enumerate() {
            for (wi, ws) in m.workspaces.iter().enumerate() {
                for w in ws.tree.iter().flat_map(|t| t.leaves()) {
                    self.locs.insert(
                        w,
                        Loc {
                            monitor: mi,
                            workspace: wi,
                            floating: false,
                        },
                    );
                }
                for &w in &ws.floating {
                    self.locs.insert(
                        w,
                        Loc {
                            monitor: mi,
                            workspace: wi,
                            floating: true,
                        },
                    );
                }
            }
        }
        for (wi, ws) in orphans {
            for w in ws.tree.iter().flat_map(|t| t.leaves()) {
                self.attach_with_overflow(w, 0, wi, true);
            }
            for w in ws.floating {
                self.attach(
                    w,
                    Loc {
                        monitor: 0,
                        workspace: wi,
                        floating: true,
                    },
                );
            }
        }
        self.focused_monitor = focused_id
            .and_then(|id| self.monitors.iter().position(|m| m.id == id))
            .unwrap_or(0);
    }

    pub fn monitor_index(&self, id: &str) -> Option<usize> {
        self.monitors.iter().position(|m| m.id == id)
    }
}

/// Rectangles for `n` cascaded windows inside `area`, back to front. The step shrinks
/// when needed so every window keeps at least 60% of the area in each dimension.
pub fn cascade(area: Rect, offset: i32, n: usize) -> Vec<Rect> {
    if n <= 1 {
        return vec![area; n];
    }
    let k = (n - 1) as i32;
    let step = offset
        .max(0)
        .min(area.w * 2 / 5 / k)
        .min(area.h * 2 / 5 / k);
    let (w, h) = (area.w - k * step, area.h - k * step);
    (0..n as i32)
        .map(|i| Rect::new(area.x + i * step, area.y + i * step, w, h))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one() -> Model<u32> {
        Model::new(
            vec![("A".into(), Rect::new(0, 0, 1000, 600))],
            3,
            Gaps::default(),
        )
    }

    fn two_mons() -> Model<u32> {
        Model::new(
            vec![
                ("A".into(), Rect::new(0, 0, 1000, 600)),
                ("B".into(), Rect::new(1000, 0, 1000, 600)),
            ],
            3,
            Gaps::default(),
        )
    }

    #[test]
    fn add_splits_focused_along_longer_axis() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false); // 1000x600 → side by side
        m.add(3, 0, None, false); // splits 2 (500x600) → stacked
        assert_eq!(m.slot(1), Some(Rect::new(0, 0, 500, 600)));
        assert_eq!(m.slot(2), Some(Rect::new(500, 0, 500, 300)));
        assert_eq!(m.slot(3), Some(Rect::new(500, 300, 500, 300)));
        assert_eq!(m.focused(), Some(3));
    }

    #[test]
    fn remove_refocuses_and_retiles() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        assert!(m.remove(2));
        assert_eq!(m.focused(), Some(1));
        assert_eq!(m.slot(1), Some(Rect::new(0, 0, 1000, 600)));
        assert!(!m.remove(2));
    }

    #[test]
    fn workspace_switch_changes_visibility() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, Some(1), false);
        assert!(m.is_visible(1) && !m.is_visible(2));
        assert!(m.switch_workspace(1));
        assert!(!m.is_visible(1) && m.is_visible(2));
        assert!(!m.switch_workspace(1));
        assert!(!m.switch_workspace(99));
    }

    #[test]
    fn send_to_workspace_moves_window() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        assert!(m.send_to_workspace(2, 2));
        assert_eq!(m.loc(2).unwrap().workspace, 2);
        assert_eq!(m.visible_layout(), vec![(1, Rect::new(0, 0, 1000, 600))]);
    }

    #[test]
    fn focus_and_swap_within_monitor() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        assert_eq!(m.focus_dir(Direction::Left), Some(1));
        assert!(m.swap_dir(Direction::Right));
        assert_eq!(m.slot(1), Some(Rect::new(500, 0, 500, 600)));
        assert_eq!(m.focused(), Some(1));
    }

    #[test]
    fn focus_crosses_to_empty_monitor_and_swap_moves_there() {
        let mut m = two_mons();
        m.add(1, 0, None, false);
        assert!(m.swap_dir(Direction::Right));
        assert_eq!(m.loc(1).unwrap().monitor, 1);
        assert_eq!(m.slot(1), Some(Rect::new(1000, 0, 1000, 600)));
        assert_eq!(m.focused_monitor, 1);
        assert_eq!(m.focus_dir(Direction::Left), None); // monitor A is empty
        assert_eq!(m.focused_monitor, 0);
    }

    #[test]
    fn swap_across_monitors() {
        let mut m = two_mons();
        m.add(1, 0, None, false);
        m.add(2, 1, None, false);
        m.set_focus(1);
        assert!(m.swap_dir(Direction::Right));
        assert_eq!(m.loc(1).unwrap().monitor, 1);
        assert_eq!(m.loc(2).unwrap().monitor, 0);
        assert_eq!(m.focused(), Some(1));
    }

    #[test]
    fn float_toggle() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        assert!(m.toggle_float(2));
        assert_eq!(m.slot(1), Some(Rect::new(0, 0, 1000, 600)));
        assert_eq!(m.slot(2), None);
        assert!(m.is_visible(2));
        assert!(m.toggle_float(2));
        assert!(m.slot(2).is_some());
    }

    #[test]
    fn learned_min_size_affects_layout() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        assert!(m.learn_min_size(2, (800, 100)));
        assert!(!m.learn_min_size(2, (700, 50)));
        assert_eq!(m.slot(2).unwrap().w, 800);
    }

    #[test]
    fn unplugged_monitor_windows_migrate() {
        let mut m = two_mons();
        m.add(1, 0, None, false);
        m.add(2, 1, None, false);
        m.add(3, 1, Some(2), false);
        m.set_focus(2);
        m.set_monitors(vec![("A".into(), Rect::new(0, 0, 1000, 600))]);
        assert_eq!(m.monitors.len(), 1);
        assert_eq!(
            m.loc(2),
            Some(Loc {
                monitor: 0,
                workspace: 0,
                floating: false
            })
        );
        assert_eq!(
            m.loc(3),
            Some(Loc {
                monitor: 0,
                workspace: 2,
                floating: false
            })
        );
        assert_eq!(m.visible_layout().len(), 2);
        assert_eq!(m.focused_monitor, 0);
    }

    #[test]
    fn replugged_monitor_keeps_identity() {
        let mut m = two_mons();
        m.add(2, 1, None, false);
        m.set_monitors(vec![
            ("B".into(), Rect::new(0, 0, 1920, 1080)),
            ("A".into(), Rect::new(1920, 0, 1000, 600)),
        ]);
        assert_eq!(m.loc(2).unwrap().monitor, 0);
        assert_eq!(m.slot(2), Some(Rect::new(0, 0, 1920, 1080)));
    }

    fn limited(mons: usize, max: usize) -> Model<u32> {
        let mut m = Model::new(
            (0..mons)
                .map(|i| (format!("M{i}"), Rect::new(i as i32 * 1000, 0, 1000, 600)))
                .collect(),
            2,
            Gaps::default(),
        );
        m.max_tiles = max;
        m.insert_mode = InsertMode::Largest;
        m
    }

    #[test]
    fn largest_mode_makes_a_grid() {
        let mut m = limited(1, 0);
        for w in 1..=4 {
            m.add(w, 0, None, false);
        }
        assert_eq!(m.slot(1), Some(Rect::new(0, 0, 500, 300)));
        assert_eq!(m.slot(4), Some(Rect::new(0, 300, 500, 300)));
        assert_eq!(m.slot(2), Some(Rect::new(500, 0, 500, 300)));
        assert_eq!(m.slot(3), Some(Rect::new(500, 300, 500, 300)));
    }

    #[test]
    fn three_windows_keep_main_tile_on_left() {
        let mut m = limited(1, 0);
        for w in 1..=3 {
            m.add(w, 0, None, false);
        }
        assert_eq!(m.slot(1), Some(Rect::new(0, 0, 500, 600)));
    }

    #[test]
    fn overflow_prefers_visible_monitor_then_next_workspace_then_float() {
        let mut m = limited(2, 2);
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        let a = m.add(3, 0, None, false);
        assert_eq!(
            a,
            Added {
                loc: Loc {
                    monitor: 1,
                    workspace: 0,
                    floating: false
                },
                overflowed: true
            }
        );
        m.add(4, 1, None, false);
        let b = m.add(5, 0, None, false);
        assert_eq!(
            b.loc,
            Loc {
                monitor: 0,
                workspace: 1,
                floating: false
            }
        );
        assert!(b.overflowed && !m.is_visible(5));
        m.add(6, 0, None, false);
        m.add(7, 1, Some(1), false);
        m.add(8, 1, Some(1), false);
        let c = m.add(9, 0, None, false);
        assert!(c.loc.floating && c.overflowed);
        assert!(m.is_visible(9));
    }

    #[test]
    fn explicit_workspace_overflows_on_same_monitor() {
        let mut m = limited(2, 1);
        m.add(1, 0, Some(1), false);
        let a = m.add(2, 0, Some(1), false);
        assert_eq!(
            a.loc,
            Loc {
                monitor: 0,
                workspace: 0,
                floating: false
            }
        );
    }

    #[test]
    fn full_workspace_refuses_moves_and_unfloat() {
        let mut m = limited(2, 1);
        m.add(1, 0, None, false);
        m.add(2, 1, None, false);
        assert!(!m.move_to(1, 1, 0)); // monitor 1 workspace 1 is full
        m.add(3, 0, Some(1), false);
        assert!(m.toggle_float(3));
        assert!(m.move_to(3, 0, 0)); // floating windows can go anywhere
        assert!(!m.set_floating(3, false)); // ...but can't tile into a full workspace
        assert!(m.swap_windows(1, 2)); // swaps never change counts
    }

    /// Two monitors: "EXT" (index 0) and the laptop "LAP" (index 1), max 3 tiles, stack mode.
    fn laptop_setup() -> Model<u32> {
        let mut m = Model::new(
            vec![
                ("EXT".into(), Rect::new(0, 0, 2000, 1000)),
                ("LAP".into(), Rect::new(2000, 0, 1000, 600)),
            ],
            2,
            Gaps::default(),
        );
        m.max_tiles = 3;
        m.insert_mode = InsertMode::Largest;
        m.overflow_mode = OverflowMode::Stack;
        m.stack_monitor = Some("LAP".into());
        m
    }

    #[test]
    fn overflow_goes_to_laptop_and_stacks_there() {
        let mut m = laptop_setup();
        for w in 1..=3 {
            m.add(w, 0, None, false);
        }
        assert!(!m.is_stacked(0, 0));
        // External is full: 4th..6th tile on the laptop (it has room for 3).
        for w in 4..=6 {
            let a = m.add(w, 0, None, false);
            assert_eq!((a.loc.monitor, a.overflowed), (1, true));
        }
        assert!(!m.is_stacked(1, 0));
        // 7th: laptop goes to a stack; every laptop window is full screen.
        m.add(7, 0, None, false);
        assert!(m.is_stacked(1, 0));
        for w in 4..=7 {
            assert_eq!(m.slot(w), Some(Rect::new(2000, 0, 1000, 600)));
        }
        // External still tiles.
        assert_eq!(m.slot(1).unwrap().w, 1000);
    }

    #[test]
    fn stack_unstacks_into_balanced_tiles() {
        let mut m = laptop_setup();
        for w in 1..=4 {
            m.add(w, 1, None, false);
        }
        assert!(m.is_stacked(1, 0));
        m.remove(2);
        assert!(!m.is_stacked(1, 0));
        // 3 tiles: main left + two stacked right.
        assert_eq!(m.slot(1), Some(Rect::new(2000, 0, 500, 600)));
        let mut right = vec![m.slot(3).unwrap(), m.slot(4).unwrap()];
        right.sort_by_key(|r| r.y);
        assert_eq!(
            right,
            vec![Rect::new(2500, 0, 500, 300), Rect::new(2500, 300, 500, 300)]
        );
    }

    #[test]
    fn up_down_cycle_through_stack_left_right_leave_it() {
        let mut m = laptop_setup();
        for w in 1..=4 {
            m.add(w, 1, None, false);
        }
        m.set_focus(4);
        let order = m.stack_order(1, 0);
        let i = order.iter().position(|w| *w == 4).unwrap();
        let n = order.len();
        assert_eq!(m.focus_dir(Direction::Down), Some(order[(i + 1) % n]));
        assert_eq!(m.focus_dir(Direction::Up), Some(4));
        assert_eq!(m.focus_dir(Direction::Up), Some(order[(i + n - 1) % n]));
        m.add(9, 0, None, false);
        m.set_focus(3);
        assert_eq!(m.focus_dir(Direction::Left), Some(9));
        // Coming back right lands on the stack's top window.
        assert_eq!(m.focus_dir(Direction::Right), Some(3));
    }

    #[test]
    fn laptop_always_accepts_moves() {
        let mut m = laptop_setup();
        for w in 1..=3 {
            m.add(w, 1, None, false);
        }
        m.add(5, 0, None, false);
        assert!(m.move_to(5, 1, 0));
        assert!(m.is_stacked(1, 0));
    }

    #[test]
    fn without_laptop_each_monitor_stacks_its_own_overflow() {
        let mut m = laptop_setup();
        m.stack_monitor = None;
        for w in 1..=4 {
            m.add(w, 1, None, false);
        }
        // 4th went to the external monitor, which had room.
        assert_eq!(m.loc(4).unwrap().monitor, 0);
        for w in 5..=8 {
            m.add(w, 0, None, false);
        }
        assert!(m.is_stacked(0, 0));
        assert_eq!(m.tile_count(0, 0), 5);
    }

    #[test]
    fn laptop_can_always_stack() {
        let mut m = laptop_setup();
        m.stack_monitor_always = true;
        m.add(1, 1, None, false);
        assert!(!m.is_stacked(1, 0)); // one window: just full screen
        m.add(2, 1, None, false);
        assert!(m.is_stacked(1, 0));
        assert_eq!(m.slot(1), m.slot(2));
        // External monitor still tiles, and overflow still lands on the laptop stack.
        for w in 3..=6 {
            m.add(w, 0, None, false);
        }
        assert!(!m.is_stacked(0, 0));
        assert_eq!(m.loc(6).unwrap().monitor, 1);
        assert_eq!(m.tile_count(1, 0), 3);
        m.remove(1);
        m.remove(2);
        assert!(!m.is_stacked(1, 0));
    }

    #[test]
    fn stack_cascades_oldest_at_back() {
        let mut m = laptop_setup();
        m.stack_monitor_always = true;
        m.stack_style = StackStyle::Cascade;
        m.cascade_offset = 40;
        for w in [7, 8, 9] {
            m.add(w, 1, None, false);
        }
        assert_eq!(m.stack_order(1, 0), vec![7, 8, 9]);
        // Laptop area 1000x600 at x=2000: steps of 40, each window 920x520.
        assert_eq!(m.slot(7), Some(Rect::new(2000, 0, 920, 520)));
        assert_eq!(m.slot(8), Some(Rect::new(2040, 40, 920, 520)));
        assert_eq!(m.slot(9), Some(Rect::new(2080, 80, 920, 520)));
        // Cycling follows the cascade order.
        m.set_focus(9);
        assert_eq!(m.focus_dir(Direction::Down), Some(7));
    }

    #[test]
    fn cascade_step_shrinks_with_many_windows() {
        let r = cascade(Rect::new(0, 0, 1000, 600), 40, 11);
        assert_eq!(r.len(), 11);
        // 600 * 0.4 / 10 = 24 px step; the front window keeps 60% of the height.
        assert_eq!(r[10], Rect::new(240, 240, 760, 360));
        assert_eq!(
            cascade(Rect::new(0, 0, 100, 100), 40, 1),
            vec![Rect::new(0, 0, 100, 100)]
        );
    }

    #[test]
    fn drag_resize_updates_ratio() {
        let mut m = one();
        m.add(1, 0, None, false);
        m.add(2, 0, None, false);
        assert!(m.drag_resize(1, Rect::new(0, 0, 300, 600)));
        assert_eq!(m.slot(2), Some(Rect::new(300, 0, 700, 600)));
    }
}
