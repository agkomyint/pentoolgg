import { useMemo, useState } from "react";
import { Plus, Trash2 } from "lucide-react";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Empty, Section } from "@/components/ui/kit";
import { useLayout } from "./layoutStore";
import { TextField } from "./fields";
import { hasOverrides, layoutOf } from "./layoutModel";

const KINDS = ["paragraph", "character", "object", "table", "cell"] as const;
type Kind = (typeof KINDS)[number];
const META = new Set(["based_on", "next", "group"]);

const show = (value: unknown) => (typeof value === "string" ? value : JSON.stringify(value));

/** Styles of every kind: usage, local-override counts, and property editing through the Rust style planner. */
export function StylesSection() {
  const doc = useStudio((s) => s.doc);
  const run = useLayout((s) => s.run);
  const [kind, setKind] = useState<Kind>("paragraph");
  const [open, setOpen] = useState<string | null>(null);
  const [newKey, setNewKey] = useState("");
  const { rows, overridden, layout } = useMemo(() => {
    const layout = layoutOf(doc);
    const usage = new Map<string, number>();
    let overridden = 0;
    for (const story of layout.stories ?? []) {
      for (const p of story.paragraphs) {
        if (p.style) usage.set(p.style, (usage.get(p.style) ?? 0) + 1);
        if (hasOverrides(p)) overridden += 1;
      }
    }
    const rows = KINDS.flatMap((k) =>
      Object.entries(layout.styles?.[k] ?? {}).map(([id, style]) => ({
        kind: k,
        id,
        style: style as Record<string, unknown>,
        base: style.based_on ?? null,
        uses: k === "paragraph" ? (usage.get(id) ?? 0) : null,
      })),
    );
    return { rows, overridden, layout };
  }, [doc]);
  const shown = rows.filter((r) => r.kind === kind);
  const act = (action: Record<string, unknown>, label: string) => run({ type: "style", action }, label);
  const clearAll = () => {
    for (const story of layout.stories ?? []) void act({ type: "clear-overrides", story: story.id }, "Clear overrides");
  };

  return (
    <Section id="layout-styles" title={`Styles (${rows.length})`} defaultOpen={false}>
      <div className="flex flex-wrap gap-1">
        {KINDS.map((k) => (
          <button
            key={k}
            onClick={() => setKind(k)}
            className={`rounded px-1.5 py-0.5 text-[11px] ${k === kind ? "bg-primary text-primary-foreground" : "text-muted-foreground hover:text-foreground"}`}
          >
            {k} {rows.filter((r) => r.kind === k).length || ""}
          </button>
        ))}
      </div>
      {overridden > 0 && (
        <div className="flex items-center gap-1.5 text-[11px] text-warning">
          <span className="flex-1">
            {overridden} paragraph{overridden === 1 ? "" : "s"} carry local overrides.
          </span>
          <Button size="sm" variant="ghost" onClick={clearAll}>
            Clear all
          </Button>
        </div>
      )}
      {shown.length === 0 && <Empty>No {kind} styles yet.</Empty>}
      <ul className="space-y-0.5">
        {shown.map((s) => (
          <li key={s.id} className="text-[12px]">
            <div className="flex items-center gap-1.5">
              <button className="min-w-0 flex-1 truncate text-left" onClick={() => setOpen(open === s.id ? null : s.id)}>
                <code>{s.id}</code>
                {s.base && <span className="ml-1 text-[10.5px] text-muted-foreground">of {s.base}</span>}
              </button>
              {s.uses !== null && <span className="w-5 text-right text-[10.5px] tabular-nums text-muted-foreground">{s.uses}</span>}
              <Button
                size="icon"
                variant="ghost"
                aria-label={`Remove style ${s.id}`}
                onClick={() => void act({ type: "remove", kind, id: s.id }, "Remove style")}
              >
                <Trash2 size={12} />
              </Button>
            </div>
            {open === s.id && (
              <div className="mt-1 space-y-1 rounded border p-1.5">
                {Object.entries(s.style)
                  .filter(([key]) => !META.has(key))
                  .map(([key, value]) => (
                    <div key={key} className="flex items-center gap-1">
                      <span className="w-24 shrink-0 truncate text-[11px] text-muted-foreground">{key}</span>
                      <TextField
                        ariaLabel={`${s.id} ${key}`}
                        value={show(value)}
                        onCommit={(v) => void act({ type: "set", kind, id: s.id, set: [`${key}=${v}`] }, "Edit style")}
                      />
                      <Button size="icon" variant="ghost" aria-label={`Unset ${key}`} onClick={() => void act({ type: "set", kind, id: s.id, unset: [key] }, "Unset property")}>
                        <Trash2 size={11} />
                      </Button>
                    </div>
                  ))}
                <div className="flex items-center gap-1">
                  <TextField ariaLabel="New property" placeholder="property=value, e.g. size=12" value={newKey} onCommit={(v) => {
                    setNewKey("");
                    if (v.includes("=")) void act({ type: "set", kind, id: s.id, set: [v] }, "Edit style");
                  }} />
                </div>
              </div>
            )}
          </li>
        ))}
      </ul>
      <div className="flex items-center gap-1">
        <TextField
          ariaLabel="New style name"
          placeholder={`New ${kind} style…`}
          value=""
          onCommit={(id) => {
            const clean = id.trim();
            if (clean) void act({ type: "create", kind, id: clean }, "Create style");
          }}
        />
        <Plus size={12} className="text-muted-foreground" />
      </div>
    </Section>
  );
}
