// Messages between the page and the trace worker, and the shapes the wasm
// module returns (crates/wasm/src/lib.rs).

export type TracePayload =
  | { kind: "bytes"; bytes: Uint8Array }
  | { kind: "rgba"; width: number; height: number; rgba: Uint8Array };

export interface TraceStats {
  paths: number;
  nodes: number;
  colors: number;
  svg_bytes: number;
  /** Per stage, in order: [name, milliseconds]. */
  stage_ms: [string, number][];
}

export interface StageDumps {
  /** `#rrggbb` per palette label; index 0 is never painted. */
  palette: string[];
  background?: number;
  polygons: { closed: boolean; pts: [number, number][] }[];
  regions?: { width: number; height: number; count: number; rgba: Uint8Array };
}

export interface WasmError {
  error: { code: string; message: string };
}

export interface WasmTraceOutput {
  svg: string;
  stats: TraceStats;
  stage_dumps?: StageDumps;
}

export type MainToWorker =
  | { type: "image"; payload: TracePayload }
  | { type: "trace"; id: number; params: string }
  | { type: "pdf"; id: number; svg: string };

export type WorkerToMain =
  | { type: "ready"; version: string; hasPdf: boolean }
  | { type: "result"; id: number; svg: string; stats: TraceStats; stage_dumps?: StageDumps; ms: number }
  | { type: "pdf"; id: number; bytes: Uint8Array }
  | { type: "error"; id: number; code: string; message: string; fatal: boolean };

export interface TraceOk {
  svg: string;
  stats: TraceStats;
  stage_dumps?: StageDumps;
  /** Wall time of the wasm call inside the worker. */
  ms: number;
}
