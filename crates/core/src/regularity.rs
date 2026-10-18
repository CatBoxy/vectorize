//! Regularity (PLAN.md M4; PolyFit, Dominici et al. 2020 §5): small moves of
//! a shape's anchor points so that configurations which are *nearly* regular
//! become exactly regular. A designer draws horizontal edges horizontal,
//! parallel edges parallel, mirrored corners mirrored; a tracer that keeps
//! the 0.3 px tilt the raster happened to measure looks traced.
//!
//! The anchors are the corner vertices the fitter will use as run end
//! points: the corners of every polygon plus the junctions of the boundary
//! graph (shared by every chain that ends there). The *lines* are the runs
//! between consecutive anchors that are already straight within the fit
//! accuracy. Every rule proposes new anchor positions and keeps them only if
//! no anchor moves further than `max_move` from where it was measured and
//! every affected line still passes within the fit accuracy of its own
//! sub-pixel samples: regularity never costs accuracy.
//!
//! An anchor where a curve ends may only move along that curve's tangent,
//! so the curve gets a little longer or shorter but never a kink (a kink
//! would cost the fitter an extra segment, and regularity must not cost
//! nodes either). An anchor between two curves with different tangents, or
//! on two axis-aligned lines, cannot move at all.
//!
//! Rules, in order (each one is a function and has a unit test):
//! 1. [`axis_align`]: lines of at least `min_len` within `angle` of
//!    horizontal or vertical become exactly axis-aligned.
//! 2. [`continue_through_junctions`]: a line that ends at a junction and a
//!    line that leaves it within `angle` of the same direction are made
//!    collinear by moving the junction onto their common line.
//! 3. [`parallel`]: lines of one shape whose directions agree within `angle`
//!    are rotated to their length-weighted mean direction.
//! 4. [`equal_length`]: lines of one shape whose lengths agree within
//!    `rel_len` are set to their mean length.
//! 5. [`mirror`]: when every anchor of a shape has a partner across a
//!    vertical or horizontal axis (or lies on it) within `max_move`, the
//!    pairs are made exactly symmetric.

use kurbo::{Point, Vec2};

/// A straight run between two anchors, with the sub-pixel samples it must
/// keep passing through.
#[derive(Clone, Debug)]
pub struct Line {
    pub a: usize,
    pub b: usize,
    pub samples: Vec<Point>,
    /// Set by [`axis_align`]: 0 horizontal, 1 vertical.
    pub axis: Option<u8>,
}

/// Anchors, lines and shapes of one image.
#[derive(Clone, Debug, Default)]
pub struct Reg {
    /// Current anchor positions.
    pub pos: Vec<Point>,
    /// Measured anchor positions; no anchor strays further than `max_move`.
    pub orig: Vec<Point>,
    /// Anchors that are junctions of the boundary graph.
    pub junction: Vec<bool>,
    /// Tangent directions of the curves ending at each anchor (unit vectors,
    /// orientation irrelevant).
    pub tangents: Vec<Vec<Vec2>>,
    pub lines: Vec<Line>,
    /// Anchor cycles, one per shape (face or loop), in boundary order.
    pub shapes: Vec<Vec<usize>>,
}

#[derive(Clone, Copy, Debug)]
pub struct RegOpts {
    /// Fit accuracy: every line must stay within this of its samples, px.
    pub accuracy: f64,
    /// Furthest an anchor may move from its measured position, px.
    pub max_move: f64,
    /// Lines shorter than this are not regularised, px.
    pub min_len: f64,
    /// Directions closer than this are "the same", radians.
    pub angle: f64,
    /// Lengths closer than this fraction are "the same".
    pub rel_len: f64,
}

/// Angle tolerance at full strength, degrees.
pub const ANGLE_DEG: f64 = 6.0;
/// Anchor move budget at full strength, in units of the fit accuracy.
pub const MOVE_PER_ACCURACY: f64 = 2.0;
/// PolyFit's minimum length for axis alignment, px.
pub const MIN_LEN: f64 = 2.0;
/// Relative length tolerance at full strength.
pub const REL_LEN: f64 = 0.08;

impl RegOpts {
    /// Options for a user `strength` in `0..=1` (0 disables every rule).
    pub fn from_strength(strength: f64, accuracy: f64) -> Self {
        let s = strength.clamp(0.0, 1.0);
        Self {
            accuracy,
            max_move: MOVE_PER_ACCURACY * accuracy * s,
            min_len: MIN_LEN,
            angle: (ANGLE_DEG * s).to_radians(),
            rel_len: REL_LEN * s,
        }
    }

    pub fn enabled(&self) -> bool {
        self.max_move > 0.0 && self.angle > 0.0
    }
}

/// Directions an anchor may move in: a basis of zero, one or two unit
/// vectors.
#[derive(Clone, Debug, PartialEq)]
pub struct Freedom(pub Vec<Vec2>);

impl Freedom {
    fn full() -> Self {
        Freedom(vec![Vec2::new(1.0, 0.0), Vec2::new(0.0, 1.0)])
    }
    /// Restrict to the line along `d` (unit).
    fn along(&self, d: Vec2) -> Self {
        match self.0.len() {
            2 => Freedom(vec![d]),
            1 => {
                if self.0[0].cross(d).abs() < 1e-6 { self.clone() } else { Freedom(Vec::new()) }
            }
            _ => Freedom(Vec::new()),
        }
    }
    /// Project a move onto the allowed directions.
    fn project(&self, m: Vec2) -> Vec2 {
        self.0.iter().map(|b| *b * b.dot(m)).fold(Vec2::ZERO, |a, b| a + b)
    }
}

impl Reg {
    pub fn add_anchor(&mut self, p: Point, junction: bool) -> usize {
        self.pos.push(p);
        self.orig.push(p);
        self.junction.push(junction);
        self.tangents.push(Vec::new());
        self.pos.len() - 1
    }

    pub fn add_line(&mut self, a: usize, b: usize, samples: Vec<Point>) -> usize {
        self.lines.push(Line { a, b, samples, axis: None });
        self.lines.len() - 1
    }

    /// A curve ends at anchor `k` heading along `t` (any orientation): the
    /// anchor may only move along it.
    pub fn add_curve_end(&mut self, k: usize, t: Vec2) {
        let n = t.hypot();
        if n > 1e-9 {
            self.tangents[k].push(t / n);
        }
    }

    fn len(&self, l: &Line) -> f64 {
        (self.pos[l.b] - self.pos[l.a]).hypot()
    }

    fn dir(&self, l: &Line) -> Vec2 {
        let d = self.pos[l.b] - self.pos[l.a];
        let n = d.hypot();
        if n > 1e-12 { d / n } else { Vec2::ZERO }
    }

    /// Which directions anchor `k` may still move in: an axis-aligned line
    /// through it pins the coordinate across the line, a curve ending there
    /// pins the move to the curve's tangent.
    fn freedom(&self, k: usize) -> Freedom {
        let mut f = Freedom::full();
        for l in &self.lines {
            if l.a == k || l.b == k {
                match l.axis {
                    Some(0) => f = f.along(Vec2::new(1.0, 0.0)),
                    Some(_) => f = f.along(Vec2::new(0.0, 1.0)),
                    None => {}
                }
            }
        }
        for t in &self.tangents[k] {
            f = f.along(*t);
        }
        f
    }

    /// Would moving these anchors keep every one within budget and within
    /// its freedom, every line through them within accuracy of its samples,
    /// and every axis-aligned line through them axis-aligned? Applies the
    /// move when so and reports whether it did.
    fn try_move(&mut self, moves: &[(usize, Point)], o: &RegOpts) -> bool {
        for &(k, p) in moves {
            if !p.x.is_finite() || !p.y.is_finite() || (p - self.orig[k]).hypot() > o.max_move + 1e-9 {
                return false;
            }
            let d = p - self.pos[k];
            if (self.freedom(k).project(d) - d).hypot() > 1e-7 {
                return false;
            }
        }
        let old: Vec<(usize, Point)> = moves.iter().map(|&(k, _)| (k, self.pos[k])).collect();
        for &(k, p) in moves {
            self.pos[k] = p;
        }
        let touched: Vec<usize> = moves.iter().map(|m| m.0).collect();
        let ok = self.lines.iter().filter(|l| touched.contains(&l.a) || touched.contains(&l.b)).all(|l| {
            let aligned = match l.axis {
                Some(0) => (self.pos[l.a].y - self.pos[l.b].y).abs() < 1e-7,
                Some(_) => (self.pos[l.a].x - self.pos[l.b].x).abs() < 1e-7,
                None => true,
            };
            aligned && self.line_fits(l, o)
        });
        if !ok {
            for (k, p) in old {
                self.pos[k] = p;
            }
        }
        ok
    }

    /// Smallest move of `anchors`, each within its freedom, that satisfies
    /// the linear constraint `Σ grad_k · Δ_k = resid` (the gradient of the
    /// constraint with respect to each anchor's position), applied through
    /// [`try_move`](Self::try_move).
    fn solve(&mut self, anchors: &[usize], grad: &[Vec2], resid: f64, o: &RegOpts) -> bool {
        let bases: Vec<Freedom> = anchors.iter().map(|&k| self.freedom(k)).collect();
        let mut g: Vec<Vec<f64>> = Vec::with_capacity(anchors.len());
        let mut norm2 = 0.0;
        for (b, gr) in bases.iter().zip(grad) {
            let row: Vec<f64> = b.0.iter().map(|v| v.dot(*gr)).collect();
            norm2 += row.iter().map(|x| x * x).sum::<f64>();
            g.push(row);
        }
        if norm2 < 1e-12 {
            return false;
        }
        let moves: Vec<(usize, Point)> = anchors
            .iter()
            .enumerate()
            .map(|(i, &k)| {
                let d = bases[i].0.iter().zip(&g[i]).map(|(v, gi)| *v * (gi * resid / norm2)).fold(Vec2::ZERO, |a, b| a + b);
                (k, self.pos[k] + d)
            })
            .collect();
        self.try_move(&moves, o)
    }

    /// Every sample within `accuracy` of the segment between the line's
    /// anchors (perpendicular distance; the samples never reach the ends).
    fn line_fits(&self, l: &Line, o: &RegOpts) -> bool {
        let (a, b) = (self.pos[l.a], self.pos[l.b]);
        let d = b - a;
        let len = d.hypot();
        if len < 1e-9 {
            return l.samples.is_empty();
        }
        l.samples.iter().all(|&s| ((s - a).cross(d) / len).abs() <= o.accuracy)
    }

    /// Lines with both ends in `shape`, by index.
    fn lines_of(&self, shape: &[usize]) -> Vec<usize> {
        (0..self.lines.len())
            .filter(|&i| shape.contains(&self.lines[i].a) && shape.contains(&self.lines[i].b))
            .collect()
    }

    /// Apply every rule in order.
    pub fn regularize(&mut self, o: &RegOpts) {
        if !o.enabled() || self.pos.is_empty() {
            return;
        }
        // `VECTORIZE_REG_SKIP=axis,junction,parallel,equal,mirror` turns
        // rules off for diagnosis.
        let skip = std::env::var("VECTORIZE_REG_SKIP").unwrap_or_default();
        if !skip.contains("axis") {
            axis_align(self, o);
        }
        if !skip.contains("junction") {
            continue_through_junctions(self, o);
        }
        if !skip.contains("parallel") {
            parallel(self, o);
        }
        if !skip.contains("equal") {
            equal_length(self, o);
        }
        if !skip.contains("mirror") {
            mirror(self, o);
        }
    }
}

/// Angle between two directions ignoring orientation, radians in `0..=π/2`.
fn line_angle(u: Vec2, v: Vec2) -> f64 {
    u.cross(v).abs().atan2(u.dot(v).abs())
}

/// Rule 1: nearly horizontal or vertical lines become exactly so. With both
/// ends free each takes the mean coordinate; a constrained end moves what
/// it can and the other end the rest.
pub fn axis_align(r: &mut Reg, o: &RegOpts) {
    for i in 0..r.lines.len() {
        let l = r.lines[i].clone();
        if r.len(&l) < o.min_len || l.axis.is_some() {
            continue;
        }
        let d = r.dir(&l);
        let (a, b) = (r.pos[l.a], r.pos[l.b]);
        let ok = if d.y.abs() <= o.angle.sin() {
            // a.y − b.y = 0
            r.solve(&[l.a, l.b], &[Vec2::new(0.0, 1.0), Vec2::new(0.0, -1.0)], b.y - a.y, o).then_some(0u8)
        } else if d.x.abs() <= o.angle.sin() {
            r.solve(&[l.a, l.b], &[Vec2::new(1.0, 0.0), Vec2::new(-1.0, 0.0)], b.x - a.x, o).then_some(1u8)
        } else {
            None
        };
        if let Some(ax) = ok {
            r.lines[i].axis = Some(ax);
        }
    }
}

/// Rule 2: at a junction where one line arrives and another leaves in
/// nearly the same direction, move the junction onto the line between the
/// two far ends so the edge continues straight through it.
pub fn continue_through_junctions(r: &mut Reg, o: &RegOpts) {
    for j in 0..r.pos.len() {
        if !r.junction[j] {
            continue;
        }
        let incident: Vec<usize> = (0..r.lines.len()).filter(|&i| r.lines[i].a == j || r.lines[i].b == j).collect();
        if incident.len() < 2 {
            continue;
        }
        let far = |i: usize| if r.lines[i].a == j { r.lines[i].b } else { r.lines[i].a };
        // The straightest pair.
        let mut best: Option<(f64, usize, usize)> = None;
        for x in 0..incident.len() {
            for y in x + 1..incident.len() {
                let (p, q) = (r.pos[far(incident[x])], r.pos[far(incident[y])]);
                let (u, v) = (r.pos[j] - p, q - r.pos[j]);
                if u.hypot() < o.min_len || v.hypot() < o.min_len {
                    continue;
                }
                let turn = u.cross(v).atan2(u.dot(v)).abs();
                if turn <= o.angle && best.map_or(true, |(t, _, _)| turn < t) {
                    best = Some((turn, far(incident[x]), far(incident[y])));
                }
            }
        }
        let Some((_, pa, pb)) = best else { continue };
        let (p, q) = (r.pos[pa], r.pos[pb]);
        let d = q - p;
        let len = d.hypot();
        if len < 1e-9 {
            continue;
        }
        // (J − p) × d = 0: gradient with respect to J is (d.y, −d.x) / len.
        let nrm = Vec2::new(d.y, -d.x) / len;
        let resid = -(r.pos[j] - p).cross(d) / len;
        r.solve(&[j], &[nrm], resid, o);
    }
}

/// Rule 3: within a shape, lines whose directions agree within `angle` are
/// rotated to the length-weighted mean direction. Axis-aligned lines anchor
/// their group and do not move.
pub fn parallel(r: &mut Reg, o: &RegOpts) {
    for s in 0..r.shapes.len() {
        let shape = r.shapes[s].clone();
        let ls: Vec<usize> = r.lines_of(&shape).into_iter().filter(|&i| r.len(&r.lines[i]) >= o.min_len).collect();
        let mut done = vec![false; ls.len()];
        for x in 0..ls.len() {
            if done[x] {
                continue;
            }
            let dx = r.dir(&r.lines[ls[x]]);
            let mut group = vec![x];
            for y in x + 1..ls.len() {
                if !done[y] && line_angle(dx, r.dir(&r.lines[ls[y]])) <= o.angle {
                    group.push(y);
                }
            }
            if group.len() < 2 {
                continue;
            }
            for &g in &group {
                done[g] = true;
            }
            // Mean direction: an axis-aligned member fixes it, else the
            // length-weighted mean of the doubled angles.
            let target = match group.iter().find_map(|&g| r.lines[ls[g]].axis) {
                Some(0) => Vec2::new(1.0, 0.0),
                Some(_) => Vec2::new(0.0, 1.0),
                None => {
                    let (mut sx, mut sy) = (0.0, 0.0);
                    for &g in &group {
                        let l = &r.lines[ls[g]];
                        let d = r.dir(l);
                        let w = r.len(l);
                        let th = 2.0 * d.y.atan2(d.x);
                        sx += w * th.cos();
                        sy += w * th.sin();
                    }
                    let th = 0.5 * sy.atan2(sx);
                    Vec2::new(th.cos(), th.sin())
                }
            };
            for &g in &group {
                let l = r.lines[ls[g]].clone();
                if l.axis.is_some() {
                    continue;
                }
                // (b − a) × target = 0: gradients (−t.y, t.x) at a, (t.y, −t.x) at b.
                let resid = -(r.pos[l.b] - r.pos[l.a]).cross(target);
                r.solve(&[l.a, l.b], &[Vec2::new(-target.y, target.x), Vec2::new(target.y, -target.x)], resid, o);
            }
        }
    }
}

/// Rule 4: within a shape, lines whose lengths agree within `rel_len` take
/// their mean length, each end moving along the line as far as it may.
pub fn equal_length(r: &mut Reg, o: &RegOpts) {
    if o.rel_len <= 0.0 {
        return;
    }
    for s in 0..r.shapes.len() {
        let shape = r.shapes[s].clone();
        let ls: Vec<usize> = r.lines_of(&shape).into_iter().filter(|&i| r.len(&r.lines[i]) >= o.min_len).collect();
        let mut done = vec![false; ls.len()];
        for x in 0..ls.len() {
            if done[x] {
                continue;
            }
            let lx = r.len(&r.lines[ls[x]]);
            let mut group = vec![x];
            for y in x + 1..ls.len() {
                let ly = r.len(&r.lines[ls[y]]);
                if !done[y] && (lx - ly).abs() <= o.rel_len * lx.max(ly) {
                    group.push(y);
                }
            }
            if group.len() < 2 {
                continue;
            }
            for &g in &group {
                done[g] = true;
            }
            let mean = group.iter().map(|&g| r.len(&r.lines[ls[g]])).sum::<f64>() / group.len() as f64;
            for &g in &group {
                let l = r.lines[ls[g]].clone();
                let d = r.dir(&l);
                // (b − a) · d = mean: gradients −d at a, d at b.
                let resid = mean - r.len(&l);
                r.solve(&[l.a, l.b], &[-d, d], resid, o);
            }
        }
    }
}

/// Rule 5: mirror symmetry about a vertical or horizontal axis through the
/// middle of a shape's anchors. Applied when every anchor finds a partner
/// (or sits on the axis) within `max_move` and every move is one its
/// anchor may make; each pair is then moved to the mirror image of its mean.
pub fn mirror(r: &mut Reg, o: &RegOpts) {
    for s in 0..r.shapes.len() {
        let shape: Vec<usize> = {
            let mut v = r.shapes[s].clone();
            v.sort_unstable();
            v.dedup();
            v
        };
        if shape.len() < 3 {
            continue;
        }
        let pts: Vec<Point> = shape.iter().map(|&k| r.pos[k]).collect();
        let (mut lo, mut hi) = (pts[0], pts[0]);
        for p in &pts {
            lo = Point::new(lo.x.min(p.x), lo.y.min(p.y));
            hi = Point::new(hi.x.max(p.x), hi.y.max(p.y));
        }
        let cen = lo.midpoint(hi);
        let avg = Point::new(pts.iter().map(|p| p.x).sum::<f64>() / pts.len() as f64, pts.iter().map(|p| p.y).sum::<f64>() / pts.len() as f64);
        // Candidate axes: x = c (vertical) and y = c (horizontal), through
        // the bounding box centre or the mean of the anchors.
        for (vertical, c) in [(true, cen.x), (true, avg.x), (false, cen.y), (false, avg.y)] {
            let refl = |p: Point| if vertical { Point::new(2.0 * c - p.x, p.y) } else { Point::new(p.x, 2.0 * c - p.y) };
            let tol = 2.0 * o.max_move;
            let mut partner = vec![usize::MAX; shape.len()];
            let mut ok = true;
            for i in 0..shape.len() {
                let m = refl(pts[i]);
                let mut best: Option<(f64, usize)> = None;
                for j in 0..shape.len() {
                    let d = (pts[j] - m).hypot();
                    if d <= tol && best.map_or(true, |(bd, _)| d < bd) {
                        best = Some((d, j));
                    }
                }
                match best {
                    Some((_, j)) => partner[i] = j,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok || partner.iter().enumerate().any(|(i, &j)| partner[j] != i) {
                continue;
            }
            // Already exact? Then nothing to do.
            let worst = (0..shape.len()).map(|i| (refl(pts[i]) - pts[partner[i]]).hypot()).fold(0.0, f64::max);
            if worst < 1e-6 {
                break;
            }
            let mut moves: Vec<(usize, Point)> = Vec::new();
            for i in 0..shape.len() {
                let j = partner[i];
                if j < i {
                    continue;
                }
                if j == i {
                    moves.push((shape[i], if vertical { Point::new(c, pts[i].y) } else { Point::new(pts[i].x, c) }));
                } else {
                    let m = pts[i].midpoint(refl(pts[j]));
                    moves.push((shape[i], m));
                    moves.push((shape[j], refl(m)));
                }
            }
            if r.try_move(&moves, o) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> RegOpts {
        RegOpts::from_strength(1.0, 0.35)
    }

    /// Samples along `a → b`, excluding the ends.
    fn samples(a: Point, b: Point) -> Vec<Point> {
        (1..10).map(|i| a.lerp(b, i as f64 / 10.0)).collect()
    }

    fn quad(r: &mut Reg, pts: [Point; 4], junction: bool) -> Vec<usize> {
        let ids: Vec<usize> = pts.iter().map(|&p| r.add_anchor(p, junction)).collect();
        for i in 0..4 {
            let (a, b) = (ids[i], ids[(i + 1) % 4]);
            r.add_line(a, b, samples(pts[i], pts[(i + 1) % 4]));
        }
        r.shapes.push(ids.clone());
        ids
    }

    #[test]
    fn axis_align_snaps_slightly_tilted_edges_only() {
        let mut r = Reg::default();
        // A 20 px square tilted by 0.3 px at one corner.
        let ids = quad(&mut r, [Point::new(0.0, 0.0), Point::new(20.0, 0.3), Point::new(20.0, 20.0), Point::new(0.0, 20.0)], false);
        axis_align(&mut r, &opts());
        assert!((r.pos[ids[0]].y - r.pos[ids[1]].y).abs() < 1e-9, "{:?}", r.pos);
        assert!((r.pos[ids[1]].x - r.pos[ids[2]].x).abs() < 1e-9, "{:?}", r.pos);
        assert_eq!(r.lines.iter().filter(|l| l.axis.is_some()).count(), 4);
        // A 45° edge is left alone, and so is a 1 px line (below min_len).
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(10.0, 10.0), false);
        r.add_line(a, b, samples(Point::new(0.0, 0.0), Point::new(10.0, 10.0)));
        let c = r.add_anchor(Point::new(20.0, 0.0), false);
        let d = r.add_anchor(Point::new(21.0, 0.2), false);
        r.add_line(c, d, Vec::new());
        axis_align(&mut r, &opts());
        assert_eq!(r.pos[b], Point::new(10.0, 10.0));
        assert_eq!(r.pos[d], Point::new(21.0, 0.2));
        // A tilt the samples contradict is refused: the samples lie on the
        // tilted line, 1.5 px off the horizontal at the far end.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(30.0, 1.5), false);
        r.add_line(a, b, samples(Point::new(0.0, 0.0), Point::new(30.0, 1.5)));
        axis_align(&mut r, &opts());
        assert_eq!(r.pos[b], Point::new(30.0, 1.5));
    }

    #[test]
    fn curve_ends_only_move_along_their_tangent() {
        // A horizontal-ish line whose right end is also the start of a
        // curve heading straight down: that end may move only vertically,
        // which is exactly what aligning the line needs.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(20.0, 0.3), false);
        r.add_line(a, b, Vec::new());
        r.add_curve_end(b, Vec2::new(0.0, 1.0));
        axis_align(&mut r, &opts());
        assert!((r.pos[a].y - r.pos[b].y).abs() < 1e-9, "{:?}", r.pos);
        assert_eq!(r.pos[b].x, 20.0);
        // A curve leaving along the line's own direction pins the end
        // across the line: only the free end moves.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(20.0, 0.3), false);
        r.add_line(a, b, Vec::new());
        r.add_curve_end(b, Vec2::new(1.0, 0.0));
        axis_align(&mut r, &opts());
        assert_eq!(r.pos[b], Point::new(20.0, 0.3));
        assert!((r.pos[a].y - 0.3).abs() < 1e-9, "{:?}", r.pos);
        // Two curves with different tangents pin the anchor completely; a
        // horizontal tangent at the other end cannot fix the y either.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(20.0, 0.3), false);
        r.add_line(a, b, Vec::new());
        r.add_curve_end(b, Vec2::new(0.0, 1.0));
        r.add_curve_end(b, Vec2::new(1.0, 1.0));
        r.add_curve_end(a, Vec2::new(1.0, 0.0));
        axis_align(&mut r, &opts());
        assert_eq!(r.pos[b], Point::new(20.0, 0.3));
        assert_eq!(r.pos[a], Point::new(0.0, 0.0));
        // A diagonal tangent at the free end does let it slide into line.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(20.0, 0.3), false);
        r.add_line(a, b, Vec::new());
        r.add_curve_end(b, Vec2::new(0.0, 1.0));
        r.add_curve_end(b, Vec2::new(1.0, 1.0));
        r.add_curve_end(a, Vec2::new(1.0, 1.0));
        axis_align(&mut r, &opts());
        assert_eq!(r.pos[b], Point::new(20.0, 0.3));
        assert!((r.pos[a] - Point::new(0.3, 0.3)).hypot() < 1e-9, "{:?}", r.pos[a]);
    }

    #[test]
    fn junction_continuation_straightens_a_line_through_a_junction() {
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let j = r.add_anchor(Point::new(10.0, 0.2), true);
        let b = r.add_anchor(Point::new(20.0, 0.0), false);
        let c = r.add_anchor(Point::new(10.0, 15.0), false);
        r.add_line(a, j, samples(Point::new(0.0, 0.0), Point::new(10.0, 0.2)));
        r.add_line(j, b, samples(Point::new(10.0, 0.2), Point::new(20.0, 0.0)));
        r.add_line(j, c, samples(Point::new(10.0, 0.2), Point::new(10.0, 15.0)));
        continue_through_junctions(&mut r, &opts());
        assert!((r.pos[j] - Point::new(10.0, 0.0)).hypot() < 1e-9, "{:?}", r.pos[j]);
        // A kink the samples contradict (0.5 px at the junction, so the
        // straightened line would miss them by more than the accuracy) stays.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let j = r.add_anchor(Point::new(10.0, 0.5), true);
        let b = r.add_anchor(Point::new(20.0, 0.0), false);
        r.add_line(a, j, samples(Point::new(0.0, 0.0), Point::new(10.0, 0.5)));
        r.add_line(j, b, samples(Point::new(10.0, 0.5), Point::new(20.0, 0.0)));
        continue_through_junctions(&mut r, &opts());
        assert_eq!(r.pos[j], Point::new(10.0, 0.5));
        // A real bend (10°) at a junction is kept.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let j = r.add_anchor(Point::new(10.0, 0.0), true);
        let b = r.add_anchor(Point::new(20.0, 1.8), false);
        r.add_line(a, j, Vec::new());
        r.add_line(j, b, Vec::new());
        continue_through_junctions(&mut r, &opts());
        assert_eq!(r.pos[j], Point::new(10.0, 0.0));
        // A non-junction corner is never moved.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let j = r.add_anchor(Point::new(10.0, 0.4), false);
        let b = r.add_anchor(Point::new(20.0, 0.0), false);
        r.add_line(a, j, Vec::new());
        r.add_line(j, b, Vec::new());
        continue_through_junctions(&mut r, &opts());
        assert_eq!(r.pos[j], Point::new(10.0, 0.4));
    }

    #[test]
    fn parallel_makes_nearly_parallel_edges_parallel() {
        // A parallelogram-ish quad whose slanted sides differ by 2°.
        let mut r = Reg::default();
        let th = 60f64.to_radians();
        let th2 = 62f64.to_radians();
        let p0 = Point::new(0.0, 20.0);
        let p1 = Point::new(30.0, 20.0);
        let p2 = p1 + Vec2::new(th2.cos(), -th2.sin()) * 10.0;
        let p3 = p0 + Vec2::new(th.cos(), -th.sin()) * 10.0;
        let ids = quad(&mut r, [p0, p1, p2, p3], false);
        // As in the pipeline: axis alignment first locks the two nearly
        // horizontal edges, then the slanted pair is made parallel.
        axis_align(&mut r, &opts());
        parallel(&mut r, &opts());
        let d1 = r.pos[ids[2]] - r.pos[ids[1]];
        let d2 = r.pos[ids[0]] - r.pos[ids[3]];
        assert!(line_angle(d1 / d1.hypot(), d2 / d2.hypot()) < 1e-9, "{:?}", r.pos);
        // Every anchor stayed within the move budget.
        for k in 0..4 {
            assert!((r.pos[k] - r.orig[k]).hypot() <= opts().max_move + 1e-9);
        }
        // The horizontal edges are exactly horizontal.
        assert!((r.pos[ids[0]].y - r.pos[ids[1]].y).abs() < 1e-9);
        assert!((r.pos[ids[2]].y - r.pos[ids[3]].y).abs() < 1e-9);
        // The slanted edges slid along the locked horizontals: their ends
        // kept their y.
        assert!((r.pos[ids[1]].y - 20.0).abs() < 1e-9 && (r.pos[ids[0]].y - 20.0).abs() < 1e-9, "{:?}", r.pos);
        // Without a locked member, a group rotates to its mean direction:
        // two lines 2° apart end up parallel.
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(0.0, 20.0), false);
        let c = r.add_anchor(Point::new(10.0, 0.0), false);
        let d = r.add_anchor(Point::new(10.7, 20.0), false);
        r.add_line(a, b, Vec::new());
        r.add_line(c, d, Vec::new());
        r.shapes.push(vec![a, b, d, c]);
        parallel(&mut r, &opts());
        let (u, v) = (r.pos[b] - r.pos[a], r.pos[d] - r.pos[c]);
        assert!(line_angle(u / u.hypot(), v / v.hypot()) < 1e-9, "{:?}", r.pos);
    }

    #[test]
    fn equal_length_evens_out_nearly_equal_edges() {
        let mut r = Reg::default();
        // Two separate horizontal lines in one shape, 20 and 20.6 px long.
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(20.0, 0.0), false);
        let c = r.add_anchor(Point::new(0.0, 10.0), false);
        let d = r.add_anchor(Point::new(20.6, 10.0), false);
        r.add_line(a, b, Vec::new());
        r.add_line(c, d, Vec::new());
        r.shapes.push(vec![a, b, d, c]);
        equal_length(&mut r, &opts());
        let l1 = (r.pos[b] - r.pos[a]).hypot();
        let l2 = (r.pos[d] - r.pos[c]).hypot();
        assert!((l1 - l2).abs() < 1e-9 && (l1 - 20.3).abs() < 1e-9, "{l1} {l2}");
        // Lengths 20 and 25 are not "equal".
        let mut r = Reg::default();
        let a = r.add_anchor(Point::new(0.0, 0.0), false);
        let b = r.add_anchor(Point::new(20.0, 0.0), false);
        let c = r.add_anchor(Point::new(0.0, 10.0), false);
        let d = r.add_anchor(Point::new(25.0, 10.0), false);
        r.add_line(a, b, Vec::new());
        r.add_line(c, d, Vec::new());
        r.shapes.push(vec![a, b, d, c]);
        equal_length(&mut r, &opts());
        assert_eq!(r.pos[d], Point::new(25.0, 10.0));
    }

    #[test]
    fn mirror_symmetry_snaps_an_almost_symmetric_pentagon() {
        let mut r = Reg::default();
        let pts = [
            Point::new(50.0, 10.0),
            Point::new(80.0, 35.3),
            Point::new(70.0, 70.0),
            Point::new(30.2, 70.0),
            Point::new(20.0, 35.0),
        ];
        let ids: Vec<usize> = pts.iter().map(|&p| r.add_anchor(p, false)).collect();
        r.shapes.push(ids.clone());
        mirror(&mut r, &opts());
        let c = 50.0;
        assert!((r.pos[ids[0]].x - c).abs() < 1e-9, "{:?}", r.pos);
        for (i, j) in [(1, 4), (2, 3)] {
            let (p, q) = (r.pos[ids[i]], r.pos[ids[j]]);
            assert!((p.x + q.x - 2.0 * c).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9, "{p:?} {q:?}");
        }
        // An asymmetric shape is left alone.
        let mut r = Reg::default();
        let pts = [Point::new(0.0, 0.0), Point::new(30.0, 0.0), Point::new(30.0, 10.0), Point::new(10.0, 25.0)];
        let ids: Vec<usize> = pts.iter().map(|&p| r.add_anchor(p, false)).collect();
        r.shapes.push(ids.clone());
        mirror(&mut r, &opts());
        for (k, p) in ids.iter().zip(&pts) {
            assert_eq!(r.pos[*k], *p);
        }
    }

    #[test]
    fn strength_zero_disables_everything() {
        let o = RegOpts::from_strength(0.0, 0.35);
        assert!(!o.enabled());
        let mut r = Reg::default();
        let ids = quad(&mut r, [Point::new(0.0, 0.0), Point::new(20.0, 0.3), Point::new(20.0, 20.0), Point::new(0.0, 20.0)], false);
        r.regularize(&o);
        assert_eq!(r.pos[ids[1]], Point::new(20.0, 0.3));
    }
}
