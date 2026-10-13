import { useMemo } from "react";
import { useStudio } from "@/store";
import { Badge } from "@/components/ui/badge";
import { Empty, Section } from "@/components/ui/kit";
import { hasOverrides, layoutOf } from "./layoutModel";

const KINDS = ["paragraph", "character", "object", "table", "cell"] as const;

/** Styles in use, with how many paragraphs carry local overrides on top. */
export function StylesSection() {
  const doc = useStudio((s) => s.doc);
  const { rows, overridden } = useMemo(() => {
    const layout = layoutOf(doc);
    const usage = new Map<string, number>();
    let overridden = 0;
    for (const story of layout.stories ?? []) {
      for (const p of story.paragraphs) {
        if (p.style) usage.set(p.style, (usage.get(p.style) ?? 0) + 1);
        if (hasOverrides(p)) overridden += 1;
      }
    }
    const rows = KINDS.flatMap((kind) =>
      Object.entries(layout.styles?.[kind] ?? {}).map(([id, style]) => ({
        kind,
        id,
        base: style.based_on ?? null,
        uses: kind === "paragraph" ? (usage.get(id) ?? 0) : null,
      })),
    );
    return { rows, overridden };
  }, [doc]);
  return (
    <Section id="layout-styles" title={`Styles (${rows.length})`} defaultOpen={false}>
      {rows.length === 0 && <Empty>No styles defined. Create them with `pentool layout-style`.</Empty>}
      {overridden > 0 && (
        <p className="text-[11px] text-warning">
          {overridden} paragraph{overridden === 1 ? "" : "s"} carry local overrides.
        </p>
      )}
      <ul className="space-y-0.5">
        {rows.map((s) => (
          <li key={`${s.kind}-${s.id}`} className="flex items-center gap-1.5 text-[12px]">
            <code className="min-w-0 flex-1 truncate">{s.id}</code>
            {s.base && <span className="text-[10.5px] text-muted-foreground">of {s.base}</span>}
            <Badge>{s.kind}</Badge>
            {s.uses !== null && <span className="w-5 text-right text-[10.5px] tabular-nums text-muted-foreground">{s.uses}</span>}
          </li>
        ))}
      </ul>
    </Section>
  );
}
