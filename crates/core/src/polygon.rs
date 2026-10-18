//! Stage 5: polygon from a closed sub-pixel contour. Potrace's optimal
//! polygon (Selinger 2003 §2.2, `trace.c`) generalised to real coordinates.
//!
//! 1. For each start point find how far a "straight" run extends: a run is
//!    straight if some line passes within `tol` (L∞) of every point. Potrace's
//!    incremental cone test is used with the box half-width scaled to `tol`.
//!    A run that uses all four pixel step directions is never straight; that
//!    rule is what keeps a 1 px line from collapsing onto its own axis.
//! 2. Shortest cycle through straight runs, ties broken by the summed
//!    least-squares deviation of the points from each segment (`penalty3`).
//! 3. Each polygon vertex moves to the point inside a `vbox` box around it
//!    that is closest to the two neighbouring least-squares lines. That
//!    intersection is where a chamfered corner actually was (Gong and
//!    Newman 2018 recover corners the same way).

use kurbo::{Point, Vec2};

#[derive(Clone, Copy, Default, Debug)]
struct Sum {
    x: f64,
    y: f64,
    x2: f64,
    xy: f64,
    y2: f64,
}

/// Polygon for a closed contour: adjusted vertex positions and, for each,
/// the index of the contour point it came from. `tol` is the per-point
/// straightness tolerance in pixels (Potrace: 0.5), `vbox` the half-width of
/// the square a vertex may be moved in during adjustment (Potrace: 0.5).
pub fn polygonize(pts: &[Point], dirs: &[u8], tol: f64, vbox: f64) -> (Vec<Point>, Vec<usize>) {
    let n = pts.len();
    if n < 3 || dirs.len() != n {
        return (pts.to_vec(), (0..n).collect());
    }
    let sums = calc_sums(pts);
    let lon = calc_lon(pts, dirs, 2.0 * tol);
    let mut po = best_polygon(pts, &sums, &lon);
    merge_cut_corners(&mut po, pts, &sums, MERGE_CHORD, MERGE_NEAR);
    let verts = adjust_vertices(pts, &sums, &po, vbox);
    (verts, po)
}

/// Polygon for an open chain with both ends pinned: adjusted vertex
/// positions (first and last are `pts[0]` and `pts[n-1]`) and the contour
/// index of each.
pub fn polygonize_open(pts: &[Point], dirs: &[u8], tol: f64, vbox: f64) -> (Vec<Point>, Vec<usize>) {
    let n = pts.len();
    if n < 3 || dirs.len() != n {
        return (pts.to_vec(), (0..n).collect());
    }
    let sums = calc_sums(pts);
    let lon = calc_lon_open(pts, dirs, 2.0 * tol);
    let mut po = best_polygon_open(pts, &sums, &lon);
    merge_cut_corners_open(&mut po, pts, &sums, MERGE_CHORD, MERGE_NEAR);
    let verts = adjust_vertices_open(pts, &sums, &po, vbox);
    (verts, po)
}

/// Open-chain `lon`: furthest `k` such that `i..=k` is straight, no wrap.
fn calc_lon_open(pts: &[Point], dirs: &[u8], h: f64) -> Vec<usize> {
    let n = pts.len();
    let mut lon = vec![0usize; n];
    for i in 0..n {
        let mut ct = [0u32; 4];
        ct[(dirs[i] & 3) as usize] += 1;
        let (mut c0, mut c1) = (Vec2::ZERO, Vec2::ZERO);
        let mut last = i;
        for k in i + 1..n {
            let forced = k == i + 1;
            ct[(dirs[k] & 3) as usize] += 1;
            if !forced && ct.iter().all(|&c| c > 0) {
                break;
            }
            let cur = pts[k] - pts[i];
            if !forced && (c0.cross(cur) < 0.0 || c1.cross(cur) > 0.0) {
                break;
            }
            if cur.x.abs() > h || cur.y.abs() > h {
                let off = Vec2::new(
                    cur.x + if cur.y >= 0.0 && (cur.y > 0.0 || cur.x < 0.0) { h } else { -h },
                    cur.y + if cur.x <= 0.0 && (cur.x < 0.0 || cur.y < 0.0) { h } else { -h },
                );
                if c0.cross(off) >= 0.0 {
                    c0 = off;
                }
                let off = Vec2::new(
                    cur.x + if cur.y <= 0.0 && (cur.y < 0.0 || cur.x < 0.0) { h } else { -h },
                    cur.y + if cur.x >= 0.0 && (cur.x > 0.0 || cur.y < 0.0) { h } else { -h },
                );
                if c1.cross(off) <= 0.0 {
                    c1 = off;
                }
            }
            last = k;
        }
        lon[i] = last;
    }
    lon
}

/// Fewest segments from 0 to n-1 over straight runs, then least penalty.
fn best_polygon_open(pts: &[Point], sums: &[Sum], lon: &[usize]) -> Vec<usize> {
    let n = pts.len();
    let mut best: Vec<(usize, f64)> = vec![(usize::MAX, f64::MAX); n];
    let mut prev = vec![0usize; n];
    best[0] = (0, 0.0);
    for j in 1..n {
        for i in (0..j).rev() {
            if lon[i] < j || best[i].0 == usize::MAX {
                continue;
            }
            let cand = (best[i].0 + 1, best[i].1 + penalty3(pts, sums, i, j));
            if cand.0 < best[j].0 || (cand.0 == best[j].0 && cand.1 < best[j].1) {
                best[j] = cand;
                prev[j] = i;
            }
        }
    }
    let mut po = vec![n - 1];
    let mut j = n - 1;
    while j > 0 {
        j = prev[j];
        po.push(j);
    }
    po.reverse();
    po
}

/// `merge_cut_corners` for an open polygon: never touches the end vertices.
fn merge_cut_corners_open(po: &mut Vec<usize>, pts: &[Point], sums: &[Sum], max_chord: f64, near: f64) {
    let n = pts.len();
    let origin = pts[0];
    loop {
        let m = po.len();
        if m <= 3 {
            return;
        }
        let mut merged = false;
        for i in 1..m - 2 {
            let (a, b) = (po[i], po[i + 1]);
            if (pts[b] - pts[a]).hypot() >= max_chord {
                continue;
            }
            let (p, q) = (po[i - 1], po[i + 2]);
            let (ca, da) = pointslope(sums, n, p, a);
            let (cb, db) = pointslope(sums, n, b, q);
            let det = da.cross(db);
            if det.abs() < 1e-9 {
                continue;
            }
            let t = (cb - ca).cross(db) / det;
            let x = origin + ca + da * t;
            let mid = pts[a].midpoint(pts[b]);
            if (x - mid).hypot() <= near {
                po.remove(i + 1);
                merged = true;
                break;
            }
        }
        if !merged {
            return;
        }
    }
}

/// `adjust_vertices` for an open polygon: the ends stay where they are.
fn adjust_vertices_open(pts: &[Point], sums: &[Sum], po: &[usize], vbox: f64) -> Vec<Point> {
    let m = po.len();
    let n = pts.len();
    let origin = pts[0];
    let mut out = Vec::with_capacity(m);
    if m == 0 {
        return out;
    }
    let mut q = vec![[[0.0f64; 3]; 3]; m.saturating_sub(1)];
    for i in 0..m.saturating_sub(1) {
        let (ctr, dir) = pointslope(sums, n, po[i], po[i + 1]);
        let d = dir.x * dir.x + dir.y * dir.y;
        if d != 0.0 {
            let v = [dir.y, -dir.x, dir.x * ctr.y - dir.y * ctr.x];
            for l in 0..3 {
                for k in 0..3 {
                    q[i][l][k] = v[l] * v[k] / d;
                }
            }
        }
    }
    out.push(pts[po[0]]);
    for i in 1..m.saturating_sub(1) {
        let s = pts[po[i]] - origin;
        let mut qq: Quad = [[0.0; 3]; 3];
        for l in 0..3 {
            for k in 0..3 {
                qq[l][k] = q[i - 1][l][k] + q[i][l][k];
            }
        }
        out.push(origin + minimise_in_box(&mut qq, s, vbox));
    }
    if m > 1 {
        out.push(pts[po[m - 1]]);
    }
    out
}

/// A polygon edge shorter than this may be a cut corner, px.
pub const MERGE_CHORD: f64 = 3.0;
/// ... when the two neighbouring edge lines meet within this distance of
/// its midpoint, px.
pub const MERGE_NEAR: f64 = 1.5;

/// An anti-aliased corner has no measured point on it, so the shortest
/// polygon puts a vertex on the last point of one edge and another on the
/// first point of the next: a short edge cutting the corner. When the lines
/// of the two edges around such a short edge meet near it, drop its second
/// vertex; adjustment then moves the first to that intersection. The flat
/// end of a narrow bar is a short edge too, but its neighbours are parallel
/// and stay.
fn merge_cut_corners(po: &mut Vec<usize>, pts: &[Point], sums: &[Sum], max_chord: f64, near: f64) {
    let n = pts.len();
    let origin = pts[0];
    let line = |i: usize, j: usize| -> (Vec2, Vec2) {
        let j = (j + n - i) % n + i;
        pointslope(sums, n, i, j)
    };
    loop {
        let m = po.len();
        if m <= 3 {
            return;
        }
        let mut merged = false;
        for i in 0..m {
            let (a, b) = (po[i], po[(i + 1) % m]);
            if (pts[b] - pts[a]).hypot() >= max_chord {
                continue;
            }
            let (p, q) = (po[(i + m - 1) % m], po[(i + 2) % m]);
            let (ca, da) = line(p, a);
            let (cb, db) = line(b, q);
            let det = da.cross(db);
            if det.abs() < 1e-9 {
                continue;
            }
            let t = (cb - ca).cross(db) / det;
            let x = origin + ca + da * t;
            let mid = pts[a].midpoint(pts[b]);
            if (x - mid).hypot() <= near {
                po.remove((i + 1) % m);
                merged = true;
                break;
            }
        }
        if !merged {
            return;
        }
    }
}

/// Prefix sums relative to `pts[0]`, `n + 1` entries.
fn calc_sums(pts: &[Point]) -> Vec<Sum> {
    let o = pts[0];
    let mut s = Vec::with_capacity(pts.len() + 1);
    s.push(Sum::default());
    let mut acc = Sum::default();
    for p in pts {
        let (x, y) = (p.x - o.x, p.y - o.y);
        acc.x += x;
        acc.y += y;
        acc.x2 += x * x;
        acc.xy += x * y;
        acc.y2 += y * y;
        s.push(acc);
    }
    s
}

/// `a <= b < c` cyclically.
#[inline]
fn cyclic(a: usize, b: usize, c: usize) -> bool {
    if a <= c {
        a <= b && b < c
    } else {
        a <= b || b < c
    }
}

/// `lon[i]`: furthest point such that `i..=lon[i]` is a straight run, made
/// consistent across starting points the way Potrace does.
///
/// Every run reaches at least the next point, so a segment from `i` to
/// `i + 1` is always admissible and the polygon always exists.
fn calc_lon(pts: &[Point], dirs: &[u8], h: f64) -> Vec<usize> {
    let n = pts.len();
    let mut pivk = vec![0usize; n];
    for i in 0..n {
        let mut ct = [0u32; 4];
        ct[(dirs[i] & 3) as usize] += 1;
        let (mut c0, mut c1) = (Vec2::ZERO, Vec2::ZERO);
        let mut last = i;
        let mut k = (i + 1) % n;
        let mut steps = 0usize;
        while k != i {
            steps += 1;
            let forced = steps <= 1;
            ct[(dirs[k] & 3) as usize] += 1;
            if !forced && ct.iter().all(|&c| c > 0) {
                break;
            }
            let cur = pts[k] - pts[i];
            if !forced && (c0.cross(cur) < 0.0 || c1.cross(cur) > 0.0) {
                break;
            }
            if cur.x.abs() > h || cur.y.abs() > h {
                let off = Vec2::new(
                    cur.x + if cur.y >= 0.0 && (cur.y > 0.0 || cur.x < 0.0) { h } else { -h },
                    cur.y + if cur.x <= 0.0 && (cur.x < 0.0 || cur.y < 0.0) { h } else { -h },
                );
                if c0.cross(off) >= 0.0 {
                    c0 = off;
                }
                let off = Vec2::new(
                    cur.x + if cur.y <= 0.0 && (cur.y < 0.0 || cur.x < 0.0) { h } else { -h },
                    cur.y + if cur.x >= 0.0 && (cur.x > 0.0 || cur.y < 0.0) { h } else { -h },
                );
                if c1.cross(off) <= 0.0 {
                    c1 = off;
                }
            }
            last = k;
            k = (k + 1) % n;
        }
        pivk[i] = last;
    }
    // Clean up: lon[i] is the largest k such that for all i' in [i, k),
    // i' < k <= pivk[i'].
    let mut lon = vec![0usize; n];
    let mut j = pivk[n - 1];
    lon[n - 1] = j;
    for i in (0..n - 1).rev() {
        if cyclic(i + 1, pivk[i], j) {
            j = pivk[i];
        }
        lon[i] = j;
    }
    let mut i = n - 1;
    loop {
        if !cyclic((i + 1) % n, j, lon[i]) {
            break;
        }
        lon[i] = j;
        if i == 0 {
            break;
        }
        i -= 1;
    }
    lon
}

/// Potrace's `penalty3`: length of segment `i..j` times the standard deviation
/// of the points' distances from it, from the prefix sums.
fn penalty3(pts: &[Point], sums: &[Sum], i: usize, j: usize) -> f64 {
    let n = pts.len();
    let (jj, r) = if j >= n { (j - n, 1.0) } else { (j, 0.0) };
    let sn = sums[n];
    let x = sums[jj + 1].x - sums[i].x + r * sn.x;
    let y = sums[jj + 1].y - sums[i].y + r * sn.y;
    let x2 = sums[jj + 1].x2 - sums[i].x2 + r * sn.x2;
    let xy = sums[jj + 1].xy - sums[i].xy + r * sn.xy;
    let y2 = sums[jj + 1].y2 - sums[i].y2 + r * sn.y2;
    let k = (jj as f64 + 1.0 - i as f64) + r * n as f64;
    let px = (pts[i].x + pts[jj].x) / 2.0 - pts[0].x;
    let py = (pts[i].y + pts[jj].y) / 2.0 - pts[0].y;
    let ey = pts[jj].x - pts[i].x;
    let ex = -(pts[jj].y - pts[i].y);
    let a = (x2 - 2.0 * x * px) / k + px * px;
    let b = (xy - x * py - y * px) / k + px * py;
    let c = (y2 - 2.0 * y * py) / k + py * py;
    let s = ex * ex * a + 2.0 * ex * ey * b + ey * ey * c;
    s.max(0.0).sqrt()
}

/// Potrace's `bestpolygon`: fewest segments, then least penalty.
///
/// One difference from Potrace: a segment `i → j` is admissible when
/// `i..=j` itself is straight. Potrace asks for `i-1..=j+1`, which on pixel
/// corners costs nothing (the corner is a path vertex) but on a sub-pixel
/// contour forbids a vertex on an anti-aliased corner, where the points on
/// the two sides never share a line.
fn best_polygon(pts: &[Point], sums: &[Sum], lon: &[usize]) -> Vec<usize> {
    let n = pts.len();
    let mut clip0 = vec![0usize; n];
    for i in 0..n {
        let mut c = lon[i];
        if c == i {
            c = (i + 1) % n;
        }
        clip0[i] = if c < i { n } else { c };
    }
    let mut clip1 = vec![0usize; n + 1];
    let mut j = 1;
    for (i, &c0) in clip0.iter().enumerate() {
        while j <= c0 {
            clip1[j] = i;
            j += 1;
        }
    }
    let mut seg0 = Vec::with_capacity(n + 1);
    let mut i = 0;
    while i < n {
        seg0.push(i);
        i = clip0[i].max(i + 1);
    }
    seg0.push(n);
    let m = seg0.len() - 1;
    let mut seg1 = vec![0usize; m + 1];
    let mut i = n;
    for j in (1..=m).rev() {
        seg1[j] = i;
        i = clip1[i];
    }
    seg1[0] = 0;

    let mut pen = vec![0.0f64; n + 1];
    let mut prev = vec![0usize; n + 1];
    for j in 1..=m {
        for i in seg1[j]..=seg0[j] {
            let mut best = -1.0;
            for k in (clip1[i]..=seg0[j - 1]).rev() {
                let thispen = penalty3(pts, sums, k, i) + pen[k];
                if best < 0.0 || thispen < best {
                    prev[i] = k;
                    best = thispen;
                }
            }
            if best < 0.0 {
                // Should not happen; keep the chain valid.
                prev[i] = i.saturating_sub(1);
                best = f64::MAX / 4.0;
            }
            pen[i] = best;
        }
    }
    let mut po = vec![0usize; m];
    let mut i = n;
    let mut j = m;
    while i > 0 && j > 0 {
        i = prev[i];
        j -= 1;
        po[j] = i;
    }
    po
}

/// Least-squares line through points `i..=j` (`j` may exceed `n` to wrap):
/// centre and unit direction, relative to `pts[0]`.
fn pointslope(sums: &[Sum], n: usize, i: usize, j: usize) -> (Vec2, Vec2) {
    let mut j = j;
    let mut r = 0usize;
    while j >= n {
        j -= n;
        r += 1;
    }
    let rf = r as f64;
    let sn = sums[n];
    let x = sums[j + 1].x - sums[i].x + rf * sn.x;
    let y = sums[j + 1].y - sums[i].y + rf * sn.y;
    let x2 = sums[j + 1].x2 - sums[i].x2 + rf * sn.x2;
    let xy = sums[j + 1].xy - sums[i].xy + rf * sn.xy;
    let y2 = sums[j + 1].y2 - sums[i].y2 + rf * sn.y2;
    let k = (j as f64 + 1.0 - i as f64) + rf * n as f64;
    let ctr = Vec2::new(x / k, y / k);
    let a = (x2 - x * x / k) / k;
    let b = (xy - x * y / k) / k;
    let c = (y2 - y * y / k) / k;
    let lambda2 = (a + c + ((a - c) * (a - c) + 4.0 * b * b).sqrt()) / 2.0;
    let (a, c) = (a - lambda2, c - lambda2);
    let dir = if a.abs() >= c.abs() {
        let l = (a * a + b * b).sqrt();
        if l != 0.0 { Vec2::new(-b / l, a / l) } else { Vec2::ZERO }
    } else {
        let l = (c * c + b * b).sqrt();
        if l != 0.0 { Vec2::new(-c / l, b / l) } else { Vec2::ZERO }
    };
    (ctr, dir)
}

type Quad = [[f64; 3]; 3];

#[inline]
fn quadform(q: &Quad, w: Vec2) -> f64 {
    let v = [w.x, w.y, 1.0];
    let mut s = 0.0;
    for l in 0..3 {
        for k in 0..3 {
            s += q[l][k] * v[l] * v[k];
        }
    }
    s
}

/// Potrace's `adjust_vertices` with a configurable box.
fn adjust_vertices(pts: &[Point], sums: &[Sum], po: &[usize], vbox: f64) -> Vec<Point> {
    let m = po.len();
    let n = pts.len();
    if m == 0 {
        return Vec::new();
    }
    let origin = pts[0];
    let mut q = vec![[[0.0f64; 3]; 3]; m];
    for i in 0..m {
        let j = po[(i + 1) % m];
        let j = (j + n - po[i]) % n + po[i];
        let (ctr, dir) = pointslope(sums, n, po[i], j);
        let d = dir.x * dir.x + dir.y * dir.y;
        if d != 0.0 {
            let v = [dir.y, -dir.x, dir.x * ctr.y - dir.y * ctr.x];
            for l in 0..3 {
                for k in 0..3 {
                    q[i][l][k] = v[l] * v[k] / d;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(m);
    for i in 0..m {
        let s = pts[po[i]] - origin;
        let j = (i + m - 1) % m;
        let mut qq: Quad = [[0.0; 3]; 3];
        for l in 0..3 {
            for k in 0..3 {
                qq[l][k] = q[j][l][k] + q[i][l][k];
            }
        }
        out.push(origin + minimise_in_box(&mut qq, s, vbox));
    }
    out
}

/// Minimise the quadratic form `qq` (sum of two line distance forms) near
/// `s`, inside the box of half-width `vbox` around it. If the lines are
/// parallel, an orthogonal axis through `s` is added until the system is
/// regular; if the minimum falls outside the box, its edges and corners are
/// searched.
fn minimise_in_box(qq: &mut Quad, s: Vec2, vbox: f64) -> Vec2 {
    let mut w = s;
    for _ in 0..4 {
        let det = qq[0][0] * qq[1][1] - qq[0][1] * qq[1][0];
        if det != 0.0 {
            w = Vec2::new(
                (-qq[0][2] * qq[1][1] + qq[1][2] * qq[0][1]) / det,
                (qq[0][2] * qq[1][0] - qq[1][2] * qq[0][0]) / det,
            );
            break;
        }
        let (v0, v1) = if qq[0][0] > qq[1][1] {
            (-qq[0][1], qq[0][0])
        } else if qq[1][1] != 0.0 {
            (-qq[1][1], qq[1][0])
        } else {
            (1.0, 0.0)
        };
        let d = v0 * v0 + v1 * v1;
        let v = [v0, v1, -v1 * s.y - v0 * s.x];
        for l in 0..3 {
            for k in 0..3 {
                qq[l][k] += v[l] * v[k] / d;
            }
        }
    }
    if (w.x - s.x).abs() <= vbox && (w.y - s.y).abs() <= vbox && w.x.is_finite() && w.y.is_finite() {
        return w;
    }
    let mut min = quadform(qq, s);
    let mut best = s;
    if qq[0][0] != 0.0 {
        for z in 0..2 {
            let wy = s.y - vbox + 2.0 * vbox * z as f64;
            let wx = -(qq[0][1] * wy + qq[0][2]) / qq[0][0];
            let cand = Vec2::new(wx, wy);
            let c = quadform(qq, cand);
            if (wx - s.x).abs() <= vbox && c < min {
                min = c;
                best = cand;
            }
        }
    }
    if qq[1][1] != 0.0 {
        for z in 0..2 {
            let wx = s.x - vbox + 2.0 * vbox * z as f64;
            let wy = -(qq[1][0] * wx + qq[1][2]) / qq[1][1];
            let cand = Vec2::new(wx, wy);
            let c = quadform(qq, cand);
            if (wy - s.y).abs() <= vbox && c < min {
                min = c;
                best = cand;
            }
        }
    }
    for l in 0..2 {
        for k in 0..2 {
            let cand = Vec2::new(s.x - vbox + 2.0 * vbox * l as f64, s.y - vbox + 2.0 * vbox * k as f64);
            let c = quadform(qq, cand);
            if c < min {
                min = c;
                best = cand;
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axis-aligned rectangle contour as `contour::subpixel` builds it:
    /// midpoints of unit steps around a `w × h` box at (0, 0) plus the four
    /// corner points, with pixel step directions.
    fn rect_contour(w: usize, h: usize) -> (Vec<Point>, Vec<u8>) {
        let mut pts = Vec::new();
        let mut dirs = Vec::new();
        let (wf, hf) = (w as f64, h as f64);
        for i in 0..w {
            pts.push(Point::new(i as f64 + 0.5, 0.0));
            dirs.push(0);
        }
        pts.push(Point::new(wf, 0.0));
        dirs.push(1);
        for i in 0..h {
            pts.push(Point::new(wf, i as f64 + 0.5));
            dirs.push(1);
        }
        pts.push(Point::new(wf, hf));
        dirs.push(2);
        for i in 0..w {
            pts.push(Point::new(wf - i as f64 - 0.5, hf));
            dirs.push(2);
        }
        pts.push(Point::new(0.0, hf));
        dirs.push(3);
        for i in 0..h {
            pts.push(Point::new(0.0, hf - i as f64 - 0.5));
            dirs.push(3);
        }
        pts.push(Point::new(0.0, 0.0));
        dirs.push(0);
        pts.rotate_right(1);
        dirs.rotate_right(1);
        (pts, dirs)
    }

    #[test]
    fn rectangle_has_four_vertices_at_its_corners() {
        let (pts, dirs) = rect_contour(12, 7);
        let (v, _) = polygonize(&pts, &dirs, 0.5, 0.5);
        assert_eq!(v.len(), 4, "{v:?}");
        let mut got: Vec<(i64, i64)> = v.iter().map(|p| ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64)).collect();
        got.sort();
        assert_eq!(got, vec![(0, 0), (0, 700), (1200, 0), (1200, 700)]);
    }

    #[test]
    fn thin_line_does_not_collapse() {
        // A 1 px tall, 20 px long line: top and bottom edges are 1 px apart,
        // within tolerance of a single line, but the four-direction rule
        // must keep the run from swallowing the whole loop.
        let (pts, dirs) = rect_contour(20, 1);
        let (v, _) = polygonize(&pts, &dirs, 0.2, 0.5);
        assert_eq!(v.len(), 4, "{v:?}");
    }

    #[test]
    fn circle_polygon_is_coarse_but_close() {
        let r = 30.0;
        let n = 240;
        let pts: Vec<Point> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                Point::new(50.0 + r * t.cos(), 50.0 + r * t.sin())
            })
            .collect();
        // Fake pixel directions from the tangent quadrant.
        let dirs: Vec<u8> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                let (dx, dy) = (-t.sin(), t.cos());
                if dx.abs() > dy.abs() { if dx > 0.0 { 0 } else { 2 } } else if dy > 0.0 { 1 } else { 3 }
            })
            .collect();
        let (v, _) = polygonize(&pts, &dirs, 0.5, 0.5);
        assert!(v.len() >= 8 && v.len() <= 40, "{} vertices", v.len());
        for p in &v {
            let d = ((p.x - 50.0).powi(2) + (p.y - 50.0).powi(2)).sqrt();
            assert!((d - r).abs() < 1.0, "vertex off the circle by {}", d - r);
        }
    }
}
