import { Button } from "@/components/ui/button";
import { usePaint, type RasterTool } from "./rasterStore";
import { useRasterLayers } from "./RasterStage";
import { useStudio } from "@/store";
import { api, postJson, type Doc } from "@/lib/api";

const field = "h-7 w-full rounded border bg-card px-1.5 text-xs";
const TOOLS: [RasterTool, string][] = [
  ["brush", "Brush"],
  ["eraser", "Eraser"],
  ["clone", "Clone"],
  ["heal", "Heal"],
  ["source", "Set clone source"],
  ["quickmask", "Quick mask"],
  ["marquee", "Marquee"],
  ["lasso", "Lasso"],
  ["wand", "Magic wand"],
];

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
    const { doc, pageId, replaceDoc, notify } = useStudio.getState();
    if (!doc || p.busy) return;
    p.set({ busy: true });
    try {
      // Raster layers need format v6 or newer; upgrade first when the document is older.
      let source: Doc = doc;
      if (source.version < 6) source = (await api.migrate(source, 6)).document;
      const id = `paint-${Date.now().toString(36)}`;
      const body = await postJson<{ document: Doc }>("/api/raster", { document: source, page: pageId, id, action: "add", args: {} });
      replaceDoc(body.document);
      p.set({ layerId: id, status: "Paint layer added. Pick a tool and paint on the preview." });
      notify("Added a paint layer. Save to keep it.");
    } catch (e) {
      p.set({ status: e instanceof Error ? e.message : "Could not add a paint layer." });
    } finally {
      p.set({ busy: false });
    }
  };

  if (!layers.length) {
    return (
      <div className="space-y-2 p-3 text-xs">
        <p className="text-muted-foreground">This page has no paint layer yet. A paint layer holds pixels and is sized to the page.</p>
        <Button size="sm" disabled={p.busy} onClick={() => void addLayer()}>
          Add paint layer
        </Button>
        {p.status && <p className="text-muted-foreground">{p.status}</p>}
      </div>
    );
  }
  const clearSelection = async () => {
    const { doc, pageId, replaceDoc } = useStudio.getState();
    if (!doc || !layer || p.busy) return;
    p.set({ busy: true });
    try {
      const body = await postJson<{ document: Doc }>("/api/raster", { document: doc, page: pageId, id: layer.id, action: "select-clear", args: {} });
      replaceDoc(body.document);
      p.set({ status: "select-clear: done." });
    } catch (e) {
      p.set({ status: e instanceof Error ? e.message : "Request rejected." });
    } finally {
      p.set({ busy: false });
    }
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
      <div className="grid grid-cols-3 gap-1">
        {TOOLS.map(([id, label]) => (
          <Button key={id} size="sm" variant={tool === id ? "default" : "outline"} aria-pressed={tool === id} onClick={() => p.set({ tool: id })}>
            {label}
          </Button>
        ))}
      </div>

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
          {tool === "wand" && <Range label="Tolerance" value={p.tolerance} min={0} max={255} onChange={(tolerance) => p.set({ tolerance })} />}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-1">
        <Button size="sm" variant="outline" onClick={() => p.set({ zoom: Math.max(0.1, p.zoom / 1.25) })}>
          −
        </Button>
        <span className="tabular-nums">
          {Math.round(p.zoom * 100)}% / {p.rotate}°
        </span>
        <Button size="sm" variant="outline" onClick={() => p.set({ zoom: Math.min(8, p.zoom * 1.25) })}>
          +
        </Button>
        <Button size="sm" variant="outline" onClick={() => p.set({ rotate: (p.rotate + 15) % 360 })}>
          Rotate 15°
        </Button>
        <Button size="sm" variant="outline" onClick={p.resetView}>
          Reset view
        </Button>
        <Button size="sm" variant="outline" onClick={() => void clearSelection()}>
          Deselect
        </Button>
      </div>

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
      <p className="text-muted-foreground">Rulers, guides, snapping and the navigator are view aids kept in the browser only.</p>
    </div>
  );
}
