import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, postJson, type Doc, type SceneNode } from "@/lib/api";
import { apply, findNode, invert, nodeWorldTransform, pageOf } from "@/lib/model";
import { useStudio } from "@/store";
import { usePaint } from "./rasterStore";

export type RasterNode = SceneNode & { x?: number; y?: number; width: number; height: number };
type Pt = { x: number; y: number; px: number; py: number };

function collect(nodes: SceneNode[] | undefined, out: RasterNode[]): RasterNode[] {
  for (const node of nodes ?? []) {
    if (node.kind === "raster") out.push(node as RasterNode);
    collect(node.children, out);
  }
  return out;
}

export function useRasterLayers(): RasterNode[] {
  const { doc, pageId } = useStudio();
  return useMemo(() => {
    const out: RasterNode[] = [];
    if (!doc || doc.version < 6) return out;
    for (const layer of pageOf(doc, pageId)?.layers ?? []) collect(layer.nodes, out);
    return out;
  }, [doc, pageId]);
}

const SNAP = 6;
const CURSOR_TOOLS = ["brush", "eraser", "clone", "heal", "quickmask"];

/** Soft-edged dabs along a path: what you see while painting matches the committed look. */
function dabs(ctx: CanvasRenderingContext2D, pts: { px: number; py: number }[], diameter: number, kind: string, color: string, hardness: number, flow: number, erase: boolean) {
  const r = Math.max(0.5, diameter / 2);
  const pixel = kind === "pixel";
  const step = pixel ? 1 : Math.max(1, diameter * 0.1);
  const rgb = erase ? "255,255,255" : color.startsWith("#") && color.length >= 7 ? [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16)).join(",") : "0,0,0";
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
  let carry = 0;
  stamp(pts[0]!.px, pts[0]!.py);
  for (let i = 1; i < pts.length; i++) {
    const a0 = pts[i - 1]!;
    const b0 = pts[i]!;
    const len = Math.hypot(b0.px - a0.px, b0.py - a0.py);
    for (let d = step - carry; d <= len; d += step) stamp(a0.px + ((b0.px - a0.px) * d) / (len || 1), a0.py + ((b0.py - a0.py) * d) / (len || 1));
    carry = (carry + len) % step;
  }
}

function transformedDabs(
  ctx: CanvasRenderingContext2D,
  pts: Pt[],
  diameter: number,
  kind: string,
  color: string,
  hardness: number,
  flow: number,
  erase: boolean,
  g: { sx: number; sy: number; inverse: [number, number, number, number, number, number]; world: [number, number, number, number, number, number] },
) {
  const [a, b, c, d, e, f] = g.world;
  ctx.save();
  ctx.setTransform(g.sx * a, g.sy * b, g.sx * c, g.sy * d, g.sx * e, g.sy * f);
  dabs(
    ctx,
    pts.map((p) => {
      const [x, y] = apply(g.inverse, p.px / g.sx, p.py / g.sy);
      return { px: x, py: y };
    }),
    diameter,
    kind,
    color,
    hardness,
    flow,
    erase,
  );
  ctx.restore();
}

export function RasterStage() {
  const { doc, pageId, replaceDoc } = useStudio();
  const layers = useRasterLayers();
  const paint = usePaint();
  const layer = layers.find((n) => n.id === paint.layerId) ?? layers[0];

  const stage = useRef<HTMLDivElement>(null);
  const view = useRef<HTMLDivElement>(null);
  const image = useRef<HTMLImageElement>(null);
  const overlay = useRef<HTMLCanvasElement>(null);
  const navigator = useRef<HTMLCanvasElement>(null);
  const generation = useRef(0);
  const [loaded, setLoaded] = useState(0);
  const [url, setUrl] = useState<string | null>(null);

  // Mutable interaction state; the overlay is redrawn imperatively.
  const live = useRef<{
    pointer: Pt | null;
    stroke: null | { samples: { x: number; y: number; pressure: number }[]; preview: Pt[]; last: Pt; lag: { px: number; py: number } } | { lasso: true } | { marquee: Pt };
    lasso: Pt[];
    selection: { x: number; y: number; width: number; height: number } | null;
    source: { x: number; y: number } | null;
    /** Strokes already released but not yet confirmed by the engine; drawn so the mark never disappears. */
    pending: { preview: Pt[]; size: number; kind: string; color: string; erase: boolean; hardness: number; flow: number; done: boolean; gen: number }[];
  }>({ pointer: null, stroke: null, lasso: [], selection: null, source: null, pending: [] });
  /** Commits run strictly in order; input is never blocked while one is in flight. */
  const queue = useRef<Promise<void>>(Promise.resolve());
  const inflight = useRef(0);
  const urlGen = useRef(0);

  // The preview is a proxy of the page rendered by the Rust engine.
  useEffect(() => {
    if (!doc || !layer) return;
    const mine = ++generation.current;
    void api
      .renderPng(doc as Doc, pageId, 1024)
      .then((blob) => {
        if (mine !== generation.current) return;
        urlGen.current = mine;
        setUrl((old) => {
          if (old) URL.revokeObjectURL(old);
          return URL.createObjectURL(blob);
        });
      })
      .catch((e) => usePaint.getState().set({ status: e instanceof Error ? e.message : "Preview failed." }));
  }, [doc, pageId, layer?.id]); // eslint-disable-line react-hooks/exhaustive-deps

  const geometry = useCallback(() => {
    const img = image.current;
    const canvas = doc ? pageOf(doc as Doc, pageId)?.canvas : undefined;
    if (!layer || !canvas || !img?.naturalWidth) return null;
    const found = findNode(pageOf(doc as Doc, pageId), layer.id);
    // A raster node's own transform affects both where its pixels appear and
    // which local pixel receives a pointer sample.
    const world = found && nodeWorldTransform(found);
    const inverse = world && invert(world);
    if (!world || !inverse) return null;
    return { node: layer, sx: img.naturalWidth / canvas.width, sy: img.naturalHeight / canvas.height, lx: layer.x ?? 0, ly: layer.y ?? 0, world, inverse };
  }, [doc, pageId, layer]);

  const draw = useCallback(() => {
    const el = overlay.current;
    const ctx = el?.getContext("2d");
    const g = geometry();
    if (!el || !ctx) return;
    ctx.clearRect(0, 0, el.width, el.height);
    if (!g) return;
    const p = usePaint.getState();
    const l = live.current;
    if (p.rulers) {
      ctx.save();
      ctx.font = "10px sans-serif";
      ctx.lineWidth = 1;
      ctx.fillStyle = "rgba(255,255,255,.85)";
      ctx.fillRect(0, 0, el.width, 12);
      ctx.fillRect(0, 0, 12, el.height);
      ctx.fillStyle = "#000";
      ctx.strokeStyle = "#000";
      for (let x = 0; x <= g.node.width; x += 10) {
        const px = (g.lx + x) * g.sx;
        const long = x % 50 === 0;
        ctx.beginPath();
        ctx.moveTo(px, long ? 0 : 7);
        ctx.lineTo(px, 12);
        ctx.stroke();
        if (long) ctx.fillText(String(x), px + 2, 9);
      }
      for (let y = 0; y <= g.node.height; y += 10) {
        const py = (g.ly + y) * g.sy;
        const long = y % 50 === 0;
        ctx.beginPath();
        ctx.moveTo(long ? 0 : 7, py);
        ctx.lineTo(12, py);
        ctx.stroke();
        if (long) ctx.fillText(String(y), 1, py - 2);
      }
      ctx.restore();
    }
    ctx.save();
    ctx.strokeStyle = "#00c8ff";
    ctx.lineWidth = 1;
    for (const guide of p.guides) {
      ctx.beginPath();
      if (guide.axis === "x") {
        const px = (g.lx + guide.at) * g.sx;
        ctx.moveTo(px, 0);
        ctx.lineTo(px, el.height);
      } else {
        const py = (g.ly + guide.at) * g.sy;
        ctx.moveTo(0, py);
        ctx.lineTo(el.width, py);
      }
      ctx.stroke();
    }
    ctx.restore();
    if (p.tool === "quickmask") {
      ctx.fillStyle = "rgba(255,0,0,.3)";
      ctx.fillRect(g.lx * g.sx, g.ly * g.sy, g.node.width * g.sx, g.node.height * g.sy);
    }
    if (l.selection) {
      const s = l.selection;
      ctx.save();
      ctx.setLineDash([4, 3]);
      ctx.strokeStyle = "#0a84ff";
      ctx.strokeRect((g.lx + s.x) * g.sx, (g.ly + s.y) * g.sy, s.width * g.sx, s.height * g.sy);
      ctx.restore();
    }
    if (l.source) {
      const x = (g.lx + l.source.x) * g.sx;
      const y = (g.ly + l.source.y) * g.sy;
      ctx.strokeStyle = "#ff9f0a";
      ctx.beginPath();
      ctx.moveTo(x - 8, y);
      ctx.lineTo(x + 8, y);
      ctx.moveTo(x, y - 8);
      ctx.lineTo(x, y + 8);
      ctx.stroke();
    }
    if (l.lasso.length) {
      ctx.strokeStyle = "#0a84ff";
      ctx.beginPath();
      l.lasso.forEach((q, i) => (i ? ctx.lineTo(q.px, q.py) : ctx.moveTo(q.px, q.py)));
      ctx.stroke();
    }
    for (const q of l.pending) if (q.preview.length) transformedDabs(ctx, q.preview, q.size, q.kind, q.color, q.hardness, q.flow, q.erase, g);
    const s = l.stroke;
    if (s && "samples" in s) {
      if (s.preview.length) transformedDabs(ctx, s.preview, p.size, p.kind, p.color, p.hardness, p.flow, p.tool === "eraser", g);
      // Stabilizer feedback: the engine smooths with this lag, so show where the dab will land.
      ctx.strokeStyle = "#30d158";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(s.lag.px, s.lag.py);
      ctx.lineTo(s.last.px, s.last.py);
      ctx.stroke();
    }
    if (l.pointer && !l.stroke && CURSOR_TOOLS.includes(p.tool)) {
      const [a, b, c, d, e, f] = g.world;
      ctx.save();
      ctx.setTransform(g.sx * a, g.sy * b, g.sx * c, g.sy * d, g.sx * e, g.sy * f);
      const x = l.pointer.x + g.lx;
      const y = l.pointer.y + g.ly;
      const r = Math.max(0.5, p.size / 2);
      ctx.lineWidth = 1;
      ctx.strokeStyle = "#fff";
      ctx.beginPath();
      ctx.arc(x, y, r + 1, 0, 7);
      ctx.stroke();
      ctx.strokeStyle = "#000";
      ctx.beginPath();
      ctx.arc(x, y, r, 0, 7);
      ctx.stroke();
      ctx.restore();
    }
  }, [geometry]);

  const drawNavigator = useCallback(() => {
    const nav = navigator.current;
    const img = image.current;
    const box = stage.current;
    const frame = view.current;
    const nctx = nav?.getContext("2d");
    if (!nav || !nctx) return;
    nctx.clearRect(0, 0, nav.width, nav.height);
    if (!img?.naturalWidth || !box || !frame) return;
    const { zoom, panX, panY } = usePaint.getState();
    const scale = Math.min(nav.width / img.naturalWidth, nav.height / img.naturalHeight);
    const w = img.naturalWidth * scale;
    const h = img.naturalHeight * scale;
    nctx.drawImage(img, 0, 0, w, h);
    const k = w / (frame.offsetWidth || w);
    const vw = Math.min(w, (box.clientWidth / zoom) * k);
    const vh = Math.min(h, (box.clientHeight / zoom) * k);
    const cx = w / 2 - (panX / zoom) * k;
    const cy = h / 2 - (panY / zoom) * k;
    nctx.strokeStyle = "#ff375f";
    nctx.lineWidth = 2;
    nctx.strokeRect(cx - vw / 2, cy - vh / 2, vw, vh);
  }, []);

  // Keep the overlay sized to the preview and redrawn when settings change.
  useEffect(() => {
    const img = image.current;
    const el = overlay.current;
    if (img?.naturalWidth && el) {
      el.width = img.naturalWidth;
      el.height = img.naturalHeight;
    }
    draw();
    drawNavigator();
  }, [loaded, draw, drawNavigator, paint.rulers, paint.guides, paint.tool, paint.size, paint.zoom, paint.rotate, paint.panX, paint.panY, layer?.id]);

  // Client point -> layer pixel, through zoom and rotation. Layout size (not the
  // rotated bounding box) gives the scale.
  const toLayer = (event: React.PointerEvent): Pt | null => {
    const g = geometry();
    const el = overlay.current;
    if (!g || !el || !el.offsetWidth) return null;
    const { zoom, rotate } = usePaint.getState();
    const rect = el.getBoundingClientRect();
    const a = (rotate * Math.PI) / 180;
    const dx = event.clientX - (rect.left + rect.width / 2);
    const dy = event.clientY - (rect.top + rect.height / 2);
    const ux = (dx * Math.cos(a) + dy * Math.sin(a)) / zoom;
    const uy = (-dx * Math.sin(a) + dy * Math.cos(a)) / zoom;
    const px = (ux * el.width) / el.offsetWidth + el.width / 2;
    const py = (uy * el.height) / el.offsetHeight + el.height / 2;
    const [parentX, parentY] = apply(g.inverse, px / g.sx, py / g.sy);
    return { x: parentX - g.lx, y: parentY - g.ly, px, py };
  };
  const snap = (p: Pt): Pt => {
    const g = geometry();
    const s = usePaint.getState();
    if (!g || !s.snap) return p;
    let { x, y } = p;
    if (s.grid > 0) {
      x = Math.round(x / s.grid) * s.grid;
      y = Math.round(y / s.grid) * s.grid;
    }
    for (const guide of s.guides) {
      if (guide.axis === "x" && Math.abs(p.x - guide.at) <= SNAP) x = guide.at;
      if (guide.axis === "y" && Math.abs(p.y - guide.at) <= SNAP) y = guide.at;
    }
    const [pageX, pageY] = apply(g.world, x + g.lx, y + g.ly);
    return { x, y, px: pageX * g.sx, py: pageY * g.sy };
  };

  const brush = () => {
    const s = usePaint.getState();
    return { kind: s.kind, size: s.size, hardness: s.hardness, flow: s.flow, smoothing: s.stabilizer };
  };

  // Pixels change only through the shared Rust engine.
  const call = (action: string, args: Record<string, unknown>, ghost?: { preview: Pt[]; size: number; kind: string; color: string; erase: boolean; hardness: number; flow: number }) => {
    const entry = ghost ? { ...ghost, done: false, gen: Infinity } : null;
    if (entry) live.current.pending.push(entry);
    inflight.current += 1;
    usePaint.getState().set({ busy: true });
    const run = async () => {
      const current = useStudio.getState();
      if (!current.doc || !layer) return;
      try {
        const body = await postJson<{ document: Doc }>("/api/raster", { document: current.doc, page: current.pageId, id: layer.id, action, args });
        replaceDoc(body.document);
        usePaint.getState().set({ status: `${action}: done. Save .pen to keep it; Undo restores the previous state.` });
      } catch (e) {
        usePaint.getState().set({ status: e instanceof Error ? e.message : "Request rejected." });
        if (entry) live.current.pending = live.current.pending.filter((q) => q !== entry);
      } finally {
        if (entry) {
          entry.done = true;
          entry.gen = generation.current + 1; // the render of the committed document
        }
        inflight.current -= 1;
        if (inflight.current === 0) usePaint.getState().set({ busy: false });
      }
    };
    queue.current = queue.current.then(run);
    return queue.current;
  };

  const down = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const s = usePaint.getState();
    let p = toLayer(event);
    if (!p) return;
    if (s.rulers && (p.py < 12 || p.px < 12)) {
      // A click on a ruler drops a guide: top ruler -> horizontal, left ruler -> vertical.
      s.set({ guides: [...s.guides, p.py < 12 ? { axis: "y", at: Math.round(p.y) } : { axis: "x", at: Math.round(p.x) }] });
      return;
    }
    p = snap(p);
    event.currentTarget.setPointerCapture(event.pointerId);
    const l = live.current;
    if (s.tool === "source") {
      l.source = { x: p.x, y: p.y };
      void call("set-clone-source", { x: p.x, y: p.y });
      draw();
      return;
    }
    if (s.tool === "wand") {
      void call("select-wand", { x: Math.max(0, Math.round(p.x)), y: Math.max(0, Math.round(p.y)), tolerance: s.tolerance, mode: s.mode });
      return;
    }
    if (s.tool === "lasso") {
      l.lasso = [p];
      l.stroke = { lasso: true };
    } else if (s.tool === "marquee") l.stroke = { marquee: p };
    else l.stroke = { samples: [{ x: p.x, y: p.y, pressure: event.pressure || 0.5 }], preview: [p], last: p, lag: p };
    draw();
  };
  const move = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const raw = toLayer(event);
    if (!raw) return;
    const p = snap(raw);
    const l = live.current;
    l.pointer = p;
    const s = l.stroke;
    if (s && "samples" in s) {
      s.samples.push({ x: p.x, y: p.y, pressure: event.pressure || 0.5 });
      const w = usePaint.getState().stabilizer;
      s.lag = { px: s.lag.px * w + p.px * (1 - w), py: s.lag.py * w + p.py * (1 - w) };
      s.preview.push({ x: p.x, y: p.y, px: s.lag.px, py: s.lag.py });
      s.last = p;
    } else if (s && "lasso" in s) l.lasso.push(p);
    else if (s && "marquee" in s) {
      const m = s.marquee;
      l.selection = { x: Math.min(m.x, p.x), y: Math.min(m.y, p.y), width: Math.abs(p.x - m.x), height: Math.abs(p.y - m.y) };
    }
    draw();
  };
  const up = async () => {
    const l = live.current;
    const s = l.stroke;
    l.stroke = null;
    if (!s) return;
    const st = usePaint.getState();
    const common = { brush: brush(), color: st.color, seed: 0 };
    if ("samples" in s) {
      const action = ({ brush: "stroke", eraser: "stroke", clone: "clone", heal: "heal", quickmask: "quickmask" } as Record<string, string>)[st.tool];
      if (action) {
        const preview = s.preview.slice();
        if (st.stabilizer > 0 && s.samples.length > 1) preview.push(s.last);
        void call(
          action,
          { ...common, samples: s.samples, blend: st.tool === "eraser" ? "erase" : "normal", erase: st.tool === "quickmask" && st.mode === "subtract" },
          { preview, size: st.size, kind: st.kind, color: st.color, erase: st.tool === "eraser", hardness: st.hardness, flow: st.flow },
        );
      }
    } else if ("lasso" in s && l.lasso.length >= 3) {
      void call("select-lasso", { points: l.lasso.map((q) => [q.x, q.y]), mode: st.mode });
    } else if ("marquee" in s && l.selection && l.selection.width > 0 && l.selection.height > 0) {
      void call("select-marquee", { ...l.selection, shape: st.shape, mode: st.mode });
    }
    l.lasso = [];
    l.selection = null;
    draw();
  };

  const centerOn = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    const u = (event.clientX - rect.left) / rect.width - 0.5;
    const v = (event.clientY - rect.top) / rect.height - 0.5;
    const frame = view.current;
    const { zoom } = usePaint.getState();
    // Center the clicked spot: pan opposite to its offset from the page center.
    usePaint.getState().set({ panX: -u * (frame?.offsetWidth ?? 0) * zoom, panY: -v * (frame?.offsetHeight ?? 0) * zoom });
  };

  if (!layer) {
    return (
      <main className="flex min-w-0 flex-1 items-center justify-center p-6 text-sm text-muted-foreground">
        No paint layer on this page yet. Use "Add paint layer" in the right panel to start painting.
      </main>
    );
  }
  return (
    <main className="relative flex min-w-0 flex-1 flex-col bg-muted/30">
      <div ref={stage} className="flex min-h-0 flex-1 items-center justify-center overflow-hidden p-4" aria-busy={paint.busy}>
        <div
          ref={view}
          className="relative inline-block max-h-full max-w-full"
          style={{ transform: `translate(${paint.panX}px, ${paint.panY}px) rotate(${paint.rotate}deg) scale(${paint.zoom})` }}
        >
          {url && (
            <img
              ref={image}
              src={url}
              alt="Raster page preview"
              draggable={false}
              className="block max-h-[calc(100vh-8rem)] max-w-full"
              onLoad={() => {
                live.current.pending = live.current.pending.filter((q) => !(q.done && urlGen.current >= q.gen));
                setLoaded((n) => n + 1);
              }}
            />
          )}
          <canvas
            ref={overlay}
            aria-label="Raster painting surface"
            className="absolute inset-0 h-full w-full cursor-crosshair touch-none"
            onPointerDown={down}
            onPointerMove={move}
            onPointerUp={() => void up()}
            onPointerLeave={() => {
              live.current.pointer = null;
              draw();
            }}
          />
        </div>
      </div>
      <canvas
        ref={navigator}
        width={160}
        height={100}
        aria-label="Navigator: click to center the view"
        onPointerDown={centerOn}
        className="absolute bottom-3 right-3 rounded border bg-card shadow"
      />
      <p role="status" className="shrink-0 border-t bg-card px-3 py-1 text-xs text-muted-foreground">
        {paint.status} Strokes are sent to the Rust engine on release; outlines are overlays only.
      </p>
    </main>
  );
}
