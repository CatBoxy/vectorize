"use client";

import type { ViewMode } from "@/components/Viewer";

interface Props {
  view: ViewMode;
  onView: (v: ViewMode) => void;
  opacity: number;
  onOpacity: (v: number) => void;
  wipe: number;
  onWipe: (v: number) => void;
  showNodes: boolean;
  onShowNodes: (v: boolean) => void;
  showHandles: boolean;
  onShowHandles: (v: boolean) => void;
  zoom: number;
  onZoom: (z: number) => void;
  onFit: () => void;
  busy: boolean;
  hasImage: boolean;
}

export const MIN_ZOOM = 0.1;
export const MAX_ZOOM = 8;

export default function Toolbar(p: Props) {
  const views: [ViewMode, string][] = [
    ["side", "Side by side"],
    ["overlay", "Overlay"],
    ["wipe", "Wipe"],
  ];
  return (
    <div className="toolbar">
      <div className="segmented">
        {views.map(([v, label]) => (
          <button key={v} type="button" className={p.view === v ? "on" : ""} onClick={() => p.onView(v)} disabled={!p.hasImage}>
            {label}
          </button>
        ))}
      </div>
      {p.view === "overlay" && (
        <label className="inline">
          Trace opacity
          <input type="range" min={0} max={1} step={0.01} value={p.opacity} onChange={(e) => p.onOpacity(Number(e.target.value))} />
          <output>{Math.round(p.opacity * 100)}%</output>
        </label>
      )}
      {p.view === "wipe" && (
        <label className="inline">
          Wipe
          <input type="range" min={0} max={100} step={0.5} value={p.wipe} onChange={(e) => p.onWipe(Number(e.target.value))} />
          <output>{Math.round(p.wipe)}%</output>
        </label>
      )}
      <label className="check inline">
        <input type="checkbox" checked={p.showNodes} onChange={(e) => p.onShowNodes(e.target.checked)} /> Nodes
      </label>
      {p.showNodes && (
        <label className="check inline">
          <input type="checkbox" checked={p.showHandles} onChange={(e) => p.onShowHandles(e.target.checked)} /> Handles
        </label>
      )}
      <span className="spacer" />
      <div className="zoom">
        <button type="button" onClick={() => p.onZoom(Math.max(MIN_ZOOM, p.zoom / 1.5))} disabled={!p.hasImage}>
          −
        </button>
        <output>{Math.round(p.zoom * 100)}%</output>
        <button type="button" onClick={() => p.onZoom(Math.min(MAX_ZOOM, p.zoom * 1.5))} disabled={!p.hasImage}>
          +
        </button>
        <button type="button" onClick={p.onFit} disabled={!p.hasImage}>
          Fit
        </button>
        <button type="button" onClick={() => p.onZoom(1)} disabled={!p.hasImage}>
          100%
        </button>
        <button type="button" onClick={() => p.onZoom(8)} disabled={!p.hasImage}>
          800%
        </button>
      </div>
      <span className={`busy${p.busy ? " on" : ""}`} aria-live="polite">
        {p.busy ? "Tracing…" : ""}
      </span>
    </div>
  );
}
