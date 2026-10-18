//! Output geometry before serialization: filled paths in source-pixel units.

use kurbo::{BezPath, PathEl};

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Fill {
    Solid([u8; 4]),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub path: BezPath,
    pub fill: Fill,
}

impl Shape {
    /// Anchor points, the number a designer sees when they select the path.
    pub fn node_count(&self) -> usize {
        self.path
            .elements()
            .iter()
            .filter(|el| !matches!(el, PathEl::ClosePath))
            .count()
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
