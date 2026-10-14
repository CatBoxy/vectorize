//! Corpus layout:
//!   corpus/src/<set>/<name>.svg         sources (fetched by corpus/fetch.sh or generated)
//!   corpus/<set>/<size>/<name>.png      rasterized, transparent canvas, fit in size×size
//!   corpus/<set>/<size>/<name>.jpg      every Nth image, quality 75, flattened on white
//!   corpus/index.json                   list of every image with set/size/name/kind

use crate::raster_svg;
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub set: String,
    pub size: u32,
    pub name: String,
    /// "png" or "jpg"
    pub kind: String,
    /// Path relative to project root.
    pub path: String,
    /// Source SVG relative to project root.
    pub source: String,
}

pub fn build(root: &Path, sizes: &[u32], jpeg_every: usize) -> Result<()> {
    let src = root.join("corpus/src");
    fs::create_dir_all(&src)?;
    write_synthetic(&src.join("synthetic"))?;

    let mut sets: Vec<PathBuf> = fs::read_dir(&src)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    sets.sort();

    let mut jobs: Vec<(String, PathBuf)> = Vec::new();
    for set in &sets {
        let set_name = set.file_name().unwrap().to_string_lossy().to_string();
        let mut files: Vec<PathBuf> = fs::read_dir(set)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "svg").unwrap_or(false))
            .collect();
        files.sort();
        for f in files {
            jobs.push((set_name.clone(), f));
        }
    }
    eprintln!("{} sources in {} sets", jobs.len(), sets.len());

    let entries: Vec<Entry> = jobs
        .par_iter()
        .enumerate()
        .flat_map(|(i, (set, svg_path))| {
            let name = svg_path.file_stem().unwrap().to_string_lossy().to_string();
            let data = match fs::read(svg_path) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("skip {}: {}", svg_path.display(), e);
                    return Vec::new();
                }
            };
            let mut out = Vec::new();
            for &size in sizes {
                let r = match raster_svg::render_fit(&data, size) {
                    Ok(r) => r,
                    Err(e) => {
                        eprintln!("skip {} @{}: {}", svg_path.display(), size, e);
                        continue;
                    }
                };
                let dir = root.join("corpus").join(set).join(size.to_string());
                let _ = fs::create_dir_all(&dir);
                let img = raster_svg::to_image(&r);
                let png = dir.join(format!("{name}.png"));
                if img.save(&png).is_ok() {
                    out.push(Entry {
                        set: set.clone(),
                        size,
                        name: name.clone(),
                        kind: "png".into(),
                        path: rel(root, &png),
                        source: rel(root, svg_path),
                    });
                }
                if jpeg_every > 0 && i % jpeg_every == 0 {
                    let flat = raster_svg::to_image(&raster_svg::flatten(&r, [255, 255, 255]));
                    let rgb = image::DynamicImage::ImageRgba8(flat).to_rgb8();
                    let jpg = dir.join(format!("{name}.jpg"));
                    if let Ok(f) = fs::File::create(&jpg) {
                        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(std::io::BufWriter::new(f), 75);
                        if enc.encode_image(&rgb).is_ok() {
                            out.push(Entry {
                                set: set.clone(),
                                size,
                                name: name.clone(),
                                kind: "jpg".into(),
                                path: rel(root, &jpg),
                                source: rel(root, svg_path),
                            });
                        }
                    }
                }
            }
            out
        })
        .collect();

    let mut entries = entries;
    entries.sort_by(|a, b| (&a.set, a.size, &a.name, &a.kind).cmp(&(&b.set, b.size, &b.name, &b.kind)));
    fs::write(root.join("corpus/index.json"), serde_json::to_string_pretty(&entries)?)?;
    eprintln!("wrote corpus/index.json with {} images", entries.len());
    Ok(())
}

pub fn load_index(root: &Path) -> Result<Vec<Entry>> {
    let p = root.join("corpus/index.json");
    let s = fs::read_to_string(&p).with_context(|| format!("{} (run `bench build-corpus` first)", p.display()))?;
    Ok(serde_json::from_str(&s)?)
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// 20 shapes with analytic ground truth. Flat colours only; every one of them
/// has a known ideal node count, listed in corpus/src/synthetic/README.md.
fn write_synthetic(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let shapes: Vec<(&str, String)> = vec![
        ("circle", svg(r##"<circle cx="50" cy="50" r="40" fill="#000"/>"##)),
        ("ring", svg(r##"<path fill="#000" fill-rule="evenodd" d="M50 10a40 40 0 1 0 0 80a40 40 0 1 0 0-80zM50 25a25 25 0 1 1 0 50a25 25 0 1 1 0-50z"/>"##)),
        ("ellipse", svg(r##"<ellipse cx="50" cy="50" rx="42" ry="26" fill="#1d4ed8"/>"##)),
        ("square", svg(r##"<rect x="15" y="15" width="70" height="70" fill="#000"/>"##)),
        ("square-rot30", svg(r##"<rect x="20" y="20" width="60" height="60" fill="#000" transform="rotate(30 50 50)"/>"##)),
        ("rounded-rect", svg(r##"<rect x="10" y="20" width="80" height="60" rx="14" fill="#dc2626"/>"##)),
        ("triangle", svg(r##"<polygon points="50,10 92,88 8,88" fill="#000"/>"##)),
        ("star5", svg(r##"<polygon points="50,6 61.8,38.2 95.1,38.2 68.1,58.8 79.4,91.8 50,72 20.6,91.8 31.9,58.8 4.9,38.2 38.2,38.2" fill="#f59e0b"/>"##)),
        ("hexagon", svg(r##"<polygon points="50,6 88,28 88,72 50,94 12,72 12,28" fill="#000"/>"##)),
        ("thin-lines", svg(r##"<rect x="10" y="20" width="80" height="1" fill="#000"/><rect x="10" y="40" width="80" height="1.5" fill="#000"/><rect x="10" y="60" width="80" height="2" fill="#000"/><rect x="20" y="10" width="1" height="80" fill="#000"/>"##)),
        ("diagonal-line", svg(r##"<path d="M10 90L90 10" stroke="#000" stroke-width="1.5" fill="none"/>"##)),
        ("checker", svg(r##"<rect width="100" height="100" fill="#fff"/><rect x="0" y="0" width="50" height="50" fill="#000"/><rect x="50" y="50" width="50" height="50" fill="#000"/>"##)),
        ("three-color-junction", svg(r##"<rect width="100" height="100" fill="#fff"/><path d="M50 50L50 0L100 0L100 100Z" fill="#ef4444"/><path d="M50 50L100 100L0 100Z" fill="#22c55e"/><path d="M50 50L0 100L0 0L50 0Z" fill="#3b82f6"/>"##)),
        ("letter-e", svg(r##"<path fill="#000" d="M20 10h60v14H36v16h38v14H36v22h46v14H20z"/>"##)),
        ("letter-s", svg(r##"<path fill="#000" d="M78 30c0-13-11-21-27-21C33 9 22 18 22 31c0 12 8 18 24 22l10 2.5c9 2.3 13 5 13 10.5 0 6-6 10-15 10-10 0-17-5-18-13H20c1 15 13 26 34 26 19 0 32-10 32-25 0-12-8-19-23-22.5L52 39c-9-2-14-4.5-14-10 0-5.5 5.5-9 14-9 9 0 14 4.5 15 11z"/>"##)),
        ("teardrop", svg(r##"<path fill="#0ea5e9" d="M50 6C50 6 86 50 86 66a36 36 0 1 1-72 0C14 50 50 6 50 6z"/>"##)),
        ("crescent", svg(r##"<path fill="#000" d="M60 10a40 40 0 1 0 0 80a32 32 0 1 1 0-80z"/>"##)),
        ("concentric", svg(r##"<circle cx="50" cy="50" r="45" fill="#111827"/><circle cx="50" cy="50" r="33" fill="#f3f4f6"/><circle cx="50" cy="50" r="21" fill="#111827"/><circle cx="50" cy="50" r="9" fill="#f3f4f6"/>"##)),
        ("pie", svg(r##"<path fill="#8b5cf6" d="M50 50L50 5A45 45 0 1 1 11 72z"/><path fill="#ec4899" d="M50 50L11 72A45 45 0 0 1 50 5z"/>"##)),
        ("tiny-details", svg(r##"<rect x="8" y="8" width="84" height="84" rx="6" fill="#000"/><circle cx="30" cy="30" r="2" fill="#fff"/><circle cx="50" cy="30" r="1.2" fill="#fff"/><circle cx="70" cy="30" r="0.7" fill="#fff"/><rect x="20" y="60" width="60" height="0.8" fill="#fff"/>"##)),
    ];
    for (name, body) in &shapes {
        fs::write(dir.join(format!("{name}.svg")), body)?;
    }
    fs::write(
        dir.join("README.md"),
        "Synthetic shapes with analytic ground truth, 100×100 user units, generated by `bench build-corpus`.\n\
         Ideal node counts (anchors): circle 4 (or 1 arc path), ring 8, ellipse 4, square 4, square-rot30 4,\n\
         rounded-rect 8, triangle 3, star5 10, hexagon 6, thin-lines 16, diagonal-line 4, checker 8 (2 shapes),\n\
         three-color-junction 3 shapes, letter-e 12, letter-s ~24, teardrop 3, crescent 3, concentric 16,\n\
         pie 2 shapes × 3-4, tiny-details tests speckle filtering: the r=0.7 dot should vanish at 64 px and survive at 512 px.\n",
    )?;
    Ok(())
}

fn svg(body: &str) -> String {
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{body}</svg>"#)
}
