//! `bench run`: trace every corpus image with one tracer, score, write JSON.
//!
//! Output: bench/out/<name>.json  (Results)
//!         bench/out/<name>/<set>/<size>/<image>.svg|.out.png|.diff.png

use crate::{corpus, metrics, raster_svg, svgstat, tracers::Tracer};
use anyhow::{Context, Result};
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct RunOpts {
    pub tracer: String,
    pub name: Option<String>,
    pub sets: Option<Vec<String>>,
    pub sizes: Option<Vec<u32>>,
    pub filter: Option<String>,
    pub params: Option<String>,
    pub images: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Row {
    pub set: String,
    pub size: u32,
    pub name: String,
    pub kind: String,
    pub input: String,
    pub svg: String,
    pub out_png: Option<String>,
    pub diff_png: Option<String>,
    pub ms: f64,
    pub paths: usize,
    pub nodes: usize,
    pub bytes: usize,
    #[serde(flatten)]
    pub m: metrics::ImageMetrics,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Bucket {
    pub n: usize,
    pub delta_e: f64,
    pub wrong_pct: f64,
    pub ssim: f64,
    pub nodes: f64,
    pub paths: f64,
    pub bytes: f64,
    pub ms: f64,
    pub failures: usize,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Results {
    pub name: String,
    pub tracer: String,
    pub params: Option<String>,
    pub date: String,
    pub in_process: bool,
    pub rows: Vec<Row>,
    /// Keyed by "<set>/<size>" and "<set>/all" and "all/<size>" and "all/all".
    pub buckets: BTreeMap<String, Bucket>,
}

/// Sets whose images are monochrome, so B/W-only tracers may be scored on them.
pub fn is_bw_set(set: &str) -> bool {
    set.ends_with("-bw") || set == "bw"
}

pub fn run(root: &Path, o: RunOpts) -> Result<()> {
    let tracer = Tracer::parse(&o.tracer, o.params.as_deref())?;
    let name = o.name.clone().unwrap_or_else(|| o.tracer.replace('/', "_"));
    let out_dir = root.join("bench/out").join(&name);
    let tmp = root.join("bench/out/tmp");
    fs::create_dir_all(&out_dir)?;
    fs::create_dir_all(&tmp)?;

    let index = corpus::load_index(root)?;
    let entries: Vec<_> = index
        .into_iter()
        .filter(|e| o.sets.as_ref().map(|s| s.contains(&e.set)).unwrap_or(true))
        .filter(|e| o.sizes.as_ref().map(|s| s.contains(&e.size)).unwrap_or(true))
        .filter(|e| o.filter.as_ref().map(|f| e.name.contains(f.as_str())).unwrap_or(true))
        .filter(|e| tracer.supports_color() || is_bw_set(&e.set))
        .collect();
    eprintln!("{}: {} images", name, entries.len());
    let done = AtomicUsize::new(0);
    let total = entries.len();

    let rows: Vec<Row> = entries
        .par_iter()
        .map(|e| {
            let input = root.join(&e.path);
            let dir = out_dir.join(&e.set).join(e.size.to_string());
            let _ = fs::create_dir_all(&dir);
            let base = format!("{}{}", e.name, if e.kind == "jpg" { "-jpg" } else { "" });
            let svg_path = dir.join(format!("{base}.svg"));
            let mut row = Row {
                set: e.set.clone(),
                size: e.size,
                name: e.name.clone(),
                kind: e.kind.clone(),
                input: e.path.clone(),
                svg: rel(root, &svg_path),
                out_png: None,
                diff_png: None,
                ms: 0.0,
                paths: 0,
                nodes: 0,
                bytes: 0,
                m: Default::default(),
                error: None,
            };
            let result = (|| -> Result<()> {
                let img = image::open(&input).with_context(|| input.display().to_string())?.to_rgba8();
                let raster = raster_svg::from_image(&img);
                let t = tracer.trace(&input, &raster, is_bw_set(&e.set), &svg_path, &tmp)?;
                row.ms = t.ms;
                let st = svgstat::stats(&t.svg);
                row.paths = st.paths;
                row.nodes = st.nodes;
                row.bytes = st.bytes;
                let rendered = raster_svg::render_exact(t.svg.as_bytes(), raster.width, raster.height)?;
                let a = raster_svg::flatten(&raster, [255, 255, 255]);
                let b = raster_svg::flatten(&rendered, [255, 255, 255]);
                row.m = metrics::compare(&a, &b);
                if o.images {
                    let op = dir.join(format!("{base}.out.png"));
                    raster_svg::to_image(&b).save(&op)?;
                    row.out_png = Some(rel(root, &op));
                    let dp = dir.join(format!("{base}.diff.png"));
                    metrics::diff_image(&a, &b).save(&dp)?;
                    row.diff_png = Some(rel(root, &dp));
                }
                Ok(())
            })();
            if let Err(err) = result {
                row.error = Some(format!("{err:#}"));
                // Worst possible scores so failures are not rewarded.
                row.m = metrics::ImageMetrics { delta_e: 100.0, wrong_pct: 100.0, ssim: 0.0 };
            }
            let d = done.fetch_add(1, Ordering::Relaxed) + 1;
            if d % 50 == 0 || d == total {
                eprintln!("  {d}/{total}");
            }
            row
        })
        .collect();

    let mut rows = rows;
    rows.sort_by(|a, b| (&a.set, a.size, &a.name, &a.kind).cmp(&(&b.set, b.size, &b.name, &b.kind)));
    let buckets = aggregate(&rows);
    let results = Results {
        name: name.clone(),
        tracer: o.tracer.clone(),
        params: o.params.clone(),
        date: now_iso(),
        in_process: matches!(tracer, Tracer::Ours(_)),
        rows,
        buckets,
    };
    let json = root.join("bench/out").join(format!("{name}.json"));
    fs::write(&json, serde_json::to_string(&results)?)?;
    print_summary(&results);
    eprintln!("wrote {}", rel(root, &json));
    Ok(())
}

pub fn aggregate(rows: &[Row]) -> BTreeMap<String, Bucket> {
    let mut acc: BTreeMap<String, Vec<&Row>> = BTreeMap::new();
    for r in rows {
        for key in [
            format!("{}/{}", r.set, r.size),
            format!("{}/all", r.set),
            format!("all/{}", r.size),
            "all/all".to_string(),
        ] {
            acc.entry(key).or_default().push(r);
        }
    }
    acc.into_iter()
        .map(|(k, rs)| {
            let n = rs.len() as f64;
            let ok: Vec<&&Row> = rs.iter().filter(|r| r.error.is_none()).collect();
            let nok = ok.len().max(1) as f64;
            let b = Bucket {
                n: rs.len(),
                delta_e: rs.iter().map(|r| r.m.delta_e).sum::<f64>() / n,
                wrong_pct: rs.iter().map(|r| r.m.wrong_pct).sum::<f64>() / n,
                ssim: rs.iter().map(|r| r.m.ssim).sum::<f64>() / n,
                nodes: ok.iter().map(|r| r.nodes as f64).sum::<f64>() / nok,
                paths: ok.iter().map(|r| r.paths as f64).sum::<f64>() / nok,
                bytes: ok.iter().map(|r| r.bytes as f64).sum::<f64>() / nok,
                ms: ok.iter().map(|r| r.ms).sum::<f64>() / nok,
                failures: rs.len() - ok.len(),
            };
            (k, b)
        })
        .collect()
}

fn print_summary(r: &Results) {
    eprintln!("{:<22} {:>5} {:>7} {:>7} {:>6} {:>8} {:>7} {:>8} {:>4}", "bucket", "n", "ΔE", "wrong%", "ssim", "nodes", "paths", "ms", "fail");
    for (k, b) in &r.buckets {
        if k.starts_with("all/") || k.ends_with("/all") {
            eprintln!(
                "{:<22} {:>5} {:>7.2} {:>7.2} {:>6.3} {:>8.1} {:>7.1} {:>8.1} {:>4}",
                k, b.n, b.delta_e, b.wrong_pct, b.ssim, b.nodes, b.paths, b.ms, b.failures
            );
        }
    }
}

/// Load `bench/out/<name>.json`, or, when there is no such run, the entry
/// of that name in `bench/baselines.json` (buckets only, no rows), so the
/// regression gate works on a machine without the baseline tracers.
pub fn load(root: &Path, name: &str) -> Result<Results> {
    let p = if name.ends_with(".json") { root.join(name) } else { root.join("bench/out").join(format!("{name}.json")) };
    if p.is_file() {
        let s = fs::read_to_string(&p).with_context(|| p.display().to_string())?;
        return Ok(serde_json::from_str(&s)?);
    }
    let bp = root.join("bench/baselines.json");
    let s = fs::read_to_string(&bp).with_context(|| format!("{} and {}", p.display(), bp.display()))?;
    let all: BTreeMap<String, Baseline> = serde_json::from_str(&s)?;
    let b = all
        .get(name)
        .with_context(|| format!("no run {} and no baseline {name} in {}", p.display(), bp.display()))?;
    Ok(Results {
        name: name.to_string(),
        tracer: b.tracer.clone(),
        params: None,
        date: b.date.clone(),
        in_process: false,
        rows: Vec::new(),
        buckets: b.buckets.clone(),
    })
}

/// One entry of `bench/baselines.json`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Baseline {
    pub date: String,
    pub tracer: String,
    pub buckets: BTreeMap<String, Bucket>,
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // civil date from days (Howard Hinnant's algorithm), UTC
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let t = secs % 86400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", t / 3600, (t % 3600) / 60, t % 60)
}
