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

type State = {
  passes: Pass[];
  report: Report | null;
  preview: Doc | null;
  score: { assessment?: { trend: string; basis: string }; before?: { score: number }; after?: { score: number } } | null;
  goal: string;
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
});

const useDraft = create<State>((set) => ({
  passes: [blank(1)],
  report: null,
  preview: null,
  score: null,
  goal: '{"coverage":[0.01,0.5]}',
  busy: false,
  status: "",
  set: (patch) => set(patch),
}));

const nums = (text: string): number[] | null => {
  const parts = text.split(/[\s,]+/).filter(Boolean).map(Number);
  return parts.every(Number.isFinite) ? parts : null;
};

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
      const brush = { size: p.size, hardness: p.hardness, spacing: 0.1 };
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

function Overlay({ passes, report, size }: { passes: Pass[]; report: Report | null; size: { w: number; h: number } }) {
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
      <Overlay passes={d.passes} report={d.report} size={size} />
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
          <input className={field} placeholder="Region x,y,w,h (optional)" value={p.region} onChange={(e) => update(i, { region: e.target.value })} />
          <textarea className={`${field} h-12`} placeholder="Protected regions: x,y,w,h per line" value={p.protect} onChange={(e) => update(i, { protect: e.target.value })} />
          <input className={field} placeholder="Palette to hold, e.g. #3070c0 #101010" value={p.palette} onChange={(e) => update(i, { palette: e.target.value })} />
        </fieldset>
      ))}
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
