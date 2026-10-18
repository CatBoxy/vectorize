// The controls in Illustrator's vocabulary (PLAN.md §1) and their mapping
// onto the core's parameter JSON (crates/core/src/params.rs). The preset
// table mirrors `Params::preset` so the sliders show a preset's values
// when it is picked; the preset name is still sent, so the fields the UI
// does not expose (colour merge distance, pixel-art upscale) come from
// the core.

export type Mode = "color" | "grayscale" | "black_and_white";
export type Method = "abutting" | "overlapping";
export type PresetName = "none" | "logo" | "sketch" | "flat_illustration" | "pixel_art";

export interface Controls {
  preset: PresetName;
  mode: Mode;
  /** Maximum colours; null = automatic. */
  colors: number | null;
  /** Detail 0..100 (Illustrator "Paths"): drives the polygon tolerance and the fit accuracy. */
  detail: number;
  /** Corners 0..100. */
  corners: number;
  /** Noise: minimum region area, px². */
  noise: number;
  method: Method;
  snapLines: boolean;
  ignoreWhite: boolean;
  /** Regularity 0..100 (M4). */
  regularity: number;
  primitives: boolean;
  /** Black-and-white threshold 0..255. */
  threshold: number;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const round = (v: number, places: number) => Math.round(v * 10 ** places) / 10 ** places;

/** Detail 0 = 2 px tolerance (loose), 100 = 0.1 px (tight), log scale. */
export function toleranceFromDetail(detail: number): number {
  return 2.0 * Math.pow(0.05, clamp(detail, 0, 100) / 100);
}

export function detailFromTolerance(tolerance: number): number {
  return Math.round((100 * Math.log(tolerance / 2)) / Math.log(0.05));
}

export const DEFAULTS: Controls = {
  preset: "none",
  mode: "color",
  colors: null,
  detail: detailFromTolerance(0.2),
  corners: 50,
  noise: 4,
  method: "abutting",
  snapLines: true,
  ignoreWhite: true,
  regularity: 50,
  primitives: true,
  threshold: 128,
};

export const PRESETS: Record<PresetName, Partial<Controls>> = {
  none: {},
  logo: { regularity: 100 },
  sketch: {
    mode: "black_and_white",
    detail: detailFromTolerance(0.3),
    corners: 30,
    noise: 8,
    snapLines: false,
    regularity: 0,
    primitives: false,
  },
  flat_illustration: { noise: 2, regularity: 25 },
  pixel_art: { corners: 100, noise: 1, regularity: 100, primitives: false },
};

export const PRESET_LABELS: Record<PresetName, string> = {
  none: "Default",
  logo: "Logo",
  sketch: "Sketch",
  flat_illustration: "Flat illustration",
  pixel_art: "Pixel art",
};

export const PRESET_HINTS: Record<PresetName, string> = {
  none: "The core defaults: colour, automatic palette, moderate regularity.",
  logo: "Few colours, full regularity: straight is straight, equal is equal, circles are circles.",
  sketch: "Black and white, soft corners, no snapping: a drawing stays a drawing.",
  flat_illustration: "More colours kept apart, light regularity.",
  pixel_art: "Traces a 4× nearest-neighbour upscale; every pixel edge is a corner.",
};

export function applyPreset(preset: PresetName): Controls {
  return { ...DEFAULTS, ...PRESETS[preset], preset };
}

/** The JSON object the wasm `trace` takes. */
export function toParams(c: Controls, dumps: boolean): Record<string, unknown> {
  const tolerance = round(toleranceFromDetail(c.detail), 3);
  const fit_accuracy = round(clamp(1.75 * tolerance, 0.1, 3.5), 3);
  const p: Record<string, unknown> = {
    mode: c.mode,
    colors: c.colors,
    tolerance,
    fit_accuracy,
    corners: round(c.corners / 100, 3),
    min_area: Math.max(1, Math.round(c.noise)),
    method: c.method,
    snap_lines: c.snapLines,
    ignore_background: c.ignoreWhite,
    regularity: round(c.regularity / 100, 3),
    primitives: c.primitives,
    threshold: Math.round(clamp(c.threshold, 0, 255)),
  };
  if (c.preset !== "none") p.preset = c.preset;
  if (dumps) p.dumps = true;
  return p;
}
