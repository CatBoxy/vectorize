//! Stages 4-7 for colour: the boundary graph (PLAN.md M2).
//!
//! Every boundary between two regions exists once. Pixel corners where
//! three or more regions meet are junctions; the chains of boundary pixel
//! edges between junctions are the graph's edges, each knowing its left and
//! right region. Loops without junctions (islands) are edges too. Each edge
//! is refined to sub-pixel points once, polygonised once with its junction
//! ends pinned, and fitted once; the faces then reuse the fitted curves in
//! either direction, so adjacent shapes share their boundary exactly and
//! there are no seams.

use crate::chain::{self, Chain};
use crate::curve;
use crate::decompose::DIRS;
use crate::fit::{self, FitOpts, CORNER_CLEAR};
use crate::mix::MixMap;
use crate::polygon;
use crate::regions::{Regions, OUTSIDE};
use kurbo::{BezPath, PathEl, Point, Vec2};
use std::collections::HashMap;

const NONE: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct Edge {
    /// Pixel corners: one per step for a loop, one more for an open chain.
    pub corners: Vec<(i32, i32)>,
    pub dirs: Vec<u8>,
    pub left: u32,
    pub right: u32,
    pub closed: bool,
    /// Fitted curve from `start` to `end` (a loop returns to `start`).
    pub start: Point,
    pub end: Point,
    pub els: Vec<PathEl>,
}

pub struct Graph {
    pub width: usize,
    pub height: usize,
    pub edges: Vec<Edge>,
    /// Per horizontal pixel edge `H(x, y)` (between pixel `(x, y-1)` above
    /// and `(x, y)` below; `x < w`, `y <= h`): the edge it belongs to and the
    /// direction that edge's step takes on it.
    h_edge: Vec<u32>,
    h_dir: Vec<u8>,
    /// Per vertical pixel edge `V(x, y)` (between `(x-1, y)` and `(x, y)`;
    /// `x <= w`, `y < h`).
    v_edge: Vec<u32>,
    v_dir: Vec<u8>,
    /// Refined junction positions by pixel corner.
    junctions: HashMap<(i32, i32), Point>,
}

/// Pixel edge of the step from corner `(x, y)` in direction `d`:
/// `(horizontal?, index)`, or `None` off the grid.
#[inline]
fn step_edge(w: usize, h: usize, x: i32, y: i32, d: u8) -> Option<(bool, usize)> {
    let (wi, hi) = (w as i32, h as i32);
    match d {
        0 if x >= 0 && x < wi && y >= 0 && y <= hi => Some((true, y as usize * w + x as usize)),
        2 if x >= 1 && x <= wi && y >= 0 && y <= hi => Some((true, y as usize * w + (x - 1) as usize)),
        1 if x >= 0 && x <= wi && y >= 0 && y < hi => Some((false, y as usize * (w + 1) + x as usize)),
        3 if x >= 0 && x <= wi && y >= 1 && y <= hi => Some((false, (y - 1) as usize * (w + 1) + x as usize)),
        _ => None,
    }
}

/// Pixels on the left and right of the step from `(x, y)` in direction `d`.
#[inline]
fn side_pixels(x: i32, y: i32, d: u8) -> ((i32, i32), (i32, i32)) {
    let (dx, dy) = DIRS[d as usize];
    let (lx, ly) = (dy, -dx);
    let (rx, ry) = (-dy, dx);
    (
        (x + (dx + lx - 1) / 2, y + (dy + ly - 1) / 2),
        (x + (dx + rx - 1) / 2, y + (dy + ry - 1) / 2),
    )
}

impl Graph {
    fn is_boundary(reg: &Regions, x: i32, y: i32, d: u8) -> bool {
        let (l, r) = side_pixels(x, y, d);
        reg.get(l.0, l.1) != reg.get(r.0, r.1)
    }

    fn degree(reg: &Regions, x: i32, y: i32) -> u8 {
        (0..4u8)
            .filter(|&d| step_edge(reg.width, reg.height, x, y, d).is_some() && Self::is_boundary(reg, x, y, d))
            .count() as u8
    }

    /// Trace the boundary graph of `reg`.
    pub fn build(reg: &Regions) -> Graph {
        let (w, h) = (reg.width, reg.height);
        let mut g = Graph {
            width: w,
            height: h,
            edges: Vec::new(),
            h_edge: vec![NONE; w * (h + 1)],
            h_dir: vec![0; w * (h + 1)],
            v_edge: vec![NONE; (w + 1) * h],
            v_dir: vec![0; (w + 1) * h],
            junctions: HashMap::new(),
        };
        let visited = |g: &Graph, x: i32, y: i32, d: u8| -> bool {
            match step_edge(w, h, x, y, d) {
                Some((true, i)) => g.h_edge[i] != NONE,
                Some((false, i)) => g.v_edge[i] != NONE,
                None => true,
            }
        };
        // Chains between junctions.
        for y in 0..=h as i32 {
            for x in 0..=w as i32 {
                if Self::degree(reg, x, y) < 3 {
                    continue;
                }
                for d in 0..4u8 {
                    if step_edge(w, h, x, y, d).is_some() && Self::is_boundary(reg, x, y, d) && !visited(&g, x, y, d) {
                        g.trace(reg, x, y, d, true);
                    }
                }
            }
        }
        // Loops without junctions.
        for y in 0..=h as i32 {
            for x in 0..w as i32 {
                if Self::is_boundary(reg, x, y, 0) && !visited(&g, x, y, 0) {
                    g.trace(reg, x, y, 0, false);
                }
            }
        }
        for y in 0..h as i32 {
            for x in 0..=w as i32 {
                if Self::is_boundary(reg, x, y, 1) && !visited(&g, x, y, 1) {
                    g.trace(reg, x, y, 1, false);
                }
            }
        }
        g
    }

    /// Follow boundary pixel edges from `(x0, y0)` heading `d0` until the
    /// next junction (`from_junction`) or back to the start (a loop).
    fn trace(&mut self, reg: &Regions, x0: i32, y0: i32, d0: u8, from_junction: bool) {
        let (w, h) = (self.width, self.height);
        let id = self.edges.len() as u32;
        let (mut x, mut y, mut d) = (x0, y0, d0);
        let mut corners = vec![(x0, y0)];
        let mut dirs = Vec::new();
        let (lp, rp) = side_pixels(x0, y0, d0);
        let (left, right) = (reg.get(lp.0, lp.1), reg.get(rp.0, rp.1));
        let limit = 4 * (w + 1) * (h + 1) + 8;
        let mut closed = false;
        loop {
            match step_edge(w, h, x, y, d) {
                Some((true, i)) => {
                    self.h_edge[i] = id;
                    self.h_dir[i] = d;
                }
                Some((false, i)) => {
                    self.v_edge[i] = id;
                    self.v_dir[i] = d;
                }
                None => break,
            }
            dirs.push(d);
            let (dx, dy) = DIRS[d as usize];
            x += dx;
            y += dy;
            corners.push((x, y));
            if dirs.len() > limit {
                break;
            }
            if !from_junction && x == x0 && y == y0 {
                closed = true;
                corners.pop();
                break;
            }
            if from_junction && Self::degree(reg, x, y) >= 3 {
                break;
            }
            // Degree 2: continue along the other boundary edge.
            let back = (d + 2) % 4;
            let next = (0..4u8).find(|&nd| {
                nd != back && step_edge(w, h, x, y, nd).is_some() && Self::is_boundary(reg, x, y, nd)
            });
            match next {
                Some(nd) => d = nd,
                None => break,
            }
        }
        self.edges.push(Edge {
            corners,
            dirs,
            left,
            right,
            closed,
            start: Point::new(x0 as f64, y0 as f64),
            end: Point::new(x as f64, y as f64),
            els: Vec::new(),
        });
    }

    /// Sub-pixel points of every edge, then junction positions from the
    /// lines of the edges meeting there.
    fn refine_chains(&self, reg: &Regions, mm: &MixMap) -> Vec<Chain> {
        self.edges
            .iter()
            .map(|e| {
                let (label, sign) = if e.left != OUTSIDE {
                    (reg.color[e.left as usize], 1.0)
                } else if e.right != OUTSIDE {
                    (reg.color[e.right as usize], -1.0)
                } else {
                    (0, 1.0)
                };
                chain::refine(&e.corners, &e.dirs, e.closed, |k| {
                    let (x, y) = e.corners[k];
                    let (l, r) = side_pixels(x, y, e.dirs[k]);
                    let cl = mm.coverage(l.0, l.1, label);
                    let cr = mm.coverage(r.0, r.1, label);
                    ((cl + cr - 1.0) as f64) * sign
                })
            })
            .collect()
    }

    /// Where the edges meeting at each junction corner actually meet: the
    /// point nearest to the lines through their first unbiased points,
    /// within a pixel of the corner.
    fn refine_junctions(&mut self, chains: &[Chain]) {
        let mut lines: HashMap<(i32, i32), Vec<(Point, Vec2)>> = HashMap::new();
        for (e, c) in self.edges.iter().zip(chains) {
            if e.closed {
                continue;
            }
            let n = c.pts.len();
            for (corner, from_start) in [(e.corners[0], true), (*e.corners.last().unwrap(), false)] {
                let idx: Vec<usize> = if from_start {
                    (1..n).filter(|&k| c.fit_ok[k] && !c.is_corner[k]).take(4).collect()
                } else {
                    (0..n - 1).rev().filter(|&k| c.fit_ok[k] && !c.is_corner[k]).take(4).collect()
                };
                if idx.len() < 2 {
                    continue;
                }
                let pts: Vec<Point> = idx.iter().map(|&k| c.pts[k]).collect();
                if let Some(line) = fit_line(&pts) {
                    lines.entry(corner).or_default().push(line);
                }
            }
        }
        for (corner, ls) in lines {
            if ls.len() < 2 {
                continue;
            }
            // Minimise sum of squared distances to the lines.
            let (mut a, mut b, mut cc, mut d, mut e) = (0.0, 0.0, 0.0, 0.0, 0.0);
            for (p, dir) in &ls {
                let nx = -dir.y;
                let ny = dir.x;
                let k = nx * p.x + ny * p.y;
                a += nx * nx;
                b += nx * ny;
                cc += ny * ny;
                d += nx * k;
                e += ny * k;
            }
            let det = a * cc - b * b;
            if det.abs() < 1e-9 {
                continue;
            }
            let x = (d * cc - b * e) / det;
            let y = (a * e - b * d) / det;
            let c0 = Point::new(corner.0 as f64, corner.1 as f64);
            let p = Point::new(x, y);
            if (p - c0).hypot() <= JUNCTION_BOX {
                self.junctions.insert(corner, p);
            } else {
                let v = p - c0;
                self.junctions.insert(corner, c0 + v * (JUNCTION_BOX / v.hypot()));
            }
        }
    }

    /// Refine, polygonise and fit every edge.
    pub fn fit(&mut self, reg: &Regions, mm: &MixMap, tol: f64, vbox: f64, alphamax: f64, o: &FitOpts) {
        let mut chains = self.refine_chains(reg, mm);
        self.refine_junctions(&chains);
        if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("chains") {
            for (i, (e, c)) in self.edges.iter().zip(&chains).enumerate() {
                let pts: Vec<String> = c
                    .pts
                    .iter()
                    .zip(&c.fit_ok)
                    .map(|(p, ok)| format!("({:.2},{:.2}){}", p.x, p.y, if *ok { "" } else { "x" }))
                    .collect();
                eprintln!(
                    "edge {i}: left {} ({}) right {} ({}) closed {} steps {} -> {}",
                    e.left,
                    if e.left == OUTSIDE { 0 } else { reg.color[e.left as usize] },
                    e.right,
                    if e.right == OUTSIDE { 0 } else { reg.color[e.right as usize] },
                    e.closed,
                    e.dirs.len(),
                    pts.join(" ")
                );
            }
        }
        for (e, c) in self.edges.iter_mut().zip(chains.iter_mut()) {
            if !e.closed {
                if let Some(p) = self.junctions.get(&e.corners[0]) {
                    c.pts[0] = *p;
                }
                if let Some(p) = self.junctions.get(e.corners.last().unwrap()) {
                    *c.pts.last_mut().unwrap() = *p;
                }
            }
            let idx: Vec<usize> = (0..c.pts.len()).filter(|&k| c.fit_ok[k]).collect();
            let pts: Vec<Point> = idx.iter().map(|&k| c.pts[k]).collect();
            let dirs: Vec<u8> = idx.iter().map(|&k| c.dirs[k]).collect();
            if e.closed {
                let (verts, po) = polygon::polygonize(&pts, &dirs, tol, vbox);
                let po: Vec<usize> = po.into_iter().map(|j| idx[j]).collect();
                let mut path = BezPath::new();
                if verts.len() >= 3 {
                    let corner = curve::corner_flags(&verts, alphamax);
                    let contour = crate::contour::Contour {
                        pts: c.pts.clone(),
                        dirs: c.dirs.clone(),
                        is_corner: c.is_corner.clone(),
                        fit_ok: c.fit_ok.clone(),
                        area: 0.0,
                    };
                    fit::append_loop(&mut path, &contour, &verts, &po, &corner, o);
                }
                let (start, mut els) = split_path(&path);
                if let Some(s) = start {
                    if els.last().map_or(true, |l| (end_of(l) - s).hypot() > 1e-9) {
                        els.push(PathEl::LineTo(s));
                    }
                    e.start = s;
                    e.end = s;
                } else {
                    e.start = c.pts[0];
                    e.end = c.pts[0];
                }
                e.els = els;
            } else {
                let (verts, po) = polygon::polygonize_open(&pts, &dirs, tol, vbox);
                let po: Vec<usize> = po.into_iter().map(|j| idx[j]).collect();
                let m = verts.len();
                e.start = c.pts[0];
                e.end = *c.pts.last().unwrap();
                e.els.clear();
                if m < 2 {
                    e.els.push(PathEl::LineTo(e.end));
                    continue;
                }
                let corner: Vec<bool> = (0..m)
                    .map(|i| i == 0 || i == m - 1 || curve::is_corner(verts[i - 1], verts[i], verts[i + 1], alphamax))
                    .collect();
                let cs: Vec<usize> = (0..m).filter(|&i| corner[i]).collect();
                if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("chains") {
                    eprintln!(
                        "  open edge: {} pts, polygon {} verts, {} corners at {:?}",
                        pts.len(),
                        m,
                        cs.len(),
                        cs.iter().map(|&i| format!("({:.1},{:.1})", verts[i].x, verts[i].y)).collect::<Vec<_>>()
                    );
                }
                for w in cs.windows(2) {
                    let (a, b) = (w[0], w[1]);
                    let (va, vb) = (verts[a], verts[b]);
                    let mut samples = vec![va];
                    for k in po[a] + 1..po[b] {
                        let p = c.pts[k];
                        if c.fit_ok[k] && (p - va).hypot() > CORNER_CLEAR && (p - vb).hypot() > CORNER_CLEAR {
                            samples.push(p);
                        }
                    }
                    samples.push(vb);
                    e.els.extend(fit::fit_run(&samples, false, o).1);
                }
                if e.els.is_empty() {
                    e.els.push(PathEl::LineTo(e.end));
                }
                if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("edges") {
                    let runs: Vec<String> = cs
                        .windows(2)
                        .map(|w| format!("{}", po[w[1]] - po[w[0]]))
                        .collect();
                    eprintln!(
                        "edge: steps {} verts {} corners {} els {} ({} lines) runs {}",
                        e.dirs.len(),
                        m,
                        cs.len(),
                        e.els.len(),
                        e.els.iter().filter(|x| matches!(x, PathEl::LineTo(_))).count(),
                        runs.join(",")
                    );
                }
            }
        }
    }

    /// The cycles of every region: lists of `(edge, forward)`.
    pub fn faces(&self, reg: &Regions) -> Vec<Vec<Vec<(u32, bool)>>> {
        let (w, h) = (self.width, self.height);
        let nr = reg.area.len();
        let mut out: Vec<Vec<Vec<(u32, bool)>>> = vec![Vec::new(); nr];
        let mut h_done = vec![0u8; w * (h + 1)];
        let mut v_done = vec![0u8; (w + 1) * h];
        // Every boundary pixel edge, from the side of each of its regions.
        for y in 0..=h as i32 {
            for x in 0..w as i32 {
                let i = y as usize * w + x as usize;
                if self.h_edge[i] == NONE {
                    continue;
                }
                let above = reg.get(x, y - 1);
                let below = reg.get(x, y);
                // Walking E from (x, y) has the pixel above on the left.
                if above != OUTSIDE && h_done[i] & side_bit(self.h_dir[i], 0) == 0 {
                    let cyc = self.walk(reg, above, x, y, 0, &mut h_done, &mut v_done);
                    out[above as usize].push(cyc);
                }
                if below != OUTSIDE && h_done[i] & side_bit(self.h_dir[i], 2) == 0 {
                    let cyc = self.walk(reg, below, x + 1, y, 2, &mut h_done, &mut v_done);
                    out[below as usize].push(cyc);
                }
            }
        }
        for y in 0..h as i32 {
            for x in 0..=w as i32 {
                let i = y as usize * (w + 1) + x as usize;
                if self.v_edge[i] == NONE {
                    continue;
                }
                let east = reg.get(x, y);
                let west = reg.get(x - 1, y);
                // Walking S from (x, y) has the pixel to the east on the left.
                if east != OUTSIDE && v_done[i] & side_bit(self.v_dir[i], 1) == 0 {
                    let cyc = self.walk(reg, east, x, y, 1, &mut h_done, &mut v_done);
                    out[east as usize].push(cyc);
                }
                if west != OUTSIDE && v_done[i] & side_bit(self.v_dir[i], 3) == 0 {
                    let cyc = self.walk(reg, west, x, y + 1, 3, &mut h_done, &mut v_done);
                    out[west as usize].push(cyc);
                }
            }
        }
        out
    }

    /// Walk the boundary of region `r` from `(x0, y0)` heading `d0`, keeping
    /// `r` on the left, and return the cycle of oriented edges.
    fn walk(&self, reg: &Regions, r: u32, x0: i32, y0: i32, d0: u8, h_done: &mut [u8], v_done: &mut [u8]) -> Vec<(u32, bool)> {
        let (w, h) = (self.width, self.height);
        let (mut x, mut y, mut d) = (x0, y0, d0);
        let mut cycle: Vec<(u32, bool)> = Vec::new();
        let limit = 4 * (w + 1) * (h + 1) + 8;
        let mut steps = 0;
        loop {
            let (id, forward) = match step_edge(w, h, x, y, d) {
                Some((true, i)) => {
                    h_done[i] |= side_bit(self.h_dir[i], d);
                    (self.h_edge[i], self.h_dir[i] == d)
                }
                Some((false, i)) => {
                    v_done[i] |= side_bit(self.v_dir[i], d);
                    (self.v_edge[i], self.v_dir[i] == d)
                }
                None => break,
            };
            if id != NONE && cycle.last() != Some(&(id, forward)) {
                cycle.push((id, forward));
            }
            let (dx, dy) = DIRS[d as usize];
            x += dx;
            y += dy;
            steps += 1;
            if steps > limit {
                break;
            }
            // Next direction: keep r on the left.
            let (al, ar) = {
                let (l, rr) = side_pixels(x, y, d);
                (reg.get(l.0, l.1), reg.get(rr.0, rr.1))
            };
            let turn_right = if al == r && ar == r {
                true
            } else if al == r {
                // straight
                if x == x0 && y == y0 && d == d0 {
                    break;
                }
                continue;
            } else if ar == r {
                reg.joined_at(x, y) == Some(reg.color[r as usize])
            } else {
                false
            };
            d = if turn_right { (d + 1) % 4 } else { (d + 3) % 4 };
            if x == x0 && y == y0 && d == d0 {
                break;
            }
        }
        // A loop edge appears once; a chain walked both ways in one cycle
        // (a spur) appears twice, which is correct.
        if cycle.len() > 1 && cycle.first() == cycle.last() {
            cycle.pop();
        }
        cycle
    }

    /// Path of one cycle from the fitted edges.
    pub fn cycle_path(&self, cycle: &[(u32, bool)], path: &mut BezPath) {
        if cycle.is_empty() {
            return;
        }
        let (e0, f0) = cycle[0];
        let e = &self.edges[e0 as usize];
        let start = if f0 { e.start } else { e.end };
        let mut els: Vec<PathEl> = Vec::new();
        for &(id, fwd) in cycle {
            let e = &self.edges[id as usize];
            if fwd {
                els.extend_from_slice(&e.els);
            } else {
                els.extend(reverse_els(e.start, &e.els));
            }
        }
        // The closing line is implied by `Z`.
        if let Some(PathEl::LineTo(p)) = els.last() {
            if (*p - start).hypot() < 1e-9 {
                els.pop();
            }
        }
        path.move_to(start);
        for el in els {
            path.push(el);
        }
        path.close_path();
    }
}

/// Junction positions may move at most this far from their pixel corner.
pub const JUNCTION_BOX: f64 = 1.0;

/// Which side of a pixel edge a walk in direction `d` sees, given the chain
/// direction `cd` on it: bit 1 for the chain's left side, bit 2 for its
/// right side.
#[inline]
fn side_bit(cd: u8, d: u8) -> u8 {
    if cd == d { 1 } else { 2 }
}

/// Least-squares line through points: centroid and unit direction.
fn fit_line(pts: &[Point]) -> Option<(Point, Vec2)> {
    let n = pts.len() as f64;
    if pts.len() < 2 {
        return None;
    }
    let c = Point::new(pts.iter().map(|p| p.x).sum::<f64>() / n, pts.iter().map(|p| p.y).sum::<f64>() / n);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p.x - c.x, p.y - c.y);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let dir = Vec2::new(theta.cos(), theta.sin());
    if sxx + syy < 1e-12 {
        return None;
    }
    Some((c, dir))
}

fn end_of(el: &PathEl) -> Point {
    match *el {
        PathEl::MoveTo(p) | PathEl::LineTo(p) | PathEl::QuadTo(_, p) | PathEl::CurveTo(_, _, p) => p,
        PathEl::ClosePath => Point::ZERO,
    }
}

/// Start point and the elements after `MoveTo`, without `ClosePath`.
fn split_path(path: &BezPath) -> (Option<Point>, Vec<PathEl>) {
    let mut start = None;
    let mut els = Vec::new();
    for el in path.elements() {
        match el {
            PathEl::MoveTo(p) => start = Some(*p),
            PathEl::ClosePath => {}
            other => els.push(*other),
        }
    }
    (start, els)
}

/// The same curve walked backwards: from the last end point back to `start`.
pub fn reverse_els(start: Point, els: &[PathEl]) -> Vec<PathEl> {
    let mut out = Vec::with_capacity(els.len());
    let mut from = Vec::with_capacity(els.len());
    let mut cur = start;
    for el in els {
        from.push(cur);
        cur = end_of(el);
    }
    for (el, &q) in els.iter().zip(&from).rev() {
        out.push(match *el {
            PathEl::CurveTo(a, b, _) => PathEl::CurveTo(b, a, q),
            PathEl::QuadTo(a, _) => PathEl::QuadTo(a, q),
            _ => PathEl::LineTo(q),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mix::{Mix, MixMap};
    use crate::regions::label;

    fn map(rows: &[&str]) -> MixMap {
        let h = rows.len();
        let w = rows[0].len();
        let mut mix = Vec::with_capacity(w * h);
        for r in rows {
            for ch in r.chars() {
                mix.push(Mix::solid(ch as u16 - 'a' as u16 + 1));
            }
        }
        MixMap { width: w, height: h, mix }
    }

    #[test]
    fn three_regions_share_edges_once() {
        // Two squares b and c on a field of a: a|b, a|c, b|c boundaries and
        // two junctions where all three meet.
        let mut mm = map(&["aaaaaa", "abbcca", "abbcca", "aaaaaa"]);
        let reg = label(&mut mm, Some(1), 1);
        let g = Graph::build(&reg);
        let ids: std::collections::HashSet<(u32, u32)> =
            g.edges.iter().map(|e| (e.left.min(e.right), e.left.max(e.right))).collect();
        // a|outside is a loop; a|b, b|c, a|c are chains between the two junctions.
        assert_eq!(g.edges.len(), 4, "{:?}", g.edges.iter().map(|e| (e.left, e.right, e.closed, e.dirs.len())).collect::<Vec<_>>());
        assert_eq!(ids.len(), 4);
        assert_eq!(g.edges.iter().filter(|e| e.closed).count(), 1);
        let faces = g.faces(&reg);
        let b = reg.get(1, 1) as usize;
        let c = reg.get(3, 1) as usize;
        let a = reg.get(0, 0) as usize;
        assert_eq!(faces[b].len(), 1);
        assert_eq!(faces[c].len(), 1);
        assert_eq!(faces[b][0].len(), 2);
        assert_eq!(faces[c][0].len(), 2);
        // a has an outer loop and an inner cycle of two chains.
        assert_eq!(faces[a].len(), 2);
        // The b|c chain is used once by each, in opposite directions.
        let bc = g.edges.iter().position(|e| !e.closed && e.left != a as u32 && e.right != a as u32).unwrap() as u32;
        let fb = faces[b][0].iter().find(|(id, _)| *id == bc).unwrap().1;
        let fc = faces[c][0].iter().find(|(id, _)| *id == bc).unwrap().1;
        assert_ne!(fb, fc);
    }

    #[test]
    fn fitted_edges_make_closed_gapless_paths() {
        let mut mm = map(&["aaaaaaaa", "abbbccca", "abbbccca", "abbbccca", "aaaaaaaa"]);
        let reg = label(&mut mm, Some(1), 1);
        let mut g = Graph::build(&reg);
        let o = FitOpts { accuracy: 0.3, snap_lines: true, window: 1.0 };
        g.fit(&reg, &mm, 0.2, 1.5, 1.0, &o);
        let faces = g.faces(&reg);
        let b = reg.get(1, 1) as usize;
        let mut path = BezPath::new();
        g.cycle_path(&faces[b][0], &mut path);
        // A rectangle: four lines.
        let lines = path.elements().iter().filter(|e| matches!(e, PathEl::LineTo(_))).count();
        assert!(lines >= 3 && lines <= 4, "{:?}", path.elements());
        // Its end returns to its start.
        let els = path.elements();
        let start = match els[0] {
            PathEl::MoveTo(p) => p,
            _ => panic!(),
        };
        let last = els[..els.len() - 1].last().unwrap();
        let p = end_of(last);
        assert!((p - start).hypot() < 1e-9 || lines == 3, "{:?}", els);
        use kurbo::Shape;
        assert!((path.area().abs() - 9.0).abs() < 0.5, "area {}", path.area());
    }

    #[test]
    fn reverse_round_trips() {
        let start = Point::new(0.0, 0.0);
        let els = vec![
            PathEl::CurveTo(Point::new(1.0, 1.0), Point::new(2.0, 1.0), Point::new(3.0, 0.0)),
            PathEl::LineTo(Point::new(5.0, 0.0)),
        ];
        let rev = reverse_els(start, &els);
        assert_eq!(rev[0], PathEl::LineTo(Point::new(3.0, 0.0)));
        assert_eq!(rev[1], PathEl::CurveTo(Point::new(2.0, 1.0), Point::new(1.0, 1.0), Point::new(0.0, 0.0)));
        let back = reverse_els(Point::new(5.0, 0.0), &rev);
        assert_eq!(back, els);
    }
}
