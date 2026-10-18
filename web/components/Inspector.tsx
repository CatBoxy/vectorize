"use client";

import type { TraceResult } from "@/app/App";

interface Props {
  enabled: boolean;
  onToggle: (v: boolean) => void;
  result: TraceResult | null;
  showRegions: boolean;
  onShowRegions: (v: boolean) => void;
  showPolygons: boolean;
  onShowPolygons: (v: boolean) => void;
}

export default function Inspector({ enabled, onToggle, result, showRegions, onShowRegions, showPolygons, onShowPolygons }: Props) {
  const dumps = result?.stage_dumps;
  const stages = result?.stats.stage_ms ?? [];
  const total = stages.reduce((s, [, ms]) => s + ms, 0);
  return (
    <section className="panel">
      <h2>
        <label className="check">
          <input type="checkbox" checked={enabled} onChange={(e) => onToggle(e.target.checked)} /> Inspector
        </label>
      </h2>
      {enabled && (
        <>
          <p className="hint">The pipeline's intermediate results: palette, region map and polygons are dumped; the boundary graph and the fitted curves are the SVG itself.</p>
          <label className="check">
            <input type="checkbox" checked={showRegions} onChange={(e) => onShowRegions(e.target.checked)} /> Region map (one colour per region)
          </label>
          <label className="check">
            <input type="checkbox" checked={showPolygons} onChange={(e) => onShowPolygons(e.target.checked)} /> Polygons (vertices before fitting)
          </label>
          {dumps && (
            <>
              <div className="swatches">
                {dumps.palette.map((c, i) =>
                  i === 0 ? null : (
                    <span
                      key={i}
                      className={`swatch${dumps.background === i ? " bg" : ""}`}
                      style={{ background: c }}
                      title={`${c}${dumps.background === i ? " (background, dropped when Ignore white is on)" : ""}`}
                    />
                  ),
                )}
                <span className="muted">
                  {dumps.palette.length - 1} colours
                  {dumps.regions ? ` · ${dumps.regions.count} regions` : ""} · {dumps.polygons.length} chains
                </span>
              </div>
            </>
          )}
          {stages.length > 0 && (
            <table className="stages">
              <tbody>
                {stages.map(([name, ms]) => (
                  <tr key={name}>
                    <td>{name}</td>
                    <td className="num">{ms.toFixed(1)} ms</td>
                  </tr>
                ))}
                <tr className="total">
                  <td>total</td>
                  <td className="num">{total.toFixed(1)} ms</td>
                </tr>
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
  );
}
