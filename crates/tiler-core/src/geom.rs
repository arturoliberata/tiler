/// Axis-aligned rectangle in physical screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_ltrb(l: i32, t: i32, r: i32, b: i32) -> Self {
        Self::new(l, t, r - l, b - t)
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }

    pub fn contains_point(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    /// Shrink by `d` on every side (never below zero size).
    pub fn inset(&self, d: i32) -> Rect {
        Rect::new(
            self.x + d,
            self.y + d,
            (self.w - 2 * d).max(0),
            (self.h - 2 * d).max(0),
        )
    }

    pub fn start(&self, axis: Axis) -> i32 {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }

    pub fn len(&self, axis: Axis) -> i32 {
        match axis {
            Axis::X => self.w,
            Axis::Y => self.h,
        }
    }

    /// True if every edge is within `tol` pixels of `other`.
    pub fn approx_eq(&self, other: &Rect, tol: i32) -> bool {
        (self.x - other.x).abs() <= tol
            && (self.y - other.y).abs() <= tol
            && (self.right() - other.right()).abs() <= tol
            && (self.bottom() - other.bottom()).abs() <= tol
    }
}

/// The axis a split divides along.
/// `X`: children are side by side (left | right). `Y`: stacked (top / bottom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::X,
            Direction::Up | Direction::Down => Axis::Y,
        }
    }

    /// Right and Down point toward increasing coordinates.
    pub fn is_forward(self) -> bool {
        matches!(self, Direction::Right | Direction::Down)
    }

    pub fn parse(s: &str) -> Option<Direction> {
        match s.to_ascii_lowercase().as_str() {
            "left" | "l" | "h" => Some(Direction::Left),
            "right" | "r" => Some(Direction::Right),
            "up" | "u" | "k" => Some(Direction::Up),
            "down" | "d" | "j" => Some(Direction::Down),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inset_never_negative() {
        assert_eq!(Rect::new(0, 0, 10, 10).inset(8), Rect::new(8, 8, 0, 0));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains_point(0, 0));
        assert!(!r.contains_point(10, 5));
    }
}
