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
use kurbo::{fit_to_cubic, BezPath, CubicBez, CurveFitSample, ParamCurveFit, ParamCurveNearest, PathEl, Point, Vec2};
use std::ops::Range;

/// Turning angle, in degrees over one tangent window, above which a point
/// inside a run is treated as a corner.
pub const CUSP_DEG: f64 = 60.0;
/// Samples closer than this to a corner vertex are not fitted, px.
pub const CORNER_CLEAR: f64 = 1.5;
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
fn fit_is_sane(els: &[PathEl], samples: &[Point], accuracy: f64) -> bool {
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
        PathEl::CurveTo(a, b, p) => bb.contains(a) && bb.contains(b) && bb.contains(p),
        PathEl::QuadTo(a, p) => bb.contains(a) && bb.contains(p),
        PathEl::LineTo(p) => bb.contains(p),
        _ => true,
    })
}

/// Last resort: Douglas-Peucker polyline simplification at `tol`, as lines.
fn simplify_lines(samples: &[Point], tol: f64, out: &mut Vec<PathEl>) {
    fn rec(s: &[Point], tol: f64, out: &mut Vec<PathEl>) {
        if s.len() < 3 {
            out.push(PathEl::LineTo(*s.last().unwrap()));
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
            out.push(PathEl::LineTo(b));
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
    if !fit_is_sane(&[PathEl::CurveTo(c.p1, c.p2, c.p3)], pts, accuracy) {
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

/// Greedy segmentation of samples `lo..=hi`: each cubic is extended as far
/// as a single fit stays within accuracy, found by exponential then binary
/// search over sample indices. A few segments more than optimal in the
/// worst case, and never slow or stuck, which kurbo's own optimiser can be.
fn fit_greedy(src: &PolySource, lo: usize, hi: usize, accuracy: f64, out: &mut Vec<PathEl>) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut a = lo;
    while a < hi {
        let mut ok = a + 1;
        let mut best: Option<CubicBez> = None;
        let mut fail: Option<usize> = None;
        let mut step = 1;
        loop {
            let e = (ok + step).min(hi);
            match try_fit(src, a, e, accuracy) {
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
                match try_fit(src, a, m, accuracy) {
                    Some(c) => {
                        ok = m;
                        best = Some(c);
                    }
                    None => f = m,
                }
            }
        }
        match best {
            Some(c) => out.push(PathEl::CurveTo(c.p1, c.p2, c.p3)),
            None => out.push(PathEl::LineTo(src.pts[ok])),
        }
        ends.push(ok);
        a = ok;
    }
    ends
}

/// Elements (no `MoveTo`) approximating an open run with no corners inside.
fn fit_open(samples: &[Point], o: &FitOpts) -> Vec<PathEl> {
    if samples.len() < 2 {
        return Vec::new();
    }
    let last = *samples.last().unwrap();
    if samples.len() == 2 {
        return vec![PathEl::LineTo(last)];
    }
    if o.snap_lines {
        if let Some(dev) = chord_deviation(samples) {
            if dev <= o.accuracy {
                return vec![PathEl::LineTo(last)];
            }
        }
    }
    let src = PolySource::new(samples, false, o.window);
    if src.len() < 3 {
        return vec![PathEl::LineTo(last)];
    }
    let mut els = Vec::new();
    fit_greedy(&src, 0, src.len() - 1, o.accuracy, &mut els);
    if fit_is_sane(&els, samples, o.accuracy) {
        return els;
    }
    let mut out = Vec::new();
    simplify_lines(samples, o.accuracy, &mut out);
    out
}

/// Start point and elements (no `MoveTo`) approximating a closed run with
/// no corners. The greedy pass pays for its seam with up to two extra
/// segments, so a second pass starts in the middle of the longest segment
/// of the first, where the curve is easy, and the shorter result wins.
fn fit_closed(samples: &[Point], o: &FitOpts) -> (Point, Vec<PathEl>) {
    let src = PolySource::new(samples, true, o.window);
    let n = src.len() - 1; // without the closing duplicate
    let mut els = Vec::new();
    let ends = fit_greedy(&src, 0, n, o.accuracy, &mut els);
    let mut best = (src.pts[0], els);
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
        let mid = (la + lb) / 2;
        let mut rotated: Vec<Point> = src.pts[..n].to_vec();
        rotated.rotate_left(mid);
        let src2 = PolySource::new(&rotated, true, o.window);
        let mut els2 = Vec::new();
        fit_greedy(&src2, 0, src2.len() - 1, o.accuracy, &mut els2);
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
pub fn fit_run(samples: &[Point], closed: bool, o: &FitOpts) -> (Point, Vec<PathEl>) {
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
pub fn append_loop(path: &mut BezPath, c: &Contour, verts: &[Point], po: &[usize], corner: &[bool], o: &FitOpts) {
    let n = c.pts.len();
    let m = verts.len();
    if n == 0 || m == 0 {
        return;
    }
    // Two polygon vertices can land on the same anti-aliased corner, a pixel
    // apart and both sharp; keep the first, or the run between them becomes
    // a hook.
    let mut corners: Vec<usize> = Vec::new();
    for i in (0..m).filter(|&i| corner[i]) {
        if let Some(&prev) = corners.last() {
            if (verts[i] - verts[prev]).hypot() <= CORNER_CLEAR {
                continue;
            }
        }
        corners.push(i);
    }
    if corners.len() >= 2 {
        let (f, l) = (corners[0], *corners.last().unwrap());
        if f != l && (verts[f] - verts[l]).hypot() <= CORNER_CLEAR {
            corners.pop();
        }
    }
    let mut els_all: Vec<PathEl> = Vec::new();
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
    if let Some(PathEl::LineTo(p)) = els_all.last() {
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
        assert_eq!(els, vec![PathEl::LineTo(Point::new(19.0, 0.01))]);
        // Without snapping it is still one segment, a straight cubic.
        let o = FitOpts { snap_lines: false, ..o };
        let (_, els) = fit_run(&pts, false, &o);
        assert_eq!(els.len(), 1, "{els:?}");
        match els[0] {
            PathEl::CurveTo(a, b, p) => {
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
        assert_eq!(start, pts[0]);
        assert!(els.len() <= 6, "{} segments", els.len());
        for e in &els {
            if let PathEl::CurveTo(_, _, p) = e {
                let d = (*p - Point::new(50.0, 50.0)).hypot();
                assert!((d - 40.0).abs() < 0.05, "end point off circle by {}", d - 40.0);
            }
        }
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
        assert!(els.iter().all(|e| matches!(e, PathEl::LineTo(_))), "{els:?}");
    }
}
