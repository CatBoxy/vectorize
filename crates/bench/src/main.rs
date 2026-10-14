//! Benchmark harness. See PLAN.md §2 M0.
//!
//!   bench build-corpus          rasterize corpus/src/**/*.svg + synthetic shapes
//!   bench run --tracer X        trace every image, score, write bench/out/X.json
//!   bench compare A B           regression gate between two result files
//!   bench report                bench/out/report.html from all result files

mod compare;
mod corpus;
mod metrics;
mod raster_svg;
mod report;
mod run;
mod seams;
mod svgstat;
mod tracers;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "bench", version)]
struct Cli {
    /// Project root (contains corpus/ and bench/).
    #[arg(long, default_value = ".")]
    root: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Rasterize corpus/src/<set>/*.svg to corpus/<set>/<size>/<name>.png and
    /// generate the synthetic set.
    BuildCorpus {
        #[arg(long, value_delimiter = ',', default_value = "64,128,256,512,1024")]
        sizes: Vec<u32>,
        /// Also JPEG-encode (quality 75) every Nth image. 0 disables.
        #[arg(long, default_value_t = 3)]
        jpeg_every: usize,
    },
    /// Trace every corpus image with one tracer and score it.
    Run {
        /// ours | potrace | vtracer | open-vectorizer | <path to executable taking IN OUT>
        #[arg(long)]
        tracer: String,
        /// Result name (default: tracer name).
        #[arg(long)]
        name: Option<String>,
        /// Restrict to sets, e.g. bw,color,synthetic.
        #[arg(long, value_delimiter = ',')]
        sets: Option<Vec<String>>,
        /// Restrict to sizes.
        #[arg(long, value_delimiter = ',')]
        sizes: Option<Vec<u32>>,
        /// Only images whose name contains this.
        #[arg(long)]
        filter: Option<String>,
        /// JSON params passed to `ours`.
        #[arg(long)]
        params: Option<String>,
        /// Skip the diff PNGs and re-rasterized output PNGs for the report.
        #[arg(long)]
        no_images: bool,
    },
    /// Fail if B regresses from A beyond the gate (L1 > 2%, nodes > 10% per size bucket).
    Compare { a: String, b: String },
    /// Build bench/out/report.html from every bench/out/*.json.
    Report {
        /// Number of worst images per tracer to show on the contact sheet.
        #[arg(long, default_value_t = 20)]
        worst: usize,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.root.canonicalize()?;
    match cli.cmd {
        Cmd::BuildCorpus { sizes, jpeg_every } => corpus::build(&root, &sizes, jpeg_every),
        Cmd::Run { tracer, name, sets, sizes, filter, params, no_images } => run::run(
            &root,
            run::RunOpts { tracer, name, sets, sizes, filter, params, images: !no_images },
        ),
        Cmd::Compare { a, b } => compare::compare(&root, &a, &b),
        Cmd::Report { worst } => report::report(&root, worst),
    }
}
