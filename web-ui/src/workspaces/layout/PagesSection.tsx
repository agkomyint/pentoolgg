import { useStudio } from "@/store";
import { Check, Row, Section } from "@/components/ui/kit";
import { cn } from "@/lib/utils";
import { useLayout } from "./layoutStore";
import { layoutOf, pageParent } from "./layoutModel";

/** Spreads as rows of page chips and parent assignment; page edits live in the page strip. */
export function PagesSection() {
  const { doc, pageId, setPage } = useStudio();
  const run = useLayout((s) => s.run);
  if (!doc) return null;
  const layout = layoutOf(doc);
  const spreads = layout.spreads ?? [];
  const inSpread = new Set(spreads.flatMap((s) => s.pages));
  const rows = [
    ...spreads,
    ...doc.pages.filter((p) => !inSpread.has(p.id)).map((p) => ({ id: `solo-${p.id}`, pages: [p.id], facing: false })),
  ];
  const index = doc.pages.findIndex((p) => p.id === pageId);
  const current = doc.pages[index];
  const parents = layout.parents ?? [];
  const spread = spreads.find((s) => s.pages.includes(pageId ?? ""));
  const label = (id: string) => {
    const at = doc.pages.findIndex((p) => p.id === id);
    const name = doc.pages[at]?.name;
    return name && name !== id ? name : at >= 0 ? `Page ${at + 1}` : id;
  };
  return (
    <Section
      id="layout-pages"
      title={`Pages (${doc.pages.length})`}
    >
      <p className="mb-1 text-[11px] text-muted-foreground">Add, reorder and delete pages in the page strip below the canvas.</p>
      <div className="max-h-52 space-y-1 overflow-auto" role="listbox" aria-label="Spreads">
        {rows.map((row) => (
          <div key={row.id} className="flex gap-1">
            {row.pages.map((id) => (
              <button
                key={id}
                role="option"
                aria-selected={id === pageId}
                onClick={() => setPage(id)}
                className={cn(
                  "h-12 min-w-0 flex-1 truncate rounded border bg-input px-1 text-[11px] hover:border-primary",
                  id === pageId && "border-primary bg-accent",
                )}
                title={pageParent(doc.pages.find((p) => p.id === id)) ? `Parent ${pageParent(doc.pages.find((p) => p.id === id))}` : undefined}
              >
                {label(id)}
              </button>
            ))}
          </div>
        ))}
      </div>
      {current && (
        <>
          <Row label="Parent">
            <select
              className="h-6 min-w-0 flex-1 px-1 text-[12px]"
              value={pageParent(current) ?? ""}
              onChange={(e) => void run({ type: "page-parent", page: current.id, parent: e.target.value || null }, "Assign parent")}
            >
              <option value="">None</option>
              {parents.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.id}
                </option>
              ))}
            </select>
          </Row>
          {spread && spread.pages.length > 1 && (
            <Check checked={!!spread.facing} onChange={(facing) => void run({ type: "spread-facing", spread: spread.id, facing }, "Facing pages")}>
              Facing pages (mirrored margins)
            </Check>
          )}
        </>
      )}
    </Section>
  );
}
