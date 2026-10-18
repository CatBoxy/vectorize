# vectorize

Raster → SVG tracer aimed at Illustrator Image Trace / Vectorizer.AI quality on
vector-born art (logos, icons, clip art, flat illustration). Rust core, WASM in
the browser, benchmark-driven. See `PLAN.md` for the roadmap and `RESEARCH.md`
for the literature.

## Layout

| Path | What |
|---|---|
| `crates/core` | Pure tracing pipeline (`vectorize_core::trace`) |
| `crates/cli` | `vectorize in.png -o out.svg` |
| `crates/bench` | Benchmark harness: corpus, metrics, baselines, report |
| `crates/wasm` | WASM wrapper (M5) |
| `web/` | Next.js app: product UI + the Arena review game (M5 / MA) |
| `corpus/` | Benchmark images (sources fetched by `corpus/fetch.sh`, not committed) |

## Bench quickstart

```sh
./corpus/fetch.sh                                  # Simple Icons, Font Awesome, OpenMoji, Twemoji
cargo run --release -p vectorize-bench -- build-corpus
cargo run --release -p vectorize-bench -- run --tracer potrace
cargo run --release -p vectorize-bench -- run --tracer vtracer
cargo run --release -p vectorize-bench -- run --tracer ours
cargo run --release -p vectorize-bench -- report   # bench/out/report.html
cargo run --release -p vectorize-bench -- compare vtracer ours
```

Baselines: Potrace (`corpus/fetch.sh` does not install it; build 1.16 from source or `apt install potrace`),
VTracer (`cargo install vtracer`). Their numbers are stored in `bench/baselines.json`, so `ours` can be
judged without them installed.

## CLI

```sh
cargo run --release -p vectorize-cli -- in.png -o out.svg --stats
cargo run --release -p vectorize-cli -- in.png --params '{"tolerance":0.2,"fit_accuracy":0.2,"curve_fit":"kurbo"}'
```

`--stats` prints node counts and per-stage milliseconds. `mode` is `color` (default), `grayscale` or
`black_and_white`; `method` is `abutting` (default: adjacent shapes share one fitted boundary, one path per
colour) or `overlapping` (stacked: each shape painted over what encloses it, fewer nodes); `colors` caps
the palette; `ignore_background` drops the border-majority colour. `curve_fit` is `kurbo` (default: fit
the dense sub-pixel contour between corners) or `potrace` (Selinger's smooth + opticurve on the polygon),
kept for comparison.

`corners` (0..1) is the Corners slider: 0 only keeps sharp corners, 1 keeps every turn (M4 corner model:
a rule-based classifier on the polygon angle, the edge lengths and whether one curve, or two curves
joining smoothly, can follow the boundary within `fit_accuracy`). `snap_lines` turns straight runs into
`L` commands; circular runs become `A` arcs regardless (a circle is three arcs).

M4 controls: `preset` is `logo`, `sketch`, `flat_illustration` or `pixel_art`; the preset's values apply
first and any other field in the JSON overrides them. `polygon` is `polyfit` (default: simplicity +
accuracy + continuity cost) or `potrace` (fewest segments). `regularity` (0..1, default 0.5) snaps nearly
axis-aligned, parallel, equal-length and mirror-symmetric edges and continues lines through junctions,
never beyond `fit_accuracy`. `primitives` (default true) emits whole circles, ellipses, rounded rectangles,
regular polygons and stars as such. `color_merge` is the OKLab distance below which palette clusters
merge. `upscale` traces a nearest-neighbour enlargement and scales the result back (the pixel-art preset
uses 4).

Debugging: `VECTORIZE_DEBUG=1` prints a summary per stage; `=elbows`, `=chains`, `=edges`, `=stack`,
`=junctions`, `=corners`, `=closed`, `=trace`, `=prims` and `=merge` print the contour points, boundary
chains, per-edge fit counts, stacking order, junction refinement, corner decisions, closed-loop fits,
fitter calls, primitive hypotheses and cut-corner merges; `VECTORIZE_PROBE=x,y` prints the colour mix of
the pixels around a point. For experiments, `VECTORIZE_REG_SKIP=axis,junction,parallel,equal,mirror`
turns regularity rules off, `VECTORIZE_CLASSIFIER=m3` restores the M3 corner test and
`VECTORIZE_POLY_W=acc,cont` overrides the polygon cost weights.

## Windows notes

The workspace builds on the `x86_64-pc-windows-gnu` toolchain without Visual Studio
(`rustup-init -y --default-host x86_64-pc-windows-gnu --profile minimal`). That toolchain ships no
assembler, so crates using `windows-sys` raw-dylib imports fail to link; the workspace disables clap's
`color` feature for that reason. Potrace and VTracer are Linux-only baselines here.
