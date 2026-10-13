import { useStudio } from "@/store";
import { findNode, pageOf } from "@/lib/model";
import { NumberField, Row, Section, Segmented } from "@/components/ui/kit";
import { useLayout } from "./layoutStore";

type Align = "top" | "center" | "bottom" | "justify";

/** Options of the selected text frame; edits run through `frame set`. */
export function FrameSection() {
  const { doc, pageId, selectedId } = useStudio();
  const run = useLayout((s) => s.run);
  const node = selectedId ? findNode(pageOf(doc, pageId), selectedId)?.node : null;
  if (!node || node.kind !== "text-frame") return null;
  const num = (key: string, fallback: number) => (typeof node[key] === "number" ? (node[key] as number) : fallback);
  const inset = Array.isArray(node.inset) ? (node.inset as number[]).map(Number) : [0, 0, 0, 0];
  const set = (options: Record<string, unknown>) =>
    void run({ type: "frame", action: { type: "set", id: node.id, ...options } }, "Frame options");
  const align = (typeof node.vertical_align === "string" ? node.vertical_align : "top") as Align;
  return (
    <Section id="layout-frame" title={`Frame ${node.id}`}>
      <div className="grid grid-cols-2 gap-x-2 gap-y-1">
        {(["x", "y", "width", "height"] as const).map((key) => (
          <Row key={key} label={key === "width" ? "W" : key === "height" ? "H" : key.toUpperCase()} className="[&>span:first-child]:w-4">
            <NumberField ariaLabel={key} value={num(key, 0)} onCommit={(n) => set({ [key]: n })} />
          </Row>
        ))}
      </div>
      <Row label="Columns">
        <NumberField ariaLabel="Columns" min={1} max={40} value={num("columns", 1)} onCommit={(n) => set({ columns: Math.round(n) })} />
      </Row>
      <Row label="Gutter">
        <NumberField ariaLabel="Gutter" min={0} value={num("gutter", 12)} onCommit={(n) => set({ gutter: n })} />
      </Row>
      <Row label="Inset">
        <NumberField
          ariaLabel="Inset (all sides)"
          min={0}
          value={inset[0] ?? 0}
          onCommit={(n) => set({ inset: [n, n, n, n].join(",") })}
        />
      </Row>
      <Row label="Align">
        <Segmented
          ariaLabel="Vertical alignment"
          value={align}
          onChange={(v) => set({ vertical_align: v })}
          options={[
            { value: "top", label: "Top" },
            { value: "center", label: "Mid" },
            { value: "bottom", label: "Bot" },
            { value: "justify", label: "Just" },
          ]}
        />
      </Row>
    </Section>
  );
}
