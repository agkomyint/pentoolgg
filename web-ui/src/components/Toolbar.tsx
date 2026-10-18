import { Brush, Circle, Copy, Crosshair, Eraser, Hand, HeartPulse, Lasso, MousePointer2, PenTool, ScanLine, Slash, Square, SquareDashed, Type, Wand2 } from "lucide-react";
import { Fragment } from "react";
import type { LucideIcon } from "lucide-react";
import { isScene, type Tool } from "@/lib/model";
import { useStudio } from "@/store";
import { Tip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import { usePaint, type RasterTool } from "@/workspaces/rasterStore";
import { startPainting } from "@/workspaces/paintLayer";

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

const PAINT_TOOLS: { id: RasterTool; label: string; key?: string; icon: LucideIcon }[] = [
  { id: "brush", label: "Brush", key: "B", icon: Brush },
  { id: "eraser", label: "Eraser", key: "E", icon: Eraser },
  { id: "clone", label: "Clone", key: "S", icon: Copy },
  { id: "heal", label: "Heal", key: "J", icon: HeartPulse },
  { id: "source", label: "Clone source", icon: Crosshair },
  { id: "quickmask", label: "Quick mask", key: "Q", icon: ScanLine },
  { id: "marquee", label: "Marquee", key: "M", icon: SquareDashed },
  { id: "lasso", label: "Lasso", icon: Lasso },
  { id: "wand", label: "Magic wand", key: "W", icon: Wand2 },
];

/**
 * One tool rail for the one canvas: vector tools and paint tools act on the
 * same page. Choosing a tool from either group switches the canvas between
 * object editing and painting; focus views (Photo, Packaging) return to it.
 */
export function Toolbar() {
  const { tool, setTool, workspaceId, setWorkspace, paintMode, doc } = useStudio();
  const paint = usePaint();
  const onCanvas = workspaceId !== "photo" && workspaceId !== "3d";
  const isPaint = paintMode && onCanvas;
  const chooseVector = (next: Tool) => {
    if (!onCanvas) setWorkspace("design");
    setTool(next);
  };
  const paintable = isScene(doc);
  return <aside className="studio-toolrail" aria-label="Creative tools">
    <div className="studio-toolrail-group" role="toolbar" aria-label="Vector tools" aria-orientation="vertical">
      {TOOLS.map((t) => <Fragment key={t.id}>
        {t.id === "hand" && <span aria-hidden="true" className="studio-toolrail-divider" />}
        <Tip label={`${t.label} (${t.key})`}><button type="button" aria-label={`${t.label} (${t.key})`} aria-pressed={onCanvas && !isPaint && tool === t.id} onClick={() => chooseVector(t.id)}
          className={cn("studio-tool-button", onCanvas && !isPaint && tool === t.id && "is-active")}><t.icon size={17} aria-hidden="true" /></button></Tip>
      </Fragment>)}
    </div>
    <span aria-hidden="true" className="studio-toolrail-divider" />
    <div className="studio-toolrail-group" role="toolbar" aria-label="Paint and retouch tools" aria-orientation="vertical">
      {PAINT_TOOLS.map((t) => <Fragment key={t.id}>
        {t.id === "marquee" && <span aria-hidden="true" className="studio-toolrail-divider" />}
        <Tip label={paintable ? `${t.label}${t.key ? ` (${t.key})` : ""}` : "Painting needs a scene document (format v4 or newer)"}><button type="button" aria-label={t.label} aria-pressed={isPaint && paint.tool === t.id} disabled={!paintable} onClick={() => startPainting(t.id)}
          className={cn("studio-tool-button", isPaint && paint.tool === t.id && "is-active")}><t.icon size={17} aria-hidden="true" /></button></Tip>
      </Fragment>)}
    </div>
  </aside>;
}

export function ToolHint() {
  const { tool } = useStudio();
  return <div className="border-t bg-card px-3 py-1 text-[11px] text-muted-foreground">{HINTS[tool]}</div>;
}
