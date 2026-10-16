import { useEffect, useRef, useState } from "react";
import { Copy, FilePlus2, FolderOpen, GripVertical, Image as ImageIcon, Plus, Trash2, Type as TypeIcon } from "lucide-react";
import { api } from "@/lib/api";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { ARTWORK_SOURCE_DRAG_TYPE } from "@/lib/artworkDrag";
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
import { findNode, pageOf } from "@/lib/model";
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
            <button
              type="button"
              draggable
              aria-label={`Drag page ${p.name ?? p.id} as artwork`}
              title="Drag onto a mockup artwork slot"
              className="grid size-12 shrink-0 cursor-grab place-items-center rounded text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:cursor-grabbing"
              onClick={(event) => { event.stopPropagation(); setPage(p.id); }}
              onDragStart={(event) => {
                event.stopPropagation();
                event.dataTransfer.effectAllowed = "copy";
                event.dataTransfer.setData(ARTWORK_SOURCE_DRAG_TYPE, JSON.stringify({ kind: "page", page: p.id }));
              }}
            ><GripVertical size={14} aria-hidden="true" /></button>
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

function DielineProductionRole() {
  const studio = useStudio();
  const page = pageOf(studio.doc, studio.pageId);
  const dielineId = page?.dieline_layout?.dieline;
  const selected = studio.doc && findNode(page, studio.selectedId)?.node;
  const path = selected?.kind === "path" ? selected : null;
  const doc = studio.doc;
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [creaseId, setCreaseId] = useState("");
  const [creaseAngle, setCreaseAngle] = useState(90);
  const [creaseDirection, setCreaseDirection] = useState<"mountain" | "valley">("valley");
  const currentRole = path?.dieline_path_role;
  const currentCrease = path?.dieline_crease_id;
  if (!doc || !dielineId || !path) return null;

  const setRole = async (role: "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap") => {
    if (busy) return;
    setBusy(true);
    setMessage(null);
    try {
      const result = await api.scene3dPathRole({ document: doc, dieline: dielineId, node: path.id, role });
      studio.replaceDoc(result.document, "Assign packaging production role");
      setMessage({ text: `${role} operation added to the ${result.result.spot} plate.` });
    } catch (error) {
      setMessage({ text: error instanceof Error ? error.message : "The production role could not be assigned.", error: true });
    } finally {
      setBusy(false);
    }
  };

  const createCrease = async () => {
    if (busy || !creaseId.trim()) return;
    setBusy(true);
    setMessage(null);
    try {
      const result = await api.scene3dPathCrease({ document: doc, dieline: dielineId, node: path.id, crease: creaseId.trim(), angle_deg: creaseAngle, direction: creaseDirection });
      studio.replaceDoc(result.document, "Create packaging fold edge");
      setMessage({ text: `Validated shared panel edge added as ${creaseDirection} fold ${result.result.crease}.` });
    } catch (error) {
      setMessage({ text: error instanceof Error ? error.message : "The path could not be used as a structural fold edge.", error: true });
    } finally {
      setBusy(false);
    }
  };

  return <section className="space-y-1.5">
    <h3 className={heading}>Packaging path</h3>
    <p className="text-[11px] text-muted-foreground">Assign this editable path to a print/finishing operation. Cut lines are through-cuts; crease marks are scores. To drive the fold preview, create a separate structural hinge on a validated shared panel edge.</p>
    {typeof currentRole === "string" && <p className="text-xs">Current operation: <span className="font-medium">{currentRole}</span></p>}
    {typeof currentCrease === "string" && <p className="text-xs">Structural fold edge: <span className="font-medium">{currentCrease}</span></p>}
    {!currentCrease && <label className="block text-xs">Production role
      <select className={`${input} min-h-10 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2`} value="" disabled={busy} aria-busy={busy} onChange={(event) => {
        if (event.target.value) void setRole(event.target.value as "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap");
      }}>
        <option value="">Choose operation…</option>
        <option value="cut">Cut · through-cut</option>
        <option value="crease">Crease · score line</option>
        <option value="perforation">Perforation · tear line</option>
        <option value="bleed">Bleed guide</option>
        <option value="safe-zone">Safe zone · not cut</option>
        <option value="glue-flap">Glue flap guide</option>
      </select>
    </label>}
    {!currentRole && !currentCrease && <fieldset className="space-y-1.5 rounded border p-2">
      <legend className="px-1 text-[11px] font-medium">Create structural fold edge</legend>
      <p className="text-[11px] text-muted-foreground">The path must exactly match the shared edge of two panels. Freeform score marks stay non-structural.</p>
      <label className="block text-xs">Stable fold ID
        <input className={`${input} min-h-10`} value={creaseId} maxLength={128} placeholder="e.g. front-left-fold" disabled={busy} onChange={(event) => setCreaseId(event.target.value)} />
      </label>
      <div className="grid grid-cols-2 gap-2">
        <label className="text-xs">Target angle · degrees
          <input className={`${input} min-h-10`} type="number" min={-180} max={180} step={1} value={creaseAngle} disabled={busy} onChange={(event) => setCreaseAngle(Number(event.target.value))} />
        </label>
        <label className="text-xs">Fold direction
          <select className={`${input} min-h-10`} value={creaseDirection} disabled={busy} onChange={(event) => setCreaseDirection(event.target.value as "mountain" | "valley")}>
            <option value="valley">Valley</option><option value="mountain">Mountain</option>
          </select>
        </label>
      </div>
      <Button type="button" variant="outline" className="min-h-10 w-full" disabled={busy || !creaseId.trim() || !Number.isFinite(creaseAngle)} aria-busy={busy} onClick={() => void createCrease()}>
        {busy ? "Validating fold edge…" : "Create fold edge"}
      </Button>
    </fieldset>}
    {message && <p role={message.error ? "alert" : "status"} className={`break-words text-[11px] ${message.error ? "text-destructive" : "text-muted-foreground"}`}>{message.text}</p>}
  </section>;
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
      <DielineProductionRole />
      <Defaults />
      <Insert />
      <Assets />
    </div>
  );
}

export { FilePlus2 };
