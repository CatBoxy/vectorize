//! vectorize-wasm: the browser entry point (PLAN.md M5).
//!
//! One function, `trace(bytes, params_json)`, decodes a PNG or JPEG, runs
//! [`vectorize_core::trace`] and returns `{ svg, stats, stage_dumps? }`.
//! Every failure is a `{ error: { code, message } }` object, never a thrown
//! exception, so the worker always gets an answer it can forward.
//! `trace_rgba` takes pixels the browser already decoded (anything it can
//! draw on a canvas: WebP, GIF, BMP, or a downscaled copy of a large image).
//!
//! A panic in the core cannot unwind on wasm32: it traps the instance and
//! the call throws a `RuntimeError` on the JavaScript side. The hook
//! installed at start records the panic message so the worker can report
//! it through [`last_panic`]; the app then discards that worker (its
//! memory is not to be trusted after a trap) and starts a fresh one.
//!
//! Two feature sets build two modules from this crate: `tracer` (the
//! default) and `pdf` (`svg_to_pdf` through svg2pdf), which the app fetches
//! only when someone exports a PDF. Both share this file's panic hook.

use serde::Serialize;
use std::cell::RefCell;
use wasm_bindgen::prelude::*;

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Longest image edge accepted, px. The web app downsizes to 2048 before
/// calling; this is the hard stop for anyone calling the module directly.
pub const MAX_EDGE: u32 = 4096;

#[wasm_bindgen(start)]
pub fn start() {
    std::panic::set_hook(Box::new(|info| {
        LAST_PANIC.with(|p| *p.borrow_mut() = Some(info.to_string()));
        console_error_panic_hook::hook(info);
    }));
    vectorize_core::set_time_source(now_ms);
}

/// `performance.now()` of whatever global scope runs us (a window or a
/// worker), else `Date.now()`.
fn now_ms() -> f64 {
    let global = js_sys::global();
    if let Ok(perf) = js_sys::Reflect::get(&global, &JsValue::from_str("performance")) {
        if let Ok(now) = js_sys::Reflect::get(&perf, &JsValue::from_str("now")) {
            if let Some(f) = now.dyn_ref::<js_sys::Function>() {
                if let Some(ms) = f.call0(&perf).ok().and_then(|v| v.as_f64()) {
                    return ms;
                }
            }
        }
    }
    js_sys::Date::now()
}

/// The message of the last panic, if there was one, and forgets it.
#[wasm_bindgen]
pub fn last_panic() -> Option<String> {
    LAST_PANIC.with(|p| p.borrow_mut().take())
}

#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Whether this module carries the PDF exporter.
#[wasm_bindgen]
pub fn has_pdf() -> bool {
    cfg!(feature = "pdf")
}

/// Whether this module carries the tracer.
#[wasm_bindgen]
pub fn has_tracer() -> bool {
    cfg!(feature = "tracer")
}

#[derive(Serialize)]
struct ErrorOut<'a> {
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: String,
}

/// `{ error: { code, message } }`. Codes: `decode`, `too_large`, `params`,
/// `svg`, `pdf`, `internal`.
fn error(code: &str, message: impl Into<String>) -> JsValue {
    serde_wasm_bindgen::to_value(&ErrorOut { error: ErrorBody { code, message: message.into() } }).unwrap_or(JsValue::NULL)
}

#[cfg(feature = "tracer")]
mod tracer {
    use super::{error, MAX_EDGE};
    use serde::Serialize;
    use std::io::Cursor;
    use vectorize_core::dumps::{Polygon, RegionMap, StageDumps};
    use vectorize_core::{Params, Raster, Stats};
    use wasm_bindgen::prelude::*;

    #[derive(Serialize)]
    struct Out<'a> {
        svg: &'a str,
        stats: &'a Stats,
        #[serde(skip_serializing_if = "Option::is_none")]
        stage_dumps: Option<DumpsOut>,
    }

    #[derive(Serialize)]
    struct DumpsOut {
        /// `#rrggbb` per palette label; index 0 is the unpainted background.
        palette: Vec<String>,
        background: Option<u16>,
        polygons: Vec<Polygon>,
        /// The region map goes across as pixels (`rgba`, set after
        /// serialisation), one colour per region, transparent where nothing
        /// is painted.
        regions: Option<RegionsOut>,
    }

    #[derive(Serialize)]
    struct RegionsOut {
        width: u32,
        height: u32,
        count: u32,
    }

    /// Trace a PNG or JPEG. `params_json` is the core's parameter JSON
    /// (`{"preset":"logo","corners":0.7}`, see `crates/core/src/params.rs`);
    /// the extra key `"dumps": true` asks for the inspector's stage dumps.
    #[wasm_bindgen]
    pub fn trace(bytes: &[u8], params_json: &str) -> JsValue {
        match decode(bytes) {
            Ok(img) => run(img, params_json),
            Err((code, msg)) => error(code, msg),
        }
    }

    /// Trace pixels the browser already decoded: `rgba` is `width * height * 4`
    /// bytes as `ImageData` holds them (straight alpha).
    #[wasm_bindgen]
    pub fn trace_rgba(width: u32, height: u32, rgba: &[u8], params_json: &str) -> JsValue {
        if width == 0 || height == 0 {
            return error("decode", "empty image");
        }
        if width.max(height) > MAX_EDGE {
            return error("too_large", format!("{width}×{height} px is over the {MAX_EDGE} px limit on the long edge"));
        }
        let want = width as usize * height as usize * 4;
        if rgba.len() != want {
            return error("decode", format!("expected {want} bytes of RGBA for {width}×{height}, got {}", rgba.len()));
        }
        run(Raster::from_rgba8(width, height, rgba.to_vec()), params_json)
    }

    fn decode(bytes: &[u8]) -> Result<Raster, (&'static str, String)> {
        if bytes.is_empty() {
            return Err(("decode", "empty input".into()));
        }
        let fmt = image::guess_format(bytes).map_err(|_| ("decode", "not a PNG or JPEG (unrecognised file header)".to_string()))?;
        if !matches!(fmt, image::ImageFormat::Png | image::ImageFormat::Jpeg) {
            return Err(("decode", format!("{fmt:?} is not decoded here; only PNG and JPEG are (the app draws other formats on a canvas and calls trace_rgba)")));
        }
        let (w, h) = image::ImageReader::with_format(Cursor::new(bytes), fmt)
            .into_dimensions()
            .map_err(|e| ("decode", e.to_string()))?;
        if w == 0 || h == 0 {
            return Err(("decode", "empty image".into()));
        }
        if w.max(h) > MAX_EDGE {
            return Err(("too_large", format!("{w}×{h} px is over the {MAX_EDGE} px limit on the long edge")));
        }
        let img = image::ImageReader::with_format(Cursor::new(bytes), fmt)
            .decode()
            .map_err(|e| ("decode", e.to_string()))?
            .into_rgba8();
        Ok(Raster::from_rgba8(w, h, img.into_raw()))
    }

    fn run(img: Raster, params_json: &str) -> JsValue {
        let text = if params_json.trim().is_empty() { "{}" } else { params_json };
        let mut value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return error("params", format!("params is not valid JSON: {e}")),
        };
        let want_dumps = value.get("dumps").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(obj) = value.as_object_mut() {
            obj.remove("dumps");
        }
        let params: Params = match serde_json::from_value(value) {
            Ok(p) => p,
            Err(e) => return error("params", format!("bad parameter: {e}")),
        };
        let mut dumps = want_dumps.then(StageDumps::default);
        let traced = vectorize_core::trace_with(&img, &params, dumps.as_mut());
        let dumps_out = dumps.as_ref().map(|d| DumpsOut {
            palette: d.palette.iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect(),
            background: d.background,
            polygons: d.polygons.clone(),
            regions: d.regions.as_ref().map(|r| RegionsOut { width: r.width, height: r.height, count: r.color.len() as u32 }),
        });
        let out = Out { svg: &traced.svg, stats: &traced.stats, stage_dumps: dumps_out };
        let js = match serde_wasm_bindgen::to_value(&out) {
            Ok(v) => v,
            Err(e) => return error("internal", e.to_string()),
        };
        // A million region ids as a JSON array would dwarf the SVG; ship the
        // map as pixels instead, ready for an ImageData.
        if let Some(d) = dumps.as_ref() {
            if let Some(r) = d.regions.as_ref() {
                let rgba = region_pixels(r, d.background);
                let arr = js_sys::Uint8Array::from(rgba.as_slice());
                if let Ok(sd) = js_sys::Reflect::get(&js, &JsValue::from_str("stage_dumps")) {
                    if let Ok(reg) = js_sys::Reflect::get(&sd, &JsValue::from_str("regions")) {
                        let _ = js_sys::Reflect::set(&reg, &JsValue::from_str("rgba"), &arr);
                    }
                }
            }
        }
        js
    }

    /// One distinct colour per region (golden-ratio hues), transparent for
    /// the background and the unpainted label 0.
    fn region_pixels(r: &RegionMap, background: Option<u16>) -> Vec<u8> {
        let colors: Vec<[u8; 4]> = r
            .color
            .iter()
            .enumerate()
            .map(|(i, &label)| if label == 0 || Some(label) == background { [0, 0, 0, 0] } else { hue_color(i as u32) })
            .collect();
        let mut out = vec![0u8; r.id.len() * 4];
        for (px, &id) in out.chunks_exact_mut(4).zip(&r.id) {
            px.copy_from_slice(&colors.get(id as usize).copied().unwrap_or([0, 0, 0, 0]));
        }
        out
    }

    fn hue_color(i: u32) -> [u8; 4] {
        let h = (i as f64 * 0.618_033_988_749_895 + 0.13).fract();
        let (s, l) = (0.72, 0.55);
        let c = (1.0 - (2.0 * l - 1.0f64).abs()) * s;
        let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0f64).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match (h * 6.0) as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        [((r + m) * 255.0).round() as u8, ((g + m) * 255.0).round() as u8, ((b + m) * 255.0).round() as u8, 200]
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn region_pixels_are_distinct_and_background_is_clear() {
            let r = RegionMap { width: 2, height: 1, id: vec![0, 1], color: vec![1, 2] };
            let px = region_pixels(&r, Some(1));
            assert_eq!(&px[0..4], &[0, 0, 0, 0]);
            assert_eq!(px[7], 200);
            assert_ne!(hue_color(3), hue_color(4));
        }

        #[test]
        fn decode_rejects_junk_and_accepts_png() {
            assert_eq!(decode(b"not an image").unwrap_err().0, "decode");
            assert_eq!(decode(b"").unwrap_err().0, "decode");
            let mut png = Vec::new();
            image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255]))
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();
            let img = decode(&png).unwrap();
            assert_eq!((img.width, img.height), (3, 2));
            assert_eq!(&img.data[..4], &[10, 20, 30, 255]);
        }
    }
}

#[cfg(feature = "tracer")]
pub use tracer::{trace, trace_rgba};

#[cfg(feature = "pdf")]
mod pdf {
    use super::error;
    use wasm_bindgen::prelude::*;

    /// Convert an SVG (ours, or any plain one with filled paths) to a PDF
    /// as a `Uint8Array`, or `{ error }`. This build is text-free: `<text>`
    /// elements are dropped.
    #[wasm_bindgen]
    pub fn svg_to_pdf(svg: &str) -> JsValue {
        let opts = svg2pdf::usvg::Options::default();
        let tree = match svg2pdf::usvg::Tree::from_str(svg, &opts) {
            Ok(t) => t,
            Err(e) => return error("svg", e.to_string()),
        };
        match svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), svg2pdf::PageOptions::default()) {
            Ok(bytes) => js_sys::Uint8Array::from(bytes.as_slice()).into(),
            Err(e) => error("pdf", e.to_string()),
        }
    }
}

#[cfg(feature = "pdf")]
pub use pdf::svg_to_pdf;
