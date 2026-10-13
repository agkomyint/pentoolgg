import { ArrowLeft, ArrowRight, Copy, FilePlus2, Trash2 } from "lucide-react";
import { useStudio, collectIds } from "@/store";
import { Button } from "@/components/ui/button";
import { Tip } from "@/components/ui/tooltip";
import { Check, Row, Section } from "@/components/ui/kit";
import { cn } from "@/lib/utils";
import { useLayout } from "./layoutStore";
import { layoutOf, nextId, pageParent } from "./layoutModel";

/** Pages panel: spreads as rows of page chips, parent assignment, page edits. */
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
  const label = (id: string) => doc.pages.find((p) => p.id === id)?.name ?? id;
  const fresh = () => nextId("page", collectIds(doc));

  const add = async () => {
    const id = fresh();
    if (await run({ type: "page-add", id, after: current?.id, parent: pageParent(current) }, "Add page")) setPage(id);
  };
  const duplicate = async () => {
    if (!current) return;
    const id = fresh();
    if (await run({ type: "page-duplicate", id: current.id, new_id: id }, "Duplicate page")) setPage(id);
  };
  const remove = async () => {
    if (!current || doc.pages.length < 2) return;
    if (!window.confirm(`Remove ${label(current.id)}? Stories only on this page are kept unplaced.`)) return;
    const neighbour = doc.pages[index + 1] ?? doc.pages[index - 1];
    if (await run({ type: "page-remove", id: current.id }, "Remove page") && neighbour) setPage(neighbour.id);
  };
  const move = (by: number) => current && void run({ type: "page-move", id: current.id, index: index + by }, "Move page");

  return (
    <Section
      id="layout-pages"
      title={`Pages (${doc.pages.length})`}
      actions={
        <span className="flex gap-0.5">
          <Tip label="Add page after this one">
            <Button variant="ghost" size="icon" className="h-5 w-5" aria-label="Add page" onClick={add}>
              <FilePlus2 size={13} />
            </Button>
          </Tip>
          <Tip label="Duplicate page">
            <Button variant="ghost" size="icon" className="h-5 w-5" aria-label="Duplicate page" onClick={duplicate}>
              <Copy size={13} />
            </Button>
          </Tip>
          <Tip label="Move earlier">
            <Button variant="ghost" size="icon" className="h-5 w-5" aria-label="Move page earlier" disabled={index < 1} onClick={() => move(-1)}>
              <ArrowLeft size={13} />
            </Button>
          </Tip>
          <Tip label="Move later">
            <Button
              variant="ghost"
              size="icon"
              className="h-5 w-5"
              aria-label="Move page later"
              disabled={index < 0 || index >= doc.pages.length - 1}
              onClick={() => move(1)}
            >
              <ArrowRight size={13} />
            </Button>
          </Tip>
          <Tip label="Remove page">
            <Button variant="ghost" size="icon" className="h-5 w-5" aria-label="Remove page" disabled={doc.pages.length < 2} onClick={remove}>
              <Trash2 size={13} />
            </Button>
          </Tip>
        </span>
      }
    >
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
