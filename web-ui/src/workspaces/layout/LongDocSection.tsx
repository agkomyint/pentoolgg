import { useState } from "react";
import { RefreshCw, Trash2 } from "lucide-react";
import { useStudio } from "@/store";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Empty, NumberField, Row, Section } from "@/components/ui/kit";
import { useLayout } from "./layoutStore";
import { layoutOf } from "./layoutModel";
import { TextField } from "./fields";

type Generated = { id: string; kind: string; story: string; stale?: boolean };
type Link = { id: string; text?: string; url?: string; page?: string; bookmark?: string };
type Bookmark = { id: string; name?: string };
type Xref = { id: string; format?: string };
type Topic = { id: string; text: string };
type LongDoc = {
  generated?: Generated[];
  hyperlinks?: Link[];
  bookmarks?: Bookmark[];
  xrefs?: Xref[];
  index_topics?: Topic[];
};

/** Contents, index, cross-references and the links panel (stale flags come from Rust). */
export function LongDocSection() {
  const doc = useStudio((s) => s.doc);
  const run = useLayout((s) => s.run);
  const layout = layoutOf(doc) as ReturnType<typeof layoutOf> & LongDoc;
  const stories = layout.stories ?? [];
  const paragraphStyles = Object.keys(layout.styles?.paragraph ?? {});
  const [story, setStory] = useState("");
  const [paragraph, setParagraph] = useState(0);
  const [text, setText] = useState("link");
  const target = story || stories[0]?.id || "";
  const act = (action: Record<string, unknown>, label: string) => run({ type: "doc", action }, label);
  const generated = layout.generated ?? [];
  const known = new Set(generated.map((g) => g.id));
  const fresh = (base: string) => {
    for (let i = 1; ; i += 1) if (!known.has(`${base}-${i}`)) return `${base}-${i}`;
  };
  const total =
    generated.length +
    (layout.hyperlinks?.length ?? 0) +
    (layout.bookmarks?.length ?? 0) +
    (layout.xrefs?.length ?? 0) +
    (layout.index_topics?.length ?? 0);
  const headings = paragraphStyles.filter((s) => /^h[1-6]$|heading|title/i.test(s)).slice(0, 4);

  return (
    <Section id="layout-longdoc" title={`Contents & links (${total})`} defaultOpen={false}>
      <div className="flex flex-wrap gap-1">
        <Button
          size="sm"
          onClick={() =>
            void act(
              { type: "generated-add", id: fresh("toc"), kind: "toc", story: fresh("toc-story"), styles: headings.length ? headings : paragraphStyles.slice(0, 1) },
              "Add table of contents",
            )
          }
        >
          + Contents
        </Button>
        <Button size="sm" onClick={() => void act({ type: "generated-add", id: fresh("index"), kind: "index", story: fresh("index-story") }, "Add index")}>
          + Index
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void act({ type: "generated-check" }, "Check generated stories")}>
          <RefreshCw size={12} /> Check
        </Button>
      </div>
      {generated.length === 0 && <Empty>No generated contents or index yet.</Empty>}
      <ul className="space-y-0.5">
        {generated.map((g) => (
          <li key={g.id} className="flex items-center gap-1.5 text-[12px]">
            <code className="min-w-0 flex-1 truncate">{g.id}</code>
            <Badge>{g.kind}</Badge>
            {g.stale && <span className="text-[10.5px] text-warning">stale</span>}
            <Button size="sm" variant="ghost" onClick={() => void act({ type: "generate", id: g.id }, "Regenerate")}>
              Update
            </Button>
            <Button size="icon" variant="ghost" aria-label={`Remove ${g.id}`} onClick={() => void act({ type: "generated-remove", id: g.id }, "Remove generated")}>
              <Trash2 size={12} />
            </Button>
          </li>
        ))}
      </ul>

      {stories.length > 0 && (
        <div className="space-y-1.5 border-t pt-2">
          <Row label="Insert in">
            <select aria-label="Story" className="h-6 min-w-0 flex-1 text-[12px]" value={target} onChange={(e) => setStory(e.target.value)}>
              {stories.map((s) => (
                <option key={s.id}>{s.id}</option>
              ))}
            </select>
            <NumberField ariaLabel="Paragraph" min={0} value={paragraph} onCommit={(n) => setParagraph(Math.round(n))} className="w-12 flex-none" />
          </Row>
          <Row label="Text">
            <TextField ariaLabel="Link text" value={text} onCommit={setText} />
          </Row>
          <div className="flex flex-wrap gap-1">
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                const url = window.prompt("URL (http, https or mailto)", "https://");
                if (url) void act({ type: "link-add", id: `link-${(layout.hyperlinks?.length ?? 0) + 1}`, story: target, paragraph, text, url }, "Add link");
              }}
            >
              Link
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => void act({ type: "bookmark-add", id: `bm-${(layout.bookmarks?.length ?? 0) + 1}`, story: target, paragraph }, "Add bookmark")}
            >
              Bookmark
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={(layout.bookmarks?.length ?? 0) === 0}
              onClick={() =>
                void act(
                  {
                    type: "xref-add",
                    id: `xref-${(layout.xrefs?.length ?? 0) + 1}`,
                    story: target,
                    paragraph,
                    format: "page",
                    target_bookmark: layout.bookmarks?.[0]?.id,
                  },
                  "Add cross-reference",
                )
              }
            >
              Cross-ref
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                const topic = window.prompt("Index topic");
                if (topic) void act({ type: "index-topic", id: `topic-${(layout.index_topics?.length ?? 0) + 1}`, text: topic }, "Add index topic");
              }}
            >
              Topic
            </Button>
          </div>
        </div>
      )}

      <Items title="Links" rows={(layout.hyperlinks ?? []).map((l) => ({ id: l.id, note: l.url ?? l.page ?? l.bookmark ?? "" }))} onRemove={(id) => void act({ type: "link-remove", id }, "Remove link")} />
      <Items title="Bookmarks" rows={(layout.bookmarks ?? []).map((b) => ({ id: b.id, note: b.name ?? "" }))} onRemove={(id) => void act({ type: "bookmark-remove", id }, "Remove bookmark")} />
      <Items title="Cross-references" rows={(layout.xrefs ?? []).map((x) => ({ id: x.id, note: x.format ?? "" }))} onRemove={(id) => void act({ type: "xref-remove", id }, "Remove cross-reference")} />
      <Items title="Index topics" rows={(layout.index_topics ?? []).map((t) => ({ id: t.id, note: t.text }))} onRemove={undefined} />
    </Section>
  );
}

function Items({ title, rows, onRemove }: { title: string; rows: { id: string; note: string }[]; onRemove?: (id: string) => void }) {
  if (rows.length === 0) return null;
  return (
    <div>
      <h4 className="mb-0.5 text-[10.5px] font-semibold uppercase tracking-wide text-muted-foreground">{title}</h4>
      <ul className="space-y-0.5">
        {rows.map((r) => (
          <li key={r.id} className="flex items-center gap-1.5 text-[12px]">
            <code className="shrink-0">{r.id}</code>
            <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">{r.note}</span>
            {onRemove && (
              <Button size="icon" variant="ghost" aria-label={`Remove ${r.id}`} onClick={() => onRemove(r.id)}>
                <Trash2 size={12} />
              </Button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
