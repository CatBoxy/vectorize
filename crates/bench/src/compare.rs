//! Regression gate: B must not be worse than A by more than the thresholds in
//! any size bucket. Exit code 1 on regression so CI and git hooks can use it.

use crate::run;
use anyhow::Result;
use std::path::Path;

const MAX_WRONG_PCT_REL: f64 = 0.02; // 2% relative increase in wrong pixels
const MAX_DELTA_E_REL: f64 = 0.02;
const MAX_NODES_REL: f64 = 0.10;

pub fn compare(root: &Path, a: &str, b: &str) -> Result<()> {
    let ra = run::load(root, a)?;
    let rb = run::load(root, b)?;
    let mut regressions = 0;
    println!("{:<22} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}", "bucket", "ΔE A", "ΔE B", "wrong A", "wrong B", "nodes A", "nodes B");
    for (k, ba) in &ra.buckets {
        if !k.starts_with("all/") {
            continue;
        }
        let Some(bb) = rb.buckets.get(k) else { continue };
        let mut flag = String::new();
        if bb.delta_e > ba.delta_e * (1.0 + MAX_DELTA_E_REL) + 0.01 {
            flag.push_str(" ΔE↑");
        }
        if bb.wrong_pct > ba.wrong_pct * (1.0 + MAX_WRONG_PCT_REL) + 0.01 {
            flag.push_str(" wrong↑");
        }
        if bb.nodes > ba.nodes * (1.0 + MAX_NODES_REL) + 0.5 {
            flag.push_str(" nodes↑");
        }
        if bb.failures > ba.failures {
            flag.push_str(" failures↑");
        }
        if !flag.is_empty() {
            regressions += 1;
        }
        println!(
            "{:<22} {:>9.2} {:>9.2} {:>9.2} {:>9.2} {:>9.1} {:>9.1}{}",
            k, ba.delta_e, bb.delta_e, ba.wrong_pct, bb.wrong_pct, ba.nodes, bb.nodes, flag
        );
    }
    if regressions > 0 {
        eprintln!("REGRESSION: {regressions} bucket(s) got worse from {a} to {b}");
        std::process::exit(1);
    }
    println!("ok: {b} is not worse than {a}");
    Ok(())
}
