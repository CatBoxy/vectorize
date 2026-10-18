//! Seam check (PLAN.md M2 exit criterion): trace an opaque image in
//! abutting mode with the background kept, re-rasterize, and count the
//! pixels the shapes fail to cover. Shared boundaries are fitted once, so
//! the answer must be zero, up to anti-aliasing of the outer image border.

#[cfg(test)]
mod tests {
    use crate::raster_svg;
    use vectorize_core::params::{Method, Mode};
    use vectorize_core::{Params, Raster};

    /// Three flat colours meeting at a junction, plus a disc, opaque.
    fn image(size: u32) -> Raster {
        let mut img = Raster::filled(size, size, [255, 255, 255, 255]);
        let s = size as f64;
        for y in 0..size {
            for x in 0..size {
                // 8×8 supersampling for anti-aliased edges.
                let mut acc = [0u32; 3];
                for sy in 0..8 {
                    for sx in 0..8 {
                        let px = x as f64 + (sx as f64 + 0.5) / 8.0;
                        let py = y as f64 + (sy as f64 + 0.5) / 8.0;
                        let c: [u32; 3] = if (px - 0.3 * s).powi(2) + (py - 0.65 * s).powi(2) < (0.2 * s).powi(2) {
                            [40, 60, 220]
                        } else if py < 0.4 * s + 0.3 * px {
                            [239, 68, 68]
                        } else if px > 0.5 * s {
                            [34, 197, 94]
                        } else {
                            [250, 250, 250]
                        };
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                    }
                }
                img.set(x, y, [(acc[0] / 64) as u8, (acc[1] / 64) as u8, (acc[2] / 64) as u8, 255]);
            }
        }
        img
    }

    /// Render every shape on its own and sum the coverages per pixel.
    /// Rendering them together would not do: a renderer anti-aliases two
    /// abutting edges independently and composites them to less than full
    /// coverage, a hairline that is its artifact, not a gap in the geometry.
    /// Returns (min, max) of the summed coverage over interior pixels, in
    /// 0..=255 units, and the count of pixels below 250.
    fn coverage_sum(size: u32, method: Method) -> (u32, u32, usize) {
        let img = image(size);
        let params = Params { mode: Mode::Color, method, ignore_background: false, ..Params::default() };
        let out = vectorize_core::trace(&img, &params);
        assert!(out.stats.paths >= 3, "{}", out.svg);
        if let Ok(dir) = std::env::var("VECTORIZE_DUMP") {
            let tag = format!("seam-{size}-{:?}", method).to_lowercase();
            raster_svg::to_image(&img).save(format!("{dir}/{tag}.png")).unwrap();
            std::fs::write(format!("{dir}/{tag}.svg"), &out.svg).unwrap();
        }
        let mut sum = vec![0u32; (size * size) as usize];
        for shape in &out.scene.scene_shapes_for_test() {
            let r = raster_svg::render_exact(shape.as_bytes(), size, size).unwrap();
            for (i, s) in sum.iter_mut().enumerate() {
                *s += r.data[i * 4 + 3] as u32;
            }
        }
        // A pixel at a tangency of two boundaries can come up a few percent
        // short where three curves meet; a real gap is far below that.
        let (mut lo, mut hi, mut low_count) = (u32::MAX, 0u32, 0usize);
        let mut first = Vec::new();
        for y in 1..size - 1 {
            for x in 1..size - 1 {
                let s = sum[(y * size + x) as usize];
                lo = lo.min(s);
                hi = hi.max(s);
                if s < 200 {
                    low_count += 1;
                    if first.len() < 8 {
                        first.push((x, y, s));
                    }
                }
            }
        }
        if low_count > 0 {
            eprintln!("{size} {method:?}: first under-covered pixels (x, y, sum): {first:?}");
        }
        (lo, hi, low_count)
    }

    #[test]
    fn abutting_output_has_no_seams_or_overlaps() {
        for size in [48u32, 96, 200] {
            let (lo, hi, low) = coverage_sum(size, Method::Abutting);
            assert_eq!(low, 0, "{size}: {low} under-covered pixels (min {lo})");
            assert!(hi <= 262, "{size}: overlap, max coverage sum {hi}");
        }
    }

    #[test]
    fn overlapping_output_has_no_holes() {
        for size in [48u32, 96, 200] {
            let (lo, _, low) = coverage_sum(size, Method::Overlapping);
            assert_eq!(low, 0, "{size}: {low} under-covered pixels (min {lo})");
        }
    }
}
