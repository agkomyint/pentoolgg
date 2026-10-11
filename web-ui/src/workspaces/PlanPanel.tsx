import { useEffect } from "react";
import { create } from "zustand";
import { Button } from "@/components/ui/button";
import { postJson, type Doc } from "@/lib/api";
import { useStudio } from "@/store";
import { useRasterLayers } from "./RasterStage";

// Editable plan authoring for agent-directed painting. Every pass is plain data
// that becomes a v0.13.0 paint plan; the preview and the commit are both the
// authoritative Rust planner (POST /api/paint/plan), so nothing is estimated here.

type Pass = {
  id: string;
  mode: "path" | "pressure";
  path: string;
  samples: string; // one "x,y,pressure" per line
  size: number;
  hardness: number;
  color: string;
  region: string; // "x,y,w,h" or empty
  protect: string; // one "x,y,w,h" per line
  palette: string; // space/comma separated hex colors
  media: string; // material kind, or empty for the default brush
};

type PassReport = {
  id: string;
  strokes?: number;
  dabs?: number;
  changed_pixels: number;
  changed_tiles: number;
  changed_bounds: [number, number, number, number] | null;
  diagnostics?: { warnings?: { basis: string; code: string; message: string }[]; measured?: Record<string, number> };
};
type Report = {
  plan_hash: string;
  totals: { passes: number; dabs: number; changed_pixels: number; changed_tiles: number };
  passes: PassReport[];
};

type Comparison = {
  size: [number, number];
  reference_hash: string;
  metrics: Record<string, unknown>;
  heatmap_png_base64?: string;
};

type State = {
  passes: Pass[];
  report: Report | null;
  preview: Doc | null;
  score: { assessment?: { trend: string; basis: string }; before?: { score: number }; after?: { score: number } } | null;
  goal: string;
  sample: { x: number; y: number; radius: number };
  reference: { name: string; base64: string } | null;
  comparison: Comparison | null;
  kinds: string[];
  crop: { rect: string; png: string; report: string } | null;
  field: { json: string; vectors: [number, number, number, number][]; summary: string };
  busy: boolean;
  status: string;
  set: (patch: Partial<State>) => void;
};

const blank = (n: number): Pass => ({
  id: `pass-${n}`,
  mode: "path",
  path: "M20 40 L200 40",
  samples: "20,80,0.3\n110,70,0.9\n200,80,0.4",
  size: 12,
  hardness: 0.8,
  color: "#3070c0",
  region: "",
  protect: "",
  palette: "",
  media: "",
});

const useDraft = create<State>((set) => ({
  passes: [blank(1)],
  report: null,
  preview: null,
  score: null,
  goal: '{"coverage":[0.01,0.5]}',
  sample: { x: 0, y: 0, radius: 2 },
  reference: null,
  comparison: null,
  kinds: [],
  crop: null,
  field: { json: '{"source":{"kind":"layer"},"method":"structure-tensor","scale":12}', vectors: [], summary: "" },
  busy: false,
  status: "",
  set: (patch) => set(patch),
}));

const nums = (text: string): number[] | null => {
  const parts = text.split(/[\s,]+/).filter(Boolean).map(Number);
  return parts.every(Number.isFinite) ? parts : null;
};

function rectOf(text: string): [number, number, number, number] | null {
  return rect(text);
}

function rect(text: string): [number, number, number, number] | null {
  const v = nums(text);
  return v && v.length === 4 ? [v[0]!, v[1]!, v[2]!, v[3]!] : null;
}

function buildPlan(passes: Pass[], target: string): Record<string, unknown> {
  return {
    schema: 1,
    engine: 1,
    id: "studio-plan",
    passes: passes.map((p) => {
      const brush: Record<string, unknown> = { size: p.size, hardness: p.hardness, spacing: 0.1 };
      if (p.media) brush.media = { kind: p.media };
      const out: Record<string, unknown> = { id: p.id, target };
      if (p.mode === "path") {
        out.action = "stroke-path";
        out.args = { path: p.path, brush, color: p.color };
      } else {
        out.action = "stroke";
        const samples = p.samples
          .split("\n")
          .map((line) => nums(line))
          .filter((v): v is number[] => !!v && v.length >= 2)
          .map((v) => ({ x: v[0]!, y: v[1]!, pressure: v[2] ?? 1 }));
        out.args = { samples, brush, color: p.color };
      }
      const region = rect(p.region);
      if (region) out.region = region;
      const protect = p.protect.split("\n").map(rect).filter((r): r is [number, number, number, number] => !!r);
      if (protect.length) out.protect = protect;
      const palette = p.palette.split(/[\s,]+/).filter((c) => /^#[0-9a-fA-F]{6}$/.test(c));
      if (palette.length) out.palette = palette;
      return out;
    }),
  };
}

function Overlay({
  passes,
  report,
  size,
  vectors,
}: {
  passes: Pass[];
  report: Report | null;
  size: { w: number; h: number };
  vectors: [number, number, number, number][];
}) {
  const s = Math.min(1, 240 / Math.max(size.w, size.h));
  return (
    <svg width={size.w * s} height={size.h * s} viewBox={`0 0 ${size.w} ${size.h}`} className="rounded border bg-card" aria-label="Plan overlay">
      {passes.map((p) => {
        const r = rect(p.region);
        return r && <rect key={`r-${p.id}`} x={r[0]} y={r[1]} width={r[2]} height={r[3]} fill="none" stroke="#3b82f6" strokeDasharray="4 3" />;
      })}
      {passes.flatMap((p) =>
        p.protect
          .split("\n")
          .map(rect)
          .map((r, i) => r && <rect key={`p-${p.id}-${i}`} x={r[0]} y={r[1]} width={r[2]} height={r[3]} fill="#ef444433" stroke="#ef4444" />),
      )}
      {vectors.map(([x, y, vx, vy], i) => (
        <line key={`f-${i}`} x1={x} y1={y} x2={x + (vx / 32767) * 10} y2={y + (vy / 32767) * 10} stroke="#a855f7" strokeWidth={1} />
      ))}
      {report?.passes.map((p) => {
        const b = p.changed_bounds;
        const warned = !!p.diagnostics?.warnings?.some((w) => w.basis === "measured");
        return b && <rect key={`c-${p.id}`} x={b[0]} y={b[1]} width={b[2]} height={b[3]} fill="#22c55e22" stroke={warned ? "#f59e0b" : "#22c55e"} />;
      })}
    </svg>
  );
}

const field = "h-7 w-full rounded border bg-card px-1.5 text-xs";

export function PlanPanel() {
  const d = useDraft();
  const layers = useRasterLayers();
  const { doc, pageId, replaceDoc, notify } = useStudio();
  const layer = layers[0];
  useEffect(() => {
    if (d.kinds.length) return;
    postJson<{ kinds: { kind: string }[] }>("/api/paint/media", {})
      .then((r) => useDraft.getState().set({ kinds: r.kinds.map((k) => k.kind) }))
      .catch(() => undefined);
  }, [d.kinds.length]);
  if (!layer || !doc) return null;
  const size = (() => {
    const canvas = doc.pages.find((p) => p.id === pageId)?.canvas ?? doc.pages[0]?.canvas;
    return { w: canvas?.width ?? 256, h: canvas?.height ?? 256 };
  })();

  const update = (i: number, patch: Partial<Pass>) => d.set({ passes: d.passes.map((p, j) => (j === i ? { ...p, ...patch } : p)), report: null, preview: null, score: null });
  const move = (i: number, by: number) => {
    const next = [...d.passes];
    const j = i + by;
    if (j < 0 || j >= next.length) return;
    [next[i], next[j]] = [next[j]!, next[i]!];
    d.set({ passes: next, report: null, preview: null, score: null });
  };

  const run = async () => {
    d.set({ busy: true, status: "" });
    try {
      const plan = buildPlan(d.passes, layer.id);
      const body = await postJson<{ result: Report; document: Doc }>("/api/paint/plan", { document: doc, page: pageId, plan, dry_run: false });
      d.set({ report: body.result, preview: body.document, score: null, status: "Preview from the Rust planner. Nothing is saved until you commit." });
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Plan rejected.", report: null, preview: null });
    } finally {
      d.set({ busy: false });
    }
  };

  const commit = () => {
    if (!d.preview || !d.report) return;
    replaceDoc(d.preview);
    notify(`Committed plan ${d.report.plan_hash.slice(0, 15)}. Undo reverts every pass at once.`);
    d.set({ preview: null, report: null, status: "Committed as one undoable step." });
  };

  // Palette sampling reads the rendered canvas (preview if one is pending, else
  // the committed document) through the Rust sampler; nothing is written.
  const canvasDoc = d.preview ?? doc;
  const addColors = (i: number, colors: string[]) => {
    const have = d.passes[i]!.palette.split(/[\s,]+/).filter(Boolean);
    const merged = [...new Set([...have, ...colors])].slice(0, 16);
    update(i, { palette: merged.join(" ") });
  };
  const samplePoint = async (i: number) => {
    try {
      const r = await postJson<{ color: string; empty: boolean }>("/api/paint/palette", {
        document: canvasDoc, page: pageId, action: "sample", ...d.sample,
      });
      if (r.empty) d.set({ status: "That point is transparent; no color sampled." });
      else {
        addColors(i, [r.color.slice(0, 7)]);
        d.set({ status: `Sampled ${r.color} at ${d.sample.x},${d.sample.y}.` });
      }
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Could not sample." });
    }
  };
  const extractPalette = async (i: number) => {
    try {
      const r = await postJson<{ colors: { color: string }[] }>("/api/paint/palette", {
        document: canvasDoc, page: pageId, action: "extract", role: "dominant", count: 6,
      });
      addColors(i, r.colors.map((c) => c.color.slice(0, 7)));
      d.set({ status: `Extracted ${r.colors.length} dominant colors from the canvas.` });
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Could not extract a palette." });
    }
  };
  const cropPreview = async () => {
    const rect = rectOf(d.crop?.rect ?? "");
    try {
      const r = await postJson<{ report: { source: [number, number] }; png_base64: string }>("/api/paint/preview", {
        document: canvasDoc, page: pageId, scope: "page", ...(rect ? { rect } : {}), max_edge: 256,
      });
      d.set({ crop: { rect: d.crop?.rect ?? "", png: r.png_base64, report: `${r.report.source[0]}x${r.report.source[1]} px region` } });
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Could not preview." });
    }
  };
  const showField = async () => {
    try {
      const r = await postJson<{ samples: [number, number, number, number][]; field: { hash?: string }; returned: number }>("/api/paint/field", {
        document: canvasDoc, layer: layer.id, field: JSON.parse(d.field.json), max_vectors: 200,
      });
      d.set({ field: { ...d.field, vectors: r.samples, summary: `${r.returned} field vectors${r.field.hash ? " · " + r.field.hash.slice(0, 19) : ""}` } });
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Could not build the field." });
    }
  };
  const loadReference = (file: File | undefined) => {
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const url = String(reader.result);
      d.set({ reference: { name: file.name, base64: url.slice(url.indexOf(",") + 1) }, comparison: null });
    };
    reader.readAsDataURL(file);
  };
  const compareReference = async () => {
    if (!d.reference) return;
    try {
      const comparison = await postJson<Comparison>("/api/paint/compare", {
        document: canvasDoc, page: pageId, scope: "page", reference_image_base64: d.reference.base64, heatmap: true,
      });
      d.set({ comparison, status: "Compared the " + (d.preview ? "preview" : "canvas") + " with the reference." });
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Could not compare." });
    }
  };

  const compare = async () => {
    if (!d.preview) return;
    try {
      const goal = JSON.parse(d.goal) as unknown;
      const score = await postJson<State["score"]>("/api/paint/score", { before: doc, after: d.preview, layer: layer.id, goal });
      d.set({ score });
    } catch (e) {
      d.set({ status: e instanceof Error ? e.message : "Could not score." });
    }
  };

  return (
    <div className="space-y-2 border-t p-3 text-xs">
      <h3 className="font-semibold">Plan passes</h3>
      <p className="text-muted-foreground">Ordered passes run on layer {layer.id} as one atomic, undoable plan. Preview and commit use the same Rust planner.</p>
      <Overlay passes={d.passes} report={d.report} size={size} vectors={d.field.vectors} />
      {d.passes.map((p, i) => (
        <fieldset key={i} className="space-y-1 rounded border p-2">
          <legend className="px-1">
            {i + 1}. {p.id}
          </legend>
          <div className="flex gap-1">
            <Button size="sm" variant="outline" disabled={i === 0} onClick={() => move(i, -1)} aria-label="Move pass up">
              ↑
            </Button>
            <Button size="sm" variant="outline" disabled={i === d.passes.length - 1} onClick={() => move(i, 1)} aria-label="Move pass down">
              ↓
            </Button>
            <Button size="sm" variant="outline" disabled={d.passes.length === 1} onClick={() => d.set({ passes: d.passes.filter((_, j) => j !== i), report: null, preview: null })}>
              Remove
            </Button>
          </div>
          <select className={field} value={p.mode} onChange={(e) => update(i, { mode: e.target.value as Pass["mode"] })}>
            <option value="path">Path (SVG d)</option>
            <option value="pressure">Pressure samples</option>
          </select>
          {p.mode === "path" ? (
            <input className={field} value={p.path} onChange={(e) => update(i, { path: e.target.value })} aria-label="Path" />
          ) : (
            <textarea className={`${field} h-16`} value={p.samples} onChange={(e) => update(i, { samples: e.target.value })} title="x,y,pressure per line" aria-label="Pressure samples" />
          )}
          <div className="grid grid-cols-3 gap-1">
            <input type="number" className={field} value={p.size} min={1} onChange={(e) => update(i, { size: Number(e.target.value) })} title="Size" />
            <input type="number" className={field} value={p.hardness} min={0} max={1} step={0.1} onChange={(e) => update(i, { hardness: Number(e.target.value) })} title="Hardness" />
            <input type="color" className={field} value={p.color} onChange={(e) => update(i, { color: e.target.value })} title="Color" />
          </div>
          <select className={field} value={p.media} onChange={(e) => update(i, { media: e.target.value })} aria-label="Material">
            <option value="">Material: default brush</option>
            {d.kinds.map((k) => (
              <option key={k} value={k}>
                Material: {k}
              </option>
            ))}
          </select>
          <input className={field} placeholder="Region x,y,w,h (optional)" value={p.region} onChange={(e) => update(i, { region: e.target.value })} />
          <textarea className={`${field} h-12`} placeholder="Protected regions: x,y,w,h per line" value={p.protect} onChange={(e) => update(i, { protect: e.target.value })} />
          <input className={field} placeholder="Palette to hold, e.g. #3070c0 #101010" value={p.palette} onChange={(e) => update(i, { palette: e.target.value })} />
          <div className="flex flex-wrap items-center gap-1">
            <Button size="sm" variant="outline" onClick={() => void extractPalette(i)}>
              Palette from canvas
            </Button>
            <Button size="sm" variant="outline" onClick={() => void samplePoint(i)}>
              Sample point
            </Button>
          </div>
        </fieldset>
      ))}
      <div className="grid grid-cols-3 gap-1">
        {(["x", "y", "radius"] as const).map((k) => (
          <label key={k} className="text-muted-foreground">
            Sample {k}
            <input type="number" min={0} className={field} value={d.sample[k]} onChange={(e) => d.set({ sample: { ...d.sample, [k]: Math.max(0, Number(e.target.value)) } })} />
          </label>
        ))}
      </div>
      <div className="space-y-1 rounded border p-2">
        <label className="block">
          Crop preview region x,y,w,h (blank for the page)
          <input className={field} value={d.crop?.rect ?? ""} onChange={(e) => d.set({ crop: { rect: e.target.value, png: d.crop?.png ?? "", report: d.crop?.report ?? "" } })} />
        </label>
        <Button size="sm" variant="outline" onClick={() => void cropPreview()}>
          Crop preview
        </Button>
        {d.crop?.png && (
          <figure>
            <img alt="Crop preview" className="max-h-40 rounded border" src={`data:image/png;base64,${d.crop.png}`} />
            <figcaption className="text-muted-foreground">{d.crop.report}</figcaption>
          </figure>
        )}
      </div>
      <div className="space-y-1 rounded border p-2">
        <label className="block">
          Stroke field (JSON)
          <textarea className={`${field} h-14`} value={d.field.json} onChange={(e) => d.set({ field: { ...d.field, json: e.target.value, vectors: [] } })} />
        </label>
        <Button size="sm" variant="outline" onClick={() => void showField()}>
          Show field vectors
        </Button>
        {d.field.summary && <p className="text-muted-foreground">{d.field.summary}; drawn in purple on the overlay.</p>}
      </div>
      <div className="space-y-1 rounded border p-2">
        <label className="block">
          Reference image
          <input type="file" accept="image/png,image/jpeg,image/webp" className={field} onChange={(e) => loadReference(e.target.files?.[0])} />
        </label>
        <Button size="sm" variant="outline" disabled={!d.reference} onClick={() => void compareReference()}>
          Compare with reference
        </Button>
        {d.comparison && (
          <>
            {d.comparison.heatmap_png_base64 && (
              <figure>
                <img alt="Value-difference heat map (brighter is more different)" className="max-h-40 rounded border" src={`data:image/png;base64,${d.comparison.heatmap_png_base64}`} />
                <figcaption className="text-muted-foreground">Difference overlay: brighter pixels differ more.</figcaption>
              </figure>
            )}
            <pre className="max-h-40 overflow-auto rounded bg-card p-1" aria-live="polite">
              {JSON.stringify(d.comparison.metrics, null, 1)}
            </pre>
          </>
        )}
        <p className="text-muted-foreground">Metrics describe pixel differences only; they are not an artistic quality score.</p>
      </div>
      <div className="flex flex-wrap gap-1">
        <Button size="sm" variant="outline" onClick={() => d.set({ passes: [...d.passes, blank(d.passes.length + 1)], report: null, preview: null })}>
          Add pass
        </Button>
        <Button size="sm" disabled={d.busy} onClick={() => void run()}>
          Preview plan
        </Button>
        <Button size="sm" disabled={!d.preview} onClick={commit}>
          Commit
        </Button>
      </div>
      {d.status && <p className="text-muted-foreground">{d.status}</p>}
      {d.report && (
        <div className="space-y-1" aria-live="polite">
          <p>
            Plan {d.report.plan_hash.slice(0, 19)}: {d.report.totals.passes} passes, {d.report.totals.dabs} dabs, {d.report.totals.changed_pixels} pixels in {d.report.totals.changed_tiles} tiles.
          </p>
          {d.report.passes.map((p) => (
            <div key={p.id} className="rounded border p-1">
              <strong>{p.id}</strong>: {p.changed_pixels} px, {p.changed_tiles} tiles
              {p.changed_bounds && ` at ${p.changed_bounds.join(",")}`}
              {p.diagnostics?.warnings?.map((w, k) => (
                <div key={k} className={w.basis === "measured" ? "text-amber-600" : "text-muted-foreground"}>
                  [{w.basis}] {w.code}: {w.message}
                </div>
              ))}
            </div>
          ))}
          <label className="block">
            Goal (JSON) for reference comparison
            <textarea className={`${field} h-12`} value={d.goal} onChange={(e) => d.set({ goal: e.target.value })} />
          </label>
          <Button size="sm" variant="outline" onClick={() => void compare()}>
            Score preview against goal
          </Button>
          {d.score && (
            <p>
              Score {d.score.before?.score} → {d.score.after?.score}: {d.score.assessment?.trend} ({d.score.assessment?.basis} judgement).
            </p>
          )}
        </div>
      )}
      <p className="text-muted-foreground">
        Disclosure: every pass above is generated geometry executed deterministically by the Rust engine; measured warnings come from pixels, heuristic ones are advisory.
      </p>
    </div>
  );
}
