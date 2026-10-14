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

`--stats` prints node counts and per-stage milliseconds. `VECTORIZE_DEBUG=1` (or `=elbows`) prints what the
contour and polygon stages did for every loop. `curve_fit` is `kurbo` (default: fit the dense sub-pixel
contour between corners) or `potrace` (Selinger's smooth + opticurve on the polygon), kept for comparison.

## Windows notes

The workspace builds on the `x86_64-pc-windows-gnu` toolchain without Visual Studio
(`rustup-init -y --default-host x86_64-pc-windows-gnu --profile minimal`). That toolchain ships no
assembler, so crates using `windows-sys` raw-dylib imports fail to link; the workspace disables clap's
`color` feature for that reason. Potrace and VTracer are Linux-only baselines here.
