//! Stage dumps for the inspector (PLAN.md M5): the cheap intermediate
//! results a trace can hand back next to the SVG. They are filled only when
//! the caller asks for them through [`crate::trace_with`], so the ordinary
//! path pays nothing.
//!
//! What exists: the palette (stage 1), the region map (stage 3) and the
//! polygon of every boundary chain (stage 5). The coverage field, the
//! boundary graph with its sub-pixel points and the fitted curves per edge
//! are not dumped: the curves are the SVG itself, and the other two are
//! large and only readable with a dedicated viewer.

use kurbo::Point;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StageDumps {
    /// Palette colours (sRGB) in label order. Index 0 is the transparent
    /// background in colour mode and the white paper in black-and-white
    /// mode; it is never painted.
    pub palette: Vec<[u8; 3]>,
    /// Label of the background colour, if one was detected.
    pub background: Option<u16>,
    /// The region map, in the units the pipeline traced (the upscaled
    /// grid for the pixel-art preset).
    pub regions: Option<RegionMap>,
    /// Polygon vertices of every boundary chain before fitting, in source
    /// pixels.
    pub polygons: Vec<Polygon>,
}

/// One region id per pixel and the palette label of every region.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegionMap {
    pub width: u32,
    pub height: u32,
    /// Row-major, `width * height`, ids compact from 0.
    pub id: Vec<u32>,
    /// Palette label per region id.
    pub color: Vec<u16>,
}

/// The polygon stage's vertices for one boundary chain.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Polygon {
    pub closed: bool,
    pub pts: Vec<(f64, f64)>,
}

impl Polygon {
    pub fn from_points(pts: &[Point], closed: bool) -> Self {
        Self { closed, pts: pts.iter().map(|p| (p.x, p.y)).collect() }
    }
}

impl StageDumps {
    /// Scale every coordinate (polygon vertices) by `s`; used to bring the
    /// pixel-art upscale back to source pixels. The region map keeps its
    /// own grid and dimensions.
    pub fn scale(&mut self, s: f64) {
        for poly in &mut self.polygons {
            for p in &mut poly.pts {
                p.0 *= s;
                p.1 *= s;
            }
        }
    }
}
