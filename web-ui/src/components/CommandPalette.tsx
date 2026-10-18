import { useEffect, useMemo, useRef, useState } from "react";
import { Search } from "lucide-react";
import { useStudio } from "@/store";
import { isScene, pagesOf, type Tool } from "@/lib/model";
import {
  addLayer,
  addPage,
  alignSelected,
  canvasView,
  deleteSelected,
  duplicatePage,
  duplicateSelected,
  exportAs,
  groupSelected,
  reorderSelected,
  selectAllOnPage,
  ungroupSelected,
  type Align,
} from "@/lib/actions";
import { useExtensionRunner, useExtensions } from "@/workspaces/ExtensionsPanel";
import type { RasterTool } from "@/workspaces/rasterStore";
import { startPainting } from "@/workspaces/paintLayer";
import { cn } from "@/lib/utils";

export type Command = { id: string; group: string; label: string; keys?: string; run: () => void; disabled?: boolean };

/** Panels the palette can open; mirrors the panel rail in App. */
export type PanelEntry = { id: string; label: string };

const TOOLS: [Tool, string, string][] = [
  ["select", "Select", "V"], ["pen", "Pen", "P"], ["rect", "Rectangle", "R"], ["ellipse", "Ellipse", "O"],
  ["line", "Line", "L"], ["text", "Text", "T"], ["hand", "Hand", "H"],
];
const PAINT: [RasterTool, string, string][] = [
  ["brush", "Brush", "B"], ["eraser", "Eraser", "E"], ["clone", "Clone stamp", "S"], ["heal", "Heal", "J"],
  ["quickmask", "Quick mask", "Q"], ["marquee", "Marquee select", "M"], ["lasso", "Lasso select", ""], ["wand", "Magic wand", "W"],
];
const ALIGN: [Align, string][] = [
  ["left", "Align left"], ["hcenter", "Align horizontal centers"], ["right", "Align right"],
  ["top", "Align top"], ["vcenter", "Align vertical centers"], ["bottom", "Align bottom"],
  ["hspace", "Distribute horizontally"], ["vspace", "Distribute vertically"],
];

/** Every Studio command for the current state; disabled ones explain nothing but stay findable. */
function studioCommands(openPanel: (id: string) => void, panels: PanelEntry[]): Command[] {
  const s = useStudio.getState();
  const any = s.selectedIds.length > 0;
  const many = s.selectedIds.length > 1;
  const scene = isScene(s.doc);
  const out: Command[] = [];
  const add = (group: string, label: string, run: () => void, keys?: string, disabled?: boolean) =>
    out.push({ id: `${group}:${label}`, group, label, run, keys, disabled });
  add("Edit", "Undo", s.undo, "Ctrl+Z", !s.past.length);
  add("Edit", "Redo", s.redo, "Ctrl+Shift+Z", !s.future.length);
  add("Edit", "Save", () => void s.save(), "Ctrl+S");
  add("Edit", "Select all", selectAllOnPage, "Ctrl+A");
  add("Edit", "Deselect", () => s.select(null), "Esc", !any);
  add("Edit", "Duplicate", duplicateSelected, "Ctrl+D", !any);
  add("Edit", "Delete", deleteSelected, "Del", !any);
  add("Arrange", "Group", groupSelected, "Ctrl+G", !any || !scene);
  add("Arrange", "Ungroup", ungroupSelected, "Ctrl+Shift+G", !any || many);
  add("Arrange", "Bring to front", () => reorderSelected("front"), "Ctrl+Shift+]", !any || many);
  add("Arrange", "Bring forward", () => reorderSelected("forward"), "Ctrl+]", !any || many);
  add("Arrange", "Send backward", () => reorderSelected("backward"), "Ctrl+[", !any || many);
  add("Arrange", "Send to back", () => reorderSelected("back"), "Ctrl+Shift+[", !any || many);
  for (const [how, label] of ALIGN) add("Arrange", label, () => alignSelected(how), undefined, !any || ((how === "hspace" || how === "vspace") && s.selectedIds.length < 3));
  add("View", "Zoom to fit", () => canvasView.fit?.(), "Shift+1");
  add("View", "Zoom to selection", () => canvasView.selection?.(), "Shift+2", !any);
  add("View", "Actual size (100%)", () => canvasView.actual?.(), "Ctrl+1");
  add("View", "Zoom in", () => canvasView.zoomBy?.(1.25), "Ctrl+=");
  add("View", "Zoom out", () => canvasView.zoomBy?.(1 / 1.25), "Ctrl+-");
  add("View", s.dark ? "Switch to light theme" : "Switch to dark theme", s.toggleDark);
  for (const [tool, label, key] of TOOLS) add("Tools", `${label} tool`, () => { if (s.workspaceId === "photo" || s.workspaceId === "3d") s.setWorkspace("design"); s.setTool(tool); }, key);
  for (const [tool, label, key] of PAINT) add("Tools", `${label} (paint)`, () => startPainting(tool), key || undefined, !scene);
  add("Page", "Add page", addPage);
  add("Page", "Duplicate page", duplicatePage, undefined, !s.doc);
  add("Page", "Add layer", addLayer, undefined, !s.doc);
  pagesOf(s.doc).forEach((page, i) =>
    add("Page", `Go to page ${i + 1}${page.name ? `: ${page.name}` : ""}`, () => s.setPage(page.id), undefined, page.id === s.pageId));
  add("Export", "Export page as PNG", () => void exportAs("png"));
  add("Export", "Export page as SVG", () => void exportAs("svg"));
  for (const panel of panels) add("Panels", `Show ${panel.label}`, () => openPanel(panel.id));
  return out;
}

/** Subsequence match with a bonus for word starts and contiguous runs; null when it does not match. */
export function fuzzyScore(query: string, text: string): number | null {
  const q = query.toLowerCase().replace(/\s+/g, "");
  if (!q) return 0;
  const t = text.toLowerCase();
  let score = 0;
  let at = -1;
  for (const ch of q) {
    const next = t.indexOf(ch, at + 1);
    if (next < 0) return null;
    score += next === at + 1 ? 3 : 0;
    score += next === 0 || /[\s:(]/.test(t[next - 1] ?? "") ? 2 : 0;
    score -= Math.min(3, next - at - 1) * 0.1;
    at = next;
  }
  return score;
}

/**
 * Ctrl/Cmd+K: search and run any Studio command, including installed
 * extension commands, like Figma's quick actions or Photoshop's search.
 */
export function CommandPalette({ open, onClose, openPanel, panels }: { open: boolean; onClose: () => void; openPanel: (id: string) => void; panels: PanelEntry[] }) {
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const { items, refresh } = useExtensions();
  const { run: runExtension, blocked } = useExtensionRunner();
  // Recompute when the palette opens so enabled states match the document.
  const commands = useMemo(() => {
    if (!open) return [];
    const ext: Command[] = (items ?? [])
      .filter((x) => x.status === "ok")
      .flatMap((x) => (x.commands?.length ? x.commands : [{ id: "", title: "Run" }]).map((c) => ({
        id: `ext:${x.name}/${c.id}`, group: "Extensions", label: `${c.title} (${x.name})`,
        run: () => void runExtension(x.name, c.id || undefined, {}, false), disabled: !!blocked,
      })));
    return [...studioCommands(openPanel, panels), ...ext];
  }, [open, items, blocked, runExtension, openPanel, panels]);
  const shown = useMemo(() => {
    if (!query.trim()) return commands.filter((c) => !c.disabled || c.group !== "Page");
    return commands
      .map((c) => ({ c, score: fuzzyScore(query, `${c.label} ${c.group}`) }))
      .filter((x): x is { c: Command; score: number } => x.score !== null)
      .sort((a, b) => Number(!!a.c.disabled) - Number(!!b.c.disabled) || b.score - a.score)
      .map((x) => x.c);
  }, [commands, query]);
  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActive(0);
    void refresh();
  }, [open, refresh]);
  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);
  if (!open) return null;
  const runAt = (i: number) => {
    const c = shown[i];
    if (!c || c.disabled) return;
    onClose();
    c.run();
  };
  return (
    <div className="fixed inset-0 z-[60] flex items-start justify-center bg-black/30 pt-[12vh]" onPointerDown={onClose}>
      <div role="dialog" aria-modal="true" aria-label="Command palette" className="w-[34rem] max-w-[92vw] overflow-hidden rounded-lg border bg-card shadow-2xl" onPointerDown={(e) => e.stopPropagation()}>
        <div className="flex items-center gap-2 border-b px-3">
          <Search size={15} aria-hidden="true" className="text-muted-foreground" />
          <input
            autoFocus
            role="combobox"
            aria-expanded="true"
            aria-controls="command-list"
            aria-activedescendant={shown[active] ? `command-${active}` : undefined}
            aria-label="Search commands"
            className="h-11 min-w-0 flex-1 bg-transparent text-sm outline-none"
            placeholder="Search commands, tools, pages, panels…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                e.preventDefault();
                const n = shown.length;
                if (n) setActive((a) => (a + (e.key === "ArrowDown" ? 1 : n - 1)) % n);
              } else if (e.key === "Enter") {
                e.preventDefault();
                runAt(active);
              } else if (e.key === "Escape") {
                e.preventDefault();
                e.stopPropagation();
                onClose();
              }
            }}
          />
          <kbd className="text-[10px] text-muted-foreground">Esc</kbd>
        </div>
        <ul id="command-list" ref={list} role="listbox" aria-label="Commands" className="max-h-[52vh] overflow-y-auto p-1">
          {shown.map((c, i) => (
            <li key={c.id}>
              {(i === 0 || shown[i - 1]!.group !== c.group) && !query.trim() && (
                <div className="px-2 pb-0.5 pt-2 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">{c.group}</div>
              )}
              <div
                id={`command-${i}`}
                data-index={i}
                role="option"
                aria-selected={i === active}
                aria-disabled={c.disabled || undefined}
                className={cn("flex cursor-pointer items-center justify-between gap-4 rounded px-2 py-1.5 text-sm", i === active && "bg-accent", c.disabled && "cursor-default opacity-40")}
                onPointerMove={() => setActive(i)}
                onClick={() => runAt(i)}
              >
                <span className="min-w-0 truncate">{c.label}{query.trim() && <span className="ml-2 text-[11px] text-muted-foreground">{c.group}</span>}</span>
                {c.keys && <kbd className="shrink-0 text-[10px] text-muted-foreground">{c.keys}</kbd>}
              </div>
            </li>
          ))}
          {shown.length === 0 && <li className="px-2 py-3 text-center text-xs text-muted-foreground">No matching commands.</li>}
        </ul>
      </div>
    </div>
  );
}

const SHEET: [string, [string, string][]][] = [
  ["Tools", [["V", "Select"], ["P", "Pen"], ["R", "Rectangle"], ["O", "Ellipse"], ["L", "Line"], ["T", "Text"], ["H / hold Space", "Hand"], ["B E S J Q M W", "Paint tools"]]],
  ["Selection", [["Click", "Select (outermost group)"], ["Shift+click", "Add or remove"], ["Ctrl+click", "Select inside groups"], ["Double-click", "Step into a group"], ["Drag on empty canvas", "Marquee select"], ["Ctrl+A", "Select all"], ["Esc", "Deselect"]]],
  ["Move and transform", [["Drag", "Move (snaps to edges and centres)"], ["Shift+drag", "Keep to one axis / keep proportions"], ["Alt+drag", "Duplicate while moving"], ["Ctrl+drag", "Move without snapping"], ["Arrows / Shift+arrows", "Nudge 1 / 10"]]],
  ["Edit", [["Ctrl+C / X / V", "Copy, cut, paste objects"], ["Ctrl+D", "Duplicate"], ["Del", "Delete"], ["Ctrl+G / Ctrl+Shift+G", "Group / ungroup"], ["Ctrl+] / Ctrl+[", "Forward / backward"], ["Ctrl+Shift+] / [", "Front / back"], ["Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"], ["Ctrl+S", "Save"]]],
  ["View", [["Ctrl+wheel / pinch", "Zoom at cursor"], ["Ctrl+= / Ctrl+-", "Zoom in / out"], ["Shift+1 / Ctrl+0", "Fit page"], ["Shift+2", "Zoom to selection"], ["Ctrl+1", "100%"]]],
  ["Studio", [["Ctrl+K", "Command palette"], ["?", "This sheet"], ["Right-click", "Canvas actions"]]],
];

/** "?" opens a cheat sheet of every canvas shortcut. */
export function ShortcutSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  if (!open) return null;
  return (
    <div className="fixed inset-0 z-[60] flex items-center justify-center bg-black/30 p-4" onPointerDown={onClose}>
      <div role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" tabIndex={-1} ref={(el) => el?.focus()}
        className="max-h-[86vh] w-[52rem] max-w-full overflow-y-auto rounded-lg border bg-card p-4 shadow-2xl outline-none"
        onPointerDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => { if (e.key === "Escape" || e.key === "?") { e.preventDefault(); e.stopPropagation(); onClose(); } }}>
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">Keyboard shortcuts</h2>
          <span className="text-[11px] text-muted-foreground">On a Mac, use Cmd for Ctrl and Option for Alt. Press Esc to close.</span>
        </div>
        <div className="grid gap-x-6 gap-y-4 sm:grid-cols-2 lg:grid-cols-3">
          {SHEET.map(([title, rows]) => (
            <section key={title} aria-label={title}>
              <h3 className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">{title}</h3>
              <dl className="grid gap-1 text-xs">
                {rows.map(([keys, what]) => (
                  <div key={keys} className="flex items-baseline justify-between gap-3">
                    <dt>{what}</dt>
                    <dd><kbd className="whitespace-nowrap rounded border bg-muted px-1 py-px text-[10px]">{keys}</kbd></dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
