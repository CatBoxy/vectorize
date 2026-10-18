//! Stage 5: polygon from a closed sub-pixel contour.
//!
//! 1. For each start point find how far a "straight" run extends: a run is
//!    straight if some line passes within `tol` (L∞) of every point. Potrace's
//!    incremental cone test is used with the box half-width scaled to `tol`.
//!    A run that uses all four pixel step directions is never straight; that
//!    rule is what keeps a 1 px line from collapsing onto its own axis.
//! 2. Shortest cycle through straight runs. Two costs are available
//!    ([`PolygonCost`]):
//!    - PolyFit (Dominici et al. 2020, the default, PLAN.md M4): every edge
//!      costs one unit of *simplicity* plus an *accuracy* term (RMS distance
//!      of its points from the segment, relative to `tol`), and every vertex a
//!      *continuity* term (squared turning angle). The states of the shortest
//!      path are edges, not vertices, so the turn between consecutive edges is
//!      known; for a fixed total turn the squared penalty is smallest when the
//!      turns are equal, which is PolyFit's curvature-variation cue without a
//!      three-edge state. Candidate edges are the straight runs; an end point
//!      that does not reach further than its start is skipped on straight
//!      stretches, which keeps the state count linear on long edges.
//!    - Potrace (Selinger 2003 §2.2, `trace.c`): fewest segments, ties broken
//!      by the summed least-squares deviation (`penalty3`). Kept as the
//!      fallback and for comparison.
//! 3. Each polygon vertex moves to the point inside a `vbox` box around it
//!    that is closest to the two neighbouring least-squares lines. That
//!    intersection is where a chamfered corner actually was (Gong and
//!    Newman 2018 recover corners the same way).

use kurbo::{Point, Vec2};

/// Which polygon cost the shortest cycle minimises.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolygonCost {
    /// Simplicity + accuracy + continuity (PolyFit).
    Polyfit,
    /// Fewest segments, then least deviation (Potrace).
    Potrace,
}

/// Options of the polygon stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PolygonOpts {
    /// Straightness tolerance in px (Potrace: 0.5 on pixel corners).
    pub tol: f64,
    /// Half-width of the vertex adjustment box, px.
    pub vbox: f64,
    pub cost: PolygonCost,
    /// Weight of the accuracy term (RMS deviation / `tol`) per edge.
    pub w_accuracy: f64,
    /// Weight of the continuity term (turning angle², radians) per vertex.
    pub w_continuity: f64,
}

impl PolygonOpts {
    pub fn new(tol: f64, vbox: f64) -> Self {
        // `VECTORIZE_POLY_W=acc,cont` overrides the weights for experiments.
        let (w_accuracy, w_continuity) = std::env::var("VECTORIZE_POLY_W")
            .ok()
            .and_then(|v| {
                let (a, c) = v.split_once(',')?;
                Some((a.trim().parse().ok()?, c.trim().parse().ok()?))
            })
            .unwrap_or((W_ACCURACY, W_CONTINUITY));
        Self { tol, vbox, cost: PolygonCost::Polyfit, w_accuracy, w_continuity }
    }
    pub fn potrace(tol: f64, vbox: f64) -> Self {
        Self { cost: PolygonCost::Potrace, ..Self::new(tol, vbox) }
    }
}

/// Default accuracy weight: an edge whose points scatter by the whole
/// tolerance costs half an extra edge.
pub const W_ACCURACY: f64 = 0.5;
/// Default continuity weight (PolyFit's curvature-variation weight): a 90°
/// turn costs 0.62 of an edge, so a real corner is never split in two, but
/// among polygons with the same edge count the one turning evenly wins.
pub const W_CONTINUITY: f64 = 0.25;
/// Most edge-to-edge transitions the PolyFit search may examine for one
/// contour before falling back to the Potrace cost.
pub const POLYFIT_BUDGET: usize = 6_000_000;

/// Polygon for a closed contour with the chosen cost; see [`polygonize`].
pub fn polygonize_with(pts: &[Point], dirs: &[u8], o: &PolygonOpts) -> (Vec<Point>, Vec<usize>) {
    let n = pts.len();
    if n < 3 || dirs.len() != n {
        return (pts.to_vec(), (0..n).collect());
    }
    let sums = calc_sums(pts);
    let lon = calc_lon(pts, dirs, 2.0 * o.tol);
    let mut po = match o.cost {
        PolygonCost::Potrace => best_polygon(pts, &sums, &lon),
        PolygonCost::Polyfit => polyfit_cycle(pts, &sums, &lon, o).unwrap_or_else(|| best_polygon(pts, &sums, &lon)),
    };
    let big = merge_cut_corners(&mut po, pts, &sums, MERGE_CHORD, MERGE_NEAR);
    let verts = adjust_vertices(pts, &sums, &po, o.vbox, &big);
    (verts, po)
}

/// Polygon for an open chain with the chosen cost; see [`polygonize_open`].
pub fn polygonize_open_with(pts: &[Point], dirs: &[u8], o: &PolygonOpts) -> (Vec<Point>, Vec<usize>) {
    let n = pts.len();
    if n < 3 || dirs.len() != n {
        return (pts.to_vec(), (0..n).collect());
    }
    let sums = calc_sums(pts);
    let lon = calc_lon_open(pts, dirs, 2.0 * o.tol);
    let mut po = match o.cost {
        PolygonCost::Potrace => best_polygon_open(pts, &sums, &lon),
        PolygonCost::Polyfit => polyfit_path(pts, &sums, &lon, o).unwrap_or_else(|| best_polygon_open(pts, &sums, &lon)),
    };
    let big = merge_cut_corners_open(&mut po, pts, &sums, MERGE_CHORD, MERGE_NEAR);
    let verts = adjust_vertices_open(pts, &sums, &po, o.vbox, &big);
    (verts, po)
}

/// RMS distance of points `i..=j` from the segment between them (`j` may
/// exceed `n` to wrap), relative to `tol`.
fn accuracy_cost(pts: &[Point], sums: &[Sum], i: usize, j: usize, tol: f64) -> f64 {
    let n = pts.len();
    let jj = if j >= n { j - n } else { j };
    let len = (pts[jj] - pts[i]).hypot();
    if len < 1e-9 {
        return 0.0;
    }
    penalty3(pts, sums, i, j) / len / tol.max(1e-6)
}

/// Turning angle at `b` between `a → b` and `b → c`, radians in `0..=π`.
#[inline]
fn turn_angle(a: Point, b: Point, c: Point) -> f64 {
    let (u, v) = (b - a, c - b);
    u.cross(v).atan2(u.dot(v)).abs()
}

/// Candidate ends of an edge starting at `i`, as unrolled indices in
/// `i+1 ..= i+reach`, where `reach(k)` is how far the straight run from `k`
/// extends. The last point of the run and the next point are always
/// candidates; an interior point is one only when the run starting there
/// reaches further than the run from `i` (otherwise it adds a vertex
/// without extending the polygon; on a long straight edge that leaves one
/// candidate) and lies within [`CANDIDATE_WINDOW`] of the run's end: the
/// simplicity term keeps edges long, so shorter edges only ever serve to
/// even out the turns, and a few points of slack suffice for that.
fn candidates(i: usize, reach: &dyn Fn(usize) -> usize, limit: usize) -> Vec<usize> {
    let r = reach(i).max(1);
    let far = (i + r).min(limit);
    let mut out = Vec::new();
    let lo = (i + 1).max(far.saturating_sub(CANDIDATE_WINDOW));
    for j in lo..far {
        if j + reach(j) > far {
            out.push(j);
        }
    }
    if far > i {
        out.push(far);
    }
    if out.first() != Some(&(i + 1)) && i + 1 <= limit {
        out.insert(0, i + 1);
    }
    out.dedup();
    out
}

/// How many points before the end of a straight run may end an edge.
pub const CANDIDATE_WINDOW: usize = 8;

/// Shortest path over edge states from vertex 0 to vertex `limit` (unrolled
/// index; `limit == n` closes a cycle). `first` restricts the first edge;
/// `closing` adds the turn at the start vertex for a cycle. Returns the
/// vertex indices (unrolled) and the total cost, or `None` when the search
/// would exceed the budget.
fn polyfit_search(
    pts: &[Point],
    sums: &[Sum],
    cand: &[Vec<usize>],
    o: &PolygonOpts,
    limit: usize,
    first: Option<usize>,
    closing: bool,
) -> Option<(Vec<usize>, f64)> {
    let n = pts.len();
    let p = |k: usize| pts[k % n];
    // Edges ending at each vertex.
    let mut rev: Vec<Vec<usize>> = vec![Vec::new(); limit + 1];
    for (i, cs) in cand.iter().enumerate().take(limit) {
        for &j in cs {
            if j <= limit {
                rev[j].push(i);
            }
        }
    }
    // One state per edge `(i, j)`, stored with the vertex `j` it ends at:
    // `(i, cost, slot)`, where `slot` indexes `all`, the flat list of states
    // used for predecessor links.
    struct St {
        from: usize,
        pred: usize,
    }
    let mut all: Vec<St> = Vec::new();
    let mut states: Vec<Vec<(usize, f64, usize)>> = vec![Vec::new(); limit + 1];
    let mut work = 0usize;
    for j in 1..=limit {
        for &i in &rev[j] {
            let edge = 1.0 + o.w_accuracy * accuracy_cost(pts, sums, i, j, o.tol);
            if i == 0 {
                if first.map_or(true, |f| f == j) {
                    all.push(St { from: 0, pred: usize::MAX });
                    states[j].push((0, edge, all.len() - 1));
                }
                continue;
            }
            if states[i].is_empty() {
                continue;
            }
            work += states[i].len();
            if work > POLYFIT_BUDGET {
                return None;
            }
            let mut best: Option<(f64, usize)> = None;
            for &(k, c, slot) in &states[i] {
                let t = turn_angle(p(k), p(i), p(j));
                let cost = c + edge + o.w_continuity * t * t;
                if best.map_or(true, |(bc, _)| cost < bc) {
                    best = Some((cost, slot));
                }
            }
            if let Some((cost, pred)) = best {
                all.push(St { from: i, pred });
                states[j].push((i, cost, all.len() - 1));
            }
        }
    }
    // Best final state, with the closing turn when it is a cycle.
    let first_vertex = |slot: usize| -> usize {
        // The first edge of the path through `slot` ends at the vertex whose
        // state has no predecessor; walk back to it.
        let mut s = slot;
        let mut to = limit;
        loop {
            let st = &all[s];
            if st.pred == usize::MAX {
                return to;
            }
            to = st.from;
            s = st.pred;
        }
    };
    let mut best: Option<(f64, usize)> = None;
    for &(k, c, slot) in &states[limit] {
        let mut cost = c;
        if closing {
            let j0 = first_vertex(slot);
            let t = turn_angle(p(k), p(0), p(j0));
            cost += o.w_continuity * t * t;
        }
        if best.map_or(true, |(bc, _)| cost < bc) {
            best = Some((cost, slot));
        }
    }
    let (cost, mut slot) = best?;
    let mut path = vec![limit];
    loop {
        let st = &all[slot];
        path.push(st.from);
        if st.pred == usize::MAX {
            break;
        }
        slot = st.pred;
    }
    path.reverse();
    Some((path, cost))
}

/// PolyFit-cost polygon of a closed contour: vertex indices starting at 0.
fn polyfit_cycle(pts: &[Point], sums: &[Sum], lon: &[usize], o: &PolygonOpts) -> Option<Vec<usize>> {
    let n = pts.len();
    let reach = |k: usize| -> usize {
        let k = k % n;
        let r = (lon[k] + n - k) % n;
        r.max(1)
    };
    let cand: Vec<Vec<usize>> = (0..n).map(|i| candidates(i, &reach, n)).collect();
    // The turn at vertex 0 between the last and the first edge is not
    // scored: vertex 0 is a real corner of the contour whenever there is
    // one, so that turn is the same for every candidate cycle to within
    // noise, and scoring it exactly would need a second pass per first edge.
    let (path, _) = polyfit_search(pts, sums, &cand, o, n, None, false)?;
    let mut po: Vec<usize> = path.iter().copied().filter(|&v| v < n).collect();
    po.dedup();
    if po.len() < 3 {
        return None;
    }
    Some(po)
}

/// PolyFit-cost polygon of an open chain from point 0 to point `n - 1`.
fn polyfit_path(pts: &[Point], sums: &[Sum], lon: &[usize], o: &PolygonOpts) -> Option<Vec<usize>> {
    let n = pts.len();
    let reach = |k: usize| -> usize { lon[k.min(n - 1)].saturating_sub(k).max(1) };
    let cand: Vec<Vec<usize>> = (0..n - 1).map(|i| candidates(i, &reach, n - 1)).collect();
    let (path, _) = polyfit_search(pts, sums, &cand, o, n - 1, None, false)?;
    Some(path)
}

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
    let big = merge_cut_corners(&mut po, pts, &sums, MERGE_CHORD, MERGE_NEAR);
    let verts = adjust_vertices(pts, &sums, &po, vbox, &big);
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
    let big = merge_cut_corners_open(&mut po, pts, &sums, MERGE_CHORD, MERGE_NEAR);
    let verts = adjust_vertices_open(pts, &sums, &po, vbox, &big);
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
fn merge_cut_corners_open(po: &mut Vec<usize>, pts: &[Point], sums: &[Sum], max_chord: f64, near: f64) -> Vec<bool> {
    let n = pts.len();
    let origin = pts[0];
    let mut big = vec![false; po.len()];
    loop {
        let m = po.len();
        if m <= 3 {
            return big;
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
            let flat = (pts[b] - pts[a]).hypot() >= MERGE_FLAT_MIN && b - a >= 3 && (a + 1..b).all(|k| on_chord(pts[a], pts[b], pts[k], MERGE_FLAT));
            let limit = if flat { near } else { near.max(MERGE_NEAR_TIP) };
            if (x - mid).hypot() <= limit {
                po.remove(i + 1);
                big.remove(i + 1);
                big[i] = true;
                merged = true;
                break;
            }
        }
        if !merged {
            return big;
        }
    }
}

/// `adjust_vertices` for an open polygon: the ends stay where they are.
fn adjust_vertices_open(pts: &[Point], sums: &[Sum], po: &[usize], vbox: f64, big: &[bool]) -> Vec<Point> {
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
        let box_i = if big.get(i).copied().unwrap_or(false) { vbox.max(TIP_BOX) } else { vbox };
        out.push(origin + minimise_in_box(&mut qq, s, box_i));
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
/// and stay. A sharp tip loses more than one pixel to anti-aliasing and its
/// apex lies further out, so when no measured point sits between the two
/// vertices, or only stair points off its chord, the lines may meet up to
/// [`MERGE_NEAR_TIP`] away, unless the points between the two vertices all
/// lie on the short edge, which makes it a real flat. Returns, per
/// surviving vertex, whether it
/// absorbed a merge and so may be moved further during adjustment.
fn merge_cut_corners(po: &mut Vec<usize>, pts: &[Point], sums: &[Sum], max_chord: f64, near: f64) -> Vec<bool> {
    let n = pts.len();
    let origin = pts[0];
    let line = |i: usize, j: usize| -> (Vec2, Vec2) {
        let j = (j + n - i) % n + i;
        pointslope(sums, n, i, j)
    };
    let mut big = vec![false; po.len()];
    loop {
        let m = po.len();
        if m <= 3 {
            return big;
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
            // With two or more measured points between the vertices the
            // short edge may be a real flat; with fewer there is no evidence
            // of one and a sharp tip is the likelier reading.
            let between = (b + n - a) % n;
            let chord = (pts[b] - pts[a]).hypot();
            let flat = chord >= MERGE_FLAT_MIN && between >= 3 && (1..between).all(|s| on_chord(pts[a], pts[b], pts[(a + s) % n], MERGE_FLAT));
            let limit = if flat { near } else { near.max(MERGE_NEAR_TIP) };
            if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("merge") {
                eprintln!(
                    "merge? a ({:.2},{:.2}) b ({:.2},{:.2}) chord {:.2} between {} x ({:.2},{:.2}) dist {:.2} limit {:.2}",
                    pts[a].x, pts[a].y, pts[b].x, pts[b].y, (pts[b] - pts[a]).hypot(), between, x.x, x.y, (x - mid).hypot(), limit
                );
            }
            if (x - mid).hypot() <= limit {
                po.remove((i + 1) % m);
                big.remove((i + 1) % m);
                let keep = if (i + 1) % m == 0 { i - 1 } else { i };
                big[keep] = true;
                merged = true;
                break;
            }
        }
        if !merged {
            return big;
        }
    }
}

/// A sharp tip's apex may lie this far from the short edge that cuts it, px.
pub const MERGE_NEAR_TIP: f64 = 4.0;
/// Points between the two vertices of a short edge that all lie within this
/// of its chord make it a real flat rather than a cut corner, px...
pub const MERGE_FLAT: f64 = 0.35;
/// ... provided the edge is at least this long; shorter, it is below the
/// resolution at which a flat can be told from the stairs of a tip.
pub const MERGE_FLAT_MIN: f64 = 1.5;

/// Is `p` within `tol` of the segment `a → b`?
fn on_chord(a: Point, b: Point, p: Point, tol: f64) -> bool {
    let d = b - a;
    let len = d.hypot();
    if len < 1e-9 {
        return (p - a).hypot() <= tol;
    }
    ((p - a).cross(d) / len).abs() <= tol
}
/// How far a vertex that absorbed a cut corner may move during adjustment.
pub const TIP_BOX: f64 = 4.0;

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

/// Potrace's `adjust_vertices` with a configurable box; vertices flagged in
/// `big` (merged cut corners, whose apex lies further out) get [`TIP_BOX`].
fn adjust_vertices(pts: &[Point], sums: &[Sum], po: &[usize], vbox: f64, big: &[bool]) -> Vec<Point> {
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
        let box_i = if big.get(i).copied().unwrap_or(false) { vbox.max(TIP_BOX) } else { vbox };
        out.push(origin + minimise_in_box(&mut qq, s, box_i));
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
    fn polyfit_cost_keeps_rectangle_and_thin_line() {
        let (pts, dirs) = rect_contour(12, 7);
        let (v, _) = polygonize_with(&pts, &dirs, &PolygonOpts::new(0.5, 0.5));
        assert_eq!(v.len(), 4, "{v:?}");
        let (pts, dirs) = rect_contour(20, 1);
        let (v, _) = polygonize_with(&pts, &dirs, &PolygonOpts::new(0.2, 0.5));
        assert_eq!(v.len(), 4, "{v:?}");
        // A long straight edge has one candidate end per start point, so the
        // search stays cheap: a 400 px square must be four vertices too.
        let (pts, dirs) = rect_contour(400, 400);
        let (v, _) = polygonize_with(&pts, &dirs, &PolygonOpts::new(0.2, 0.5));
        assert_eq!(v.len(), 4, "{v:?}");
    }

    #[test]
    fn polyfit_cost_turns_evenly_on_a_circle() {
        let r = 30.0;
        let n = 240;
        let pts: Vec<Point> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                Point::new(50.0 + r * t.cos(), 50.0 + r * t.sin())
            })
            .collect();
        let dirs: Vec<u8> = (0..n)
            .map(|i| {
                let t = i as f64 / n as f64 * std::f64::consts::TAU;
                let (dx, dy) = (-t.sin(), t.cos());
                if dx.abs() > dy.abs() { if dx > 0.0 { 0 } else { 2 } } else if dy > 0.0 { 1 } else { 3 }
            })
            .collect();
        let (v, _) = polygonize_with(&pts, &dirs, &PolygonOpts::new(0.5, 0.5));
        let (vp, _) = polygonize_with(&pts, &dirs, &PolygonOpts::potrace(0.5, 0.5));
        assert!(v.len() >= 8 && v.len() <= vp.len() + 2, "{} vs potrace {}", v.len(), vp.len());
        // Turning angles vary less than with the Potrace cost.
        let spread = |v: &[Point]| {
            let m = v.len();
            let turns: Vec<f64> = (0..m).map(|i| turn_angle(v[(i + m - 1) % m], v[i], v[(i + 1) % m])).collect();
            let mean = turns.iter().sum::<f64>() / m as f64;
            (turns.iter().map(|t| (t - mean).powi(2)).sum::<f64>() / m as f64).sqrt()
        };
        assert!(spread(&v) <= spread(&vp) + 1e-9, "{} vs {}", spread(&v), spread(&vp));
        for p in &v {
            let d = ((p.x - 50.0).powi(2) + (p.y - 50.0).powi(2)).sqrt();
            assert!((d - r).abs() < 1.0, "vertex off the circle by {}", d - r);
        }
    }

    #[test]
    fn sharp_tip_cut_corner_is_merged_and_recovered() {
        // A 36° tip pointing up: two edges meeting at (50, 0), whose apex
        // the raster lost. The contour has the edges' points and, across
        // the tip, two stair points 1.3 px apart with one more between.
        let half = 18f64.to_radians();
        let left = |t: f64| Point::new(50.0 - t * half.sin(), t * half.cos());
        let right = |t: f64| Point::new(50.0 + t * half.sin(), t * half.cos());
        let mut pts = Vec::new();
        let mut dirs = Vec::new();
        // Up the left edge from far away to the cut, across, down the right.
        for k in (3..60).rev() {
            pts.push(left(k as f64));
            dirs.push(3u8);
        }
        pts.push(left(2.0));
        dirs.push(3);
        pts.push(Point::new(50.0, 1.5));
        dirs.push(0);
        pts.push(right(2.0));
        dirs.push(1);
        for k in 3..60 {
            pts.push(right(k as f64));
            dirs.push(1u8);
        }
        // Close along the bottom.
        let n_side = pts.len();
        let b = pts[n_side - 1];
        let a = pts[0];
        let steps = 20;
        for k in 1..steps {
            pts.push(b.lerp(a, k as f64 / steps as f64));
            dirs.push(2u8);
        }
        let (v, _) = polygonize_with(&pts, &dirs, &PolygonOpts::new(0.2, 1.5));
        // Four vertices: the apex, the two bottom corners... and the apex
        // must sit near (50, 0), recovered from the two edge lines.
        let apex = v.iter().copied().min_by(|p, q| p.y.partial_cmp(&q.y).unwrap()).unwrap();
        assert!((apex - Point::new(50.0, 0.0)).hypot() < 0.3, "apex {apex:?} in {v:?}");
        assert!(v.len() <= 4, "{v:?}");
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
