//! vectorize-core: raster → SVG tracer.
//!
//! Pure library: no I/O, no threads, no allocation outside `Vec`. The CLI, the
//! bench harness and the WASM wrapper all call [`trace`].
//!
//! Pipeline (see PLAN.md §1):
//!   Raster → Palette → CoverageFields → RegionMap → BoundaryGraph → PolyGraph
//!   → CurveGraph → Scene → SVG + Stats
//!
//! M0 ships only the types, the SVG writer and a stub `trace` that emits the
//! bounding rectangle of the non-background pixels. Every later milestone
//! replaces one stage at a time behind the same signature.

pub mod params;
pub mod raster;
pub mod scene;
pub mod svg;

pub use params::Params;
pub use raster::Raster;
pub use scene::{Fill, Scene, Shape};

/// Numbers the harness and the UI report for every trace.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Stats {
    pub paths: usize,
    pub nodes: usize,
    pub colors: usize,
    pub svg_bytes: usize,
}

/// Output of a trace: the SVG text and its statistics.
#[derive(Clone, Debug)]
pub struct Traced {
    pub svg: String,
    pub stats: Stats,
    pub scene: Scene,
}

/// Trace a raster image into an SVG scene.
pub fn trace(img: &Raster, params: &Params) -> Traced {
    let scene = stub::trace_stub(img, params);
    finish(scene)
}

/// Assemble `Scene` → SVG string + stats. Shared by every tracer version.
pub fn finish(scene: Scene) -> Traced {
    let svg = svg::write(&scene);
    let stats = Stats {
        paths: scene.shapes.len(),
        nodes: scene.shapes.iter().map(|s| s.node_count()).sum(),
        colors: scene.distinct_colors(),
        svg_bytes: svg.len(),
    };
    Traced { svg, stats, scene }
}

mod stub {
    use crate::{Fill, Params, Raster, Scene, Shape};
    use kurbo::{BezPath, Rect};

    /// M0 placeholder: one rectangle covering the non-background pixels, filled
    /// with the mean colour of those pixels. Exists so the harness runs end to
    /// end and scores it worst.
    pub fn trace_stub(img: &Raster, _params: &Params) -> Scene {
        let bg = img.corner_color();
        let (mut x0, mut y0, mut x1, mut y1) = (img.width, img.height, 0, 0);
        let (mut sum, mut n) = ([0u64; 3], 0u64);
        for y in 0..img.height {
            for x in 0..img.width {
                let p = img.get(x, y);
                if p[3] < 128 {
                    continue;
                }
                // Transparent background: every opaque pixel is foreground.
                if bg[3] >= 128 && color_dist2(p, bg) < 24 * 24 {
                    continue;
                }
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
                for c in 0..3 {
                    sum[c] += p[c] as u64;
                }
                n += 1;
            }
        }
        let mut scene = Scene::new(img.width, img.height);
        if n == 0 {
            return scene;
        }
        let mean = [
            (sum[0] / n) as u8,
            (sum[1] / n) as u8,
            (sum[2] / n) as u8,
            255,
        ];
        let rect = Rect::new(x0 as f64, y0 as f64, x1 as f64, y1 as f64);
        let path: BezPath = kurbo::Shape::to_path(&rect, 0.0);
        scene.shapes.push(Shape {
            path,
            fill: Fill::Solid(mean),
        });
        scene
    }

    fn color_dist2(a: [u8; 4], b: [u8; 4]) -> i32 {
        (0..3)
            .map(|c| {
                let d = a[c] as i32 - b[c] as i32;
                d * d
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_traces_a_square() {
        let mut img = Raster::filled(16, 16, [255, 255, 255, 255]);
        for y in 4..12 {
            for x in 4..12 {
                img.set(x, y, [0, 0, 0, 255]);
            }
        }
        let out = trace(&img, &Params::default());
        assert_eq!(out.stats.paths, 1);
        assert_eq!(out.stats.colors, 1);
        assert!(out.svg.contains("viewBox=\"0 0 16 16\""));
        assert!(out.svg.contains("#000000"));
    }

    #[test]
    fn stub_on_blank_image_is_empty() {
        let img = Raster::filled(8, 8, [255, 255, 255, 255]);
        let out = trace(&img, &Params::default());
        assert_eq!(out.stats.paths, 0);
    }
}
