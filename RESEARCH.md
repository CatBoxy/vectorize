# Image Vectorization: Research Notes and Plan

Date: 2026-10-13. Goal: build a personal raster-to-SVG tracer good enough to release publicly.

## 1. What "tracing" actually is

A bitmap has many plausible vector interpretations. A tracer picks the one a human would
have drawn: smooth where the artist meant smooth, sharp where they meant a corner, with as few
shapes and nodes as possible. Potrace's paper says it directly: tracing cannot be local, because a
staircase of widely spaced steps must still become one straight line.

Every serious tracer, from Potrace (2003) to Adobe's 2025 research, runs the same five stages:

1. **Preprocess**: denoise, sharpen, estimate the palette.
2. **Segment**: assign each pixel to a region (one color, or one gradient).
3. **Extract boundaries**: walk region borders into closed polylines, ideally with sub-pixel positions.
4. **Fit curves**: simplify to polygons, decide corners vs smooth joins, fit cubic Béziers.
5. **Assemble and emit**: layer shapes (stacked or abutting), write SVG.

Quality is decided almost entirely in stages 3 and 4, plus how stage 2 handles anti-aliased edge
pixels.

## 2. How the known systems work

### Potrace (Selinger 2003), the reference for black and white
Source: potrace.sourceforge.net/potrace.pdf

- **Path decomposition**: pixel corners are graph vertices; walk edges keeping black on the left.
  Each closed loop is removed by inverting its interior, then recurse. A turn policy (default
  `minority`) decides ambiguous 2x2 checkerboard corners. Loops with area < `turdsize` are dropped
  (despeckle).
- **Optimal polygon**: a subpath is "straight" if some segment passes within 1/2 pixel of every
  vertex and the subpath does not use all four directions. Among polygons built from straight
  subpaths, minimize segment count first, then a penalty = segment length x stddev of point
  distances. Solved as a shortest cycle on a graph. This is the step that is non-local and O(n^2).
- **Vertex adjustment**: move each polygon vertex to the least-squares intersection of its two
  neighbouring fitted lines, constrained to the unit square around the original.
- **Corners vs curves**: at each vertex compute alpha = 4/3 x (how far a tangent line can be pushed
  toward the vertex). alpha > `alphamax` (default 1.0) is a corner, else a Bézier with
  alpha clamped to [0.55, 1]. 0.55 is the quarter-circle constant.
- **Curve optimization**: merge consecutive same-convexity Béziers into one that preserves enclosed
  area; accept if tangency error < `opttolerance` (0.2). Shortest path over acceptable merges.
- Weakness (per PolyFit paper): the optimal polygon smooths away small details that the spline
  fit cannot recover, and it has no notion of regularity (axis alignment, symmetry).

### VTracer (visioncortex, Rust, MIT), the best open-source color tracer
Source: github.com/visioncortex/vtracer

- Pipeline: color clustering (hierarchical, in a perceptual space) -> path walking -> polygon
  simplification -> corner detection by signed angle -> spline fitting by iterative subdivision.
  Entirely O(n), deliberately skipping Potrace's optimal polygon.
- Two layering modes: `stacked` (default; shapes painted over each other, so no holes and fewer
  nodes than Illustrator) and `cutout`/mosaic (abutting shapes with shared boundaries; v1.0 added
  gapless shared-boundary tracing).
- Parameters: `filter_speckle`, `color_precision` (bits per channel), `gradient_step` (layer
  difference), `corner_threshold` (default 60 deg), `segment_length` (default 4), `splice_threshold`
  (45 deg), `path_precision`, plus v1.0 extras: `--simplify` tolerance, palette snapping in OKLab,
  `--max-colors`, Bradley-Roth adaptive threshold, watershed segmentation.
- Ships as crate, CLI, Python wheel and npm/WASM. Good baseline and reference implementation,
  but its corners and curve quality are visibly below Vector Magic and Illustrator on logos.

### Adobe Illustrator Image Trace
Adobe publishes no algorithm. From the panel options and a community post by an Adobe engineer,
the pipeline is: palette estimation -> pixel assignment -> noise removal by region area -> boundary
smoothing with Bézier fitting. Exposed controls, which define the UX vocabulary users expect:

| Control | Meaning |
|---|---|
| Mode | Color / Grayscale / Black and White |
| Palette | Automatic, Limited (N colors), Full Tone, Document Library |
| Colors / Grays / Threshold | palette size or binarization cutoff |
| Paths | fit tightness (low = tighter to pixels) |
| Corners | likelihood a bend becomes a corner point |
| Noise | minimum region area in px |
| Method | Abutting (shared edges) vs Overlapping (stacked) |
| Create Fills / Strokes, Max Stroke Width | centerline tracing of thin lines |
| Snap Curves to Lines | nearly straight curves become straight |
| Ignore White | drop white fills |

Known weakness: many anchor points on noisy input; the 2025 Adobe paper shows Image Trace
needing 8584 paths where their new method needs 1417.

### Vector Magic (Diebel, Stanford 2008) and Vectorizer.AI (same team)
- Vector Magic came from Diebel's thesis "Bayesian Image Vectorization: The Probabilistic
  Inversion of Vector Image Rasterization" (ProQuest AAI3332816, not freely online). The framing
  matters: treat the bitmap as the output of a rasterizer applied to an unknown vector image plus
  noise, and search for the most probable vector image. Anti-aliasing becomes evidence, not noise.
- Vectorizer.AI (vectorizer.ai) is the current quality benchmark according to every 2026 review.
  Its stated feature list is the best public spec of what "really good" means:
  - Deep learning networks plus classical geometry ("Deep Vector Engine")
  - A proprietary planar graph structure ("Vector Graph") enabling local edits and optimizations
  - **Full shape fitting**: circles, ellipses, rounded rectangles, stars, with rotation
  - **Curve support**: lines, circular and elliptical arcs, quadratic and cubic Béziers, each where appropriate
  - **Clean corners** and per-corner sharp/round control
  - **Symmetry modelling**: mirror and rotational symmetry detection
  - **Adaptive simplification** of faint boundaries
  - **Sub-pixel precision**: boundaries placed from anti-aliasing values; captures sub-pixel features
  - Automatic palette size detection with override; 32-bit RGBA with partial transparency
  - GPU for the networks, multi-core CPU for the geometry
  - Pricing: free preview, about $10/month web, API from $9.99 for 50 credits

### Figma Vectorize (Feb 2026), Recraft, Kittl
One-click, AI-credit gated, controls limited to palette size and path simplification. No
algorithm details published. Confirms the market: people pay $10/month for good tracing.

## 3. Academic state of the art (what beats the products)

### Sub-pixel boundaries from anti-aliasing
- Manson, Smith, Schaefer, "Contouring Discrete Indicator Functions" (Eurographics 2010).
  Marching squares with linear interpolation produces aliasing and oscillation on coverage data.
  Their fix: in each pair of adjacent cells, solve for the line whose enclosed area matches the two
  coverage values exactly (four configurations, closed form, no iteration). Directly applicable:
  treat a per-color coverage field as the indicator function.
- Gong and Newman, "Isocontouring with sharp corner features" (MGV 2018): marching squares
  chamfers true corners; detect likely-corner cells and reconstruct the vertex by intersecting
  fitted lines from both sides. open-vectorizer does exactly this.

### Perception-aligned curve fitting (the UBC/Adobe line of work)
- Hoshyari et al., "Perception-Driven Semi-Structured Boundary Vectorization" (SIGGRAPH 2018):
  joint spline fitting and corner detection using a learned discontinuity metric plus global
  simplicity/continuity analysis.
- Dominici et al., **PolyFit** (SIGGRAPH 2020): the key paper for logo/clip-art quality. Four
  perceptual cues: **accuracy, simplicity, continuity, regularity**. Method: build a graph of
  candidate polygon edges that stay within an accuracy threshold of the raster boundary, find the
  shortest cycle under a cost combining those cues (with curvature-variation weight 0.25), then a
  small classifier (99.3% test accuracy, trained on ~100 annotated images) decides at each polygon
  corner between {corner, one curve, line+curve, curve+curve}, then a global G1/G2 spline
  optimization with regularity enforcement (axis-aligned edges >= 2 px, parallel edges, symmetric
  corners, continuations across junctions). Users preferred it 3:1 over Potrace and Hoshyari,
  16:1 on regions under 16x16 px. About 1 second for a 64x64 input. Potrace's polygon step
  smooths out detail; PolyFit's does not.
- Kopf and Lischinski, "Depixelizing Pixel Art" (SIGGRAPH 2011): similarity graph with
  heuristics for resolving crossing diagonals, then spline fitting. Still the right approach for
  tiny pixel-art inputs and the segmentation PolyFit builds on.

### Gradients (Adobe's current production-grade approach)
- Chakraborty et al., **"Image Vectorization via Gradient Reconstruction"** (Eurographics /
  CGF 2025, open access, PDF at techmatt.github.io). Fully automatic, fixed parameters:
  1. Mumford-Shah smoothing (Strekalovskiy and Cremers 2014 discrete form, alpha=1.0,
     lambda=1.5) yields a smoothed image plus a discontinuity map D.
  2. Greedy color-difference merge in CIELAB (threshold tau_s = 10) on smooth pixels, then a
     graph multicut that forbids merging segments separated by D.
  3. Per segment fit the cheapest of: constant color (boundary-distance-weighted mean, to cancel
     anti-aliasing), linear gradient (direction from the structure tensor of the color gradient,
     any number of stops), or radial gradient (center, eccentricity, affine transform).
  4. Edge pixels are partitioned and merged into neighbouring segments.
  5. Curve fitting on a **planar straight-line graph**: collect valence-2 chains into edge
     paths between junction nodes, pair up paths across junctions for continuity, simplify each
     path by dynamic programming with junction positions fixed, then reassemble. This is gapless
     by construction.
  Runtime 0.5 s for 512x512, ~2 s for 1024, ~9 s for 2048 (CPU). Beats ARDECO, LIVE (1800 s) and
  Optimize-and-Reduce (260 s) on L1/SSIM with comparable path counts.

### Deep learning end to end
- DiffVG (2020), LIVE (2022), SuperSVG (2024), StarVector, OmniSVG, and Adobe's **VectorArk**
  (CVPR 2026, "rounded polygon representation" plus a degradation model for real-world inputs,
  code "coming soon"). Survey "Image Vectorization: a Review" (Dziuba et al. 2023) concludes:
  online classical tools give the best quality, LIVE took 22 hours on one image, and no fast
  universal automatic method exists. The 2025 Adobe paper agrees: ML methods top out at a few
  hundred regions and are too slow.
- Conclusion for us: ML is useful as a **front end** (segmentation masks, denoising, palette,
  super-resolution of edges) the way Vectorizer.AI uses it, not as the geometry engine.
  Geometry stays classical.

### Line art / centerlines
- Bessmeltsev and Solomon, "Vectorization of Line Drawings via PolyVector Fields" (TOG 2019)
  fixes junction connectivity that skeletonization (Autotrace centerline, Zhang-Suen thinning)
  gets wrong. Needed only if we support "Strokes" mode. Defer.

### Open-source peers to measure against
- **open-vectorizer** (vardirhq, Rust/WASM, MIT, 2025): palette from interior pixels only,
  per-color coverage fields, 0.5 isoline contours, corner recovery by line intersection,
  primitive hypotheses (circle, ellipse, rect) and a tolerance ladder of curve fits, each
  candidate rasterized and scored against the source. ~150 KB gzipped WASM, Web Worker, 2048 px
  cap. Beats Potrace/VTracer on accuracy for synthetic shapes; VTracer still 2-4x faster and
  fewer nodes on organic shapes. Closest in spirit to what we want; validates the approach.
- **kurbo** (Rust, Raph Levien): `fit_to_bezpath(source, accuracy)` fits cubics by matching
  signed area and first moment (quartic solve), error measured as approximate Fréchet distance,
  recursive subdivision; `fit_to_bezpath_opt` searches for fewer segments. Far better than
  Schneider's 1990 Graphics Gems least-squares fitter, which is what most hobby tracers use.

## 4. What separates "good" from "really good"

Ranked by visual impact per unit of effort:

1. **Use anti-aliasing as data.** Per-color coverage fields and area-matching contouring give
   boundaries to ~0.01 px. Every cheap tracer thresholds and throws this away; Vector Magic,
   Vectorizer.AI and open-vectorizer keep it. This alone removes most "wobble".
2. **Gapless shared boundaries.** Trace the planar boundary graph once and fit each edge path once
   with junctions pinned, then build shapes from the shared curves (Adobe 2025, VTracer mosaic).
   Fixes hairline gaps and double-fitted seams, and halves node count versus per-region tracing.
3. **Perceptual corner and primitive decisions.** PolyFit's accuracy/simplicity/continuity/
   regularity cost, line and arc primitives, axis-alignment and symmetry snapping, "snap curves
   to lines". This is what makes a traced logo look authored instead of traced.
4. **Minimal-node curve fitting with a real error metric.** Area/moment matching (kurbo) with
   Fréchet-style error and global segment-count minimization, then rasterize-and-compare to accept.
5. **Smart palette.** Estimate colors from interior pixels only, in OKLab/CIELAB, with automatic
   color count. Edge-mixed pixels are unmixed into coverage, never given their own palette entry.
6. **Gradients** (Adobe 2025 recipe). Big win for illustrations and AI-generated images, large
   effort. Phase it late.

## 5. Plan

### Stack recommendation
- **Core in Rust**, compiled to **WebAssembly**, running in a Web Worker in the browser. The whole
  pipeline is CPU geometry; Rust gives native speed, one codebase for CLI + web + future API, and
  client-side execution means zero server cost, zero privacy concerns, and free hosting on Vercel.
  Crates: `image` (decoding, dev only; browser decodes in prod), `kurbo` (Bézier math and fitting),
  `palette`/own OKLab code, `resvg` + `usvg` (re-rasterize for the benchmark), `wasm-bindgen`.
- **Front end in Next.js on Vercel** (your existing stack): drag and drop, side-by-side and
  overlay compare, parameter panel mirroring Illustrator's vocabulary, SVG/PDF/PNG export.
- Python is fine for throwaway experiments but not for the product: too slow in the browser and
  would force a paid server.

### Phase 0: benchmark harness first (week 1)
Nothing else can be judged without it.
- Corpus: (a) synthetic shapes with known geometry, (b) ~200 openly licensed SVG logos/icons
  (Simple Icons, OpenMoji, Twemoji, Font Awesome) rasterized at 64 to 1024 px with anti-aliasing,
  some re-encoded as JPEG q=75, (c) a handful of scanned sketches and flat illustrations.
- Metrics per output: L1 in CIELAB and SSIM after re-rasterizing with resvg, path count, node
  count, file size, runtime. Also a contact-sheet HTML page for eyeballing.
- Baselines wired in from day one: Potrace, VTracer, open-vectorizer. Manually collected
  Vectorizer.AI and Illustrator outputs for a dozen images as the quality ceiling.

### Phase 1: black-and-white core at Potrace-or-better quality (weeks 2-3)
- Coverage field from grayscale -> area-matching contour (Manson 2010) -> corner recovery (line
  intersection) -> polygon simplification with explicit accuracy threshold -> corner/smooth
  classification (start with Potrace's alpha test, swap for PolyFit's cost later) -> cubic fitting
  via kurbo with G1 joins -> merge pass -> SVG.
- Exit criterion: on the logo corpus, equal or better L1 than Potrace with fewer nodes.

### Phase 2: color (weeks 4-6)
- Palette estimation from interior pixels in OKLab, auto color count with user override.
- Soft assignment: each pixel gets coverage per palette color (two-color unmixing at edges).
- Connected components, speckle removal by area, region adjacency graph.
- Planar boundary graph: trace shared edges once, junction nodes, edge paths, fit each once with
  pinned endpoints, assemble closed shapes. Offer both Abutting and Overlapping output.
- Transparency: treat alpha as coverage of a "background" color.
- Exit criterion: beats VTracer on L1 and node count across the corpus; no visible seams.

### Phase 3: the "authored look" (weeks 7-10)
- PolyFit-style cost (accuracy, simplicity, continuity, regularity) for polygon and corner choice.
- Primitives: straight lines, circular/elliptical arcs, full circles and rounded rects, accepted by
  rasterize-and-compare.
- Regularity: axis-alignment, parallelism, mirror symmetry snapping, continuation across junctions.
- "Snap curves to lines", "Ignore white", noise slider, corners slider, paths slider.

### Phase 4: product (parallel with phase 3)
- Next.js app, WASM in a worker, live re-trace on slider change (sub-second at 1024 px target),
  before/after overlay, node-count readout, SVG/PDF/PNG export, presets (Logo, Sketch, Photo,
  Pixel art). Free, client-side. Optional later: Rust server API for batch/paid use.

### Phase 5: stretch
- Gradients via the Adobe 2025 recipe (Mumford-Shah + discontinuity-aware multicut + linear/
  radial fits).
- Centerline strokes for line art.
- Optional ML front end (segmentation / denoising) if the classical pipeline hits a ceiling on
  photos and scans.

### Honest effort estimate
Potrace parity in Rust is a one-to-two week evening project if you follow the paper. The
color pipeline with gapless shared boundaries is the real engineering (another three to four
weeks). Phase 3 is open-ended and is where the product either becomes worth $10/month or does not.

## 6. Sources
- Potrace paper: https://potrace.sourceforge.net/potrace.pdf
- VTracer: https://github.com/visioncortex/vtracer
- open-vectorizer: https://github.com/vardirhq/open-vectorizer
- kurbo fitting: https://docs.rs/kurbo/latest/kurbo/fn.fit_to_bezpath.html and
  https://raphlinus.github.io/curves/2022/09/09/parallel-beziers.html
- Vectorizer.AI feature list: https://vectorizer.ai/
- Vector Magic history: https://vectormagic.com/about
- Adobe Image Trace docs: https://helpx.adobe.com/illustrator/using/image-trace.html
- Chakraborty et al. 2025, Image Vectorization via Gradient Reconstruction:
  https://techmatt.github.io/pdfs/imageVectorizationViaGradientReconstruction.pdf
- Dominici et al. 2020, PolyFit: https://www.cs.ubc.ca/labs/imager/tr/2020/ClipArtVectorization/
- Hoshyari et al. 2018: https://www.cs.ubc.ca/labs/imager/tr/2018/PerceptionDrivenVectorization/
- Manson, Smith, Schaefer 2010, Contouring Discrete Indicator Functions:
  https://people.engr.tamu.edu/schaefer/research/indicator_contour.pdf
- Gong and Newman 2018, Isocontouring with sharp corner features: https://mgv.sggw.edu.pl/article/view/2300
- Kopf and Lischinski 2011, Depixelizing Pixel Art: https://johanneskopf.de/publications/pixelart
- Bessmeltsev and Solomon 2019, PolyVector Fields: https://arxiv.org/abs/1801.01922
- Dziuba et al. 2023, Image Vectorization: a Review: https://arxiv.org/abs/2306.06441
- VectorArk (CVPR 2026): https://arxiv.org/abs/2605.24398
- Clair Obscur / COVec (2025): https://arxiv.org/abs/2511.20034
- Recraft 2026 tool comparison: https://www.recraft.ai/blog/best-image-vectorizers
- Figma Vectorize launch: https://forum.figma.com/product-updates-3/vectorize-images-in-design-and-draw-50595
