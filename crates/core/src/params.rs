//! User-facing parameters in Illustrator vocabulary (PLAN.md §1), mapped to
//! what the pipeline actually consumes.

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

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Params {
    pub mode: Mode,
    /// Maximum colours. `None` = automatic by OKLab merge distance.
    pub colors: Option<u32>,
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
}

impl Default for Params {
    fn default() -> Self {
        Self {
            mode: Mode::Color,
            colors: None,
            threshold: 128,
            tolerance: 0.2,
            corners: 0.5,
            min_area: 4,
            method: Method::Abutting,
            snap_lines: true,
            ignore_background: true,
            curve_fit: CurveFit::Kurbo,
            fit_accuracy: 0.35,
        }
    }
}
