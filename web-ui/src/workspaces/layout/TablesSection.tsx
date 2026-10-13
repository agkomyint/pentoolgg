import { useMemo, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Empty, NumberField, Row, Section } from "@/components/ui/kit";
import { useLayout } from "./layoutStore";
import { layoutOf, nextId } from "./layoutModel";
import { TextField } from "./fields";

type Cell = { row?: number; col?: number; paragraphs?: { runs?: { text?: string }[] }[] };
type Table = {
  id: string;
  columns: { width?: number }[];
  rows: { height?: number }[];
  cells?: Cell[];
  header_rows?: number;
  placed?: unknown;
};

const cellText = (cell?: Cell) =>
  (cell?.paragraphs ?? []).map((p) => (p.runs ?? []).map((r) => r.text ?? "").join("")).join("\n");
const MAX_SHOWN = 12;

/** Table editing: create, edit cell text, add/remove rows and columns, merge, place in a story. */
export function TablesSection() {
  const doc = useStudio((s) => s.doc);
  const run = useLayout((s) => s.run);
  const layout = layoutOf(doc) as { tables?: Table[] };
  const tables = layout.tables ?? [];
  const stories = layoutOf(doc).stories ?? [];
  const [pick, setPick] = useState<string | null>(null);
  const [rows, setRows] = useState(3);
  const [cols, setCols] = useState(3);
  const [at, setAt] = useState<{ row: number; col: number }>({ row: 0, col: 0 });
  const table = tables.find((t) => t.id === pick) ?? tables[0];
  const ids = useMemo(() => new Set(tables.map((t) => t.id)), [tables]);
  const act = (action: Record<string, unknown>, label: string) => run({ type: "table", action }, label);

  const cellAt = (r: number, c: number) => table?.cells?.find((x) => (x.row ?? 0) === r && (x.col ?? 0) === c);
  const create = async () => {
    const id = nextId("table", ids);
    if (await act({ type: "create", id, rows, columns: cols, header_rows: 1 }, "Create table")) setPick(id);
  };

  return (
    <Section id="layout-tables" title={`Tables (${tables.length})`} defaultOpen={false}>
      <div className="flex items-center gap-1.5 text-[12px]">
        <NumberField ariaLabel="Rows" min={1} max={200} value={rows} onCommit={(n) => setRows(Math.round(n))} />
        <span className="text-muted-foreground">×</span>
        <NumberField ariaLabel="Columns" min={1} max={40} value={cols} onCommit={(n) => setCols(Math.round(n))} />
        <Button size="sm" onClick={() => void create()}>
          <Plus size={12} /> New
        </Button>
      </div>
      {tables.length === 0 && <Empty>No tables. Create one, then place it in a story.</Empty>}
      {table && (
        <>
          <div className="flex items-center gap-1">
            <select aria-label="Table" className="h-6 min-w-0 flex-1 text-[12px]" value={table.id} onChange={(e) => setPick(e.target.value)}>
              {tables.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.id} ({t.rows.length}×{t.columns.length})
                </option>
              ))}
            </select>
            <Button size="icon" variant="ghost" aria-label="Delete table" onClick={() => void act({ type: "delete", id: table.id }, "Delete table")}>
              <Trash2 size={13} />
            </Button>
          </div>
          <div className="max-h-56 overflow-auto rounded border">
            <table className="w-full border-collapse text-[11px]">
              <tbody>
                {table.rows.slice(0, MAX_SHOWN).map((_, r) => (
                  <tr key={r} className={r < (table.header_rows ?? 0) ? "bg-muted/60" : undefined}>
                    {table.columns.slice(0, MAX_SHOWN).map((__, c) => (
                      <td key={c} className="border p-0" onFocusCapture={() => setAt({ row: r, col: c })}>
                        <TextField
                          ariaLabel={`Cell ${r + 1},${c + 1}`}
                          className="h-6 rounded-none border-0 bg-transparent"
                          value={cellText(cellAt(r, c))}
                          onCommit={(text) => void act({ type: "cell-set", id: table.id, row: r, col: c, text }, "Edit cell")}
                        />
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {(table.rows.length > MAX_SHOWN || table.columns.length > MAX_SHOWN) && (
            <p className="text-[10.5px] text-muted-foreground">Showing the first {MAX_SHOWN} rows and columns.</p>
          )}
          <div className="flex flex-wrap gap-1">
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "add-row", id: table.id, at: at.row + 1, count: 1 }, "Add row")}>
              + Row
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "add-column", id: table.id, at: at.col + 1, count: 1, width: 72 }, "Add column")}>
              + Column
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "remove-row", id: table.id, at: at.row }, "Remove row")}>
              − Row
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "remove-column", id: table.id, at: at.col }, "Remove column")}>
              − Column
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "merge", id: table.id, row: at.row, col: at.col, rows: 1, cols: 2 }, "Merge cells")}>
              Merge →
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "split", id: table.id, row: at.row, col: at.col }, "Split cell")}>
              Split
            </Button>
          </div>
          <Row label="Header rows">
            <NumberField
              ariaLabel="Header rows"
              min={0}
              max={table.rows.length}
              value={table.header_rows ?? 0}
              onCommit={(n) => void act({ type: "set", id: table.id, header_rows: Math.round(n) }, "Header rows")}
            />
          </Row>
          <PlaceTable
            id={table.id}
            stories={stories.map((s) => s.id)}
            placed={!!table.placed}
            onPlace={(action) => void act(action, "Place table")}
          />
        </>
      )}
    </Section>
  );
}

function PlaceTable({
  id,
  stories,
  placed,
  onPlace,
}: {
  id: string;
  stories: string[];
  placed: boolean;
  onPlace: (action: Record<string, unknown>) => void;
}) {
  const [story, setStory] = useState("");
  const [paragraph, setParagraph] = useState(0);
  if (stories.length === 0) return null;
  const target = story || stories[0]!;
  return (
    <div className="flex items-center gap-1 text-[12px]">
      <select aria-label="Story" className="h-6 min-w-0 flex-1" value={target} onChange={(e) => setStory(e.target.value)}>
        {stories.map((s) => (
          <option key={s}>{s}</option>
        ))}
      </select>
      <NumberField ariaLabel="Paragraph" min={0} value={paragraph} onCommit={(n) => setParagraph(Math.round(n))} className="w-12 flex-none" />
      <Button size="sm" onClick={() => onPlace({ type: "place", id, story: target, paragraph })}>
        {placed ? "Move" : "Place"}
      </Button>
    </div>
  );
}
