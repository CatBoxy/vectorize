//! Stage 1-2 for black-and-white mode: a darkness field in `[0, 1]` per pixel.
//!
//! The input is flattened on white and reduced to sRGB luminance, exactly what
//! Potrace sees through a PGM, then remapped so that `threshold` lands on 0.5.
//! Anti-aliased edge pixels keep their fractional value: that fraction is the
//! sub-pixel evidence the contour stage uses (PLAN.md M1, RESEARCH.md §4.1).

use crate::bitmap::Bitmap;
use crate::Raster;

#[derive(Clone, Debug)]
pub struct Coverage {
    pub width: usize,
    pub height: usize,
    /// Row-major, `width * height`.
    pub f: Vec<f32>,
}

impl Coverage {
    /// Darkness of pixel `(x, y)`; 0 outside the image (the canvas is white).
    #[inline]
    pub fn get(&self, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return 0.0;
        }
        self.f[y as usize * self.width + x as usize]
    }
}

/// Darkness field: 1 = black, 0 = white, with `threshold` (0..=255, luminance)
/// mapped onto 0.5 so that the 0.5 isoline is the user's black/white cut.
pub fn darkness(img: &Raster, threshold: u8) -> Coverage {
    let n = (img.width * img.height) as usize;
    let mut f = Vec::with_capacity(n);
    // Darkness at which a pixel turns black.
    let iso = 1.0 - threshold as f32 / 255.0;
    for i in 0..n {
        let p = &img.data[i * 4..i * 4 + 4];
        let a = p[3] as u32;
        let lum = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
        // Composite on white.
        let lum = (lum * a + 255 * (255 - a)) as f32 / (255.0 * 255.0);
        let d = 1.0 - lum;
        let c = if iso <= 0.0 {
            1.0
        } else if iso >= 1.0 {
            0.0
        } else if d >= iso {
            0.5 + 0.5 * (d - iso) / (1.0 - iso)
        } else {
            0.5 * d / iso
        };
        f.push(c.clamp(0.0, 1.0));
    }
    Coverage { width: img.width as usize, height: img.height as usize, f }
}

/// Pixels at or above 0.5 darkness are foreground.
pub fn binarize(cov: &Coverage) -> Bitmap {
    let mut bm = Bitmap::new(cov.width, cov.height);
    for (i, &v) in cov.f.iter().enumerate() {
        bm.bits[i] = (v >= 0.5) as u8;
    }
    bm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_maps_to_half() {
        let mut img = Raster::filled(3, 1, [255, 255, 255, 255]);
        img.set(1, 0, [0, 0, 0, 255]);
        img.set(2, 0, [100, 100, 100, 255]);
        let c = darkness(&img, 128);
        assert!(c.get(0, 0) < 0.01);
        assert!((c.get(1, 0) - 1.0).abs() < 1e-6);
        assert!(c.get(2, 0) > 0.5);
        // A high threshold makes mid grey black, a low one makes it white.
        assert!(darkness(&img, 200).get(2, 0) > 0.5);
        assert!(darkness(&img, 50).get(2, 0) < 0.5);
        // Outside the image is white.
        assert_eq!(c.get(-1, 0), 0.0);
    }

    #[test]
    fn alpha_is_coverage() {
        let mut img = Raster::filled(1, 1, [0, 0, 0, 0]);
        img.set(0, 0, [0, 0, 0, 64]);
        let c = darkness(&img, 128);
        let expect = 0.5 * (64.0 / 255.0) / (127.0 / 255.0);
        assert!((c.get(0, 0) - expect).abs() < 1e-3);
    }
}
