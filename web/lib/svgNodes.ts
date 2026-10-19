// Node overlay: parse the path data of an SVG into its nodes (segment end
// points), classify each as a corner or a smooth join by comparing the
// incoming and outgoing tangents, and collect the cubic control arms.
// Handles the absolute commands the tracer emits (M L C A Z) and, for
// pasted SVGs, the rest of the grammar including relative forms.

export interface PathNode {
  x: number;
  y: number;
  corner: boolean;
}

export interface Handle {
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

export interface ParsedNodes {
  nodes: PathNode[];
  handles: Handle[];
  segments: number;
}

type Vec = [number, number];

interface Segment {
  end: Vec;
  /** Unit tangent leaving the start. */
  out: Vec;
  /** Unit tangent arriving at the end. */
  inn: Vec;
}

const CORNER_DEG = 12;

function norm(v: Vec): Vec {
  const h = Math.hypot(v[0], v[1]);
  return h > 1e-12 ? [v[0] / h, v[1] / h] : [0, 0];
}

function angleDeg(a: Vec, b: Vec): number {
  if ((a[0] === 0 && a[1] === 0) || (b[0] === 0 && b[1] === 0)) return 0;
  const d = Math.max(-1, Math.min(1, a[0] * b[0] + a[1] * b[1]));
  return (Math.acos(d) * 180) / Math.PI;
}

/** Tangents of an SVG arc at its two ends (W3C SVG 1.1 F.6.5). */
function arcTangents(p1: Vec, p2: Vec, rx: number, ry: number, phiDeg: number, large: boolean, sweep: boolean): [Vec, Vec] {
  const chord: Vec = norm([p2[0] - p1[0], p2[1] - p1[1]]);
  rx = Math.abs(rx);
  ry = Math.abs(ry);
  if (rx < 1e-9 || ry < 1e-9) return [chord, chord];
  const phi = (phiDeg * Math.PI) / 180;
  const cos = Math.cos(phi);
  const sin = Math.sin(phi);
  const dx = (p1[0] - p2[0]) / 2;
  const dy = (p1[1] - p2[1]) / 2;
  const x1p = cos * dx + sin * dy;
  const y1p = -sin * dx + cos * dy;
  const lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
  if (lambda > 1) {
    const s = Math.sqrt(lambda);
    rx *= s;
    ry *= s;
  }
  const num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
  const den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
  let coef = den > 0 ? Math.sqrt(Math.max(0, num / den)) : 0;
  if (large === sweep) coef = -coef;
  const cxp = (coef * rx * y1p) / ry;
  const cyp = (-coef * ry * x1p) / rx;
  const theta1 = Math.atan2((y1p - cyp) / ry, (x1p - cxp) / rx);
  const theta2 = Math.atan2((-y1p - cyp) / ry, (-x1p - cxp) / rx);
  const sign = sweep ? 1 : -1;
  const tangent = (theta: number): Vec => {
    const tx = -rx * Math.sin(theta);
    const ty = ry * Math.cos(theta);
    return norm([sign * (cos * tx - sin * ty), sign * (sin * tx + cos * ty)]);
  };
  return [tangent(theta1), tangent(theta2)];
}

const NUM = /[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?/g;
const CMD = /[MmLlHhVvCcSsQqTtAaZz]/;

function tokenize(d: string): { cmd: string; args: number[] }[] {
  const out: { cmd: string; args: number[] }[] = [];
  let i = 0;
  while (i < d.length) {
    const ch = d[i];
    if (CMD.test(ch)) {
      let j = i + 1;
      while (j < d.length && !CMD.test(d[j])) j++;
      const args = (d.slice(i + 1, j).match(NUM) || []).map(Number);
      out.push({ cmd: ch, args });
      i = j;
    } else {
      i++;
    }
  }
  return out;
}

function parsePath(d: string, nodes: PathNode[], handles: Handle[]): number {
  let segments = 0;
  let cur: Vec = [0, 0];
  let start: Vec = [0, 0];
  let subpath: Segment[] = [];
  let lastCubicCtrl: Vec | null = null;
  let lastQuadCtrl: Vec | null = null;

  const flush = (closed: boolean) => {
    if (subpath.length === 0) return;
    const n = subpath.length;
    for (let i = 0; i < n; i++) {
      const seg = subpath[i];
      const next = i + 1 < n ? subpath[i + 1] : closed ? subpath[0] : null;
      const corner = next ? angleDeg(seg.inn, next.out) > CORNER_DEG : true;
      // Skip a degenerate closing segment's end: it is the start node.
      if (i === n - 1 && closed && Math.hypot(seg.end[0] - start[0], seg.end[1] - start[1]) < 1e-9 && n > 1) {
        continue;
      }
      nodes.push({ x: seg.end[0], y: seg.end[1], corner });
    }
    if (!closed) {
      nodes.push({ x: start[0], y: start[1], corner: true });
    } else if (n > 0) {
      const last = subpath[n - 1];
      const closes = Math.hypot(last.end[0] - start[0], last.end[1] - start[1]) < 1e-9;
      if (!closes) nodes.push({ x: start[0], y: start[1], corner: angleDeg(norm([start[0] - last.end[0], start[1] - last.end[1]]), subpath[0].out) > CORNER_DEG });
    }
    segments += n;
    subpath = [];
  };

  const line = (to: Vec) => {
    const t = norm([to[0] - cur[0], to[1] - cur[1]]);
    subpath.push({ end: to, out: t, inn: t });
    cur = to;
    lastCubicCtrl = null;
    lastQuadCtrl = null;
  };
  const cubic = (c1: Vec, c2: Vec, to: Vec) => {
    let out = norm([c1[0] - cur[0], c1[1] - cur[1]]);
    let inn = norm([to[0] - c2[0], to[1] - c2[1]]);
    const chord = norm([to[0] - cur[0], to[1] - cur[1]]);
    if (out[0] === 0 && out[1] === 0) out = norm([c2[0] - cur[0], c2[1] - cur[1]]);
    if (out[0] === 0 && out[1] === 0) out = chord;
    if (inn[0] === 0 && inn[1] === 0) inn = norm([to[0] - c1[0], to[1] - c1[1]]);
    if (inn[0] === 0 && inn[1] === 0) inn = chord;
    handles.push({ x1: cur[0], y1: cur[1], x2: c1[0], y2: c1[1] });
    handles.push({ x1: to[0], y1: to[1], x2: c2[0], y2: c2[1] });
    subpath.push({ end: to, out, inn });
    cur = to;
    lastCubicCtrl = c2;
    lastQuadCtrl = null;
  };

  for (const { cmd, args } of tokenize(d)) {
    const rel = cmd === cmd.toLowerCase() && cmd !== "Z" && cmd !== "z";
    const abs = (x: number, y: number): Vec => (rel ? [cur[0] + x, cur[1] + y] : [x, y]);
    switch (cmd.toUpperCase()) {
      case "M": {
        flush(false);
        for (let i = 0; i + 1 < args.length; i += 2) {
          const p = abs(args[i], args[i + 1]);
          if (i === 0) {
            cur = p;
            start = p;
          } else {
            line(p);
          }
        }
        lastCubicCtrl = null;
        lastQuadCtrl = null;
        break;
      }
      case "L":
        for (let i = 0; i + 1 < args.length; i += 2) line(abs(args[i], args[i + 1]));
        break;
      case "H":
        for (const x of args) line(rel ? [cur[0] + x, cur[1]] : [x, cur[1]]);
        break;
      case "V":
        for (const y of args) line(rel ? [cur[0], cur[1] + y] : [cur[0], y]);
        break;
      case "C":
        for (let i = 0; i + 5 < args.length; i += 6) {
          cubic(abs(args[i], args[i + 1]), abs(args[i + 2], args[i + 3]), abs(args[i + 4], args[i + 5]));
        }
        break;
      case "S":
        for (let i = 0; i + 3 < args.length; i += 4) {
          const c1: Vec = lastCubicCtrl ? [2 * cur[0] - lastCubicCtrl[0], 2 * cur[1] - lastCubicCtrl[1]] : cur;
          cubic(c1, abs(args[i], args[i + 1]), abs(args[i + 2], args[i + 3]));
        }
        break;
      case "Q":
        for (let i = 0; i + 3 < args.length; i += 4) {
          const q = abs(args[i], args[i + 1]);
          const to = abs(args[i + 2], args[i + 3]);
          const c1: Vec = [cur[0] + (2 / 3) * (q[0] - cur[0]), cur[1] + (2 / 3) * (q[1] - cur[1])];
          const c2: Vec = [to[0] + (2 / 3) * (q[0] - to[0]), to[1] + (2 / 3) * (q[1] - to[1])];
          cubic(c1, c2, to);
          lastQuadCtrl = q;
          lastCubicCtrl = null;
        }
        break;
      case "T":
        for (let i = 0; i + 1 < args.length; i += 2) {
          const q: Vec = lastQuadCtrl ? [2 * cur[0] - lastQuadCtrl[0], 2 * cur[1] - lastQuadCtrl[1]] : cur;
          const to = abs(args[i], args[i + 1]);
          const c1: Vec = [cur[0] + (2 / 3) * (q[0] - cur[0]), cur[1] + (2 / 3) * (q[1] - cur[1])];
          const c2: Vec = [to[0] + (2 / 3) * (q[0] - to[0]), to[1] + (2 / 3) * (q[1] - to[1])];
          cubic(c1, c2, to);
          lastQuadCtrl = q;
          lastCubicCtrl = null;
        }
        break;
      case "A":
        for (let i = 0; i + 6 < args.length; i += 7) {
          const to = abs(args[i + 5], args[i + 6]);
          const [out, inn] = arcTangents(cur, to, args[i], args[i + 1], args[i + 2], args[i + 3] !== 0, args[i + 4] !== 0);
          subpath.push({ end: to, out, inn });
          cur = to;
          lastCubicCtrl = null;
          lastQuadCtrl = null;
        }
        break;
      case "Z":
        if (Math.hypot(cur[0] - start[0], cur[1] - start[1]) > 1e-9) line(start);
        flush(true);
        cur = start;
        break;
    }
  }
  flush(false);
  return segments;
}

const D_ATTR = /\bd\s*=\s*"([^"]*)"/g;

export function parseNodes(svg: string): ParsedNodes {
  const nodes: PathNode[] = [];
  const handles: Handle[] = [];
  let segments = 0;
  for (const m of svg.matchAll(D_ATTR)) {
    segments += parsePath(m[1], nodes, handles);
  }
  return { nodes, handles, segments };
}
