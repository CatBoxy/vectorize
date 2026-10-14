//! Adapters: every tracer is `fn(png_path, out_svg_path) -> svg text`. Ours runs
//! in-process; the baselines are subprocesses, so their time includes process
//! start-up (noted in the report).

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use vectorize_core::{Params, Raster};

pub struct TraceOut {
    pub svg: String,
    pub ms: f64,
}

#[derive(Clone, Debug)]
pub enum Tracer {
    Ours(Params),
    Potrace,
    VTracer,
    OpenVectorizer,
    /// Any executable invoked as `exe IN.png OUT.svg`.
    External(PathBuf),
}

impl Tracer {
    pub fn parse(name: &str, params: Option<&str>) -> Result<Self> {
        Ok(match name {
            "ours" => {
                let p = match params {
                    Some(s) => serde_json::from_str(s).context("--params")?,
                    None => Params::default(),
                };
                Tracer::Ours(p)
            }
            "potrace" => Tracer::Potrace,
            "vtracer" => Tracer::VTracer,
            "open-vectorizer" => Tracer::OpenVectorizer,
            other => Tracer::External(PathBuf::from(other)),
        })
    }

    /// Black-and-white tracers are only scored on sets whose images are monochrome.
    pub fn supports_color(&self) -> bool {
        !matches!(self, Tracer::Potrace)
    }

    pub fn trace(&self, input: &Path, raster: &Raster, is_bw: bool, out_svg: &Path, tmp: &Path) -> Result<TraceOut> {
        match self {
            Tracer::Ours(params) => {
                let mut p = params.clone();
                if is_bw {
                    p.mode = vectorize_core::params::Mode::BlackAndWhite;
                }
                let t = Instant::now();
                let out = vectorize_core::trace(raster, &p);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::fs::write(out_svg, &out.svg)?;
                Ok(TraceOut { svg: out.svg, ms })
            }
            Tracer::Potrace => {
                // potrace reads PNM/BMP; write a PGM of the flattened luminance.
                let pgm = tmp.join(format!("{}.pgm", stem(input)));
                write_pgm(raster, &pgm)?;
                let exe = which("potrace").ok_or_else(|| anyhow!("potrace not found in PATH or ~/.local/bin"))?;
                // -k 0.5 threshold, --svg, keep natural pixel units (resolution 72 dpi → 1 unit = 1 px)
                let t = Instant::now();
                let st = Command::new(exe)
                    .args(["--svg", "-k", "0.5", "-r", "72", "-o"])
                    .arg(out_svg)
                    .arg(&pgm)
                    .status()?;
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let _ = std::fs::remove_file(&pgm);
                if !st.success() {
                    return Err(anyhow!("potrace failed on {}", input.display()));
                }
                let svg = std::fs::read_to_string(out_svg)?;
                let svg = normalize_potrace(&svg, raster.width, raster.height);
                std::fs::write(out_svg, &svg)?;
                Ok(TraceOut { svg, ms })
            }
            Tracer::VTracer => {
                let exe = which("vtracer").ok_or_else(|| anyhow!("vtracer not found (cargo install vtracer)"))?;
                // VTracer reads transparent pixels as black; feed it the image flattened on
                // white, which is also the canvas the metrics are computed on.
                let flat_png = tmp.join(format!("{}-{}.png", stem(input), raster.width));
                crate::raster_svg::to_image(&crate::raster_svg::flatten(raster, [255, 255, 255])).save(&flat_png)?;
                let mut cmd = Command::new(exe);
                cmd.arg("--input").arg(&flat_png).arg("--output").arg(out_svg);
                if is_bw {
                    cmd.args(["--colormode", "bw"]);
                }
                let t = Instant::now();
                let st = cmd.status()?;
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let _ = std::fs::remove_file(&flat_png);
                if !st.success() {
                    return Err(anyhow!("vtracer failed on {}", input.display()));
                }
                let svg = std::fs::read_to_string(out_svg)?;
                Ok(TraceOut { svg, ms })
            }
            Tracer::OpenVectorizer => {
                let exe = which("open-vectorizer")
                    .or_else(|| which("openvec"))
                    .ok_or_else(|| anyhow!("open-vectorizer CLI not found"))?;
                let t = Instant::now();
                let st = Command::new(exe).arg(input).arg("-o").arg(out_svg).status()?;
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                if !st.success() {
                    return Err(anyhow!("open-vectorizer failed on {}", input.display()));
                }
                let svg = std::fs::read_to_string(out_svg)?;
                Ok(TraceOut { svg, ms })
            }
            Tracer::External(exe) => {
                let t = Instant::now();
                let st = Command::new(exe).arg(input).arg(out_svg).status()?;
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                if !st.success() {
                    return Err(anyhow!("{} failed on {}", exe.display(), input.display()));
                }
                let svg = std::fs::read_to_string(out_svg)?;
                Ok(TraceOut { svg, ms })
            }
        }
    }
}

fn stem(p: &Path) -> String {
    p.file_stem().unwrap().to_string_lossy().to_string()
}

pub fn which(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(home.clone()).join(".local/bin"));
        dirs.push(PathBuf::from(home).join(".cargo/bin"));
    }
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), name.to_string()]
    } else {
        vec![name.to_string()]
    };
    dirs.into_iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)).collect::<Vec<_>>())
        .find(|p| p.is_file())
}

/// Flatten on white, luminance → binary PGM (P5).
fn write_pgm(r: &Raster, path: &Path) -> Result<()> {
    let flat = crate::raster_svg::flatten(r, [255, 255, 255]);
    let mut out = format!("P5\n{} {}\n255\n", r.width, r.height).into_bytes();
    for i in (0..flat.data.len()).step_by(4) {
        let (rr, g, b) = (flat.data[i] as u32, flat.data[i + 1] as u32, flat.data[i + 2] as u32);
        out.push(((rr * 299 + g * 587 + b * 114) / 1000) as u8);
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Potrace emits pt units with a flipped transform group. Rewrite the header so
/// the viewBox is `0 0 w h` in pixels; the inner transform already maps its
/// coordinates to that space when `-r 72` is used (1 pt = 1 px).
fn normalize_potrace(svg: &str, w: u32, h: u32) -> String {
    // Replace the <svg ...> opening tag wholesale.
    if let (Some(s), Some(e)) = (svg.find("<svg"), svg.find('>')) {
        if e > s {
            let head = format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\""
            );
            // potrace: <svg version="1.0" ... width="Wpt" height="Hpt" viewBox="0 0 W H" preserveAspectRatio=...>
            return format!("{}{}", head, &svg[e..]);
        }
    }
    svg.to_string()
}
