import { useState } from "react";
import { useStudio } from "@/store";
import { findNode, pageOf } from "@/lib/model";
import { Button } from "@/components/ui/button";
import { NumberField, Row, Section } from "@/components/ui/kit";
import { useLayout } from "./layoutStore";
import { layoutOf } from "./layoutModel";

const WRAPS = ["none", "bounding-box", "shape", "alpha", "clipping-path", "jump-object", "jump-next-column"] as const;
const SIDES = ["both", "left", "right", "towards-spine", "away-from-spine", "largest"] as const;
const MODES = ["inline", "above-line", "custom"] as const;
const ALIGNS = ["left", "center", "right"] as const;

type Wrap = { mode?: string; offset?: number[]; side?: string };

/** Text wrap and anchoring of the selected object (everything but text frames). */
export function WrapSection() {
  const { doc, pageId, selectedId } = useStudio();
  const run = useLayout((s) => s.run);
  const node = selectedId ? findNode(pageOf(doc, pageId), selectedId)?.node : null;
  const [story, setStory] = useState("");
  const [paragraph, setParagraph] = useState(0);
  const [mode, setMode] = useState<(typeof MODES)[number]>("inline");
  const [align, setAlign] = useState<(typeof ALIGNS)[number]>("left");
  const [offset, setOffset] = useState(0);
  if (!node || node.kind === "text-frame") return null;

  const stories = layoutOf(doc).stories ?? [];
  const target = story || stories[0]?.id || "";
  const wrap = (node as { wrap?: Wrap }).wrap;
  const anchored = stories.some((s) => s.paragraphs.some((p) => p.runs.some((r) => (r as { anchor?: string }).anchor === node.id)));
  const wrapOp = (action: Record<string, unknown>, label: string) => void run({ type: "wrap", action }, label);
  const options = { mode, ...(mode === "above-line" ? { align } : {}), ...(mode === "custom" ? { x_offset: offset } : {}) };
  const margin = wrap?.offset?.[0] ?? 0;

  return (
    <Section id="layout-wrap" title={`Wrap & anchor · ${node.id}`}>
      <Row label="Text wrap">
        <select
          aria-label="Text wrap"
          className="h-6 min-w-0 flex-1 text-[12px]"
          value={wrap?.mode ?? "none"}
          onChange={(e) => {
            const value = e.target.value;
            if (value === "none") wrapOp({ type: "clear", id: node.id }, "Clear wrap");
            else wrapOp({ type: "set", id: node.id, mode: value, offset: wrap?.offset ? wrap.offset.join(",") : "6,6,6,6", side: wrap?.side }, "Text wrap");
          }}
        >
          {WRAPS.map((w) => (
            <option key={w}>{w}</option>
          ))}
        </select>
      </Row>
      {wrap?.mode && wrap.mode !== "none" && (
        <>
          <Row label="Offset">
            <NumberField
              ariaLabel="Wrap offset (all sides)"
              min={0}
              value={margin}
              onCommit={(n) => wrapOp({ type: "set", id: node.id, mode: wrap.mode, offset: [n, n, n, n].join(","), side: wrap.side }, "Wrap offset")}
            />
          </Row>
          <Row label="Side">
            <select
              aria-label="Wrap side"
              className="h-6 min-w-0 flex-1 text-[12px]"
              value={wrap.side ?? "both"}
              onChange={(e) => wrapOp({ type: "set", id: node.id, mode: wrap.mode, side: e.target.value }, "Wrap side")}
            >
              {SIDES.map((w) => (
                <option key={w}>{w}</option>
              ))}
            </select>
          </Row>
        </>
      )}
      {stories.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <Row label="Anchor">
            <select aria-label="Anchor mode" className="h-6 min-w-0 flex-1 text-[12px]" value={mode} onChange={(e) => setMode(e.target.value as typeof mode)}>
              {MODES.map((m) => (
                <option key={m}>{m}</option>
              ))}
            </select>
          </Row>
          {mode === "above-line" && (
            <Row label="Align">
              <select aria-label="Anchor align" className="h-6 min-w-0 flex-1 text-[12px]" value={align} onChange={(e) => setAlign(e.target.value as typeof align)}>
                {ALIGNS.map((m) => (
                  <option key={m}>{m}</option>
                ))}
              </select>
            </Row>
          )}
          {mode === "custom" && (
            <Row label="X offset">
              <NumberField ariaLabel="Anchor x offset" value={offset} onCommit={setOffset} />
            </Row>
          )}
          {anchored ? (
            <div className="flex gap-1">
              <Button size="sm" onClick={() => wrapOp({ type: "anchor-set", object: node.id, options }, "Anchor options")}>
                Apply
              </Button>
              <Button size="sm" variant="ghost" onClick={() => wrapOp({ type: "anchor-remove", object: node.id }, "Detach anchor")}>
                Detach
              </Button>
            </div>
          ) : (
            <div className="flex items-center gap-1 text-[12px]">
              <select aria-label="Anchor story" className="h-6 min-w-0 flex-1" value={target} onChange={(e) => setStory(e.target.value)}>
                {stories.map((s) => (
                  <option key={s.id}>{s.id}</option>
                ))}
              </select>
              <NumberField ariaLabel="Anchor paragraph" min={0} value={paragraph} onCommit={(n) => setParagraph(Math.round(n))} className="w-12 flex-none" />
              <Button size="sm" onClick={() => wrapOp({ type: "anchor-add", object: node.id, story: target, paragraph, options }, "Anchor object")}>
                Anchor
              </Button>
            </div>
          )}
        </div>
      )}
    </Section>
  );
}
