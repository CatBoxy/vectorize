# Vectorize: Build Plan

Goal: a raster-to-SVG tracer that beats Illustrator Image Trace on logos, icons, clip art,
text and flat illustration, runs entirely in the browser, and is good enough to release.
Research behind every choice here: `RESEARCH.md`. Interactive explainer: `.explain/`.

## 0. Decisions (locked unless a milestone proves them wrong)

| Decision | Choice | Why |
|---|---|---|
| Core language | Rust | Native speed, one codebase for CLI, WASM and a future API. kurbo and resvg are Rust. |
| Delivery | WebAssembly in a Web Worker inside a Next.js app on Vercel | Zero server cost, images never leave the browser, your existing stack. |
| Input class for v1 | Vector-born art: logos, icons, clip art, text, flat illustration | Where quality is visible and rewarded. Photos are posterization for every tool. |
| Output for v1 | Solid fills only, SVG | Gradients and strokes are stretch milestones. |
| Quality bar | Beat Potrace and VTracer on the benchmark by M3; match Illustrator on logos by M4 | Measurable, with a manual Vectorizer.AI ceiling for reference. |
| Method | Classical geometry. No ML in v1. | Every ML tracer is minutes per image; the products that win use ML only as a front end. |
| License | Core MIT or Apache-2.0, app proprietary if you want to charge | Keeps forking and contributions possible. |

Machine today: rustc 1.78 (update with `rustup update`, kurbo and wasm-bindgen want 1.80+),
node 21, no wasm-pack, no potrace, no vtracer, no resvg. M0 installs these.

## 1. Architecture

One Cargo workspace plus a web app:

```
vectorize/
  Cargo.toml                 workspace
  crates/
    core/                    pure pipeline, no I/O, no std::fs, compiles to wasm32
    cli/                     `vectorize in.png -o out.svg --preset logo --dump stages/`
    wasm/                    wasm-bindgen wrapper, one function: trace(bytes, params) -> svg + stats
    bench/                   corpus builder, metrics, baselines, HTML report
  web/                       Next.js app (App Router), worker, inspector UI
  corpus/                    rasterized test images + ground truth SVGs (git-lfs or generated)
  RESEARCH.md  PLAN.md
```

### Pipeline as typed stages

Each stage is a pure function from one data type to the next, and each has a `dump()` that
writes an SVG overlay. The inspector UI and the benchmark both consume these dumps, so every
stage is debuggable in isolation from day one.

```
Raster (RGBA, u8)
  │ 1. palette          Palette { colors: Vec<Lab>, bg: Option<usize> }
  │ 2. coverage         CoverageFields { per color: Vec<f32> plane }       "pixel p is 0.37 red, 0.63 white"
  │ 3. regions          RegionMap { label: Vec<u32>, color_of: Vec<usize>, area, bbox }
  │ 4. boundary graph   BoundaryGraph { nodes (junctions), edges (dense polylines, sub-pixel), face per side }
  │ 5. polygonize       PolyGraph { same graph, each edge now a sparse polyline + corner flags }
  │ 6. fit              CurveGraph { each edge a BezPath (lines, cubics, arcs later) }
  │ 7. assemble         Scene { shapes: Vec<Shape{ fill, path, z }> }  (stacked or abutting)
  └ 8. emit             String (SVG), plus Stats { paths, nodes, bytes, ms per stage }
```

Key invariant: **a boundary between two regions exists once** in the graph. Both faces
reference it. It is simplified once and fitted once with its junction endpoints pinned. That is
what makes the output gapless and halves node count versus per-region tracing.

### Parameters (user-facing, Illustrator vocabulary) and what they drive

| User control | Internal |
|---|---|
| Mode: Color / Grayscale / B&W | palette estimator |
| Colors (auto or N) | palette size, auto by merge-distance in OKLab |
| Detail (Illustrator "Paths") | fit tolerance in px, 0.1 to 2.0 |
| Corners | corner threshold on the alpha / angle test |
| Noise | min region area in px² |
| Method: Abutting / Overlapping | assembler |
| Snap curves to lines | straight-run detection tolerance |
| Ignore white | drop background-colour shapes |

## 2. Milestones

Times assume part-time evenings and weekends. Each milestone has an exit criterion measured
by the harness, not by eye alone.

### M0. Benchmark harness (week 1)

Nothing else can be judged without it. Build this before writing a line of tracing code.

- `cargo run -p bench -- build-corpus`: rasterize ~200 openly licensed SVGs (Simple Icons,
  OpenMoji, Twemoji, Font Awesome Free, a handful of real logos you have rights to) at 64, 128,
  256, 512 and 1024 px with anti-aliasing, via resvg. Also JPEG-encode a third at quality 75,
  and add 20 synthetic shapes with analytic ground truth (circle, rounded rect, star, text glyphs,
  thin 1 px lines, a 2-colour checker).
- `cargo run -p bench -- run --tracer X`: for every image, trace, re-rasterize the SVG with
  resvg, compute **L1 in CIELAB**, **SSIM**, **path count**, **node count**, **bytes**, **ms**.
  Write `bench/out/report.html`: a contact sheet of input / output / diff, sortable table,
  per-size averages.
- Baselines wired in from day one: Potrace (apt), VTracer (`cargo install vtracer`),
  open-vectorizer (git clone, cargo build). Store their numbers as a fixed baseline table.
- Ceiling: manually trace 12 images with Vectorizer.AI's free preview and Illustrator if you
  have access. Save SVGs. These are the reference for eyeballing, not for metrics.
- Regression gate: `bench -- compare A B` fails if L1 regresses more than 2% or node count
  more than 10% on any size bucket.

Exit: report renders, baselines have numbers, a dummy tracer that emits one rectangle scores
worst.

### MA. The Arena: human-in-the-loop review game (weekend during M0, then alongside every milestone)

The thresholds that decide corner vs curve, snap vs keep, merge vs split are taste. The harness
cannot measure taste; Juan can, but only if judging is cheap and habitual. The Arena is a small
web game that turns a stream of candidate traces into labelled preferences. Every milestone from
M1 on consumes its output.

**Card types (what each one teaches the tracer)**

- **Duel.** Two traces of the same image, blind, sides randomized. Pick left / right / same.
  Answers "did this build beat the previous one" and "did we beat VTracer / Illustrator" with
  pairwise preference, which is far more reliable than absolute ratings.
- **Spot the flaw.** One trace overlaid on the original. Tap where it looks wrong and pick a
  label: missed corner, false corner, wobbly curve, seam, lost detail, blob, wrong colour.
  Tap position is snapped to the nearest boundary node and stored. This is the labelled
  corner dataset for M4 (PolyFit trained on ~100 annotated images).
- **Turing round.** Original raster and re-rasterized trace at the same size. Which one is the
  vector? Confusion rate is the true end-state metric: at 50% the trace is indistinguishable.
- **Boss fight.** The ten worst images from the harness for the current build, once per build.
  A cleared boss means a failure class is fixed; it goes into the golden corpus.

**Rules that keep the data honest**

- Blind always. The UI never reveals which side is the new build or a baseline.
- ~10% of cards are repeats shown later in the session or the next day. Self-agreement is the
  noise floor; a preference smaller than it is not a result.
- Rewards only for participation, streaks and consistency, never for a particular answer.
- Sessions of 2-5 minutes, phone first, keyboard shortcuts on desktop (←/→/space).

**The pet**

- The pet is drawn by the tracer. A hand-drawn raster sprite sheet (idle, happy, hungry, sick,
  evolving) is traced by the current build at trace time. Its appearance *is* the quality:
  a wobbly blob at M1, clean corners at M4.
- Stats read straight from the harness: Accuracy (L1), Elegance (node count vs VTracer),
  Speed (ms @1024), Steadiness (Juan's self-agreement). Hunger = unreviewed cards in the queue.
  Mood = regression gate on the latest build. Evolution = a milestone exit criterion met.
- Pushing a build makes it hungry. Each review is a meal. Streaks keep it happy.
- Hard effort cap: one weekend for the MVP, one evening per milestone for an evolution.
  The pet exists to serve the review loop, not the other way round.

**Plumbing**

- `web/` Next.js app, route `/arena`, backed by Upstash Redis (already used in Amoeba). Single
  secret link for auth, no accounts. Deployed on Vercel next to the product.
- `cargo run -p bench -- arena push --build <id>`: packs a batch of cards (input PNG,
  candidate SVGs re-rasterized to PNG at display size, metadata) and uploads them.
- `cargo run -p bench -- arena pull`: downloads votes as JSON into `bench/arena/<date>.json`.
  Outputs: per-build win rates vs each baseline, Turing confusion rate, flaw heatmap per image,
  labelled corner list, self-agreement score.
- The M4 exit criterion ("blind A/B prefers ours ≥ 80% vs VTracer, ≥ 50% vs Illustrator") is
  measured here, not on an ad hoc page.

**Throughput.** ~5 s per card, 10 min per day ≈ 100 judgments/day, ~700/week. More preference
data per week than the papers used in total. The bottleneck becomes build iteration speed,
which is where it should be.

Exit for MVP: a build pushed from the bench shows up on the phone within a minute, 50 cards
reviewed in one session, `arena pull` prints win rates and self-agreement, the pet renders
from the current tracer and changes mood with the queue.

### M1. Black-and-white core, Potrace or better (weeks 2-3)

- Coverage field from luminance (or alpha for transparent PNGs).
- Boundary extraction by **area-matching contouring** (Manson, Smith, Schaefer 2010) on the
  coverage field, not marching squares with linear interpolation, not thresholding.
  Fallback for the first days: marching squares at 0.5, swap later and watch the metric move.
- Corner recovery: detect straight runs on both sides of a chamfered vertex, fit lines, intersect
  (Gong and Newman 2018).
- Region loops and holes with correct orientation; despeckle by area; `fill-rule: nonzero`.
- Polygonization: implement Potrace's optimal polygon (fewest segments within 0.5 px, then
  penalty) over the sub-pixel contour. It is O(n²) per contour, fine at these sizes.
- Corner test: Potrace alpha with alphamax = 1.0 as the first corner model.
- Fitting: `kurbo::fit_to_bezpath_opt` on each run between corners, G1 at smooth joins.
- SVG writer: relative commands, 2 decimals, `h`/`v`/`s` shorthands, one path per colour.

Exit: on the B&W corpus, L1 ≤ Potrace and nodes ≤ Potrace on every size bucket. Thin 1 px
lines survive. No speckles above the noise threshold are lost.

### M2. Colour and the boundary graph (weeks 4-7)

The hard engineering milestone.

- Palette: convert to OKLab. Mark edge pixels (local contrast above a threshold). Cluster only
  interior pixels (k-means++ seeded by a 5-bit histogram). Auto colour count: merge clusters
  closer than a perceptual distance, user override. Detect background colour (border majority).
- Coverage per colour: for each pixel, unmix against its two nearest palette colours along the
  line between them; interior pixels are 1.0 for their colour. Output one f32 plane per colour.
- Regions: label connected components per colour on the argmax map; merge speckles into the
  neighbour with the longest shared border (not deleted, merged, so no holes appear).
- **Boundary graph**: walk the label map once. Vertices where 3+ labels meet are junction nodes.
  Edges are dense sub-pixel polylines between junctions, positioned by the two colours' coverage
  isoline (0.5 between the two faces). Each edge stores left face and right face. Validate: every
  face's boundary is a closed cycle of edges; assert in debug builds.
- Simplify and fit per edge with endpoints pinned at junctions (Adobe 2025 recipe). Continuity
  across a junction: pair up edges that continue smoothly through it and share a tangent.
- Assembler, two modes: abutting (each face emits its cycle of shared curves) and stacked
  (sort by area descending, each shape extends under smaller ones; needs the union of the face
  with everything on top, computed on the label map before tracing).
- Transparency: alpha is coverage of a virtual background colour; emit without background.

Exit: beats VTracer on L1 and node count at every size; zero seams in abutting mode (check by
rendering on a magenta background and counting magenta pixels: must be 0).

### M3. Curve quality (weeks 8-9)

- Replace the first-pass corner model with a combined test: Potrace alpha, turning angle, and
  an accuracy test (would a smooth fit through this vertex exceed tolerance?). Expose as the
  Corners slider.
- Straight-run detection before fitting: runs whose dense points are within tolerance of a line
  become line segments (Snap Curves to Lines). Axis-aligned runs snap to exact axis.
- Circular arc detection: least-squares circle on a run; accept if max deviation < tolerance;
  emit as `A` commands. One arc replaces up to four cubics.
- Global node minimization: `fit_to_bezpath_opt` already does it per run; add a merge pass across
  smooth joins like Potrace's opticurve.
- Accept-by-rasterization: for any candidate (line, arc, cubic run), rasterize into a small local
  buffer and compare to the coverage field. Keep the simplest candidate under tolerance.

Exit: node count ≤ 70% of VTracer at equal or better L1. Circles in the synthetic set come
out as 1 to 4 segments.

### M4. The authored look (weeks 10-13)

This is what separates "traced" from "drawn". PolyFit's four cues, implemented incrementally,
each one checked on the contact sheet.

- Polygon cost rewritten as PolyFit's: accuracy + simplicity + continuity + regularity, shortest
  cycle on the candidate-edge graph. Keep Potrace's as a fallback behind a flag.
- Regularity: axis alignment for runs ≥ 2 px, parallel edges, equal-length edges, mirror symmetry
  snapping of corner positions within a shape, continuation of edges across junctions.
- Corner classifier: start rule-based with the features PolyFit lists (fit accuracy of each
  configuration, polygon angle, edge lengths, raster resolution). If rules plateau, train a tiny
  decision tree on the Spot-the-flaw corner labels from the Arena; export as Rust code.
- Primitive hypotheses: full circle, ellipse, rounded rectangle, regular polygon. Propose from
  the polygon, accept by rasterization.
- Auto presets: Logo (few colours, strong regularity), Sketch (B&W, soft corners), Flat
  illustration (more colours, less snapping), Pixel art (nearest-neighbour upscale then trace,
  or Kopf-Lischinski later).

Exit: the Arena (Juan plus three friends via the secret link) prefers your output over VTracer
≥ 80% and over Illustrator ≥ 50% on the logo set, above the self-agreement noise floor. Node count within 20% of Vectorizer.AI on the 12
reference images.

### M5. Product (weeks 10-14, in parallel with M4)

- `crates/wasm`: `trace(png_bytes, params_json) -> { svg, stats, stage_dumps? }`. Build with
  wasm-pack, target `web`, optimize with `wasm-opt -O3`. Budget ≤ 400 KB gzipped.
- Next.js app: drag-and-drop, paste, URL. Worker runs the trace; UI stays responsive. Debounced
  re-trace on slider change; target < 1 s at 1024 px, cap input at 2048 px long edge with a
  warning like every competitor.
- Views: side by side, overlay with opacity, wipe slider, node overlay, zoom to 800%.
- Inspector mode: toggle stage dumps (palette, regions, boundary graph, polygon, curves). This is
  your own debugging tool and a differentiator nobody else ships.
- Export: SVG (optimized), PNG at chosen scale, PDF via `svg2pdf` (Rust, compiles to WASM).
  Copy SVG to clipboard.
- No accounts, no uploads, no analytics beyond a page counter. Say so on the page.

Exit: a stranger can drop a logo and download a clean SVG in under 30 seconds without reading
anything.

### M6. Hardening and launch (weeks 15-16)

- Edge-case list, each with a corpus image and a test: 3+ way junctions, 1 px features, JPEG
  ringing, semi-transparent PNGs, hairline strokes, text at 12 px, images with 1 colour, images
  with 60 colours, 4000 px inputs, non-square pixels (no), indexed PNG, EXIF rotation.
- Fuzz the core with random noise images; it must never panic (WASM panics kill the worker).
- Performance pass: profile with `perf` on native, `console.time` per stage in WASM, parallelize
  per-edge fitting with rayon natively (WASM stays single-threaded in v1).
- Landing page with before/after examples, a short "how it works" linking to the explainer,
  and honest limits (photos, gradients).
- Launch: Show HN / r/graphic_design / designer friends. Collect failing images; they become
  the corpus for v1.1.

### M7. Stretch (after launch, in order of value)

1. Gradients: Mumford-Shah smoothing + discontinuity-aware multicut + linear/radial fits
   (Adobe 2025). Big win for illustration and AI-generated images.
2. Strokes / centrelines for line art (frame fields, Bessmeltsev and Solomon 2019).
3. Pixel art mode (Kopf and Lischinski 2011).
4. Optional server API in Rust (axum) if people ask to pay for batch use.
5. ML front end for photos and scans, only if the classical pipeline plateaus there.

## 3. Testing strategy

- **Unit**: synthetic shapes with analytic truth. A circle of radius r must come back within
  0.05 px RMS. A square must have exactly 4 nodes. A 3-colour image must have exactly 3 palette
  entries.
- **Property**: for any output, re-rasterize and assert max coverage error < tolerance × 2.
  Assert no NaN coordinates, every path closed, no zero-area shapes.
- **Golden**: the corpus with stored metrics; CI fails on regression beyond the gate.
- **Visual**: the contact-sheet report, reviewed by eye at every milestone. Keep a "worst 20"
  page sorted by L1; it tells you what to fix next.
- **Fuzz**: `cargo fuzz` on the core entry point.

## 4. Risks and what to do about them

| Risk | Mitigation |
|---|---|
| Boundary graph bugs (dangling edges, wrong faces) eat weeks | Debug-build assertions that every face is a closed cycle; the inspector overlay; start on 2-colour images and add colours one at a time |
| Perceptual tuning never converges | Fixed corpus, blind A/B page, stop when the A/B number stops moving; ship with presets |
| WASM too slow at 2048 px | Cap at 2048, downscale with a warning, profile per stage, move per-edge fitting to a work queue so the UI can show progress |
| kurbo fit API changes (marked experimental) | Pin the version; the fitter is one module behind a trait, Schneider as fallback |
| Rust 1.78 too old for dependencies | `rustup update` in M0 |
| Scope creep into gradients / photos | They are M7. The exit criteria above do not mention them |
| Nobody uses it | Launch at M6 with a narrow promise (logos and icons) and collect failures; the product is the corpus |

## 5. Definition of done for v1

- Beats VTracer and Potrace on the benchmark at every size, in L1 and node count.
- On the 12 reference logos, output is visually indistinguishable from Illustrator's at 100% and
  cleaner at 800% (fewer nodes, true corners, straight lines are straight, circles are round).
- No seams in abutting mode, no holes in stacked mode.
- 1024 px logo traced in under 1 s in Chrome on a laptop.
- Works offline after first load. No uploads.

## 6. Week 1 checklist

Status 2026-10-13: steps 1-7 done (Potrace built from source into ~/.local/bin instead of apt;
resvg used as a crate, not a CLI; open-vectorizer baseline pending). Step 8 (Arena MVP) pending.
Baselines live in bench/baselines.json; VTracer is fed white-flattened inputs because it reads
transparent pixels as black.

Status 2026-10-14: M1 exit criterion met on the bw corpus (835 images, defaults):

| size | ΔE ours | ΔE Potrace | nodes ours | nodes Potrace | ms ours (in-process) |
|---|---|---|---|---|---|
| 64 | 1.01 | 1.98 | 34.2 | 35.4 | 2.9 |
| 128 | 0.46 | 0.98 | 35.9 | 44.6 | 5.0 |
| 256 | 0.23 | 0.51 | 39.4 | 54.0 | 6.8 |
| 512 | 0.12 | 0.24 | 44.2 | 64.0 | 12.4 |
| 1024 | 0.06 | 0.12 | 50.2 | 75.7 | 33.5 |

`bench compare potrace ours` passes (it reads bench/baselines.json when no potrace run exists). The
worst bw images are all 64 px JPEGs with ringing specks and the Starbucks logo.

Status 2026-10-14, later: M2 exit criterion met. Full corpus (1500 images, defaults), ours vs VTracer:

| size | ΔE ours | ΔE VTracer | nodes ours | nodes VTracer | ms ours (in-process) |
|---|---|---|---|---|---|
| 64 | 1.50 | 4.24 | 66.7 | 95.9 | 4.0 |
| 128 | 0.68 | 2.33 | 67.0 | 129.9 | 8.7 |
| 256 | 0.36 | 1.34 | 92.3 | 174.7 | 16.4 |
| 512 | 0.19 | 0.81 | 138.9 | 246.7 | 44.6 |
| 1024 | 0.10 | 0.56 | 227.4 | 406.2 | 145.5 |

Colour set alone (535 images): abutting ΔE 0.95, 275 nodes, 3.8 paths; overlapping (stacked) ΔE 0.92,
224 nodes, 6.0 paths; VTracer ΔE 2.56, 390 nodes, 29 paths. The bw set improved to ΔE 0.36 / 34.0 nodes
from the shared contour fixes. Both `bench compare` gates pass. Zero seams: verified by rendering each
shape alone and checking that coverages sum to one everywhere (rendering abutting shapes together shows
hairlines from the renderer's own anti-aliasing, which is not a gap in the geometry).

What M2 ended up as, and where it departs from the recipe above:
- Palette: OKLab k-means seeded from a 5-bit histogram of interior pixels, merge below 0.04, then an
  iterative pass that adds the most populous cluster of opaque edge pixels not explainable as a
  colour or an sRGB mix of two colours (a 1 px line has no interior; adding black explains every grey
  at once). Background: border majority (transparent, a colour, or none).
- Coverage per colour is not stored as planes: each pixel is a mix of two labels with a fraction.
  Unmixing is done in sRGB, where anti-aliasing actually blends, against the pair of colours present
  in the pixel's 5×5 neighbourhood (the nearest palette colour overall is only a fallback partner;
  insisting on it gave every black-on-yellow edge a red endpoint and 2000-node outlines).
- Regions: 4-connected components with a per-corner diagonal-join policy (join the non-background,
  else the rarer, colour), speckles merged into the neighbour with the longest border and recoloured.
- Boundary graph: junctions are pixel corners of degree ≥ 3; chains between them and junction-free
  loops are the edges. Each is refined to sub-pixel points by the M1 rule with the left region's
  coverage, polygonised (open variant with pinned ends), fitted once, and stored with its left and
  right face. Junction positions: least-squares meeting point of the lines through each incident
  chain's first unbiased points, within 1 px of the corner. Faces are walked with the M1 pixel rule
  and reuse the fitted edges forwards or reversed, so adjacent shapes share geometry exactly.
- Abutting output: one path per colour. Overlapping (stacked) output: each region's outer boundary
  plus only the holes that border something unpainted, painted in order of containment depth, where
  a region's parent is the region in whose hole it lies (flood fill of the adjacency graph from the
  hole's boundary without crossing the owner, innermost holes first, nothing larger than the hole).
  Area order and shared-border-length heuristics both fail on outline rings and JPEG halos.
- The run-length rule for real corners and chamfers went from 2 to 3 steps: a smooth curve through
  45° makes runs of one and two, and the old rule manufactured corners off the curve.
- Continuity across junctions (pairing edges that continue smoothly through a junction) is not done;
  each chain's end tangent is one-sided. Deferred to M3 with the corner model.


What M1 ended up as, and where it departs from the recipe above:
- Boundary: Potrace path decomposition for topology, then one sub-pixel point per pixel step by
  one-dimensional area matching of the two coverages across the edge (exact for a straight edge at
  any angle), plus an elbow at every turn. Single-step chamfers at anti-aliased 90° corners are
  repaired to the intersection of the two edge lines. Full Manson 2010 contouring was not needed.
- Polygon: Potrace's optimal polygon on real coordinates with tolerance 0.2 px, without the
  `i-1..j+1` extension (it forbids a vertex on an anti-aliased corner), on measured points and real
  corners only, with cut-corner vertex pairs merged. Vertex adjustment box 1.5 px.
- Corners: Potrace alpha on the polygon, plus any point where the contour turns > 60° within ±1 px
  of arc. Known gap: 1 px notches at thin-line ends round the ends; Potrace alpha misses corners
  whose adjacent edges are short. Both are M3 (corner model) work.
- Fit: kurbo `fit_to_cubic` driven by a greedy exponential/binary search per segment, with the
  fitted cubic re-checked against the samples and sane-control-point guards (kurbo returns wild
  arms on nearly straight data). `fit_to_bezpath_opt` was 100× slower and could hang. Fit accuracy
  0.35 px: 0.3 gives ΔE 0.94 @64 but 36.0 nodes (Potrace 35.4); 0.4 gives 32.8 nodes at ΔE 1.08.
  Straight runs become lines. The ported Potrace smooth+opticurve stays behind
  `curve_fit: "potrace"` for comparison (more nodes above 128 px, worse ΔE everywhere).
- Output: one path per image, nonzero winding, consecutive corners joined by single lines.


What M3 ended up as, and where it departs from the recipe above:
- Corners: a polygon vertex is a corner by Potrace's alpha test, or when the polygon turns by at
  least `min_turn` there and no single line, arc or cubic follows the contour from the previous
  vertex to the next within the fit accuracy. The Corners slider drives both: alphamax 1.4 − 0.8c,
  min_turn (40 − 40c)°. The accuracy test does not depend on edge lengths, which is where alpha alone
  fails (thin-line ends, corners between short edges). Cut-corner vertex pairs are no longer merged
  again at fit time (the polygon stage already did), so the 1 px end of a thin line keeps both corners.
- Primitives: every run is tried as a line (chord deviation ≤ accuracy, `snap_lines`), then as a
  circular arc (Kåsa least-squares circle, centre moved onto the chord's perpendicular bisector,
  monotonic sweep, every sample within accuracy; emitted as an SVG `A`), then as a cubic. One arc
  sweeps at most 170°: an `A` is defined by its ends and radius, and near 180° a chord shortened by
  0.01 px of coordinate rounding sags a 100 px half circle by a pixel. A whole circle is three 120°
  arcs. Circles are M + 3 arcs at every size (ΔE 0.01–0.15); pie@1024 went from 112 nodes to 7.
  Axis-aligned snapping of line ends was not needed: measured contour points already sit on the axis.
- Continuity across junctions: each chain end borrows five ghost points from the chain that continues
  it through the junction (within 20° of straight), so end tangents are two-sided and the shared curve
  meets smoothly. The least-squares junction point is only used when it is consistent with the
  incident chains (0.25 px).
- Node minimisation: the greedy exponential/binary search per run and the multi-start closed fit from
  M1 stayed. Opticurve-style merging across smooth joins was not needed for the exit bar (52% of
  VTracer's nodes at ΔE 0.54 vs 1.86), and accept-by-rasterization was not implemented: every
  primitive is already checked against the sub-pixel samples, which is the same test at lower cost.
- Found on the way: the seam between two abutting shapes rasterized on an opaque canvas is a 1 px line
  of a tint containing the canvas (each shape anti-aliased separately shows white through). The M2
  palette pass promoted that tint to a colour of its own, the speckle merge then handed its pixels to
  either side, and the boundary became a staircase. Opaque images now explain such pixels as a mix of
  two neighbours plus the canvas (background colour or white) and recover the coverage from the
  painting order; transparent images show the same seam as partial alpha and were already handled.
- Known gap: aliased input (no anti-aliasing at all: pixel art, binarised scans) carries no sub-pixel
  information, the 0.2 px polygon tolerance then puts a vertex on every stair step, and the corner
  model and arc fitter faithfully trace the jaggies. Potrace's 0.5 px tolerance is the fix, gated on
  the absence of intermediate coverage values. Deferred to M6 hardening.
- Numbers (full corpus, defaults): bw ΔE 0.33 / 31.3 nodes (M2: 0.36 / 34.0; Potrace 0.77 / 54.7);
  colour abutting 0.93 / 258 (M2: 0.95 / 275; VTracer 2.56 / 390); synthetic 0.29 / 14.1. All sets:
  ΔE 0.54 vs VTracer 1.86, nodes 110.7 vs 210.7, both `compare` gates pass at every size.


What M4 ended up as, and where it departs from the recipe above:
- Polygon: the shortest cycle now minimises PolyFit's cost (`polygon.rs`, `PolygonCost::Polyfit`,
  the default): one unit of simplicity per edge, an accuracy term (RMS deviation of the edge's points
  relative to the tolerance, weight 0.5) and a continuity term (squared turning angle at every vertex,
  weight 0.25, the curvature-variation cue: for a fixed total turn the squared penalty is smallest when
  the turns are equal). States are edges rather than vertices so the turn between consecutive edges is
  known; candidate ends of an edge are the end of its straight run, the next point, and the last eight
  points of the run that extend the reach, which keeps the search linear on long straight edges. The
  turn at vertex 0 is not scored (vertex 0 is a real corner of the contour whenever there is one; a
  second pass per first edge doubled the cost for nothing measurable). Potrace's fewest-segments cost
  stays behind `"polygon": "potrace"`. Measured on the colour set the two costs are within a node of
  each other at equal ΔE, and the continuity weight makes no measurable difference between 0 and 0.25
  either: the fitter works from the samples, not from the polygon, so the polygon only decides where
  corners may be. The PolyFit search costs about twice Potrace's time on the bw set and 15% on colour
  (`VECTORIZE_POLY_W` overrides the weights for experiments).
- Sharp tips: a cut corner whose short edge has no measured flat between its vertices (fewer than two
  points, or a chord under 1.5 px) may have its apex up to 4 px past the edge, and the merged vertex
  may move that far during adjustment; and the biased zone around a corner now grows with sharpness
  (`fit::corner_clear`, 0.75 / tan(θ/2) + 0.5 px, 1.5–5 px), for run samples and for primitive tests
  alike. The star's tips were two corners 1.3 px apart at every size before this.
- Regularity (`regularity.rs`, slider `regularity` 0..1, default 0.5): anchors are the corner vertices
  plus the junctions (shared by every chain meeting there); lines are the runs already straight within
  accuracy. Rules in order: axis alignment of lines ≥ 2 px within 6°·s, continuation of a line through a
  junction, parallel lines of one face to their length-weighted mean direction, equal lengths within
  8%·s, and mirror symmetry about a vertical or horizontal axis when every anchor of a face has a partner.
  Every move is accepted only if no anchor strays more than 2 × accuracy × s from its measured position
  and every line still passes within accuracy of its own samples; an anchor where a curve ends may only
  slide along that curve's tangent, so a curve gets longer or shorter but never a kink (without that the
  rules cost two nodes per colour image). Departures from PolyFit: the rules move the fitter's run end
  points, not a spline, and the per-face shape is the boundary-graph face, so a snap on a shared edge
  moves both shapes at once and stays gapless. On Simple Icons at 256 px the rules change 81 of 113
  outputs for a total of +2 nodes; at 8× zoom the two versions are indistinguishable, which is the point
  (sub-pixel moves) and also why the Arena, not the bench, has to judge them.
- Corner classifier (`fit::classify_corners`): rule-based on PolyFit's features, no training (the Arena
  labels do not exist). In order: open-chain ends are corners; a turn below `min_turn` is smooth; a turn
  one line, arc or cubic follows from the previous vertex to the next within accuracy is smooth; a turn of
  `strong_turn` or more is a corner; a turn between two edges both shorter than 2 px is smooth (no pixel
  evidence); otherwise the two sides are fitted separately and their tangents at the vertex compared,
  within `g1_angle` they join smoothly. The Corners slider drives min_turn (40° → 0°), strong_turn
  (100° → 50°) and g1_angle (20° → 10°). Potrace alpha is gone from the default path (it misses the 60°
  corners of a hexagon and depends on edge-length ratios); `VECTORIZE_CLASSIFIER=m3` restores it. Against
  the M3 rule the new one saves 0.4 nodes per bw image and 0.8 per colour image at equal ΔE.
- Primitives (`primitives.rs`, `primitives: true`): regular polygon and star (from the classified
  corners: equal angular slots, one or two radii), full circle (three arcs, as before), ellipse (from the
  area moments, refined by Gauss-Newton on the first-order point distance; emitted as three elliptical
  `A` arcs, `El::EllipseTo`) and rounded rectangle (orientation from the principal axes or the longest
  side, extents from the straight parts, radius from a circle fit per corner zone; four lines and four
  arcs). Each is accepted only if every sample is within accuracy of its outline, and on a smooth loop
  only if it has no more nodes than the ordinary fit (a six-pixel blob is four cubics, not an eight-node
  rounded rectangle; without that rule primitives cost 1.8 nodes per colour image). The synthetic
  circle, ellipse, rounded rect, hexagon, square and star come out as M + 3, M + 3, 8, 6, 4 and 10 nodes
  at all five sizes. Not attempted: primitives for faces made of several chains (a circle split by a
  junction), rotated ellipses are fine but a rounded rectangle's rotation is only estimated, no
  rasterize-and-compare (the sample test is the same check at lower cost).
- Presets (`Params::preset`, JSON `"preset"` with per-field overrides laid over it): Logo (regularity 1),
  Sketch (black and white, tolerance 0.3, corners 0.3, min_area 8, no line snapping, accuracy 0.5, no
  regularity or primitives), Flat illustration (colour merge 0.025, min_area 2, regularity 0.25) and
  Pixel art (nearest-neighbour upscale ×4, every bend a corner, min_area 1, no primitives; the scene is
  scaled back so coordinates stay in source pixels and land on pixel edges). `color_merge` and `upscale`
  are new parameters; the palette merge distance was a constant.
- Found on the way: the bench hung once per full run on a different large colour JPEG each time, and the
  same image traced alone in half a second. Speckle merging broke ties by hash-map iteration order and
  the palette's seed order did too, so one run in a thousand produced a different region map and, in
  rare cases, a chain the fitter chokes on. Ties now go to the nearest colour (then the lowest region
  id) and equal histogram counts to key order. The nearest-colour rule alone took the colour set from
  257.9 to 237.9 nodes at better ΔE (0.93 → 0.92): JPEG speckles
  merge coherently instead of at random. Everything is deterministic now.
- Numbers (full corpus, defaults):

| size | ΔE ours | ΔE VTracer | nodes ours | nodes VTracer | M3 ΔE / nodes | ms ours |
|---|---|---|---|---|---|---|
| 64 | 1.42 | 4.24 | 60.7 | 95.9 | 1.42 / 62.5 | 7.7 |
| 128 | 0.65 | 2.33 | 63.4 | 129.9 | 0.65 / 65.8 | 16.2 |
| 256 | 0.34 | 1.34 | 81.6 | 174.7 | 0.34 / 87.5 | 29.4 |
| 512 | 0.18 | 0.81 | 118.8 | 246.7 | 0.18 / 129.3 | 71.9 |
| 1024 | 0.10 | 0.56 | 190.3 | 406.2 | 0.10 / 208.2 | 206.4 |

  bw ΔE 0.33 / 30.4 nodes (M3 0.33 / 31.3; Potrace 0.77 / 54.7); colour abutting 0.92 / 237.9
  (M3 0.93 / 257.9; VTracer 2.56 / 390); synthetic 0.28 / 13.3 (M3 0.29 / 14.2). All sets: ΔE 0.54 vs VTracer
  1.86, nodes 103.0 vs 210.7 (49%). Both `compare` gates pass at every size, and so does `compare M3base M4`.
  Average time per image 66 ms against 43 ms for M3 (the PolyFit search and the primitive tests).
- The exit criterion is not measured. The Arena (MA) does not exist yet, so the blind preference against
  VTracer (≥ 80%) and Illustrator (≥ 50%) and the node count against Vectorizer.AI on the 12 reference
  images are open; nothing above should be read as meeting them. The bench says what it can: fewer nodes
  than M3 at equal or better error at every size, and the synthetic primitives come out as primitives.
  Looked at by eye: the 20 worst images of the M4 report (by wrong-pixel %, contact sheet of input / trace / ΔE heat) are all 64 px, nineteen of them colour emoji (mostly the JPEG copies) plus the Starbucks logo. Every trace reads as the right picture, with the right colours, no seams and nothing missing; the error heat sits on the 1–2 px dark outlines that every emoji has at that size and on the dartboard's concentric 1 px rings (671 nodes, the hardest image in the corpus), where sub-pixel placement and JPEG ringing dominate. None of them is an M4 failure class; they are the resolution limit, and M6's aliased-input work is where they belong.


What M5 ended up as, and where it departs from the recipe above:
- `crates/wasm` (wasm-bindgen, `wasm-pack build --target web`): `trace(bytes, params_json)` for PNG and
  JPEG bytes and `trace_rgba(width, height, rgba, params_json)` for pixels the browser decoded, both
  returning `{ svg, stats, stage_dumps? }` or `{ error: { code, message } }` (`decode`, `too_large`,
  `params`), never a thrown exception; `stage_dumps` comes back when the params JSON has `"dumps": true`.
  A panic in the core cannot be caught on wasm32 (no unwinding): it traps the instance and the call throws
  a RuntimeError on the JavaScript side. The panic hook logs it and keeps the message for `last_panic()`;
  the worker reports it as a `panic` error and the page replaces the worker and re-sends the image. Stage
  timings work in the browser: the module installs `performance.now()` as the core's clock
  (`set_time_source`).
- Size: the tracer alone is 821 KB raw, 341 KB gzipped (wasm-pack 0.15, wasm-opt 117 `-O3`, fat LTO, one
  codegen unit, panic=abort; `-Oz` saves under 1%). With svg2pdf compiled in it is 1,421 KB / 594 KB, over
  the 400 KB budget, so the crate has two feature sets, `tracer` (default) and `pdf`, built as two packages:
  the PDF module (705 KB / 306 KB gzipped, usvg without text, images or filters) is fetched the first time
  someone exports a PDF. svg2pdf 0.13 builds for wasm32 unchanged.
- Speed, measured inside the worker in Chromium (the desktop app's browser pane) on the Windows 10
  machine: the 1024 px Simple Icons Coca-Cola logo traces in 380 ms the first time (JIT tier-up included)
  and 244 ms warm (palette 25, unmix 6, regions 62, graph 41, fit 95, assemble 6, emit 1 ms inside the
  core; the rest is PNG decoding and the crossing to JavaScript); a 512 px OpenMoji emoji in 148 ms first,
  80 ms warm. The native bench averages 206 ms at 1024 px, so wasm runs at roughly 1.2–1.8× native. The
  1 s target holds without any work on it.
- Web app (`web/`, Next.js 16 App Router on Turbopack, no UI library): the whole page is the drop zone;
  paste takes an image or a URL; the URL field fetches from the page, and a CORS refusal says so and
  suggests saving the file. Decoding is the browser's (`createImageBitmap`): a PNG within the cap goes to
  the module as its own bytes, a JPEG goes through a canvas so EXIF orientation applies to the traced
  pixels as to the shown ones, and WebP, GIF and BMP go the same way. Inputs over 2048 px on the long edge
  are downscaled with a warning. One worker loads the module once and keeps the image; slider changes
  re-trace after 150 ms of quiet, one trace in flight and only the latest queued. Controls: preset (the
  four M4 presets; their values are mirrored in the sliders and the preset name is still sent, so colour
  merge and upscale come from the core), Mode, Colours (auto or N), Detail (0–100 mapped to the polygon
  tolerance 2 → 0.1 px on a log scale, fit accuracy 1.75× that), Corners, Noise (px²), Method, Snap curves
  to lines, Ignore white; Regularity, primitives and the B&W threshold under "advanced". Views: side by
  side, overlay with opacity, wipe (slider and line), node overlay (the SVG path data parsed; a node is a
  corner when its incoming and outgoing tangents differ by more than 12°, arcs included; cubic handles
  optional), wheel zoom about the cursor up to 800%, drag to pan, fit and 100%.
- Inspector: the core had no dump hooks, so the cheap ones were added (`trace_with`, `StageDumps`):
  palette swatches with the background marked, the region map (one hashed colour per region, shipped as
  RGBA pixels and drawn on a pixelated canvas; for the pixel-art preset it is the 4× grid), the polygon
  vertices of every boundary chain (`graph::Edge` now keeps them), and the per-stage milliseconds. Not
  dumped: the coverage/mix field and the boundary graph's sub-pixel points (large, and only readable with
  a dedicated viewer), and the fitted curves, which are the SVG itself; the node overlay is their view.
- Export: SVG file and copy to clipboard (the writer's output as is, no extra optimiser pass), PNG at
  0.5–8× through a canvas, PDF through the second module. The footer says no accounts, no uploads, no
  analytics, and none were added, not even the page counter the recipe allowed.
- Not done: the "stranger in under 30 seconds" exit criterion is unmeasured (nobody but the author has
  used it); working offline after the first load (a service worker, from the v1 definition of done); the
  Arena (MA); a deployment. Deploying to Vercel needs the project's root directory set to `web`, Node 20+,
  and the wasm packages at build time: either committed under `web/public/wasm` (1.5 MB of binaries per
  rebuild) or built in Vercel's install step (install rustup, the wasm32 target and wasm-pack, then
  `npm run build:wasm -- --build`). `/wasm/*` already gets immutable cache headers from `next.config.ts`,
  `next build` passes locally with the type check, and there are no environment variables or API routes.
- Found on the way: `cargo install wasm-pack` fails on the GNU toolchain (it compiles windows-sys); the
  prebuilt MSVC binary works and fetches wasm-bindgen and binaryen itself. wasm-pack 0.15 ignores
  `[package.metadata.wasm-pack.profile.<custom>]` but still runs wasm-opt for a `--profile` build.
  Turbopack handles `new Worker(new URL("./x.ts", import.meta.url))` and a `turbopackIgnore` dynamic import
  of the glue from `public/` without configuration. The desktop app's browser pane is 800 px wide, which is
  how a toolbar overflow (the page scrolled sideways when a button out of view took focus) was caught.


1. `rustup update`; `cargo install wasm-pack vtracer resvg`; `sudo apt install potrace`.
2. `cargo new --lib crates/core`, workspace Cargo.toml, `cargo add kurbo` in core.
3. Corpus: script that downloads Simple Icons and OpenMoji SVGs, rasterizes with resvg at the
   five sizes, writes `corpus/{set}/{size}/{name}.png` plus the source SVG.
4. Metrics: L1 in CIELAB and SSIM over re-rasterized output; node and path counting from the
   SVG; a timing wrapper.
5. Baseline runs for Potrace and VTracer; commit `bench/baselines.json`.
6. `report.html` contact sheet. Look at VTracer's worst 20. Those failures are your spec.
7. Stub `core::trace()` that returns a bounding rectangle, run it through the harness end to end.
8. Weekend: Arena MVP. `web/` Next.js + Upstash, Duel and Spot-the-flaw cards, `arena push`
   from the bench with Potrace vs VTracer duels on the corpus, `arena pull`. Draw the pet
   sprite sheet (egg + blob stages) and trace it with the stub.
