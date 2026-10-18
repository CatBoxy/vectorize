"use client";

// The product page (PLAN.md M5): one image, the Illustrator-style
// controls, a worker tracing it off the main thread, and the views to
// judge the result. Nothing here talks to a server.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import Controls from "@/components/Controls";
import Exports from "@/components/Exports";
import InputPanel from "@/components/InputPanel";
import Inspector from "@/components/Inspector";
import Toolbar from "@/components/Toolbar";
import Viewer, { type ViewMode } from "@/components/Viewer";
import { TraceClient, isSuperseded } from "@/lib/client";
import { formatBytes } from "@/lib/export";
import { fetchImage, loadImage, MAX_EDGE, type LoadedImage } from "@/lib/image";
import { DEFAULTS, toParams, type Controls as ControlsState } from "@/lib/params";
import type { TraceOk } from "@/lib/protocol";

export interface TraceResult extends TraceOk {
  /** Object URL of the SVG, for `<img>` display. */
  svgUrl: string;
}

const DEBOUNCE_MS = 150;

export default function App() {
  const clientRef = useRef<TraceClient | null>(null);
  const pendingRef = useRef(0);
  const [engine, setEngine] = useState<{ version: string; hasPdf: boolean } | null>(null);
  const [image, setImage] = useState<LoadedImage | null>(null);
  const [controls, setControls] = useState<ControlsState>(DEFAULTS);
  const [inspector, setInspector] = useState(false);
  const [result, setResult] = useState<TraceResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);

  const [view, setView] = useState<ViewMode>("side");
  const [opacity, setOpacity] = useState(0.6);
  const [wipe, setWipe] = useState(50);
  const [showNodes, setShowNodes] = useState(false);
  const [showHandles, setShowHandles] = useState(false);
  const [showRegions, setShowRegions] = useState(true);
  const [showPolygons, setShowPolygons] = useState(true);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [fitRequest, setFitRequest] = useState(0);

  useEffect(() => {
    const client = new TraceClient();
    client.onReady = (info) => setEngine(info);
    client.onFatal = (message) => {
      pendingRef.current = 0;
      setBusy(false);
      setError(message);
    };
    clientRef.current = client;
    return () => {
      client.terminate();
      clientRef.current = null;
    };
  }, []);

  const open = useCallback(async (blob: Blob, name: string) => {
    setError(null);
    try {
      const img = await loadImage(blob, name);
      setImage((prev) => {
        if (prev) URL.revokeObjectURL(prev.url);
        return img;
      });
      clientRef.current?.setImage(img.payload);
      setResult((prev) => {
        if (prev) URL.revokeObjectURL(prev.svgUrl);
        return null;
      });
      setFitRequest((n) => n + 1);
      setNotice(
        img.downscaled
          ? `Downscaled from ${img.originalWidth}×${img.originalHeight} to ${img.width}×${img.height}: inputs are capped at ${MAX_EDGE} px on the long edge.`
          : null,
      );
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const openUrl = useCallback(
    async (url: string) => {
      setError(null);
      try {
        const { blob, name } = await fetchImage(url);
        await open(blob, name);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [open],
  );

  // Paste: an image, or a URL as text.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      const items = e.clipboardData?.items;
      if (!items) return;
      for (const item of items) {
        if (item.kind === "file" && item.type.startsWith("image/")) {
          const f = item.getAsFile();
          if (f) {
            e.preventDefault();
            void open(f, f.name || "pasted.png");
            return;
          }
        }
      }
      const text = e.clipboardData?.getData("text/plain")?.trim();
      if (text && /^https?:\/\//i.test(text)) {
        e.preventDefault();
        void openUrl(text);
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [open, openUrl]);

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setDragging(false);
      const file = e.dataTransfer.files?.[0];
      if (file) {
        void open(file, file.name);
        return;
      }
      const uri = e.dataTransfer.getData("text/uri-list") || e.dataTransfer.getData("text/plain");
      if (uri && /^https?:\/\//i.test(uri.trim())) void openUrl(uri.trim().split("\n")[0]);
    },
    [open, openUrl],
  );

  // Debounced re-trace on any change of image, controls or inspector.
  const paramsJson = useMemo(() => JSON.stringify(toParams(controls, inspector)), [controls, inspector]);
  useEffect(() => {
    const client = clientRef.current;
    if (!image || !client) return;
    const timer = setTimeout(() => {
      pendingRef.current += 1;
      setBusy(true);
      client
        .trace(paramsJson)
        .then((r) => {
          setResult((prev) => {
            if (prev) URL.revokeObjectURL(prev.svgUrl);
            return { ...r, svgUrl: URL.createObjectURL(new Blob([r.svg], { type: "image/svg+xml;charset=utf-8" })) };
          });
          setError(null);
        })
        .catch((e: Error) => {
          if (!isSuperseded(e)) setError(e.message);
        })
        .finally(() => {
          pendingRef.current = Math.max(0, pendingRef.current - 1);
          setBusy(pendingRef.current > 0);
        });
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [image, paramsJson]);

  const onZoomPan = useCallback((z: number, p: { x: number; y: number }) => {
    setZoom(z);
    setPan(p);
  }, []);

  const status = result
    ? `${image?.width}×${image?.height} · traced in ${result.ms.toFixed(0)} ms · ${result.stats.paths} paths · ${result.stats.nodes} nodes · ${result.stats.colors} colours · ${formatBytes(result.stats.svg_bytes)}`
    : image
      ? `${image.width}×${image.height}`
      : "";

  return (
    <div
      className={`app${dragging ? " dragging" : ""}`}
      onDragOver={(e) => {
        e.preventDefault();
        if (!dragging) setDragging(true);
      }}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDragging(false);
      }}
      onDrop={onDrop}
    >
      <aside className="sidebar">
        <header className="brand">
          <h1>Vectorize</h1>
          <p>Raster → SVG for logos, icons and flat illustration. Runs in your browser.</p>
        </header>
        <InputPanel image={image} onFile={(f) => void open(f, f.name)} onUrl={openUrl} />
        <Controls value={controls} onChange={setControls} />
        <Exports
          image={image}
          result={result}
          hasPdf={engine?.hasPdf ?? false}
          requestPdf={(svg) => clientRef.current?.pdf(svg) ?? Promise.reject(new Error("no worker"))}
          onError={setError}
        />
        <Inspector
          enabled={inspector}
          onToggle={setInspector}
          result={result}
          showRegions={showRegions}
          onShowRegions={setShowRegions}
          showPolygons={showPolygons}
          onShowPolygons={setShowPolygons}
        />
        <footer className="about">
          <p>No accounts, no uploads, no analytics. Your image never leaves this browser: the tracer is WebAssembly running in a Web Worker.</p>
          <p className="muted">{engine ? `Engine ${engine.version}${engine.hasPdf ? "" : " · PDF export unavailable"}` : "Loading the tracer…"}</p>
        </footer>
      </aside>
      <main className="stage">
        <Toolbar
          view={view}
          onView={setView}
          opacity={opacity}
          onOpacity={setOpacity}
          wipe={wipe}
          onWipe={setWipe}
          showNodes={showNodes}
          onShowNodes={setShowNodes}
          showHandles={showHandles}
          onShowHandles={setShowHandles}
          zoom={zoom}
          onZoom={(z) => setFitZoom(z)}
          onFit={() => setFitRequest((n) => n + 1)}
          busy={busy}
          hasImage={!!image}
        />
        {image ? (
          <Viewer
            image={image}
            result={result}
            view={view}
            opacity={opacity}
            wipe={wipe}
            showNodes={showNodes}
            showHandles={showHandles}
            showRegions={inspector && showRegions}
            showPolygons={inspector && showPolygons}
            zoom={zoom}
            pan={pan}
            onZoomPan={onZoomPan}
            fitRequest={fitRequest}
          />
        ) : (
          <div className="empty">
            <div className="empty-card">
              <h2>Drop a PNG or JPEG here</h2>
              <p>or paste an image, or enter a URL on the left. The trace starts immediately.</p>
              <p className="muted">Best on vector-born art: logos, icons, clip art, flat illustration. Photos come out posterised, like in every tracer.</p>
            </div>
          </div>
        )}
        <div className="statusbar">
          <span className="status">{status}</span>
          {notice && <span className="notice">{notice}</span>}
          {error && <span className="error">{error}</span>}
        </div>
      </main>
    </div>
  );

  function setFitZoom(z: number) {
    // Zoom about the centre of the stage: keep the image point that is at
    // the centre where it is.
    const el = document.querySelector(".viewport");
    const rect = el?.getBoundingClientRect();
    const cx = rect ? rect.width / 2 : 0;
    const cy = rect ? rect.height / 2 : 0;
    setPan({ x: cx - ((cx - pan.x) / zoom) * z, y: cy - ((cy - pan.y) / zoom) * z });
    setZoom(z);
  }
}
