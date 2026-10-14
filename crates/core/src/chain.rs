//! Sub-pixel refinement of a chain of pixel steps, open or closed: the
//! general form of what `contour` does for black-and-white loops.
//!
//! Input: the pixel corners the chain passes through, the direction of
//! every step and, per step, how far the true edge sits past the pixel edge
//! toward the right-hand pixel (from the two coverages, see `contour`).
//! Output: one point per step plus an *elbow* at every turn, with the same
//! stair / real-corner / chamfer classification as `contour`, and for an
//! open chain the two end corners as first and last point.

use kurbo::Point;

#[derive(Clone, Debug)]
pub struct Chain {
    pub pts: Vec<Point>,
    pub dirs: Vec<u8>,
    pub is_corner: Vec<bool>,
    pub fit_ok: Vec<bool>,
    pub closed: bool,
}

/// What the turns of a step sequence mean, indexed by the step before each
/// turn. See `contour` for the rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Turns {
    pub genuine: Vec<bool>,
    pub skip: Vec<bool>,
    pub chamfer_after: Vec<bool>,
    pub run_len: Vec<usize>,
}

/// A run of this many steps is a straight edge, not a stair.
pub const LONG: usize = 3;

/// Runs of equal direction as `(start, dir, len)`. Cyclic chains start at a
/// turn; open chains at step 0.
fn runs_of(dirs: &[u8], closed: bool) -> Vec<(usize, u8, usize)> {
    let n = dirs.len();
    let mut runs = Vec::new();
    if n == 0 {
        return runs;
    }
    let first = if closed {
        match (0..n).find(|&k| dirs[k] != dirs[(k + n - 1) % n]) {
            Some(f) => f,
            None => return vec![(0, dirs[0], n)],
        }
    } else {
        0
    };
    let mut k = first;
    let mut seen = 0;
    while seen < n {
        let d = dirs[k];
        let start = k;
        let mut len = 0;
        while seen < n && dirs[k] == d {
            len += 1;
            seen += 1;
            k = (k + 1) % n;
            if !closed && k == 0 {
                break;
            }
        }
        runs.push((start, d, len));
        if closed && k == first {
            break;
        }
    }
    runs
}

pub fn classify_turns(dirs: &[u8], closed: bool) -> Turns {
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
    let runs = runs_of(dirs, closed);
    let q = runs.len();
    for &(start, _, len) in &runs {
        for j in 0..len {
            t.run_len[(start + j) % n] = len;
        }
    }
    // Run `i + k` in the chain, or `None` past the ends of an open chain.
    let at = |i: usize, k: isize| -> Option<(usize, u8, usize)> {
        let j = i as isize + k;
        if closed {
            Some(runs[j.rem_euclid(q as isize) as usize])
        } else if j >= 0 && (j as usize) < q {
            Some(runs[j as usize])
        } else {
            None
        }
    };
    for i in 0..q {
        let (start_a, d_a, len_a) = runs[i];
        let Some((start_b, d_b, len_b)) = at(i, 1) else { continue };
        let c = at(i, 2);
        let d = at(i, 3);
        let before = at(i, -1);
        let last_a = (start_a + len_a - 1) % n;
        // Runs must be at least `LONG` steps: a smooth curve passing through
        // 45° makes runs of one and two, and a chamfer or a real corner made
        // of such runs would be a false corner off the curve.
        if let (Some((start_c, d_c, len_c)), Some((_, d_d, len_d))) = (c, d) {
            if len_b == 1 && len_c == 1 && d_b == d_d && d_c == d_a && len_a >= LONG && len_d >= LONG {
                t.skip[start_b] = true;
                t.skip[start_c] = true;
                t.chamfer_after[last_a] = true;
                t.genuine[last_a] = true;
                continue;
            }
        }
        let u_after = c.map_or(false, |(_, d_c, len_c)| d_c == (d_a + 2) % 4 && len_a >= LONG && len_c >= LONG);
        let u_before =
            before.map_or(false, |(_, d_bef, len_bef)| d_bef == (d_b + 2) % 4 && len_b >= LONG && len_bef >= LONG);
        t.genuine[last_a] = (len_a >= LONG && len_b >= LONG) || u_after || u_before;
    }
    t
}

/// Refine a chain. `corners` are the pixel corners: for a closed chain one
/// per step (`corners[k]` starts step `k`); for an open chain one more, the
/// end corner. `shift(k)` is the signed offset of step `k`'s edge point
/// toward the right-hand pixel, in pixels.
pub fn refine(corners: &[(i32, i32)], dirs: &[u8], closed: bool, shift: impl Fn(usize) -> f64) -> Chain {
    let n = dirs.len();
    let dvec = |d: u8| crate::decompose::DIRS[d as usize];
    let mut mids = Vec::with_capacity(n);
    for k in 0..n {
        let (x, y) = corners[k];
        let (dx, dy) = dvec(dirs[k]);
        let (rx, ry) = (-dy, dx);
        let s = shift(k).clamp(-0.5, 0.5);
        mids.push(Point::new(
            x as f64 + dx as f64 * 0.5 + s * rx as f64,
            y as f64 + dy as f64 * 0.5 + s * ry as f64,
        ));
    }
    let turns = classify_turns(dirs, closed);
    let run_len = &turns.run_len;
    let back = |k: usize| -> usize { (run_len[k] - 1).min(2) };
    let edge_pos = |k: usize, d: usize, forward: bool| -> Point {
        let j = if forward { (k + d) % n } else { (k + n - d) % n };
        mids[j]
    };
    let has_next = |k: usize| closed || k + 1 < n;
    let has_prev = |k: usize| closed || k > 0;

    let cap = 2 * n + 2;
    let mut pts = Vec::with_capacity(cap);
    let mut dirs_out = Vec::with_capacity(cap);
    let mut is_corner = Vec::with_capacity(cap);
    let mut fit_ok = Vec::with_capacity(cap);
    if !closed {
        let (x, y) = corners[0];
        pts.push(Point::new(x as f64, y as f64));
        dirs_out.push(dirs[0]);
        is_corner.push(true);
        fit_ok.push(true);
    }
    for k in 0..n {
        if turns.skip[k] {
            continue;
        }
        let dk = dirs[k];
        let horizontal = dk % 2 == 0;
        let k1 = (k + 1) % n;
        let km = (k + n - 1) % n;
        let real_after = turns.chamfer_after[k] || (has_next(k) && dirs[k1] != dk && turns.genuine[k]);
        let real_before = has_prev(k)
            && ((turns.chamfer_after[(k + n - 3) % n] && turns.skip[km]) || (dirs[km] != dk && turns.genuine[km]));
        // Next to a junction the first step is biased the same way.
        let at_end = !closed && (k == 0 || k + 1 == n);
        pts.push(mids[k]);
        dirs_out.push(dk);
        is_corner.push(false);
        fit_ok.push(!(((real_after || real_before) && run_len[k] >= 3) || (at_end && run_len[k] >= 2)));
        if turns.chamfer_after[k] {
            let kd = (k + 3) % n;
            let a = edge_pos(k, back(k), false);
            let d = edge_pos(kd, back(kd), true);
            let c = if horizontal { Point::new(d.x, a.y) } else { Point::new(a.x, d.y) };
            pts.push(c);
            dirs_out.push(dirs[kd]);
            is_corner.push(true);
            fit_ok.push(true);
            continue;
        }
        if has_next(k) && dirs[k1] != dk {
            let (a, b) = if turns.genuine[k] {
                (edge_pos(k, back(k), false), edge_pos(k1, back(k1), true))
            } else {
                (mids[k], mids[k1])
            };
            let c = if horizontal { Point::new(b.x, a.y) } else { Point::new(a.x, b.y) };
            pts.push(c);
            dirs_out.push(dirs[k1]);
            is_corner.push(true);
            fit_ok.push(turns.genuine[k]);
        }
    }
    if closed {
        // The elbow at the first corner was emitted last; start there, as
        // the polygon stage always uses point 0 as a vertex.
        if is_corner.last() == Some(&true) {
            pts.rotate_right(1);
            dirs_out.rotate_right(1);
            is_corner.rotate_right(1);
            fit_ok.rotate_right(1);
        }
    } else {
        let (x, y) = corners[n];
        pts.push(Point::new(x as f64, y as f64));
        dirs_out.push(dirs[n - 1]);
        is_corner.push(true);
        fit_ok.push(true);
    }
    Chain { pts, dirs: dirs_out, is_corner, fit_ok, closed }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_chain_has_its_corners_at_the_ends() {
        // E E E then S S S: an L from (0,0) to (3,3).
        let corners = [(0, 0), (1, 0), (2, 0), (3, 0), (3, 1), (3, 2), (3, 3)];
        let dirs = [0u8, 0, 0, 1, 1, 1];
        let c = refine(&corners, &dirs, false, |_| 0.0);
        assert_eq!(c.pts[0], Point::new(0.0, 0.0));
        assert_eq!(*c.pts.last().unwrap(), Point::new(3.0, 3.0));
        // Steps + one elbow (real: both runs >= LONG) + two ends.
        assert_eq!(c.pts.len(), 6 + 1 + 2);
        let elbows: Vec<&Point> = c.pts.iter().zip(&c.is_corner).filter(|(_, &b)| b).map(|(p, _)| p).collect();
        assert_eq!(elbows.len(), 3);
        assert_eq!(*elbows[1], Point::new(3.0, 0.0));
        assert!(c.fit_ok.iter().zip(&c.is_corner).all(|(&ok, &corner)| ok || !corner));
    }

    #[test]
    fn open_runs_do_not_wrap() {
        let t = classify_turns(&[0, 0, 0, 1, 1, 1, 2, 2, 2], false);
        assert_eq!(t.run_len, vec![3; 9]);
        assert!(t.genuine[2] && t.genuine[5]);
        assert!(!t.genuine[8]);
        let t = classify_turns(&[0, 0, 0, 1, 1, 1, 2, 2, 2], true);
        assert!(t.genuine[8]);
        // Runs of two are stairs, not edges.
        let t = classify_turns(&[0, 0, 1, 1, 2, 2], false);
        assert!(!t.genuine[1] && !t.genuine[3]);
    }
}
