//! SVG → RGBA8 via resvg. Used for building the corpus and for re-rasterizing
//! tracer output. Both go through the same function so the comparison is fair.

use anyhow::{anyhow, Context, Result};
use resvg::{tiny_skia, usvg};
use vectorize_core::Raster;

/// Render `svg` so that its viewBox fits in a `size`×`size` square, preserving
/// aspect ratio (icon on a transparent canvas). Returns straight RGBA.
pub fn render_fit(svg: &[u8], size: u32) -> Result<Raster> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg, &opt).context("parsing SVG")?;
    let s = tree.size();
    let (sw, sh) = (s.width() as f64, s.height() as f64);
    if sw <= 0.0 || sh <= 0.0 {
        return Err(anyhow!("SVG has no size"));
    }
    let scale = size as f64 / sw.max(sh);
    let w = (sw * scale).round().max(1.0) as u32;
    let h = (sh * scale).round().max(1.0) as u32;
    render_scaled(&tree, w, h, scale as f32)
}

/// Render `svg` at exactly `w`×`h` pixels (its viewBox is assumed to already be
/// `w`×`h`, which is what every tracer emits for a `w`×`h` input).
pub fn render_exact(svg: &[u8], w: u32, h: u32) -> Result<Raster> {
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(svg, &opt).context("parsing output SVG")?;
    let s = tree.size();
    let sx = w as f32 / s.width();
    let sy = h as f32 / s.height();
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("pixmap"))?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());
    Ok(to_raster(&pixmap))
}

fn render_scaled(tree: &usvg::Tree, w: u32, h: u32, scale: f32) -> Result<Raster> {
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or_else(|| anyhow!("pixmap"))?;
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Ok(to_raster(&pixmap))
}

fn to_raster(pixmap: &tiny_skia::Pixmap) -> Raster {
    let mut data = Vec::with_capacity((pixmap.width() * pixmap.height() * 4) as usize);
    for p in pixmap.pixels() {
        let c = p.demultiply();
        data.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Raster::from_rgba8(pixmap.width(), pixmap.height(), data)
}

pub fn to_image(r: &Raster) -> image::RgbaImage {
    image::RgbaImage::from_raw(r.width, r.height, r.data.clone()).expect("raster size")
}

pub fn from_image(img: &image::RgbaImage) -> Raster {
    Raster::from_rgba8(img.width(), img.height(), img.as_raw().clone())
}

/// Composite onto an opaque background (white by default). Transparent input
/// and transparent output must be judged on the same canvas.
pub fn flatten(r: &Raster, bg: [u8; 3]) -> Raster {
    let mut out = r.clone();
    for i in (0..out.data.len()).step_by(4) {
        let a = out.data[i + 3] as u32;
        if a == 255 {
            continue;
        }
        for c in 0..3 {
            let v = out.data[i + c] as u32;
            out.data[i + c] = ((v * a + bg[c] as u32 * (255 - a) + 127) / 255) as u8;
        }
        out.data[i + 3] = 255;
    }
    out
}
