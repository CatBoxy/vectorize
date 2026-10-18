//! Black-and-white tracer (PLAN.md M1): darkness coverage → binary paths →
//! sub-pixel contours → optimal polygon → corner classification → curve
//! fitting → one filled path per image in the mean foreground colour.

use crate::params::CurveFit;
use crate::scene::Path;
use crate::{contour, coverage, curve, decompose, fit, polygon, primitives, regularity, Fill, Params, Raster, Scene, Shape, StageClock};

/// Half-width of the box a polygon vertex may move in during adjustment, px.
/// Potrace uses 0.5 around a pixel corner; here a vertex on an anti-aliased
/// corner is the nearest measured point, up to ~1.5 px from the true corner
/// once cut-corner pairs are merged.
pub const VERTEX_BOX: f64 = 1.5;
/// Potrace `opttolerance`: how far a merged curve may stray, px.
pub const OPT_TOLERANCE: f64 = 0.2;
/// Half-width of the tangent estimation window, px of arc length.
pub const TANGENT_WINDOW: f64 = 1.0;

pub fn trace_bw(img: &Raster, params: &Params, clock: &mut StageClock) -> Scene {
    let mut scene = Scene::new(img.width, img.height);
    if img.width == 0 || img.height == 0 {
        return scene;
    }
    let cov = coverage::darkness(img, params.threshold);
    let bm = coverage::binarize(&cov);
    clock.lap("coverage");
    let paths = decompose::decompose(&bm, params.min_area.max(1));
    clock.lap("decompose");
    let amax = alphamax(params.corners);
    let fit_opts = fit::FitOpts {
        accuracy: params.fit_accuracy.clamp(0.02, 4.0),
        snap_lines: params.snap_lines,
        window: TANGENT_WINDOW,
    };
    let copts = corner_opts(params);
    let popts = polygon_opts(params);
    let contours: Vec<contour::Contour> = paths.iter().map(|p| contour::subpixel(p, &cov)).collect();
    clock.lap("contour");
    // Polygonise the measured points and the real corners only; stair
    // elbows are off the boundary and would only break straight runs.
    let polygons: Vec<(Vec<kurbo::Point>, Vec<usize>)> = contours
        .iter()
        .map(|c| {
            let idx: Vec<usize> = (0..c.pts.len()).filter(|&k| c.fit_ok[k]).collect();
            let pts: Vec<kurbo::Point> = idx.iter().map(|&k| c.pts[k]).collect();
            let dirs: Vec<u8> = idx.iter().map(|&k| c.dirs[k]).collect();
            let (verts, po) = polygon::polygonize_with(&pts, &dirs, &popts);
            (verts, po.into_iter().map(|j| idx[j]).collect())
        })
        .collect();
    clock.lap("polygon");
    let dbg = std::env::var("VECTORIZE_DEBUG").is_ok();
    let mut bez = Path::new();
    let accuracy = params.fit_accuracy.clamp(0.02, 4.0);
    let prim_opts = primitives::PrimOpts::new(accuracy);
    let ropts = regularity::RegOpts::from_strength(params.regularity, accuracy);
    let mut reg = regularity::Reg::default();
    let mut pending: Vec<(Vec<usize>, Vec<fit::Run>)> = Vec::new();
    for (c, (verts, po)) in contours.iter().zip(&polygons) {
        if dbg {
            let corners = fit::classify_corners(&c.pts, &c.fit_ok, verts, po, true, &copts).iter().filter(|&&b| b).count();
            eprintln!(
                "loop: contour {} pts ({} elbows, {} fit_ok), polygon {} verts, {} corners, po {:?}",
                c.pts.len(),
                c.is_corner.iter().filter(|&&b| b).count(),
                c.fit_ok.iter().filter(|&&b| b).count(),
                verts.len(),
                corners,
                po
            );
            if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("elbows") {
                for k in 0..c.pts.len() {
                    if c.is_corner[k] {
                        eprintln!("  elbow {k}: ({:.2}, {:.2}) fit_ok={}", c.pts[k].x, c.pts[k].y, c.fit_ok[k]);
                    }
                }
            }
        }
        if verts.len() < 3 {
            continue;
        }
        match params.curve_fit {
            CurveFit::Potrace => {
                let segs = curve::fit(verts, amax, OPT_TOLERANCE);
                curve::append_subpath(&mut bez, &segs);
            }
            CurveFit::Kurbo => {
                let corner = fit::classify_corners(&c.pts, &c.fit_ok, verts, po, true, &copts);
                let n = c.pts.len();
                let corners: Vec<usize> = (0..verts.len()).filter(|&i| corner[i]).collect();
                // Every usable point, from the first polygon vertex round.
                let first = po[0] % n;
                let samples: Vec<kurbo::Point> = (0..n).map(|k| (first + k) % n).filter(|&k| c.fit_ok[k]).map(|k| c.pts[k]).collect();
                if samples.len() < 3 {
                    continue;
                }
                // M4: a whole primitive, when every sample agrees. A smooth
                // loop is also fitted the ordinary way and the primitive
                // must not cost nodes against that fit (a six-pixel blob is
                // four cubics, not an eight-node rounded rectangle).
                let prim = if params.primitives {
                    let cpts: Vec<kurbo::Point> = corners.iter().map(|&i| verts[i]).collect();
                    primitives::propose(&samples, &cpts, &prim_opts)
                } else {
                    None
                };
                if corners.is_empty() {
                    let (start, els) = fit::fit_run(&samples, true, &fit_opts);
                    let (start, els) = match prim {
                        Some((p, ps, pels)) if pels.len() <= els.len() => {
                            if dbg {
                                eprintln!("  primitive: {p:?} ({} vs {} fitted)", pels.len(), els.len());
                            }
                            (ps, pels)
                        }
                        _ => (start, els),
                    };
                    fit::push_loop(&mut bez, start, els);
                    continue;
                }
                if let Some((p, start, els)) = prim {
                    if dbg {
                        eprintln!("  primitive: {p:?}");
                    }
                    fit::push_loop(&mut bez, start, els);
                    continue;
                }
                // Anchors and runs, regularised together once every loop is planned.
                let ids: Vec<usize> = corners.iter().map(|&i| reg.add_anchor(verts[i], false)).collect();
                let clear: Vec<f64> = corners.iter().map(|&i| fit::corner_clear(fit::turn_at(verts[(i + verts.len() - 1) % verts.len()], verts[i], verts[(i + 1) % verts.len()]))).collect();
                let mut runs = Vec::with_capacity(corners.len());
                for (ci, &a) in corners.iter().enumerate() {
                    let cj = (ci + 1) % corners.len();
                    let b = corners[cj];
                    let (va, vb) = (verts[a], verts[b]);
                    let smp = fit::run_samples(&c.pts, &c.fit_ok, po[a] % n, po[b] % n, true, va, vb, clear[ci], clear[cj]);
                    let run = fit::plan_run(smp, va, vb, &fit_opts);
                    let (ia, ib) = (ids[ci], ids[(ci + 1) % ids.len()]);
                    if run.line {
                        reg.add_line(ia, ib, run.samples.clone());
                    } else if let Some((ta, tb)) = fit::run_end_tangents(&run, va, vb) {
                        reg.add_curve_end(ia, ta);
                        reg.add_curve_end(ib, tb);
                    }
                    runs.push(run);
                }
                reg.shapes.push(ids.clone());
                pending.push((ids, runs));
            }
        }
    }
    reg.regularize(&ropts);
    for (ids, runs) in pending {
        let m = ids.len();
        let start = reg.pos[ids[0]];
        let mut els = Vec::new();
        for (i, run) in runs.iter().enumerate() {
            let (va, vb) = (reg.pos[ids[i]], reg.pos[ids[(i + 1) % m]]);
            els.extend(fit::emit_run(run, va, vb, &[], &[], &fit_opts));
        }
        fit::push_loop(&mut bez, start, els);
    }
    clock.lap("fit");
    if bez.elements().is_empty() {
        return scene;
    }
    scene.shapes.push(Shape { path: bez, fill: Fill::Solid(foreground_color(img, &bm)) });
    scene
}

/// Polygon stage options from the user parameters.
pub fn polygon_opts(params: &Params) -> polygon::PolygonOpts {
    polygon::PolygonOpts { cost: params.polygon, ..polygon::PolygonOpts::new(params.tolerance.clamp(0.05, 4.0), VERTEX_BOX) }
}

/// Illustrator-style `corners` slider (0..=1) → Potrace `alphamax`.
/// 0.5 is Potrace's default 1.0; 0 never makes corners; 1 makes almost
/// every bend one.
pub fn alphamax(corners: f64) -> f64 {
    1.4 - 0.8 * corners.clamp(0.0, 1.0)
}

/// Corner decision options from the user parameters (Corners slider `c` in
/// 0..=1): the smallest turn that may be a corner, 40° at 0 down to 0° at 1
/// (20° at the default 0.5); the turn that always is one, 100° down to 50°
/// (75°); and how far the two sides' tangents may disagree and still join
/// smoothly, 20° down to 10° (15°). Alpha is kept for the Potrace fitter.
pub fn corner_opts(params: &Params) -> fit::CornerOpts {
    let c = params.corners.clamp(0.0, 1.0);
    fit::CornerOpts {
        alphamax: alphamax(c),
        min_turn: (40.0 - 40.0 * c).to_radians(),
        strong_turn: (100.0 - 50.0 * c).to_radians(),
        g1_angle: (20.0 - 10.0 * c).to_radians(),
        short_edge: fit::SHORT_EDGE,
        accuracy: params.fit_accuracy.clamp(0.02, 4.0),
        window: TANGENT_WINDOW,
    }
}

/// Mean colour of the interior foreground pixels (all four neighbours also
/// foreground, so anti-aliased edge pixels do not lighten it), composited on
/// white. Falls back to every foreground pixel for shapes with no interior.
fn foreground_color(img: &Raster, bm: &crate::bitmap::Bitmap) -> [u8; 4] {
    let (w, h) = (bm.width as i32, bm.height as i32);
    let mean = |interior_only: bool| -> Option<[u8; 4]> {
        let (mut sum, mut n) = ([0u64; 3], 0u64);
        for y in 0..h {
            for x in 0..w {
                if !bm.get(x, y) {
                    continue;
                }
                if interior_only
                    && !(bm.get(x - 1, y) && bm.get(x + 1, y) && bm.get(x, y - 1) && bm.get(x, y + 1))
                {
                    continue;
                }
                let i = (y * w + x) as usize * 4;
                let p = &img.data[i..i + 4];
                let a = p[3] as u64;
                for ch in 0..3 {
                    sum[ch] += (p[ch] as u64 * a + 255 * (255 - a)) / 255;
                }
                n += 1;
            }
        }
        (n > 0).then(|| [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8, 255])
    };
    mean(true).or_else(|| mean(false)).unwrap_or([0, 0, 0, 255])
}
