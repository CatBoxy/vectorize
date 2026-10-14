use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use vectorize_core::{Params, Raster};

/// Trace a raster image (PNG/JPEG) to SVG.
#[derive(Parser, Debug)]
#[command(name = "vectorize", version)]
struct Args {
    /// Input PNG or JPEG.
    input: PathBuf,
    /// Output SVG (default: stdout).
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Parameters as JSON, e.g. '{"mode":"black_and_white","tolerance":0.3}'.
    #[arg(short, long)]
    params: Option<String>,
    /// Print stats as JSON to stderr.
    #[arg(long)]
    stats: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let img = image::open(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let raster = Raster::from_rgba8(w, h, img.into_raw());
    let params: Params = match &args.params {
        Some(p) => serde_json::from_str(p).context("parsing --params")?,
        None => Params::default(),
    };
    let out = vectorize_core::trace(&raster, &params);
    match &args.output {
        Some(p) => std::fs::write(p, &out.svg).with_context(|| format!("writing {}", p.display()))?,
        None => println!("{}", out.svg),
    }
    if args.stats {
        eprintln!("{}", serde_json::to_string(&out.stats)?);
    }
    Ok(())
}
