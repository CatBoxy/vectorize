"use client";

import { useState } from "react";
import {
  applyPreset,
  PRESET_HINTS,
  PRESET_LABELS,
  toleranceFromDetail,
  type Controls as ControlsState,
  type Mode,
  type PresetName,
} from "@/lib/params";

interface Props {
  value: ControlsState;
  onChange: (next: ControlsState) => void;
}

const PRESET_ORDER: PresetName[] = ["none", "logo", "sketch", "flat_illustration", "pixel_art"];

export default function Controls({ value, onChange }: Props) {
  const [advanced, setAdvanced] = useState(false);
  const set = <K extends keyof ControlsState>(key: K, v: ControlsState[K]) => onChange({ ...value, [key]: v });
  const tolerance = toleranceFromDetail(value.detail);

  return (
    <section className="panel">
      <h2>Trace</h2>

      <label className="row">
        <span>Preset</span>
        <select value={value.preset} onChange={(e) => onChange(applyPreset(e.target.value as PresetName))}>
          {PRESET_ORDER.map((p) => (
            <option key={p} value={p}>
              {PRESET_LABELS[p]}
            </option>
          ))}
        </select>
      </label>
      <p className="hint">{PRESET_HINTS[value.preset]}</p>

      <div className="row">
        <span>Mode</span>
        <div className="segmented">
          {(["color", "grayscale", "black_and_white"] as Mode[]).map((m) => (
            <button key={m} type="button" className={value.mode === m ? "on" : ""} onClick={() => set("mode", m)}>
              {m === "color" ? "Colour" : m === "grayscale" ? "Grey" : "B&W"}
            </button>
          ))}
        </div>
      </div>

      {value.mode !== "black_and_white" && (
        <label className="row">
          <span>Colours</span>
          <span className="inline">
            <input
              type="checkbox"
              checked={value.colors === null}
              onChange={(e) => set("colors", e.target.checked ? null : 8)}
            />{" "}
            auto
            {value.colors !== null && (
              <input
                type="number"
                min={2}
                max={64}
                value={value.colors}
                onChange={(e) => set("colors", Math.max(2, Math.min(64, Number(e.target.value) || 2)))}
              />
            )}
          </span>
        </label>
      )}

      {value.mode === "black_and_white" && (
        <Slider label="Threshold" value={value.threshold} min={0} max={255} step={1} onChange={(v) => set("threshold", v)} />
      )}

      <Slider
        label="Detail"
        value={value.detail}
        min={0}
        max={100}
        step={1}
        display={`${value.detail} · ${tolerance.toFixed(2)} px`}
        onChange={(v) => set("detail", v)}
      />
      <Slider label="Corners" value={value.corners} min={0} max={100} step={1} onChange={(v) => set("corners", v)} />
      <Slider label="Noise" value={value.noise} min={1} max={100} step={1} display={`${value.noise} px²`} onChange={(v) => set("noise", v)} />

      <div className="row">
        <span>Method</span>
        <div className="segmented">
          <button type="button" className={value.method === "abutting" ? "on" : ""} onClick={() => set("method", "abutting")}>
            Abutting
          </button>
          <button type="button" className={value.method === "overlapping" ? "on" : ""} onClick={() => set("method", "overlapping")}>
            Overlapping
          </button>
        </div>
      </div>

      <label className="check">
        <input type="checkbox" checked={value.snapLines} onChange={(e) => set("snapLines", e.target.checked)} /> Snap curves to lines
      </label>
      <label className="check">
        <input type="checkbox" checked={value.ignoreWhite} onChange={(e) => set("ignoreWhite", e.target.checked)} /> Ignore white (the background colour)
      </label>

      <button type="button" className="link" onClick={() => setAdvanced((a) => !a)}>
        {advanced ? "Hide" : "Show"} advanced
      </button>
      {advanced && (
        <div className="advanced">
          <Slider label="Regularity" value={value.regularity} min={0} max={100} step={1} onChange={(v) => set("regularity", v)} />
          <label className="check">
            <input type="checkbox" checked={value.primitives} onChange={(e) => set("primitives", e.target.checked)} /> Whole circles, ellipses, rounded rectangles, polygons
          </label>
          <button type="button" className="link" onClick={() => onChange(applyPreset(value.preset))}>
            Reset to preset values
          </button>
        </div>
      )}
    </section>
  );
}

function Slider({
  label,
  value,
  min,
  max,
  step,
  display,
  onChange,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  display?: string;
  onChange: (v: number) => void;
}) {
  return (
    <label className="row slider">
      <span>{label}</span>
      <input type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <output>{display ?? value}</output>
    </label>
  );
}
