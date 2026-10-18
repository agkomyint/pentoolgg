import { ChevronRight, Eye, EyeOff, GripVertical, Lock, Plus, Trash2, Unlock } from "lucide-react";
import { useState } from "react";
import type { SceneNode } from "@/lib/api";
import { useStudio } from "@/store";
import { usePaint } from "@/workspaces/rasterStore";
import { cn } from "@/lib/utils";
import { addLayer, moveInStack, patchLayer, removeLayer, renameNode } from "@/lib/actions";
import { children, layerNodes, nodeKind, pageOf } from "@/lib/model";
import { Button } from "@/components/ui/button";
import { ARTWORK_SOURCE_DRAG_TYPE } from "@/lib/artworkDrag";

/** Drag payload for reordering objects in the layers list. */
const STACK_DRAG_TYPE = "application/x-pentool-stack";
type Drop = "above" | "below" | "into";

/** Name field shown in place of a label while renaming; Enter or blur commits, Esc cancels. */
function RenameField({ value, onDone }: { value: string; onDone: (next: string | null) => void }) {
  return (
    <input
      autoFocus
      aria-label="Name"
      defaultValue={value}
      className="min-w-0 flex-1 rounded border bg-background px-1 text-xs outline-none ring-1 ring-primary"
      onFocus={(e) => e.currentTarget.select()}
      onClick={(e) => e.stopPropagation()}
      onBlur={(e) => onDone(e.currentTarget.value.trim() || null)}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") e.currentTarget.blur();
        else if (e.key === "Escape") {
          e.currentTarget.value = "";
          e.currentTarget.blur();
        }
      }}
    />
  );
}

function Row({ node, depth, layerId, locked }: { node: SceneNode; depth: number; layerId: string; locked: boolean }) {
  const { selectedIds, select, doc, pageId, hoverId, setHover } = useStudio();
  const chosen = selectedIds.includes(node.id);
  const [open, setOpen] = useState(depth < 1);
  const [renaming, setRenaming] = useState(false);
  const [drop, setDrop] = useState<Drop | null>(null);
  const pick = (toggle = false) => { select(node.id, toggle ? "toggle" : "replace"); if (node.kind === "raster") usePaint.getState().set({ layerId: node.id }); };
  const kids = children(node);
  const layer = pageOf(doc, pageId)?.layers?.find((l) => l.id === layerId);
  const kind = nodeKind(node, layer);
  const label = typeof node.name === "string" && node.name ? node.name : node.id;
  const zone = (e: React.DragEvent<HTMLElement>): Drop => {
    const r = e.currentTarget.getBoundingClientRect();
    const f = (e.clientY - r.top) / r.height;
    if (node.kind === "group" && f > 0.25 && f < 0.75) return "into";
    return f < 0.5 ? "above" : "below";
  };
  const startDrag = (e: React.DragEvent<HTMLElement>) => {
    e.stopPropagation();
    e.dataTransfer.effectAllowed = "copyMove";
    e.dataTransfer.setData(STACK_DRAG_TYPE, node.id);
    e.dataTransfer.setData(ARTWORK_SOURCE_DRAG_TYPE, JSON.stringify({ kind: "node", page: pageId, node: node.id }));
  };
  return (
    <li>
      <div
        data-node={node.id}
        draggable={!renaming && !locked}
        onDragStart={startDrag}
        onDragOver={(e) => {
          if (!e.dataTransfer.types.includes(STACK_DRAG_TYPE) || locked) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          setDrop(zone(e));
        }}
        onDragLeave={() => setDrop(null)}
        onDrop={(e) => {
          const id = e.dataTransfer.getData(STACK_DRAG_TYPE);
          setDrop(null);
          if (!id) return;
          e.preventDefault();
          e.stopPropagation();
          moveInStack(id, { node: node.id, where: zone(e) });
          if (zone(e) === "into") setOpen(true);
        }}
        className={cn(
          "relative flex cursor-default items-center gap-1 rounded px-1 py-0.5 hover:bg-accent",
          chosen && "bg-accent font-medium",
          !chosen && hoverId === node.id && "bg-accent/60",
          locked && "opacity-60",
          drop === "into" && "ring-2 ring-inset ring-primary",
        )}
        style={{ paddingLeft: depth * 12 + 4 }}
        onPointerEnter={() => setHover(node.id)}
        onPointerLeave={() => setHover(null)}
        onClick={(event) => pick(event.shiftKey || event.ctrlKey || event.metaKey)}
      >
        {drop === "above" && <span aria-hidden="true" className="pointer-events-none absolute inset-x-0 top-0 h-0.5 bg-primary" />}
        {drop === "below" && <span aria-hidden="true" className="pointer-events-none absolute inset-x-0 bottom-0 h-0.5 bg-primary" />}
        {kids.length > 0 ? (
          <button
            aria-label={open ? "Collapse" : "Expand"}
            onClick={(e) => {
              e.stopPropagation();
              setOpen(!open);
            }}
          >
            <ChevronRight size={12} className={cn("transition-transform", open && "rotate-90")} />
          </button>
        ) : (
          <span className="w-3" />
        )}
        {renaming ? (
          <RenameField value={label} onDone={(next) => { setRenaming(false); if (next && next !== label) renameNode(node.id, next); }} />
        ) : (
          <button type="button" className="min-w-0 flex-1 truncate text-left" aria-pressed={chosen} title="Double-click to rename"
            onClick={(event) => { event.stopPropagation(); pick(event.shiftKey || event.ctrlKey || event.metaKey); }}
            onDoubleClick={(event) => { event.stopPropagation(); if (!locked) setRenaming(true); }}
            onKeyDown={(event) => { if (event.key === "F2" && !locked) { event.preventDefault(); setRenaming(true); } }}>{label}</button>
        )}
        <button
          type="button"
          draggable
          disabled={node.kind === "scene3d"}
          aria-label={`Drag ${label} artwork source`}
          title={node.kind === "scene3d" ? "3D objects cannot be linked as artwork" : "Drag to reorder, or onto a mockup artwork slot"}
          className="grid size-7 shrink-0 cursor-grab place-items-center rounded text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-30"
          onClick={(event) => { event.stopPropagation(); pick(); }}
          onDragStart={startDrag}
        ><GripVertical size={14} aria-hidden="true" /></button>
        <span className="ml-auto text-[10px] text-muted-foreground">{kind}</span>
      </div>
      {open && kids.length > 0 && (
        <ul>
          {[...kids].reverse().map((child) => (
            <Row key={child.id} node={child} depth={depth + 1} layerId={layerId} locked={locked} />
          ))}
        </ul>
      )}
    </li>
  );
}

export function Outline() {
  const { doc, pageId, layerId, setLayer } = useStudio();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dropLayer, setDropLayer] = useState<string | null>(null);
  const page = pageOf(doc, pageId);
  if (!page) return <p className="text-muted-foreground">No page loaded.</p>;
  const layers = [...(page.layers ?? [])].reverse();
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Layers</h3>
        <Button variant="ghost" size="icon" aria-label="Add layer" onClick={addLayer}>
          <Plus size={14} />
        </Button>
      </div>
      <ul className="space-y-2">
        {layers.map((layer) => (
          <li key={layer.id}>
            <div
              className={cn("flex items-center gap-1 rounded px-1 py-0.5", layer.id === layerId && "bg-primary/10", dropLayer === layer.id && "ring-2 ring-inset ring-primary")}
              onDragOver={(e) => {
                if (!e.dataTransfer.types.includes(STACK_DRAG_TYPE) || layer.locked) return;
                e.preventDefault();
                e.dataTransfer.dropEffect = "move";
                setDropLayer(layer.id);
              }}
              onDragLeave={() => setDropLayer(null)}
              onDrop={(e) => {
                const id = e.dataTransfer.getData(STACK_DRAG_TYPE);
                setDropLayer(null);
                if (!id) return;
                e.preventDefault();
                moveInStack(id, { layer: layer.id });
              }}
            >
              <button aria-label={layer.visible === false ? "Show layer" : "Hide layer"} onClick={() => patchLayer(layer.id, { visible: layer.visible === false })}>
                {layer.visible === false ? <EyeOff size={13} /> : <Eye size={13} />}
              </button>
              <button aria-label={layer.locked ? "Unlock layer" : "Lock layer"} onClick={() => patchLayer(layer.id, { locked: !layer.locked })}>
                {layer.locked ? <Lock size={13} /> : <Unlock size={13} className="opacity-40" />}
              </button>
              {renaming === layer.id ? (
                <RenameField value={layer.name ?? layer.id} onDone={(next) => { setRenaming(null); if (next && next !== (layer.name ?? layer.id)) patchLayer(layer.id, { name: next }); }} />
              ) : (
                <button className="min-w-0 flex-1 truncate text-left text-xs font-semibold" title="Double-click to rename" onClick={() => setLayer(layer.id)} onDoubleClick={() => setRenaming(layer.id)}>
                  {layer.name ?? layer.id}
                </button>
              )}
              <button aria-label="Delete layer" disabled={(page.layers?.length ?? 0) <= 1} onClick={() => removeLayer(layer.id)} className="opacity-60 hover:opacity-100 disabled:opacity-20">
                <Trash2 size={12} />
              </button>
            </div>
            <ul>
              {[...layerNodes(layer)].reverse().map((node) => (
                <Row key={node.id} node={node} depth={0} layerId={layer.id} locked={!!layer.locked} />
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </div>
  );
}
