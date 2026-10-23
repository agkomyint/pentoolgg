import { useCallback, useEffect, useMemo, useRef } from "react";
import { postJson, type Doc } from "@/lib/api";
import { apply, findNode, invert, nodeWorldTransform, pageOf, type Matrix } from "@/lib/model";
import { useStudio } from "@/store";
import { usePaint } from "./rasterStore";
import { addPaintLayer, collectRasters, type RasterNode } from "./paintLayer";

export type { RasterNode } from "./paintLayer";

export function useRasterLayers(): RasterNode[] {
  const { doc, pageId } = useStudio();
  return useMemo(() => collectRasters(doc, pageId), [doc, pageId]);
}

/** A pointer sample: `x`/`y` in layer pixels, `gx`/`gy` in page units. */
type Pt = { x: number; y: number; gx: number; gy: number };
type Ghost = { pts: Pt[]; size: number; kind: string; color: string; erase: boolean; hardness: number; flow: number; clip: number[] | null; done: boolean; order: number };
type Geometry = { node: RasterNode | null; lx: number; ly: number; width: number; height: number; world: Matrix; inverse: Matrix };

const SNAP = 6;
const CURSOR_TOOLS = ["brush", "eraser", "clone", "heal", "quickmask"];
const STROKE_ACTIONS: Record<string, string> = { brush: "stroke", eraser: "stroke", clone: "clone", heal: "heal", quickmask: "quickmask" };
/** Largest overlay backing store; beyond it the overlay is upscaled like the preview. */
const MAX_BACKING = 8192;

/** Soft-edged dabs along a path, in layer-parent units, matching the committed look. */
function dabs(ctx: CanvasRenderingContext2D, pts: { x: number; y: number }[], diameter: number, kind: string, rgb: string, hardness: number, flow: number) {
  const r = Math.max(0.5, diameter / 2);
  const pixel = kind === "pixel";
  const step = pixel ? 1 : Math.max(0.5, diameter * 0.1);
  const a = Math.min(1, Math.max(0, flow));
  const h = Math.min(0.98, Math.max(0, hardness));
  const stamp = (x: number, y: number) => {
    if (pixel) {
      const size = Math.max(1, Math.round(diameter));
      ctx.fillStyle = `rgba(${rgb},${a})`;
      ctx.fillRect(Math.floor(x) - Math.floor(size / 2), Math.floor(y) - Math.floor(size / 2), size, size);
    } else if (kind === "hard-round") {
      ctx.fillStyle = `rgba(${rgb},${a})`;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 7);
      ctx.fill();
    } else {
      const g = ctx.createRadialGradient(x, y, r * h, x, y, r);
      g.addColorStop(0, `rgba(${rgb},${a})`);
      g.addColorStop(0.25, `rgba(${rgb},${a * 0.84375})`);
      g.addColorStop(0.5, `rgba(${rgb},${a * 0.5})`);
      g.addColorStop(0.75, `rgba(${rgb},${a * 0.15625})`);
      g.addColorStop(1, `rgba(${rgb},0)`);
      ctx.fillStyle = g;
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 7);
      ctx.fill();
    }
  };
  if (!pts.length) return;
  let carry = 0;
  stamp(pts[0]!.x, pts[0]!.y);
  for (let i = 1; i < pts.length; i++) {
    const a0 = pts[i - 1]!;
    const b0 = pts[i]!;
    const len = Math.hypot(b0.x - a0.x, b0.y - a0.y);
    for (let d = step - carry; d <= len; d += step) stamp(a0.x + ((b0.x - a0.x) * d) / (len || 1), a0.y + ((b0.y - a0.y) * d) / (len || 1));
    carry = (carry + len) % step;
  }
}

function rgbOf(color: string): string {
  return color.startsWith("#") && color.length >= 7 ? [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16)).join(",") : "0,0,0";
}

/**
 * Paint tools on the main canvas. The overlay sits on the page preview, shares
 * its zoom and scroll, and draws strokes immediately; pixels change only
 * through POST /api/raster, and a stroke stays drawn until the preview of the
 * committed document is on screen.
 */
export function PaintOverlay({ canvas, cssWidth, cssHeight, renderedDoc, onPanStart }: {
  canvas: { width: number; height: number; background?: string };
  cssWidth: number;
  cssHeight: number;
  /** The document whose render is currently visible under the overlay. */
  renderedDoc: Doc | null;
  /** Lets the canvas take middle-button, Space and hand-tool drags as pans. */
  onPanStart: (event: React.PointerEvent) => boolean;
}) {
  const { doc, pageId } = useStudio();
  const paint = usePaint();
  const layers = useRasterLayers();
  const layer = layers.find((n) => n.id === paint.layerId) ?? layers[0] ?? null;
  const overlay = useRef<HTMLCanvasElement>(null);
  const live = useRef<{
    pointer: Pt | null;
    stroke: null | { samples: { x: number; y: number; pressure: number }[]; preview: Pt[]; lag: Pt } | { lasso: true } | { marquee: Pt };
    lasso: Pt[];
    marquee: { x: number; y: number; width: number; height: number } | null;
    source: { x: number; y: number } | null;
    pending: Ghost[];
  }>({ pointer: null, stroke: null, lasso: [], marquee: null, source: null, pending: [] });
  const queue = useRef<Promise<void>>(Promise.resolve());
  const inflight = useRef(0);
  const commits = useRef(new WeakMap<object, number>());
  const nextOrder = useRef(1);

  // Backing store follows the on-screen size so strokes stay sharp when zoomed.
  const dpr = typeof window === "undefined" ? 1 : window.devicePixelRatio || 1;
  const fitBacking = Math.min(1, MAX_BACKING / Math.max(1, cssWidth * dpr, cssHeight * dpr));
  const backingW = Math.max(1, Math.round(cssWidth * dpr * fitBacking));
  const backingH = Math.max(1, Math.round(cssHeight * dpr * fitBacking));

  const geometry = useCallback((): Geometry | null => {
    const page = pageOf(doc, pageId);
    if (!page) return null;
    if (!layer) {
      // No paint layer yet: the first stroke creates a page-sized one at the origin.
      return { node: null, lx: 0, ly: 0, width: canvas.width, height: canvas.height, world: [1, 0, 0, 1, 0, 0], inverse: [1, 0, 0, 1, 0, 0] };
    }
    const found = findNode(page, layer.id);
    const world = found && nodeWorldTransform(found);
    const inverse = world && invert(world);
    if (!world || !inverse) return null;
    return { node: layer, lx: layer.x ?? 0, ly: layer.y ?? 0, width: layer.width, height: layer.height, world, inverse };
  }, [doc, pageId, layer, canvas.width, canvas.height]);

  const draw = useCallback(() => {
    const el = overlay.current;
    const ctx = el?.getContext("2d");
    if (!el || !ctx) return;
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, el.width, el.height);
    const g = geometry();
    if (!g) return;
    const p = usePaint.getState();
    const l = live.current;
    const sx = el.width / canvas.width;
    const sy = el.height / canvas.height;
    const [a, b, c, d, e, f] = g.world;
    const layerSpace = () => ctx.setTransform(sx * a, sy * b, sx * c, sy * d, sx * e, sy * f);
    const toParent = (q: Pt) => {
      const [x, y] = apply(g.inverse, q.gx, q.gy);
      return { x, y };
    };
    const screenPx = 1 / Math.max(1e-6, sx * Math.hypot(a, b));
    if (p.rulers) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.font = `${10 * dpr}px sans-serif`;
      ctx.lineWidth = 1;
      const band = 14 * dpr;
      ctx.fillStyle = "rgba(255,255,255,.85)";
      ctx.fillRect(0, 0, el.width, band);
      ctx.fillRect(0, 0, band, el.height);
      ctx.fillStyle = "#000";
      ctx.strokeStyle = "#000";
      const unit = sx * Math.hypot(a, b);
      const step = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000].find((s) => s * unit >= 8 * dpr) ?? 1000;
      for (let x = 0; x <= g.width; x += step) {
        const [px] = apply(g.world, g.lx + x, g.ly);
        const long = x % (step * 5) === 0;
        ctx.beginPath();
        ctx.moveTo(px * sx, long ? 0 : band / 2);
        ctx.lineTo(px * sx, band);
        ctx.stroke();
        if (long) ctx.fillText(String(x), px * sx + 2, band - 4);
      }
      for (let y = 0; y <= g.height; y += step) {
        const [, py] = apply(g.world, g.lx, g.ly + y);
        const long = y % (step * 5) === 0;
        ctx.beginPath();
        ctx.moveTo(long ? 0 : band / 2, py * sy);
        ctx.lineTo(band, py * sy);
        ctx.stroke();
        if (long) ctx.fillText(String(y), 1, py * sy - 2);
      }
    }
    layerSpace();
    ctx.lineWidth = screenPx;
    ctx.strokeStyle = "#00c8ff";
    for (const guide of p.guides) {
      ctx.beginPath();
      if (guide.axis === "x") {
        ctx.moveTo(g.lx + guide.at, g.ly);
        ctx.lineTo(g.lx + guide.at, g.ly + g.height);
      } else {
        ctx.moveTo(g.lx, g.ly + guide.at);
        ctx.lineTo(g.lx + g.width, g.ly + guide.at);
      }
      ctx.stroke();
    }
    const ants = (x: number, y: number, w: number, h: number, ellipse = false) => {
      for (const [color, offset] of [["#fff", 0], ["#000", 4]] as const) {
        ctx.save();
        ctx.lineWidth = screenPx;
        ctx.setLineDash([4 * screenPx, 4 * screenPx]);
        ctx.lineDashOffset = offset * screenPx;
        ctx.strokeStyle = color;
        ctx.beginPath();
        if (ellipse) ctx.ellipse(x + w / 2, y + h / 2, Math.max(0, w / 2), Math.max(0, h / 2), 0, 0, 7);
        else ctx.rect(x, y, w, h);
        ctx.stroke();
        ctx.restore();
      }
    };
    if (p.selection && !l.marquee) {
      const [x0, y0, x1, y1] = p.selection as [number, number, number, number];
      ants(g.lx + x0, g.ly + y0, x1 - x0, y1 - y0);
    }
    if (l.marquee) ants(g.lx + l.marquee.x, g.ly + l.marquee.y, l.marquee.width, l.marquee.height, p.shape === "ellipse");
    if (l.source) {
      const x = g.lx + l.source.x;
      const y = g.ly + l.source.y;
      const r = 8 * screenPx;
      ctx.lineWidth = 1.5 * screenPx;
      ctx.strokeStyle = "#ff9f0a";
      ctx.beginPath();
      ctx.moveTo(x - r, y);
      ctx.lineTo(x + r, y);
      ctx.moveTo(x, y - r);
      ctx.lineTo(x, y + r);
      ctx.stroke();
      ctx.beginPath();
      ctx.arc(x, y, r * 0.6, 0, 7);
      ctx.stroke();
    }
    if (l.lasso.length) {
      ctx.lineWidth = screenPx;
      ctx.strokeStyle = "#0a84ff";
      ctx.beginPath();
      l.lasso.forEach((q, i) => (i ? ctx.lineTo(g.lx + q.x, g.ly + q.y) : ctx.moveTo(g.lx + q.x, g.ly + q.y)));
      ctx.stroke();
    }
    const paper = canvas.background && /^#[0-9a-f]{6}$/i.test(canvas.background) ? rgbOf(canvas.background) : "255,255,255";
    // The engine keeps strokes inside the selection; the preview shows the same.
    const clipped = (clip: number[] | null, paint: () => void) => {
      ctx.save();
      if (clip) {
        ctx.beginPath();
        ctx.rect(g.lx + clip[0]!, g.ly + clip[1]!, clip[2]! - clip[0]!, clip[3]! - clip[1]!);
        ctx.clip();
      }
      paint();
      ctx.restore();
    };
    for (const q of l.pending) clipped(q.clip, () => dabs(ctx, q.pts.map(toParent), q.size, q.kind, q.erase ? paper : rgbOf(q.color), q.hardness, q.flow));
    const s = l.stroke;
    if (s && "samples" in s) {
      const erase = p.tool === "eraser";
      const color = p.tool === "quickmask" ? "#ff3b30" : p.color;
      if (p.tool === "brush" || erase) clipped(p.selection, () => dabs(ctx, s.preview.map(toParent), p.size, p.kind, erase ? paper : rgbOf(color), p.hardness, p.flow));
      else if (p.tool === "quickmask") dabs(ctx, s.preview.map(toParent), p.size, p.kind, rgbOf(color), p.hardness, 0.5);
      if (p.stabilizer > 0 && s.preview.length) {
        const last = s.preview.at(-1)!;
        const from = toParent(s.lag);
        const to = toParent(last);
        ctx.lineWidth = screenPx;
        ctx.strokeStyle = "#30d158";
        ctx.beginPath();
        ctx.moveTo(from.x, from.y);
        ctx.lineTo(to.x, to.y);
        ctx.stroke();
      }
    }
    if (l.pointer && CURSOR_TOOLS.includes(p.tool)) {
      const x = l.pointer.x + g.lx;
      const y = l.pointer.y + g.ly;
      const r = Math.max(0.5, p.size / 2);
      ctx.lineWidth = screenPx;
      ctx.strokeStyle = "rgba(255,255,255,.9)";
      ctx.beginPath();
      ctx.arc(x, y, r + screenPx, 0, 7);
      ctx.stroke();
      ctx.strokeStyle = "rgba(0,0,0,.9)";
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 7);
      ctx.stroke();
    }
  }, [geometry, canvas.width, canvas.height, canvas.background, dpr]);

  // A committed stroke leaves the overlay once its render is visible.
  useEffect(() => {
    const shown = renderedDoc ? commits.current.get(renderedDoc) : undefined;
    if (shown !== undefined) live.current.pending = live.current.pending.filter((q) => !(q.done && q.order <= shown));
    draw();
  }, [renderedDoc, draw]);
  // Undo, redo or a reload replaces the document with one no stroke produced.
  useEffect(() => {
    if (doc && !commits.current.has(doc) && inflight.current === 0) live.current.pending = live.current.pending.filter((q) => !q.done);
  }, [doc]);
  // The selection outline belongs to one layer.
  useEffect(() => {
    usePaint.getState().set({ selection: null });
    live.current.source = null;
  }, [layer?.id, pageId]);
  useEffect(() => {
    draw();
  }, [draw, backingW, backingH, paint.tool, paint.size, paint.rulers, paint.guides, paint.shape, paint.selection]);

  const toLayer = (event: { clientX: number; clientY: number }): Pt | null => {
    const g = geometry();
    const el = overlay.current;
    if (!g || !el) return null;
    const rect = el.getBoundingClientRect();
    if (!rect.width || !rect.height) return null;
    const gx = ((event.clientX - rect.left) / rect.width) * canvas.width;
    const gy = ((event.clientY - rect.top) / rect.height) * canvas.height;
    const [x, y] = apply(g.inverse, gx, gy);
    return { x: x - g.lx, y: y - g.ly, gx, gy };
  };
  const snap = (q: Pt): Pt => {
    const g = geometry();
    const s = usePaint.getState();
    if (!g || !s.snap) return q;
    let { x, y } = q;
    if (s.grid > 0) {
      x = Math.round(x / s.grid) * s.grid;
      y = Math.round(y / s.grid) * s.grid;
    }
    for (const guide of s.guides) {
      if (guide.axis === "x" && Math.abs(q.x - guide.at) <= SNAP) x = guide.at;
      if (guide.axis === "y" && Math.abs(q.y - guide.at) <= SNAP) y = guide.at;
    }
    const [gx, gy] = apply(g.world, x + g.lx, y + g.ly);
    return { x, y, gx, gy };
  };

  // Pixels change only through the shared Rust engine; commits run in order and
  // never block input. The first edit on a page without a paint layer adds one.
  const call = (action: string, args: Record<string, unknown>, ghost?: Omit<Ghost, "done" | "order">) => {
    const entry: Ghost | null = ghost ? { ...ghost, done: false, order: Infinity } : null;
    if (entry) live.current.pending.push(entry);
    inflight.current += 1;
    usePaint.getState().set({ busy: true });
    const run = async () => {
      try {
        let id = layer?.id ?? null;
        if (!id || !collectRasters(useStudio.getState().doc, useStudio.getState().pageId).some((n) => n.id === id)) {
          id = collectRasters(useStudio.getState().doc, useStudio.getState().pageId)[0]?.id ?? (await addPaintLayer());
        }
        if (!id) throw new Error("Could not add a paint layer to this page.");
        const current = useStudio.getState();
        if (!current.doc) return;
        const body = await postJson<{ document: Doc; result?: { bounds?: number[] | null; selection?: { bounds?: number[] | null } } }>("/api/raster", { document: current.doc, page: current.pageId, id, action, args });
        if (useStudio.getState().doc !== current.doc) throw new Error("The project changed while this edit was processing. Try the edit again.");
        const order = nextOrder.current++;
        commits.current.set(body.document, order);
        if (entry) {
          entry.done = true;
          entry.order = order;
        }
        current.replaceDoc(body.document);
        const bounds = body.result && ("bounds" in body.result ? body.result.bounds : body.result.selection?.bounds);
        if ((action.startsWith("select-") && action !== "select-clear") || action === "quickmask") usePaint.getState().set({ selection: bounds ?? null });
        if (action === "select-clear") usePaint.getState().set({ selection: null });
        usePaint.getState().set({ status: STATUS[action] ?? "Done." });
        // A successful edit supersedes an earlier error (e.g. "Choose a clone source first").
        if (useStudio.getState().message?.error) useStudio.setState({ message: null });
      } catch (e) {
        usePaint.getState().set({ status: e instanceof Error ? e.message : "The paint engine rejected this edit." });
        useStudio.getState().notify(e instanceof Error ? e.message : "The paint engine rejected this edit.", true);
        if (entry) live.current.pending = live.current.pending.filter((q) => q !== entry);
      } finally {
        inflight.current -= 1;
        if (inflight.current === 0) usePaint.getState().set({ busy: false });
        draw();
      }
    };
    queue.current = queue.current.then(run);
    return queue.current;
  };

  const down = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (onPanStart(event)) return;
    if (event.button !== 0) return;
    const s = usePaint.getState();
    let q = toLayer(event);
    if (!q) return;
    const el = overlay.current!;
    const rect = el.getBoundingClientRect();
    if (s.rulers && (event.clientY - rect.top < 14 || event.clientX - rect.left < 14)) {
      // Clicking a ruler drops a guide: top ruler -> horizontal, left ruler -> vertical.
      s.set({ guides: [...s.guides, event.clientY - rect.top < 14 ? { axis: "y", at: Math.round(q.y) } : { axis: "x", at: Math.round(q.x) }] });
      return;
    }
    q = snap(q);
    event.preventDefault();
    el.setPointerCapture(event.pointerId);
    const l = live.current;
    // Alt-click with clone or heal picks the source, as in other editors.
    if (s.tool === "source" || ((s.tool === "clone" || s.tool === "heal") && event.altKey)) {
      l.source = { x: q.x, y: q.y };
      void call("set-clone-source", { x: q.x, y: q.y });
      draw();
      return;
    }
    if ((s.tool === "clone" || s.tool === "heal") && !l.source) {
      s.set({ status: "Alt-click (or use the Clone source tool) to choose where to copy from first." });
      useStudio.getState().notify("Choose a clone source first: Alt-click the area to copy from.", true);
      return;
    }
    if (s.tool === "wand") {
      void call("select-wand", { x: Math.max(0, Math.round(q.x)), y: Math.max(0, Math.round(q.y)), tolerance: s.tolerance, sample: s.sample, gradient: s.gradient, mode: s.mode });
      return;
    }
    if (s.tool === "lasso") {
      l.lasso = [q];
      l.stroke = { lasso: true };
    } else if (s.tool === "marquee") {
      l.stroke = { marquee: q };
      l.marquee = { x: q.x, y: q.y, width: 0, height: 0 };
    } else l.stroke = { samples: [{ x: q.x, y: q.y, pressure: event.pressure || 0.5 }], preview: [q], lag: q };
    draw();
  };

  const move = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const l = live.current;
    const events = l.stroke && "samples" in l.stroke && typeof event.nativeEvent.getCoalescedEvents === "function" ? event.nativeEvent.getCoalescedEvents() : [];
    for (const sample of events.length ? events : [event]) {
      const raw = toLayer(sample);
      if (!raw) return;
      const q = snap(raw);
      l.pointer = q;
      const s = l.stroke;
      if (s && "samples" in s) {
        s.samples.push({ x: q.x, y: q.y, pressure: sample.pressure || 0.5 });
        const w = usePaint.getState().stabilizer;
        const gx = s.lag.gx * w + q.gx * (1 - w);
        const gy = s.lag.gy * w + q.gy * (1 - w);
        s.lag = { ...q, gx, gy };
        s.preview.push(s.lag);
      } else if (s && "lasso" in s) l.lasso.push(q);
      else if (s && "marquee" in s) {
        const m = s.marquee;
        let w = q.x - m.x;
        let h = q.y - m.y;
        if (event.shiftKey) {
          const side = Math.max(Math.abs(w), Math.abs(h));
          w = Math.sign(w || 1) * side;
          h = Math.sign(h || 1) * side;
        }
        l.marquee = { x: w < 0 ? m.x + w : m.x, y: h < 0 ? m.y + h : m.y, width: Math.abs(w), height: Math.abs(h) };
      }
    }
    draw();
  };

  const up = () => {
    const l = live.current;
    const s = l.stroke;
    l.stroke = null;
    if (!s) return;
    const st = usePaint.getState();
    if ("samples" in s) {
      const action = STROKE_ACTIONS[st.tool];
      if (action) {
        const brush = { kind: st.kind, size: st.size, hardness: st.hardness, flow: st.flow, smoothing: st.stabilizer };
        const visible = st.tool === "brush" || st.tool === "eraser";
        void call(
          action,
          { brush, color: st.color, seed: 0, samples: s.samples, blend: st.tool === "eraser" ? "erase" : "normal", erase: st.tool === "quickmask" && st.mode === "subtract" },
          visible ? { pts: s.preview.slice(), size: st.size, kind: st.kind, color: st.color, erase: st.tool === "eraser", hardness: st.hardness, flow: st.flow, clip: st.selection } : undefined,
        );
      }
    } else if ("lasso" in s && l.lasso.length >= 3) {
      void call("select-lasso", { points: l.lasso.map((q) => [q.x, q.y]), mode: st.mode });
    } else if ("marquee" in s && l.marquee && l.marquee.width > 0 && l.marquee.height > 0) {
      void call("select-marquee", { ...l.marquee, shape: st.shape, mode: st.mode });
    }
    l.lasso = [];
    l.marquee = null;
    draw();
  };

  // Photoshop-style keys while painting: [ and ] resize the brush, Ctrl+D deselects.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (event.defaultPrevented || target?.closest("input, textarea, select, [contenteditable=true]")) return;
      const s = usePaint.getState();
      if (!event.ctrlKey && !event.metaKey && (event.key === "[" || event.key === "]")) {
        event.preventDefault();
        const step = s.size < 10 ? 1 : s.size < 50 ? 5 : 10;
        s.set({ size: Math.max(1, Math.min(200, s.size + (event.key === "]" ? step : -step))) });
      } else if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "d") {
        event.preventDefault();
        if (s.selection && layer) void call("select-clear", {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <canvas
      ref={overlay}
      width={backingW}
      height={backingH}
      aria-label="Raster painting surface"
      aria-busy={paint.busy}
      className="absolute inset-0 h-full w-full touch-none"
      style={{ cursor: CURSOR_TOOLS.includes(paint.tool) ? "none" : "crosshair" }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
      onPointerLeave={() => {
        live.current.pointer = null;
        draw();
      }}
    />
  );
}

const STATUS: Record<string, string> = {
  stroke: "Stroke applied. Undo (Ctrl+Z) reverts it; Save keeps it.",
  clone: "Cloned. Undo (Ctrl+Z) reverts it.",
  heal: "Healed. Undo (Ctrl+Z) reverts it.",
  quickmask: "Quick mask painted into the selection. Brush strokes now stay inside it; Ctrl+D deselects.",
  "set-clone-source": "Clone source set. Paint with Clone or Heal to copy from it.",
  "select-marquee": "Selection made. Brush strokes stay inside it; Ctrl+D deselects.",
  "select-lasso": "Selection made. Brush strokes stay inside it; Ctrl+D deselects.",
  "select-wand": "Selection made. Brush strokes stay inside it; Ctrl+D deselects.",
  "select-clear": "Selection cleared.",
  "select-modify": "Selection changed.",
  "select-refine": "Selection snapped to image edges.",
  lift: "Pixels copied to a new layer.",
};
