import { useState } from "react";
import { RotateCcw, RotateCw } from "lucide-react";
import type { Doc, SceneNode } from "@/lib/api";
import { useStudio } from "@/store";
import { clone, findNode, pageOf } from "@/lib/model";
import { Button } from "@/components/ui/button";

type Params = Record<string, number>;
type Op = { id: string; kind: string; version?: number; enabled?: boolean; params?: Params };

const heading = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

function opsOf(node: SceneNode): Op[] {
  return Array.isArray(node.operations) ? (node.operations as Op[]) : [];
}

/** Set (params) or remove (null) the single operation of `kind` on an image node. */
function withOp(node: SceneNode, kind: string, params: Params | null): void {
  const ops = opsOf(node).filter((o) => o.kind !== kind || params !== null);
  if (params !== null) {
    const at = ops.findIndex((o) => o.kind === kind);
    if (at >= 0) ops[at] = { ...ops[at]!, params, enabled: true };
    else ops.push({ id: `op-${kind}`, kind, version: 1, enabled: true, params });
  }
  node.operations = ops;
}

function mutate(doc: Doc, pageId: string | undefined, id: string, change: (n: SceneNode) => void): Doc {
  const next = clone(doc);
  const found = findNode(pageOf(next, pageId), id);
  if (found) change(found.node);
  return next;
}

/** Live preview: render a throw-away copy while a control is dragged, commit one undo step on release. */
function useLive(node: SceneNode) {
  const store = useStudio();
  const live = (change: (n: SceneNode) => void) => {
    const { doc, pageId } = useStudio.getState();
    if (doc) store.setPreview(mutate(doc, pageId, node.id, change));
  };
  const commit = (change: (n: SceneNode) => void) => {
    store.setPreview(null);
    store.edit("Adjust image", (d, { pageId }) => {
      const found = findNode(pageOf(d, pageId), node.id);
      if (found) change(found.node);
    });
  };
  return { live, commit };
}

export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  neutral,
  onLive,
  onCommit,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  neutral: number;
  onLive: (v: number) => void;
  onCommit: (v: number) => void;
}) {
  const [drag, setDrag] = useState<number | null>(null);
  const shown = drag ?? value;
  const finish = () => {
    if (drag !== null) onCommit(drag);
    setDrag(null);
  };
  return (
    <label className="block text-xs">
      <span className="flex justify-between">
        <span>{label}</span>
        <span className="flex items-center gap-1.5 tabular-nums text-muted-foreground">
          {Math.round(shown * 100) / 100}
          {shown !== neutral && (
            <button
              className="underline"
              onClick={() => {
                setDrag(null);
                onCommit(neutral);
              }}
            >
              reset
            </button>
          )}
        </span>
      </span>
      <input
        type="range"
        className="mt-0.5 w-full accent-[var(--primary)]"
        min={min}
        max={max}
        step={step}
        value={shown}
        onChange={(e) => {
          const v = Number(e.target.value);
          setDrag(v);
          onLive(v);
        }}
        onPointerUp={finish}
        onKeyUp={finish}
        onBlur={finish}
      />
    </label>
  );
}

export function ImageAdjust({ node }: { node: SceneNode }) {
  const { live, commit } = useLive(node);
  const ops = opsOf(node);
  const get = (kind: string, key: string, fallback: number) =>
    ops.find((o) => o.kind === kind && o.enabled !== false)?.params?.[key] ?? fallback;
  const has = (kind: string) => ops.some((o) => o.kind === kind && o.enabled !== false);

  // Each slider writes one operation and keeps that operation's other parameters.
  const op = (kind: string, key: string, neutral: Params, isNeutral: (p: Params) => boolean) => ({
    onLive: (v: number) =>
      live((n) => withOp(n, kind, { ...neutral, ...(opsOf(n).find((o) => o.kind === kind)?.params ?? {}), [key]: v })),
    onCommit: (v: number) =>
      commit((n) => {
        const p = { ...neutral, ...(opsOf(n).find((o) => o.kind === kind)?.params ?? {}), [key]: v };
        withOp(n, kind, isNeutral(p) ? null : p);
      }),
  });
  const bc = { brightness: 0, contrast: 0 };
  const hs = { hue: 0, saturation: 0 };
  const lv = { black: 0, white: 255, gamma: 1 };
  const bcNeutral = (p: Params) => !p.brightness && !p.contrast;
  const hsNeutral = (p: Params) => !p.hue && !p.saturation;
  const lvNeutral = (p: Params) => p.black === 0 && p.white === 255 && p.gamma === 1;
  const degrees = get("rotate", "degrees", 0);
  const rotate = (delta: number) =>
    commit((n) => {
      const d = (((degrees + delta) % 360) + 360) % 360;
      withOp(n, "rotate", d === 0 ? null : { degrees: d });
    });
  const opacity = typeof node.opacity === "number" ? node.opacity : 1;
  const crop = Array.isArray(node.crop) && node.crop.length === 4 ? (node.crop as number[]) : [0, 0, 1, 1];
  // The crop rectangle must stay inside the source: x+width <= 1 and y+height <= 1.
  const cropWith = (i: number, v: number) => {
    const c = crop.map((value, j) => (j === i ? v : value));
    if (c[0]! + c[2]! > 1) (i === 0 ? (c[0] = 1 - c[2]!) : (c[2] = 1 - c[0]!));
    if (c[1]! + c[3]! > 1) (i === 1 ? (c[1] = 1 - c[3]!) : (c[3] = 1 - c[1]!));
    c[2] = Math.max(0.01, c[2]!);
    c[3] = Math.max(0.01, c[3]!);
    return c;
  };

  return (
    <div className="space-y-4">
      <section className="space-y-2">
        <h3 className={heading}>Transform</h3>
        <div className="flex flex-wrap items-center gap-1.5">
          <Button size="sm" variant="outline" onClick={() => rotate(-90)}>
            <RotateCcw size={12} /> 90°
          </Button>
          <Button size="sm" variant="outline" onClick={() => rotate(90)}>
            <RotateCw size={12} /> 90°
          </Button>
          <span className="text-[11px] text-muted-foreground">rotated {degrees}°</span>
        </div>
        <label className="block text-xs">
          Fit
          <select
            className="mt-0.5 h-7 w-full rounded border bg-card px-1.5 text-xs"
            value={String(node.fit ?? "contain")}
            onChange={(e) => commit((n) => void (n.fit = e.target.value))}
          >
            {["contain", "cover", "fill", "none", "scale-down"].map((f) => (
              <option key={f}>{f}</option>
            ))}
          </select>
        </label>
        <Slider
          label="Opacity"
          value={opacity}
          min={0}
          max={1}
          step={0.01}
          neutral={1}
          onLive={(v) => live((n) => void (n.opacity = v))}
          onCommit={(v) => commit((n) => void (n.opacity = v))}
        />
      </section>

      <section className="space-y-2">
        <h3 className={heading}>Crop (fraction of source)</h3>
        {(["x", "y", "width", "height"] as const).map((label, i) => (
          <Slider
            key={label}
            label={label}
            value={crop[i]!}
            min={0}
            max={1}
            step={0.01}
            neutral={i < 2 ? 0 : 1}
            onLive={(v) => live((n) => void (n.crop = cropWith(i, v)))}
            onCommit={(v) => commit((n) => void (n.crop = cropWith(i, v)))}
          />
        ))}
      </section>

      <section className="space-y-2">
        <h3 className={heading}>Light</h3>
        <Slider label="Brightness" value={get("brightness-contrast", "brightness", 0)} min={-100} max={100} neutral={0} {...op("brightness-contrast", "brightness", bc, bcNeutral)} />
        <Slider label="Contrast" value={get("brightness-contrast", "contrast", 0)} min={-100} max={100} neutral={0} {...op("brightness-contrast", "contrast", bc, bcNeutral)} />
        <Slider label="Black point" value={get("levels", "black", 0)} min={0} max={254} neutral={0} {...op("levels", "black", lv, lvNeutral)} />
        <Slider label="White point" value={get("levels", "white", 255)} min={1} max={255} neutral={255} {...op("levels", "white", lv, lvNeutral)} />
        <Slider label="Gamma" value={get("levels", "gamma", 1)} min={0.1} max={4} step={0.05} neutral={1} {...op("levels", "gamma", lv, lvNeutral)} />
      </section>

      <section className="space-y-2">
        <h3 className={heading}>Color</h3>
        <Slider label="Hue" value={get("hue-saturation", "hue", 0)} min={-180} max={180} neutral={0} {...op("hue-saturation", "hue", hs, hsNeutral)} />
        <Slider label="Saturation" value={get("hue-saturation", "saturation", 0)} min={-100} max={100} neutral={0} {...op("hue-saturation", "saturation", hs, hsNeutral)} />
        <label className="flex items-center gap-1.5 text-xs">
          <input type="checkbox" checked={has("grayscale")} onChange={(e) => commit((n) => withOp(n, "grayscale", e.target.checked ? {} : null))} />
          Grayscale
        </label>
      </section>

      <section className="space-y-2">
        <h3 className={heading}>Detail</h3>
        <Slider label="Blur radius" value={get("blur", "radius", 0)} min={0} max={50} step={0.5} neutral={0} {...op("blur", "radius", { radius: 0 }, (p) => !p.radius)} />
        <Slider label="Sharpen radius" value={get("sharpen", "radius", 0)} min={0} max={20} step={0.5} neutral={0} {...op("sharpen", "radius", { radius: 0, amount: 100 }, (p) => !p.radius)} />
        <Slider label="Sharpen amount" value={get("sharpen", "amount", 100)} min={0} max={500} neutral={100} {...op("sharpen", "amount", { radius: 1, amount: 100 }, () => false)} />
      </section>
    </div>
  );
}
