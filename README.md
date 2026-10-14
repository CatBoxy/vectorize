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
VTracer (`cargo install vtracer`).
