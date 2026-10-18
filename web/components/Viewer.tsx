"use client";

// The views: side by side, overlay with opacity, wipe; a shared zoom and
// pan (wheel to zoom about the cursor, drag to pan, up to 800%); the node
// overlay parsed from the SVG; and the inspector's region map and polygon
// overlays.

import { useEffect, useLayoutEffect, useMemo, useRef, type CSSProperties, type PointerEvent, type ReactNode } from "react";
import type { TraceResult } from "@/app/App";
import { MAX_ZOOM, MIN_ZOOM } from "@/components/Toolbar";
import type { LoadedImage } from "@/lib/image";
import { parseNodes } from "@/lib/svgNodes";

export type ViewMode = "side" | "overlay" | "wipe";

interface Pan {
  x: number;
  y: number;
}

interface Props {
  image: LoadedImage;
  result: TraceResult | null;
  view: ViewMode;
  opacity: number;
  wipe: number;
  showNodes: boolean;
  showHandles: boolean;
  showRegions: boolean;
  showPolygons: boolean;
  zoom: number;
  pan: Pan;
  onZoomPan: (zoom: number, pan: Pan) => void;
  /** Increment to fit the image to the viewport. */
  fitRequest: number;
}

const clampZoom = (z: number) => Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z));

export default function Viewer(p: Props) {
  const { image, result, view, zoom, pan, onZoomPan } = p;
  const rootRef = useRef<HTMLDivElement>(null);
  const w = image.width;
  const h = image.height;

  // Fit on request (new image, Fit button) using the first pane's size.
  useLayoutEffect(() => {
    const pane = rootRef.current?.querySelector(".viewport");
    if (!pane) return;
    const rect = pane.getBoundingClientRect();
    const margin = 24;
    const z = clampZoom(Math.min((rect.width - margin) / w, (rect.height - margin) / h));
    onZoomPan(z, { x: (rect.width - w * z) / 2, y: (rect.height - h * z) / 2 });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [p.fitRequest, w, h, view]);

  const nodes = useMemo(() => (result && p.showNodes ? parseNodes(result.svg) : null), [result, p.showNodes]);

  const original = (
    <img
      className="layer"
      src={image.url}
      width={w}
      height={h}
      alt=""
      draggable={false}
      style={{ imageRendering: zoom >= 2 ? "pixelated" : "auto" }}
    />
  );
  const traced = (style?: CSSProperties) =>
    result ? (
      <img className="layer" src={result.svgUrl} width={w} height={h} alt="" draggable={false} style={style} />
    ) : (
      <div className="layer placeholder">Tracing…</div>
    );
  const inspectorLayers = (
    <>
      {p.showRegions && result?.stage_dumps?.regions && <RegionLayer regions={result.stage_dumps.regions} width={w} height={h} />}
      {p.showPolygons && result?.stage_dumps && <PolygonLayer polygons={result.stage_dumps.polygons} width={w} height={h} zoom={zoom} />}
    </>
  );
  const nodeLayer = nodes && <NodeLayer parsed={nodes} width={w} height={h} zoom={zoom} handles={p.showHandles} />;

  const common = { zoom, pan, onZoomPan, width: w, height: h };

  return (
    <div ref={rootRef} className={`viewer ${view}`}>
      {view === "side" && (
        <>
          <Viewport {...common} label="Original">
            {original}
            {inspectorLayers}
          </Viewport>
          <Viewport {...common} label="Trace">
            {traced()}
            {nodeLayer}
          </Viewport>
        </>
      )}
      {view === "overlay" && (
        <Viewport {...common} label={`Trace at ${Math.round(p.opacity * 100)}% over the original`}>
          {original}
          {inspectorLayers}
          {traced({ opacity: p.opacity })}
          {nodeLayer}
        </Viewport>
      )}
      {view === "wipe" && (
        <Viewport {...common} label="Original | Trace">
          {original}
          {inspectorLayers}
          {traced({ clipPath: `inset(0 0 0 ${p.wipe}%)` })}
          {nodeLayer}
          <div className="wipe-line" style={{ left: `${p.wipe}%`, borderLeftWidth: `${1 / zoom}px` }} />
        </Viewport>
      )}
    </div>
  );
}

function Viewport({
  zoom,
  pan,
  onZoomPan,
  width,
  height,
  label,
  children,
}: {
  zoom: number;
  pan: Pan;
  onZoomPan: (zoom: number, pan: Pan) => void;
  width: number;
  height: number;
  label: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const latest = useRef({ zoom, pan });
  latest.current = { zoom, pan };
  const drag = useRef<{ x: number; y: number; pan: Pan } | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const { zoom, pan } = latest.current;
      const rect = el.getBoundingClientRect();
      const cx = e.clientX - rect.left;
      const cy = e.clientY - rect.top;
      const factor = Math.exp(-e.deltaY * (e.deltaMode === 1 ? 0.05 : 0.0015));
      const z = clampZoom(zoom * factor);
      onZoomPan(z, { x: cx - ((cx - pan.x) / zoom) * z, y: cy - ((cy - pan.y) / zoom) * z });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [onZoomPan]);

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    drag.current = { x: e.clientX, y: e.clientY, pan: latest.current.pan };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const d = drag.current;
    if (!d) return;
    onZoomPan(latest.current.zoom, { x: d.pan.x + (e.clientX - d.x), y: d.pan.y + (e.clientY - d.y) });
  };
  const onPointerUp = (e: PointerEvent<HTMLDivElement>) => {
    drag.current = null;
    e.currentTarget.releasePointerCapture(e.pointerId);
  };

  return (
    <div
      ref={ref}
      className="viewport"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      onDoubleClick={() => onZoomPan(1, latest.current.pan)}
    >
      <div
        className="content"
        style={{ width, height, transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`, transformOrigin: "0 0" }}
      >
        {children}
      </div>
      <span className="viewport-label">{label}</span>
    </div>
  );
}

function RegionLayer({ regions, width, height }: { regions: { width: number; height: number; rgba: Uint8Array }; width: number; height: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    if (!canvas) return;
    canvas.width = regions.width;
    canvas.height = regions.height;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const data = new Uint8ClampedArray(regions.rgba.buffer as ArrayBuffer, regions.rgba.byteOffset, regions.rgba.byteLength);
    ctx.putImageData(new ImageData(data, regions.width, regions.height), 0, 0);
  }, [regions]);
  return <canvas ref={ref} className="layer regions" style={{ width, height }} />;
}

function PolygonLayer({
  polygons,
  width,
  height,
  zoom,
}: {
  polygons: { closed: boolean; pts: [number, number][] }[];
  width: number;
  height: number;
  zoom: number;
}) {
  const total = polygons.reduce((n, p) => n + p.pts.length, 0);
  const dots = total <= 6000;
  return (
    <svg className="layer overlay" viewBox={`0 0 ${width} ${height}`} width={width} height={height}>
      {polygons.map((poly, i) => {
        const pts = poly.pts.map(([x, y]) => `${x},${y}`).join(" ");
        return poly.closed ? (
          <polygon key={i} points={pts} fill="none" stroke="#ff2bd6" strokeWidth={1 / zoom} />
        ) : (
          <polyline key={i} points={pts} fill="none" stroke="#ff2bd6" strokeWidth={1 / zoom} />
        );
      })}
      {dots &&
        polygons.map((poly, i) =>
          poly.pts.map(([x, y], j) => <circle key={`${i}-${j}`} cx={x} cy={y} r={1.6 / zoom} fill="#ff2bd6" />),
        )}
    </svg>
  );
}

function NodeLayer({
  parsed,
  width,
  height,
  zoom,
  handles,
}: {
  parsed: ReturnType<typeof parseNodes>;
  width: number;
  height: number;
  zoom: number;
  handles: boolean;
}) {
  const r = 3 / zoom;
  const sw = 1 / zoom;
  return (
    <svg className="layer overlay" viewBox={`0 0 ${width} ${height}`} width={width} height={height}>
      {handles &&
        parsed.handles.map((hd, i) => (
          <g key={`h${i}`}>
            <line x1={hd.x1} y1={hd.y1} x2={hd.x2} y2={hd.y2} stroke="#4fc3f7" strokeWidth={sw} />
            <circle cx={hd.x2} cy={hd.y2} r={r * 0.6} fill="#4fc3f7" />
          </g>
        ))}
      {parsed.nodes.map((n, i) =>
        n.corner ? (
          <rect key={i} x={n.x - r} y={n.y - r} width={2 * r} height={2 * r} fill="#ff5252" stroke="#fff" strokeWidth={sw * 0.75} />
        ) : (
          <circle key={i} cx={n.x} cy={n.y} r={r} fill="#2979ff" stroke="#fff" strokeWidth={sw * 0.75} />
        ),
      )}
    </svg>
  );
}
