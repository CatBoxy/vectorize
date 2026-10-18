// The trace worker: loads the wasm module once, keeps the current image,
// and runs every trace off the main thread. Errors from the module are
// `{ error }` objects and are forwarded; a trap (a panic in the core) is
// caught here, reported with the panic message, and marked fatal so the
// page replaces this worker instead of reusing a tainted instance.

import type { MainToWorker, TracePayload, WasmError, WasmTraceOutput, WorkerToMain } from "../lib/protocol";

interface WasmModule {
  default(input?: { module_or_path?: string | URL }): Promise<unknown>;
  trace?(bytes: Uint8Array, params: string): WasmTraceOutput | WasmError;
  trace_rgba?(width: number, height: number, rgba: Uint8Array, params: string): WasmTraceOutput | WasmError;
  svg_to_pdf?(svg: string): Uint8Array | WasmError;
  last_panic(): string | undefined;
  version(): string;
  has_pdf(): boolean;
}

interface Scope {
  location: Location;
  postMessage(message: WorkerToMain, transfer?: Transferable[]): void;
  onmessage: ((ev: MessageEvent<MainToWorker>) => void) | null;
}

const scope = self as unknown as Scope;
const base = scope.location.origin;

let tracer: Promise<WasmModule> | null = null;
let pdf: Promise<WasmModule> | null = null;
let payload: TracePayload | null = null;

async function loadModule(dir: string): Promise<WasmModule> {
  const js = `${base}/wasm/${dir}vectorize_wasm.js`;
  const wasm = `${base}/wasm/${dir}vectorize_wasm_bg.wasm`;
  const mod = (await import(/* webpackIgnore: true */ /* turbopackIgnore: true */ js)) as WasmModule;
  await mod.default({ module_or_path: wasm });
  return mod;
}

function loadTracer(): Promise<WasmModule> {
  if (!tracer) {
    tracer = loadModule("").catch((e) => {
      tracer = null;
      throw e;
    });
  }
  return tracer;
}

function loadPdf(): Promise<WasmModule> {
  if (!pdf) {
    pdf = loadModule("pdf/").catch((e) => {
      pdf = null;
      throw e;
    });
  }
  return pdf;
}

async function pdfAvailable(): Promise<boolean> {
  try {
    const r = await fetch(`${base}/wasm/pdf/vectorize_wasm_bg.wasm`, { method: "HEAD" });
    return r.ok;
  } catch {
    return false;
  }
}

function post(msg: WorkerToMain, transfer?: Transferable[]) {
  scope.postMessage(msg, transfer);
}

function isError(v: unknown): v is WasmError {
  return typeof v === "object" && v !== null && "error" in v;
}

/** A trap in the module, or a panic recorded by its hook. */
function trapOf(mod: WasmModule | null, e: unknown): string | null {
  const panic = mod?.last_panic();
  if (panic) return panic;
  if (e instanceof WebAssembly.RuntimeError) return e.message;
  return null;
}

scope.onmessage = async (ev) => {
  const msg = ev.data;
  switch (msg.type) {
    case "image":
      payload = msg.payload;
      break;
    case "trace": {
      let mod: WasmModule | null = null;
      try {
        mod = await loadTracer();
        if (!payload) throw new Error("no image loaded");
        if (!mod.trace || !mod.trace_rgba) throw new Error("this module has no tracer");
        const t0 = performance.now();
        const out =
          payload.kind === "bytes"
            ? mod.trace(payload.bytes, msg.params)
            : mod.trace_rgba(payload.width, payload.height, payload.rgba, msg.params);
        const ms = performance.now() - t0;
        if (isError(out)) {
          post({ type: "error", id: msg.id, code: out.error.code, message: out.error.message, fatal: false });
        } else {
          const transfer: Transferable[] = [];
          const rgba = out.stage_dumps?.regions?.rgba;
          if (rgba) transfer.push(rgba.buffer as ArrayBuffer);
          post({ type: "result", id: msg.id, svg: out.svg, stats: out.stats, stage_dumps: out.stage_dumps, ms }, transfer);
        }
      } catch (e) {
        const trap = trapOf(mod, e);
        if (trap !== null) {
          tracer = null;
          post({ type: "error", id: msg.id, code: "panic", message: `the tracer crashed: ${trap}`, fatal: true });
        } else {
          post({ type: "error", id: msg.id, code: "worker", message: e instanceof Error ? e.message : String(e), fatal: false });
        }
      }
      break;
    }
    case "pdf": {
      let mod: WasmModule | null = null;
      try {
        mod = await loadPdf();
        if (!mod.svg_to_pdf) throw new Error("this module has no PDF exporter");
        const out = mod.svg_to_pdf(msg.svg);
        if (isError(out)) {
          post({ type: "error", id: msg.id, code: out.error.code, message: out.error.message, fatal: false });
        } else {
          post({ type: "pdf", id: msg.id, bytes: out }, [out.buffer as ArrayBuffer]);
        }
      } catch (e) {
        const trap = trapOf(mod, e);
        if (trap !== null) {
          pdf = null;
          post({ type: "error", id: msg.id, code: "panic", message: `the PDF exporter crashed: ${trap}`, fatal: true });
        } else {
          post({ type: "error", id: msg.id, code: "pdf", message: e instanceof Error ? e.message : String(e), fatal: false });
        }
      }
      break;
    }
  }
};

// Warm up: fetch and compile the tracer before the first image arrives.
void (async () => {
  try {
    const [mod, hasPdf] = await Promise.all([loadTracer(), pdfAvailable()]);
    post({ type: "ready", version: mod.version(), hasPdf });
  } catch (e) {
    post({ type: "error", id: -1, code: "load", message: `could not load the tracer: ${e instanceof Error ? e.message : String(e)}`, fatal: true });
  }
})();
