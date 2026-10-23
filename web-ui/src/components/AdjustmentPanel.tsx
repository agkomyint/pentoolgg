// Properties for adjustment layers and layer masks: Photoshop's Properties
// panel. Controls preview live while dragged and commit one undo step on release.
import { useRef, useState } from "react";
import { Plus, RotateCcw, X } from "lucide-react";
import type { SceneNode } from "@/lib/api";
import { useStudio } from "@/store";
import { findNode, pageOf } from "@/lib/model";
import { ADJUSTMENTS, adjustmentBase, adjustmentLabel, clipBaseOf, previewComposite, setComposite, toggleClip } from "@/lib/layers";
import { runScene } from "@/lib/actions";
import { Slider } from "@/workspaces/ImageAdjust";
import { Button } from "@/components/ui/button";

type Params = Record<string, unknown>;
type Stop = { position: number; rgb: [number, number, number] };

const heading = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";
const num = (v: unknown, fallback: number) => (typeof v === "number" && Number.isFinite(v) ? v : fallback);
const round = (n: number, places = 4) => Math.round(n * 10 ** places) / 10 ** places;

/** Live and committed edits of one adjustment's params. */
function useParams(node: SceneNode) {
  const params = (node.params ?? {}) as Params;
  const live = (next: Params) => previewComposite(node.id, { params: next });
  const commit = (next: Params) => {
    useStudio.getState().setPreview(null);
    setComposite(node.id, { params: next }, `Edit ${adjustmentLabel(node.adjustment)}`);
  };
  return { params, live, commit };
}

/** One numeric param as a slider. `scale` shows a fraction as a percentage. */
function ParamSlider({ node, name, label, min, max, step = 1, neutral, fix }: {
  node: SceneNode; name: string; label: string; min: number; max: number; step?: number; neutral: number; fix?: (p: Params) => Params;
}) {
  const { params, live, commit } = useParams(node);
  const make = (v: number) => (fix ?? ((p) => p))({ ...params, [name]: v });
  return <Slider label={label} value={num(params[name], neutral)} min={min} max={max} step={step} neutral={neutral} onLive={(v) => live(make(v))} onCommit={(v) => commit(make(v))} />;
}

/** Curves: drag points, click the line to add one, double-click a point to remove it. */
function CurvesEditor({ node }: { node: SceneNode }) {
  const { params, live, commit } = useParams(node);
  const saved = (Array.isArray(params.points) ? params.points : [[0, 0], [255, 255]]) as [number, number][];
  const [drag, setDrag] = useState<{ index: number; points: [number, number][] } | null>(null);
  const svg = useRef<SVGSVGElement>(null);
  const points = drag?.points ?? saved;
  const size = 200;
  const toSvg = ([x, y]: [number, number]) => [(x / 255) * size, size - (y / 255) * size] as const;
  const fromEvent = (e: React.PointerEvent): [number, number] => {
    const r = svg.current!.getBoundingClientRect();
    const x = Math.round(((e.clientX - r.left) / r.width) * 255);
    const y = Math.round((1 - (e.clientY - r.top) / r.height) * 255);
    return [Math.min(255, Math.max(0, x)), Math.min(255, Math.max(0, y))];
  };
  // Inputs stay strictly increasing, so a point cannot pass its neighbours.
  const place = (list: [number, number][], index: number, [x, y]: [number, number]) => {
    const next = list.map((p) => [...p] as [number, number]);
    const lo = index > 0 ? next[index - 1]![0] + 1 : 0;
    const hi = index < next.length - 1 ? next[index + 1]![0] - 1 : 255;
    next[index] = [Math.min(hi, Math.max(lo, x)), y];
    return next;
  };
  const path = points.map((p, i) => `${i ? "L" : "M"}${toSvg(p).join(",")}`).join(" ");
  return (
    <div className="space-y-1">
      <svg ref={svg} viewBox={`0 0 ${size} ${size}`} className="aspect-square w-full touch-none rounded border bg-background" role="img" aria-label="Curve"
        onPointerDown={(e) => {
          if (e.target !== e.currentTarget) return;
          if (points.length >= 16) return;
          const at = fromEvent(e);
          const index = points.findIndex((p) => p[0] > at[0]);
          if (index <= 0 || points.some((p) => p[0] === at[0])) return;
          const next = [...points.slice(0, index), at, ...points.slice(index)];
          e.currentTarget.setPointerCapture(e.pointerId);
          setDrag({ index, points: next });
          live({ ...params, points: next });
        }}
        onPointerMove={(e) => {
          if (!drag) return;
          const next = place(drag.points, drag.index, fromEvent(e));
          setDrag({ ...drag, points: next });
          live({ ...params, points: next });
        }}
        onPointerUp={() => {
          if (!drag) return;
          commit({ ...params, points: drag.points });
          setDrag(null);
        }}>
        {[1, 2, 3].map((i) => (
          <g key={i} className="stroke-border" strokeWidth={0.5}>
            <line x1={(i * size) / 4} x2={(i * size) / 4} y1={0} y2={size} />
            <line y1={(i * size) / 4} y2={(i * size) / 4} x1={0} x2={size} />
          </g>
        ))}
        <line x1={0} y1={size} x2={size} y2={0} className="stroke-muted-foreground" strokeWidth={0.5} strokeDasharray="3 3" />
        <path d={path} fill="none" className="pointer-events-none stroke-foreground" strokeWidth={1.5} />
        {points.map((p, i) => {
          const [cx, cy] = toSvg(p);
          return (
            <circle key={i} cx={cx} cy={cy} r={5} className="cursor-grab fill-background stroke-primary" strokeWidth={1.5}
              onPointerDown={(e) => {
                e.stopPropagation();
                e.currentTarget.ownerSVGElement?.setPointerCapture(e.pointerId);
                setDrag({ index: i, points });
              }}
              onDoubleClick={() => {
                if (i === 0 || i === points.length - 1 || points.length <= 2) return;
                commit({ ...params, points: points.filter((_, j) => j !== i) });
              }} />
          );
        })}
      </svg>
      <p className="text-[11px] text-muted-foreground">Click the line to add a point, drag to shape, double-click a point to remove it.</p>
    </div>
  );
}

const hex = (rgb: number[]) => `#${rgb.map((c) => Math.round(c).toString(16).padStart(2, "0")).join("")}`;
const rgbOf = (value: string): [number, number, number] => [1, 3, 5].map((i) => parseInt(value.slice(i, i + 2), 16)) as [number, number, number];

/** Gradient map stops: colour and position; the end stops stay at 0 and 1. */
function GradientStops({ node }: { node: SceneNode }) {
  const { params, commit } = useParams(node);
  const stops = (Array.isArray(params.stops) ? params.stops : []) as Stop[];
  const set = (next: Stop[]) => commit({ ...params, stops: next });
  const preview = `linear-gradient(to right, ${stops.map((s) => `${hex(s.rgb)} ${s.position * 100}%`).join(", ")})`;
  return (
    <div className="space-y-1.5">
      <div className="h-4 rounded border" style={{ background: preview }} aria-hidden="true" />
      {stops.map((stop, i) => {
        const end = i === 0 || i === stops.length - 1;
        const lo = i > 0 ? stops[i - 1]!.position + 0.01 : 0;
        const hi = i < stops.length - 1 ? stops[i + 1]!.position - 0.01 : 1;
        return (
          <div key={i} className="flex items-center gap-1.5 text-xs">
            <input type="color" aria-label={`Stop ${i + 1} colour`} className="h-6 w-8 rounded border bg-card" value={hex(stop.rgb)}
              onChange={(e) => set(stops.map((s, j) => (j === i ? { ...s, rgb: rgbOf(e.target.value) } : s)))} />
            <input type="number" aria-label={`Stop ${i + 1} position`} disabled={end} min={0} max={100} className="h-6 w-14 rounded border bg-card px-1 text-right tabular-nums disabled:opacity-50"
              defaultValue={Math.round(stop.position * 100)} key={`${i}-${stop.position}`}
              onBlur={(e) => {
                const v = Math.min(hi, Math.max(lo, Number(e.target.value) / 100));
                if (Number.isFinite(v) && v !== stop.position) set(stops.map((s, j) => (j === i ? { ...s, position: round(v) } : s)));
              }} />
            <span className="text-muted-foreground">%</span>
            {!end && (
              <button type="button" aria-label={`Remove stop ${i + 1}`} className="ml-auto rounded p-0.5 hover:bg-accent" onClick={() => set(stops.filter((_, j) => j !== i))}>
                <X size={12} />
              </button>
            )}
          </div>
        );
      })}
      <Button variant="outline" size="sm" disabled={stops.length >= 16} onClick={() => {
        // Add a stop in the widest gap, coloured halfway between its neighbours.
        let gap = 0;
        for (let i = 1; i < stops.length; i++) if (stops[i]!.position - stops[i - 1]!.position > stops[gap + 1]!.position - stops[gap]!.position) gap = i - 1;
        const a = stops[gap]!, b = stops[gap + 1]!;
        const mid: Stop = { position: round((a.position + b.position) / 2), rgb: a.rgb.map((c, k) => Math.round((c + b.rgb[k]!) / 2)) as Stop["rgb"] };
        set([...stops.slice(0, gap + 1), mid, ...stops.slice(gap + 1)]);
      }}><Plus size={12} /> Add stop</Button>
    </div>
  );
}

/** Channel mixer: pick an output channel, then mix the source channels into it. */
function ChannelMixer({ node }: { node: SceneNode }) {
  const { params, live, commit } = useParams(node);
  const [row, setRow] = useState(0);
  const matrix = (Array.isArray(params.matrix) ? params.matrix : [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0]]) as number[][];
  const make = (col: number, v: number) => ({ ...params, matrix: matrix.map((r, i) => (i === row ? r.map((c, j) => (j === col ? v / 100 : c)) : r)) });
  return (
    <div className="space-y-1.5">
      <label className="flex items-center gap-2 text-xs">
        <span className="text-muted-foreground">Output channel</span>
        <select className="h-6 rounded border bg-card px-1 text-xs" value={row} onChange={(e) => setRow(Number(e.target.value))}>
          {["Red", "Green", "Blue"].map((c, i) => <option key={c} value={i}>{c}</option>)}
        </select>
      </label>
      {["Red", "Green", "Blue", "Constant"].map((label, col) => (
        <Slider key={`${row}-${col}`} label={`${label} %`} value={Math.round((matrix[row]?.[col] ?? 0) * 100)} min={-200} max={200} neutral={col === row ? 100 : 0}
          onLive={(v) => live(make(col, v))} onCommit={(v) => commit(make(col, v))} />
      ))}
    </div>
  );
}

/** Black & White weights always add up to 100%: moving one rescales the others. */
function normalizeWeights(changed: string) {
  return (p: Params): Params => {
    const keys = ["red", "green", "blue"];
    const v = Math.min(1, Math.max(0, num(p[changed], 0)));
    const others = keys.filter((k) => k !== changed);
    const rest = others.reduce((sum, k) => sum + num(p[k], 0), 0);
    const out: Params = { ...p, [changed]: round(v) };
    for (const k of others) out[k] = rest > 0 ? round((num(p[k], 0) / rest) * (1 - v)) : round((1 - v) / 2);
    // Absorb rounding into the last weight so the sum is exactly 1.
    const last = others[others.length - 1]!;
    out[last] = round(1 - keys.filter((k) => k !== last).reduce((sum, k) => sum + num(out[k], 0), 0));
    return out;
  };
}

function KindControls({ node }: { node: SceneNode }) {
  const p = { node };
  switch (node.adjustment) {
    case "brightness-contrast":
      return <><ParamSlider {...p} name="brightness" label="Brightness" min={-100} max={100} neutral={0} /><ParamSlider {...p} name="contrast" label="Contrast" min={-100} max={100} neutral={0} /></>;
    case "levels": {
      const levels = (q: Params): Params => {
        const black = Math.min(num(q.black, 0), 254);
        return { ...q, black, white: Math.max(num(q.white, 255), black + 1) };
      };
      return <>
        <ParamSlider {...p} name="black" label="Shadows (input black)" min={0} max={254} neutral={0} fix={levels} />
        <ParamSlider {...p} name="gamma" label="Midtones (gamma)" min={0.1} max={10} step={0.01} neutral={1} />
        <ParamSlider {...p} name="white" label="Highlights (input white)" min={1} max={255} neutral={255} fix={(q) => ({ ...q, black: Math.min(num(q.black, 0), num(q.white, 255) - 1) })} />
      </>;
    }
    case "curves":
      return <CurvesEditor node={node} />;
    case "exposure":
      return <ParamSlider {...p} name="stops" label="Exposure (stops)" min={-8} max={8} step={0.01} neutral={0} />;
    case "vibrance":
      return <ParamSlider {...p} name="amount" label="Vibrance" min={-100} max={100} neutral={0} />;
    case "hue-saturation":
      return <><ParamSlider {...p} name="hue" label="Hue" min={-180} max={180} neutral={0} /><ParamSlider {...p} name="saturation" label="Saturation" min={-100} max={100} neutral={0} /></>;
    case "color-balance":
      return <>
        <ParamSlider {...p} name="red" label="Cyan ↔ Red" min={-100} max={100} neutral={0} />
        <ParamSlider {...p} name="green" label="Magenta ↔ Green" min={-100} max={100} neutral={0} />
        <ParamSlider {...p} name="blue" label="Yellow ↔ Blue" min={-100} max={100} neutral={0} />
      </>;
    case "black-and-white":
      return <>
        {(["red", "green", "blue"] as const).map((k, i) => (
          <ParamSlider key={k} {...p} name={k} label={`${["Reds", "Greens", "Blues"][i]} weight`} min={0} max={1} step={0.01} neutral={[0.2126, 0.7152, 0.0722][i]!} fix={normalizeWeights(k)} />
        ))}
        <p className="text-[11px] text-muted-foreground">The weights always add up to 1, so overall brightness is kept.</p>
      </>;
    case "channel-mixer":
      return <ChannelMixer node={node} />;
    case "posterize":
      return <ParamSlider {...p} name="levels" label="Levels" min={2} max={64} neutral={4} />;
    case "threshold":
      return <ParamSlider {...p} name="level" label="Threshold level" min={0} max={255} neutral={128} />;
    case "gradient-map":
      return <GradientStops node={node} />;
    case "invert":
      return <p className="text-[11px] text-muted-foreground">Invert has no settings.</p>;
    default:
      return <p className="text-[11px] text-muted-foreground">Edit this adjustment in Advanced object data.</p>;
  }
}

/** Adjustment layer settings: kind controls, what it affects, preview toggle, reset. */
export function AdjustmentPanel({ node, locked }: { node: SceneNode; locked: boolean }) {
  const { doc, pageId } = useStudio();
  const base = adjustmentBase(node);
  const scope = (node.scope ?? {}) as { kind?: string };
  const baseNode = base ? findNode(pageOf(doc, pageId), base)?.node : undefined;
  const spec = ADJUSTMENTS.find((a) => a.kind === node.adjustment);
  return (
    <section className={locked ? "pointer-events-none space-y-2 opacity-60" : "space-y-2"}>
      <div className="flex items-center justify-between">
        <h4 className={heading}>{adjustmentLabel(node.adjustment)}</h4>
        <span className="flex items-center gap-1">
          <label className="flex items-center gap-1 text-[11px] text-muted-foreground" title="Turn the adjustment off without hiding the layer">
            <input type="checkbox" checked={node.enabled !== false} onChange={(e) => setComposite(node.id, { enabled: e.target.checked }, "Toggle adjustment")} />
            On
          </label>
          {spec && (
            <Button variant="ghost" size="icon" aria-label="Reset to defaults" title="Reset to defaults" onClick={() => setComposite(node.id, { params: spec.params() }, "Reset adjustment")}>
              <RotateCcw size={13} />
            </Button>
          )}
        </span>
      </div>
      <label className="block text-xs">
        <span className="text-muted-foreground">Affects</span>
        <select className="mt-0.5 h-7 w-full rounded border bg-card px-1.5 text-xs" value={base ? "clip" : scope.kind === "below" ? "below" : "other"}
          onChange={(e) => {
            if ((e.target.value === "clip") !== !!base) toggleClip(node.id);
            else if (e.target.value === "below") setComposite(node.id, { scope: { kind: "below" } }, "Adjustment scope");
          }}>
          <option value="below">All layers below (in this page layer)</option>
          <option value="clip">Only the layer below (clipped)</option>
          {scope.kind !== "below" && !base && <option value="other">Custom scope (set by an agent)</option>}
        </select>
      </label>
      {base && <p className="text-[11px] text-muted-foreground">Clipped to {String(baseNode?.name ?? base)}.</p>}
      <KindControls node={node} />
    </section>
  );
}

type Mask = { resource: string; invert?: boolean; density?: number; feather?: number; linked?: boolean };

/** Layer mask controls for any compositing node: density, feather, invert, delete. */
export function MaskPanel({ node, locked }: { node: SceneNode; locked: boolean }) {
  const mask = node.mask as Mask | undefined;
  if (!mask) {
    return (
      <section className="space-y-1">
        <h4 className={heading}>Mask</h4>
        <p className="text-[11px] text-muted-foreground">
          To mask this layer with a shape, select both in Layers (Shift-click), then right-click this layer and choose Mask with selected shape.
          {clipBaseOf(node) === null && " Alt-click the layer to clip it to the layer below instead."}
        </p>
      </section>
    );
  }
  const change = (patch: Partial<Mask>) => ({ mask: { ...mask, ...patch } });
  return (
    <section className={locked ? "pointer-events-none space-y-2 opacity-60" : "space-y-2"}>
      <div className="flex items-center justify-between">
        <h4 className={heading}>Mask</h4>
        <Button variant="ghost" size="sm" onClick={() => void runScene([{ type: "mask-detach", id: node.id }])}>Delete mask</Button>
      </div>
      <Slider label="Density %" value={Math.round(num(mask.density, 1) * 100)} min={0} max={100} neutral={100}
        onLive={(v) => previewComposite(node.id, change({ density: v / 100 }))}
        onCommit={(v) => { useStudio.getState().setPreview(null); setComposite(node.id, change({ density: v / 100 }), "Mask density"); }} />
      <Slider label="Feather px" value={num(mask.feather, 0)} min={0} max={256} step={0.5} neutral={0}
        onLive={(v) => previewComposite(node.id, change({ feather: v }))}
        onCommit={(v) => { useStudio.getState().setPreview(null); setComposite(node.id, change({ feather: v }), "Mask feather"); }} />
      <div className="flex gap-3 text-xs">
        <label className="flex items-center gap-1">
          <input type="checkbox" checked={!!mask.invert} onChange={(e) => setComposite(node.id, change({ invert: e.target.checked }), "Invert mask")} /> Invert
        </label>
        <label className="flex items-center gap-1" title="When linked, the mask moves with the layer">
          <input type="checkbox" checked={mask.linked !== false} onChange={(e) => setComposite(node.id, change({ linked: e.target.checked }), "Link mask")} /> Linked
        </label>
      </div>
    </section>
  );
}
