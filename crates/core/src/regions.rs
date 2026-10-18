//! Stage 3 for colour: regions (PLAN.md M2).
//!
//! Pixels are labelled by their dominant palette colour, grouped into
//! 4-connected components, and the two diagonal pixels of every 2×2
//! checkerboard are joined for one of the two colours (the one that is not
//! the background, else the rarer one), so thin diagonal lines survive.
//! Regions smaller than `min_area` are merged into the neighbour they share
//! the longest border with, and their pixels take that neighbour's colour,
//! so the boundary stage never sees them.

use crate::mix::{Mix, MixMap};

pub const OUTSIDE: u32 = u32::MAX;

#[derive(Clone, Debug)]
pub struct Regions {
    pub width: usize,
    pub height: usize,
    /// Region id per pixel, compact from 0.
    pub id: Vec<u32>,
    /// Palette label per region.
    pub color: Vec<u16>,
    /// Pixels per region.
    pub area: Vec<u32>,
    /// Per pixel corner `(x, y)` with `x in 0..=w`, `y in 0..=h`: the label
    /// whose diagonal pixels are joined there, or `u16::MAX`.
    pub diag: Vec<u16>,
}

impl Regions {
    /// Region of pixel `(x, y)`, `OUTSIDE` beyond the image.
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return OUTSIDE;
        }
        self.id[y as usize * self.width + x as usize]
    }
    /// Label joined diagonally at corner `(x, y)`, if any.
    #[inline]
    pub fn joined_at(&self, x: i32, y: i32) -> Option<u16> {
        if x < 0 || y < 0 || x as usize > self.width || y as usize > self.height {
            return None;
        }
        let v = self.diag[y as usize * (self.width + 1) + x as usize];
        (v != u16::MAX).then_some(v)
    }
}

struct Uf(Vec<u32>);
impl Uf {
    fn find(&mut self, mut i: u32) -> u32 {
        while self.0[i as usize] != i {
            let p = self.0[i as usize];
            self.0[i as usize] = self.0[p as usize];
            i = p;
        }
        i
    }
    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[rb as usize] = ra;
        }
    }
}

/// Label the pixels of `mm`. `background` is the palette label of the
/// background, if known; `min_area` the speckle threshold in pixels;
/// `color_dist(a, b)` the perceptual distance between two palette labels,
/// used to break ties between neighbours a speckle borders equally.
pub fn label(mm: &mut MixMap, background: Option<u16>, min_area: u32, color_dist: impl Fn(u16, u16) -> f32) -> Regions {
    let (w, h) = (mm.width, mm.height);
    let n = w * h;
    let mut lab: Vec<u16> = mm.mix.iter().map(|m| m.label()).collect();
    let mut uf = Uf((0..n as u32).collect());
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if x + 1 < w && lab[i + 1] == lab[i] {
                uf.union(i as u32, (i + 1) as u32);
            }
            if y + 1 < h && lab[i + w] == lab[i] {
                uf.union(i as u32, (i + w) as u32);
            }
        }
    }
    // Checkerboards: join the diagonal pair of one colour.
    let mut count = std::collections::HashMap::<u16, u32>::new();
    for &l in &lab {
        *count.entry(l).or_insert(0) += 1;
    }
    let mut diag = vec![u16::MAX; (w + 1) * (h + 1)];
    for y in 1..h {
        for x in 1..w {
            let (tl, tr, bl, br) = (lab[(y - 1) * w + x - 1], lab[(y - 1) * w + x], lab[y * w + x - 1], lab[y * w + x]);
            if tl == br && tr == bl && tl != tr {
                let (p, q) = (tl, tr);
                let pick = if Some(p) == background {
                    q
                } else if Some(q) == background {
                    p
                } else if count[&p] <= count[&q] {
                    p
                } else {
                    q
                };
                diag[y * (w + 1) + x] = pick;
                if pick == p {
                    uf.union(((y - 1) * w + x - 1) as u32, (y * w + x) as u32);
                } else {
                    uf.union(((y - 1) * w + x) as u32, (y * w + x - 1) as u32);
                }
            }
        }
    }

    // Compact ids and areas.
    let mut id = vec![0u32; n];
    let mut area: Vec<u32> = Vec::new();
    let mut color: Vec<u16> = Vec::new();
    let mut root_to_id = std::collections::HashMap::<u32, u32>::new();
    for i in 0..n {
        let r = uf.find(i as u32);
        let k = *root_to_id.entry(r).or_insert_with(|| {
            area.push(0);
            color.push(lab[i]);
            (area.len() - 1) as u32
        });
        id[i] = k;
        area[k as usize] += 1;
    }

    // Speckles: merge into the neighbour with the longest shared border,
    // smallest first, until none is left.
    if min_area > 1 {
        let mut merged_into: Vec<u32> = (0..area.len() as u32).collect();
        let resolve = |merged_into: &Vec<u32>, mut k: u32| {
            while merged_into[k as usize] != k {
                k = merged_into[k as usize];
            }
            k
        };
        for _round in 0..8 {
            let mut order: Vec<u32> = (0..area.len() as u32)
                .filter(|&k| merged_into[k as usize] == k && area[k as usize] < min_area)
                .collect();
            if order.is_empty() {
                break;
            }
            order.sort_by_key(|&k| area[k as usize]);
            // Shared border lengths per (region, neighbour).
            let mut border = std::collections::HashMap::<(u32, u32), u32>::new();
            for y in 0..h {
                for x in 0..w {
                    let a = resolve(&merged_into, id[y * w + x]);
                    for (nx, ny) in [(x + 1, y), (x, y + 1)] {
                        if nx >= w || ny >= h {
                            continue;
                        }
                        let b = resolve(&merged_into, id[ny * w + nx]);
                        if a != b {
                            *border.entry((a, b)).or_insert(0) += 1;
                            *border.entry((b, a)).or_insert(0) += 1;
                        }
                    }
                }
            }
            let mut any = false;
            for k in order {
                if merged_into[k as usize] != k || area[k as usize] >= min_area {
                    continue;
                }
                // Longest shared border wins; ties go to the nearest colour,
                // then the lowest region id, so the result never depends on
                // hash iteration order (which made one run in a thousand
                // differ from the next and hang the bench on a rare chain).
                let mut best: Option<(u32, u32, f32)> = None;
                for (&(a, b), &len) in &border {
                    if a == k {
                        let b = resolve(&merged_into, b);
                        if b == k {
                            continue;
                        }
                        let d = color_dist(color[k as usize], color[b as usize]);
                        let better = match best {
                            None => true,
                            Some((bb, bl, bd)) => len > bl || (len == bl && (d < bd || (d == bd && b < bb))),
                        };
                        if better {
                            best = Some((b, len, d));
                        }
                    }
                }
                if let Some((target, _, _)) = best {
                    merged_into[k as usize] = target;
                    area[target as usize] += area[k as usize];
                    area[k as usize] = 0;
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
        // Apply: relabel pixels and recolour their mixes.
        let mut remap: Vec<u32> = vec![0; area.len()];
        let mut new_area = Vec::new();
        let mut new_color = Vec::new();
        for k in 0..area.len() as u32 {
            if merged_into[k as usize] == k {
                remap[k as usize] = new_area.len() as u32;
                new_area.push(area[k as usize]);
                new_color.push(color[k as usize]);
            }
        }
        for k in 0..area.len() as u32 {
            let r = resolve(&merged_into, k);
            remap[k as usize] = remap[r as usize];
        }
        for i in 0..n {
            let old = id[i];
            let nk = remap[old as usize];
            id[i] = nk;
            let c = new_color[nk as usize];
            if lab[i] != c {
                lab[i] = c;
                mm.mix[i] = Mix::solid(c);
            }
        }
        area = new_area;
        color = new_color;
        // Diagonal joins whose colour vanished at that corner are moot.
        for y in 1..h {
            for x in 1..w {
                let v = &mut diag[y * (w + 1) + x];
                if *v != u16::MAX {
                    let (tl, tr, bl, br) = (lab[(y - 1) * w + x - 1], lab[(y - 1) * w + x], lab[y * w + x - 1], lab[y * w + x]);
                    if !(tl == br && tr == bl && tl != tr) {
                        *v = u16::MAX;
                    }
                }
            }
        }
    }
    Regions { width: w, height: h, id, color, area, diag }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn components_and_speckles() {
        let mut mm = map(&["aaaaab", "aabaab", "aaaaab", "cccccb"]);
        let r = label(&mut mm, Some(1), 2, |_, _| 0.0);
        // The single b inside a is a speckle: merged into a, recoloured.
        assert_eq!(r.get(2, 1), r.get(0, 0));
        assert_eq!(mm.mix[1 * 6 + 2], Mix::solid(1));
        // Column of b on the right is one region, the c row another.
        assert_eq!(r.get(5, 0), r.get(5, 3));
        assert_ne!(r.get(5, 0), r.get(0, 3));
        assert_eq!(r.color[r.get(0, 3) as usize], 3);
        assert_eq!(r.area.iter().sum::<u32>(), 24);
        assert_eq!(r.get(-1, 0), OUTSIDE);
    }

    #[test]
    fn checkerboard_joins_the_rarer_colour() {
        // Diagonal line of b through a field of a: b is joined, so it is
        // one region and survives the speckle filter.
        let mut mm = map(&["baaa", "abaa", "aaba", "aaab"]);
        let r = label(&mut mm, Some(1), 2, |_, _| 0.0);
        assert_eq!(r.get(0, 0), r.get(3, 3));
        assert_eq!(r.area[r.get(0, 0) as usize], 4);
        assert_eq!(r.joined_at(1, 1), Some(2));
        assert_eq!(r.joined_at(0, 0), None);
    }
}
