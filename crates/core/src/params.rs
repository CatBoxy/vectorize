//! User-facing parameters in Illustrator vocabulary (PLAN.md §1), mapped to
//! what the pipeline actually consumes, plus the M4 presets.
//!
//! JSON: `{"preset": "logo", "corners": 0.7}` starts from the Logo preset
//! and overrides what is given; without a preset the defaults apply.

pub use crate::polygon::PolygonCost;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    BlackAndWhite,
    Grayscale,
    Color,
}

/// How regions are assembled (Illustrator "Method").
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Adjacent regions share boundaries; no shape sits under another.
    Abutting,
    /// Regions are stacked back-to-front (VTracer "stacked", Illustrator "Overlapping").
    Overlapping,
}

/// Which curve fitter runs between corners. `Kurbo` fits the dense
/// sub-pixel contour; `Potrace` is Selinger's smooth + opticurve on the
/// polygon, kept for comparison in the bench.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CurveFit {
    Potrace,
    Kurbo,
}

/// Starting points for the common kinds of input (PLAN.md M4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Few colours, strong regularity, whole primitives.
    Logo,
    /// Black and white, soft corners, no snapping: a drawing stays a drawing.
    Sketch,
    /// More colours, light regularity.
    FlatIllustration,
    /// Nearest-neighbour upscale, every bend a corner, blocky output.
    PixelArt,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(from = "ParamsPatch")]
pub struct Params {
    /// The preset these parameters started from, if any.
    pub preset: Option<Preset>,
    pub mode: Mode,
    /// Maximum colours. `None` = automatic by OKLab merge distance.
    pub colors: Option<u32>,
    /// Palette clusters closer than this in OKLab are one colour (0.02 is
    /// about a just-noticeable difference; logos use far larger steps).
    pub color_merge: f32,
    /// Threshold for black-and-white mode, 0..=255.
    pub threshold: u8,
    /// Polygon straightness tolerance in source pixels (Illustrator "Paths").
    /// 0.1 = tight, 2.0 = loose. Potrace uses 0.5 on pixel corners; the
    /// sub-pixel contour is accurate enough for 0.2, which also keeps 1 px
    /// lines from turning into slivers.
    pub tolerance: f64,
    /// Corner sensitivity 0..=1 (Illustrator "Corners"). Higher = more corners.
    pub corners: f64,
    /// Minimum region area in pixels (Illustrator "Noise", Potrace `turdsize`).
    pub min_area: u32,
    pub method: Method,
    /// Snap nearly straight curves to lines (Illustrator "Snap Curves To Lines").
    pub snap_lines: bool,
    /// Drop the background colour (Illustrator "Ignore White").
    pub ignore_background: bool,
    /// Curve fitter between corners.
    pub curve_fit: CurveFit,
    /// Maximum distance between a fitted curve and the measured boundary, px.
    pub fit_accuracy: f64,
    /// Cost minimised by the polygon stage (PLAN.md M4): PolyFit's
    /// simplicity + accuracy + continuity, or Potrace's fewest segments.
    pub polygon: PolygonCost,
    /// Regularity 0..=1 (PLAN.md M4): how far nearly axis-aligned,
    /// parallel, equal or symmetric edges are snapped to exactly that,
    /// always within the fit accuracy. 0 disables.
    pub regularity: f64,
    /// Emit whole circles, ellipses, rounded rectangles, regular polygons
    /// and stars as those primitives when every sample agrees.
    pub primitives: bool,
    /// Nearest-neighbour upscale factor applied before tracing; the output
    /// is scaled back, so coordinates stay in source pixels. 1 = none.
    pub upscale: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            preset: None,
            mode: Mode::Color,
            colors: None,
            color_merge: 0.04,
            threshold: 128,
            tolerance: 0.2,
            corners: 0.5,
            min_area: 4,
            method: Method::Abutting,
            snap_lines: true,
            ignore_background: true,
            curve_fit: CurveFit::Kurbo,
            fit_accuracy: 0.35,
            polygon: PolygonCost::Polyfit,
            regularity: 0.5,
            primitives: true,
            upscale: 1,
        }
    }
}

impl Params {
    /// The parameters of a preset.
    pub fn preset(p: Preset) -> Self {
        let base = Self::default();
        let mut out = match p {
            Preset::Logo => Self { regularity: 1.0, ..base },
            Preset::Sketch => Self {
                mode: Mode::BlackAndWhite,
                tolerance: 0.3,
                corners: 0.3,
                min_area: 8,
                snap_lines: false,
                fit_accuracy: 0.5,
                regularity: 0.0,
                primitives: false,
                ..base
            },
            Preset::FlatIllustration => Self { color_merge: 0.025, min_area: 2, regularity: 0.25, ..base },
            Preset::PixelArt => Self {
                upscale: 4,
                corners: 1.0,
                min_area: 1,
                regularity: 1.0,
                primitives: false,
                ..base
            },
        };
        out.preset = Some(p);
        out
    }
}

/// Every field optional: what the JSON gives is laid over the preset (or
/// the defaults).
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(default)]
pub struct ParamsPatch {
    pub preset: Option<Preset>,
    pub mode: Option<Mode>,
    pub colors: Option<Option<u32>>,
    pub color_merge: Option<f32>,
    pub threshold: Option<u8>,
    pub tolerance: Option<f64>,
    pub corners: Option<f64>,
    pub min_area: Option<u32>,
    pub method: Option<Method>,
    pub snap_lines: Option<bool>,
    pub ignore_background: Option<bool>,
    pub curve_fit: Option<CurveFit>,
    pub fit_accuracy: Option<f64>,
    pub polygon: Option<PolygonCost>,
    pub regularity: Option<f64>,
    pub primitives: Option<bool>,
    pub upscale: Option<u32>,
}

impl From<ParamsPatch> for Params {
    fn from(p: ParamsPatch) -> Self {
        let mut out = match p.preset {
            Some(preset) => Params::preset(preset),
            None => Params::default(),
        };
        macro_rules! take {
            ($($f:ident),*) => { $( if let Some(v) = p.$f { out.$f = v; } )* };
        }
        take!(
            mode,
            colors,
            color_merge,
            threshold,
            tolerance,
            corners,
            min_area,
            method,
            snap_lines,
            ignore_background,
            curve_fit,
            fit_accuracy,
            polygon,
            regularity,
            primitives,
            upscale
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_without_preset_is_the_defaults_plus_overrides() {
        let p: Params = serde_json::from_str(r#"{"corners": 0.8, "polygon": "potrace"}"#).unwrap();
        assert_eq!(p.corners, 0.8);
        assert_eq!(p.polygon, PolygonCost::Potrace);
        assert_eq!(p.mode, Mode::Color);
        assert_eq!(p.preset, None);
        let d: Params = serde_json::from_str("{}").unwrap();
        assert_eq!(d, Params::default());
    }

    #[test]
    fn presets_apply_then_overrides_win() {
        let p: Params = serde_json::from_str(r#"{"preset": "sketch"}"#).unwrap();
        assert_eq!(p, Params::preset(Preset::Sketch));
        assert_eq!(p.mode, Mode::BlackAndWhite);
        assert!(!p.snap_lines && !p.primitives && p.regularity == 0.0);
        let p: Params = serde_json::from_str(r#"{"preset": "sketch", "snap_lines": true, "min_area": 2}"#).unwrap();
        assert!(p.snap_lines && p.min_area == 2 && p.mode == Mode::BlackAndWhite);
        let p: Params = serde_json::from_str(r#"{"preset": "pixel_art"}"#).unwrap();
        assert_eq!(p.upscale, 4);
        assert_eq!(p.corners, 1.0);
        let p: Params = serde_json::from_str(r#"{"preset": "logo"}"#).unwrap();
        assert_eq!(p.regularity, 1.0);
        let p: Params = serde_json::from_str(r#"{"preset": "flat_illustration", "colors": 12}"#).unwrap();
        assert_eq!(p.colors, Some(12));
        assert!(p.color_merge < 0.04);
        // Round trip through serialisation keeps the preset name.
        let s = serde_json::to_string(&Params::preset(Preset::Logo)).unwrap();
        assert!(s.contains("\"preset\":\"logo\""), "{s}");
        let back: Params = serde_json::from_str(&s).unwrap();
        assert_eq!(back, Params::preset(Preset::Logo));
    }
}
