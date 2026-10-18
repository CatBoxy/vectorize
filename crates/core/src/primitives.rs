//! Primitive hypotheses (PLAN.md M4): a closed boundary that is a whole
//! circle, ellipse, rounded rectangle, regular polygon or star is emitted as
//! that primitive, with the node count a designer would use, instead of a
//! chain of fitted pieces. Each hypothesis is *proposed* cheaply from the
//! polygon and the samples' moments and *accepted* only if every sub-pixel
//! sample lies within the fit accuracy of the primitive's outline, so a
//! hypothesis never costs accuracy (Vectorizer.AI's "full shape fitting",
//! open-vectorizer's rasterize-and-compare at a fraction of the cost).
//!
//! Order: regular polygon and star (from the classified corners), circle,
//! ellipse, rounded rectangle. The first accepted wins.

use crate::scene::El;
use kurbo::{Point, Vec2};
use std::f64::consts::{PI, TAU};

/// Options of the hypothesis stage.
#[derive(Clone, Copy, Debug)]
pub struct PrimOpts {
    /// Every sample must be within this of the primitive, px.
    pub accuracy: f64,
    /// Most corners a regular polygon or star may have.
    pub max_sides: usize,
}

impl PrimOpts {
    pub fn new(accuracy: f64) -> Self {
        Self { accuracy, max_sides: MAX_SIDES }
    }
}

/// Regular polygons and stars with more corners than this are left to the
/// general fitter (they are circles to the eye anyway).
pub const MAX_SIDES: usize = 12;
/// A rounded rectangle needs a corner radius of at least this, px; below
/// it the shape is a rectangle and the corner model handles it.
pub const MIN_ROUND: f64 = 0.75;

/// Shapes an accepted hypothesis can describe.
#[derive(Clone, Debug, PartialEq)]
pub enum Primitive {
    Circle { c: Point, r: f64 },
    Ellipse { c: Point, rx: f64, ry: f64, rot: f64 },
    RoundedRect { c: Point, w: f64, h: f64, r: f64, rot: f64 },
    /// `radii` has one entry (regular polygon) or two (star), repeating.
    Polygon { c: Point, radii: Vec<f64>, m: usize, phase: f64 },
}

/// Try every hypothesis on a closed loop. `samples` are the usable contour
/// points in boundary order; `corners` the classified corner vertices in
/// the same order (empty when the loop is smooth). Returns the start point
/// and the elements (no `MoveTo`, no `Close`) of the accepted primitive,
/// wound like the samples.
pub fn propose(samples: &[Point], corners: &[Point], o: &PrimOpts) -> Option<(Primitive, Point, Vec<El>)> {
    if samples.len() < 8 {
        return None;
    }
    let ccw = crate::contour::signed_area2(samples) > 0.0;
    let mut cands: Vec<(Primitive, bool)> = Vec::new();
    if corners.len() >= 3 && corners.len() <= o.max_sides {
        cands.extend(propose_polygon(corners, 1).map(|p| (p, true)));
        if corners.len() >= 6 && corners.len() % 2 == 0 {
            cands.extend(propose_polygon(corners, 2).map(|p| (p, true)));
        }
    }
    if corners.is_empty() {
        cands.extend(propose_circle(samples).map(|p| (p, false)));
        cands.extend(propose_ellipse(samples).map(|p| (p, false)));
    }
    if corners.len() <= 4 {
        cands.extend(propose_rounded_rect(samples, corners).into_iter().map(|p| (p, false)));
    }
    // Samples inside a corner's clearance are biased by the corner itself
    // (see `fit::corner_clear`) and do not testify against a polygon.
    let m = corners.len();
    let clear: Vec<f64> = (0..m).map(|i| crate::fit::corner_clear(crate::fit::turn_at(corners[(i + m - 1) % m], corners[i], corners[(i + 1) % m]))).collect();
    let away: Vec<Point> = samples.iter().copied().filter(|s| corners.iter().zip(&clear).all(|(c, cl)| (*s - *c).hypot() > *cl)).collect();
    for (p, polygonal) in cands {
        let test = if polygonal { &away } else { samples };
        if !test.is_empty() && accept(&p, test, o) {
            let (start, els) = emit(&p, samples[0], ccw);
            return Some((p, start, els));
        }
    }
    None
}

/// Signed distance from `p` to the primitive's outline (positive outside),
/// first-order accurate for the ellipse, exact otherwise.
pub fn distance(p: &Primitive, q: Point) -> f64 {
    match p {
        Primitive::Circle { c, r } => (q - *c).hypot() - r,
        Primitive::Ellipse { c, rx, ry, rot } => {
            let v = rotate(q - *c, -rot);
            let (x, y) = (v.x / rx, v.y / ry);
            let f = x * x + y * y - 1.0;
            let g = Vec2::new(2.0 * x / rx, 2.0 * y / ry).hypot();
            if g < 1e-12 { f } else { f / g }
        }
        Primitive::RoundedRect { c, w, h, r, rot } => {
            let v = rotate(q - *c, -rot);
            let (hw, hh) = (0.5 * w, 0.5 * h);
            let (x, y) = (v.x.abs(), v.y.abs());
            let (ix, iy) = (hw - r, hh - r);
            if x <= ix {
                y - hh
            } else if y <= iy {
                x - hw
            } else {
                (Vec2::new(x - ix, y - iy)).hypot() - r
            }
        }
        Primitive::Polygon { c, radii, m, phase } => {
            // Distance to the nearest edge segment.
            let vert = |k: usize| -> Point {
                let a = phase + k as f64 * TAU / *m as f64;
                *c + Vec2::new(a.cos(), a.sin()) * radii[k % radii.len()]
            };
            let mut best = f64::MAX;
            for k in 0..*m {
                let (a, b) = (vert(k), vert(k + 1));
                let d = b - a;
                let t = ((q - a).dot(d) / d.hypot2().max(1e-12)).clamp(0.0, 1.0);
                best = best.min((q - a.lerp(b, t)).hypot());
            }
            best
        }
    }
}

fn accept(p: &Primitive, samples: &[Point], o: &PrimOpts) -> bool {
    let worst = samples.iter().map(|&s| distance(p, s).abs()).fold(0.0, f64::max);
    if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("prims") {
        eprintln!("primitive {p:?}: worst sample distance {worst:.3} ({})", if worst <= o.accuracy { "accepted" } else { "rejected" });
    }
    worst <= o.accuracy
}

#[inline]
fn rotate(v: Vec2, a: f64) -> Vec2 {
    let (s, c) = a.sin_cos();
    Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}

/// Area, centroid and central second moments of the polygon through
/// `pts` (Green's theorem). `None` for degenerate input.
fn moments(pts: &[Point]) -> Option<(f64, Point, [f64; 3])> {
    let n = pts.len();
    let (mut a2, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (pts[i], pts[(i + 1) % n]);
        let cr = p.x * q.y - q.x * p.y;
        a2 += cr;
        cx += (p.x + q.x) * cr;
        cy += (p.y + q.y) * cr;
    }
    let area = 0.5 * a2;
    if area.abs() < 1e-9 {
        return None;
    }
    let c = Point::new(cx / (3.0 * a2), cy / (3.0 * a2));
    let (mut ixx, mut iyy, mut ixy) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (p, q) = (pts[i] - c, pts[(i + 1) % n] - c);
        let cr = p.x * q.y - q.x * p.y;
        ixx += (p.x * p.x + p.x * q.x + q.x * q.x) * cr;
        iyy += (p.y * p.y + p.y * q.y + q.y * q.y) * cr;
        ixy += (p.x * q.y + 2.0 * p.x * p.y + 2.0 * q.x * q.y + q.x * p.y) * cr;
    }
    let s = area.signum();
    Some((area.abs(), c, [s * ixx / 12.0, s * iyy / 12.0, s * ixy / 24.0]))
}

fn propose_circle(samples: &[Point]) -> Option<Primitive> {
    let (c, r) = crate::fit::fit_circle(samples)?;
    (r > 1.0 && r < 1.0e5).then_some(Primitive::Circle { c, r })
}

/// Ellipse from the area moments (exact for a filled ellipse: the covariance
/// eigenvalues are `a²/4` and `b²/4`), refined by a few Gauss-Newton steps
/// on the first-order point distance.
fn propose_ellipse(samples: &[Point]) -> Option<Primitive> {
    let (area, c, [ixx, iyy, ixy]) = moments(samples)?;
    let (mxx, myy, mxy) = (ixx / area, iyy / area, ixy / area);
    let tr = mxx + myy;
    let det = mxx * myy - mxy * mxy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
    if l2 <= 1e-9 {
        return None;
    }
    let rot = 0.5 * (2.0 * mxy).atan2(mxx - myy);
    let mut e = Primitive::Ellipse { c, rx: 2.0 * l1.sqrt(), ry: 2.0 * l2.sqrt(), rot };
    // Refine: parameters (cx, cy, rx, ry, rot), numeric Jacobian.
    for _ in 0..6 {
        let Primitive::Ellipse { c, rx, ry, rot } = e else { unreachable!() };
        let base = [c.x, c.y, rx, ry, rot];
        let make = |v: [f64; 5]| Primitive::Ellipse { c: Point::new(v[0], v[1]), rx: v[2], ry: v[3], rot: v[4] };
        let stride = (samples.len() / 256).max(1);
        let pts: Vec<Point> = samples.iter().step_by(stride).copied().collect();
        let res: Vec<f64> = pts.iter().map(|&q| distance(&e, q)).collect();
        let eps = [1e-3, 1e-3, 1e-3, 1e-3, 1e-5];
        let mut jt_j = [[0.0f64; 5]; 5];
        let mut jt_r = [0.0f64; 5];
        let mut cols: Vec<Vec<f64>> = Vec::with_capacity(5);
        for k in 0..5 {
            let mut v = base;
            v[k] += eps[k];
            let pk = make(v);
            cols.push(pts.iter().zip(&res).map(|(&q, &r0)| (distance(&pk, q) - r0) / eps[k]).collect());
        }
        for a in 0..5 {
            for b in 0..5 {
                jt_j[a][b] = cols[a].iter().zip(&cols[b]).map(|(x, y)| x * y).sum();
            }
            jt_r[a] = cols[a].iter().zip(&res).map(|(x, r)| x * r).sum();
            jt_j[a][a] += 1e-9;
        }
        let Some(step) = solve5(jt_j, jt_r) else { break };
        let mut v = base;
        for k in 0..5 {
            v[k] -= step[k];
        }
        if !(v[2] > 0.5 && v[3] > 0.5 && v[2].is_finite() && v[3].is_finite() && v[4].is_finite()) {
            break;
        }
        e = make(v);
        if step.iter().all(|s| s.abs() < 1e-6) {
            break;
        }
    }
    let Primitive::Ellipse { rx, ry, .. } = e else { unreachable!() };
    (rx.max(ry) / rx.min(ry) < 40.0).then_some(e)
}

/// Gaussian elimination with partial pivoting for a 5×5 system.
fn solve5(mut a: [[f64; 5]; 5], mut b: [f64; 5]) -> Option<[f64; 5]> {
    for col in 0..5 {
        let piv = (col..5).max_by(|&i, &j| a[i][col].abs().partial_cmp(&a[j][col].abs()).unwrap())?;
        if a[piv][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..5 {
            let f = a[row][col] / a[col][col];
            for k in col..5 {
                a[row][k] -= f * a[col][k];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 5];
    for row in (0..5).rev() {
        let s: f64 = (row + 1..5).map(|k| a[row][k] * x[k]).sum();
        x[row] = (b[row] - s) / a[row][row];
    }
    Some(x)
}

/// Rounded rectangle: orientation from the longest side of the sample
/// polygon's bounding directions (axis-aligned first, then the dominant
/// edge direction), extents from the rotated samples, corner radius from
/// the area deficit `w h − A = (4 − π) r²`.
fn propose_rounded_rect(samples: &[Point], corners: &[Point]) -> Vec<Primitive> {
    let Some((area, c0, [ixx, iyy, ixy])) = moments(samples) else { return Vec::new() };
    let mut rots = vec![0.0];
    // Principal axis of the area as a second orientation guess; a rotated
    // rectangle has its principal axes along its sides.
    let rot = 0.5 * (2.0 * ixy).atan2(ixx - iyy);
    if rot.abs() > 0.5f64.to_radians() {
        rots.push(rot);
    }
    // With four corners the longest side gives the orientation directly.
    if corners.len() == 4 {
        let mut best = (0.0, 0.0);
        for i in 0..4 {
            let d = corners[(i + 1) % 4] - corners[i];
            if d.hypot() > best.0 {
                best = (d.hypot(), d.y.atan2(d.x));
            }
        }
        rots.push(best.1);
    }
    let mut out = Vec::new();
    for rot in rots {
        let local: Vec<Vec2> = samples.iter().map(|&p| rotate(p - c0, -rot)).collect();
        let (mut lo, mut hi) = (local[0], local[0]);
        for v in &local {
            lo = Vec2::new(lo.x.min(v.x), lo.y.min(v.y));
            hi = Vec2::new(hi.x.max(v.x), hi.y.max(v.y));
        }
        // Extents from the raw extremes give a first radius through the
        // area deficit; then each side is re-measured as the mean of the
        // samples on its straight part (within half a pixel of the extreme
        // and clear of the corner zones), and the radius as the mean
        // distance of the corner-zone samples from the inner corner. Two
        // rounds settle both.
        let (mut x0, mut x1, mut y0, mut y1) = (lo.x, hi.x, lo.y, hi.y);
        let (mut w, mut h) = (x1 - x0, y1 - y0);
        if w < 2.0 || h < 2.0 {
            continue;
        }
        let deficit = w * h - area;
        if deficit <= 0.0 {
            continue;
        }
        let mut r = (deficit / (4.0 - PI)).sqrt();
        if r < MIN_ROUND || r > 0.5 * w.min(h) + 0.5 {
            continue;
        }
        r = r.min(0.5 * w.min(h));
        let mut mid = Vec2::new(0.5 * (x0 + x1), 0.5 * (y0 + y1));
        for _ in 0..3 {
            let (ix, iy) = (0.5 * w - r, 0.5 * h - r);
            let side = |pick: &dyn Fn(&Vec2) -> f64, other: &dyn Fn(&Vec2) -> f64, extreme: f64, mid_other: f64, inner_other: f64| -> f64 {
                let near: Vec<f64> = local
                    .iter()
                    .filter(|v| (pick(v) - extreme).abs() <= 0.5 && (other(v) - mid_other).abs() <= inner_other - 0.25)
                    .map(|v| pick(v))
                    .collect();
                if near.is_empty() { extreme } else { near.iter().sum::<f64>() / near.len() as f64 }
            };
            x0 = side(&|v| v.x, &|v| v.y, x0, mid.y, iy);
            x1 = side(&|v| v.x, &|v| v.y, x1, mid.y, iy);
            y0 = side(&|v| v.y, &|v| v.x, y0, mid.x, ix);
            y1 = side(&|v| v.y, &|v| v.x, y1, mid.x, ix);
            w = x1 - x0;
            h = y1 - y0;
            mid = Vec2::new(0.5 * (x0 + x1), 0.5 * (y0 + y1));
            // Radius: a least-squares circle through the samples of each
            // corner zone (the mean distance from the inner corner would be
            // biased by any error in that corner's position).
            let (ix, iy) = (0.5 * w - r, 0.5 * h - r);
            let mut radii = Vec::with_capacity(4);
            for (sx, sy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
                let zone: Vec<Point> = local
                    .iter()
                    .filter(|v| (v.x - mid.x) * sx > ix + 0.25 && (v.y - mid.y) * sy > iy + 0.25)
                    .map(|v| Point::new(v.x, v.y))
                    .collect();
                if zone.len() >= 4 {
                    if let Some((_, rr)) = crate::fit::fit_circle(&zone) {
                        radii.push(rr);
                    }
                }
            }
            if radii.len() >= 2 {
                r = (radii.iter().sum::<f64>() / radii.len() as f64).min(0.5 * w.min(h));
            }
        }
        if r < MIN_ROUND || w < 2.0 || h < 2.0 {
            continue;
        }
        let c = c0 + rotate(mid, rot);
        if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("prims") {
            eprintln!("rrect candidate: c ({:.2},{:.2}) w {w:.3} h {h:.3} r {r:.3} rot {:.3} area {area:.1}", c.x, c.y, rot.to_degrees());
        }
        out.push(Primitive::RoundedRect { c, w, h, r, rot });
    }
    out
}

/// Regular polygon (`period` 1) or star (`period` 2) through the corner
/// vertices: centre at their mean, equal angular steps, one radius per
/// class. Corners that do not sit at equal angles are rejected here; the
/// outline test against the samples decides the rest.
fn propose_polygon(corners: &[Point], period: usize) -> Option<Primitive> {
    let m = corners.len();
    if m < 3 || m % period != 0 {
        return None;
    }
    let c = Point::new(corners.iter().map(|p| p.x).sum::<f64>() / m as f64, corners.iter().map(|p| p.y).sum::<f64>() / m as f64);
    let angles: Vec<f64> = corners.iter().map(|p| (*p - c).atan2()).collect();
    let dists: Vec<f64> = corners.iter().map(|p| (*p - c).hypot()).collect();
    // Orientation: sign of the first angular step.
    let step = TAU / m as f64;
    let wrap = |x: f64| (x + PI).rem_euclid(TAU) - PI;
    let s = wrap(angles[1] - angles[0]).signum();
    if s == 0.0 {
        return None;
    }
    // Phase: circular mean of `angle_k − s·k·step`.
    let (mut sx, mut sy) = (0.0, 0.0);
    for (k, &a) in angles.iter().enumerate() {
        let t = a - s * k as f64 * step;
        sx += t.cos();
        sy += t.sin();
    }
    let phase = sy.atan2(sx);
    // Every corner within a quarter step of its slot.
    for (k, &a) in angles.iter().enumerate() {
        if wrap(a - (phase + s * k as f64 * step)).abs() > 0.25 * step {
            return None;
        }
    }
    let mut radii = vec![0.0; period];
    for (k, &d) in dists.iter().enumerate() {
        radii[k % period] += d / (m / period) as f64;
    }
    if radii.iter().any(|&r| r < 1.0) || (period == 2 && (radii[0] - radii[1]).abs() < 1.0) {
        return None;
    }
    // The primitive lists its corners with increasing angle from `phase`;
    // a clockwise corner list is the same shape walked the other way.
    let p = Primitive::Polygon { c, radii, m, phase };
    Some(if s > 0.0 { p } else { reverse_polygon(p) })
}

/// A polygon listed clockwise: re-express it with increasing angles so the
/// emitter can walk `k = 0..m`.
fn reverse_polygon(p: Primitive) -> Primitive {
    match p {
        Primitive::Polygon { c, radii, m, phase } => {
            // Corner k sat at phase − k·step with radius radii[k % period];
            // walking the other way from the same first corner: angle
            // phase + k·step has radius radii[(m − k) % period].
            let period = radii.len();
            let r2: Vec<f64> = (0..period).map(|k| radii[(m - k) % period]).collect();
            Primitive::Polygon { c, radii: r2, m, phase }
        }
        other => other,
    }
}

/// Elements of the primitive as a closed loop starting near `near`, wound
/// counter-clockwise (positive signed area in y-down coordinates) when
/// `ccw`, else clockwise.
fn emit(p: &Primitive, near: Point, ccw: bool) -> (Point, Vec<El>) {
    let dir = if ccw { 1.0 } else { -1.0 };
    match p {
        Primitive::Circle { c, r } => {
            let a0 = (near - *c).atan2();
            let at = |k: f64| *c + Vec2::new((a0 + dir * k * TAU / 3.0).cos(), (a0 + dir * k * TAU / 3.0).sin()) * *r;
            let start = at(0.0);
            let sweep = ccw;
            (
                start,
                vec![
                    El::ArcTo { r: *r, large: false, sweep, end: at(1.0) },
                    El::ArcTo { r: *r, large: false, sweep, end: at(2.0) },
                    El::ArcTo { r: *r, large: false, sweep, end: start },
                ],
            )
        }
        Primitive::Ellipse { c, rx, ry, rot } => {
            let v = rotate(near - *c, -rot);
            let t0 = (v.y / ry).atan2(v.x / rx);
            let at = |k: f64| {
                let t = t0 + dir * k * TAU / 3.0;
                *c + rotate(Vec2::new(rx * t.cos(), ry * t.sin()), *rot)
            };
            let start = at(0.0);
            let sweep = ccw;
            let arc = |end: Point| El::EllipseTo { rx: *rx, ry: *ry, rot: *rot, large: false, sweep, end };
            (start, vec![arc(at(1.0)), arc(at(2.0)), arc(start)])
        }
        Primitive::RoundedRect { c, w, h, r, rot } => {
            let (hw, hh) = (0.5 * w, 0.5 * h);
            let (ix, iy) = (hw - r, hh - r);
            // Counter-clockwise in y-down screen space is clockwise on
            // paper: walk top edge left→right, right edge down, and so on,
            // which has positive signed area in y-down coordinates.
            let local: Vec<Vec2> = vec![
                Vec2::new(-ix, -hh),
                Vec2::new(ix, -hh),
                Vec2::new(hw, -iy),
                Vec2::new(hw, iy),
                Vec2::new(ix, hh),
                Vec2::new(-ix, hh),
                Vec2::new(-hw, iy),
                Vec2::new(-hw, -iy),
            ];
            let mut pts: Vec<Point> = local.iter().map(|&v| *c + rotate(v, *rot)).collect();
            if !ccw {
                pts.reverse();
                // Reversed: the sequence now starts at (-ix,-hh)? No: the
                // reversed list starts at (-hw,-iy); rotate so a line comes
                // first, as in the forward order.
                pts.rotate_left(1);
            }
            // Forward order: line, arc, line, arc, ... starting with a line.
            // Reversed+rotated order: starts at (-hw,-iy) → (-hw, iy)? Build
            // generically: consecutive points alternate line / arc, and a
            // pair is an arc when both points lie on the same corner circle.
            let is_arc = |a: Point, b: Point| {
                let (la, lb) = (rotate(a - *c, -rot), rotate(b - *c, -rot));
                (la.x.abs() > ix + 1e-9 || la.y.abs() > iy + 1e-9) && (lb.x.abs() > ix + 1e-9 || lb.y.abs() > iy + 1e-9) && (la - lb).hypot() < 2.0 * r + 1e-9 && ((la.x.abs() - lb.x.abs()).abs() > 1e-9 && (la.y.abs() - lb.y.abs()).abs() > 1e-9)
            };
            let n = pts.len();
            let start = pts[0];
            let mut els = Vec::with_capacity(n);
            for k in 0..n {
                let (a, b) = (pts[k], pts[(k + 1) % n]);
                if (a - b).hypot() < 1e-9 {
                    continue;
                }
                if is_arc(a, b) {
                    els.push(El::ArcTo { r: *r, large: false, sweep: ccw, end: b });
                } else {
                    els.push(El::LineTo(b));
                }
            }
            (start, els)
        }
        Primitive::Polygon { c, radii, m, phase } => {
            let vert = |k: usize| -> Point {
                let a = phase + dir * k as f64 * TAU / *m as f64;
                *c + Vec2::new(a.cos(), a.sin()) * radii[k % radii.len()]
            };
            let start = vert(0);
            let els: Vec<El> = (1..*m).map(|k| El::LineTo(vert(k))).collect();
            (start, els)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::Path;

    fn o() -> PrimOpts {
        PrimOpts::new(0.3)
    }

    fn path_of(start: Point, els: &[El]) -> Path {
        let mut p = Path::new();
        p.move_to(start);
        for e in els {
            p.push(*e);
        }
        p.close_path();
        p
    }

    /// Worst distance of a dense walk of `path` from the primitive, and the
    /// signed area of the path.
    fn check(path: &Path, prim: &Primitive) -> (f64, f64) {
        use kurbo::ParamCurve;
        let mut worst = 0.0f64;
        for seg in path.to_bez().segments() {
            for i in 0..=40 {
                let q = seg.eval(i as f64 / 40.0);
                worst = worst.max(distance(prim, q).abs());
            }
        }
        (worst, path.area())
    }

    fn noisy(pts: Vec<Point>) -> Vec<Point> {
        pts.into_iter().enumerate().map(|(i, p)| p + Vec2::new(0.05 * ((i % 3) as f64 - 1.0), 0.05 * ((i % 5) as f64 - 2.0) / 2.0)).collect()
    }

    #[test]
    fn circle_is_three_arcs() {
        let (c, r) = (Point::new(40.0, 50.0), 30.0);
        let pts: Vec<Point> = noisy((0..300).map(|i| c + Vec2::new((i as f64 / 300.0 * TAU).cos(), (i as f64 / 300.0 * TAU).sin()) * r).collect());
        let (p, start, els) = propose(&pts, &[], &o()).expect("circle");
        assert!(matches!(p, Primitive::Circle { .. }), "{p:?}");
        assert_eq!(els.len(), 3);
        let (worst, area) = check(&path_of(start, &els), &p);
        assert!(worst < 0.01, "{worst}");
        assert!((area - PI * r * r).abs() / (PI * r * r) < 0.01 && area > 0.0, "{area}");
    }

    #[test]
    fn ellipse_is_three_elliptical_arcs_with_rotation() {
        let c = Point::new(60.0, 45.0);
        let (rx, ry, rot) = (42.0, 26.0, 0.4);
        let pts: Vec<Point> = (0..400)
            .map(|i| {
                let t = i as f64 / 400.0 * TAU;
                c + rotate(Vec2::new(rx * t.cos(), ry * t.sin()), rot)
            })
            .collect();
        // Clockwise in y-down (reverse the list) must stay clockwise.
        let mut cw = pts.clone();
        cw.reverse();
        for (pts, ccw) in [(pts, true), (cw, false)] {
            let (p, start, els) = propose(&pts, &[], &o()).expect("ellipse");
            let Primitive::Ellipse { c: ec, rx: erx, ry: ery, rot: erot } = p.clone() else { panic!("{p:?}") };
            assert!((ec - c).hypot() < 0.05 && (erx - rx).abs() < 0.05 && (ery - ry).abs() < 0.05, "{p:?}");
            let wrap = |x: f64| (x + PI / 2.0).rem_euclid(PI) - PI / 2.0;
            assert!(wrap(erot - rot).abs() < 1e-3, "{p:?}");
            assert_eq!(els.len(), 3);
            assert!(els.iter().all(|e| matches!(e, El::EllipseTo { .. })));
            let (worst, area) = check(&path_of(start, &els), &p);
            assert!(worst < 0.02, "{worst}");
            assert!((area.abs() - PI * rx * ry).abs() / (PI * rx * ry) < 0.01, "{area}");
            assert_eq!(area > 0.0, ccw, "winding");
        }
    }

    #[test]
    fn rounded_rect_is_four_lines_and_four_arcs() {
        let (c, w, h, r) = (Point::new(50.0, 50.0), 80.0, 60.0, 14.0);
        let prim = Primitive::RoundedRect { c, w, h, r, rot: 0.0 };
        // Sample the outline by walking the ideal path.
        let (start, els) = emit(&prim, Point::new(10.0, 20.0), true);
        let bez = path_of(start, &els).to_bez();
        let mut pts = Vec::new();
        use kurbo::ParamCurve;
        for seg in bez.segments() {
            for i in 0..40 {
                pts.push(seg.eval(i as f64 / 40.0));
            }
        }
        let pts = noisy(pts);
        let (p, s, e) = propose(&pts, &[], &o()).expect("rounded rect");
        let Primitive::RoundedRect { c: pc, w: pw, h: ph, r: pr, .. } = p.clone() else { panic!("{p:?}") };
        assert!((pc - c).hypot() < 0.1 && (pw - w).abs() < 0.2 && (ph - h).abs() < 0.2 && (pr - r).abs() < 0.3, "{p:?}");
        assert_eq!(e.len(), 8, "{e:?}");
        assert_eq!(e.iter().filter(|x| matches!(x, El::ArcTo { .. })).count(), 4);
        let (worst, area) = check(&path_of(s, &e), &p);
        assert!(worst < 0.02 && area > 0.0, "{worst} {area}");
        // A plain rectangle is not a rounded rectangle.
        let rect: Vec<Point> = (0..40).map(|i| Point::new(i as f64, 0.0)).chain((0..20).map(|i| Point::new(40.0, i as f64))).chain((0..40).map(|i| Point::new(40.0 - i as f64, 20.0))).chain((0..20).map(|i| Point::new(0.0, 20.0 - i as f64))).collect();
        assert!(propose(&rect, &[], &o()).is_none());
    }

    #[test]
    fn hexagon_and_star_snap_to_regular_shapes() {
        let c = Point::new(50.0, 50.0);
        // Slightly irregular hexagon: corners jittered by up to 0.2 px.
        let corners: Vec<Point> = (0..6)
            .map(|k| {
                let a = -PI / 2.0 + k as f64 * TAU / 6.0;
                c + Vec2::new(a.cos(), a.sin()) * 44.0 + Vec2::new(0.2 * ((k % 2) as f64 - 0.5), -0.15 * ((k % 3) as f64 - 1.0))
            })
            .collect();
        let mut pts = Vec::new();
        for k in 0..6 {
            for i in 0..30 {
                pts.push(corners[k].lerp(corners[(k + 1) % 6], i as f64 / 30.0));
            }
        }
        let (p, s, e) = propose(&pts, &corners, &o()).expect("hexagon");
        let Primitive::Polygon { radii, m, .. } = p.clone() else { panic!("{p:?}") };
        assert_eq!((m, radii.len()), (6, 1));
        assert!((radii[0] - 44.0).abs() < 0.2);
        assert_eq!(e.len(), 5);
        let path = path_of(s, &e);
        // All six sides equal.
        let v: Vec<Point> = path.elements().iter().filter_map(|x| x.end()).collect();
        let lens: Vec<f64> = (0..6).map(|k| (v[(k + 1) % 6] - v[k]).hypot()).collect();
        assert!(lens.iter().all(|l| (l - lens[0]).abs() < 1e-9), "{lens:?}");
        assert!(check(&path, &p).1 > 0.0);

        // Five-pointed star, clockwise: 10 corners, two radii.
        let star: Vec<Point> = (0..10)
            .map(|k| {
                let a = -PI / 2.0 - k as f64 * TAU / 10.0;
                let r = if k % 2 == 0 { 45.0 } else { 20.0 };
                c + Vec2::new(a.cos(), a.sin()) * r
            })
            .collect();
        let mut pts = Vec::new();
        for k in 0..10 {
            for i in 0..20 {
                pts.push(star[k].lerp(star[(k + 1) % 10], i as f64 / 20.0));
            }
        }
        let (p, s, e) = propose(&pts, &star, &o()).expect("star");
        let Primitive::Polygon { radii, m, .. } = p.clone() else { panic!("{p:?}") };
        assert_eq!(m, 10);
        assert_eq!(radii.len(), 2);
        let path = path_of(s, &e);
        let (worst, area) = check(&path, &p);
        assert!(worst < 1e-6, "{worst}");
        assert!(area < 0.0, "clockwise input must stay clockwise");
        assert_eq!(e.len(), 9);
        // The emitted vertices are the star's own vertices.
        for v in path.elements().iter().filter_map(|x| x.end()) {
            assert!(star.iter().any(|q| (*q - v).hypot() < 1e-6), "{v:?}");
        }
        // An irregular quadrilateral is not a regular polygon.
        let quad = [Point::new(0.0, 0.0), Point::new(30.0, 0.0), Point::new(30.0, 12.0), Point::new(0.0, 18.0)];
        let mut pts = Vec::new();
        for k in 0..4 {
            for i in 0..20 {
                pts.push(quad[k].lerp(quad[(k + 1) % 4], i as f64 / 20.0));
            }
        }
        assert!(propose(&pts, &quad, &o()).is_none());
    }
}
