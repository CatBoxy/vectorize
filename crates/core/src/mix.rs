//! Stage 2 for colour: per-pixel coverage of palette colours (PLAN.md M2).
//!
//! Every pixel is at most a mix of two labels: `a` with coverage `1 - t`
//! and `b` with coverage `t`. Interior pixels are 1.0 for their nearest
//! colour. An edge pixel is unmixed against the pair of palette colours,
//! taken from the interior pixels around it, whose segment in OKLab passes
//! closest to it. Partial alpha is coverage of the transparent background
//! (label 0). The coverage planes the contour stage reads are derived from
//! this without storing one plane per colour.

use crate::palette::{conflation_t, dist_rgb, project_rgb, project_tri_rgb, Analysis, Palette, CANVAS, RGB_MIX_TOL};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mix {
    pub a: u16,
    pub b: u16,
    pub t: f32,
}

impl Mix {
    #[inline]
    pub fn solid(label: u16) -> Self {
        Self { a: label, b: label, t: 0.0 }
    }
    /// Coverage of `label` in this pixel.
    #[inline]
    pub fn coverage(&self, label: u16) -> f32 {
        let mut c = 0.0;
        if self.a == label {
            c += 1.0 - self.t;
        }
        if self.b == label {
            c += self.t;
        }
        c
    }
    /// The label with the larger coverage.
    #[inline]
    pub fn label(&self) -> u16 {
        if self.t > 0.5 { self.b } else { self.a }
    }
}

/// Coverage field over the image for any label.
pub struct MixMap {
    pub width: usize,
    pub height: usize,
    pub mix: Vec<Mix>,
}

impl MixMap {
    /// Coverage of `label` at `(x, y)`; outside the image it is the
    /// transparent background, so 1 for label 0 and 0 otherwise.
    #[inline]
    pub fn coverage(&self, x: i32, y: i32, label: u16) -> f32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return if label == 0 { 1.0 } else { 0.0 };
        }
        self.mix[y as usize * self.width + x as usize].coverage(label)
    }
}

/// Unmix every pixel against the palette.
pub fn unmix(an: &Analysis, pal: &Palette) -> MixMap {
    let (w, h) = (an.width, an.height);
    let n = w * h;
    let mut mix = vec![Mix::default(); n];
    if pal.is_empty() {
        return MixMap { width: w, height: h, mix };
    }
    // Nearest colour for every opaque pixel.
    let nearest: Vec<u16> = (0..n).map(|i| if an.alpha[i] == 0 { 0 } else { pal.nearest(an.lab[i]) }).collect();
    // The canvas that shows through a seam between abutting shapes, when
    // the image is opaque: its background colour, else white.
    let canvas = if an.alpha.iter().all(|&a| a >= 128) {
        Some(match pal.background {
            Some(b) if b > 0 => pal.rgb[b as usize],
            _ => CANVAS,
        })
    } else {
        None
    };
    // Is there real transparency within two pixels? A partially transparent
    // pixel with none nearby is a *conflation* pixel: the renderer
    // anti-aliased two abutting shapes separately and their coverages did
    // not sum to one. Its colour is still a mix of the two neighbours.
    let near_transparent = |x: usize, y: usize| -> bool {
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                    return true;
                }
                if an.alpha[ny as usize * w + nx as usize] < 128 {
                    return true;
                }
            }
        }
        false
    };
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let a = an.alpha[i] as f32 / 255.0;
            if an.alpha[i] == 0 {
                mix[i] = Mix::solid(0);
                continue;
            }
            let n1 = nearest[i];
            if an.alpha[i] < 253 && near_transparent(x, y) {
                // Partially transparent: the colour part snaps to one colour.
                mix[i] = Mix { a: 0, b: n1, t: a };
                continue;
            }
            if an.interior[i] {
                mix[i] = Mix::solid(n1);
                continue;
            }
            // Opaque edge pixel: it mixes two colours that are both present
            // around it, so the candidates are the interior colours nearby.
            // The nearest palette colour overall is only a fallback partner
            // (a 1 px line has no interior of its own) and may well be a
            // colour from elsewhere in the image.
            let p = an.lab[i];
            let mut cands: [u16; 10] = [0; 10];
            let mut nc = 0;
            for dy in -2i32..=2 {
                for dx in -2i32..=2 {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx as usize >= w || ny as usize >= h {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if !an.interior[j] {
                        continue;
                    }
                    let c = nearest[j];
                    if !cands[..nc].contains(&c) && nc < cands.len() - 1 {
                        cands[nc] = c;
                        nc += 1;
                    }
                }
            }
            // The pixel's own nearest colour is always a candidate: at a
            // tangency the colour it mostly is may have no interior pixel
            // within reach, and projecting onto the wrong pair relabels it.
            if !cands[..nc].contains(&n1) && nc < cands.len() - 1 {
                cands[nc] = n1;
                nc += 1;
            }
            if nc < 2 {
                {
                    // Second-nearest colour overall.
                    let mut best = (f32::MAX, n1);
                    for (k, c) in pal.lab.iter().enumerate().skip(1) {
                        if k as u16 != n1 {
                            let d = c.dist2(p);
                            if d < best.0 {
                                best = (d, k as u16);
                            }
                        }
                    }
                    if best.1 != n1 {
                        cands[nc] = best.1;
                        nc += 1;
                    }
                }
            }
            // Anti-aliasing blends in sRGB: unmix there. Best single colour
            // first, then every pair.
            let prgb = an.rgb[i];
            let mut best_solid = (f32::MAX, cands[0]);
            for &c in &cands[..nc] {
                let d = dist_rgb(prgb, pal.rgb[c as usize]);
                if d < best_solid.0 {
                    best_solid = (d, c);
                }
            }
            let mut best = (best_solid.0, best_solid.1, best_solid.1, 0.0f32);
            for ia in 0..nc {
                for ib in ia + 1..nc {
                    let (ca, cb) = (cands[ia], cands[ib]);
                    let (t, res) = project_rgb(pal.rgb[ca as usize], pal.rgb[cb as usize], prgb);
                    if res < best.0 - 1e-3 {
                        best = (res, ca, cb, t);
                    }
                }
            }
            // Explained by nothing: a conflation pixel, where the canvas
            // shows through the seam between two abutting shapes.
            if let Some(canvas) = canvas.filter(|_| best.0 >= RGB_MIX_TOL) {
                for ia in 0..nc {
                    for ib in ia + 1..nc {
                        let (ca, cb) = (cands[ia], cands[ib]);
                        let (w, res) = project_tri_rgb(pal.rgb[ca as usize], pal.rgb[cb as usize], canvas, prgb);
                        if res < RGB_MIX_TOL && res < best.0 - 1e-3 && w[2] > 0.02 {
                            best = (res, ca, cb, conflation_t(w));
                        }
                    }
                }
            }
            mix[i] = if best.1 == best.2 { Mix::solid(best.1) } else { Mix { a: best.1, b: best.2, t: best.3 } };
        }
    }
    MixMap { width: w, height: h, mix }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{analyse, estimate, PaletteOpts};
    use crate::Raster;

    #[test]
    fn edge_pixels_are_unmixed() {
        // Red left, blue right, a 50/50 mixed column in between.
        let mut img = Raster::filled(20, 10, [255, 255, 255, 255]);
        for y in 0..10 {
            for x in 0..10 {
                img.set(x, y, [220, 30, 30, 255]);
            }
            for x in 11..20 {
                img.set(x, y, [40, 60, 220, 255]);
            }
            img.set(10, y, [130, 45, 125, 255]);
        }
        let an = analyse(&img, 0.03);
        let pal = estimate(&img, &an, &PaletteOpts::default());
        assert_eq!(pal.len(), 3, "{:?}", pal.rgb);
        let mm = unmix(&an, &pal);
        let red = pal.nearest(an.lab[5]);
        let blue = pal.nearest(an.lab[15]);
        let m = mm.mix[5 * 20 + 10];
        assert_ne!(m.a, m.b, "{m:?}");
        let (cr, cb) = (m.coverage(red), m.coverage(blue));
        assert!((cr - 0.5).abs() < 0.15 && (cb - 0.5).abs() < 0.15, "{m:?} red {cr} blue {cb}");
        assert_eq!(mm.mix[5 * 20 + 3], Mix::solid(red));
        assert_eq!(mm.coverage(-1, 5, 0), 1.0);
    }

    #[test]
    fn conflation_seam_is_a_mix_of_its_neighbours() {
        // Blue left, green right, abutting on a white canvas: the seam
        // column is blue at half coverage over green at half coverage over
        // white, as a renderer that anti-aliases each shape separately
        // leaves it. Not a colour of its own, and a 50/50 boundary.
        let (blue, green) = ([59u8, 130, 246], [34u8, 197, 94]);
        let seam = [
            (0.5 * 59.0 + 0.25 * 34.0 + 0.25 * 255.0) as u8,
            (0.5 * 130.0 + 0.25 * 197.0 + 0.25 * 255.0) as u8,
            (0.5 * 246.0 + 0.25 * 94.0 + 0.25 * 255.0) as u8,
        ];
        // On a white page (a 2 px frame), so the canvas is the background.
        let mut img = Raster::filled(25, 16, [255, 255, 255, 255]);
        for y in 2..14 {
            for x in 2..12 {
                img.set(x, y, [blue[0], blue[1], blue[2], 255]);
            }
            img.set(12, y, [seam[0], seam[1], seam[2], 255]);
            for x in 13..23 {
                img.set(x, y, [green[0], green[1], green[2], 255]);
            }
        }
        let an = analyse(&img, 0.03);
        let pal = estimate(&img, &an, &PaletteOpts::default());
        assert_eq!(pal.len(), 4, "{:?}", pal.rgb);
        let mm = unmix(&an, &pal);
        let b = pal.nearest(an.lab[7 * 25 + 5]);
        let g = pal.nearest(an.lab[7 * 25 + 18]);
        let m = mm.mix[7 * 25 + 12];
        let (cb, cg) = (m.coverage(b), m.coverage(g));
        assert!((cb - 0.5).abs() < 0.1 && (cg - 0.5).abs() < 0.1, "{m:?} blue {cb} green {cg}");
    }

    #[test]
    fn alpha_is_background_coverage() {
        let mut img = Raster::filled(4, 4, [0, 0, 0, 0]);
        img.set(1, 1, [200, 0, 0, 255]);
        img.set(2, 1, [200, 0, 0, 255]);
        img.set(1, 2, [200, 0, 0, 255]);
        img.set(2, 2, [200, 0, 0, 64]);
        let an = analyse(&img, 0.03);
        let pal = estimate(&img, &an, &PaletteOpts::default());
        let mm = unmix(&an, &pal);
        let red = pal.nearest(an.lab[5]);
        let m = mm.mix[2 * 4 + 2];
        assert_eq!(m.a, 0);
        assert_eq!(m.b, red);
        assert!((m.t - 64.0 / 255.0).abs() < 1e-3);
        assert_eq!(mm.mix[0], Mix::solid(0));
    }
}
