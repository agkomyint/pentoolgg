import { useEffect, useRef, useState } from "react";
import { Copy, FilePlus2, FolderOpen, Image as ImageIcon, Plus, Trash2, Type as TypeIcon } from "lucide-react";
import { api } from "@/lib/api";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import {
  addImage,
  addPage,
  duplicatePage,
  embedFont,
  importPen,
  insertAsset,
  removePage,
  renamePage,
  resizeCanvas,
} from "@/lib/actions";
import { pageOf } from "@/lib/model";
import { cn } from "@/lib/utils";

const input = "mt-0.5 h-7 w-full rounded border bg-card px-1.5 text-xs";
const heading = "text-[11px] font-semibold uppercase tracking-wide text-muted-foreground";

function Pages() {
  const { doc, pageId, setPage } = useStudio();
  return (
    <section className="space-y-1.5">
      <div className="flex items-center justify-between">
        <h3 className={heading}>Pages</h3>
        <div className="flex">
          <Button variant="ghost" size="icon" aria-label="Add page" onClick={addPage}>
            <Plus size={14} />
          </Button>
          <Button variant="ghost" size="icon" aria-label="Duplicate page" onClick={duplicatePage}>
            <Copy size={14} />
          </Button>
        </div>
      </div>
      <ul className="space-y-0.5">
        {doc?.pages.map((p) => (
          <li key={p.id} className={cn("flex items-center gap-1 rounded px-1.5 py-1 hover:bg-accent", p.id === pageId && "bg-accent font-medium")}>
            <button className="min-w-0 flex-1 truncate text-left" onClick={() => setPage(p.id)}>
              {p.name ?? p.id}
            </button>
            <button
              aria-label="Rename page"
              className="text-[11px] text-muted-foreground"
              onClick={() => {
                const next = window.prompt("Page name", p.name ?? p.id);
                if (next) renamePage(p.id, next);
              }}
            >
              rename
            </button>
            <button aria-label="Delete page" disabled={(doc?.pages.length ?? 0) <= 1} onClick={() => removePage(p.id)} className="opacity-60 hover:opacity-100 disabled:opacity-20">
              <Trash2 size={12} />
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function CanvasSettings() {
  const { doc, pageId } = useStudio();
  const page = pageOf(doc, pageId);
  const canvas = page?.canvas as { width: number; height: number; background?: string } | undefined;
  const [w, setW] = useState(1200);
  const [h, setH] = useState(800);
  useEffect(() => {
    if (canvas) {
      setW(canvas.width);
      setH(canvas.height);
    }
  }, [canvas?.width, canvas?.height, pageId]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!canvas) return null;
  const bg = typeof canvas.background === "string" && /^#[0-9a-f]{6}/i.test(canvas.background) ? canvas.background.slice(0, 7) : "#ffffff";
  return (
    <section className="space-y-1.5">
      <h3 className={heading}>Canvas</h3>
      <div className="grid grid-cols-3 gap-2">
        <label className="text-xs">
          Width
          <input className={input} type="number" min={1} max={16384} value={w} onChange={(e) => setW(Number(e.target.value))} onBlur={() => w !== canvas.width && resizeCanvas(w, h)} />
        </label>
        <label className="text-xs">
          Height
          <input className={input} type="number" min={1} max={16384} value={h} onChange={(e) => setH(Number(e.target.value))} onBlur={() => h !== canvas.height && resizeCanvas(w, h)} />
        </label>
        <label className="text-xs">
          Background
          <input className="mt-0.5 h-7 w-full rounded border bg-card" type="color" value={bg} onChange={(e) => resizeCanvas(canvas.width, canvas.height, e.target.value)} />
        </label>
      </div>
    </section>
  );
}

function Defaults() {
  const { look, setLook } = useStudio();
  return (
    <section className="space-y-1.5">
      <h3 className={heading}>New object style</h3>
      <div className="grid grid-cols-3 gap-2">
        <label className="text-xs">
          Fill
          <input className="mt-0.5 h-7 w-full rounded border bg-card" type="color" value={look.fill} onChange={(e) => setLook({ fill: e.target.value })} />
        </label>
        <label className="text-xs">
          Stroke
          <input className="mt-0.5 h-7 w-full rounded border bg-card" type="color" value={look.stroke} onChange={(e) => setLook({ stroke: e.target.value })} />
        </label>
        <label className="text-xs">
          Width
          <input className={input} type="number" min={0} step={0.5} value={look.strokeWidth} onChange={(e) => setLook({ strokeWidth: Number(e.target.value) })} />
        </label>
      </div>
    </section>
  );
}

function Insert() {
  const { notify } = useStudio();
  const imageInput = useRef<HTMLInputElement>(null);
  const fontInput = useRef<HTMLInputElement>(null);
  const penInput = useRef<HTMLInputElement>(null);
  const [opts, setOpts] = useState({ x: 0, y: 0, scale: 1, rotation: 0, prefix: "", expand: false });
  const [families, setFamilies] = useState<string[]>([]);
  useEffect(() => {
    api.fonts().then((f) => setFamilies([...new Set(f.faces.map((x) => x.family))])).catch(() => undefined);
  }, []);
  return (
    <section className="space-y-2">
      <h3 className={heading}>Insert</h3>
      <datalist id="studio-fonts">
        {families.map((f) => (
          <option key={f} value={f} />
        ))}
      </datalist>
      <div className="flex flex-wrap gap-1.5">
        <Button variant="outline" size="sm" onClick={() => imageInput.current?.click()}>
          <ImageIcon size={12} /> Image...
        </Button>
        <Button variant="outline" size="sm" onClick={() => fontInput.current?.click()}>
          <TypeIcon size={12} /> Embed font...
        </Button>
        <Button variant="outline" size="sm" onClick={() => penInput.current?.click()}>
          <FolderOpen size={12} /> Import .pen...
        </Button>
      </div>
      <p className="text-[11px] text-muted-foreground">Tip: paste (Ctrl+V) or drop an image onto the canvas.</p>
      <input ref={imageInput} hidden type="file" accept="image/png,image/jpeg,image/webp" onChange={(e) => {
        const file = e.target.files?.[0];
        if (file) void addImage(file, file.name);
        e.target.value = "";
      }} />
      <input ref={fontInput} hidden type="file" accept=".ttf,.otf,.woff,.woff2" onChange={(e) => {
        const file = e.target.files?.[0];
        if (file) void embedFont(file);
        e.target.value = "";
      }} />
      <input ref={penInput} hidden type="file" accept=".pen,application/json" onChange={async (e) => {
        const file = e.target.files?.[0];
        e.target.value = "";
        if (!file) return;
        try {
          if (file.size > 64 * 1024 * 1024) throw new Error("Import is limited to 64 MiB.");
          await importPen(JSON.parse(await file.text()), opts);
        } catch (error) {
          notify(error instanceof Error ? error.message : "Import failed.", true);
        }
      }} />
      <details className="text-xs">
        <summary className="cursor-pointer text-muted-foreground">Import placement</summary>
        <div className="mt-1 grid grid-cols-2 gap-2">
          {(["x", "y", "scale", "rotation"] as const).map((key) => (
            <label key={key}>
              {key}
              <input className={input} type="number" value={opts[key]} step={key === "scale" ? 0.1 : 1} onChange={(e) => setOpts({ ...opts, [key]: Number(e.target.value) })} />
            </label>
          ))}
          <label className="col-span-2">
            ID prefix
            <input className={input} value={opts.prefix} onChange={(e) => setOpts({ ...opts, prefix: e.target.value })} />
          </label>
          <label className="col-span-2 flex items-center gap-1.5">
            <input type="checkbox" checked={opts.expand} onChange={(e) => setOpts({ ...opts, expand: e.target.checked })} /> Expand canvas to fit
          </label>
        </div>
      </details>
    </section>
  );
}

function Assets() {
  const [query, setQuery] = useState("");
  const [items, setItems] = useState<{ id: string; name: string; library: string; version: string }[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const timer = window.setTimeout(() => {
      api
        .assets(query)
        .then((r) => {
          setItems(r.assets);
          setError(null);
        })
        .catch((e: unknown) => setError(e instanceof Error ? e.message : "Search failed."));
    }, 150);
    return () => window.clearTimeout(timer);
  }, [query]);
  return (
    <section className="space-y-1.5">
      <h3 className={heading}>Assets</h3>
      <input className={input} placeholder="Search installed libraries" value={query} onChange={(e) => setQuery(e.target.value)} />
      {error && <p className="text-xs text-destructive">{error}</p>}
      <ul className="space-y-1">
        {items.map((a) => (
          <li key={`${a.library}/${a.id}`} className="flex items-center gap-2 rounded border bg-card px-2 py-1">
            <span className="min-w-0 flex-1">
              <strong className="block truncate text-xs">{a.name}</strong>
              <small className="block truncate text-[10px] text-muted-foreground">
                {a.library} · {a.id} · {a.version}
              </small>
            </span>
            <Button size="sm" variant="outline" onClick={() => void insertAsset(`${a.library}/${a.id}`)}>
              Add
            </Button>
          </li>
        ))}
        {!items.length && !error && <li className="text-xs text-muted-foreground">No matching assets.</li>}
      </ul>
    </section>
  );
}

export function DesignPanel() {
  const { doc } = useStudio();
  if (!doc) return <p className="text-muted-foreground">Loading...</p>;
  return (
    <div className="space-y-5">
      <Pages />
      <CanvasSettings />
      <Defaults />
      <Insert />
      <Assets />
    </div>
  );
}

export { FilePlus2 };
