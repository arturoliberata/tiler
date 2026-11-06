//! Directional neighbour lookup by rectangle geometry.

use crate::geom::{Direction, Rect};

fn overlaps(a0: i32, a1: i32, b0: i32, b1: i32) -> bool {
    a0 < b1 && b0 < a1
}

/// Best candidate in `dir` from `from`. Prefers candidates that overlap on the
/// perpendicular axis, then the smallest gap, then the closest centre.
pub fn neighbor<W: Copy>(from: Rect, cands: &[(W, Rect)], dir: Direction) -> Option<W> {
    let (fcx, fcy) = from.center();
    cands
        .iter()
        .filter_map(|&(w, r)| {
            let (cx, cy) = r.center();
            let (beyond, gap, perp, overlap) = match dir {
                Direction::Left => (
                    cx < fcx,
                    from.x - r.right(),
                    (cy - fcy).abs(),
                    overlaps(r.y, r.bottom(), from.y, from.bottom()),
                ),
                Direction::Right => (
                    cx > fcx,
                    r.x - from.right(),
                    (cy - fcy).abs(),
                    overlaps(r.y, r.bottom(), from.y, from.bottom()),
                ),
                Direction::Up => (
                    cy < fcy,
                    from.y - r.bottom(),
                    (cx - fcx).abs(),
                    overlaps(r.x, r.right(), from.x, from.right()),
                ),
                Direction::Down => (
                    cy > fcy,
                    r.y - from.bottom(),
                    (cx - fcx).abs(),
                    overlaps(r.x, r.right(), from.x, from.right()),
                ),
            };
            beyond.then_some((w, (!overlap, gap.max(0), perp)))
        })
        .min_by_key(|(_, key)| *key)
        .map(|(w, _)| w)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1 | 2
    //   | 3
    fn grid() -> Vec<(u32, Rect)> {
        vec![
            (1, Rect::new(0, 0, 500, 600)),
            (2, Rect::new(500, 0, 500, 300)),
            (3, Rect::new(500, 300, 500, 300)),
        ]
    }

    #[test]
    fn finds_neighbours() {
        let g = grid();
        let others = |w: u32| {
            g.iter()
                .copied()
                .filter(|(x, _)| *x != w)
                .collect::<Vec<_>>()
        };
        assert_eq!(neighbor(g[0].1, &others(1), Direction::Right), Some(2));
        assert_eq!(neighbor(g[2].1, &others(3), Direction::Left), Some(1));
        assert_eq!(neighbor(g[1].1, &others(2), Direction::Down), Some(3));
        assert_eq!(neighbor(g[2].1, &others(3), Direction::Up), Some(2));
        assert_eq!(neighbor(g[0].1, &others(1), Direction::Left), None);
    }

    #[test]
    fn prefers_overlap_over_distance() {
        let from = Rect::new(0, 0, 100, 100);
        let cands = [
            (1, Rect::new(500, 0, 100, 100)),
            (2, Rect::new(110, 400, 100, 100)),
        ];
        assert_eq!(neighbor(from, &cands, Direction::Right), Some(1));
    }
}
