import { Circle, Hand, MousePointer2, PenTool, Slash, Square, Type } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useStudio } from "@/store";
import type { Tool } from "@/lib/model";
import { Tip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

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

export function Toolbar() {
  const { tool, setTool } = useStudio();
  return (
    <div className="flex w-12 shrink-0 flex-col items-center gap-1 border-r bg-card py-2" role="toolbar" aria-label="Tools">
      {TOOLS.map((t) => (
        <Tip key={t.id} label={`${t.label} (${t.key})`}>
          <button
            aria-label={t.label}
            aria-pressed={tool === t.id}
            onClick={() => setTool(t.id)}
            className={cn("flex h-9 w-9 items-center justify-center rounded-md text-muted-foreground hover:bg-accent", tool === t.id && "bg-primary text-primary-foreground hover:bg-primary")}
          >
            <t.icon size={17} />
          </button>
        </Tip>
      ))}
    </div>
  );
}

export function ToolHint() {
  const { tool } = useStudio();
  return <div className="border-t bg-card px-3 py-1 text-[11px] text-muted-foreground">{HINTS[tool]}</div>;
}
