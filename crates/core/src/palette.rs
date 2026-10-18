//! Stage 1 for colour: estimate the palette (PLAN.md M2).
//!
//! Colours live in OKLab, where Euclidean distance is close to perceptual
//! difference. Only *interior* pixels (no neighbour differs by more than a
//! small contrast, fully opaque) vote for the palette, so anti-aliased edge
//! pixels, which are mixes of two palette colours, never become colours of
//! their own. Seeds come from a 5-bit RGB histogram, k-means refines them,
//! and clusters closer than `merge_dist` are merged; the user may cap the
//! count. Label 0 is reserved for the transparent background.

use crate::Raster;

/// OKLab colour. `l` is 0..1, `a` and `b` roughly -0.4..0.4.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Lab {
    pub l: f32,
    pub a: f32,
    pub b: f32,
}

impl Lab {
    #[inline]
    pub fn dist(self, o: Lab) -> f32 {
        ((self.l - o.l).powi(2) + (self.a - o.a).powi(2) + (self.b - o.b).powi(2)).sqrt()
    }
    #[inline]
    pub fn dist2(self, o: Lab) -> f32 {
        (self.l - o.l).powi(2) + (self.a - o.a).powi(2) + (self.b - o.b).powi(2)
    }
    #[inline]
    fn add(self, o: Lab) -> Lab {
        Lab { l: self.l + o.l, a: self.a + o.a, b: self.b + o.b }
    }
    #[inline]
    fn sub(self, o: Lab) -> Lab {
        Lab { l: self.l - o.l, a: self.a - o.a, b: self.b - o.b }
    }
    #[inline]
    fn scale(self, s: f32) -> Lab {
        Lab { l: self.l * s, a: self.a * s, b: self.b * s }
    }
    #[inline]
    fn dot(self, o: Lab) -> f32 {
        self.l * o.l + self.a * o.a + self.b * o.b
    }
    /// Point on the segment `self → o` at fraction `t`.
    #[inline]
    pub fn lerp(self, o: Lab, t: f32) -> Lab {
        self.add(o.sub(self).scale(t))
    }
    /// Fraction along `self → o` nearest to `p`, clamped to 0..1, and the
    /// residual distance from `p` to that point.
    pub fn project(self, o: Lab, p: Lab) -> (f32, f32) {
        let d = o.sub(self);
        let len2 = d.dot(d);
        if len2 < 1e-12 {
            return (0.0, p.dist(self));
        }
        let t = (p.sub(self).dot(d) / len2).clamp(0.0, 1.0);
        (t, p.dist(self.lerp(o, t)))
    }
}

fn srgb_to_linear_table() -> [f32; 256] {
    let mut t = [0f32; 256];
    for (i, v) in t.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    }
    t
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let v = if c <= 0.003_130_8 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
    (v * 255.0).round() as u8
}

/// sRGB bytes → OKLab (Ottosson 2020).
pub fn srgb_to_oklab(rgb: [u8; 3], lin: &[f32; 256]) -> Lab {
    let (r, g, b) = (lin[rgb[0] as usize], lin[rgb[1] as usize], lin[rgb[2] as usize]);
    let l = 0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    Lab {
        l: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        a: 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        b: 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    }
}

/// OKLab → sRGB bytes, clamped to gamut.
pub fn oklab_to_srgb(c: Lab) -> [u8; 3] {
    let l = c.l + 0.396_337_78 * c.a + 0.215_803_76 * c.b;
    let m = c.l - 0.105_561_346 * c.a - 0.063_854_17 * c.b;
    let s = c.l - 0.089_484_18 * c.a - 1.291_485_5 * c.b;
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    let r = 4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s;
    let g = -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s;
    let b = -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s;
    [linear_to_srgb(r), linear_to_srgb(g), linear_to_srgb(b)]
}

/// How far (sRGB, 0..255 scale) a pixel may sit from the segment between
/// two palette colours and still count as a mix of them.
pub const RGB_MIX_TOL: f32 = 28.0;

#[derive(Clone, Debug)]
pub struct PaletteOpts {
    /// Cap on the number of opaque colours; `None` = automatic.
    pub max_colors: Option<u32>,
    /// Clusters closer than this (OKLab) are one colour. 0.02 ≈ just
    /// noticeable; logos use far larger steps.
    pub merge_dist: f32,
    /// A pixel whose 4-neighbours differ by more than this is an edge pixel
    /// and does not vote for the palette.
    pub edge_contrast: f32,
}

impl Default for PaletteOpts {
    fn default() -> Self {
        Self { max_colors: None, merge_dist: 0.04, edge_contrast: 0.03 }
    }
}

#[derive(Clone, Debug)]
pub struct Palette {
    /// Index 0 is the transparent background and is never a real colour.
    pub lab: Vec<Lab>,
    pub rgb: Vec<[u8; 3]>,
    /// Label of the background: 0 (transparent), an opaque colour that
    /// dominates the image border, or `None` when no colour does.
    pub background: Option<u16>,
}

impl Palette {
    pub fn len(&self) -> usize {
        self.lab.len()
    }
    pub fn is_empty(&self) -> bool {
        self.lab.len() <= 1
    }
    /// Nearest opaque colour to `c`.
    pub fn nearest(&self, c: Lab) -> u16 {
        let mut best = 1usize;
        let mut bd = f32::MAX;
        for (i, p) in self.lab.iter().enumerate().skip(1) {
            let d = p.dist2(c);
            if d < bd {
                bd = d;
                best = i;
            }
        }
        best as u16
    }
}

/// Per-pixel analysis shared by the palette and the unmixing stage.
pub struct Analysis {
    pub width: usize,
    pub height: usize,
    pub lab: Vec<Lab>,
    pub rgb: Vec<[u8; 3]>,
    pub alpha: Vec<u8>,
    /// Fully opaque and no contrasting 4-neighbour.
    pub interior: Vec<bool>,
}

/// Fraction along the sRGB segment `a → b` nearest to `p`, clamped to
/// 0..1, and the residual distance (0..255 scale). Anti-aliasing blends in
/// sRGB, so mixes are judged there, not in OKLab.
pub fn project_rgb(a: [u8; 3], b: [u8; 3], p: [u8; 3]) -> (f32, f32) {
    let d = [b[0] as f32 - a[0] as f32, b[1] as f32 - a[1] as f32, b[2] as f32 - a[2] as f32];
    let v = [p[0] as f32 - a[0] as f32, p[1] as f32 - a[1] as f32, p[2] as f32 - a[2] as f32];
    let len2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    let t = if len2 < 1e-6 { 0.0 } else { ((v[0] * d[0] + v[1] * d[1] + v[2] * d[2]) / len2).clamp(0.0, 1.0) };
    let r = [v[0] - t * d[0], v[1] - t * d[1], v[2] - t * d[2]];
    (t, (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt())
}

pub fn dist_rgb(a: [u8; 3], b: [u8; 3]) -> f32 {
    let d = [a[0] as f32 - b[0] as f32, a[1] as f32 - b[1] as f32, a[2] as f32 - b[2] as f32];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

pub fn analyse(img: &Raster, edge_contrast: f32) -> Analysis {
    let (w, h) = (img.width as usize, img.height as usize);
    let lin = srgb_to_linear_table();
    let n = w * h;
    let mut lab = Vec::with_capacity(n);
    let mut rgb = Vec::with_capacity(n);
    let mut alpha = Vec::with_capacity(n);
    for i in 0..n {
        let p = &img.data[i * 4..i * 4 + 4];
        lab.push(srgb_to_oklab([p[0], p[1], p[2]], &lin));
        rgb.push([p[0], p[1], p[2]]);
        alpha.push(p[3]);
    }
    let c2 = edge_contrast * edge_contrast;
    let mut interior = vec![false; n];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if alpha[i] < 253 {
                continue;
            }
            let mut ok = true;
            for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
                if nx >= w || ny >= h {
                    continue;
                }
                let j = ny * w + nx;
                if alpha[j] < 253 || lab[i].dist2(lab[j]) > c2 {
                    ok = false;
                    break;
                }
            }
            interior[i] = ok;
        }
    }
    Analysis { width: w, height: h, lab, rgb, alpha, interior }
}

/// Estimate the palette from an analysed image.
pub fn estimate(img: &Raster, an: &Analysis, o: &PaletteOpts) -> Palette {
    let n = an.width * an.height;
    // Voting pixels: interior; if there are none, every opaque pixel.
    let mut votes: Vec<usize> = (0..n).filter(|&i| an.interior[i]).collect();
    if votes.is_empty() {
        votes = (0..n).filter(|&i| an.alpha[i] >= 128).collect();
    }
    let mut lab = vec![Lab::default()];
    if votes.is_empty() {
        return Palette { lab, rgb: vec![[0, 0, 0]], background: Some(0) };
    }

    // Seeds from a 5-bit RGB histogram of the voting pixels.
    let mut hist: std::collections::HashMap<u16, (u32, Lab)> = std::collections::HashMap::new();
    for &i in &votes {
        let p = &img.data[i * 4..i * 4 + 3];
        let key = ((p[0] as u16 >> 3) << 10) | ((p[1] as u16 >> 3) << 5) | (p[2] as u16 >> 3);
        let e = hist.entry(key).or_insert((0, Lab::default()));
        e.0 += 1;
        e.1 = e.1.add(an.lab[i]);
    }
    let mut bins: Vec<(u32, Lab)> = hist.values().map(|&(c, s)| (c, s.scale(1.0 / c as f32))).collect();
    bins.sort_by(|a, b| b.0.cmp(&a.0));
    let min_count = ((votes.len() as f32 * 0.0005) as u32).max(2);
    let max_k = o.max_colors.map(|m| m.max(1) as usize).unwrap_or(32).max(1);
    let mut seeds: Vec<Lab> = Vec::new();
    for &(count, c) in &bins {
        if count < min_count && !seeds.is_empty() {
            break;
        }
        if seeds.iter().all(|s| s.dist(c) >= o.merge_dist) {
            seeds.push(c);
            if seeds.len() >= max_k.max(32) {
                break;
            }
        }
    }
    if seeds.is_empty() {
        seeds.push(bins[0].1);
    }

    // k-means on a subsample of the voting pixels.
    let stride = (votes.len() / 60_000).max(1);
    let sample: Vec<Lab> = votes.iter().step_by(stride).map(|&i| an.lab[i]).collect();
    let mut centers = seeds;
    let mut weights = vec![0u32; centers.len()];
    for _ in 0..8 {
        let mut sum = vec![Lab::default(); centers.len()];
        let mut cnt = vec![0u32; centers.len()];
        for &p in &sample {
            let mut best = 0;
            let mut bd = f32::MAX;
            for (k, c) in centers.iter().enumerate() {
                let d = c.dist2(p);
                if d < bd {
                    bd = d;
                    best = k;
                }
            }
            sum[best] = sum[best].add(p);
            cnt[best] += 1;
        }
        let mut next = Vec::with_capacity(centers.len());
        let mut nw = Vec::with_capacity(centers.len());
        for k in 0..centers.len() {
            if cnt[k] > 0 {
                next.push(sum[k].scale(1.0 / cnt[k] as f32));
                nw.push(cnt[k]);
            }
        }
        centers = next;
        weights = nw;
        if centers.is_empty() {
            break;
        }
    }
    if centers.is_empty() {
        centers.push(bins[0].1);
        weights.push(1);
    }

    // Merge close clusters.
    merge_closest(&mut centers, &mut weights, |d, _| d < o.merge_dist);

    // Colours that only ever appear on edge pixels: a 1 px line has no
    // interior, so its colour has not voted. An opaque edge pixel that is
    // clearly away from every colour and from every sRGB segment between
    // two colours (so not an anti-aliasing mix, nor JPEG ringing, which
    // overshoots a little) is unexplained; well-populated clusters of those
    // are added.
    // One colour is added per round, the most populous, because adding
    // black explains every grey between black and white at once.
    let far = 2.0 * o.merge_dist;
    let edge_px: Vec<usize> = (0..n).filter(|&i| an.alpha[i] >= 253 && !an.interior[i]).collect();
    let opaque = (0..n).filter(|&i| an.alpha[i] >= 253).count();
    let min_w = ((opaque as f32 * 0.002) as u32).max(8);
    for _round in 0..12 {
        let crgb: Vec<[u8; 3]> = centers.iter().map(|&c| oklab_to_srgb(c)).collect();
        let mut extra: Vec<Lab> = Vec::new();
        let mut extra_w: Vec<u32> = Vec::new();
        for &i in &edge_px {
            let p = an.lab[i];
            let prgb = an.rgb[i];
            let near = centers.iter().any(|c| c.dist(p) < far)
                || (0..crgb.len())
                    .any(|a| (a + 1..crgb.len()).any(|b| project_rgb(crgb[a], crgb[b], prgb).1 < RGB_MIX_TOL));
            if near {
                continue;
            }
            match extra.iter().position(|c| c.dist(p) < far) {
                Some(k) => {
                    let wk = extra_w[k] as f32;
                    extra[k] = extra[k].scale(wk).add(p).scale(1.0 / (wk + 1.0));
                    extra_w[k] += 1;
                }
                None => {
                    extra.push(p);
                    extra_w.push(1);
                }
            }
        }
        let Some((k, &wgt)) = extra_w.iter().enumerate().max_by_key(|(_, &w)| w) else { break };
        if wgt < min_w {
            break;
        }
        centers.push(extra[k]);
        weights.push(wgt);
    }

    // Enforce the cap.
    merge_closest(&mut centers, &mut weights, |_, k| k > max_k);

    lab.extend(centers.iter().copied());
    let rgb = lab.iter().map(|&c| oklab_to_srgb(c)).collect();
    let mut pal = Palette { lab, rgb, background: None };
    pal.background = detect_background(an, &pal);
    pal
}

/// Repeatedly merge the closest pair of centres while `stop(dist, count)`
/// says so, weighting by population.
fn merge_closest(centers: &mut Vec<Lab>, weights: &mut Vec<u32>, stop: impl Fn(f32, usize) -> bool) {
    while centers.len() > 1 {
        let mut best = (f32::MAX, 0usize, 1usize);
        for i in 0..centers.len() {
            for j in i + 1..centers.len() {
                let d = centers[i].dist(centers[j]);
                if d < best.0 {
                    best = (d, i, j);
                }
            }
        }
        if !stop(best.0, centers.len()) {
            break;
        }
        let (_, i, j) = best;
        let (wi, wj) = (weights[i] as f32, weights[j] as f32);
        let merged = centers[i].scale(wi).add(centers[j].scale(wj)).scale(1.0 / (wi + wj).max(1.0));
        centers[i] = merged;
        weights[i] += weights[j];
        centers.remove(j);
        weights.remove(j);
    }
}

/// Majority of the one-pixel image border: transparent, one colour, or none.
fn detect_background(an: &Analysis, pal: &Palette) -> Option<u16> {
    let (w, h) = (an.width, an.height);
    if w == 0 || h == 0 {
        return Some(0);
    }
    let mut counts = vec![0u32; pal.len()];
    let mut total = 0u32;
    let mut vote = |i: usize| {
        total += 1;
        if an.alpha[i] < 128 {
            counts[0] += 1;
        } else {
            counts[pal.nearest(an.lab[i]) as usize] += 1;
        }
    };
    for x in 0..w {
        vote(x);
        if h > 1 {
            vote((h - 1) * w + x);
        }
    }
    for y in 1..h.saturating_sub(1) {
        vote(y * w);
        if w > 1 {
            vote(y * w + w - 1);
        }
    }
    let (best, &c) = counts.iter().enumerate().max_by_key(|(_, &c)| c)?;
    if c * 2 > total {
        Some(best as u16)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oklab_round_trips() {
        let lin = srgb_to_linear_table();
        for rgb in [[0, 0, 0], [255, 255, 255], [255, 0, 0], [17, 24, 39], [128, 200, 50]] {
            let back = oklab_to_srgb(srgb_to_oklab(rgb, &lin));
            for c in 0..3 {
                assert!((back[c] as i32 - rgb[c] as i32).abs() <= 1, "{rgb:?} -> {back:?}");
            }
        }
        let white = srgb_to_oklab([255, 255, 255], &lin);
        assert!((white.l - 1.0).abs() < 1e-3 && white.a.abs() < 1e-3 && white.b.abs() < 1e-3);
    }

    /// Three flat colours inside a transparent margin, anti-aliased edges.
    fn three_colors() -> Raster {
        let mut img = Raster::filled(50, 50, [0, 0, 0, 0]);
        for y in 0..40 {
            for x in 0..40 {
                let (c, a): ([u8; 3], u8) = if y < 20 && x < 20 {
                    ([220, 30, 30], 255)
                } else if y < 20 {
                    ([30, 200, 60], 255)
                } else if x < 30 {
                    ([40, 60, 220], 255)
                } else {
                    ([0, 0, 0], 0)
                };
                img.set(x + 5, y + 5, [c[0], c[1], c[2], a]);
            }
        }
        // Mixed edge pixels between red and green.
        for y in 5..25 {
            img.set(25, y, [125, 115, 45, 255]);
        }
        // Half-transparent edge against the canvas.
        for y in 25..45 {
            img.set(35, y, [40, 60, 220, 128]);
        }
        img
    }

    #[test]
    fn finds_three_colors_and_transparent_background() {
        let img = three_colors();
        let an = analyse(&img, 0.03);
        let pal = estimate(&img, &an, &PaletteOpts::default());
        assert_eq!(pal.len(), 4, "{:?}", pal.rgb);
        assert_eq!(pal.background, Some(0));
        let mut rgb = pal.rgb[1..].to_vec();
        rgb.sort();
        let mut want = vec![[220, 30, 30], [30, 200, 60], [40, 60, 220]];
        want.sort();
        for (a, b) in rgb.iter().zip(&want) {
            for c in 0..3 {
                assert!((a[c] as i32 - b[c] as i32).abs() <= 2, "{rgb:?} vs {want:?}");
            }
        }
        // Edge pixels are not interior.
        assert!(!an.interior[15 * 50 + 25] && !an.interior[15 * 50 + 24]);
        assert!(an.interior[15 * 50 + 15]);
    }

    #[test]
    fn opaque_border_is_the_background_and_cap_applies() {
        let mut img = Raster::filled(30, 30, [255, 255, 255, 255]);
        for y in 5..25 {
            for x in 5..25 {
                img.set(x, y, [0, 0, 0, 255]);
            }
        }
        let an = analyse(&img, 0.03);
        let pal = estimate(&img, &an, &PaletteOpts::default());
        assert_eq!(pal.len(), 3);
        let bg = pal.background.unwrap() as usize;
        assert_eq!(pal.rgb[bg], [255, 255, 255]);
        let pal = estimate(&img, &an, &PaletteOpts { max_colors: Some(1), ..Default::default() });
        assert_eq!(pal.len(), 2);
    }
}
