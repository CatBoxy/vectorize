// The page's side of the worker: one worker, one image, at most one trace
// in flight and at most one waiting (slider changes only ever queue the
// latest). A fatal error (a trap in the module) replaces the worker and
// re-sends the image.

import type { TraceOk, TracePayload, WorkerToMain } from "./protocol";

type Waiter<T> = { id: number; resolve: (v: T) => void; reject: (e: Error) => void };

export class TraceClient {
  private worker: Worker | null = null;
  private nextId = 1;
  private payload: TracePayload | null = null;
  private inflight: Waiter<TraceOk> | null = null;
  private pending: { params: string; waiter: Waiter<TraceOk> } | null = null;
  private pdfWaiters = new Map<number, Waiter<Uint8Array>>();

  onReady: ((info: { version: string; hasPdf: boolean }) => void) | null = null;
  onFatal: ((message: string) => void) | null = null;

  constructor() {
    this.spawn();
  }

  private spawn() {
    const w = new Worker(new URL("../worker/trace.worker.ts", import.meta.url), { type: "module" });
    w.onmessage = (ev: MessageEvent<WorkerToMain>) => this.handle(ev.data);
    w.onerror = (ev) => {
      this.failAll(new Error(ev.message || "worker error"));
      this.onFatal?.(ev.message || "the worker failed");
    };
    this.worker = w;
    if (this.payload) w.postMessage({ type: "image", payload: this.payload });
  }

  private handle(msg: WorkerToMain) {
    switch (msg.type) {
      case "ready":
        this.onReady?.({ version: msg.version, hasPdf: msg.hasPdf });
        break;
      case "result":
        if (this.inflight && this.inflight.id === msg.id) {
          const w = this.inflight;
          this.inflight = null;
          w.resolve({ svg: msg.svg, stats: msg.stats, stage_dumps: msg.stage_dumps, ms: msg.ms });
        }
        this.drain();
        break;
      case "pdf": {
        const w = this.pdfWaiters.get(msg.id);
        if (w) {
          this.pdfWaiters.delete(msg.id);
          w.resolve(msg.bytes);
        }
        break;
      }
      case "error": {
        const err = Object.assign(new Error(msg.message), { code: msg.code });
        if (this.inflight && this.inflight.id === msg.id) {
          const w = this.inflight;
          this.inflight = null;
          w.reject(err);
        }
        const pw = this.pdfWaiters.get(msg.id);
        if (pw) {
          this.pdfWaiters.delete(msg.id);
          pw.reject(err);
        }
        if (msg.fatal) {
          this.worker?.terminate();
          this.failAll(err);
          this.onFatal?.(msg.message);
          this.spawn();
        } else {
          this.drain();
        }
        break;
      }
    }
  }

  private failAll(err: Error) {
    this.inflight?.reject(err);
    this.inflight = null;
    this.pending?.waiter.reject(err);
    this.pending = null;
    for (const w of this.pdfWaiters.values()) w.reject(err);
    this.pdfWaiters.clear();
  }

  private drain() {
    if (this.inflight || !this.pending || !this.worker) return;
    const { params, waiter } = this.pending;
    this.pending = null;
    this.inflight = waiter;
    this.worker.postMessage({ type: "trace", id: waiter.id, params });
  }

  /** Hand the worker a new image. Copied, not transferred: it is re-sent if the worker is replaced. */
  setImage(payload: TracePayload) {
    this.payload = payload;
    this.worker?.postMessage({ type: "image", payload });
  }

  /** Trace the current image. A call made while one is running waits; a
   * second one replaces it (the first rejects with `superseded`). */
  trace(params: string): Promise<TraceOk> {
    return new Promise<TraceOk>((resolve, reject) => {
      const waiter = { id: this.nextId++, resolve, reject };
      if (this.pending) this.pending.waiter.reject(Object.assign(new Error("superseded"), { code: "superseded" }));
      this.pending = { params, waiter };
      this.drain();
    });
  }

  pdf(svg: string): Promise<Uint8Array> {
    return new Promise<Uint8Array>((resolve, reject) => {
      if (!this.worker) return reject(new Error("no worker"));
      const id = this.nextId++;
      this.pdfWaiters.set(id, { id, resolve, reject });
      this.worker.postMessage({ type: "pdf", id, svg });
    });
  }

  terminate() {
    this.worker?.terminate();
    this.worker = null;
    this.failAll(new Error("closed"));
  }
}

export function isSuperseded(e: unknown): boolean {
  return typeof e === "object" && e !== null && (e as { code?: string }).code === "superseded";
}
