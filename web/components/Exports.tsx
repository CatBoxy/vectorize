"use client";

import { useState } from "react";
import type { TraceResult } from "@/app/App";
import { download, formatBytes, svgBlob, svgToPng } from "@/lib/export";
import { baseName, type LoadedImage } from "@/lib/image";

interface Props {
  image: LoadedImage | null;
  result: TraceResult | null;
  hasPdf: boolean;
  requestPdf: (svg: string) => Promise<Uint8Array>;
  onError: (message: string | null) => void;
}

export default function Exports({ image, result, hasPdf, requestPdf, onError }: Props) {
  const [scale, setScale] = useState(2);
  const [copied, setCopied] = useState(false);
  const [working, setWorking] = useState<string | null>(null);
  const ready = !!image && !!result;
  const name = image ? baseName(image.name) : "trace";

  const run = async (what: string, job: () => Promise<void>) => {
    setWorking(what);
    onError(null);
    try {
      await job();
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    } finally {
      setWorking(null);
    }
  };

  return (
    <section className="panel">
      <h2>Export</h2>
      <div className="row wrap">
        <button type="button" disabled={!ready} onClick={() => result && download(svgBlob(result.svg), `${name}.svg`)}>
          Download SVG
        </button>
        <button
          type="button"
          disabled={!ready}
          onClick={() =>
            result &&
            void run("copy", async () => {
              await navigator.clipboard.writeText(result.svg);
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            })
          }
        >
          {copied ? "Copied" : "Copy SVG"}
        </button>
      </div>
      <div className="row wrap">
        <button
          type="button"
          disabled={!ready || working === "png"}
          onClick={() =>
            image &&
            result &&
            void run("png", async () => {
              const blob = await svgToPng(result.svg, image.width, image.height, scale);
              download(blob, `${name}@${scale}x.png`);
            })
          }
        >
          {working === "png" ? "Rendering…" : "Download PNG"}
        </button>
        <select value={scale} onChange={(e) => setScale(Number(e.target.value))} disabled={!ready}>
          {[0.5, 1, 2, 4, 8].map((s) => (
            <option key={s} value={s}>
              {s}× {image ? `(${Math.round(image.width * s)}×${Math.round(image.height * s)})` : ""}
            </option>
          ))}
        </select>
      </div>
      <div className="row wrap">
        <button
          type="button"
          disabled={!ready || !hasPdf || working === "pdf"}
          title={hasPdf ? "Vector PDF through svg2pdf, converted in the worker" : "The PDF module is not deployed"}
          onClick={() =>
            result &&
            void run("pdf", async () => {
              const bytes = await requestPdf(result.svg);
              download(new Blob([bytes as BlobPart], { type: "application/pdf" }), `${name}.pdf`);
            })
          }
        >
          {working === "pdf" ? "Converting…" : "Download PDF"}
        </button>
        {result && <span className="muted">SVG {formatBytes(result.stats.svg_bytes)}</span>}
      </div>
    </section>
  );
}
