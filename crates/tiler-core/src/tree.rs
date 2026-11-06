//! Binary space partition tree and the pure layout function.

use std::collections::HashMap;
use std::hash::Hash;

use crate::geom::{Axis, Rect};

pub const MIN_RATIO: f32 = 0.1;
pub const MAX_RATIO: f32 = 0.9;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gaps {
    /// Space between the monitor work area and the outermost windows.
    pub outer: i32,
    /// Space between adjacent windows.
    pub inner: i32,
}

/// Learned minimum sizes (w, h) for windows that refuse to shrink.
pub type MinSizes<W> = HashMap<W, (i32, i32)>;

#[derive(Clone, Debug, PartialEq)]
pub enum Node<W> {
    Leaf(W),
    Split {
        axis: Axis,
        /// Fraction of the available length given to `a`.
        ratio: f32,
        a: Box<Node<W>>,
        b: Box<Node<W>>,
    },
}

impl<W: Copy + Eq + Hash> Node<W> {
    pub fn contains(&self, w: W) -> bool {
        match self {
            Node::Leaf(x) => *x == w,
            Node::Split { a, b, .. } => a.contains(w) || b.contains(w),
        }
    }

    pub fn leaves(&self) -> Vec<W> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves(&self, out: &mut Vec<W>) {
        match self {
            Node::Leaf(w) => out.push(*w),
            Node::Split { a, b, .. } => {
                a.collect_leaves(out);
                b.collect_leaves(out);
            }
        }
    }

    pub fn first_leaf(&self) -> W {
        match self {
            Node::Leaf(w) => *w,
            Node::Split { a, .. } => a.first_leaf(),
        }
    }

    pub fn map_leaves(&mut self, f: &mut impl FnMut(W) -> W) {
        match self {
            Node::Leaf(w) => *w = f(*w),
            Node::Split { a, b, .. } => {
                a.map_leaves(f);
                b.map_leaves(f);
            }
        }
    }

    pub fn replace_leaf(&mut self, old: W, new: W) {
        self.map_leaves(&mut |w| if w == old { new } else { w });
    }

    pub fn swap_leaves(&mut self, x: W, y: W) {
        self.map_leaves(&mut |w| {
            if w == x {
                y
            } else if w == y {
                x
            } else {
                w
            }
        });
    }

    /// Split leaf `target` along `axis`; `target` stays first, `new` goes second.
    pub fn insert_at(&mut self, target: W, new: W, axis: Axis) -> bool {
        match self {
            Node::Leaf(x) if *x == target => {
                let old = Node::Leaf(*x);
                *self = Node::Split {
                    axis,
                    ratio: 0.5,
                    a: Box::new(old),
                    b: Box::new(Node::Leaf(new)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => {
                a.insert_at(target, new, axis) || b.insert_at(target, new, axis)
            }
        }
    }

    /// Walk from leaf `w` upward and call `f(ratio, w_is_in_a)` on the first
    /// ancestor split along `axis` for which `accept(w_is_in_a)` is true.
    /// Returns `Some(true)` if `f` was called, `Some(false)` if `w` was found but no
    /// split qualified, `None` if `w` is not in this subtree.
    pub fn adjust_nearest(
        &mut self,
        w: W,
        axis: Axis,
        accept: &dyn Fn(bool) -> bool,
        f: &mut dyn FnMut(&mut f32, bool),
    ) -> Option<bool> {
        match self {
            Node::Leaf(x) => (*x == w).then_some(false),
            Node::Split {
                axis: ax,
                ratio,
                a,
                b,
            } => {
                let (done, in_a) = if let Some(d) = a.adjust_nearest(w, axis, accept, f) {
                    (d, true)
                } else {
                    (b.adjust_nearest(w, axis, accept, f)?, false)
                };
                if done {
                    return Some(true);
                }
                if *ax == axis && accept(in_a) {
                    f(ratio, in_a);
                    return Some(true);
                }
                Some(false)
            }
        }
    }

    /// Move the nearest divider along `axis` that touches `w` by `delta`
    /// (positive = toward right/bottom). Returns true if anything changed.
    pub fn move_divider(&mut self, w: W, axis: Axis, delta: f32) -> bool {
        self.adjust_nearest(w, axis, &|_| true, &mut |r, _| {
            *r = (*r + delta).clamp(MIN_RATIO, MAX_RATIO)
        }) == Some(true)
    }

    /// The user resized window `w` with the mouse from `old` to `new`.
    /// Move the dividers that correspond to each edge that changed.
    /// `area` must already have the outer gap removed.
    pub fn resize_to(
        &mut self,
        w: W,
        area: Rect,
        inner: i32,
        mins: &MinSizes<W>,
        old: Rect,
        new: Rect,
    ) -> bool {
        const TOL: i32 = 3;
        let mut changed = false;
        // (edge moved?, axis, w must be in `a`?, new edge position)
        let edges = [
            ((new.x - old.x).abs() > TOL, Axis::X, false, new.x),
            (
                (new.right() - old.right()).abs() > TOL,
                Axis::X,
                true,
                new.right(),
            ),
            ((new.y - old.y).abs() > TOL, Axis::Y, false, new.y),
            (
                (new.bottom() - old.bottom()).abs() > TOL,
                Axis::Y,
                true,
                new.bottom(),
            ),
        ];
        for (moved, axis, in_a, pos) in edges {
            if moved {
                changed |= self.set_edge(w, area, inner, mins, axis, in_a, pos) == Some(true);
            }
        }
        changed
    }

    #[allow(clippy::too_many_arguments)]
    fn set_edge(
        &mut self,
        w: W,
        r: Rect,
        inner: i32,
        mins: &MinSizes<W>,
        axis: Axis,
        want_in_a: bool,
        pos: i32,
    ) -> Option<bool> {
        match self {
            Node::Leaf(x) => (*x == w).then_some(false),
            Node::Split { .. } => {
                let (ra, rb) = self.child_rects(r, inner, mins);
                let Node::Split {
                    axis: ax,
                    ratio,
                    a,
                    b,
                } = self
                else {
                    unreachable!()
                };
                let (done, in_a) =
                    if let Some(d) = a.set_edge(w, ra, inner, mins, axis, want_in_a, pos) {
                        (d, true)
                    } else {
                        (b.set_edge(w, rb, inner, mins, axis, want_in_a, pos)?, false)
                    };
                if done {
                    return Some(true);
                }
                if *ax == axis && in_a == want_in_a {
                    let avail = (r.len(axis) - inner).max(1) as f32;
                    let a_len = if want_in_a {
                        pos - r.start(axis)
                    } else {
                        pos - inner - r.start(axis)
                    };
                    *ratio = (a_len as f32 / avail).clamp(MIN_RATIO, MAX_RATIO);
                    return Some(true);
                }
                Some(false)
            }
        }
    }

    /// Minimum (w, h) this subtree needs, given learned per-window minimums.
    pub fn min_size(&self, inner: i32, mins: &MinSizes<W>) -> (i32, i32) {
        match self {
            Node::Leaf(w) => mins.get(w).copied().unwrap_or((0, 0)),
            Node::Split { axis, a, b, .. } => {
                let (aw, ah) = a.min_size(inner, mins);
                let (bw, bh) = b.min_size(inner, mins);
                match axis {
                    Axis::X => (aw + bw + inner, ah.max(bh)),
                    Axis::Y => (aw.max(bw), ah + bh + inner),
                }
            }
        }
    }

    /// Rectangles for the two children of a split occupying `r`.
    /// The ratio is honoured unless a child's minimum size forces otherwise.
    fn child_rects(&self, r: Rect, inner: i32, mins: &MinSizes<W>) -> (Rect, Rect) {
        let Node::Split { axis, ratio, a, b } = self else {
            return (r, r);
        };
        let pick = |(w, h): (i32, i32)| if *axis == Axis::X { w } else { h };
        let avail = (r.len(*axis) - inner).max(0);
        let ma = pick(a.min_size(inner, mins));
        let mb = pick(b.min_size(inner, mins));
        let mut a_len = (avail as f32 * ratio).round() as i32;
        if ma + mb <= avail {
            a_len = a_len.clamp(ma, avail - mb);
        } else if ma + mb > 0 {
            // Can't satisfy both: share the shortfall proportionally.
            a_len = ((avail as i64 * ma as i64) / (ma + mb) as i64) as i32;
        }
        let b_len = avail - a_len;
        match axis {
            Axis::X => (
                Rect::new(r.x, r.y, a_len, r.h),
                Rect::new(r.x + a_len + inner, r.y, b_len, r.h),
            ),
            Axis::Y => (
                Rect::new(r.x, r.y, r.w, a_len),
                Rect::new(r.x, r.y + a_len + inner, r.w, b_len),
            ),
        }
    }

    fn place(&self, r: Rect, inner: i32, mins: &MinSizes<W>, out: &mut Vec<(W, Rect)>) {
        match self {
            Node::Leaf(w) => out.push((*w, r)),
            Node::Split { a, b, .. } => {
                let (ra, rb) = self.child_rects(r, inner, mins);
                a.place(ra, inner, mins, out);
                b.place(rb, inner, mins, out);
            }
        }
    }
}

/// Remove `w` from an optional tree. Its sibling takes over the parent's space.
pub fn remove<W: Copy + Eq + Hash>(root: &mut Option<Node<W>>, w: W) -> bool {
    match root.take() {
        None => false,
        Some(n) => {
            let (n2, removed) = remove_inner(n, w);
            *root = n2;
            removed
        }
    }
}

fn remove_inner<W: Copy + Eq + Hash>(n: Node<W>, w: W) -> (Option<Node<W>>, bool) {
    match n {
        Node::Leaf(x) if x == w => (None, true),
        leaf @ Node::Leaf(_) => (Some(leaf), false),
        Node::Split { axis, ratio, a, b } => {
            let (a2, ra) = remove_inner(*a, w);
            if ra {
                return match a2 {
                    None => (Some(*b), true),
                    Some(a2) => (
                        Some(Node::Split {
                            axis,
                            ratio,
                            a: Box::new(a2),
                            b,
                        }),
                        true,
                    ),
                };
            }
            let a2 = a2.expect("unchanged subtree is never empty");
            let (b2, rb) = remove_inner(*b, w);
            match b2 {
                None => (Some(a2), true),
                Some(b2) => (
                    Some(Node::Split {
                        axis,
                        ratio,
                        a: Box::new(a2),
                        b: Box::new(b2),
                    }),
                    rb,
                ),
            }
        }
    }
}

/// Pure layout: tree + work area + gaps (+ learned minimum sizes) → window rects.
pub fn layout<W: Copy + Eq + Hash>(
    root: &Node<W>,
    area: Rect,
    gaps: Gaps,
    mins: &MinSizes<W>,
) -> Vec<(W, Rect)> {
    let mut out = Vec::new();
    root.place(area.inset(gaps.outer), gaps.inner, mins, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect::new(0, 0, 1000, 600);

    fn no_mins() -> MinSizes<u32> {
        HashMap::new()
    }

    fn two() -> Node<u32> {
        let mut t = Node::Leaf(1);
        t.insert_at(1, 2, Axis::X);
        t
    }

    #[test]
    fn single_window_fills_area_minus_outer_gap() {
        let t = Node::Leaf(1);
        let l = layout(
            &t,
            AREA,
            Gaps {
                outer: 10,
                inner: 10,
            },
            &no_mins(),
        );
        assert_eq!(l, vec![(1, Rect::new(10, 10, 980, 580))]);
    }

    #[test]
    fn two_windows_split_evenly_with_inner_gap() {
        let l = layout(
            &two(),
            AREA,
            Gaps {
                outer: 0,
                inner: 10,
            },
            &no_mins(),
        );
        assert_eq!(
            l,
            vec![
                (1, Rect::new(0, 0, 495, 600)),
                (2, Rect::new(505, 0, 495, 600))
            ]
        );
    }

    #[test]
    fn remove_promotes_sibling() {
        let mut t = Some(two());
        t.as_mut().unwrap().insert_at(2, 3, Axis::Y);
        assert!(remove(&mut t, 1));
        let t = t.unwrap();
        assert_eq!(t.leaves(), vec![2, 3]);
        assert!(matches!(t, Node::Split { axis: Axis::Y, .. }));
    }

    #[test]
    fn remove_last_leaves_empty() {
        let mut t = Some(Node::Leaf(1));
        assert!(remove(&mut t, 1));
        assert!(t.is_none());
        assert!(!remove(&mut t, 1));
    }

    #[test]
    fn remove_missing_is_noop() {
        let mut t = Some(two());
        assert!(!remove(&mut t, 9));
        assert_eq!(t.unwrap().leaves(), vec![1, 2]);
    }

    #[test]
    fn min_size_pushes_divider() {
        let mut mins = no_mins();
        mins.insert(2, (700, 0));
        let l = layout(&two(), AREA, Gaps::default(), &mins);
        assert_eq!(l[1].1.w, 700);
        assert_eq!(l[0].1.w, 300);
    }

    #[test]
    fn impossible_mins_share_proportionally() {
        let mut mins = no_mins();
        mins.insert(1, (800, 0));
        mins.insert(2, (800, 0));
        let l = layout(&two(), AREA, Gaps::default(), &mins);
        assert_eq!(l[0].1.w, 500);
        assert_eq!(l[1].1.w, 500);
    }

    #[test]
    fn move_divider_uses_nearest_matching_axis() {
        // 1 | (2 / 3): moving 3's X divider must change the root split.
        let mut t = two();
        t.insert_at(2, 3, Axis::Y);
        assert!(t.move_divider(3, Axis::X, 0.1));
        let Node::Split { ratio, .. } = &t else {
            panic!()
        };
        assert!((ratio - 0.6).abs() < 1e-6);
        // No Y split above 1.
        assert!(!t.move_divider(1, Axis::Y, 0.1));
    }

    #[test]
    fn move_divider_clamps() {
        let mut t = two();
        t.move_divider(1, Axis::X, 5.0);
        let Node::Split { ratio, .. } = &t else {
            panic!()
        };
        assert_eq!(*ratio, MAX_RATIO);
    }

    #[test]
    fn mouse_resize_moves_matching_divider() {
        let mut t = two();
        let mins = no_mins();
        let old = Rect::new(0, 0, 500, 600);
        let new = Rect::new(0, 0, 700, 600); // dragged right edge of window 1
        assert!(t.resize_to(1, AREA, 0, &mins, old, new));
        let l = layout(&t, AREA, Gaps::default(), &mins);
        assert_eq!(l[0].1.w, 700);

        // Drag the left edge of window 2 back to 400.
        let old2 = l[1].1;
        let new2 = Rect::from_ltrb(400, 0, 1000, 600);
        assert!(t.resize_to(2, AREA, 0, &mins, old2, new2));
        assert_eq!(layout(&t, AREA, Gaps::default(), &mins)[0].1.w, 400);
    }

    #[test]
    fn mouse_resize_on_outer_edge_does_nothing() {
        let mut t = two();
        let old = Rect::new(0, 0, 500, 600);
        let new = Rect::from_ltrb(50, 0, 500, 600); // left edge of leftmost window
        assert!(!t.resize_to(1, AREA, 0, &no_mins(), old, new));
    }

    #[test]
    fn swap_and_replace() {
        let mut t = two();
        t.swap_leaves(1, 2);
        assert_eq!(t.leaves(), vec![2, 1]);
        t.replace_leaf(1, 7);
        assert_eq!(t.leaves(), vec![2, 7]);
    }
}
