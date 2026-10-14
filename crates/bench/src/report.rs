//! `bench report`: one HTML page from every bench/out/*.json.
//! Sections: summary table per tracer × size, per-set table, and a contact
//! sheet of the worst N images per tracer (input / output / diff).

use crate::run::{self, Results};
use anyhow::Result;
use std::fs;
use std::path::Path;

pub fn report(root: &Path, worst: usize) -> Result<()> {
    let out = root.join("bench/out");
    let mut names: Vec<String> = fs::read_dir(&out)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|e| e == "json").unwrap_or(false))
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();
    names.sort();
    let results: Vec<Results> = names.iter().filter_map(|n| run::load(root, n).ok()).collect();
    if results.is_empty() {
        anyhow::bail!("no results in bench/out; run `bench run --tracer ...` first");
    }

    let mut sizes: Vec<u32> = results.iter().flat_map(|r| r.rows.iter().map(|x| x.size)).collect();
    sizes.sort_unstable();
    sizes.dedup();
    let mut sets: Vec<String> = results.iter().flat_map(|r| r.rows.iter().map(|x| x.set.clone())).collect();
    sets.sort();
    sets.dedup();

    let mut h = String::new();
    h.push_str(HEAD);
    h.push_str("<main><h1>vectorize bench</h1>");
    h.push_str(&format!("<p class=muted>{} tracer runs · {} sets · sizes {}</p>", results.len(), sets.len(), sizes.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(", ")));

    // Summary: all sets, per size.
    h.push_str("<h2>All sets, by size</h2>");
    h.push_str(&bucket_table(&results, &sizes.iter().map(|s| format!("all/{s}")).chain(["all/all".to_string()]).collect::<Vec<_>>()));

    // Per set, all sizes.
    h.push_str("<h2>By set, all sizes</h2>");
    h.push_str(&bucket_table(&results, &sets.iter().map(|s| format!("{s}/all")).collect::<Vec<_>>()));

    // Worst images per tracer.
    for r in &results {
        let mut rows: Vec<&run::Row> = r.rows.iter().collect();
        rows.sort_by(|a, b| b.m.wrong_pct.partial_cmp(&a.m.wrong_pct).unwrap().then(b.m.delta_e.partial_cmp(&a.m.delta_e).unwrap()));
        h.push_str(&format!("<h2>{} — worst {} by wrong-pixel % <span class=muted>({})</span></h2>", esc(&r.name), worst.min(rows.len()), esc(&r.date)));
        if let Some(p) = &r.params {
            h.push_str(&format!("<p class=muted>params: <code>{}</code></p>", esc(p)));
        }
        h.push_str("<div class=sheet>");
        for row in rows.iter().take(worst) {
            h.push_str("<figure>");
            h.push_str(&format!("<figcaption><b>{}</b> {}/{} {}{}</figcaption>", esc(&row.name), esc(&row.set), row.size, if row.kind == "jpg" { "jpeg " } else { "" }, row.error.as_ref().map(|e| format!("<span class=err>ERROR {}</span>", esc(e))).unwrap_or_default()));
            h.push_str("<div class=tri>");
            h.push_str(&format!("<img src=\"../../{}\" title=\"input\">", row.input));
            match &row.out_png {
                Some(p) => h.push_str(&format!("<a href=\"../../{}\"><img src=\"../../{}\" title=\"traced (click for SVG)\"></a>", row.svg, p)),
                None => h.push_str("<div class=blank>no output</div>"),
            }
            match &row.diff_png {
                Some(p) => h.push_str(&format!("<img src=\"../../{}\" title=\"ΔE heat\">", p)),
                None => h.push_str("<div class=blank></div>"),
            }
            h.push_str("</div>");
            h.push_str(&format!("<div class=nums>wrong {:.2}% · ΔE {:.2} · ssim {:.3} · {} nodes / {} paths · {:.0} ms</div>", row.m.wrong_pct, row.m.delta_e, row.m.ssim, row.nodes, row.paths, row.ms));
            h.push_str("</figure>");
        }
        h.push_str("</div>");
    }
    h.push_str("<p class=muted>Timing for subprocess tracers includes process start-up. wrong% = pixels with ΔE<sub>76</sub> &gt; 10 after flattening both images on white. Nodes counted as path commands (Z excluded), identical rules for every tracer.</p>");
    h.push_str("</main></body></html>");
    let path = out.join("report.html");
    fs::write(&path, h)?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

fn bucket_table(results: &[Results], keys: &[String]) -> String {
    let mut h = String::from("<table><thead><tr><th>bucket</th><th>tracer</th><th>n</th><th>wrong %</th><th>ΔE</th><th>SSIM</th><th>nodes</th><th>paths</th><th>bytes</th><th>ms</th><th>fail</th></tr></thead><tbody>");
    for k in keys {
        // best (lowest) wrong% and nodes in this bucket, for highlighting
        let best_wrong = results.iter().filter_map(|r| r.buckets.get(k)).map(|b| b.wrong_pct).fold(f64::INFINITY, f64::min);
        // Fewest nodes only counts among tracers that are also accurate (within 2× of the
        // best wrong-pixel rate); otherwise the one-rectangle stub would always "win".
        let best_nodes = results
            .iter()
            .filter_map(|r| r.buckets.get(k))
            .filter(|b| b.wrong_pct <= best_wrong * 2.0 + 0.5)
            .map(|b| b.nodes)
            .fold(f64::INFINITY, f64::min);
        let mut first = true;
        for r in results {
            let Some(b) = r.buckets.get(k) else { continue };
            h.push_str("<tr>");
            h.push_str(&format!("<td>{}</td>", if first { esc(k) } else { String::new() }));
            first = false;
            h.push_str(&format!("<td>{}</td><td>{}</td>", esc(&r.name), b.n));
            h.push_str(&format!("<td class=\"{}\">{:.2}</td>", if b.wrong_pct <= best_wrong { "best" } else { "" }, b.wrong_pct));
            h.push_str(&format!("<td>{:.2}</td><td>{:.3}</td>", b.delta_e, b.ssim));
            let node_best = b.nodes <= best_nodes && b.wrong_pct <= best_wrong * 2.0 + 0.5;
            h.push_str(&format!("<td class=\"{}\">{:.1}</td>", if node_best { "best" } else { "" }, b.nodes));
            h.push_str(&format!("<td>{:.1}</td><td>{:.0}</td><td>{:.1}</td><td>{}</td>", b.paths, b.bytes, b.ms, b.failures));
            h.push_str("</tr>");
        }
    }
    h.push_str("</tbody></table>");
    h
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const HEAD: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>vectorize bench</title>
<style>
:root{--bg:#fafaf9;--fg:#1c1917;--muted:#78716c;--card:#fff;--line:#e7e5e4;--accent:#2563eb;--best:#dcfce7;--err:#b91c1c}
@media(prefers-color-scheme:dark){:root{--bg:#0c0a09;--fg:#f5f5f4;--muted:#a8a29e;--card:#1c1917;--line:#292524;--accent:#60a5fa;--best:#14532d;--err:#fca5a5}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:14px/1.5 system-ui,sans-serif}
main{max-width:1400px;margin:0 auto;padding:24px 16px 64px}h1{font-size:1.5rem}h2{font-size:1.1rem;margin-top:32px}
.muted{color:var(--muted)}table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th,td{padding:4px 8px;text-align:right;border-bottom:1px solid var(--line)}th:first-child,td:first-child,th:nth-child(2),td:nth-child(2){text-align:left}
td.best{background:var(--best);font-weight:600}
.sheet{display:grid;grid-template-columns:repeat(auto-fill,minmax(330px,1fr));gap:12px}
figure{margin:0;background:var(--card);border:1px solid var(--line);border-radius:10px;padding:10px}
figcaption{font-size:.85rem;margin-bottom:6px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.tri{display:grid;grid-template-columns:1fr 1fr 1fr;gap:4px}.tri img,.blank{width:100%;aspect-ratio:1;object-fit:contain;background:
repeating-conic-gradient(#e5e5e5 0 25%,#fff 0 50%) 0 0/16px 16px;border-radius:4px;image-rendering:auto}
.blank{display:grid;place-items:center;color:var(--muted);font-size:.8rem}.nums{font-size:.8rem;color:var(--muted);margin-top:6px}
.err{color:var(--err);font-weight:600}code{font-size:.85em}
</style></head><body>"#;
