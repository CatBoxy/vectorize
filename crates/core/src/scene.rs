//! Output geometry before serialization: filled paths in source-pixel units.
//!
//! A [`Path`] is a sequence of SVG-like elements. Unlike `kurbo::BezPath`
//! it can hold circular arcs (`A` commands), which replace up to four
//! cubics each on circles and rounded corners (PLAN.md M3). `to_bez`
//! converts arcs to cubics for anything that needs a `BezPath`.

use kurbo::{BezPath, PathEl, Point, Rect, Shape as _};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Fill {
    Solid([u8; 4]),
}

/// One path element. `ArcTo` is a circular arc of radius `r` from the
/// current point to `end`, with SVG's large-arc and sweep flags;
/// `EllipseTo` an elliptical arc with radii `rx`, `ry` and the x axis
/// rotated by `rot` radians (PLAN.md M4 ellipse primitive).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum El {
    MoveTo(Point),
    LineTo(Point),
    CurveTo(Point, Point, Point),
    ArcTo { r: f64, large: bool, sweep: bool, end: Point },
    EllipseTo { rx: f64, ry: f64, rot: f64, large: bool, sweep: bool, end: Point },
    Close,
}

impl El {
    /// Where the element ends; `None` for `Close`.
    pub fn end(&self) -> Option<Point> {
        match *self {
            El::MoveTo(p) | El::LineTo(p) | El::CurveTo(_, _, p) | El::ArcTo { end: p, .. } | El::EllipseTo { end: p, .. } => Some(p),
            El::Close => None,
        }
    }

    pub fn from_path_el(el: PathEl) -> El {
        match el {
            PathEl::MoveTo(p) => El::MoveTo(p),
            PathEl::LineTo(p) => El::LineTo(p),
            PathEl::QuadTo(a, p) => El::CurveTo(a, a, p),
            PathEl::CurveTo(a, b, p) => El::CurveTo(a, b, p),
            PathEl::ClosePath => El::Close,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Path(pub Vec<El>);

impl Path {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn elements(&self) -> &[El] {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn push(&mut self, el: El) {
        self.0.push(el);
    }
    pub fn extend(&mut self, other: Path) {
        self.0.extend(other.0);
    }
    pub fn move_to(&mut self, p: Point) {
        self.0.push(El::MoveTo(p));
    }
    pub fn line_to(&mut self, p: Point) {
        self.0.push(El::LineTo(p));
    }
    pub fn curve_to(&mut self, a: Point, b: Point, p: Point) {
        self.0.push(El::CurveTo(a, b, p));
    }
    pub fn close_path(&mut self) {
        self.0.push(El::Close);
    }

    pub fn from_bez(b: &BezPath) -> Path {
        Path(b.elements().iter().map(|e| El::from_path_el(*e)).collect())
    }

    /// Cubic-only version, arcs flattened to cubics within 0.01 px.
    pub fn to_bez(&self) -> BezPath {
        let mut out = BezPath::new();
        let mut start = Point::ZERO;
        let mut cur = Point::ZERO;
        for el in &self.0 {
            match *el {
                El::MoveTo(p) => {
                    out.move_to(p);
                    start = p;
                    cur = p;
                }
                El::LineTo(p) => {
                    out.line_to(p);
                    cur = p;
                }
                El::CurveTo(a, b, p) => {
                    out.curve_to(a, b, p);
                    cur = p;
                }
                El::ArcTo { .. } | El::EllipseTo { .. } => {
                    let (rx, ry, rot, large, sweep, end) = match *el {
                        El::EllipseTo { rx, ry, rot, large, sweep, end } => (rx, ry, rot, large, sweep, end),
                        El::ArcTo { r, large, sweep, end } => (r, r, 0.0, large, sweep, end),
                        _ => unreachable!(),
                    };
                    let svg = kurbo::SvgArc {
                        from: cur,
                        to: end,
                        radii: kurbo::Vec2::new(rx, ry),
                        x_rotation: rot,
                        large_arc: large,
                        sweep,
                    };
                    match kurbo::Arc::from_svg_arc(&svg) {
                        Some(arc) => {
                            for seg in arc.append_iter(0.01) {
                                out.push(seg);
                            }
                        }
                        None => out.line_to(end),
                    }
                    cur = end;
                }
                El::Close => {
                    out.close_path();
                    cur = start;
                }
            }
        }
        out
    }

    /// Signed area (arcs included), as `kurbo::Shape::area`.
    pub fn area(&self) -> f64 {
        self.to_bez().area()
    }

    pub fn bounding_box(&self) -> Rect {
        self.to_bez().bounding_box()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub path: Path,
    pub fill: Fill,
}

impl Shape {
    /// Anchor points, the number a designer sees when they select the path.
    pub fn node_count(&self) -> usize {
        self.path.elements().iter().filter(|el| !matches!(el, El::Close)).count()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub width: u32,
    pub height: u32,
    /// Back to front.
    pub shapes: Vec<Shape>,
}

impl Scene {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, shapes: Vec::new() }
    }

    /// One SVG document per shape, each filled black: lets a test render the
    /// shapes separately and add up their coverages.
    pub fn scene_shapes_for_test(&self) -> Vec<String> {
        self.shapes
            .iter()
            .map(|s| {
                let one = Scene {
                    width: self.width,
                    height: self.height,
                    shapes: vec![Shape { path: s.path.clone(), fill: Fill::Solid([0, 0, 0, 255]) }],
                };
                crate::svg::write(&one)
            })
            .collect()
    }

    /// Scale every coordinate (and arc radius) by `s`.
    pub fn scale(&mut self, s: f64) {
        for shape in &mut self.shapes {
            for el in &mut shape.path.0 {
                *el = match *el {
                    El::MoveTo(p) => El::MoveTo((p.to_vec2() * s).to_point()),
                    El::LineTo(p) => El::LineTo((p.to_vec2() * s).to_point()),
                    El::CurveTo(a, b, p) => El::CurveTo((a.to_vec2() * s).to_point(), (b.to_vec2() * s).to_point(), (p.to_vec2() * s).to_point()),
                    El::ArcTo { r, large, sweep, end } => El::ArcTo { r: r * s, large, sweep, end: (end.to_vec2() * s).to_point() },
                    El::EllipseTo { rx, ry, rot, large, sweep, end } => {
                        El::EllipseTo { rx: rx * s, ry: ry * s, rot, large, sweep, end: (end.to_vec2() * s).to_point() }
                    }
                    El::Close => El::Close,
                };
            }
        }
    }

    pub fn distinct_colors(&self) -> usize {
        let mut cs: Vec<[u8; 4]> = self
            .shapes
            .iter()
            .map(|s| match s.fill {
                Fill::Solid(c) => c,
            })
            .collect();
        cs.sort_unstable();
        cs.dedup();
        cs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arc_round_trips_through_cubics() {
        // A full circle of radius 10 as two half arcs.
        let mut p = Path::new();
        p.move_to(Point::new(10.0, 0.0));
        p.push(El::ArcTo { r: 10.0, large: false, sweep: true, end: Point::new(-10.0, 0.0) });
        p.push(El::ArcTo { r: 10.0, large: false, sweep: true, end: Point::new(10.0, 0.0) });
        p.close_path();
        let area = p.area().abs();
        assert!((area - std::f64::consts::PI * 100.0).abs() < 0.5, "area {area}");
        let bb = p.bounding_box();
        assert!((bb.width() - 20.0).abs() < 0.05 && (bb.height() - 20.0).abs() < 0.05, "{bb:?}");
    }
}
