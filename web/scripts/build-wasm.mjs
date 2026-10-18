// Copy the wasm packages built by wasm-pack into public/wasm, where the
// worker fetches them at runtime. Run from web/:
//
//   npm run build:wasm            copy crates/wasm/pkg (tracer) and pkg-pdf (PDF exporter)
//   npm run build:wasm -- --build run wasm-pack for both first (needs wasm-pack and the wasm32 target)
//
// The two packages come from one crate and two feature sets, so the
// tracer stays under its size budget and the PDF module is fetched only
// when someone exports a PDF (README.md has the numbers).

import { cpSync, existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import { execSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { readFileSync } from "node:fs";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const crate = join(repo, "crates", "wasm");
const out = join(here, "..", "public", "wasm");

const build = process.argv.includes("--build");
if (build) {
  const common = "wasm-pack build crates/wasm --target web --profile wasm-release";
  for (const [dir, extra] of [
    ["pkg", "-- --no-default-features --features tracer"],
    ["pkg-pdf", "-- --no-default-features --features pdf"],
  ]) {
    console.log(`> ${common} --out-dir ${dir} ${extra}`);
    execSync(`${common} --out-dir ${dir} ${extra}`, { cwd: repo, stdio: "inherit" });
  }
}

const copies = [
  ["pkg", ""],
  ["pkg-pdf", "pdf"],
];
for (const [src, dst] of copies) {
  const from = join(crate, src);
  const to = join(out, dst);
  if (!existsSync(join(from, "vectorize_wasm_bg.wasm"))) {
    if (src === "pkg") {
      console.error(`missing ${from}: build it with\n  ${"wasm-pack build crates/wasm --target web --profile wasm-release"}`);
      process.exit(1);
    }
    console.warn(`no ${src}: PDF export will be unavailable in the app`);
    continue;
  }
  rmSync(to, { recursive: true, force: true });
  mkdirSync(to, { recursive: true });
  for (const f of readdirSync(from)) {
    if (/\.(wasm|js|ts)$/.test(f)) cpSync(join(from, f), join(to, f));
  }
  const wasm = join(to, "vectorize_wasm_bg.wasm");
  const raw = statSync(wasm).size;
  const gz = gzipSync(readFileSync(wasm), { level: 9 }).length;
  console.log(`${src} -> public/wasm/${dst || "."}: ${(raw / 1024).toFixed(0)} KB raw, ${(gz / 1024).toFixed(0)} KB gzipped`);
}
