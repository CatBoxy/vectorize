//! Minimal SVG writer. Coordinates are rounded to 0.01 px (Potrace uses 0.1;
//! the finer grid costs ~1 byte per coordinate and keeps sub-pixel fits honest).

use crate::scene::El;
use crate::{Fill, Scene};
use std::fmt::Write;

pub fn write(scene: &Scene) -> String {
    let mut s = String::with_capacity(256 + scene.shapes.len() * 128);
    let _ = write!(
        s,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" width=\"{}\" height=\"{}\">",
        scene.width, scene.height, scene.width, scene.height
    );
    for shape in &scene.shapes {
        let Fill::Solid(c) = shape.fill;
        s.push_str("<path fill=\"");
        push_color(&mut s, c);
        s.push('"');
        if c[3] != 255 {
            let _ = write!(s, " fill-opacity=\"{}\"", fmt(c[3] as f64 / 255.0));
        }
        s.push_str(" d=\"");
        push_path(&mut s, shape.path.elements());
        s.push_str("\"/>");
    }
    s.push_str("</svg>");
    s
}

fn push_color(s: &mut String, c: [u8; 4]) {
    let _ = write!(s, "#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
}

fn push_path(s: &mut String, els: &[El]) {
    for el in els {
        match *el {
            El::MoveTo(p) => {
                let _ = write!(s, "M{} {}", fmt(p.x), fmt(p.y));
            }
            El::LineTo(p) => {
                let _ = write!(s, "L{} {}", fmt(p.x), fmt(p.y));
            }
            El::CurveTo(a, b, p) => {
                let _ = write!(s, "C{} {} {} {} {} {}", fmt(a.x), fmt(a.y), fmt(b.x), fmt(b.y), fmt(p.x), fmt(p.y));
            }
            El::ArcTo { r, large, sweep, end } => {
                let _ = write!(
                    s,
                    "A{} {} 0 {} {} {} {}",
                    fmt(r),
                    fmt(r),
                    large as u8,
                    sweep as u8,
                    fmt(end.x),
                    fmt(end.y)
                );
            }
            El::EllipseTo { rx, ry, rot, large, sweep, end } => {
                let _ = write!(
                    s,
                    "A{} {} {} {} {} {} {}",
                    fmt(rx),
                    fmt(ry),
                    fmt(rot.to_degrees()),
                    large as u8,
                    sweep as u8,
                    fmt(end.x),
                    fmt(end.y)
                );
            }
            El::Close => s.push('Z'),
        }
    }
}

/// Shortest decimal with at most 2 fractional digits, no trailing zeros.
pub fn fmt(v: f64) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() {
        format!("{}", r as i64)
    } else {
        let t = format!("{:.2}", r);
        t.trim_end_matches('0').to_string()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn fmt_is_compact() {
        assert_eq!(super::fmt(3.0), "3");
        assert_eq!(super::fmt(3.10), "3.1");
        assert_eq!(super::fmt(3.14159), "3.14");
        assert_eq!(super::fmt(-0.004), "0");
    }
}
