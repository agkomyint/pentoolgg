// Pure document helpers shared by the canvas, outline and inspector. The server
// remains the validator: every render and save sends the document through Rust.
import type { Doc, Layer, Page, SceneNode } from "@/lib/api";

export type Matrix = [number, number, number, number, number, number];
export type Rect = { x: number; y: number; width: number; height: number };

export const IDENTITY: Matrix = [1, 0, 0, 1, 0, 0];

export const isScene = (doc: Doc | null | undefined): boolean => !!doc && doc.version >= 4;

export function uid(prefix: string): string {
  const random = typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID().slice(0, 8) : Date.now().toString(36);
  return `${prefix}-${random}`;
}

export function clone<T>(value: T): T {
  return structuredClone(value);
}

export function pageOf(doc: Doc | null, pageId: string | undefined): Page | undefined {
  return doc?.pages?.find((p) => p.id === pageId) ?? doc?.pages?.[0];
}

/** Direct children of a layer in paint order. Legacy v3 layers keep paths then texts. */
export function layerNodes(layer: Layer): SceneNode[] {
  if (Array.isArray(layer.nodes)) return layer.nodes;
  return [
    ...(layer.paths ?? []).map((n) => (n.kind ? n : Object.assign(n, { kind: undefined }))),
    ...(layer.texts ?? []),
  ];
}

export function nodeKind(node: SceneNode, layer?: Layer): string {
  if (typeof node.kind === "string") return node.kind;
  if (layer?.texts?.includes(node)) return "text";
  return "path";
}

export function children(node: SceneNode): SceneNode[] {
  return Array.isArray(node.children) ? node.children : [];
}

export type Found = {
  node: SceneNode;
  layer: Layer;
  /** Array holding the node: a layer array or a group's children. */
  siblings: SceneNode[];
  parent: SceneNode | null;
  /** Product of the parent group transforms. */
  world: Matrix;
};

export function multiply(a: Matrix, b: Matrix): Matrix {
  return [
    a[0] * b[0] + a[2] * b[1],
    a[1] * b[0] + a[3] * b[1],
    a[0] * b[2] + a[2] * b[3],
    a[1] * b[2] + a[3] * b[3],
    a[0] * b[4] + a[2] * b[5] + a[4],
    a[1] * b[4] + a[3] * b[5] + a[5],
  ];
}

export function nodeMatrix(node: SceneNode): Matrix {
  const t = node.transform;
  return Array.isArray(t) && t.length === 6 ? (t.map(Number) as Matrix) : IDENTITY;
}

export function invert(m: Matrix): Matrix | null {
  const det = m[0] * m[3] - m[1] * m[2];
  if (!det || !Number.isFinite(det)) return null;
  return [
    m[3] / det,
    -m[1] / det,
    -m[2] / det,
    m[0] / det,
    (m[2] * m[5] - m[3] * m[4]) / det,
    (m[1] * m[4] - m[0] * m[5]) / det,
  ];
}

export function apply(m: Matrix, x: number, y: number): [number, number] {
  return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
}

export function findNode(page: Page | undefined, id: string | null): Found | null {
  if (!page || !id) return null;
  const visit = (
    list: SceneNode[],
    layer: Layer,
    parent: SceneNode | null,
    world: Matrix,
  ): Found | null => {
    for (const node of list) {
      if (node.id === id) return { node, layer, siblings: list, parent, world };
      const kids = children(node);
      if (kids.length) {
        const hit = visit(kids, layer, node, multiply(world, nodeMatrix(node)));
        if (hit) return hit;
      }
    }
    return null;
  };
  for (const layer of page.layers ?? []) {
    if (Array.isArray(layer.nodes)) {
      const hit = visit(layer.nodes, layer, null, IDENTITY);
      if (hit) return hit;
    } else {
      for (const key of ["paths", "texts"] as const) {
        const list = layer[key];
        const node = list?.find((n) => n.id === id);
        if (node && list) return { node, layer, siblings: list, parent: null, world: IDENTITY };
      }
    }
  }
  return null;
}

export function collectIds(doc: Doc): Set<string> {
  const ids = new Set<string>();
  const visit = (nodes: SceneNode[]) => {
    for (const node of nodes) {
      ids.add(node.id);
      visit(children(node));
    }
  };
  for (const page of doc.pages ?? []) {
    ids.add(page.id);
    for (const layer of page.layers ?? []) {
      ids.add(layer.id);
      visit(layerNodes(layer));
    }
  }
  return ids;
}

// ---- style accessors: v4+ keeps `style.<key>.fallback`, v3 keeps plain fields ----

export function getStyle(node: SceneNode, key: string): unknown {
  const style = node.style as Record<string, { fallback?: unknown }> | undefined;
  if (style && key in style) return style[key]?.fallback;
  return node[key];
}

export function setStyle(node: SceneNode, key: string, value: unknown, scene: boolean): void {
  if (scene) {
    const style = (node.style ??= {}) as Record<string, { fallback?: unknown; ref?: string }>;
    const entry = style[key] ?? { fallback: value };
    entry.fallback = value;
    // A manual edit detaches the value from its shared token.
    delete entry.ref;
    style[key] = entry;
  } else {
    node[key] = value;
  }
}

// ---- bounds ----

const NUMBER = /[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?/g;

/** Control-polygon bounds of an SVG path, absolute or relative commands. */
export function pathBounds(d: string): Rect | null {
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  const take = (x: number, y: number) => {
    if (!Number.isFinite(x) || !Number.isFinite(y)) return;
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x);
    maxY = Math.max(maxY, y);
  };
  let cx = 0;
  let cy = 0;
  let sx = 0;
  let sy = 0;
  for (const match of d.matchAll(/([a-zA-Z])([^a-zA-Z]*)/g)) {
    const command = match[1]!;
    const args = (match[2]!.match(NUMBER) ?? []).map(Number);
    const lower = command.toLowerCase();
    const rel = command === lower;
    const ox = rel ? cx : 0;
    const oy = rel ? cy : 0;
    if (lower === "z") {
      cx = sx;
      cy = sy;
      continue;
    }
    const step = { m: 2, l: 2, t: 2, h: 1, v: 1, c: 6, s: 4, q: 4, a: 7 }[lower];
    if (!step) continue;
    for (let i = 0; i + step <= args.length; i += step) {
      const a = args.slice(i, i + step);
      if (lower === "h") {
        cx = a[0]! + ox;
        take(cx, cy);
      } else if (lower === "v") {
        cy = a[0]! + oy;
        take(cx, cy);
      } else if (lower === "a") {
        cx = a[5]! + ox;
        cy = a[6]! + oy;
        take(cx, cy);
      } else {
        for (let k = 0; k < step; k += 2) take(a[k]! + ox, a[k + 1]! + oy);
        cx = a[step - 2]! + ox;
        cy = a[step - 1]! + oy;
        if (lower === "m") {
          sx = cx;
          sy = cy;
        }
      }
    }
  }
  if (!Number.isFinite(minX)) return null;
  return { x: minX, y: minY, width: maxX - minX, height: maxY - minY };
}

/** Bounds in the node's parent space (its own transform applied). */
export function localBounds(node: SceneNode, layer?: Layer): Rect | null {
  const kind = nodeKind(node, layer);
  const n = (key: string) => Number(node[key] ?? 0);
  let box: Rect | null = null;
  if (kind === "rect" || kind === "image" || kind === "raster" || kind === "photo" || kind === "text-frame" || kind === "graphic-frame") {
    if (typeof node.width === "number" && typeof node.height === "number") {
      box = { x: n("x"), y: n("y"), width: n("width"), height: n("height") };
    }
  } else if (kind === "ellipse") {
    box = { x: n("cx") - n("radius_x"), y: n("cy") - n("radius_y"), width: 2 * n("radius_x"), height: 2 * n("radius_y") };
  } else if (kind === "line") {
    const x = Math.min(n("x1"), n("x2"));
    const y = Math.min(n("y1"), n("y2"));
    box = { x, y, width: Math.abs(n("x2") - n("x1")), height: Math.abs(n("y2") - n("y1")) };
  } else if (kind === "path") {
    box = typeof node.d === "string" ? pathBounds(node.d) : null;
  } else if (kind === "text") {
    const size = Number(node.font_size ?? 16);
    const lines = String(node.content ?? "").split(/\r\n?|\n/);
    const longest = Math.max(1, ...lines.map((l) => l.length));
    const width = longest * size * 0.56;
    const height = lines.length * size * Number(node.line_height ?? 1.2);
    const align = String(node.align ?? "left");
    const left = align === "center" ? n("x") - width / 2 : align === "right" ? n("x") - width : n("x");
    box = { x: left, y: n("y") - size, width, height };
  } else if (kind === "group") {
    let union: Rect | null = null;
    for (const child of children(node)) {
      const b = boundsIn(child, layer);
      if (b) union = unite(union, b);
    }
    // Group bounds are the children's bounds before the group's own transform.
    return union ? transformRect(union, nodeMatrix(node)) : null;
  }
  if (!box) return null;
  return transformRect(box, nodeMatrix(node));
}

function boundsIn(node: SceneNode, layer?: Layer): Rect | null {
  return localBounds(node, layer);
}

export function unite(a: Rect | null, b: Rect): Rect {
  if (!a) return b;
  const x = Math.min(a.x, b.x);
  const y = Math.min(a.y, b.y);
  return { x, y, width: Math.max(a.x + a.width, b.x + b.width) - x, height: Math.max(a.y + a.height, b.y + b.height) - y };
}

export function transformRect(r: Rect, m: Matrix): Rect {
  const points = [apply(m, r.x, r.y), apply(m, r.x + r.width, r.y), apply(m, r.x, r.y + r.height), apply(m, r.x + r.width, r.y + r.height)];
  const xs = points.map((p) => p[0]);
  const ys = points.map((p) => p[1]);
  return { x: Math.min(...xs), y: Math.min(...ys), width: Math.max(...xs) - Math.min(...xs), height: Math.max(...ys) - Math.min(...ys) };
}

/** Bounds of a found node in page space. */
export function pageBounds(found: Found): Rect | null {
  const local = localBounds(found.node, found.layer);
  return local ? transformRect(local, found.world) : null;
}

/** Top-most visible, unlocked node under a page point, depth first. */
export function hitTest(page: Page | undefined, x: number, y: number): Found | null {
  if (!page) return null;
  let best: Found | null = null;
  const visit = (list: SceneNode[], layer: Layer, parent: SceneNode | null, world: Matrix) => {
    for (const node of list) {
      if (node.visible === false) continue;
      const kids = children(node);
      if (kids.length) visit(kids, layer, node, multiply(world, nodeMatrix(node)));
      if (kind(node) === "group") continue;
      const found: Found = { node, layer, siblings: list, parent, world };
      const b = pageBounds(found);
      if (b && x >= b.x - 2 && x <= b.x + b.width + 2 && y >= b.y - 2 && y <= b.y + b.height + 2) best = found;
    }
  };
  const kind = (n: SceneNode) => (typeof n.kind === "string" ? n.kind : "path");
  for (const layer of page.layers ?? []) {
    if (layer.visible === false || layer.locked) continue;
    if (Array.isArray(layer.nodes)) visit(layer.nodes, layer, null, IDENTITY);
    else {
      for (const key of ["paths", "texts"] as const) {
        for (const node of layer[key] ?? []) {
          const found: Found = { node, layer, siblings: layer[key]!, parent: null, world: IDENTITY };
          const b = pageBounds({ ...found, node: key === "texts" ? Object.assign({}, node, { kind: "text" }) : node });
          if (b && x >= b.x - 2 && x <= b.x + b.width + 2 && y >= b.y - 2 && y <= b.y + b.height + 2) best = found;
        }
      }
    }
  }
  return best;
}

// ---- path construction ----

export type PenPoint = { x: number; y: number; in?: { x: number; y: number }; out?: { x: number; y: number } };
const f = (v: number) => (Math.round(v * 100) / 100).toFixed(2);

export function penPathData(points: PenPoint[], closed = false): string {
  const list = closed && points.length ? [...points, points[0]!] : points;
  const body = list
    .map((p, i) => {
      if (!i) return `M ${f(p.x)} ${f(p.y)}`;
      const prev = list[i - 1]!;
      const a = prev.out ?? prev;
      const b = p.in ?? p;
      return `C ${f(a.x)} ${f(a.y)} ${f(b.x)} ${f(b.y)} ${f(p.x)} ${f(p.y)}`;
    })
    .join(" ");
  return closed ? `${body} Z` : body;
}

const K = 0.5522847498;
export function rectPath(r: Rect): string {
  return `M ${f(r.x)} ${f(r.y)} L ${f(r.x + r.width)} ${f(r.y)} L ${f(r.x + r.width)} ${f(r.y + r.height)} L ${f(r.x)} ${f(r.y + r.height)} Z`;
}
export function ellipsePath(r: Rect): string {
  const rx = r.width / 2;
  const ry = r.height / 2;
  const cx = r.x + rx;
  const cy = r.y + ry;
  return (
    `M ${f(cx + rx)} ${f(cy)} ` +
    `C ${f(cx + rx)} ${f(cy + ry * K)} ${f(cx + rx * K)} ${f(cy + ry)} ${f(cx)} ${f(cy + ry)} ` +
    `C ${f(cx - rx * K)} ${f(cy + ry)} ${f(cx - rx)} ${f(cy + ry * K)} ${f(cx - rx)} ${f(cy)} ` +
    `C ${f(cx - rx)} ${f(cy - ry * K)} ${f(cx - rx * K)} ${f(cy - ry)} ${f(cx)} ${f(cy - ry)} ` +
    `C ${f(cx + rx * K)} ${f(cy - ry)} ${f(cx + rx)} ${f(cy - ry * K)} ${f(cx + rx)} ${f(cy)} Z`
  );
}

export type Tool = "select" | "pen" | "rect" | "ellipse" | "line" | "text" | "hand";

export type Appearance = { fill: string; stroke: string; strokeWidth: number };

/** Build a new object for the active document format. */
export function makeNode(
  tool: Exclude<Tool, "select" | "hand">,
  id: string,
  scene: boolean,
  look: Appearance,
  geometry: { rect?: Rect; d?: string; at?: { x: number; y: number }; closed?: boolean; line?: [number, number, number, number] },
): { node: SceneNode; bucket: "nodes" | "paths" | "texts" } {
  const stroke = look.stroke;
  const width = look.strokeWidth;
  const wrap = (value: unknown) => ({ fallback: value });
  if (tool === "text") {
    const at = geometry.at ?? { x: 0, y: 0 };
    const base = { id, content: "Text", x: at.x, y: at.y, font_family: "Atkinson Hyperlegible", font_size: 32, font_weight: 400 };
    return scene
      ? { node: { kind: "text", ...base, style: { fill: wrap(look.stroke) } }, bucket: "nodes" }
      : {
          node: { ...base, italic: false, fill: look.stroke, align: "left", letter_spacing: 0, line_height: 1.2, transform: [1, 0, 0, 1, 0, 0] },
          bucket: "texts",
        };
  }
  const rect = geometry.rect ?? { x: 0, y: 0, width: 100, height: 100 };
  if (scene && tool === "rect") {
    return { node: { kind: "rect", id, ...rect, style: { fill: wrap(look.fill === "none" ? look.stroke : look.fill) } }, bucket: "nodes" };
  }
  if (scene && tool === "ellipse") {
    return {
      node: {
        kind: "ellipse",
        id,
        cx: rect.x + rect.width / 2,
        cy: rect.y + rect.height / 2,
        radius_x: Math.max(0.5, rect.width / 2),
        radius_y: Math.max(0.5, rect.height / 2),
        style: { fill: wrap(look.fill === "none" ? look.stroke : look.fill) },
      },
      bucket: "nodes",
    };
  }
  let d = geometry.d ?? "";
  if (tool === "rect") d = rectPath(rect);
  if (tool === "ellipse") d = ellipsePath(rect);
  if (tool === "line") {
    const [x1, y1, x2, y2] = geometry.line ?? [0, 0, 0, 0];
    d = `M ${f(x1)} ${f(y1)} L ${f(x2)} ${f(y2)}`;
  }
  const closed = tool === "rect" || tool === "ellipse" || !!geometry.closed;
  const fill = tool === "pen" || tool === "line" ? (geometry.closed ? look.fill : "none") : look.fill;
  return scene
    ? {
        node: {
          kind: "path",
          id,
          d,
          style: {
            fill: wrap(fill),
            stroke: wrap(stroke),
            stroke_width: wrap(width),
            stroke_linecap: wrap("round"),
            stroke_linejoin: wrap("round"),
          },
        },
        bucket: "nodes",
      }
    : {
        node: { id, d, stroke, stroke_width: width, fill, stroke_linecap: "round", stroke_linejoin: "round", stroke_miterlimit: 4, closed },
        bucket: "paths",
      };
}

/** Insert a node at the top of a layer's paint order for any document version. */
export function addToLayer(layer: Layer, node: SceneNode, bucket: "nodes" | "paths" | "texts"): void {
  const key = Array.isArray(layer.nodes) ? "nodes" : bucket === "nodes" ? "nodes" : bucket;
  const list = (layer[key] ??= []) as SceneNode[];
  list.push(node);
}

/** Left-multiply a translation in parent space. */
export function translateNode(node: SceneNode, dx: number, dy: number, scene: boolean, kind: string): void {
  if (scene || Array.isArray(node.transform)) {
    const m = nodeMatrix(node);
    node.transform = [m[0], m[1], m[2], m[3], m[4] + dx, m[5] + dy];
  } else if (kind === "text") {
    node.x = Number(node.x ?? 0) + dx;
    node.y = Number(node.y ?? 0) + dy;
  } else {
    node.transform = [1, 0, 0, 1, dx, dy];
  }
}

/** Shift an SVG path's absolute coordinates by (dx, dy); relative commands are untouched. */
export function shiftPath(d: string, dx: number, dy: number): string {
  return d.replace(/([a-zA-Z])([^a-zA-Z]*)/g, (all, command: string, rest: string) => {
    if (command === command.toLowerCase() || command === "Z") return all;
    const nums = (rest.match(NUMBER) ?? []).map(Number);
    if (command === "H") return `H ${f(nums[0]! + dx)}`;
    if (command === "V") return `V ${f(nums[0]! + dy)}`;
    if (command === "A") {
      const out: string[] = [];
      for (let i = 0; i + 7 <= nums.length; i += 7) {
        const a = nums.slice(i, i + 7);
        out.push(`${f(a[0]!)} ${f(a[1]!)} ${f(a[2]!)} ${a[3]} ${a[4]} ${f(a[5]! + dx)} ${f(a[6]! + dy)}`);
      }
      return `A ${out.join(" ")}`;
    }
    const pairs: string[] = [];
    for (let i = 0; i + 1 < nums.length; i += 2) pairs.push(`${f(nums[i]! + dx)} ${f(nums[i + 1]! + dy)}`);
    return `${command} ${pairs.join(" ")}`;
  });
}

/** Apply a scale/translate matrix to a path's coordinates (no rotation or skew). */
export function affinePath(d: string, m: Matrix): string {
  const sx = m[0];
  const sy = m[3];
  return d.replace(/([a-zA-Z])([^a-zA-Z]*)/g, (all, command: string, rest: string) => {
    const lower = command.toLowerCase();
    if (lower === "z") return all;
    const rel = command === lower;
    const nums = (rest.match(NUMBER) ?? []).map(Number);
    const px = (v: number) => f(rel ? v * sx : v * sx + m[4]);
    const py = (v: number) => f(rel ? v * sy : v * sy + m[5]);
    if (lower === "h") return `${command} ${nums.map(px).join(" ")}`;
    if (lower === "v") return `${command} ${nums.map(py).join(" ")}`;
    if (lower === "a") {
      const out: string[] = [];
      for (let i = 0; i + 7 <= nums.length; i += 7) {
        const a = nums.slice(i, i + 7);
        out.push(`${f(Math.abs(a[0]! * sx))} ${f(Math.abs(a[1]! * sy))} ${f(a[2]!)} ${a[3]} ${a[4]} ${px(a[5]!)} ${py(a[6]!)}`);
      }
      return `${command} ${out.join(" ")}`;
    }
    const pairs: string[] = [];
    for (let i = 0; i + 1 < nums.length; i += 2) pairs.push(`${px(nums[i]!)} ${py(nums[i + 1]!)}`);
    return `${command} ${pairs.join(" ")}`;
  });
}
