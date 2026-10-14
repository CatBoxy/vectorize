//! Image-space metrics between the input raster and the re-rasterized trace.
//! Both are flattened on white before they get here.

use vectorize_core::Raster;

#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct ImageMetrics {
    /// Mean per-pixel CIE76 ΔE in CIELAB (0 = identical; 2.3 ≈ just noticeable).
    pub delta_e: f64,
    /// Fraction of pixels with ΔE > 10, i.e. visibly wrong pixels, in percent.
    pub wrong_pct: f64,
    /// Structural similarity on L*, 11×11 Gaussian window, σ = 1.5.
    pub ssim: f64,
}

pub fn compare(a: &Raster, b: &Raster) -> ImageMetrics {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let n = (a.width * a.height) as usize;
    let mut la = vec![0f32; n];
    let mut lb = vec![0f32; n];
    let mut sum_de = 0.0f64;
    let mut wrong = 0usize;
    for i in 0..n {
        let pa = srgb_to_lab(&a.data[i * 4..i * 4 + 3]);
        let pb = srgb_to_lab(&b.data[i * 4..i * 4 + 3]);
        la[i] = pa[0];
        lb[i] = pb[0];
        let de = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
        sum_de += de as f64;
        if de > 10.0 {
            wrong += 1;
        }
    }
    ImageMetrics {
        delta_e: sum_de / n as f64,
        wrong_pct: 100.0 * wrong as f64 / n as f64,
        ssim: ssim(&la, &lb, a.width as usize, a.height as usize),
    }
}

/// Per-pixel ΔE map scaled to 0..255 for the diff images in the report.
pub fn diff_image(a: &Raster, b: &Raster) -> image::RgbaImage {
    let mut out = image::RgbaImage::new(a.width, a.height);
    for (i, px) in out.pixels_mut().enumerate() {
        let pa = srgb_to_lab(&a.data[i * 4..i * 4 + 3]);
        let pb = srgb_to_lab(&b.data[i * 4..i * 4 + 3]);
        let de = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
        let t = (de / 40.0).clamp(0.0, 1.0);
        // white → red heat; faint grey copy of the input underneath so you can locate it.
        let base = 235.0 + 0.08 * (pa[0] - 50.0);
        let r = base + (255.0 - base) * t;
        let g = base * (1.0 - t);
        let bl = base * (1.0 - t);
        *px = image::Rgba([r as u8, g as u8, bl as u8, 255]);
    }
    out
}

fn srgb_to_lab(rgb: &[u8]) -> [f32; 3] {
    fn lin(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }
    let (r, g, b) = (lin(rgb[0]), lin(rgb[1]), lin(rgb[2]));
    let x = (0.4124564 * r + 0.3575761 * g + 0.1804375 * b) / 0.95047;
    let y = 0.2126729 * r + 0.7151522 * g + 0.0721750 * b;
    let z = (0.0193339 * r + 0.1191920 * g + 0.9503041 * b) / 1.08883;
    fn f(t: f32) -> f32 {
        if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 }
    }
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// Standard SSIM (Wang et al. 2004) on a 0..100 luminance plane.
fn ssim(a: &[f32], b: &[f32], w: usize, h: usize) -> f64 {
    if w < 11 || h < 11 {
        return 1.0;
    }
    let k: Vec<f32> = {
        let sigma = 1.5f32;
        let mut k: Vec<f32> = (-5..=5).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
        let s: f32 = k.iter().sum();
        k.iter_mut().for_each(|v| *v /= s);
        k
    };
    let blur = |src: &[f32]| -> Vec<f32> {
        let mut tmp = vec![0f32; w * h];
        for y in 0..h {
            for x in 5..w - 5 {
                let mut s = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    s += kv * src[y * w + x + j - 5];
                }
                tmp[y * w + x] = s;
            }
        }
        let mut out = vec![0f32; w * h];
        for y in 5..h - 5 {
            for x in 0..w {
                let mut s = 0.0;
                for (j, kv) in k.iter().enumerate() {
                    s += kv * tmp[(y + j - 5) * w + x];
                }
                out[y * w + x] = s;
            }
        }
        out
    };
    let mul = |p: &[f32], q: &[f32]| -> Vec<f32> { p.iter().zip(q).map(|(x, y)| x * y).collect() };
    let mu_a = blur(a);
    let mu_b = blur(b);
    let saa = blur(&mul(a, a));
    let sbb = blur(&mul(b, b));
    let sab = blur(&mul(a, b));
    // L = 100 (luminance range), K1 = 0.01, K2 = 0.03
    let c1 = (0.01f32 * 100.0).powi(2);
    let c2 = (0.03f32 * 100.0).powi(2);
    let mut sum = 0.0f64;
    let mut n = 0usize;
    for y in 5..h - 5 {
        for x in 5..w - 5 {
            let i = y * w + x;
            let (ma, mb) = (mu_a[i], mu_b[i]);
            let va = saa[i] - ma * ma;
            let vb = sbb[i] - mb * mb;
            let cov = sab[i] - ma * mb;
            let s = ((2.0 * ma * mb + c1) * (2.0 * cov + c2)) / ((ma * ma + mb * mb + c1) * (va + vb + c2));
            sum += s as f64;
            n += 1;
        }
    }
    if n == 0 { 1.0 } else { sum / n as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identical_is_perfect() {
        let r = Raster::filled(32, 32, [40, 90, 200, 255]);
        let m = compare(&r, &r);
        assert!(m.delta_e.abs() < 1e-6 && m.wrong_pct == 0.0 && (m.ssim - 1.0).abs() < 1e-4);
    }
    #[test]
    fn black_vs_white_is_bad() {
        let a = Raster::filled(32, 32, [0, 0, 0, 255]);
        let b = Raster::filled(32, 32, [255, 255, 255, 255]);
        let m = compare(&a, &b);
        assert!(m.delta_e > 90.0 && m.wrong_pct == 100.0);
    }
}
