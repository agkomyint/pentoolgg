import { Brush, Circle, Copy, Crosshair, Eraser, Hand, HeartPulse, Lasso, MousePointer2, PenTool, ScanLine, Slash, Square, Type, Wand2 } from "lucide-react";
import { Fragment, useEffect } from "react";
import type { LucideIcon } from "lucide-react";
import { useStudio } from "@/store";
import type { Tool } from "@/lib/model";
import { Tip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { usePaint, type RasterTool } from "@/workspaces/rasterStore";

const TOOLS: { id: Tool; label: string; key: string; icon: LucideIcon }[] = [
  { id: "select", label: "Select", key: "V", icon: MousePointer2 },
  { id: "pen", label: "Pen", key: "P", icon: PenTool },
  { id: "rect", label: "Rectangle", key: "R", icon: Square },
  { id: "ellipse", label: "Ellipse", key: "O", icon: Circle },
  { id: "line", label: "Line", key: "L", icon: Slash },
  { id: "text", label: "Text", key: "T", icon: Type },
  { id: "hand", label: "Hand (hold Space)", key: "H", icon: Hand },
];

const HINTS: Record<Tool, string> = {
  select: "Click to select, drag to move, drag handles to resize (Shift keeps proportions). Arrow keys nudge.",
  pen: "Click to add points, drag for curves, click the first point to close, Enter to finish, Esc to cancel.",
  rect: "Drag to draw a rectangle (Shift for a square).",
  ellipse: "Drag to draw an ellipse (Shift for a circle).",
  line: "Drag to draw a line.",
  text: "Click the page to place text, then edit it in Properties.",
  hand: "Drag to pan. Ctrl + wheel zooms.",
};

const PAINT_TOOLS: { id: RasterTool; label: string; icon: LucideIcon }[] = [
  { id: "brush", label: "Brush", icon: Brush },
  { id: "eraser", label: "Eraser", icon: Eraser },
  { id: "clone", label: "Clone", icon: Copy },
  { id: "heal", label: "Heal", icon: HeartPulse },
  { id: "source", label: "Clone source", icon: Crosshair },
  { id: "quickmask", label: "Quick mask", icon: ScanLine },
  { id: "marquee", label: "Marquee", icon: Square },
  { id: "lasso", label: "Lasso", icon: Lasso },
  { id: "wand", label: "Magic wand", icon: Wand2 },
];

export function Toolbar() {
  const { tool, setTool, workspaceId, setWorkspace } = useStudio();
  const paint = usePaint();
  const isPaint = workspaceId === "paint";
  const isDrawing = !["paint", "photo", "3d"].includes(workspaceId);
  const chooseVector = (next: Tool) => {
    if (!isDrawing) setWorkspace("design");
    setTool(next);
  };
  const choosePaint = (next: RasterTool) => {
    if (!isPaint) setWorkspace("paint");
    paint.set({ tool: next });
  };
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || target?.closest("input, textarea, select, [contenteditable=true]") || ["photo", "3d"].includes(workspaceId)) return;
      const key = event.key.toLowerCase();
      if (key === "b" || key === "e") {
        event.preventDefault();
        useStudio.getState().setWorkspace("paint");
        usePaint.getState().set({ tool: key === "b" ? "brush" : "eraser" });
      } else if (key === "v" && workspaceId === "paint") {
        event.preventDefault();
        useStudio.getState().setWorkspace("design");
        useStudio.getState().setTool("select");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [workspaceId]);
  return <aside className="studio-toolrail" aria-label="Creative tools">
    <div className="studio-toolrail-group" role="toolbar" aria-label="Vector tools" aria-orientation="vertical">
      {TOOLS.map((t) => <Fragment key={t.id}>
        {t.id === "hand" && <span aria-hidden="true" className="studio-toolrail-divider" />}
        <Tip label={`${t.label} (${t.key})`}><button type="button" aria-label={`${t.label} (${t.key})`} aria-pressed={isDrawing && tool === t.id} onClick={() => chooseVector(t.id)}
          className={cn("studio-tool-button", isDrawing && tool === t.id && "is-active")}><t.icon size={17} aria-hidden="true" /></button></Tip>
      </Fragment>)}
    </div>
    <span aria-hidden="true" className="studio-toolrail-divider" />
    <div className="studio-toolrail-group" role="toolbar" aria-label="Paint and retouch tools" aria-orientation="vertical">
      {PAINT_TOOLS.map((t) => <Fragment key={t.id}>
        {t.id === "marquee" && <span aria-hidden="true" className="studio-toolrail-divider" />}
        <Tip label={`${t.label}${t.id === "brush" ? " (B)" : t.id === "eraser" ? " (E)" : ""}`}><button type="button" aria-label={t.label} aria-pressed={isPaint && paint.tool === t.id} onClick={() => choosePaint(t.id)}
          className={cn("studio-tool-button", isPaint && paint.tool === t.id && "is-active")}><t.icon size={17} aria-hidden="true" /></button></Tip>
      </Fragment>)}
    </div>
  </aside>;
}

export function ToolHint() {
  const { tool } = useStudio();
  return <div className="border-t bg-card px-3 py-1 text-[11px] text-muted-foreground">{HINTS[tool]}</div>;
}
