import { Button } from "@/components/ui/button";
import { usePaint } from "./rasterStore";
import { useRasterLayers } from "./PaintOverlay";
import { useStudio } from "@/store";
import { postJson, type Doc } from "@/lib/api";
import { addPaintLayer } from "./paintLayer";
import { PlanPanel } from "./PlanPanel";

const field = "h-7 w-full rounded border bg-card px-1.5 text-xs";

function Range({ label, value, min, max, step = 1, onChange }: { label: string; value: number; min: number; max: number; step?: number; onChange: (v: number) => void }) {
  return (
    <label className="grid grid-cols-[84px_1fr_40px] items-center gap-2 text-xs">
      <span>{label}</span>
      <input type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} />
      <output className="text-right tabular-nums">{value}</output>
    </label>
  );
}

export function RasterPanel() {
  const p = usePaint();
  const layers = useRasterLayers();
  const layer = layers.find((n) => n.id === p.layerId) ?? layers[0];

  const addLayer = async () => {
    if (p.busy) return;
    p.set({ busy: true });
    try {
      await addPaintLayer();
    } finally {
      p.set({ busy: false });
    }
  };
  const clearSelection = async () => {
    const { doc, pageId, replaceDoc } = useStudio.getState();
    if (!doc || !layer || p.busy) return;
    p.set({ busy: true });
    try {
      const body = await postJson<{ document: Doc }>("/api/raster", { document: doc, page: pageId, id: layer.id, action: "select-clear", args: {} });
      replaceDoc(body.document);
      p.set({ selection: null, status: "Selection cleared." });
    } catch (e) {
      p.set({ status: e instanceof Error ? e.message : "Request rejected." });
    } finally {
      p.set({ busy: false });
    }
  };

  const selectionAction = async (action: string, args: Record<string, unknown>, done: string) => {
    const { doc, pageId, replaceDoc } = useStudio.getState();
    if (!doc || !layer || p.busy) return;
    p.set({ busy: true });
    try {
      const body = await postJson<{ document: Doc; result?: { bounds?: number[] | null; selection?: { bounds?: number[] | null } } }>("/api/raster", { document: doc, page: pageId, id: layer.id, action, args });
      replaceDoc(body.document);
      const bounds = body.result?.bounds ?? body.result?.selection?.bounds;
      if (action !== "lift") p.set({ selection: bounds ?? null });
      p.set({ status: done });
    } catch (e) {
      p.set({ status: e instanceof Error ? e.message : "Request rejected." });
      useStudio.getState().notify(e instanceof Error ? e.message : "Request rejected.", true);
    } finally {
      p.set({ busy: false });
    }
  };
  const modify = (op: string) => void selectionAction("select-modify", { op, amount: p.amount }, `Selection ${op} applied.`);
  const toNewLayer = (cut: boolean) => {
    const taken = new Set(layers.map((n) => n.id));
    let n = 1;
    while (taken.has(`cutout-${n}`)) n += 1;
    void selectionAction("lift", { new_id: `cutout-${n}`, cut }, cut ? `Cut to layer cutout-${n}.` : `Copied to layer cutout-${n}.`);
  };

  const tool = p.tool;
  const paints = ["brush", "eraser", "clone", "heal", "quickmask"].includes(tool);
  const selecting = ["marquee", "lasso", "wand", "quickmask"].includes(tool);
  return (
    <div className="space-y-3 text-xs">
      <label className="block space-y-1">
        <span className="text-muted-foreground">Layer</span>
        <select className={field} value={layer?.id ?? ""} onChange={(e) => p.set({ layerId: e.target.value })}>
          {layers.map((n) => (
            <option key={n.id} value={n.id}>
              {n.id} ({n.width}x{n.height})
            </option>
          ))}
        </select>
      </label>

      {paints && (
        <div className="space-y-2">
          <label className="block space-y-1">
            <span className="text-muted-foreground">Brush</span>
            <select className={field} value={p.kind} onChange={(e) => p.set({ kind: e.target.value })}>
              {["hard-round", "soft-round", "pixel"].map((k) => (
                <option key={k}>{k}</option>
              ))}
            </select>
          </label>
          <Range label="Size" value={p.size} min={1} max={200} onChange={(size) => p.set({ size })} />
          <Range label="Hardness" value={p.hardness} min={0} max={1} step={0.05} onChange={(hardness) => p.set({ hardness })} />
          <Range label="Flow" value={p.flow} min={0.05} max={1} step={0.05} onChange={(flow) => p.set({ flow })} />
          <Range label="Stabilizer" value={p.stabilizer} min={0} max={0.95} step={0.05} onChange={(stabilizer) => p.set({ stabilizer })} />
          <label className="flex items-center gap-2">
            <span>Color</span>
            <input type="color" value={p.color} onChange={(e) => p.set({ color: e.target.value })} />
          </label>
        </div>
      )}

      {selecting && (
        <div className="space-y-2">
          <label className="block space-y-1">
            <span className="text-muted-foreground">Selection mode</span>
            <select className={field} value={p.mode} onChange={(e) => p.set({ mode: e.target.value })}>
              {["replace", "add", "subtract", "intersect"].map((m) => (
                <option key={m}>{m}</option>
              ))}
            </select>
          </label>
          {tool === "marquee" && (
            <label className="block space-y-1">
              <span className="text-muted-foreground">Marquee</span>
              <select className={field} value={p.shape} onChange={(e) => p.set({ shape: e.target.value })}>
                <option>rect</option>
                <option>ellipse</option>
              </select>
            </label>
          )}
          {tool === "wand" && (
            <>
              <Range label="Tolerance" value={p.tolerance} min={0} max={255} onChange={(tolerance) => p.set({ tolerance })} />
              <label className="block space-y-1">
                <span className="text-muted-foreground">Sample size</span>
                <select className={field} value={p.sample} onChange={(e) => p.set({ sample: Number(e.target.value) })}>
                  {[1, 3, 5, 11].map((n) => (
                    <option key={n} value={n}>
                      {n === 1 ? "Point" : `${n} x ${n} average`}
                    </option>
                  ))}
                </select>
              </label>
              <Range label="Gradient" value={p.gradient} min={0} max={64} onChange={(gradient) => p.set({ gradient })} />
              <p className="text-muted-foreground">Gradient follows slow color changes, so a sky or studio sweep selects as one region. 0 compares with the clicked pixel.</p>
            </>
          )}
        </div>
      )}

      {selecting && (
        <div className="space-y-1">
          <Range label="Amount" value={p.amount} min={1} max={64} onChange={(amount) => p.set({ amount })} />
          <div className="flex flex-wrap gap-1">
            {["invert", "feather", "expand", "contract", "smooth"].map((op) => (
              <Button key={op} size="sm" variant="outline" disabled={!p.selection || p.busy} onClick={() => modify(op)}>
                {op[0].toUpperCase() + op.slice(1)}
              </Button>
            ))}
            <Button size="sm" variant="outline" disabled={!p.selection || p.busy} title="Snap a rough selection to the edges of the subject" onClick={() => void selectionAction("select-refine", { band: Math.min(64, Math.max(1, p.amount * 3)) }, "Selection snapped to image edges.")}>
              Refine edge
            </Button>
            <Button size="sm" variant="outline" disabled={!p.selection || p.busy} onClick={() => toNewLayer(false)}>
              Copy to layer
            </Button>
            <Button size="sm" variant="outline" disabled={!p.selection || p.busy} onClick={() => toNewLayer(true)}>
              Cut to layer
            </Button>
          </div>
        </div>
      )}

      <div className="flex flex-wrap items-center gap-1">
        <Button size="sm" variant="outline" disabled={!p.selection} onClick={() => void clearSelection()}>
          Deselect (Ctrl+D)
        </Button>
        <Button size="sm" variant="outline" onClick={() => void addLayer()}>
          New paint layer
        </Button>
      </div>
      <p className="text-muted-foreground">[ and ] change the brush size. Alt-click with Clone or Heal sets the source. Hold Space or the middle button to pan; Ctrl+wheel zooms.</p>

      <div className="space-y-1 border-t pt-2">
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={p.rulers} onChange={(e) => p.set({ rulers: e.target.checked })} /> Rulers (click a ruler to add a guide)
        </label>
        <label className="flex items-center gap-2">
          <input type="checkbox" checked={p.snap} onChange={(e) => p.set({ snap: e.target.checked })} /> Snap
        </label>
        <label className="flex items-center gap-2">
          Grid
          <input
            type="number"
            min={0}
            max={512}
            value={p.grid}
            title="Grid step in layer pixels; 0 turns the grid off"
            onChange={(e) => p.set({ grid: Number(e.target.value) })}
            className="h-7 w-20 rounded border bg-card px-1.5"
          />
        </label>
        <Button size="sm" variant="outline" onClick={() => p.set({ guides: [] })}>
          Clear guides
        </Button>
      </div>
      <p className="text-muted-foreground">Rulers, guides and snapping are view aids kept in the browser only.</p>
      <PlanPanel />
    </div>
  );
}
