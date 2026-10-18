//! Stage 6: corners, Béziers and curve optimisation after Potrace
//! (Selinger 2003 §2.3-2.4, `trace.c` `smooth` and `opticurve`).
//!
//! Each polygon vertex becomes either a corner (two straight pieces) or a
//! cubic Bézier spanning the midpoints of its two edges, with control points
//! pulled toward the vertex by `alpha`. `alpha` measures how far the vertex
//! sticks out from the chord of its neighbours; above `alphamax` it is a
//! corner. `opticurve` then merges runs of same-convexity Béziers into single
//! cubics that preserve enclosed area, as long as every original vertex and
//! edge stays within `opttolerance` of the merged curve, choosing the
//! fewest segments by dynamic programming.
//!
//! Output differs from Potrace in one way that costs nothing in accuracy:
//! consecutive corners are joined by one straight line instead of two, so
//! a square is four nodes, not eight.

use kurbo::{BezPath, Point, Vec2};

const COS179: f64 = -0.999_847_695_156_391_3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    /// Straight from the previous end to `vertex`, straight on to `end`.
    Corner { vertex: Point, end: Point },
    /// Cubic from the previous end through `c0`, `c1` to `end`.
    Curve { c0: Point, c1: Point, end: Point },
}

impl Seg {
    pub fn end(&self) -> Point {
        match *self {
            Seg::Corner { end, .. } | Seg::Curve { end, .. } => end,
        }
    }
    pub fn is_corner(&self) -> bool {
        matches!(self, Seg::Corner { .. })
    }
}

/// Potrace's private curve: one entry per polygon vertex.
#[derive(Clone, Debug)]
struct Curve {
    corner: Vec<bool>,
    c: Vec<[Point; 3]>,
    vertex: Vec<Point>,
    alpha: Vec<f64>,
}

impl Curve {
    fn len(&self) -> usize {
        self.vertex.len()
    }
    fn rotate_left(&mut self, r: usize) {
        self.corner.rotate_left(r);
        self.c.rotate_left(r);
        self.vertex.rotate_left(r);
        self.alpha.rotate_left(r);
    }
}

/// Fit a closed polygon. `alphamax` is the corner threshold (Potrace 1.0),
/// `opttolerance` the curve-merge tolerance (Potrace 0.2; 0 disables).
pub fn fit(verts: &[Point], alphamax: f64, opttolerance: f64) -> Vec<Seg> {
    if verts.is_empty() {
        return Vec::new();
    }
    let mut cv = smooth(verts, alphamax);
    // Start at a corner when there is one, so no curve run wraps across the
    // start of the cyclic dynamic programme.
    if let Some(r) = cv.corner.iter().position(|&c| c) {
        cv.rotate_left(r);
    }
    let mut cv = if opttolerance > 0.0 && cv.len() >= 3 { opticurve(&cv, opttolerance) } else { cv };
    // opticurve lists the segment that ends at the anchor last; put a corner
    // back at index 0 so the path writer can start there.
    if let Some(r) = cv.corner.iter().position(|&c| c) {
        cv.rotate_left(r);
    }
    (0..cv.len())
        .map(|i| {
            if cv.corner[i] {
                Seg::Corner { vertex: cv.c[i][1], end: cv.c[i][2] }
            } else {
                Seg::Curve { c0: cv.c[i][0], c1: cv.c[i][1], end: cv.c[i][2] }
            }
        })
        .collect()
}

/// Which polygon vertices are corners under the Potrace alpha test.
pub fn corner_flags(verts: &[Point], alphamax: f64) -> Vec<bool> {
    if verts.is_empty() {
        return Vec::new();
    }
    smooth(verts, alphamax).corner
}

/// The Potrace alpha test for one vertex `v` between `prev` and `next`.
pub fn is_corner(prev: Point, v: Point, next: Point, alphamax: f64) -> bool {
    let denom = ddenom(prev, next);
    let alpha = if denom != 0.0 {
        let dd = (dpara(prev, v, next) / denom).abs();
        (if dd > 1.0 { 1.0 - 1.0 / dd } else { 0.0 }) / 0.75
    } else {
        4.0 / 3.0
    };
    alpha >= alphamax
}

/// Append one closed subpath for `segs` (as produced by [`fit`]).
pub fn append_subpath(path: &mut BezPath, segs: &[Seg]) {
    let m = segs.len();
    if m == 0 {
        return;
    }
    if let Seg::Corner { vertex, .. } = segs[0] {
        path.move_to(vertex);
        for i in 0..m {
            match segs[i] {
                Seg::Corner { vertex, end } => {
                    if i > 0 {
                        path.line_to(vertex);
                    }
                    if !segs[(i + 1) % m].is_corner() {
                        path.line_to(end);
                    }
                }
                Seg::Curve { c0, c1, end } => path.curve_to(c0, c1, end),
            }
        }
    } else {
        path.move_to(segs[m - 1].end());
        for s in segs {
            match *s {
                Seg::Curve { c0, c1, end } => path.curve_to(c0, c1, end),
                Seg::Corner { vertex, end } => {
                    path.line_to(vertex);
                    path.line_to(end);
                }
            }
        }
    }
    path.close_path();
}

// ---- geometry helpers (Potrace names) ----

#[inline]
fn interval(lambda: f64, a: Point, b: Point) -> Point {
    Point::new(a.x + lambda * (b.x - a.x), a.y + lambda * (b.y - a.y))
}

/// `(p1 - p0) × (p2 - p0)`
#[inline]
fn dpara(p0: Point, p1: Point, p2: Point) -> f64 {
    (p1 - p0).cross(p2 - p0)
}

/// `(p1 - p0) × (p3 - p2)`
#[inline]
fn cprod(p0: Point, p1: Point, p2: Point, p3: Point) -> f64 {
    (p1 - p0).cross(p3 - p2)
}

/// `(p1 - p0) · (p2 - p0)`
#[inline]
fn iprod(p0: Point, p1: Point, p2: Point) -> f64 {
    (p1 - p0).dot(p2 - p0)
}

/// `(p1 - p0) · (p3 - p2)`
#[inline]
fn iprod1(p0: Point, p1: Point, p2: Point, p3: Point) -> f64 {
    (p1 - p0).dot(p3 - p2)
}

#[inline]
fn ddist(p: Point, q: Point) -> f64 {
    (p - q).hypot()
}

#[inline]
fn sign(x: f64) -> i8 {
    if x > 0.0 {
        1
    } else if x < 0.0 {
        -1
    } else {
        0
    }
}

/// Potrace `ddenom`: L1-style normaliser for the distance of a vertex from
/// the chord of its neighbours.
fn ddenom(p0: Point, p2: Point) -> f64 {
    let r = Vec2::new(-(sign(p2.y - p0.y) as f64), sign(p2.x - p0.x) as f64);
    r.y * (p2.x - p0.x) - r.x * (p2.y - p0.y)
}

fn bezier(t: f64, p0: Point, p1: Point, p2: Point, p3: Point) -> Point {
    let s = 1.0 - t;
    Point::new(
        s * s * s * p0.x + 3.0 * (s * s * t) * p1.x + 3.0 * (t * t * s) * p2.x + t * t * t * p3.x,
        s * s * s * p0.y + 3.0 * (s * s * t) * p1.y + 3.0 * (t * t * s) * p2.y + t * t * t * p3.y,
    )
}

/// Parameter in `[0, 1]` where the Bézier is tangent to `q1 - q0`, or -1.
fn tangent(p0: Point, p1: Point, p2: Point, p3: Point, q0: Point, q1: Point) -> f64 {
    let a = cprod(p0, p1, q0, q1);
    let b = cprod(p1, p2, q0, q1);
    let c = cprod(p2, p3, q0, q1);
    let (qa, qb, qc) = (a - 2.0 * b + c, -2.0 * a + 2.0 * b, a);
    let d = qb * qb - 4.0 * qa * qc;
    if qa == 0.0 || d < 0.0 {
        return -1.0;
    }
    let s = d.sqrt();
    let r1 = (-qb + s) / (2.0 * qa);
    let r2 = (-qb - s) / (2.0 * qa);
    if (0.0..=1.0).contains(&r1) {
        r1
    } else if (0.0..=1.0).contains(&r2) {
        r2
    } else {
        -1.0
    }
}

// ---- smooth ----

fn smooth(v: &[Point], alphamax: f64) -> Curve {
    let m = v.len();
    let mut cv = Curve {
        corner: vec![false; m],
        c: vec![[Point::ZERO; 3]; m],
        vertex: v.to_vec(),
        alpha: vec![0.0; m],
    };
    for i in 0..m {
        let j = (i + 1) % m;
        let k = (i + 2) % m;
        let p4 = interval(0.5, v[k], v[j]);
        let denom = ddenom(v[i], v[k]);
        let mut alpha = if denom != 0.0 {
            let dd = (dpara(v[i], v[j], v[k]) / denom).abs();
            let a = if dd > 1.0 { 1.0 - 1.0 / dd } else { 0.0 };
            a / 0.75
        } else {
            4.0 / 3.0
        };
        if alpha >= alphamax || m < 3 {
            cv.corner[j] = true;
            cv.c[j] = [v[j], v[j], p4];
        } else {
            alpha = alpha.clamp(0.55, 1.0);
            let p2 = interval(0.5 + 0.5 * alpha, v[i], v[j]);
            let p3 = interval(0.5 + 0.5 * alpha, v[k], v[j]);
            cv.c[j] = [p2, p3, p4];
        }
        cv.alpha[j] = alpha;
    }
    cv
}

// ---- opticurve ----

#[derive(Clone, Copy, Debug)]
struct Opti {
    pen: f64,
    c: [Point; 2],
    s: f64,
    alpha: f64,
}

/// Can segments `i+1 ..= j` be replaced by one Bézier? Returns its penalty.
fn opti_penalty(cv: &Curve, i: usize, j: usize, opttolerance: f64, convc: &[i8], areac: &[f64]) -> Option<Opti> {
    let m = cv.len();
    if i == j {
        return None;
    }
    let i1 = (i + 1) % m;
    let conv = convc[i1];
    if conv == 0 {
        return None;
    }
    let d = ddist(cv.vertex[i], cv.vertex[i1]);
    let mut k = i1;
    while k != j {
        let k1 = (k + 1) % m;
        let k2 = (k + 2) % m;
        if convc[k1] != conv {
            return None;
        }
        if sign(cprod(cv.vertex[i], cv.vertex[i1], cv.vertex[k1], cv.vertex[k2])) != conv {
            return None;
        }
        if iprod1(cv.vertex[i], cv.vertex[i1], cv.vertex[k1], cv.vertex[k2])
            < d * ddist(cv.vertex[k1], cv.vertex[k2]) * COS179
        {
            return None;
        }
        k = k1;
    }

    let p0 = cv.c[i % m][2];
    let p1 = cv.vertex[(i + 1) % m];
    let p2 = cv.vertex[j % m];
    let p3 = cv.c[j % m][2];

    let mut area = areac[j] - areac[i];
    area -= dpara(cv.vertex[0], cv.c[i][2], cv.c[j][2]) / 2.0;
    if i >= j {
        area += areac[m];
    }

    let a1 = dpara(p0, p1, p2);
    let a2 = dpara(p0, p1, p3);
    let a3 = dpara(p0, p2, p3);
    let a4 = a1 + a3 - a2;
    if a2 == a1 {
        return None;
    }
    let t = a3 / (a3 - a4);
    let s = a2 / (a2 - a1);
    let big_a = a2 * t / 2.0;
    if big_a == 0.0 {
        return None;
    }
    let r = area / big_a;
    let alpha = 2.0 - (4.0 - r / 0.3).sqrt();
    if !alpha.is_finite() {
        return None;
    }
    let c0 = interval(t * alpha, p0, p1);
    let c1 = interval(s * alpha, p3, p2);
    let (p1, p2) = (c0, c1);

    let mut pen = 0.0;
    // Every polygon edge must be nearly tangent to the proposed curve.
    let mut k = (i + 1) % m;
    while k != j {
        let k1 = (k + 1) % m;
        let tt = tangent(p0, p1, p2, p3, cv.vertex[k], cv.vertex[k1]);
        if tt < -0.5 {
            return None;
        }
        let pt = bezier(tt, p0, p1, p2, p3);
        let d = ddist(cv.vertex[k], cv.vertex[k1]);
        if d == 0.0 {
            return None;
        }
        let d1 = dpara(cv.vertex[k], cv.vertex[k1], pt) / d;
        if d1.abs() > opttolerance {
            return None;
        }
        if iprod(cv.vertex[k], cv.vertex[k1], pt) < 0.0 || iprod(cv.vertex[k1], cv.vertex[k], pt) < 0.0 {
            return None;
        }
        pen += d1 * d1;
        k = k1;
    }
    // Every original curve must stay close to the proposed one.
    let mut k = i;
    while k != j {
        let k1 = (k + 1) % m;
        let tt = tangent(p0, p1, p2, p3, cv.c[k][2], cv.c[k1][2]);
        if tt < -0.5 {
            return None;
        }
        let pt = bezier(tt, p0, p1, p2, p3);
        let d = ddist(cv.c[k][2], cv.c[k1][2]);
        if d == 0.0 {
            return None;
        }
        let mut d1 = dpara(cv.c[k][2], cv.c[k1][2], pt) / d;
        let mut d2 = dpara(cv.c[k][2], cv.c[k1][2], cv.vertex[k1]) / d;
        d2 *= 0.75 * cv.alpha[k1];
        if d2 < 0.0 {
            d1 = -d1;
            d2 = -d2;
        }
        if d1 < d2 - opttolerance {
            return None;
        }
        if d1 < d2 {
            pen += (d1 - d2) * (d1 - d2);
        }
        k = k1;
    }
    Some(Opti { pen, c: [c0, c1], s, alpha })
}

fn opticurve(cv: &Curve, opttolerance: f64) -> Curve {
    let m = cv.len();
    let convc: Vec<i8> = (0..m)
        .map(|i| {
            if cv.corner[i] {
                0
            } else {
                sign(dpara(cv.vertex[(i + m - 1) % m], cv.vertex[i], cv.vertex[(i + 1) % m]))
            }
        })
        .collect();
    let mut areac = vec![0.0f64; m + 1];
    let mut area = 0.0;
    let p0 = cv.vertex[0];
    for i in 0..m {
        let i1 = (i + 1) % m;
        if !cv.corner[i1] {
            let alpha = cv.alpha[i1];
            area += 0.3 * alpha * (4.0 - alpha) * dpara(cv.c[i][2], cv.vertex[i1], cv.c[i1][2]) / 2.0;
            area += dpara(p0, cv.c[i][2], cv.c[i1][2]) / 2.0;
        }
        areac[i + 1] = area;
    }

    let mut pt = vec![0usize; m + 1];
    let mut pen = vec![0.0f64; m + 1];
    let mut len = vec![0usize; m + 1];
    let mut opt: Vec<Option<Opti>> = vec![None; m + 1];
    for j in 1..=m {
        pt[j] = j - 1;
        pen[j] = pen[j - 1];
        len[j] = len[j - 1] + 1;
        let mut i = j as isize - 2;
        while i >= 0 {
            let iu = i as usize;
            match opti_penalty(cv, iu, j % m, opttolerance, &convc, &areac) {
                None => break,
                Some(o) => {
                    if len[j] > len[iu] + 1 || (len[j] == len[iu] + 1 && pen[j] > pen[iu] + o.pen) {
                        pt[j] = iu;
                        pen[j] = pen[iu] + o.pen;
                        len[j] = len[iu] + 1;
                        opt[j] = Some(o);
                    }
                }
            }
            i -= 1;
        }
    }
    let om = len[m];
    let mut out = Curve {
        corner: Vec::with_capacity(om),
        c: Vec::with_capacity(om),
        vertex: Vec::with_capacity(om),
        alpha: Vec::with_capacity(om),
    };
    let mut j = m;
    for _ in 0..om {
        let jm = j % m;
        if pt[j] == j - 1 {
            out.corner.push(cv.corner[jm]);
            out.c.push(cv.c[jm]);
            out.vertex.push(cv.vertex[jm]);
            out.alpha.push(cv.alpha[jm]);
        } else {
            let o = opt[j].expect("merged segment without data");
            out.corner.push(false);
            out.c.push([o.c[0], o.c[1], cv.c[jm][2]]);
            out.vertex.push(interval(o.s, cv.c[jm][2], cv.vertex[jm]));
            out.alpha.push(o.alpha);
        }
        j = pt[j];
    }
    out.corner.reverse();
    out.c.reverse();
    out.vertex.reverse();
    out.alpha.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kurbo::PathEl;

    fn nodes(p: &BezPath) -> usize {
        p.elements().iter().filter(|e| !matches!(e, PathEl::ClosePath)).count()
    }

    #[test]
    fn square_is_four_lines() {
        let v = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0), Point::new(0.0, 10.0)];
        let segs = fit(&v, 1.0, 0.2);
        assert!(segs.iter().all(|s| s.is_corner()));
        let mut p = BezPath::new();
        append_subpath(&mut p, &segs);
        assert_eq!(nodes(&p), 4);
        assert!(matches!(p.elements()[0], PathEl::MoveTo(_)));
        assert!(p.elements()[1..4].iter().all(|e| matches!(e, PathEl::LineTo(_))));
    }

    #[test]
    fn regular_polygon_becomes_few_curves() {
        // A 48-gon approximating a circle: no corners, and opticurve should
        // merge it down to a handful of cubics.
        let n = 48;
        let v: Vec<Point> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                Point::new(100.0 + 80.0 * t.cos(), 100.0 + 80.0 * t.sin())
            })
            .collect();
        let segs = fit(&v, 1.0, 0.2);
        assert!(segs.iter().all(|s| !s.is_corner()));
        assert!(segs.len() <= 12, "{} segments", segs.len());
        // Every segment end stays on the circle.
        for s in &segs {
            let d = (s.end() - Point::new(100.0, 100.0)).hypot();
            assert!((d - 80.0).abs() < 0.5);
        }
    }

    #[test]
    fn d_shape_has_two_corners_and_starts_at_one() {
        // Semicircle on the right, straight side on the left: the two
        // junctions are 90° corners, the arc should merge into few cubics.
        // Polygon edges are ~8 px, as the polygon stage would make them.
        let n = 12;
        let mut v: Vec<Point> = (0..=n)
            .map(|i| {
                let t = -std::f64::consts::FRAC_PI_2 + i as f64 / n as f64 * std::f64::consts::PI;
                Point::new(50.0 + 30.0 * t.cos(), 50.0 + 30.0 * t.sin())
            })
            .collect();
        v.push(Point::new(50.0, 65.0));
        v.push(Point::new(50.0, 35.0));
        let segs = fit(&v, 1.0, 0.2);
        assert!(segs[0].is_corner(), "{segs:?}");
        assert_eq!(segs.iter().filter(|s| s.is_corner()).count(), 2, "{segs:?}");
        let mut p = BezPath::new();
        append_subpath(&mut p, &segs);
        assert!(nodes(&p) <= 8, "{} nodes: {:?}", nodes(&p), p.elements());
    }
}
