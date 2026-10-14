//! Stage 3a: Potrace-style path decomposition of a binary bitmap
//! (Selinger 2003 §2.1, `decompose.c`).
//!
//! Vertices are pixel corners with integer coordinates, y down. Each closed
//! path is walked with the current foreground on its left. After a path is
//! found its interior is XOR-ed out of a working copy, so holes surface as
//! paths of their own (`sign = -1`) and islands inside holes come back as
//! `+1`. Ambiguous 2×2 checkerboard corners follow the `minority` turn policy.
//! Paths enclosing fewer than `min_area` pixels are dropped (`turdsize`).

use crate::bitmap::Bitmap;

/// Unit step for each direction index: E, S, W, N.
pub const DIRS: [(i32, i32); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];

#[derive(Clone, Debug)]
pub struct PixelPath {
    /// Closed loop of pixel corners; step `k` goes from `pts[k]` to `pts[(k+1) % n]`.
    pub pts: Vec<(i32, i32)>,
    /// Direction index of step `k`.
    pub dirs: Vec<u8>,
    /// `+1`: the pixels on the left are foreground in the original image.
    /// `-1`: a hole; the pixels on the left are background.
    pub sign: i8,
    /// Enclosed area in pixels.
    pub area: f64,
}

pub fn decompose(bm: &Bitmap, min_area: u32) -> Vec<PixelPath> {
    let mut work = bm.clone();
    let mut out = Vec::new();
    let w = bm.width.max(1);
    let mut scan = 0usize;
    while let Some(idx) = work.next_set(scan) {
        scan = idx;
        let (x0, y0) = ((idx % w) as i32, (idx / w) as i32);
        let sign = if bm.get(x0, y0) { 1 } else { -1 };
        let mut path = find_path(&work, x0, y0);
        xor_path(&mut work, &path);
        path.sign = sign;
        if path.area >= min_area as f64 {
            out.push(path);
        }
    }
    out
}

#[inline]
fn dir_index(dx: i32, dy: i32) -> u8 {
    match (dx, dy) {
        (1, 0) => 0,
        (0, 1) => 1,
        (-1, 0) => 2,
        _ => 3,
    }
}

/// Walk the boundary starting at the top-left corner of foreground pixel
/// `(x0, y0)`, whose upper and left neighbours are background. Foreground
/// stays on the left: facing direction `d`, left is `(dy, -dx)` in screen
/// coordinates.
fn find_path(bm: &Bitmap, x0: i32, y0: i32) -> PixelPath {
    let (mut x, mut y) = (x0, y0);
    let (mut dx, mut dy) = (0i32, 1i32);
    let mut pts = Vec::new();
    let mut dirs = Vec::new();
    let mut area2: i64 = 0;
    let limit = 4 * (bm.width + 1) * (bm.height + 1) + 8;
    loop {
        pts.push((x, y));
        dirs.push(dir_index(dx, dy));
        let (nx, ny) = (x + dx, y + dy);
        area2 += x as i64 * ny as i64 - nx as i64 * y as i64;
        x = nx;
        y = ny;
        if (x == x0 && y == y0) || pts.len() > limit {
            break;
        }
        let (lx, ly) = (dy, -dx);
        let (rx, ry) = (-dy, dx);
        let ahead_left = bm.get(x + (dx + lx - 1) / 2, y + (dy + ly - 1) / 2);
        let ahead_right = bm.get(x + (dx + rx - 1) / 2, y + (dy + ry - 1) / 2);
        let turn_right = match (ahead_left, ahead_right) {
            (true, true) => Some(true),
            (false, false) => Some(false),
            (true, false) => None,
            // Checkerboard: connect the minority colour.
            (false, true) => Some(!majority(bm, x, y)),
        };
        match turn_right {
            Some(true) => {
                let t = dx;
                dx = -dy;
                dy = t;
            }
            Some(false) => {
                let t = dx;
                dx = dy;
                dy = -t;
            }
            None => {}
        }
    }
    PixelPath { pts, dirs, sign: 1, area: (area2.abs() as f64) / 2.0 }
}

/// Potrace's `majority`: look at square rings of growing radius around the
/// vertex and return whether foreground wins the first ring that is not a tie.
fn majority(bm: &Bitmap, x: i32, y: i32) -> bool {
    for i in 2..5 {
        let mut ct = 0i32;
        for a in (-i + 1)..=(i - 1) {
            ct += if bm.get(x + a, y + i - 1) { 1 } else { -1 };
            ct += if bm.get(x + i - 1, y + a - 1) { 1 } else { -1 };
            ct += if bm.get(x + a - 1, y - i) { 1 } else { -1 };
            ct += if bm.get(x - i, y + a) { 1 } else { -1 };
        }
        if ct > 0 {
            return true;
        } else if ct < 0 {
            return false;
        }
    }
    false
}

/// Flip every pixel inside the loop: pair up the vertical crossings on each
/// pixel row and toggle the spans between them.
fn xor_path(bm: &mut Bitmap, p: &PixelPath) {
    let mut v: Vec<(i32, i32)> = Vec::with_capacity(p.pts.len() / 2);
    for (k, &(x, y)) in p.pts.iter().enumerate() {
        match p.dirs[k] {
            1 => v.push((y, x)),
            3 => v.push((y - 1, x)),
            _ => {}
        }
    }
    v.sort_unstable();
    for pair in v.chunks(2) {
        if let [(r, x1), (r2, x2)] = pair {
            debug_assert_eq!(r, r2, "odd number of crossings on a row");
            for x in *x1..*x2 {
                bm.toggle(x, *r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap(rows: &[&str]) -> Bitmap {
        let h = rows.len();
        let w = rows[0].len();
        let mut bm = Bitmap::new(w, h);
        for (y, r) in rows.iter().enumerate() {
            for (x, c) in r.chars().enumerate() {
                bm.set(x, y, c == '#');
            }
        }
        bm
    }

    #[test]
    fn single_pixel_is_a_unit_square() {
        let bm = bitmap(&["...", ".#.", "..."]);
        let p = decompose(&bm, 1);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].pts, vec![(1, 1), (1, 2), (2, 2), (2, 1)]);
        assert_eq!(p[0].dirs, vec![1, 0, 3, 2]);
        assert_eq!(p[0].area, 1.0);
        assert_eq!(p[0].sign, 1);
    }

    #[test]
    fn ring_gives_outer_and_hole() {
        let bm = bitmap(&["#####", "#...#", "#.#.#", "#...#", "#####"]);
        let p = decompose(&bm, 1);
        assert_eq!(p.len(), 3);
        assert_eq!((p[0].sign, p[0].area), (1, 25.0));
        assert_eq!((p[1].sign, p[1].area), (-1, 9.0));
        assert_eq!((p[2].sign, p[2].area), (1, 1.0));
        // Every path is closed and every step is a unit step.
        for path in &p {
            let n = path.pts.len();
            for k in 0..n {
                let (x, y) = path.pts[k];
                let (nx, ny) = path.pts[(k + 1) % n];
                let (dx, dy) = DIRS[path.dirs[k] as usize];
                assert_eq!((x + dx, y + dy), (nx, ny));
            }
        }
    }

    #[test]
    fn despeckle_drops_small_loops_but_keeps_flipping() {
        let bm = bitmap(&["####", "#.##", "####", "...#"]);
        let p = decompose(&bm, 2);
        // Outer blob (area 13) kept, the 1-pixel hole dropped.
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].area, 13.0);
    }

    #[test]
    fn checkerboard_minority_policy() {
        // Two black pixels on a diagonal in a white field: black is the
        // minority, so they are connected into one path.
        let bm = bitmap(&["....", ".#..", "..#.", "...."]);
        let p = decompose(&bm, 1);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].area, 2.0);
    }

    #[test]
    fn border_touching_shape() {
        let bm = bitmap(&["##", "##"]);
        let p = decompose(&bm, 1);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].area, 4.0);
        assert_eq!(p[0].pts.len(), 8);
    }
}
