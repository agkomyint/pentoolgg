import {
  Aperture, Blend, Brush, Camera, ChartColumn, ChevronRight, Circle, Component, Contrast, CornerLeftDown, Droplet, Eye, EyeOff,
  Folder, FolderPlus, GripVertical, Image, Layers, Lock, Palette, PenTool, Plus, Rainbow, Scale, SlidersHorizontal, Spline, Square,
  SquareDashed, Sun, Trash2, Type, Unlock, type LucideIcon,
} from "lucide-react";
import { useState } from "react";
import type { SceneNode } from "@/lib/api";
import { useStudio } from "@/store";
import { usePaint } from "@/workspaces/rasterStore";
import { cn } from "@/lib/utils";
import { addLayer, deleteSelected, duplicateSelected, groupSelected, moveInStack, patchLayer, removeLayer, renameNode, runScene } from "@/lib/actions";
import { children, findNode, isScene, layerNodes, nodeKind, pageOf } from "@/lib/model";
import {
  ADJUSTMENTS, BLEND_MODES, COMPOSITE_VERSION, addAdjustment, adjustmentLabel, clipBaseOf, hasFill, maskWithShape, previewComposite,
  setComposite, toggleClip, toggleVisible,
} from "@/lib/layers";
import { Button } from "@/components/ui/button";
import { ARTWORK_SOURCE_DRAG_TYPE } from "@/lib/artworkDrag";

const KIND_ICONS: Record<string, LucideIcon> = {
  group: Folder, rect: Square, ellipse: Circle, line: PenTool, path: Spline, text: Type, image: Image, photo: Camera,
  raster: Brush, instance: Component, fill: Droplet,
};
const ADJUSTMENT_ICONS: Record<string, LucideIcon> = {
  "brightness-contrast": Sun, levels: ChartColumn, curves: Spline, exposure: Aperture, vibrance: Droplet, "hue-saturation": Palette,
  "color-balance": Scale, "black-and-white": Contrast, "channel-mixer": Blend, invert: Contrast, posterize: Layers, threshold: Contrast,
  "gradient-map": Rainbow,
};

function iconFor(node: SceneNode, kind: string): LucideIcon {
  if (node.kind === "adjustment") return ADJUSTMENT_ICONS[String(node.adjustment)] ?? SlidersHorizontal;
  return KIND_ICONS[kind] ?? Square;
}

type MenuItem = { label: string; run: () => void; disabled?: boolean; danger?: boolean } | "-";

/** Small popup menu used for the row context menu and the adjustment menu. */
function PopupMenu({ at, items, label, onClose }: { at: { x: number; y: number }; items: MenuItem[]; label: string; onClose: () => void }) {
  return (
    <>
      <div className="fixed inset-0 z-40" onPointerDown={onClose} onContextMenu={(e) => { e.preventDefault(); onClose(); }} />
      <div role="menu" aria-label={label} className="fixed z-50 grid min-w-52 gap-0.5 rounded-md border bg-card p-1 text-xs shadow-lg"
        style={{ left: Math.min(at.x, window.innerWidth - 220), top: Math.max(8, Math.min(at.y, window.innerHeight - 24 - items.length * 28)) }}
        onKeyDown={(e) => e.key === "Escape" && onClose()}>
        {items.map((item, i) => item === "-"
          ? <div key={i} role="separator" className="my-0.5 h-px bg-border" />
          : (
            <button key={item.label} type="button" role="menuitem" disabled={item.disabled} autoFocus={i === 0}
              className={cn("rounded px-2 py-1.5 text-left hover:bg-accent disabled:opacity-40", item.danger && "text-destructive")}
              onClick={() => { onClose(); item.run(); }}>{item.label}</button>
          ))}
      </div>
    </>
  );
}

const SHAPES = new Set(["rect", "ellipse", "path", "line", "text", "group"]);

const adjustmentItems = (): MenuItem[] => ADJUSTMENTS.map((a) => ({ label: `${a.label}...`, run: () => void addAdjustment(a.kind) }));

/** Percent field with live preview while typing and one undo step on commit. */
function Percent({ label, value, onLive, onCommit, disabled }: { label: string; value: number; onLive: (v: number) => void; onCommit: (v: number) => void; disabled?: boolean }) {
  const [text, setText] = useState<string | null>(null);
  const shown = text ?? String(Math.round(value * 100));
  const commit = () => {
    if (text === null) return;
    const n = Math.min(100, Math.max(0, Number(text)));
    setText(null);
    useStudio.getState().setPreview(null);
    if (Number.isFinite(n) && n / 100 !== value) onCommit(n / 100);
  };
  return (
    <label className="flex items-center gap-1 text-[11px] text-muted-foreground" title={label === "Fill" ? "Fill fades the pixels but keeps effects at full strength" : "Opacity fades the layer and its effects"}>
      {label}
      <input aria-label={label} disabled={disabled} inputMode="numeric" className="h-6 w-11 rounded border bg-background px-1 text-right text-[11px] tabular-nums text-foreground disabled:opacity-40"
        value={shown}
        onChange={(e) => {
          setText(e.target.value);
          const n = Number(e.target.value);
          if (Number.isFinite(n) && n >= 0 && n <= 100) onLive(n / 100);
        }}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "ArrowUp" || e.key === "ArrowDown") {
            e.preventDefault();
            const n = Math.min(100, Math.max(0, Number(shown) + (e.key === "ArrowUp" ? 1 : -1) * (e.shiftKey ? 10 : 1)));
            setText(String(n));
            onLive(n / 100);
          }
        }} />
      %
    </label>
  );
}

/** Photoshop's strip above the layer list: blend mode, Opacity and Fill of the selected layer. */
function CompositeBar() {
  const { doc, pageId, selectedId } = useStudio();
  const found = findNode(pageOf(doc, pageId), selectedId);
  const node = found?.node;
  const ready = !!node && isScene(doc) && !found.layer.locked;
  const composite = (doc?.version ?? 0) >= COMPOSITE_VERSION;
  const passThrough = node?.kind === "group" && node.isolation === "pass-through";
  const mode = passThrough ? "pass-through" : String(node?.blend_mode ?? "normal");
  const opacity = typeof node?.opacity === "number" ? node.opacity : 1;
  const fill = typeof node?.content_opacity === "number" ? node.content_opacity : 1;
  return (
    <div className="flex flex-wrap items-center gap-1.5 rounded border bg-muted/30 p-1">
      <select aria-label="Blend mode" disabled={!ready} value={mode} title={composite ? "Blend mode" : "Choosing a blend mode upgrades the file to format v6"}
        className="h-6 min-w-0 flex-1 rounded border bg-background px-1 text-[11px] capitalize disabled:opacity-40"
        onChange={(e) => {
          if (!node) return;
          const next = e.target.value;
          if (node.kind === "group") {
            setComposite(node.id, next === "pass-through"
              ? { isolation: "pass-through", blend_mode: undefined }
              : { isolation: "isolated", blend_mode: next === "normal" ? undefined : next }, "Blend mode");
          } else setComposite(node.id, { blend_mode: next === "normal" && node.kind !== "adjustment" ? undefined : next }, "Blend mode");
        }}>
        {node?.kind === "group" && <option value="pass-through">Pass through</option>}
        {BLEND_MODES.map((m) => <option key={m} value={m}>{m.replace(/-/g, " ")}</option>)}
      </select>
      <Percent label="Opacity" value={opacity} disabled={!ready}
        onLive={(v) => node && previewComposite(node.id, { opacity: v })}
        onCommit={(v) => node && setComposite(node.id, { opacity: v === 1 && node.kind !== "adjustment" ? undefined : v }, "Opacity")} />
      <Percent label="Fill" value={fill} disabled={!ready || !hasFill(node)}
        onLive={(v) => node && previewComposite(node.id, { content_opacity: v })}
        onCommit={(v) => node && setComposite(node.id, { content_opacity: v === 1 ? undefined : v }, "Fill")} />
    </div>
  );
}

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

function Row({ node, depth, layerId, locked, isBase }: { node: SceneNode; depth: number; layerId: string; locked: boolean; isBase: boolean }) {
  const { selectedIds, select, doc, pageId, hoverId, setHover } = useStudio();
  const chosen = selectedIds.includes(node.id);
  const [open, setOpen] = useState(depth < 1);
  const [renaming, setRenaming] = useState(false);
  const [drop, setDrop] = useState<Drop | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const pick = (toggle = false) => { select(node.id, toggle ? "toggle" : "replace"); if (node.kind === "raster") usePaint.getState().set({ layerId: node.id }); };
  const kids = children(node);
  const page = pageOf(doc, pageId);
  const layer = page?.layers?.find((l) => l.id === layerId);
  const kind = nodeKind(node, layer);
  const adjustment = node.kind === "adjustment";
  const label = typeof node.name === "string" && node.name ? node.name : adjustment ? adjustmentLabel(node.adjustment) : node.id;
  const clipped = clipBaseOf(node) !== null;
  const hidden = node.visible === false;
  const mask = node.mask as { invert?: boolean } | undefined;
  const effects = Array.isArray(node.effects) && node.effects.length > 0;
  const Icon = iconFor(node, kind);
  const scene = isScene(doc);
  // Another selected vector shape can become this layer's mask.
  const shape = selectedIds.find((id) => id !== node.id && SHAPES.has(String(findNode(page, id)?.node.kind)));
  const menuItems = (): MenuItem[] => [
    { label: clipped ? "Release clipping mask" : "Create clipping mask", run: () => toggleClip(node.id), disabled: locked || !scene },
    "-",
    { label: "Duplicate layer", run: () => { select(node.id); duplicateSelected(); }, disabled: locked },
    { label: "Group layers", run: () => { if (!chosen) select(node.id); groupSelected(); }, disabled: locked || !scene },
    { label: hidden ? "Show layer" : "Hide layer", run: () => toggleVisible(node.id), disabled: locked },
    "-",
    { label: shape ? "Mask with selected shape" : "Mask with shape (also select a shape)", run: () => void (shape && maskWithShape(node.id, shape)), disabled: locked || !shape || !scene },
    { label: "Invert mask", run: () => setComposite(node.id, { mask: { ...mask, invert: !mask?.invert } }, "Invert mask"), disabled: locked || !mask },
    { label: "Delete mask", run: () => void runScene([{ type: "mask-detach", id: node.id }]), disabled: locked || !mask },
    "-",
    { label: "Delete layer", run: () => { select(node.id); deleteSelected(); }, disabled: locked, danger: true },
  ];
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
  // Alt-click clips to the layer below, as in Photoshop.
  const click = (event: React.MouseEvent) => {
    event.stopPropagation();
    if (event.altKey && !locked) return toggleClip(node.id);
    pick(event.shiftKey || event.ctrlKey || event.metaKey);
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
          // Alt-drop clips the layer to the one it lands on, as in Photoshop.
          moveInStack(id, { node: node.id, where: zone(e) }, e.altKey);
          if (zone(e) === "into") setOpen(true);
        }}
        onContextMenu={(e) => {
          e.preventDefault();
          e.stopPropagation();
          if (!chosen) pick();
          setMenu({ x: e.clientX, y: e.clientY });
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
        onClick={click}
      >
        {drop === "above" && <span aria-hidden="true" className="pointer-events-none absolute inset-x-0 top-0 h-0.5 bg-primary" />}
        {drop === "below" && <span aria-hidden="true" className="pointer-events-none absolute inset-x-0 bottom-0 h-0.5 bg-primary" />}
        <button type="button" aria-label={hidden ? `Show ${label}` : `Hide ${label}`} disabled={locked}
          className={cn("grid size-5 shrink-0 place-items-center rounded hover:bg-muted", hidden && "text-muted-foreground")}
          onClick={(e) => { e.stopPropagation(); toggleVisible(node.id); }}>
          {hidden ? <EyeOff size={12} /> : <Eye size={12} />}
        </button>
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
        {clipped && <CornerLeftDown size={12} aria-label="Clipped to the layer below" className="shrink-0 text-muted-foreground" />}
        <Icon size={13} aria-hidden="true" className={cn("shrink-0", adjustment ? "text-primary" : "text-muted-foreground", hidden && "opacity-40")} />
        {renaming ? (
          <RenameField value={label} onDone={(next) => { setRenaming(false); if (next && next !== label) renameNode(node.id, next); }} />
        ) : (
          <button type="button" className={cn("min-w-0 flex-1 truncate text-left", isBase && "underline decoration-dotted underline-offset-2", hidden && "text-muted-foreground")}
            aria-pressed={chosen} title={`${adjustment ? `${adjustmentLabel(node.adjustment)} adjustment` : kind}. Double-click to rename, Alt-click to clip.`}
            onClick={click}
            onDoubleClick={(event) => { event.stopPropagation(); if (!locked) setRenaming(true); }}
            onKeyDown={(event) => { if (event.key === "F2" && !locked) { event.preventDefault(); setRenaming(true); } }}>{label}</button>
        )}
        {effects && <span title="Layer effects" className="shrink-0 rounded border px-0.5 text-[9px] font-semibold italic text-muted-foreground">fx</span>}
        {mask && (
          <span title={mask.invert ? "Layer mask (inverted)" : "Layer mask"}
            className={cn("grid size-5 shrink-0 place-items-center rounded border", mask.invert ? "bg-foreground text-background" : "bg-background")}>
            <SquareDashed size={11} aria-hidden="true" />
          </span>
        )}
        <button
          type="button"
          draggable
          disabled={node.kind === "scene3d"}
          aria-label={`Drag ${label} artwork source`}
          title={node.kind === "scene3d" ? "3D objects cannot be linked as artwork" : "Drag to reorder, or onto a mockup artwork slot"}
          className="grid size-6 shrink-0 cursor-grab place-items-center rounded text-muted-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:cursor-grabbing disabled:cursor-not-allowed disabled:opacity-30"
          onClick={(event) => { event.stopPropagation(); pick(); }}
          onDragStart={startDrag}
        ><GripVertical size={13} aria-hidden="true" /></button>
      </div>
      {menu && <PopupMenu at={menu} items={menuItems()} label={`${label} actions`} onClose={() => setMenu(null)} />}
      {open && kids.length > 0 && <Rows nodes={kids} depth={depth + 1} layerId={layerId} locked={locked} />}
    </li>
  );
}

/** Rows top-first; a layer is underlined as a clipping base when the layer above is clipped to it. */
function Rows({ nodes, depth, layerId, locked }: { nodes: SceneNode[]; depth: number; layerId: string; locked: boolean }) {
  const rows = nodes.map((node, i) => ({ node, isBase: !!nodes[i + 1] && clipBaseOf(nodes[i + 1]!) === node.id }));
  return (
    <ul>
      {rows.reverse().map(({ node, isBase }) => (
        <Row key={node.id} node={node} depth={depth} layerId={layerId} locked={locked} isBase={isBase} />
      ))}
    </ul>
  );
}

export function Outline() {
  const { doc, pageId, layerId, setLayer, selectedId } = useStudio();
  const [renaming, setRenaming] = useState<string | null>(null);
  const [dropLayer, setDropLayer] = useState<string | null>(null);
  const [adjustMenu, setAdjustMenu] = useState<{ x: number; y: number } | null>(null);
  const page = pageOf(doc, pageId);
  if (!page) return <p className="text-muted-foreground">No page loaded.</p>;
  const layers = [...(page.layers ?? [])].reverse();
  const scene = isScene(doc);
  const selected = findNode(page, selectedId);
  const editable = !!selected && !selected.layer.locked;
  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between">
        <h3 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Layers</h3>
        <Button variant="ghost" size="icon" aria-label="Add layer" title="New page layer" onClick={addLayer}>
          <Plus size={14} />
        </Button>
      </div>
      {scene && <CompositeBar />}
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
            <Rows nodes={layerNodes(layer)} depth={0} layerId={layer.id} locked={!!layer.locked} />
          </li>
        ))}
      </ul>
      {scene && (
        <div role="toolbar" aria-label="Layer actions" className="flex items-center gap-0.5 border-t pt-1">
          <Button variant="ghost" size="icon" aria-label="New adjustment layer" title="New adjustment layer (above the selected layer)" aria-haspopup="menu"
            onClick={(e) => { const r = e.currentTarget.getBoundingClientRect(); setAdjustMenu({ x: r.left, y: r.top - 8 - ADJUSTMENTS.length * 28 }); }}>
            <SlidersHorizontal size={14} />
          </Button>
          <Button variant="ghost" size="icon" aria-label="Create or release clipping mask" title="Clip to the layer below (Alt-click a layer)" disabled={!editable}
            onClick={() => selected && toggleClip(selected.node.id)}>
            <CornerLeftDown size={14} />
          </Button>
          <Button variant="ghost" size="icon" aria-label="Group layers" title="Group layers (Ctrl+G)" disabled={!editable} onClick={groupSelected}>
            <FolderPlus size={14} />
          </Button>
          <Button variant="ghost" size="icon" aria-label="Duplicate layer" title="Duplicate layer (Ctrl+J)" disabled={!editable} onClick={duplicateSelected}>
            <Plus size={14} />
          </Button>
          <Button variant="ghost" size="icon" className="ml-auto" aria-label="Delete selected layer" title="Delete selected layer" disabled={!editable} onClick={deleteSelected}>
            <Trash2 size={14} />
          </Button>
        </div>
      )}
      {adjustMenu && <PopupMenu at={adjustMenu} items={adjustmentItems()} label="New adjustment layer" onClose={() => setAdjustMenu(null)} />}
    </div>
  );
}
