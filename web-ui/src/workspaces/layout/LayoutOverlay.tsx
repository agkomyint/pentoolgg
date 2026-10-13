import { useStudio } from "@/store";
import { pageOf } from "@/lib/model";
import { useLayout } from "./layoutStore";
import { layoutOf } from "./layoutModel";

/**
 * Drawn inside the canvas SVG (page units) in the Layout workspace: frame
 * edges, thread links, overset markers and margin/column guides. Geometry comes
 * from the composed overview, never from browser-side text measurement.
 */
export function LayoutOverlay({ px }: { px: number }) {
  const { doc, pageId, workspaceId, selectedId } = useStudio();
  const { overview, overlays, threadFrom } = useLayout();
  if (workspaceId !== "layout" || !doc || doc.version < 9) return null;
  const page = pageOf(doc, pageId);
  if (!page) return null;
  const frames = (overview?.frames ?? []).filter((f) => f.page === page.id);
  const byId = new Map((overview?.frames ?? []).map((f) => [f.id, f]));
  const canvas = page.canvas ?? { width: 612, height: 792 };

  const layout = layoutOf(doc);
  const own = (page as { layout?: { margins?: number[]; columns?: number; gutter?: number } }).layout ?? {};
  const defaults = (layout as { page_defaults?: { margins?: number[]; columns?: number; gutter?: number } }).page_defaults ?? {};
  const margins = own.margins ?? defaults.margins;
  const columns = own.columns ?? defaults.columns ?? 1;
  const gutter = own.gutter ?? defaults.gutter ?? 12;
  const grid = (layout as { baseline_grid?: { increment: number; start?: number; relative_to?: string } }).baseline_grid;
  const gridStart = (grid?.start ?? 0) + (grid?.relative_to === "margin" ? (margins?.[0] ?? 0) : 0);
  const guides = ((layout as { guides?: { orientation?: string; position?: number; page?: string }[] }).guides ?? [])
    .filter((g) => typeof g.position === "number" && (!g.page || g.page === page.id))
    .map((g) => ({ orientation: g.orientation === "v" ? "v" : "h", position: g.position as number }));
  const spread = (layout.spreads ?? []).find((s) => s.pages.includes(page.id));
  const mirrored = !!spread?.facing && spread.pages.length > 1;
  const left = margins && mirrored ? (spread!.pages[0] === page.id ? margins[3]! : margins[1]!) : margins?.[1];
  const right = margins && mirrored ? (spread!.pages[0] === page.id ? margins[1]! : margins[3]!) : margins?.[3];

  return (
    <g pointerEvents="none">
      {overlays.margins && margins && margins.length === 4 && (
        <g stroke="#e11d8f" strokeWidth={px} fill="none" opacity={0.7}>
          <rect x={left} y={margins[0]} width={canvas.width - (left ?? 0) - (right ?? 0)} height={canvas.height - margins[0]! - margins[2]!} />
          {Array.from({ length: Math.max(0, columns - 1) }, (_, i) => {
            const inner = canvas.width - (left ?? 0) - (right ?? 0);
            const col = (inner - gutter * (columns - 1)) / columns;
            const x = (left ?? 0) + (i + 1) * col + i * gutter;
            return (
              <g key={i} strokeDasharray={`${3 * px} ${3 * px}`}>
                <line x1={x} x2={x} y1={margins[0]} y2={canvas.height - margins[2]!} />
                <line x1={x + gutter} x2={x + gutter} y1={margins[0]} y2={canvas.height - margins[2]!} />
              </g>
            );
          })}
        </g>
      )}
      {overlays.baseline && grid && (
        <g stroke="#06b6d4" strokeWidth={px * 0.5} opacity={0.55}>
          {Array.from({ length: Math.min(2000, Math.floor((canvas.height - gridStart) / grid.increment) + 1) }, (_, i) => (
            <line key={i} x1={0} x2={canvas.width} y1={gridStart + i * grid.increment} y2={gridStart + i * grid.increment} />
          ))}
        </g>
      )}
      {overlays.margins &&
        guides.map((g, i) => (
          <line
            key={`g-${i}`}
            stroke="#06b6d4"
            strokeWidth={px}
            x1={g.orientation === "v" ? g.position : 0}
            x2={g.orientation === "v" ? g.position : canvas.width}
            y1={g.orientation === "v" ? 0 : g.position}
            y2={g.orientation === "v" ? canvas.height : g.position}
          />
        ))}
      {overlays.frames &&
        frames.map((f) => (
          <rect
            key={f.id}
            x={f.x}
            y={f.y}
            width={f.width}
            height={f.height}
            fill="none"
            stroke={f.id === selectedId ? "var(--primary)" : f.id === threadFrom ? "#f59e0b" : "#7c3aed"}
            strokeWidth={px * (f.id === selectedId ? 1.5 : 1)}
            strokeDasharray={f.id === selectedId ? undefined : `${4 * px} ${3 * px}`}
            opacity={0.85}
          />
        ))}
      {overlays.threads &&
        frames.map((f) => {
          const next = f.next ? byId.get(f.next) : undefined;
          if (!next) return null;
          const same = next.page === f.page;
          const ax = f.x + f.width;
          const ay = f.y + f.height;
          const bx = same ? next.x : f.x + f.width;
          const by = same ? next.y : f.y + f.height + 14 * px;
          return (
            <g key={`t-${f.id}`} stroke="#7c3aed" fill="#7c3aed">
              <rect x={ax - 5 * px} y={ay - 5 * px} width={10 * px} height={10 * px} />
              {same ? (
                <line x1={ax} y1={ay} x2={bx} y2={by} strokeWidth={px} strokeDasharray={`${5 * px} ${3 * px}`} />
              ) : (
                <text x={ax - 4 * px} y={ay + 14 * px} fontSize={10 * px} textAnchor="end" stroke="none">
                  continues on {next.page}
                </text>
              )}
            </g>
          );
        })}
      {overlays.overset &&
        frames
          .filter((f) => f.overset)
          .map((f) => (
            <g key={`o-${f.id}`}>
              <rect x={f.x + f.width - 6 * px} y={f.y + f.height - 6 * px} width={12 * px} height={12 * px} fill="#dc2626" />
              <text
                x={f.x + f.width}
                y={f.y + f.height + 3.5 * px}
                fontSize={11 * px}
                fill="#fff"
                textAnchor="middle"
                fontWeight={700}
              >
                +
              </text>
            </g>
          ))}
    </g>
  );
}
