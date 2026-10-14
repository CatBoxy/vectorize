//! Stage 6 (default): fit each run of contour between two corners with
//! kurbo's cubic fitter (area and moment matching, Levien 2022), which
//! minimises the number of cubics for a given accuracy. Straight runs
//! become lines.
//!
//! Corners come from two places. The polygon stage marks sharp vertices with
//! Potrace's alpha test, which works at the scale of the polygon. Inside a
//! run, any point where the tangent turns by more than [`CUSP_DEG`] within
//! one tangent window is a corner too; that catches the ends of 1 px lines
//! and other features too small for the polygon to resolve. Everything
//! between two corners is one smooth run and is fitted from the dense
//! sub-pixel contour points, not from the polygon, so the curve passes
//! through the measured boundary instead of through chord midpoints (the
//! bias Potrace's `smooth` has, about a third of a pixel on a circle).

use crate::contour::Contour;
use crate::scene::{El, Path};
use kurbo::{fit_to_cubic, CubicBez, CurveFitSample, ParamCurveFit, ParamCurveNearest, Point, Vec2};
use std::ops::Range;

/// One fitted primitive over a range of samples, ending at the range's
/// last sample. Lines are preferred over arcs and arcs over cubics: the
/// simpler the primitive, the more the output looks drawn, not traced.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Prim {
    Line(Point),
    Arc { r: f64, large: bool, sweep: bool, end: Point },
    Cubic(CubicBez),
}

impl Prim {
    fn el(&self) -> El {
        match *self {
            Prim::Line(p) => El::LineTo(p),
            Prim::Arc { r, large, sweep, end } => El::ArcTo { r, large, sweep, end },
            Prim::Cubic(c) => El::CurveTo(c.p1, c.p2, c.p3),
        }
    }
}

/// Turning angle, in degrees over one tangent window, above which a point
/// inside a run is treated as a corner.
pub const CUSP_DEG: f64 = 60.0;
/// Samples closer than this to a corner vertex are not fitted, px.
pub const CORNER_CLEAR: f64 = 1.5;
/// Longest sweep of one arc, degrees.
pub const MAX_ARC_DEG: f64 = 170.0;
/// End tangents within this angle of the chord, on opposite sides of it,
/// make a range too degenerate for the cubic fitter (radians, ~3°).
pub const DEGENERATE_RAD: f64 = 0.052;

#[derive(Clone, Copy, Debug)]
pub struct FitOpts {
    /// Maximum distance between the fitted curve and the contour, px.
    pub accuracy: f64,
    /// Emit a line when the whole run is within `accuracy` of its chord.
    pub snap_lines: bool,
    /// Half-width, in px of arc length, of the window used to estimate
    /// tangents on the polyline.
    pub window: f64,
}

/// Arc-length parametrised polyline, open or closed.
pub struct PolySource {
    pts: Vec<Point>,
    cum: Vec<f64>,
    total: f64,
    closed: bool,
    window: f64,
    /// Vertex indices of cusps (see [`cusps`](Self::cusps)), computed once.
    cusp_idx: Vec<usize>,
}

impl PolySource {
    pub fn new(pts: &[Point], closed: bool, window: f64) -> Self {
        let mut p: Vec<Point> = Vec::with_capacity(pts.len() + 1);
        for &q in pts {
            if p.last().map_or(true, |l| (*l - q).hypot() > 1e-9) {
                p.push(q);
            }
        }
        if closed && p.len() > 1 && (*p.last().unwrap() - p[0]).hypot() <= 1e-9 {
            p.pop();
        }
        if closed && p.len() > 1 {
            p.push(p[0]);
        }
        let mut cum = Vec::with_capacity(p.len());
        let mut s = 0.0;
        cum.push(0.0);
        for i in 1..p.len() {
            s += (p[i] - p[i - 1]).hypot();
            cum.push(s);
        }
        let mut src = Self { pts: p, cum, total: s.max(1e-12), closed, window, cusp_idx: Vec::new() };
        src.cusp_idx = src.find_cusps();
        src
    }

    /// Points, with the closing duplicate for closed sources.
    pub fn points(&self) -> &[Point] {
        &self.pts
    }

    pub fn len(&self) -> usize {
        self.pts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pts.is_empty()
    }

    /// Index `i` with `cum[i] <= s < cum[i+1]`, clamped to a valid segment.
    fn seg_index(&self, s: f64) -> usize {
        let n = self.pts.len();
        let s = s.clamp(0.0, self.total);
        let i = self.cum.partition_point(|&c| c <= s);
        i.saturating_sub(1).min(n - 2)
    }

    fn seg_dir(&self, i: usize) -> Vec2 {
        let len = self.cum[i + 1] - self.cum[i];
        if len > 0.0 {
            (self.pts[i + 1] - self.pts[i]) / len
        } else {
            Vec2::ZERO
        }
    }

    /// Point at arc length `s`, clamped (open) or wrapped (closed).
    fn at(&self, s: f64) -> Point {
        let s = if self.closed { s.rem_euclid(self.total) } else { s.clamp(0.0, self.total) };
        let i = self.seg_index(s);
        let len = self.cum[i + 1] - self.cum[i];
        let u = if len > 0.0 { ((s - self.cum[i]) / len).clamp(0.0, 1.0) } else { 0.0 };
        self.pts[i].lerp(self.pts[i + 1], u)
    }

    /// Cosine of the turn at vertex `i`, measured between the chords one
    /// window before and one window after it. `None` at the ends of an open
    /// polyline.
    fn turn_cos(&self, i: usize) -> Option<f64> {
        let w = self.window.max(1e-6);
        let s = self.cum[i];
        if !self.closed && (s < w || s > self.total - w) {
            return None;
        }
        if self.closed && self.total < 2.0 * w {
            return None;
        }
        let a = self.at(s) - self.at(s - w);
        let b = self.at(s + w) - self.at(s);
        let (la, lb) = (a.hypot(), b.hypot());
        if la < 1e-9 || lb < 1e-9 {
            return None;
        }
        Some(a.dot(b) / (la * lb))
    }

    /// Vertex indices (into [`points`](Self::points), excluding the closing
    /// duplicate) where the polyline turns by more than [`CUSP_DEG`], with
    /// non-maximum suppression over one window.
    pub fn cusps(&self) -> &[usize] {
        &self.cusp_idx
    }

    fn find_cusps(&self) -> Vec<usize> {
        let n = if self.closed { self.pts.len().saturating_sub(1) } else { self.pts.len() };
        let limit = CUSP_DEG.to_radians().cos();
        let cand: Vec<(usize, f64)> = (0..n)
            .filter_map(|i| self.turn_cos(i).filter(|&c| c < limit).map(|c| (i, c)))
            .collect();
        if cand.is_empty() {
            return Vec::new();
        }
        // Keep the sharpest of any cluster closer than one window.
        let w = self.window.max(1e-6);
        let mut out: Vec<(usize, f64)> = Vec::new();
        for &(i, c) in &cand {
            if let Some(last) = out.last_mut() {
                if self.cum[i] - self.cum[last.0] < w {
                    if c < last.1 {
                        *last = (i, c);
                    }
                    continue;
                }
            }
            out.push((i, c));
        }
        if self.closed && out.len() > 1 {
            // The first and last cluster may touch across the seam.
            let (f, l) = (out[0], *out.last().unwrap());
            if self.total - self.cum[l.0] + self.cum[f.0] < w {
                if l.1 < f.1 {
                    out.remove(0);
                } else {
                    out.pop();
                }
            }
        }
        out.into_iter().map(|(i, _)| i).collect()
    }
}

impl ParamCurveFit for PolySource {
    fn sample_pt_tangent(&self, t: f64, _sign: f64) -> CurveFitSample {
        let s = t * self.total;
        let p = self.at(s);
        let w = self.window.min(self.total / 2.0);
        let (a, b) = if self.closed {
            (s - w, s + w)
        } else if s - w < 0.0 {
            (0.0, (2.0 * w).min(self.total))
        } else if s + w > self.total {
            ((self.total - 2.0 * w).max(0.0), self.total)
        } else {
            (s - w, s + w)
        };
        let mut tangent = self.at(b) - self.at(a);
        if tangent.hypot() < 1e-9 {
            tangent = self.seg_dir(self.seg_index(s));
        }
        if tangent.hypot() < 1e-9 {
            tangent = Vec2::new(1.0, 0.0);
        }
        CurveFitSample { p, tangent }
    }

    fn sample_pt_deriv(&self, t: f64) -> (Point, Vec2) {
        let s = t * self.total;
        let i = self.seg_index(s);
        (self.at(s), self.seg_dir(i) * self.total)
    }

    /// Exact `∫ y dx`, `∫ x y dx`, `∫ y² dx` over the polyline.
    fn moment_integrals(&self, range: Range<f64>) -> (f64, f64, f64) {
        let (s0, s1) = (range.start * self.total, range.end * self.total);
        let mut acc = (0.0, 0.0, 0.0);
        if s1 <= s0 {
            return acc;
        }
        let mut i = self.seg_index(s0);
        while i + 1 < self.pts.len() && self.cum[i] < s1 {
            let a = self.cum[i].max(s0);
            let b = self.cum[i + 1].min(s1);
            let len = self.cum[i + 1] - self.cum[i];
            if b > a && len > 0.0 {
                let (p, q) = (self.pts[i], self.pts[i + 1]);
                let pa = p.lerp(q, (a - self.cum[i]) / len);
                let pb = p.lerp(q, (b - self.cum[i]) / len);
                let (dx, dy) = (pb.x - pa.x, pb.y - pa.y);
                let (x0, y0) = (pa.x, pa.y);
                acc.0 += dx * (y0 + dy / 2.0);
                acc.1 += dx * (x0 * y0 + (x0 * dy + y0 * dx) / 2.0 + dx * dy / 3.0);
                acc.2 += dx * (y0 * y0 + y0 * dy + dy * dy / 3.0);
            }
            i += 1;
        }
        acc
    }

    /// Safety net: runs are already split at cusps before fitting, but if
    /// one slipped through, report it so the fitter never chases a hairpin.
    /// The fitter calls this at every subdivision, so it must be cheap: a
    /// binary search over the precomputed cusp positions.
    fn break_cusp(&self, range: Range<f64>) -> Option<f64> {
        if self.cusp_idx.is_empty() {
            return None;
        }
        let (s0, s1) = (range.start * self.total, range.end * self.total);
        let w = self.window.max(1e-6);
        if s1 - s0 < 2.0 * w {
            return None;
        }
        let first = self.cusp_idx.partition_point(|&i| self.cum[i] <= s0 + w);
        let s = self.cum[*self.cusp_idx.get(first)?];
        if s < s1 - w {
            Some(s / self.total)
        } else {
            None
        }
    }
}

/// Largest distance of any sample from the chord between the first and last,
/// or `None` if the samples do not progress monotonically along the chord
/// (a run that doubles back is never a line).
fn chord_deviation(samples: &[Point]) -> Option<f64> {
    let a = samples[0];
    let b = *samples.last().unwrap();
    let d = b - a;
    let len = d.hypot();
    if len < 1e-9 {
        return None;
    }
    let mut worst: f64 = 0.0;
    let mut last_proj = f64::NEG_INFINITY;
    for p in samples {
        let v = *p - a;
        worst = worst.max((v.cross(d) / len).abs());
        let proj = v.dot(d) / len;
        if proj + 0.5 < last_proj {
            return None;
        }
        last_proj = last_proj.max(proj);
    }
    Some(worst)
}

/// Reject a fit whose control points fly off: kurbo's moment matching can
/// return arms hundreds of pixels long on nearly straight data. Every
/// control point must stay inside the samples' bounding box grown by half
/// its diagonal plus a few times the accuracy.
fn fit_is_sane(els: &[El], samples: &[Point], accuracy: f64) -> bool {
    if els.is_empty() {
        return false;
    }
    let mut bb = kurbo::Rect::from_points(samples[0], samples[0]);
    for p in samples {
        bb = bb.union_pt(*p);
    }
    let m = 0.5 * (bb.width().hypot(bb.height())) + 4.0 * accuracy + 1.0;
    let bb = bb.inflate(m, m);
    els.iter().all(|e| match *e {
        El::CurveTo(a, b, p) => bb.contains(a) && bb.contains(b) && bb.contains(p),
        El::LineTo(p) | El::ArcTo { end: p, .. } => bb.contains(p),
        _ => true,
    })
}

/// Least-squares circle through `pts` (Kåsa's algebraic fit): centre and
/// radius, or `None` when the points are collinear or too few.
pub fn fit_circle(pts: &[Point]) -> Option<(Point, f64)> {
    if pts.len() < 3 {
        return None;
    }
    // Centre the data for conditioning.
    let n = pts.len() as f64;
    let mx = pts.iter().map(|p| p.x).sum::<f64>() / n;
    let my = pts.iter().map(|p| p.y).sum::<f64>() / n;
    let (mut suu, mut suv, mut svv, mut suuu, mut svvv, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let (u, v) = (p.x - mx, p.y - my);
        suu += u * u;
        suv += u * v;
        svv += v * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let det = suu * svv - suv * suv;
    if det.abs() < 1e-9 {
        return None;
    }
    let b1 = 0.5 * (suuu + suvv);
    let b2 = 0.5 * (svvv + svuu);
    let uc = (b1 * svv - b2 * suv) / det;
    let vc = (b2 * suu - b1 * suv) / det;
    let r = (uc * uc + vc * vc + (suu + svv) / n).sqrt();
    if !r.is_finite() {
        return None;
    }
    Some((Point::new(uc + mx, vc + my), r))
}

/// A circular arc through samples `a..=e` of `src`, pinned at both ends,
/// if every sample is within `accuracy` of it and the samples sweep
/// monotonically around its centre.
fn try_arc(src: &PolySource, a: usize, e: usize, accuracy: f64) -> Option<Prim> {
    let pts = &src.pts[a..=e];
    if pts.len() < 4 {
        return None;
    }
    let (p, q) = (pts[0], pts[pts.len() - 1]);
    let chord = q - p;
    let len = chord.hypot();
    if len < 4.0 * accuracy {
        return None;
    }
    let (c0, _) = fit_circle(pts)?;
    // Keep the ends exactly on the arc: slide the centre onto the chord's
    // perpendicular bisector.
    let mid = p.midpoint(q);
    let nrm = Vec2::new(-chord.y, chord.x) / len;
    let c = mid + nrm * (c0 - mid).dot(nrm);
    let r = (p - c).hypot();
    if !(r.is_finite() && r > 2.0 * accuracy && r < 1.0e5) {
        return None;
    }
    let mut prev = pts[0] - c;
    let mut sign = 0.0f64;
    let mut sweep_angle = 0.0;
    for w in pts.windows(2) {
        let v = w[1] - c;
        let cr = prev.cross(v);
        if cr != 0.0 {
            if sign == 0.0 {
                sign = cr.signum();
            } else if cr.signum() != sign {
                return None; // doubles back
            }
        }
        sweep_angle += cr.atan2(prev.dot(v)).abs();
        prev = v;
    }
    // An SVG arc is defined by its ends and radius; near 180° the centre is
    // ill-conditioned (a chord shortened by 0.01 px by coordinate rounding
    // makes a 100 px half circle sag by a pixel), so long sweeps are split.
    if sweep_angle > MAX_ARC_DEG.to_radians() {
        return None;
    }
    let stride = (pts.len() / 64).max(1);
    for s in pts.iter().step_by(stride).chain(std::iter::once(&q)) {
        if ((*s - c).hypot() - r).abs() > accuracy {
            return None;
        }
    }
    // SVG flags: sweep = clockwise on screen (positive cross in y-down
    // coordinates); large = the arc's midpoint and the centre on the same
    // side of the chord.
    let s_mid = pts[pts.len() / 2];
    let sweep = (s_mid - p).cross(q - s_mid) > 0.0;
    let side = |x: Point| chord.cross(x - p).signum();
    let large = side(s_mid) == side(c) && sweep_angle > std::f64::consts::PI;
    Some(Prim::Arc { r, large, sweep, end: q })
}

/// Last resort: Douglas-Peucker polyline simplification at `tol`, as lines.
fn simplify_lines(samples: &[Point], tol: f64, out: &mut Vec<El>) {
    fn rec(s: &[Point], tol: f64, out: &mut Vec<El>) {
        if s.len() < 3 {
            out.push(El::LineTo(*s.last().unwrap()));
            return;
        }
        let (a, b) = (s[0], *s.last().unwrap());
        let d = b - a;
        let len = d.hypot();
        let (mut worst, mut at) = (0.0f64, 0usize);
        for (i, p) in s.iter().enumerate().skip(1).take(s.len() - 2) {
            let v = *p - a;
            let dist = if len > 1e-9 { (v.cross(d) / len).abs() } else { v.hypot() };
            if dist > worst {
                worst = dist;
                at = i;
            }
        }
        if worst <= tol || at == 0 {
            out.push(El::LineTo(b));
        } else {
            rec(&s[..=at], tol, out);
            rec(&s[at..], tol, out);
        }
    }
    rec(samples, tol, out);
}

/// One cubic over samples `a..=e` of `src`, if kurbo finds one within
/// `accuracy` and it really stays within `1.25 × accuracy` of the samples
/// (kurbo checks only 20 points, which is too few for a long range) with
/// control points that do not fly off.
/// The simplest primitive that fits samples `a..=e`: a line when every
/// sample is within `accuracy` of the chord, else an arc, else a cubic.
fn try_prim(src: &PolySource, a: usize, e: usize, accuracy: f64, snap_lines: bool) -> Option<Prim> {
    let pts = &src.pts[a..=e];
    if let Some(dev) = chord_deviation(pts) {
        if dev <= accuracy {
            let (p0, p3) = (pts[0], pts[pts.len() - 1]);
            return Some(if snap_lines {
                Prim::Line(p3)
            } else {
                Prim::Cubic(CubicBez::new(p0, p0.lerp(p3, 1.0 / 3.0), p0.lerp(p3, 2.0 / 3.0), p3))
            });
        }
    }
    if let Some(arc) = try_arc(src, a, e, accuracy) {
        return Some(arc);
    }
    try_fit(src, a, e, accuracy).map(Prim::Cubic)
}

fn try_fit(src: &PolySource, a: usize, e: usize, accuracy: f64) -> Option<CubicBez> {
    let pts = &src.pts[a..=e];
    // Straight enough for a line: kurbo's moment matching is ill-conditioned
    // on such data and returns wild control arms, so hand it a straight
    // cubic instead (collinear controls at the thirds).
    if let Some(dev) = chord_deviation(pts) {
        if dev <= accuracy {
            let (p0, p3) = (pts[0], pts[pts.len() - 1]);
            return Some(CubicBez::new(p0, p0.lerp(p3, 1.0 / 3.0), p0.lerp(p3, 2.0 / 3.0), p3));
        }
    }
    let (ta, te) = (src.cum[a] / src.total, src.cum[e] / src.total);
    let dbg = std::env::var("VECTORIZE_DEBUG").is_ok();
    let trace = std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("trace");
    // A straight run with a jog at one end: both end tangents are nearly
    // parallel to the chord and turned to opposite sides of it, while the
    // samples are not on the chord. kurbo's area/moment equations are
    // singular there; the candidates it produces have control arms hundreds
    // of pixels long, and checking their error takes minutes. Refuse, and
    // let the greedy search shorten the range instead.
    let s0 = src.sample_pt_tangent(ta, 1.0);
    let s1 = src.sample_pt_tangent(te, -1.0);
    let chord = s1.p - s0.p;
    if chord.hypot() > 1e-9 {
        let th = chord.atan2();
        let wrap = |x: f64| (x + core::f64::consts::PI).rem_euclid(core::f64::consts::TAU) - core::f64::consts::PI;
        let th0 = wrap(s0.tangent.atan2() - th);
        let th1 = wrap(th - s1.tangent.atan2());
        if th0.abs() < DEGENERATE_RAD && th1.abs() < DEGENERATE_RAD && th0 * th1 <= 0.0 {
            if dbg {
                eprintln!("try_fit {a}..{e}: degenerate S (th0 {th0:.4}, th1 {th1:.4})");
            }
            return None;
        }
    }
    if trace {
        eprintln!(
            "try_fit {a}..{e}: kurbo t {ta:.6}..{te:.6} total {:.3} closed {} tan0 {:?} tan1 {:?} pts {:?}",
            src.total,
            src.closed,
            s0.tangent,
            s1.tangent,
            pts.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
        );
    }
    let Some((c, err2)) = fit_to_cubic(src, ta..te, accuracy) else {
        if dbg {
            eprintln!("try_fit {a}..{e}: kurbo None (t {ta:.4}..{te:.4}, pts {:?} .. {:?})", src.pts[a], src.pts[e]);
        }
        return None;
    };
    if !fit_is_sane(&[El::CurveTo(c.p1, c.p2, c.p3)], pts, accuracy) {
        if dbg {
            eprintln!("try_fit {a}..{e}: insane {c:?}");
        }
        return None;
    }
    if trace {
        eprintln!("try_fit {a}..{e}: kurbo gave {c:?}, verifying");
    }
    let limit = (accuracy * 1.25).powi(2);
    let stride = (pts.len() / 64).max(1);
    for p in pts.iter().step_by(stride) {
        let d2 = c.nearest(*p, 0.02).distance_sq;
        if d2 > limit {
            if dbg {
                eprintln!("try_fit {a}..{e}: sample {p:?} off by {:.3} (kurbo err {:.3}) {c:?}", d2.sqrt(), err2.sqrt());
            }
            return None;
        }
    }
    Some(c)
}

/// Greedy segmentation of samples `lo..=hi`: each primitive is extended as
/// far as a single fit stays within accuracy, found by exponential then
/// binary search over sample indices. A few segments more than optimal in
/// the worst case, and never slow or stuck, which kurbo's own optimiser
/// can be.
fn fit_greedy(src: &PolySource, lo: usize, hi: usize, accuracy: f64, snap_lines: bool, out: &mut Vec<El>) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut a = lo;
    while a < hi {
        let mut ok = a + 1;
        let mut best: Option<Prim> = None;
        let mut fail: Option<usize> = None;
        let mut step = 1;
        loop {
            let e = (ok + step).min(hi);
            match try_prim(src, a, e, accuracy, snap_lines) {
                Some(c) => {
                    ok = e;
                    best = Some(c);
                    if e == hi {
                        break;
                    }
                    step *= 2;
                }
                None => {
                    fail = Some(e);
                    break;
                }
            }
        }
        if let Some(mut f) = fail {
            while f - ok > 1 {
                let m = (ok + f) / 2;
                match try_prim(src, a, m, accuracy, snap_lines) {
                    Some(c) => {
                        ok = m;
                        best = Some(c);
                    }
                    None => f = m,
                }
            }
        }
        match best {
            Some(c) => out.push(c.el()),
            None => out.push(El::LineTo(src.pts[ok])),
        }
        ends.push(ok);
        a = ok;
    }
    ends
}

/// Elements (no `MoveTo`) approximating samples `a..=b` of `src`, an open
/// run with no corners inside. Points of `src` outside the range are ghosts
/// that only inform the tangents at the ends.
fn fit_open_range(src: &PolySource, a: usize, b: usize, o: &FitOpts) -> Vec<El> {
    if b <= a {
        return Vec::new();
    }
    let samples = &src.pts[a..=b];
    let last = samples[samples.len() - 1];
    if samples.len() == 2 {
        return vec![El::LineTo(last)];
    }
    if o.snap_lines {
        if let Some(dev) = chord_deviation(samples) {
            if dev <= o.accuracy {
                return vec![El::LineTo(last)];
            }
        }
    }
    let mut els = Vec::new();
    fit_greedy(src, a, b, o.accuracy, o.snap_lines, &mut els);
    if fit_is_sane(&els, samples, o.accuracy) {
        return els;
    }
    let mut out = Vec::new();
    simplify_lines(samples, o.accuracy, &mut out);
    out
}

/// Elements (no `MoveTo`) approximating an open run with no corners inside.
fn fit_open(samples: &[Point], o: &FitOpts) -> Vec<El> {
    if samples.len() < 2 {
        return Vec::new();
    }
    let src = PolySource::new(samples, false, o.window);
    if src.len() < 2 {
        return Vec::new();
    }
    fit_open_range(&src, 0, src.len() - 1, o)
}

/// Fit an open run whose ends continue smoothly into other curves: `prefix`
/// leads into the first sample and `suffix` continues after the last (both
/// ordered along the run). They only shape the end tangents, so the join is
/// G1 across a junction instead of a kink.
pub fn fit_run_ghost(samples: &[Point], prefix: &[Point], suffix: &[Point], o: &FitOpts) -> Vec<El> {
    if samples.len() < 2 {
        return Vec::new();
    }
    let mut all: Vec<Point> = Vec::with_capacity(prefix.len() + samples.len() + suffix.len());
    all.extend_from_slice(prefix);
    all.extend_from_slice(samples);
    all.extend_from_slice(suffix);
    let src = PolySource::new(&all, false, o.window);
    // Dedup inside `PolySource::new` may have shifted indices; find the
    // run's first and last sample by position.
    let pts = src.points();
    let find = |p: Point| (0..pts.len()).find(|&i| (pts[i] - p).hypot() < 1e-9);
    let (Some(lo), Some(hi)) = (find(samples[0]), find(samples[samples.len() - 1])) else {
        return fit_open(samples, o);
    };
    let hi = hi.max(lo);
    let cusps: Vec<usize> = src.cusps().iter().copied().filter(|&c| c > lo && c < hi).collect();
    let mut bounds = vec![lo];
    bounds.extend(cusps);
    bounds.push(hi);
    let mut els = Vec::new();
    for w in bounds.windows(2) {
        els.extend(fit_open_range(&src, w[0], w[1], o));
    }
    els
}

/// Options for deciding which polygon vertices are corners.
#[derive(Clone, Copy, Debug)]
pub struct CornerOpts {
    /// Potrace alpha threshold.
    pub alphamax: f64,
    /// Below this turning angle (radians) a vertex is never a corner.
    pub min_turn: f64,
    pub accuracy: f64,
    pub window: f64,
}

/// Which polygon vertices are corners (PLAN.md M3): Potrace's alpha test,
/// or, when the polygon turns by at least `min_turn` there, the accuracy
/// test: a vertex is a corner if no single cubic fits the contour from the
/// previous vertex to the next within `accuracy`. That second test does not
/// depend on the polygon's edge lengths, which is where alpha alone fails
/// (corners next to short edges, ends of thin lines). For an open chain
/// both ends are corners.
pub fn classify_corners(pts: &[Point], fit_ok: &[bool], verts: &[Point], po: &[usize], closed: bool, o: &CornerOpts) -> Vec<bool> {
    let m = verts.len();
    let n = pts.len();
    (0..m)
        .map(|i| {
            if !closed && (i == 0 || i == m - 1) {
                return true;
            }
            if m < 3 {
                return true;
            }
            let (ip, inx) = ((i + m - 1) % m, (i + 1) % m);
            let (p, v, q) = (verts[ip], verts[i], verts[inx]);
            if crate::curve::is_corner(p, v, q, o.alphamax) {
                return true;
            }
            let (a, b) = (v - p, q - v);
            let turn = a.cross(b).atan2(a.dot(b)).abs();
            if turn < o.min_turn {
                return false;
            }
            let span = if closed { (po[inx] + n - po[ip]) % n } else { po[inx].saturating_sub(po[ip]) };
            if span == 0 {
                return false;
            }
            let mut samples = Vec::with_capacity(span + 1);
            samples.push(p);
            let mut k = (po[ip] + 1) % n;
            for _ in 1..span {
                if fit_ok[k] {
                    samples.push(pts[k]);
                }
                k = (k + 1) % n;
            }
            samples.push(q);
            let smooth = smooth_across(&samples, o);
            if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("corners") {
                let src = PolySource::new(&samples, false, o.window);
                let prim = if src.len() >= 3 { try_prim(&src, 0, src.len() - 1, o.accuracy, true) } else { None };
                eprintln!(
                    "corner? v{i} ({:.2},{:.2}) turn {:.1}° {} samples -> smooth {smooth} via {:?}",
                    v.x,
                    v.y,
                    turn.to_degrees(),
                    samples.len(),
                    prim.map(|p| p.el())
                );
            }
            !smooth
        })
        .collect()
}

/// Can one primitive (line, arc or cubic) follow `samples` end to end
/// within `accuracy`?
fn smooth_across(samples: &[Point], o: &CornerOpts) -> bool {
    if samples.len() < 3 {
        return true;
    }
    let src = PolySource::new(samples, false, o.window);
    if src.len() < 3 {
        return true;
    }
    try_prim(&src, 0, src.len() - 1, o.accuracy, true).is_some()
}

/// Start point and elements (no `MoveTo`) approximating a closed run with
/// no corners. How far each greedy cubic reaches depends on the noise it
/// happens to meet, so the seam costs anything from zero to three extra
/// segments depending on where the pass starts. Several starts are tried,
/// evenly spaced plus the middle of the first pass's longest segment, and
/// the shortest result wins.
fn fit_closed(samples: &[Point], o: &FitOpts) -> (Point, Vec<El>) {
    let src = PolySource::new(samples, true, o.window);
    let n = src.len() - 1; // without the closing duplicate
    // A whole circle: three arcs of 120° (two half circles would be
    // numerically fragile, see `try_arc`).
    if let Some((c, r)) = fit_circle(&src.pts[..n]) {
        if r > 2.0 * o.accuracy && src.pts[..n].iter().all(|p| ((*p - c).hypot() - r).abs() <= o.accuracy) {
            let v = src.pts[0] - c;
            let a0 = v.atan2();
            let sweep = crate::contour::signed_area2(&src.pts[..n]) > 0.0;
            let dir = if sweep { 1.0 } else { -1.0 };
            let at = |k: f64| {
                let a = a0 + dir * k * (2.0 * std::f64::consts::PI / 3.0);
                c + Vec2::new(a.cos(), a.sin()) * r
            };
            let start = at(0.0);
            let els = vec![
                El::ArcTo { r, large: false, sweep, end: at(1.0) },
                El::ArcTo { r, large: false, sweep, end: at(2.0) },
                El::ArcTo { r, large: false, sweep, end: start },
            ];
            return (start, els);
        }
    }
    let mut els = Vec::new();
    let ends = fit_greedy(&src, 0, n, o.accuracy, o.snap_lines, &mut els);
    let mut best = (src.pts[0], els);
    let mut starts: Vec<usize> = vec![n / 4, n / 2, 3 * n / 4];
    if ends.len() >= 2 {
        let mut a = 0;
        let (mut la, mut lb) = (0usize, 0usize);
        for &b in &ends {
            if b - a > lb - la {
                la = a;
                lb = b;
            }
            a = b;
        }
        starts.push((la + lb) / 2);
    }
    let dbg = std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("closed");
    if dbg {
        eprintln!("closed run: {} samples, start 0: {} segments ends {:?}", n, ends.len(), ends);
    }
    for start in starts {
        if start == 0 || start >= n {
            continue;
        }
        let mut rotated: Vec<Point> = src.pts[..n].to_vec();
        rotated.rotate_left(start);
        let src2 = PolySource::new(&rotated, true, o.window);
        let mut els2 = Vec::new();
        let ends2 = fit_greedy(&src2, 0, src2.len() - 1, o.accuracy, o.snap_lines, &mut els2);
        if dbg {
            eprintln!("  start {start}: {} segments ends {:?}", ends2.len(), ends2);
        }
        if els2.len() < best.1.len() {
            best = (src2.pts[0], els2);
        }
    }
    if fit_is_sane(&best.1, src.points(), o.accuracy) {
        return best;
    }
    let mut out = Vec::new();
    simplify_lines(src.points(), o.accuracy, &mut out);
    (src.pts[0], out)
}

/// Path elements (no `MoveTo`) for `samples`, split at cusps. For a closed
/// run the elements start at the returned point (a cusp if there is one,
/// else `samples[0]`) and come back to it.
pub fn fit_run(samples: &[Point], closed: bool, o: &FitOpts) -> (Point, Vec<El>) {
    if samples.is_empty() {
        return (Point::ZERO, Vec::new());
    }
    let src = PolySource::new(samples, closed, o.window);
    let pts = src.points();
    let n = if closed { pts.len().saturating_sub(1) } else { pts.len() };
    if n < 2 {
        return (pts[0], Vec::new());
    }
    let cusps: Vec<usize> = src.cusps().to_vec();
    if closed && cusps.is_empty() {
        if n < 3 {
            return (pts[0], Vec::new());
        }
        return fit_closed(&pts[..n], o);
    }
    // Split into open pieces between consecutive cusps.
    let mut bounds: Vec<usize> = Vec::with_capacity(cusps.len() + 2);
    if closed {
        bounds.extend(cusps.iter().copied());
        bounds.push(cusps[0] + n);
    } else {
        bounds.push(0);
        bounds.extend(cusps.iter().copied().filter(|&c| c > 0 && c < n - 1));
        bounds.push(n - 1);
    }
    let start = pts[bounds[0] % n];
    let mut els = Vec::new();
    for w in bounds.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b <= a {
            continue;
        }
        let piece: Vec<Point> = (a..=b).map(|i| pts[i % n]).collect();
        els.extend(fit_open(&piece, o));
    }
    (start, els)
}

/// Append one closed subpath: the loop `c`, polygonised into `verts` at
/// contour indices `po`, with `corner[i]` marking sharp vertices.
pub fn append_loop(path: &mut Path, c: &Contour, verts: &[Point], po: &[usize], corner: &[bool], o: &FitOpts) {
    let n = c.pts.len();
    let m = verts.len();
    if n == 0 || m == 0 {
        return;
    }
    // Cut-corner vertex pairs were merged by the polygon stage; what is
    // left a pixel apart is a real short edge, such as the end of a line.
    let corners: Vec<usize> = (0..m).filter(|&i| corner[i]).collect();
    let mut els_all: Vec<El> = Vec::new();
    let start;
    if corners.is_empty() {
        // One closed run over every usable point, starting at vertex 0.
        let first = po[0] % n;
        let samples: Vec<Point> = (0..n)
            .map(|k| (first + k) % n)
            .filter(|&k| c.fit_ok[k])
            .map(|k| c.pts[k])
            .collect();
        if samples.len() < 3 {
            return;
        }
        let (s, els) = fit_run(&samples, true, o);
        start = s;
        els_all = els;
    } else {
        start = verts[corners[0]];
        // A corner vertex is the intersection of the two fitted edge lines,
        // which on an anti-aliased corner lies past the stair-step points
        // around it. Those points would put a hook on the corner, so leave
        // out every sample within `clear` of either end vertex.
        let clear = CORNER_CLEAR;
        for (ci, &a) in corners.iter().enumerate() {
            let b = corners[(ci + 1) % corners.len()];
            let (ia, ib) = (po[a] % n, po[b] % n);
            let steps = (ib + n - ia) % n;
            let steps = if steps == 0 { n } else { steps };
            let (va, vb) = (verts[a], verts[b]);
            let mut samples = Vec::with_capacity(steps + 1);
            samples.push(va);
            let mut k = (ia + 1) % n;
            for _ in 1..steps {
                let p = c.pts[k];
                if c.fit_ok[k] && (p - va).hypot() > clear && (p - vb).hypot() > clear {
                    samples.push(p);
                }
                k = (k + 1) % n;
            }
            samples.push(vb);
            els_all.extend(fit_run(&samples, false, o).1);
        }
    }
    if els_all.is_empty() {
        return;
    }
    // The closing line is implied by `Z`.
    if let Some(El::LineTo(p)) = els_all.last() {
        if (*p - start).hypot() < 1e-9 {
            els_all.pop();
        }
    }
    path.move_to(start);
    for e in els_all {
        path.push(e);
    }
    path.close_path();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moment_integrals_are_exact_on_a_polyline() {
        let pts = [
            Point::new(1.0, 2.0),
            Point::new(4.0, 7.0),
            Point::new(6.5, 3.0),
            Point::new(2.0, 1.5),
            Point::new(9.0, 9.0),
        ];
        let src = PolySource::new(&pts, false, 0.5);
        for (t0, t1) in [(0.0, 1.0), (0.1, 0.9), (0.33, 0.34), (0.5, 0.75)] {
            let exact = src.moment_integrals(t0..t1);
            // Brute force: midpoint rule with many steps.
            let n = 200_000;
            let mut num = (0.0, 0.0, 0.0);
            let dt = (t1 - t0) / n as f64;
            for k in 0..n {
                let t = t0 + (k as f64 + 0.5) * dt;
                let (p, d) = src.sample_pt_deriv(t);
                let a = d.x * p.y * dt;
                num.0 += a;
                num.1 += p.x * a;
                num.2 += p.y * a;
            }
            let rel = |a: f64, b: f64| (a - b).abs() / (1.0 + a.abs());
            assert!(rel(exact.0, num.0) < 1e-4, "{exact:?} vs {num:?}");
            assert!(rel(exact.1, num.1) < 1e-4, "{exact:?} vs {num:?}");
            assert!(rel(exact.2, num.2) < 1e-4, "{exact:?} vs {num:?}");
        }
    }

    #[test]
    fn straight_run_snaps_to_a_line() {
        let pts: Vec<Point> = (0..20).map(|i| Point::new(i as f64, 0.01 * (i % 2) as f64)).collect();
        let o = FitOpts { accuracy: 0.3, snap_lines: true, window: 1.0 };
        let (_, els) = fit_run(&pts, false, &o);
        assert_eq!(els, vec![El::LineTo(Point::new(19.0, 0.01))]);
        // Without snapping it is still one segment, a straight cubic.
        let o = FitOpts { snap_lines: false, ..o };
        let (_, els) = fit_run(&pts, false, &o);
        assert_eq!(els.len(), 1, "{els:?}");
        match els[0] {
            El::CurveTo(a, b, p) => {
                assert!(a.y.abs() < 0.02 && b.y.abs() < 0.02 && (p.x - 19.0).abs() < 1e-9, "{els:?}");
            }
            _ => panic!("{els:?}"),
        }
    }

    #[test]
    fn closed_circle_fits_with_few_cubics() {
        let n = 400;
        let pts: Vec<Point> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                Point::new(50.0 + 40.0 * t.cos(), 50.0 + 40.0 * t.sin())
            })
            .collect();
        let o = FitOpts { accuracy: 0.1, snap_lines: true, window: 1.0 };
        let (start, els) = fit_run(&pts, true, &o);
        assert!((start - pts[0]).hypot() < 0.05);
        // A circle is three arcs.
        assert_eq!(els.len(), 3, "{els:?}");
        for e in &els {
            match e {
                El::ArcTo { r, end, .. } => {
                    assert!((r - 40.0).abs() < 0.05, "radius {r}");
                    let d = (*end - Point::new(50.0, 50.0)).hypot();
                    assert!((d - 40.0).abs() < 0.05, "end point off circle by {}", d - 40.0);
                }
                other => panic!("{other:?}"),
            }
        }
        // The arcs trace the circle: area and winding of the sampled loop.
        let mut path = Path::new();
        path.move_to(start);
        for e in &els {
            path.push(*e);
        }
        path.close_path();
        let area = path.area();
        let want = crate::contour::signed_area2(&pts) / 2.0;
        assert!((area - want).abs() < 20.0, "area {area} vs {want}");
    }

    #[test]
    fn arc_fits_a_quarter_circle_and_rejects_a_bend() {
        let r = 30.0;
        let pts: Vec<Point> = (0..=60)
            .map(|i| {
                let t = i as f64 / 60.0 * std::f64::consts::FRAC_PI_2;
                Point::new(10.0 + r * t.cos(), 10.0 + r * t.sin())
            })
            .collect();
        let src = PolySource::new(&pts, false, 1.0);
        let arc = try_arc(&src, 0, src.len() - 1, 0.1).expect("arc");
        match arc {
            Prim::Arc { r: rr, large, end, .. } => {
                assert!((rr - r).abs() < 0.05 && !large && (end - pts[60]).hypot() < 1e-9, "{arc:?}");
            }
            _ => panic!("{arc:?}"),
        }
        // Rendered through cubics, the arc passes through the middle sample.
        let mut path = Path::new();
        path.move_to(pts[0]);
        path.push(arc.el());
        let bez = path.to_bez();
        let mid = pts[30];
        let near = bez.segments().map(|s| kurbo::ParamCurveNearest::nearest(&s, mid, 0.01).distance_sq.sqrt()).fold(f64::MAX, f64::min);
        assert!(near < 0.05, "arc misses the middle sample by {near}");
        // An L is not an arc.
        let l: Vec<Point> = (0..10).map(|i| Point::new(i as f64, 0.0)).chain((1..10).map(|i| Point::new(9.0, i as f64))).collect();
        let src = PolySource::new(&l, false, 1.0);
        assert!(try_arc(&src, 0, src.len() - 1, 0.3).is_none());
    }

    #[test]
    fn hairpin_terminates_and_splits() {
        // A 1 px line whose polygon lost its far end: the run doubles back.
        let mut pts: Vec<Point> = (0..30).map(|i| Point::new(i as f64 + 0.5, 0.0)).collect();
        pts.push(Point::new(30.0, 0.5));
        pts.extend((0..30).rev().map(|i| Point::new(i as f64 + 0.5, 1.0)));
        let o = FitOpts { accuracy: 0.3, snap_lines: true, window: 1.0 };
        let (_, els) = fit_run(&pts, false, &o);
        assert!(!els.is_empty() && els.len() <= 6, "{els:?}");
    }

    #[test]
    fn closed_rectangle_splits_at_its_corners() {
        // Rectangle sampled every half pixel, starting mid-side.
        let mut pts = Vec::new();
        for i in 0..40 {
            pts.push(Point::new(10.0 + i as f64 * 0.5, 0.0));
        }
        for i in 0..20 {
            pts.push(Point::new(30.0, i as f64 * 0.5));
        }
        for i in 0..60 {
            pts.push(Point::new(30.0 - i as f64 * 0.5, 10.0));
        }
        for i in 0..20 {
            pts.push(Point::new(0.0, 10.0 - i as f64 * 0.5));
        }
        for i in 0..20 {
            pts.push(Point::new(i as f64 * 0.5, 0.0));
        }
        let o = FitOpts { accuracy: 0.3, snap_lines: true, window: 1.0 };
        let (start, els) = fit_run(&pts, true, &o);
        assert_eq!(start, Point::new(30.0, 0.0));
        assert_eq!(els.len(), 4, "{els:?}");
        assert!(els.iter().all(|e| matches!(e, El::LineTo(_))), "{els:?}");
    }
}
