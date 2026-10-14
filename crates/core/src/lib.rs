//! vectorize-core: raster → SVG tracer.
//!
//! Pure library: no I/O, no threads, no allocation outside `Vec`. The CLI, the
//! bench harness and the WASM wrapper all call [`trace`].
//!
//! Pipeline (see PLAN.md §1):
//!   Raster → Palette → CoverageFields → RegionMap → BoundaryGraph → PolyGraph
//!   → CurveGraph → Scene → SVG + Stats
//!
//! M1 is the black-and-white pipeline ([`bw`]): darkness coverage,
//! Potrace-style path decomposition, sub-pixel contours from anti-aliasing,
//! optimal polygon, corner classification and curve fitting.
//! M2 is the colour pipeline ([`color`]): OKLab palette, per-pixel unmixing,
//! regions, and a boundary graph whose every edge is fitted once and shared
//! by the faces on both sides.

pub mod bitmap;
pub mod bw;
pub mod chain;
pub mod color;
pub mod contour;
pub mod coverage;
pub mod curve;
pub mod decompose;
pub mod fit;
pub mod graph;
pub mod mix;
pub mod palette;
pub mod params;
pub mod polygon;
pub mod raster;
pub mod regions;
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
    /// Wall time per pipeline stage, in order. Zeros on wasm32.
    #[serde(default)]
    pub stage_ms: Vec<(String, f64)>,
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
    let mut clock = StageClock::start();
    let scene = match params.mode {
        params::Mode::BlackAndWhite => bw::trace_bw(img, params, &mut clock),
        params::Mode::Grayscale | params::Mode::Color => color::trace_color(img, params, &mut clock),
    };
    finish(scene, clock)
}

/// Assemble `Scene` → SVG string + stats. Shared by every tracer version.
pub fn finish(scene: Scene, mut clock: StageClock) -> Traced {
    let svg = svg::write(&scene);
    clock.lap("emit");
    let stats = Stats {
        paths: scene.shapes.len(),
        nodes: scene.shapes.iter().map(|s| s.node_count()).sum(),
        colors: scene.distinct_colors(),
        svg_bytes: svg.len(),
        stage_ms: clock.laps,
    };
    Traced { svg, stats, scene }
}

/// Per-stage wall clock. A no-op on wasm32, where `Instant` is unavailable.
#[derive(Debug, Default)]
pub struct StageClock {
    #[cfg(not(target_arch = "wasm32"))]
    t: Option<std::time::Instant>,
    pub laps: Vec<(String, f64)>,
}

impl StageClock {
    pub fn start() -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            t: Some(std::time::Instant::now()),
            laps: Vec::new(),
        }
    }

    /// Record the time since the previous lap under `name`.
    pub fn lap(&mut self, name: &str) {
        #[cfg(not(target_arch = "wasm32"))]
        let ms = {
            let now = std::time::Instant::now();
            let ms = self.t.map(|t| now.duration_since(t).as_secs_f64() * 1000.0).unwrap_or(0.0);
            self.t = Some(now);
            ms
        };
        #[cfg(target_arch = "wasm32")]
        let ms = 0.0;
        self.laps.push((name.to_string(), ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::El;
    use kurbo::ParamCurve;

    /// The M1 pipeline, which these tests exercise.
    fn bw() -> Params {
        Params { mode: params::Mode::BlackAndWhite, ..Params::default() }
    }

    /// Anti-aliased disc: coverage by 8×8 supersampling.
    fn disc(size: u32, cx: f64, cy: f64, r: f64) -> Raster {
        let mut img = Raster::filled(size, size, [255, 255, 255, 255]);
        for y in 0..size {
            for x in 0..size {
                let mut inside = 0;
                for sy in 0..8 {
                    for sx in 0..8 {
                        let px = x as f64 + (sx as f64 + 0.5) / 8.0;
                        let py = y as f64 + (sy as f64 + 0.5) / 8.0;
                        if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                            inside += 1;
                        }
                    }
                }
                let v = 255 - (inside * 255 / 64) as u8;
                img.set(x, y, [v, v, v, 255]);
            }
        }
        img
    }

    #[test]
    fn square_is_four_nodes() {
        let mut img = Raster::filled(16, 16, [255, 255, 255, 255]);
        for y in 4..12 {
            for x in 4..12 {
                img.set(x, y, [0, 0, 0, 255]);
            }
        }
        let out = trace(&img, &bw());
        assert_eq!(out.stats.paths, 1);
        assert_eq!(out.stats.nodes, 4, "{}", out.svg);
        assert_eq!(out.stats.colors, 1);
        assert!(out.svg.contains("viewBox=\"0 0 16 16\""));
        assert!(out.svg.contains("#000000"));
        let bbox = out.scene.shapes[0].path.bounding_box();
        assert!((bbox.x0 - 4.0).abs() < 1e-6 && (bbox.x1 - 12.0).abs() < 1e-6, "{bbox:?}");
    }

    #[test]
    fn blank_image_is_empty() {
        let img = Raster::filled(8, 8, [255, 255, 255, 255]);
        let out = trace(&img, &bw());
        assert_eq!(out.stats.paths, 0);
    }

    #[test]
    fn circle_is_few_nodes_and_subpixel_accurate() {
        let (cx, cy, r) = (64.0, 64.0, 40.0);
        let img = disc(128, cx, cy, r);
        let out = trace(&img, &Params { fit_accuracy: 0.2, ..bw() });
        assert_eq!(out.stats.paths, 1);
        assert!(out.stats.nodes <= 8, "{} nodes: {}", out.stats.nodes, out.svg);
        // Walk the path densely and measure radial error.
        let path = &out.scene.shapes[0].path;
        let mut worst: f64 = 0.0;
        for seg in path.to_bez().segments() {
            for i in 0..=50 {
                let p = seg.eval(i as f64 / 50.0);
                let d = ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt();
                worst = worst.max((d - r).abs());
            }
        }
        assert!(worst < 0.25, "max radial error {worst:.3} px");
    }

    #[test]
    fn ring_has_two_loops_with_opposite_winding() {
        let mut img = disc(96, 48.0, 48.0, 36.0);
        let hole = disc(96, 48.0, 48.0, 18.0);
        for i in (0..img.data.len()).step_by(4) {
            let v = 255 - hole.data[i];
            // Hole: where the inner disc is dark, make the pixel light.
            img.data[i] = img.data[i].max(v);
            img.data[i + 1] = img.data[i];
            img.data[i + 2] = img.data[i];
        }
        let out = trace(&img, &bw());
        let els = out.scene.shapes[0].path.elements();
        let moves = els.iter().filter(|e| matches!(e, El::MoveTo(_))).count();
        assert_eq!(moves, 2, "{}", out.svg);
        // Signed area of the whole path: outer minus hole.
        let area = out.scene.shapes[0].path.area().abs();
        let expect = std::f64::consts::PI * (36.0f64.powi(2) - 18.0f64.powi(2));
        assert!((area - expect).abs() / expect < 0.02, "area {area:.1} vs {expect:.1}");
    }

    #[test]
    fn thin_line_survives() {
        let mut img = Raster::filled(40, 20, [255, 255, 255, 255]);
        for x in 5..35 {
            img.set(x, 10, [0, 0, 0, 255]);
        }
        let out = trace(&img, &Params { min_area: 1, ..bw() });
        assert_eq!(out.stats.paths, 1);
        assert_eq!(out.stats.nodes, 4, "{}", out.svg);
        let bbox = out.scene.shapes[0].path.bounding_box();
        assert!((bbox.height() - 1.0).abs() < 1e-6 && (bbox.width() - 30.0).abs() < 1e-6, "{bbox:?}");
    }

    #[test]
    fn speckles_are_dropped() {
        let mut img = Raster::filled(32, 32, [255, 255, 255, 255]);
        for y in 8..24 {
            for x in 8..24 {
                img.set(x, y, [0, 0, 0, 255]);
            }
        }
        img.set(2, 2, [0, 0, 0, 255]);
        img.set(30, 30, [0, 0, 0, 255]);
        img.set(31, 30, [0, 0, 0, 255]);
        let out = trace(&img, &bw());
        assert_eq!(out.stats.nodes, 4, "{}", out.svg);
    }

    #[test]
    fn noise_never_panics_and_paths_close() {
        let mut seed = 0x1234_5678u32;
        for size in [1u32, 2, 3, 7, 16, 33] {
            let mut img = Raster::filled(size, size, [255, 255, 255, 255]);
            for y in 0..size {
                for x in 0..size {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let v = (seed % 256) as u8;
                    img.set(x, y, [v, v, v, (seed >> 8) as u8 | 128]);
                }
            }
            let out = trace(&img, &Params { min_area: 1, ..bw() });
            for s in &out.scene.shapes {
                let els = s.path.elements();
                assert!(matches!(els.last(), Some(El::Close)));
                for el in els {
                    for p in match *el {
                        El::MoveTo(p) | El::LineTo(p) => vec![p],
                        El::CurveTo(a, b, p) => vec![a, b, p],
                        El::ArcTo { end, .. } => vec![end],
                        El::Close => vec![],
                    } {
                        assert!(p.x.is_finite() && p.y.is_finite());
                    }
                }
            }
        }
    }
}
