import { useMemo, useState } from "react";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { addImage, runScene } from "@/lib/actions";
import { collectIds, findNode, layerNodes, pageOf } from "@/lib/model";
import type { SceneNode } from "@/lib/api";
import { useRef } from "react";
import { ImageAdjust } from "./ImageAdjust";

const input = "mt-0.5 h-7 w-full rounded border bg-card px-1.5 text-xs";
const heading = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

const IMAGE_OPS = ["crop", "resize", "rotate", "brightness-contrast", "levels", "curves", "hue-saturation", "blur", "sharpen", "grayscale"];
const ADJUSTMENTS = ["brightness-contrast", "levels", "curves", "hue-saturation", "exposure", "vibrance", "color-balance", "black-and-white", "channel-mixer", "gradient-map", "invert", "posterize", "threshold"];
const BLENDS = ["normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn", "hard-light", "soft-light", "difference", "exclusion"];

function walk(nodes: SceneNode[], out: SceneNode[] = []): SceneNode[] {
  for (const node of nodes) {
    out.push(node);
    if (Array.isArray(node.children)) walk(node.children, out);
  }
  return out;
}

function useNodes() {
  const { doc, pageId, selectedId } = useStudio();
  const page = pageOf(doc, pageId);
  const all = useMemo(() => walk((page?.layers ?? []).flatMap((l) => layerNodes(l))), [page]);
  return { doc, page, pageId, all, selected: findNode(page, selectedId)?.node ?? null, selectedId };
}

async function run(ops: unknown[]) {
  await runScene(ops);
}

function Json({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <label className="block text-xs">
      {label}
      <textarea className="mt-0.5 h-20 w-full rounded border bg-card p-1.5 font-mono text-[11px]" value={value} onChange={(e) => onChange(e.target.value)} spellCheck={false} />
    </label>
  );
}

function parse(source: string): Record<string, unknown> | null {
  try {
    const value = JSON.parse(source || "{}") as unknown;
    return value && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

export function ImagePanel() {
  const { doc, all, selected } = useNodes();
  const { notify } = useStudio();
  const file = useRef<HTMLInputElement>(null);
  const images = all.filter((n) => n.kind === "image");
  const target = selected?.kind === "image" ? selected : images[0] ?? null;
  const [kind, setKind] = useState(IMAGE_OPS[0]!);
  const [params, setParams] = useState("{}");
  const ops = (target?.operations as { id: string; kind: string; params?: unknown; enabled?: boolean }[] | undefined) ?? [];
  const parsed = parse(params);
  return (
    <div className="space-y-4">
      <section className="space-y-2">
        <h3 className={heading}>Raster images (v0.7)</h3>
        <Button variant="outline" size="sm" onClick={() => file.current?.click()}>
          Add image...
        </Button>
        <input ref={file} hidden type="file" accept="image/png,image/jpeg,image/webp" onChange={(e) => {
          const f = e.target.files?.[0];
          if (f) void addImage(f, f.name);
          e.target.value = "";
        }} />
        <p className="text-[11px] text-muted-foreground">Or paste / drop an image on the canvas. Source pixels are never modified; operations are non-destructive.</p>
      </section>
      {doc && doc.version < 5 && <p className="text-xs text-muted-foreground">Image operations need format v5+. Adding an image offers the upgrade.</p>}
      {target && doc && doc.version >= 5 && (
        <section className="space-y-2">
          <h3 className={heading}>Adjust {target.id} (live)</h3>
          <ImageAdjust key={target.id} node={target} />
        </section>
      )}
      {target ? (
        <section className="space-y-2">
          <h3 className={heading}>Operations on {target.id}</h3>
          <ol className="space-y-1">
            {ops.map((op) => (
              <li key={op.id} className="flex items-center gap-1 rounded border bg-card px-2 py-1 text-xs">
                <span className="min-w-0 flex-1 truncate">
                  {op.kind} <code className="text-[10px] text-muted-foreground">{JSON.stringify(op.params ?? {})}</code>
                </span>
                <button className="text-[11px] underline" onClick={() => void run([{ type: op.enabled === false ? "image-op-enable" : "image-op-disable", id: target.id, op_id: op.id }])}>
                  {op.enabled === false ? "enable" : "disable"}
                </button>
                <button className="text-[11px] underline" onClick={() => void run([{ type: "image-op-remove", id: target.id, op_id: op.id }])}>
                  remove
                </button>
              </li>
            ))}
            {!ops.length && <li className="text-xs text-muted-foreground">No operations yet.</li>}
          </ol>
          <label className="block text-xs">
            Add operation
            <select className={input} value={kind} onChange={(e) => setKind(e.target.value)}>
              {IMAGE_OPS.map((k) => (
                <option key={k}>{k}</option>
              ))}
            </select>
          </label>
          <Json label="Parameters (JSON)" value={params} onChange={setParams} />
          <Button size="sm" onClick={() => (parsed ? void run([{ type: "image-op-add", id: target.id, op: kind, params: parsed }]) : notify("Parameters must be a JSON object.", true))}>
            Add operation
          </Button>
        </section>
      ) : (
        <p className="text-xs text-muted-foreground">No image on this page yet.</p>
      )}
    </div>
  );
}

export function CompositePanel() {
  const { doc, all, selected, page } = useNodes();
  const { notify } = useStudio();
  const [blend, setBlend] = useState("normal");
  const [opacity, setOpacity] = useState(1);
  const [adjustment, setAdjustment] = useState(ADJUSTMENTS[0]!);
  const [params, setParams] = useState("{}");
  const [mask, setMask] = useState("");
  const [base, setBase] = useState("");
  if (!doc) return null;
  if (doc.version < 6) {
    return (
      <p className="text-xs text-muted-foreground">
        Compositing needs document format v6. Upgrade with <code>pentool migrate document.pen --target 6</code>, then reload.
      </p>
    );
  }
  const layerId = page?.layers?.[0]?.id ?? "layer-1";
  const ids = collectIds(doc);
  let adjId = "adjustment-1";
  for (let i = 1; ids.has(adjId); i++) adjId = `adjustment-${i}`;
  const parsed = parse(params);
  return (
    <div className="space-y-4">
      <section className="space-y-2">
        <h3 className={heading}>Appearance of {selected?.id ?? "(select an object)"}</h3>
        <div className="grid grid-cols-2 gap-2">
          <label className="text-xs">
            Blend
            <select className={input} value={blend} onChange={(e) => setBlend(e.target.value)}>
              {BLENDS.map((b) => (
                <option key={b}>{b}</option>
              ))}
            </select>
          </label>
          <label className="text-xs">
            Opacity
            <input className={input} type="number" min={0} max={1} step={0.05} value={opacity} onChange={(e) => setOpacity(Number(e.target.value))} />
          </label>
        </div>
        <Button size="sm" disabled={!selected} onClick={() => selected && void run([{ type: "composite-set", id: selected.id, settings: { blend_mode: blend, opacity } }])}>
          Apply appearance
        </Button>
      </section>
      <section className="space-y-2">
        <h3 className={heading}>Adjustment layer</h3>
        <select className={input} value={adjustment} onChange={(e) => setAdjustment(e.target.value)}>
          {ADJUSTMENTS.map((a) => (
            <option key={a}>{a}</option>
          ))}
        </select>
        <Json label="Parameters (JSON)" value={params} onChange={setParams} />
        <Button size="sm" onClick={() => (parsed ? void run([{ type: "adjustment-add", id: adjId, settings: { layer: layerId, adjustment, params: parsed } }]) : notify("Parameters must be a JSON object.", true))}>
          Add adjustment
        </Button>
      </section>
      <section className="space-y-2">
        <h3 className={heading}>Masks and clipping</h3>
        <label className="block text-xs">
          Mask name
          <input className={input} value={mask} onChange={(e) => setMask(e.target.value)} />
        </label>
        <div className="flex flex-wrap gap-1.5">
          <Button size="sm" variant="outline" disabled={!selected || !mask} onClick={() => selected && void run([{ type: "mask-attach", id: selected.id, name: mask, settings: {} }])}>
            Attach
          </Button>
          <Button size="sm" variant="outline" disabled={!selected} onClick={() => selected && void run([{ type: "mask-detach", id: selected.id }])}>
            Detach
          </Button>
        </div>
        <label className="block text-xs">
          Clip to base
          <select className={input} value={base} onChange={(e) => setBase(e.target.value)}>
            <option value="">(choose)</option>
            {all.filter((n) => n.id !== selected?.id).map((n) => (
              <option key={n.id}>{n.id}</option>
            ))}
          </select>
        </label>
        <div className="flex gap-1.5">
          <Button size="sm" variant="outline" disabled={!selected || !base} onClick={() => selected && void run([{ type: "clip-add", id: selected.id, base }])}>
            Clip
          </Button>
          <Button size="sm" variant="outline" disabled={!selected} onClick={() => selected && void run([{ type: "clip-remove", id: selected.id }])}>
            Release
          </Button>
        </div>
      </section>
    </div>
  );
}

/** Raw batch console: anything the Rust scene batch accepts. */
export function SceneConsole() {
  const [text, setText] = useState('[\n  {"type":"put-shape","id":"hello","layer":"layer-1","shape":"rect","x":40,"y":40,"width":160,"height":90,"fill":"#2563eb"}\n]');
  const { notify } = useStudio();
  return (
    <section className="space-y-2">
      <h3 className={heading}>Scene operations (JSON)</h3>
      <textarea className="h-40 w-full rounded border bg-card p-1.5 font-mono text-[11px]" value={text} onChange={(e) => setText(e.target.value)} spellCheck={false} />
      <Button
        size="sm"
        onClick={() => {
          try {
            const ops = JSON.parse(text) as unknown;
            if (!Array.isArray(ops)) throw new Error("Operations must be a JSON array.");
            void run(ops);
          } catch (error) {
            notify(error instanceof Error ? error.message : "Invalid JSON.", true);
          }
        }}
      >
        Apply batch
      </Button>
      <p className="text-[11px] text-muted-foreground">Runs through the same Rust batch planner as the CLI; the result is one undo step.</p>
    </section>
  );
}
