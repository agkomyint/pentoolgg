import { useEffect, useMemo, useState } from "react";
import { Link2, Link2Off, Plus, Trash2, WrapText } from "lucide-react";
import { useStudio } from "@/store";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tip } from "@/components/ui/tooltip";
import { Empty, Section } from "@/components/ui/kit";
import { collectIds } from "@/lib/model";
import { cn } from "@/lib/utils";
import { useLayout } from "./layoutStore";
import {
  hasOverrides,
  isPlainParagraph,
  layoutOf,
  nextId,
  paragraphText,
  type Paragraph,
  type Story,
} from "./layoutModel";

/** One paragraph: a text box that keeps its style, runs and overrides. */
function ParagraphRow({
  story,
  index,
  paragraph,
  styles,
}: {
  story: Story;
  index: number;
  paragraph: Paragraph;
  styles: string[];
}) {
  const edit = useStudio((s) => s.edit);
  const plain = isPlainParagraph(paragraph);
  const [text, setText] = useState(paragraphText(paragraph));
  useEffect(() => setText(paragraphText(paragraph)), [paragraph]);
  const mutate = (label: string, change: (list: Paragraph[], at: number) => void) =>
    edit(label, (doc) => {
      const target = layoutOf(doc).stories?.find((s) => s.id === story.id);
      if (target) change(target.paragraphs, index);
    });
  const commit = () => {
    if (!plain || text === paragraphText(paragraph)) return;
    const lines = text.split(/\r\n?|\n/);
    mutate("Edit story text", (list, at) => {
      const original = list[at]!;
      const first = original.runs[0]!;
      original.runs = [{ ...first, text: lines[0] ?? "" }];
      lines.slice(1).forEach((line, k) => {
        const copy: Paragraph = { runs: [{ text: line }] };
        if (original.style) copy.style = original.style;
        list.splice(at + 1 + k, 0, copy);
      });
    });
  };
  return (
    <div className="space-y-1 rounded border bg-input p-1.5">
      <div className="flex items-center gap-1">
        <select
          aria-label="Paragraph style"
          className="h-5 min-w-0 flex-1 px-1 text-[11px]"
          value={paragraph.style ?? ""}
          onChange={(e) =>
            mutate("Paragraph style", (list, at) => {
              if (e.target.value) list[at]!.style = e.target.value;
              else delete list[at]!.style;
            })
          }
        >
          <option value="">No style</option>
          {styles.map((id) => (
            <option key={id} value={id}>
              {id}
            </option>
          ))}
        </select>
        {hasOverrides(paragraph) && (
          <Tip label="Local overrides. Click to clear them.">
            <button
              className="rounded bg-warning/20 px-1 text-[10px] text-warning"
              onClick={() => mutate("Clear overrides", (list, at) => delete list[at]!.overrides)}
            >
              + overrides
            </button>
          </Tip>
        )}
        <Tip label="Remove paragraph">
          <button
            className="text-muted-foreground hover:text-destructive disabled:opacity-30"
            aria-label="Remove paragraph"
            disabled={story.paragraphs.length < 2}
            onClick={() => mutate("Remove paragraph", (list, at) => void list.splice(at, 1))}
          >
            <Trash2 size={12} />
          </button>
        </Tip>
      </div>
      {plain ? (
        <textarea
          aria-label={`Paragraph ${index + 1}`}
          className="block min-h-10 w-full resize-y border-0 bg-transparent px-0.5 text-[12px] focus-visible:outline-0"
          value={text}
          rows={Math.min(8, Math.max(2, Math.ceil(text.length / 38)))}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
        />
      ) : (
        <p className="px-0.5 text-[12px] text-muted-foreground" title="Edit styled runs with the text story CLI">
          {paragraphText(paragraph) || "(no text)"}{" "}
          <Badge>{paragraph.runs.length} runs, read-only</Badge>
        </p>
      )}
    </div>
  );
}

export function StoriesSection() {
  const { doc, pageId, layerId, selectedId, select } = useStudio();
  const { overview, error, run, threadFrom, setThreadFrom } = useLayout();
  const [open, setOpen] = useState<string | null>(null);
  const stories = layoutOf(doc).stories ?? [];
  const styles = useMemo(() => Object.keys(layoutOf(doc).styles?.paragraph ?? {}), [doc]);
  const rows = new Map((overview?.stories ?? []).map((s) => [s.id, s]));
  const frames = overview?.frames ?? [];
  const selectedFrame = frames.find((f) => f.id === selectedId);
  const active = stories.find((s) => s.id === (open ?? selectedFrame?.story));

  // Click-to-thread: the next frame selected on the canvas becomes the successor.
  useEffect(() => {
    if (!threadFrom || !selectedId || selectedId === threadFrom) return;
    if (!frames.some((f) => f.id === selectedId)) return;
    setThreadFrom(null);
    void run({ type: "frame", action: { type: "thread", from: threadFrom, to: selectedId } }, "Thread frames");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedId]);

  const addStory = async () => {
    if (!doc || !pageId || !layerId) return;
    const taken = collectIds(doc);
    const story = nextId("story", new Set([...taken, ...stories.map((s) => s.id)]));
    const frame = nextId("frame", taken);
    const page = doc.pages.find((p) => p.id === pageId);
    const w = page?.canvas?.width ?? 612;
    const h = page?.canvas?.height ?? 792;
    const made = await run({ type: "story", action: { type: "add", id: story, text: "New story. Replace this text." } }, "Add story");
    if (!made) return;
    await run(
      {
        type: "frame",
        action: { type: "add", id: frame, story, page: pageId, layer: layerId, x: w * 0.1, y: h * 0.1, width: w * 0.8, height: h * 0.3 },
      },
      "Add text frame",
    );
    setOpen(story);
    select(frame);
  };

  return (
    <Section
      id="layout-stories"
      title={`Stories (${stories.length})`}
      actions={
        <Tip label="New story with a text frame on this page">
          <Button variant="ghost" size="icon" className="h-5 w-5" aria-label="New story" onClick={addStory}>
            <Plus size={13} />
          </Button>
        </Tip>
      }
    >
      {error && <p className="rounded border border-destructive/50 px-2 py-1 text-[11px] text-destructive">{error}</p>}
      {stories.length === 0 && <Empty>No stories yet. Use + to add a story with a frame.</Empty>}
      <ul className="space-y-1">
        {stories.map((story) => {
          const row = rows.get(story.id);
          const chain = frames.filter((f) => f.story === story.id);
          const isOpen = active?.id === story.id;
          return (
            <li key={story.id} className={cn("rounded border", isOpen && "border-primary")}>
              <button
                className="flex w-full items-center gap-1.5 px-2 py-1 text-left"
                aria-expanded={isOpen}
                onClick={() => setOpen(isOpen ? "" : story.id)}
              >
                <code className="min-w-0 flex-1 truncate text-[11px]">{story.id}</code>
                {row?.overset && <Badge variant="destructive">overset</Badge>}
                {row && !row.placed && <Badge variant="warning">unplaced</Badge>}
                <span className="text-[10.5px] text-muted-foreground">
                  {chain.length} frame{chain.length === 1 ? "" : "s"}
                </span>
              </button>
              {isOpen && (
                <div className="space-y-1.5 border-t p-1.5">
                  {chain.length > 0 && (
                    <div className="flex flex-wrap items-center gap-1">
                      {chain.map((f, i) => (
                        <span key={f.id} className="flex items-center gap-1">
                          {i > 0 && <span className="text-muted-foreground">→</span>}
                          <button
                            className={cn(
                              "rounded border px-1 text-[10.5px] hover:border-primary",
                              f.id === selectedId && "border-primary bg-accent",
                              f.overset && "text-destructive",
                            )}
                            onClick={() => select(f.id)}
                          >
                            {f.id}
                          </button>
                        </span>
                      ))}
                    </div>
                  )}
                  <div className="flex flex-wrap gap-1">
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={!selectedFrame || selectedFrame.story !== story.id}
                      onClick={() => selectedFrame && setThreadFrom(threadFrom ? null : selectedFrame.id)}
                    >
                      <Link2 size={12} /> {threadFrom ? "Pick next frame…" : "Thread to…"}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={!selectedFrame || selectedFrame.story !== story.id || !selectedFrame.next}
                      onClick={() => selectedFrame && void run({ type: "frame", action: { type: "unthread", id: selectedFrame.id } }, "Unthread")}
                    >
                      <Link2Off size={12} /> Unthread
                    </Button>
                    <Tip label="Add pages from the last page's parent until the text fits">
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={!row?.overset}
                        onClick={() => void run({ type: "story", action: { type: "flow", id: story.id } }, "Flow story")}
                      >
                        <WrapText size={12} /> Autoflow
                      </Button>
                    </Tip>
                  </div>
                  {threadFrom && (
                    <p className="text-[11px] text-primary">Click the frame that should receive the overflow of {threadFrom}.</p>
                  )}
                  {story.paragraphs.map((p, i) => (
                    <ParagraphRow key={`${story.id}-${i}`} story={story} index={i} paragraph={p} styles={styles} />
                  ))}
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() =>
                      useStudio.getState().edit("Add paragraph", (d) => {
                        const target = layoutOf(d).stories?.find((s) => s.id === story.id);
                        target?.paragraphs.push({ runs: [{ text: "" }] });
                      })
                    }
                  >
                    <Plus size={12} /> Paragraph
                  </Button>
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </Section>
  );
}
