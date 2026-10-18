//! Colour tracer (PLAN.md M2): palette → per-pixel mix → regions →
//! boundary graph → one fitted curve per boundary → one path per colour.

use crate::bw::{corner_opts, polygon_opts, TANGENT_WINDOW};
use crate::dumps::{Polygon, RegionMap, StageDumps};
use crate::fit::FitOpts;
use crate::graph::Graph;
use crate::palette::{self, PaletteOpts};
use crate::params::{Method, Mode};
use crate::{mix, regions, Fill, Params, Raster, Scene, Shape, StageClock};
use crate::scene::Path;
use std::collections::BTreeMap;

/// Clusters closer than this in OKLab are one colour (the default of
/// `Params::color_merge`).
pub const MERGE_DIST: f32 = 0.04;
/// A pixel with a 4-neighbour further than this in OKLab is an edge pixel.
pub const EDGE_CONTRAST: f32 = 0.03;

pub fn trace_color(img: &Raster, params: &Params, clock: &mut StageClock, mut dumps: Option<&mut StageDumps>) -> Scene {
    let mut scene = Scene::new(img.width, img.height);
    if img.width == 0 || img.height == 0 {
        return scene;
    }
    let mut an = palette::analyse(img, EDGE_CONTRAST);
    if params.mode == Mode::Grayscale {
        for (c, rgb) in an.lab.iter_mut().zip(an.rgb.iter_mut()) {
            c.a = 0.0;
            c.b = 0.0;
            *rgb = palette::oklab_to_srgb(*c);
        }
    }
    let pal = palette::estimate(
        img,
        &an,
        &PaletteOpts { max_colors: params.colors, merge_dist: params.color_merge.clamp(0.005, 0.3), edge_contrast: EDGE_CONTRAST },
    );
    clock.lap("palette");
    let mut mm = mix::unmix(&an, &pal);
    clock.lap("unmix");
    if let Ok(probe) = std::env::var("VECTORIZE_PROBE") {
        if let Some((x, y)) = probe.split_once(',').and_then(|(a, b)| Some((a.parse::<i32>().ok()?, b.parse::<i32>().ok()?))) {
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let (px, py) = (x + dx, y + dy);
                    if px < 0 || py < 0 || px as usize >= an.width || py as usize >= an.height {
                        continue;
                    }
                    let i = py as usize * an.width + px as usize;
                    let m = mm.mix[i];
                    eprintln!(
                        "probe ({px},{py}): rgb {:?} a {} interior {} mix a={} b={} t={:.3}",
                        an.rgb[i], an.alpha[i], an.interior[i], m.a, m.b, m.t
                    );
                }
            }
        }
    }
    let reg = regions::label(&mut mm, pal.background, params.min_area.max(1), |a, b| {
        pal.lab.get(a as usize).zip(pal.lab.get(b as usize)).map_or(0.0, |(x, y)| x.dist(*y))
    });
    clock.lap("regions");
    if let Some(d) = dumps.as_deref_mut() {
        d.palette = pal.rgb.clone();
        d.background = pal.background;
        d.regions = Some(RegionMap { width: reg.width as u32, height: reg.height as u32, id: reg.id.clone(), color: reg.color.clone() });
    }
    let mut g = Graph::build(&reg);
    clock.lap("graph");
    let o = FitOpts {
        accuracy: params.fit_accuracy.clamp(0.02, 4.0),
        snap_lines: params.snap_lines,
        window: TANGENT_WINDOW,
    };
    let accuracy = params.fit_accuracy.clamp(0.02, 4.0);
    let ropts = crate::regularity::RegOpts::from_strength(params.regularity, accuracy);
    let prim = crate::primitives::PrimOpts::new(accuracy);
    g.fit(&reg, &mm, &polygon_opts(params), &corner_opts(params), &o, &ropts, params.primitives.then_some(&prim));
    clock.lap("fit");
    if let Some(d) = dumps.as_deref_mut() {
        d.polygons = g.edges.iter().map(|e| Polygon::from_points(&e.poly, e.closed)).collect();
    }
    let faces = g.faces(&reg);
    let skip_bg = if params.ignore_background { pal.background } else { None };
    let mut shapes: Vec<(u16, Path)> = Vec::new();
    match params.method {
        Method::Abutting => {
            // Every region with all its cycles, one path per colour.
            let mut by_color: BTreeMap<u16, Path> = BTreeMap::new();
            for (r, cycles) in faces.iter().enumerate() {
                let color = reg.color[r];
                if color == 0 || Some(color) == skip_bg {
                    continue;
                }
                let path = by_color.entry(color).or_default();
                for cyc in cycles {
                    g.cycle_path(cyc, path);
                }
            }
            shapes.extend(by_color);
        }
        Method::Overlapping => {
            // Each region painted largest first, so what sits inside it is
            // drawn over it instead of being cut out: its outer boundary,
            // plus only the holes that border a region nobody paints (the
            // transparent canvas or the dropped background). Every other
            // hole's boundary is emitted once, by the inner shape.
            let painted = |r: usize| reg.color[r] != 0 && Some(reg.color[r]) != skip_bg;
            let nr = faces.len();
            // Cycles of every region, with the outer one (largest area) and,
            // across that outer boundary, the enclosing region: the parent.
            // An outline ring has fewer pixels than the face inside it, so
            // the order must follow containment depth, not area.
            let mut cycles: Vec<Vec<(f64, bool, Path)>> = vec![Vec::new(); nr];
            let mut outer: Vec<Option<usize>> = vec![None; nr];
            for r in 0..nr {
                let mut best: Option<(f64, usize)> = None;
                for (ci, cyc) in faces[r].iter().enumerate() {
                    let mut p = Path::new();
                    g.cycle_path(cyc, &mut p);
                    let a = p.area().abs();
                    let borders_unpainted = cyc.iter().any(|&(id, _)| {
                        let e = &g.edges[id as usize];
                        let other = if e.left == r as u32 { e.right } else { e.left };
                        other == regions::OUTSIDE || !painted(other as usize)
                    });
                    cycles[r].push((a, borders_unpainted, p));
                    if best.map_or(true, |(ba, _)| a > ba) {
                        best = Some((a, ci));
                    }
                }
                outer[r] = best.map(|(_, ci)| ci);
            }
            // Parent of a region: the region in whose hole it lies. The
            // shared-boundary length cannot tell "enclosed by" from "next
            // to" (a JPEG halo around an outline touches it as much as the
            // background does), and a region can lie in a hole without
            // touching its boundary. Everything inside a hole of B is what
            // the adjacency graph reaches from the hole's boundary without
            // crossing B; the innermost enclosures are assigned first so
            // every region gets its immediate parent.
            let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nr];
            for e in &g.edges {
                if e.left != regions::OUTSIDE && e.right != regions::OUTSIDE && e.left != e.right {
                    adj[e.left as usize].push(e.right as usize);
                    adj[e.right as usize].push(e.left as usize);
                }
            }
            let mut by_size: Vec<usize> = (0..nr).filter(|&r| outer[r].is_some()).collect();
            by_size.sort_by(|&a, &b| {
                cycles[a][outer[a].unwrap()].0.partial_cmp(&cycles[b][outer[b].unwrap()].0).unwrap()
            });
            let mut parent: Vec<Option<usize>> = vec![None; nr];
            let mut seen = vec![usize::MAX; nr];
            let outer_area = |r: usize| outer[r].map_or(f64::MAX, |ci| cycles[r][ci].0);
            for b in by_size {
                for (ci, cyc) in faces[b].iter().enumerate() {
                    if Some(ci) == outer[b] {
                        continue;
                    }
                    // Nothing larger than the hole can be inside it. Without
                    // this, a one-pixel "hole" pinched off by a diagonal join
                    // would flood the whole image through the region that
                    // passes through the pinch.
                    let hole_area = cycles[b][ci].0 + 0.5;
                    let mut stack: Vec<usize> = cyc
                        .iter()
                        .map(|&(id, _)| {
                            let e = &g.edges[id as usize];
                            if e.left == b as u32 { e.right } else { e.left }
                        })
                        .filter(|&a| a != regions::OUTSIDE && a as usize != b)
                        .map(|a| a as usize)
                        .collect();
                    while let Some(v) = stack.pop() {
                        if v == b || seen[v] == b || outer_area(v) > hole_area {
                            continue;
                        }
                        seen[v] = b;
                        if parent[v].is_none() {
                            parent[v] = Some(b);
                        }
                        stack.extend(adj[v].iter().copied());
                    }
                }
            }
            let mut depth = vec![0usize; nr];
            for r in 0..nr {
                let mut d = 0;
                let mut p = parent[r];
                let mut guard = 0;
                while let Some(q) = p {
                    d += 1;
                    guard += 1;
                    if guard > nr {
                        break;
                    }
                    p = parent[q];
                }
                depth[r] = d;
            }
            let mut order: Vec<usize> = (0..nr).filter(|&r| painted(r) && outer[r].is_some()).collect();
            order.sort_by(|&a, &b| depth[a].cmp(&depth[b]).then(reg.area[b].cmp(&reg.area[a])));
            if std::env::var("VECTORIZE_DEBUG").as_deref() == Ok("stack") {
                for r in 0..nr {
                    let desc: Vec<String> = faces[r]
                        .iter()
                        .enumerate()
                        .map(|(ci, cyc)| {
                            let mut others: Vec<u32> = cyc
                                .iter()
                                .map(|&(id, _)| {
                                    let e = &g.edges[id as usize];
                                    if e.left == r as u32 { e.right } else { e.left }
                                })
                                .collect();
                            others.sort();
                            others.dedup();
                            format!("c{ci}{} area {:.0} borders {others:?}", if Some(ci) == outer[r] { "*" } else { "" }, cycles[r][ci].0)
                        })
                        .collect();
                    eprintln!("stack: region {r} colour {} area {}: {}", reg.color[r], reg.area[r], desc.join(" | "));
                }
                for &r in &order {
                    eprintln!(
                        "stack: region {r} colour {} area {} depth {} parent {:?} cycles {} outer area {:.0}",
                        reg.color[r],
                        reg.area[r],
                        depth[r],
                        parent[r],
                        cycles[r].len(),
                        outer[r].map(|i| cycles[r][i].0).unwrap_or(0.0)
                    );
                }
            }
            for r in order {
                let mut path = Path::new();
                for (i, (_, keep, p)) in cycles[r].iter().enumerate() {
                    if Some(i) == outer[r] || *keep {
                        path.extend(p.clone());
                    }
                }
                // Merge with the previous shape when it has the same colour:
                // same depth, no overlap, one path.
                match shapes.last_mut() {
                    Some((c, last)) if *c == reg.color[r] => last.extend(path),
                    _ => shapes.push((reg.color[r], path)),
                }
            }
        }
    }
    clock.lap("assemble");
    if std::env::var("VECTORIZE_DEBUG").is_ok() {
        eprintln!(
            "colour: palette {} {:?} (bg {:?}), regions {}, edges {} ({} loops), shapes {}",
            pal.len() - 1,
            pal.rgb[1..].iter().map(|c| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])).collect::<Vec<_>>(),
            pal.background,
            reg.area.len(),
            g.edges.len(),
            g.edges.iter().filter(|e| e.closed).count(),
            shapes.len()
        );
    }
    for (color, path) in shapes {
        if path.elements().is_empty() {
            continue;
        }
        let rgb = pal.rgb[color as usize];
        scene.shapes.push(Shape { path, fill: Fill::Solid([rgb[0], rgb[1], rgb[2], 255]) });
    }
    scene
}
