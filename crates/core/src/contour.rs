//! Stage 3b: sub-pixel contour from a pixel path and the coverage field.
//!
//! Every step of a pixel path separates a foreground pixel from a background
//! pixel. Where exactly the true edge crosses that pixel pair is read from
//! their two coverage values by one-dimensional area matching (the per-pair
//! case of Manson, Smith and Schaefer 2010): a pair whose darkness sums to
//! `1 + s` has its edge `s` pixels past the shared pixel edge, into the
//! lighter pixel. The result is one point per step, within half a pixel of
//! the pixel edge, accurate to a few hundredths of a pixel on straight and
//! gently curved edges.
//!
//! At every turn of the pixel path an *elbow* is added: the intersection of
//! the two shifted edge lines. The polygon stage needs those points to exist
//! (Selinger 2003 §2.2 requires a straight run one point past each segment
//! end), and on a crisp corner the elbow is exactly the true corner. On an
//! anti-aliased staircase it is not on the boundary at all, so elbows are
//! flagged `fit_ok` only where the run structure says the turn is real.

use crate::coverage::Coverage;
use crate::decompose::{PixelPath, DIRS};
use kurbo::Point;

#[derive(Clone, Debug)]
pub struct Contour {
    /// Closed: one point per pixel step plus one per turn. Foreground is on
    /// the left. Point 0 is the elbow at the pixel path's first vertex.
    pub pts: Vec<Point>,
    /// Direction index of the pixel step each point sits on (an elbow
    /// carries the direction of the step that follows it).
    pub dirs: Vec<u8>,
    /// `true` for elbows, `false` for measured midpoints.
    pub is_corner: Vec<bool>,
    /// `true` for points the curve fitter may use: every midpoint and every
    /// elbow that is a real corner rather than a stair step.
    pub fit_ok: Vec<bool>,
    pub area: f64,
}

pub fn subpixel(path: &PixelPath, cov: &Coverage) -> Contour {
    let n = path.pts.len();
    let sign = path.sign as f32;
    // Shifted midpoint of every step.
    let mut mids = Vec::with_capacity(n);
    for k in 0..n {
        let (x, y) = path.pts[k];
        let (dx, dy) = DIRS[path.dirs[k] as usize];
        let (lx, ly) = (dy, -dx);
        let (rx, ry) = (-dy, dx);
        let cl = cov.get(x + (dx + lx - 1) / 2, y + (dy + ly - 1) / 2);
        let cr = cov.get(x + (dx + rx - 1) / 2, y + (dy + ry - 1) / 2);
        // Positive = toward the right-hand (background at trace time) pixel.
        let shift = ((cl + cr - 1.0) * sign).clamp(-0.5, 0.5) as f64;
        let mx = x as f64 + dx as f64 * 0.5;
        let my = y as f64 + dy as f64 * 0.5;
        mids.push(Point::new(mx + shift * rx as f64, my + shift * ry as f64));
    }
    let turns = classify_turns(&path.dirs);

    // Next to a real corner the pixel pair of a step is cut by the other
    // edge too, so its shift is biased. Read the edge position from the
    // step `back` steps into the run instead, and keep the biased midpoints
    // out of the curve fit when the run has enough others.
    let run_len = &turns.run_len;
    let back = |k: usize| -> usize { (run_len[k] - 1).min(2) };
    let edge_pos = |k: usize, d: usize, forward: bool| -> Point {
        let j = if forward { (k + d) % n } else { (k + n - d) % n };
        mids[j]
    };

    let mut pts = Vec::with_capacity(2 * n);
    let mut dirs = Vec::with_capacity(2 * n);
    let mut is_corner = Vec::with_capacity(2 * n);
    let mut fit_ok = Vec::with_capacity(2 * n);
    for k in 0..n {
        if turns.skip[k] {
            continue;
        }
        let dk = path.dirs[k];
        let horizontal = dk % 2 == 0;
        let k1 = (k + 1) % n;
        let km = (k + n - 1) % n;
        let real_after = turns.chamfer_after[k] || (path.dirs[k1] != dk && turns.genuine[k]);
        let real_before = turns.chamfer_after[(k + n - 3) % n] && turns.skip[km]
            || (path.dirs[km] != dk && turns.genuine[km]);
        pts.push(mids[k]);
        dirs.push(dk);
        is_corner.push(false);
        fit_ok.push(!((real_after || real_before) && run_len[k] >= 3));
        if turns.chamfer_after[k] {
            // The two single steps after k are a chamfered corner; the true
            // corner is where the edge before and the edge after it meet.
            let kd = (k + 3) % n;
            let a = edge_pos(k, back(k), false);
            let d = edge_pos(kd, back(kd), true);
            let c = if horizontal { Point::new(d.x, a.y) } else { Point::new(a.x, d.y) };
            pts.push(c);
            dirs.push(path.dirs[kd]);
            is_corner.push(true);
            fit_ok.push(true);
            continue;
        }
        if path.dirs[k1] != dk {
            let (a, b) = if turns.genuine[k] {
                (edge_pos(k, back(k), false), edge_pos(k1, back(k1), true))
            } else {
                (mids[k], mids[k1])
            };
            let c = if horizontal { Point::new(b.x, a.y) } else { Point::new(a.x, b.y) };
            pts.push(c);
            dirs.push(path.dirs[k1]);
            is_corner.push(true);
            fit_ok.push(turns.genuine[k]);
        }
    }
    // The pixel path always turns at its first vertex (it arrives heading
    // west and leaves heading south), so the last point above is the elbow
    // at that vertex. Start the loop there: the polygon stage always uses
    // point 0 as a vertex, and a corner is free while a midpoint is not.
    if path.sign < 0 {
        // Holes were walked with the hole on the left; turn them around so
        // every loop has the dark side on its left and nonzero filling works.
        // Reversing also moves that elbow to the front.
        pts.reverse();
        dirs.reverse();
        is_corner.reverse();
        fit_ok.reverse();
        for d in dirs.iter_mut() {
            *d = (*d + 2) % 4;
        }
    } else {
        pts.rotate_right(1);
        dirs.rotate_right(1);
        is_corner.rotate_right(1);
        fit_ok.rotate_right(1);
    }
    Contour { pts, dirs, is_corner, fit_ok, area: path.area }
}

/// What the turns of a pixel path mean, indexed by the step before each turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Turns {
    /// The turn after step `k` is a real corner, not a stair step.
    genuine: Vec<bool>,
    /// Step `k` is one of the two single steps of a chamfered corner and is
    /// left out of the contour.
    skip: Vec<bool>,
    /// Step `k` is the last step before a chamfered corner: emit the corner
    /// where its edge meets the edge three steps on.
    chamfer_after: Vec<bool>,
    /// Length of the run step `k` belongs to.
    run_len: Vec<usize>,
}

/// Group the steps into runs of equal direction and classify every turn.
///
/// The turn between runs A and B is real when both runs are at least two
/// steps long (two edges meet at an angle), or when B is a single step
/// between two longer runs that head in opposite directions (a U-turn: the
/// end of a thin line). Single steps between runs of the same direction are
/// stairs on a sloped edge.
///
/// A *chamfer* is the pattern A, b, c, D with b and c single steps, b
/// parallel to D and c parallel to A: an anti-aliased 90° corner whose
/// corner pixel fell under the threshold. Its two steps are dropped and the
/// corner is put back where the edges A and D meet.
fn classify_turns(dirs: &[u8]) -> Turns {
    let n = dirs.len();
    let mut t = Turns {
        genuine: vec![false; n],
        skip: vec![false; n],
        chamfer_after: vec![false; n],
        run_len: vec![1; n],
    };
    if n == 0 {
        return t;
    }
    // Runs as (start index, direction, length), cyclic. Start at a turn.
    let first = (0..n).find(|&k| dirs[k] != dirs[(k + n - 1) % n]);
    let Some(first) = first else {
        return t; // one direction only: cannot happen for a closed path
    };
    let mut runs: Vec<(usize, u8, usize)> = Vec::new();
    let mut k = first;
    loop {
        let d = dirs[k];
        let mut len = 0;
        let start = k;
        while dirs[k] == d && len < n {
            len += 1;
            k = (k + 1) % n;
        }
        runs.push((start, d, len));
        for j in 0..len {
            t.run_len[(start + j) % n] = len;
        }
        if k == first {
            break;
        }
    }
    let q = runs.len();
    for i in 0..q {
        let (start_a, d_a, len_a) = runs[i];
        let (start_b, d_b, len_b) = runs[(i + 1) % q];
        let (start_c, d_c, len_c) = runs[(i + 2) % q];
        let (_, d_d, len_d) = runs[(i + 3) % q];
        let (_, d_before_a, len_before_a) = runs[(i + q - 1) % q];
        let last_a = (start_a + len_a - 1) % n;
        // Runs must be at least `LONG` steps; see `chain::classify_turns`.
        let long = crate::chain::LONG;
        if q >= 4 && len_b == 1 && len_c == 1 && d_b == d_d && d_c == d_a && len_a >= long && len_d >= long {
            t.skip[start_b] = true;
            t.skip[start_c] = true;
            t.chamfer_after[last_a] = true;
            t.genuine[last_a] = true;
            continue;
        }
        t.genuine[last_a] = (len_a >= long && len_b >= long)
            || (d_c == (d_a + 2) % 4 && len_a >= long && len_c >= long)
            || (d_before_a == (d_b + 2) % 4 && len_b >= long && len_before_a >= long);
    }
    t
}

/// Twice the signed area (shoelace); the sign tells the orientation.
pub fn signed_area2(pts: &[Point]) -> f64 {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bitmap::Bitmap;
    use crate::coverage;
    use crate::decompose::decompose;
    use crate::Raster;

    /// A black square whose right edge covers 30% of column 6.
    #[test]
    fn edge_moves_by_coverage() {
        let mut img = Raster::filled(10, 10, [255, 255, 255, 255]);
        for y in 2..8 {
            for x in 2..6 {
                img.set(x, y, [0, 0, 0, 255]);
            }
            img.set(6, y, [179, 179, 179, 255]); // 30% dark
        }
        let cov = coverage::darkness(&img, 128);
        let bm = coverage::binarize(&cov);
        let paths = decompose(&bm, 1);
        assert_eq!(paths.len(), 1);
        let c = subpixel(&paths[0], &cov);
        // Points on the right edge (x ≈ 6) should sit at ~6.3.
        let right: Vec<f64> = c.pts.iter().filter(|p| p.x > 5.5).map(|p| p.x).collect();
        assert!(!right.is_empty());
        for x in right {
            assert!((x - 6.3).abs() < 0.03, "x = {x}");
        }
        // Left edge is crisp: exactly 2.0.
        for p in c.pts.iter().filter(|p| p.x < 2.5) {
            assert!((p.x - 2.0).abs() < 1e-6);
        }
        // All four elbows are real corners.
        assert_eq!(c.is_corner.iter().filter(|&&b| b).count(), 4);
        assert!(c.is_corner.iter().zip(&c.fit_ok).all(|(&corner, &ok)| ok || !corner));
    }

    #[test]
    fn hole_is_reversed() {
        let mut bm = Bitmap::new(5, 5);
        for y in 0..5 {
            for x in 0..5 {
                bm.set(x, y, !(1..4).contains(&x) || !(1..4).contains(&y));
            }
        }
        let cov = Coverage { width: 5, height: 5, f: bm.bits.iter().map(|&b| b as f32).collect() };
        let paths = decompose(&bm, 1);
        assert_eq!(paths.len(), 2);
        let a = subpixel(&paths[0], &cov);
        let b = subpixel(&paths[1], &cov);
        assert!(signed_area2(&a.pts) * signed_area2(&b.pts) < 0.0, "outer and hole wind the same way");
        // Both loops start on an elbow, at the first pixel's corner.
        assert!(a.is_corner[0] && b.is_corner[0]);
        assert_eq!(a.pts[0], Point::new(0.0, 0.0));
        assert_eq!(b.pts[0], Point::new(1.0, 1.0));
        // Pixel steps plus turns: 20 + 4 and 12 + 4.
        assert_eq!(a.pts.len(), 24);
        assert_eq!(b.pts.len(), 16);
    }

    #[test]
    fn stairs_are_not_corners_but_line_ends_are() {
        // Diagonal staircase: E S E S ... closed by a long W run and N run.
        let dirs: Vec<u8> = [0, 1, 0, 1, 0, 1, 0, 1, 2, 2, 2, 2, 3, 3, 3, 3].to_vec();
        let t = classify_turns(&dirs);
        let g = &t.genuine;
        // Turns inside the staircase (after steps 0..6) are stairs.
        for k in 0..7 {
            assert!(!g[k], "step {k}");
        }
        // Long W run meets long N run: a real corner. The N run meets the
        // staircase with a single step: left to the polygon stage.
        assert!(g[11] && !g[15], "{g:?}");
        assert!(!t.skip.iter().any(|&b| b));

        // A 1 px line: E×6, S×1, W×6, N×1. Every turn is real.
        let dirs: Vec<u8> = [0, 0, 0, 0, 0, 0, 1, 2, 2, 2, 2, 2, 2, 3].to_vec();
        let t = classify_turns(&dirs);
        let g = &t.genuine;
        assert!(g[5] && g[6] && g[12] && g[13], "{g:?}");
        assert_eq!(g.iter().filter(|&&b| b).count(), 4);
    }

    #[test]
    fn chamfered_corners_are_put_back() {
        // Anti-aliased square: every corner pixel is missing, so each 90°
        // turn is S×4, E×1, S×1, E×4 and so on.
        let dirs: Vec<u8> = [1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 3, 0, 3, 3, 3, 3, 2, 3, 2, 2, 2, 2, 1, 2].to_vec();
        let t = classify_turns(&dirs);
        assert_eq!(t.skip.iter().filter(|&&b| b).count(), 8, "{t:?}");
        for k in [3usize, 9, 15, 21] {
            assert!(t.chamfer_after[k] && t.genuine[k], "step {k}: {t:?}");
        }
        for k in [4usize, 5, 10, 11, 16, 17, 22, 23] {
            assert!(t.skip[k], "step {k}");
        }

        // End to end: a 20 px square rendered with its edges at .4, so the
        // corner pixels (0.36 coverage) are white.
        let mut img = Raster::filled(30, 30, [255, 255, 255, 255]);
        for y in 4..26 {
            for x in 4..26 {
                let cx: f64 = if x == 4 { 0.6 } else if x == 25 { 0.4 } else { 1.0 };
                let cy: f64 = if y == 4 { 0.6 } else if y == 25 { 0.4 } else { 1.0 };
                let v = (255.0 * (1.0 - cx * cy)).round() as u8;
                img.set(x, y, [v, v, v, 255]);
            }
        }
        let cov = coverage::darkness(&img, 128);
        let bm = coverage::binarize(&cov);
        let paths = decompose(&bm, 1);
        assert_eq!(paths.len(), 1);
        let c = subpixel(&paths[0], &cov);
        let corners: Vec<Point> = (0..c.pts.len()).filter(|&k| c.is_corner[k]).map(|k| c.pts[k]).collect();
        assert_eq!(corners.len(), 4, "{corners:?}");
        assert!(c.is_corner[0] && c.fit_ok[0]);
        for p in &corners {
            let near = |v: f64| (v - 4.4).abs() < 0.05 || (v - 25.4).abs() < 0.05;
            assert!(near(p.x) && near(p.y), "{p:?}");
        }
        // Every usable point is either a corner or on one of the four edges;
        // the midpoints right next to a corner are biased and excluded.
        for (k, p) in c.pts.iter().enumerate() {
            let on = |v: f64| (v - 4.4).abs() < 0.05 || (v - 25.4).abs() < 0.05;
            if c.fit_ok[k] {
                assert!(on(p.x) || on(p.y), "{k}: {p:?}");
            }
        }
        assert!(c.fit_ok.iter().filter(|&&b| !b).count() >= 6);
    }
}
