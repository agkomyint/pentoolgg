import { useEffect, useMemo, useState } from "react";
import {
  AlignCenterHorizontal,
  AlignCenterVertical,
  AlignEndHorizontal,
  AlignEndVertical,
  AlignHorizontalSpaceAround,
  AlignStartHorizontal,
  AlignStartVertical,
  AlignVerticalSpaceAround,
  ArrowDown,
  ArrowUp,
  ChevronsDown,
  ChevronsUp,
  Copy,
  Group,
  Trash2,
  Ungroup,
} from "lucide-react";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import {
  alignSelected,
  deleteSelected,
  duplicateSelected,
  groupSelected,
  moveNodeBy,
  renameNode,
  reorderSelected,
  replaceNodeJson,
  transformNodePage,
  ungroupSelected,
  updateNode,
} from "@/lib/actions";
import { findNode, getStyle, isScene, nodeKind, pageBounds, pageOf, setStyle } from "@/lib/model";

const input = "mt-0.5 h-7 w-full rounded border bg-card px-1.5 text-xs";

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block text-xs">
      <span className="text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}

function Num({ label, value, onCommit, step = 1 }: { label: string; value: number; onCommit: (n: number) => void; step?: number }) {
  const [text, setText] = useState(String(round(value)));
  useEffect(() => setText(String(round(value))), [value]);
  return (
    <Field label={label}>
      <input
        className={input}
        type="number"
        step={step}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={() => {
          const n = Number(text);
          if (Number.isFinite(n) && n !== round(value)) onCommit(n);
          else setText(String(round(value)));
        }}
        onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      />
    </Field>
  );
}

const round = (n: number) => Math.round(n * 100) / 100;

function color(value: unknown, fallback = "#000000"): string {
  return typeof value === "string" && /^#[0-9a-f]{6}$/i.test(value) ? value : fallback;
}

function Colour({ label, value, onCommit, allowNone }: { label: string; value: unknown; onCommit: (v: string) => void; allowNone?: boolean }) {
  const none = value === "none" || value === undefined;
  return (
    <Field label={label}>
      <span className="flex items-center gap-1.5">
        <input
          type="color"
          className="mt-0.5 h-7 w-9 rounded border bg-card"
          value={color(value)}
          onChange={(e) => onCommit(e.target.value)}
        />
        {allowNone && (
          <button className="rounded border px-1.5 py-1 text-[11px] hover:bg-accent" onClick={() => onCommit("none")}>
            {none ? "none (set)" : "none"}
          </button>
        )}
        <code className="truncate text-[10px] text-muted-foreground">{typeof value === "string" ? value : ""}</code>
      </span>
    </Field>
  );
}

const ALIGN = [
  ["left", "Align left", AlignStartVertical],
  ["hcenter", "Align horizontal centres", AlignCenterVertical],
  ["right", "Align right", AlignEndVertical],
  ["top", "Align top", AlignStartHorizontal],
  ["vcenter", "Align vertical centres", AlignCenterHorizontal],
  ["bottom", "Align bottom", AlignEndHorizontal],
  ["hspace", "Distribute horizontally", AlignHorizontalSpaceAround],
  ["vspace", "Distribute vertically", AlignVerticalSpaceAround],
] as const;

/** Align and distribute; one object aligns to the page. */
function AlignBar({ count }: { count: number }) {
  return (
    <div className="flex flex-wrap gap-0.5" role="group" aria-label="Align">
      {ALIGN.map(([how, label, Icon]) => (
        <Button
          key={how}
          variant="ghost"
          size="icon"
          aria-label={label}
          title={count < 2 && !how.endsWith("space") ? `${label} (to page)` : label}
          disabled={how.endsWith("space") && count < 3}
          onClick={() => alignSelected(how)}
        >
          <Icon size={14} />
        </Button>
      ))}
    </div>
  );
}

export function Properties() {
  const { doc, pageId, selectedId, selectedIds } = useStudio();
  const page = pageOf(doc, pageId);
  const found = useMemo(() => findNode(page, selectedId), [page, selectedId]);
  const [json, setJson] = useState("");
  const [jsonOpen, setJsonOpen] = useState(false);
  useEffect(() => { if (jsonOpen) setJson(found ? JSON.stringify(found.node, null, 2) : ""); }, [found?.node, jsonOpen]);

  if (!doc || !found) {
    return <p className="text-muted-foreground">Select an object on the canvas or in Layers to edit it. Shift-click or drag a box to select several.</p>;
  }
  if (selectedIds.length > 1) {
    return (
      <div className="space-y-3">
        <span className="rounded bg-accent px-1.5 py-0.5 text-[11px] font-medium">{selectedIds.length} objects</span>
        <AlignBar count={selectedIds.length} />
        <div className="flex flex-wrap gap-1">
          <Button variant="outline" size="sm" onClick={duplicateSelected}>
            <Copy size={12} /> Duplicate
          </Button>
          <Button variant="outline" size="sm" onClick={deleteSelected}>
            <Trash2 size={12} /> Delete
          </Button>
          <Button variant="outline" size="sm" onClick={groupSelected} disabled={!isScene(doc)}>
            <Group size={12} /> Group
          </Button>
        </div>
        <p className="text-[11px] text-muted-foreground">Drag any selected object to move them together; Alt+drag copies.</p>
      </div>
    );
  }
  const { node, layer } = found;
  const scene = isScene(doc);
  const kind = nodeKind(node, layer);
  const box = pageBounds(found);
  const locked = !!layer.locked;
  const edit = (change: (n: typeof node, scene: boolean) => void) => updateNode(node.id, (n, _f, s) => change(n, s));
  const style = (key: string, value: unknown) => edit((n, s) => setStyle(n, key, value, s));
  const hasStroke = kind === "path" || kind === "line";
  const textual = kind === "text";

  return (
    <div className="space-y-3">
      <div className="flex items-center gap-2">
        <span className="rounded bg-accent px-1.5 py-0.5 text-[11px] font-medium">{kind}</span>
        <code className="truncate text-[11px] text-muted-foreground" title={node.id}>
          {node.id}
        </code>
        {locked && <span className="text-[11px] text-destructive">layer locked</span>}
      </div>

      <div className="flex flex-wrap gap-1">
        <Button variant="outline" size="sm" onClick={duplicateSelected} disabled={locked}>
          <Copy size={12} /> Duplicate
        </Button>
        <Button variant="outline" size="sm" onClick={deleteSelected} disabled={locked}>
          <Trash2 size={12} /> Delete
        </Button>
        <Button variant="ghost" size="icon" aria-label="Bring to front" onClick={() => reorderSelected("front")} disabled={locked}>
          <ChevronsUp size={14} />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Forward" onClick={() => reorderSelected("forward")} disabled={locked}>
          <ArrowUp size={14} />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Backward" onClick={() => reorderSelected("backward")} disabled={locked}>
          <ArrowDown size={14} />
        </Button>
        <Button variant="ghost" size="icon" aria-label="Send to back" onClick={() => reorderSelected("back")} disabled={locked}>
          <ChevronsDown size={14} />
        </Button>
        {kind === "group" ? (
          <Button variant="outline" size="sm" onClick={ungroupSelected} disabled={locked}>
            <Ungroup size={12} /> Ungroup
          </Button>
        ) : (
          <Button variant="outline" size="sm" onClick={groupSelected} disabled={locked || !scene}>
            <Group size={12} /> Group
          </Button>
        )}
      </div>

      <AlignBar count={1} />

      <Field label="Name">
        <input
          className={input}
          defaultValue={typeof node.name === "string" ? node.name : ""}
          placeholder={node.id}
          key={node.id}
          onBlur={(e) => e.target.value !== (node.name ?? "") && renameNode(node.id, e.target.value)}
        />
      </Field>

      {box && (
        <section className="space-y-2">
          <h4 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Layout</h4>
          <div className="grid grid-cols-2 gap-2">
            <Num label="X" value={box.x} onCommit={(n) => moveNodeBy(node.id, n - box.x, 0)} />
            <Num label="Y" value={box.y} onCommit={(n) => moveNodeBy(node.id, 0, n - box.y)} />
            <Num
              label="Width"
              value={box.width}
              onCommit={(n) => box.width > 0 && n > 0 && transformNodePage(node.id, [n / box.width, 0, 0, 1, box.x * (1 - n / box.width), 0])}
            />
            <Num
              label="Height"
              value={box.height}
              onCommit={(n) => box.height > 0 && n > 0 && transformNodePage(node.id, [1, 0, 0, n / box.height, 0, box.y * (1 - n / box.height)])}
            />
            <Num
              label="Rotate (deg, about centre)"
              value={0}
              onCommit={(deg) => {
                const r = (deg * Math.PI) / 180;
                const cx = box.x + box.width / 2;
                const cy = box.y + box.height / 2;
                const c = Math.cos(r);
                const s = Math.sin(r);
                transformNodePage(node.id, [c, s, -s, c, cx - c * cx + s * cy, cy - s * cx - c * cy]);
              }}
            />
          </div>
        </section>
      )}

      {(kind === "rect" || kind === "ellipse" || kind === "path" || kind === "text" || kind === "line") && (
        <section className="space-y-2">
          <h4 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Appearance</h4>
          {kind !== "line" && (
            <Colour label="Fill" value={getStyle(node, "fill")} allowNone={kind === "path"} onCommit={(v) => style("fill", v)} />
          )}
          {hasStroke && (
            <>
              <Colour label="Stroke" value={getStyle(node, "stroke")} allowNone onCommit={(v) => style("stroke", v)} />
              <Num label="Stroke width" value={Number(getStyle(node, "stroke_width") ?? 1)} step={0.5} onCommit={(n) => style("stroke_width", Math.max(0, n))} />
              <div className="grid grid-cols-2 gap-2">
                <Field label="Cap">
                  <select className={input} value={String(getStyle(node, "stroke_linecap") ?? "round")} onChange={(e) => style("stroke_linecap", e.target.value)}>
                    <option>butt</option>
                    <option>round</option>
                    <option>square</option>
                  </select>
                </Field>
                <Field label="Join">
                  <select className={input} value={String(getStyle(node, "stroke_linejoin") ?? "round")} onChange={(e) => style("stroke_linejoin", e.target.value)}>
                    <option>miter</option>
                    <option>round</option>
                    <option>bevel</option>
                  </select>
                </Field>
              </div>
            </>
          )}
          {kind === "rect" && (
            <div className="grid grid-cols-2 gap-2">
              <Num label="Radius X" value={Number(node.radius_x ?? 0)} onCommit={(n) => edit((x) => void (x.radius_x = Math.max(0, n)))} />
              <Num label="Radius Y" value={Number(node.radius_y ?? 0)} onCommit={(n) => edit((x) => void (x.radius_y = Math.max(0, n)))} />
            </div>
          )}
        </section>
      )}

      {textual && (
        <section className="space-y-2">
          <h4 className="text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">Text</h4>
          <Field label="Content">
            <textarea
              className={`${input} h-20 py-1`}
              key={`${node.id}-content-${String(node.content ?? "")}`}
              defaultValue={String(node.content ?? "")}
              onBlur={(e) => e.target.value !== node.content && edit((n) => void (n.content = e.target.value))}
            />
          </Field>
          <Field label="Font family">
            <input
              className={input}
              list="studio-fonts"
              defaultValue={String(node.font_family ?? "")}
              key={`${node.id}-ff-${String(node.font_family ?? "")}`}
              onBlur={(e) => e.target.value && e.target.value !== node.font_family && edit((n) => void (n.font_family = e.target.value))}
            />
          </Field>
          <div className="grid grid-cols-2 gap-2">
            <Num label="Size" value={Number(node.font_size ?? 16)} onCommit={(n) => edit((x) => void (x.font_size = Math.max(1, n)))} />
            <Field label="Weight">
              <select className={input} value={String(node.font_weight ?? 400)} onChange={(e) => edit((n) => void (n.font_weight = Number(e.target.value)))}>
                {[100, 200, 300, 400, 500, 600, 700, 800, 900].map((w) => (
                  <option key={w}>{w}</option>
                ))}
              </select>
            </Field>
            <Num label="Letter spacing" value={Number(node.letter_spacing ?? 0)} step={0.1} onCommit={(n) => edit((x) => void (x.letter_spacing = n))} />
            <Num label="Line height" value={Number(node.line_height ?? 1.2)} step={0.05} onCommit={(n) => edit((x) => void (x.line_height = Math.max(0.5, n)))} />
            <Field label="Align">
              <select className={input} value={String(node.align ?? "left")} onChange={(e) => edit((n) => void (n.align = e.target.value))}>
                <option>left</option>
                <option>center</option>
                <option>right</option>
              </select>
            </Field>
            <Field label="Italic">
              <input type="checkbox" className="mt-2 block" checked={!!node.italic} onChange={(e) => edit((n) => void (n.italic = e.target.checked))} />
            </Field>
          </div>
        </section>
      )}

      <details className="text-xs" open={jsonOpen} onToggle={(event) => setJsonOpen(event.currentTarget.open)}>
        <summary className="cursor-pointer text-muted-foreground">Advanced object data</summary>
        {jsonOpen && <><textarea className="mt-1 h-48 w-full rounded border bg-card p-1.5 font-mono text-[11px]" value={json} onChange={(e) => setJson(e.target.value)} spellCheck={false} />
        <Button size="sm" className="mt-1" onClick={() => replaceNodeJson(node.id, json)} disabled={locked}>
          Apply JSON
        </Button></>}
      </details>
    </div>
  );
}
