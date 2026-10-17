import { ChevronRight, Eye, EyeOff, GripVertical, Lock, Plus, Trash2, Unlock } from "lucide-react";
import { useState } from "react";
import type { SceneNode } from "@/lib/api";
import { useStudio } from "@/store";
import { cn } from "@/lib/utils";
import { addLayer, patchLayer, removeLayer } from "@/lib/actions";
import { children, layerNodes, nodeKind, pageOf } from "@/lib/model";
import { Button } from "@/components/ui/button";
import { ARTWORK_SOURCE_DRAG_TYPE } from "@/lib/artworkDrag";

function Row({ node, depth, layerId, locked }: { node: SceneNode; depth: number; layerId: string; locked: boolean }) {
  const { selectedId, select, doc, pageId } = useStudio();
  const [open, setOpen] = useState(depth < 1);
  const kids = children(node);
  const layer = pageOf(doc, pageId)?.layers?.find((l) => l.id === layerId);
  const kind = nodeKind(node, layer);
  return (
    <li>
      <div
        className={cn(
          "flex cursor-default items-center gap-1 rounded px-1 py-0.5 hover:bg-accent",
          selectedId === node.id && "bg-accent font-medium",
          locked && "opacity-60",
        )}
        style={{ paddingLeft: depth * 12 + 4 }}
        onClick={() => select(node.id)}
      >
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
        <span className="truncate">{typeof node.name === "string" && node.name ? node.name : node.id}</span>
        <button
          type="button"
          draggable
          disabled={node.kind === "scene3d"}
          aria-label={`Drag ${typeof node.name === "string" && node.name ? node.name : node.id} artwork source`}
          title={node.kind === "scene3d" ? "3D objects cannot be linked as artwork" : "Drag onto a mockup artwork slot"}
          className="grid size-12 shrink-0 cursor-grab place-items-center rounded text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-30"
          onClick={(event) => { event.stopPropagation(); select(node.id); }}
          onDragStart={(event) => {
            event.stopPropagation();
            event.dataTransfer.effectAllowed = "copy";
            event.dataTransfer.setData(ARTWORK_SOURCE_DRAG_TYPE, JSON.stringify({ kind: "node", page: pageId, node: node.id }));
          }}
        ><GripVertical size={14} aria-hidden="true" /></button>
        <span className="ml-auto text-[10px] text-muted-foreground">{kind}</span>
      </div>
      {open && kids.length > 0 && (
        <ul>
          {kids.map((child) => (
            <Row key={child.id} node={child} depth={depth + 1} layerId={layerId} locked={locked} />
          ))}
        </ul>
      )}
    </li>
  );
}

export function Outline() {
  const { doc, pageId, layerId, setLayer, setPage } = useStudio();
  const page = pageOf(doc, pageId);
  if (!page) return <p className="text-muted-foreground">No page loaded.</p>;
  const layers = [...(page.layers ?? [])].reverse();
  return (
    <div className="space-y-2">
      {(doc?.pages.length ?? 0) > 0 && <section aria-label="Page artwork sources" className="space-y-1">
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Pages · drag to mockup</h3>
        <ul className="space-y-0.5">
          {doc?.pages.map((page) => <li key={page.id} className="flex min-w-0 items-center gap-1 rounded px-1 py-0.5">
            <button type="button" className="min-w-0 flex-1 truncate rounded px-1 text-left text-xs hover:bg-accent focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2" onClick={() => setPage(page.id)}>
              {page.name ?? page.id}
            </button>
            <button
              type="button"
              draggable
              aria-label={`Drag page ${page.name ?? page.id} as artwork`}
              title="Drag onto a mockup artwork slot"
              className="grid size-12 shrink-0 cursor-grab place-items-center rounded text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:cursor-grabbing"
              onClick={(event) => { event.stopPropagation(); setPage(page.id); }}
              onDragStart={(event) => {
                event.dataTransfer.effectAllowed = "copy";
                event.dataTransfer.setData(ARTWORK_SOURCE_DRAG_TYPE, JSON.stringify({ kind: "page", page: page.id }));
              }}
            ><GripVertical size={14} aria-hidden="true" /></button>
          </li>)}
        </ul>
      </section>}
      <div className="flex items-center justify-between">
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Layers</h3>
        <Button variant="ghost" size="icon" aria-label="Add layer" onClick={addLayer}>
          <Plus size={14} />
        </Button>
      </div>
      <ul className="space-y-2">
        {layers.map((layer) => (
          <li key={layer.id}>
            <div className={cn("flex items-center gap-1 rounded px-1 py-0.5", layer.id === layerId && "bg-primary/10")}>
              <button aria-label={layer.visible === false ? "Show layer" : "Hide layer"} onClick={() => patchLayer(layer.id, { visible: layer.visible === false })}>
                {layer.visible === false ? <EyeOff size={13} /> : <Eye size={13} />}
              </button>
              <button aria-label={layer.locked ? "Unlock layer" : "Lock layer"} onClick={() => patchLayer(layer.id, { locked: !layer.locked })}>
                {layer.locked ? <Lock size={13} /> : <Unlock size={13} className="opacity-40" />}
              </button>
              <button className="min-w-0 flex-1 truncate text-left text-xs font-semibold" onClick={() => setLayer(layer.id)} onDoubleClick={() => {
                const next = window.prompt("Layer name", layer.name ?? layer.id);
                if (next) patchLayer(layer.id, { name: next });
              }}>
                {layer.name ?? layer.id}
              </button>
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
