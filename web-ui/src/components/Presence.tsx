import { useEffect, useMemo, useRef, useState } from "react";
import { Bot, History, RotateCcw, X } from "lucide-react";
import { api, type HistoryActor, type HistoryEntry, type Page } from "@/lib/api";
import { findNode, pageBounds, unite, type Rect } from "@/lib/model";
import { useStudio, type RemoteChange } from "@/store";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

const PALETTE = ["#7c3aed", "#db2777", "#ea580c", "#0891b2", "#16a34a", "#2563eb", "#c026d3", "#d97706"];

/** A stable colour per actor, like a collaborator's cursor colour in Figma or Docs. */
export function actorColor(actor: HistoryActor | null | undefined): string {
  const key = `${actor?.kind ?? ""}:${actor?.name ?? ""}`;
  let hash = 0;
  for (const ch of key) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return PALETTE[hash % PALETTE.length]!;
}

export function actorName(actor: HistoryActor | null | undefined): string {
  return actor?.name ?? (actor?.kind === "agent" ? "An agent" : actor ? "Someone" : "Another writer");
}

const OPERATIONS: Record<string, string> = {
  "browser-save": "Saved in Studio",
  restore: "Restored a version",
  undo: "Undid a change",
  redo: "Redid a change",
  "collab-sync": "Synced with a peer",
  "ai-accept": "Accepted an AI proposal",
};

export function operationLabel(operation: string | null | undefined): string {
  if (!operation) return "Changed the document";
  return OPERATIONS[operation] ?? operation.replace(/[-_]+/g, " ").replace(/^\w/, (c) => c.toUpperCase());
}

export function ago(ms: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 10) return "just now";
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(ms).toLocaleString();
}

function Avatar({ actor, size = 22 }: { actor: HistoryActor | null | undefined; size?: number }) {
  const agent = actor?.kind === "agent";
  return (
    <span
      className={cn("presence-avatar inline-grid shrink-0 place-items-center rounded-full font-semibold text-white", agent && "presence-avatar-agent")}
      style={{ width: size, height: size, background: actorColor(actor), fontSize: size * 0.45 }}
      aria-hidden="true"
    >
      {agent ? <Bot size={size * 0.6} /> : actorName(actor).slice(0, 1).toUpperCase()}
    </span>
  );
}

const CURSOR = "M0 0 L0 16 L4.5 12 L7.5 19 L10 18 L7 11 L13 11 Z";

/**
 * Canvas overlay (page coordinates; `px` is one screen pixel) for the newest
 * remote change: a glowing outline in the actor's colour around each changed
 * node, and the actor's cursor gliding to it with a name tag. Agents get a
 * sparkle trail so a change made by AI is easy to tell apart.
 */
export function RemoteActivity({ page, px }: { page: Page | undefined; px: number }) {
  const change = useStudio((st) => st.remoteChange);
  const [shown, setShown] = useState<RemoteChange | null>(null);
  const [fading, setFading] = useState(false);
  const [at, setAt] = useState<[number, number] | null>(null);
  const last = useRef<[number, number] | null>(null);

  const boxes = useMemo(() => {
    if (!shown || shown.pageId !== page?.id) return [];
    const out: { id: string; box: Rect }[] = [];
    for (const id of shown.ids.slice(0, 40)) {
      const found = findNode(page, id);
      const box = found ? pageBounds(found) : null;
      if (box) out.push({ id, box });
    }
    return out;
  }, [shown, page]);
  const union = useMemo(() => boxes.reduce<Rect | null>((u, b) => unite(u, b.box), null), [boxes]);

  useEffect(() => {
    if (!change || Date.now() - change.at > 5000) return;
    setShown(change);
    setFading(false);
    const fade = window.setTimeout(() => setFading(true), 3400);
    const hide = window.setTimeout(() => setShown(null), 4400);
    return () => {
      window.clearTimeout(fade);
      window.clearTimeout(hide);
    };
  }, [change]);

  // Glide from where the cursor last was (or just outside the change) to the change.
  useEffect(() => {
    if (!union) return;
    const target: [number, number] = [union.x + union.width, union.y + union.height];
    const from = last.current ?? [union.x - 90 * px, union.y - 60 * px];
    setAt(from);
    const frame = window.requestAnimationFrame(() => window.requestAnimationFrame(() => setAt(target)));
    last.current = target;
    return () => window.cancelAnimationFrame(frame);
    // Only a new change moves the cursor; zooming keeps it where it is.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown?.seq, union?.x, union?.y, union?.width, union?.height]);

  if (!shown || !boxes.length) return null;
  const color = actorColor(shown.actor);
  const agent = shown.actor?.kind === "agent";
  const name = `${agent ? "✦ " : ""}${actorName(shown.actor)}`;
  const sub = shown.actor?.directed_by ? `for ${shown.actor.directed_by}` : "";
  const label = sub ? `${name} · ${sub}` : name;
  const pill = label.length * 6.4 + 16;
  return (
    <g className={cn("remote-activity", fading && "remote-activity-fading")} pointerEvents="none" aria-hidden="true" style={{ color }}>
      {boxes.map(({ id, box }) => (
        <g key={`${shown.seq}-${id}`}>
          <rect className="remote-flash" x={box.x} y={box.y} width={box.width} height={box.height} fill={color} rx={3 * px} />
          <rect
            className={cn("remote-outline", agent && "remote-outline-agent")}
            x={box.x - 3 * px}
            y={box.y - 3 * px}
            width={box.width + 6 * px}
            height={box.height + 6 * px}
            rx={4 * px}
            fill="none"
            stroke={color}
            strokeWidth={2 * px}
            strokeDasharray={agent ? `${6 * px} ${4 * px}` : undefined}
          />
        </g>
      ))}
      {at && (
        <g className="remote-cursor" style={{ transform: `translate(${at[0]}px, ${at[1]}px) scale(${px})` }}>
          {agent && (
            <g className="remote-sparkles" fill={color}>
              <path className="remote-sparkle" d="M-10 -6 l1.6 3.4 3.4 1.6 -3.4 1.6 -1.6 3.4 -1.6 -3.4 -3.4 -1.6 3.4 -1.6z" />
              <path className="remote-sparkle" d="M16 -12 l1.2 2.6 2.6 1.2 -2.6 1.2 -1.2 2.6 -1.2 -2.6 -2.6 -1.2 2.6 -1.2z" />
              <path className="remote-sparkle" d="M-4 22 l1 2 2 1 -2 1 -1 2 -1 -2 -2 -1 2 -1z" />
            </g>
          )}
          <path d={CURSOR} fill={color} stroke="#fff" strokeWidth={1.5} strokeLinejoin="round" />
          <g transform="translate(12 18)">
            <rect width={pill} height={20} rx={10} fill={color} className={cn(agent && "remote-pill-agent")} />
            <text x={8} y={14} fill="#fff" fontSize={11} fontWeight={600} fontFamily="Inter, system-ui, sans-serif">
              {label}
            </text>
          </g>
        </g>
      )}
    </g>
  );
}

/** Short notice of who changed what, beside the canvas. */
export function ActivityToast() {
  const change = useStudio((st) => st.remoteChange);
  const open = useStudio((st) => st.setHistoryOpen);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!change || Date.now() - change.at > 5000) return;
    setVisible(true);
    const hide = window.setTimeout(() => setVisible(false), 4500);
    return () => window.clearTimeout(hide);
  }, [change]);
  if (!change || !visible) return null;
  const count = change.ids.length;
  return (
    <button type="button" role="status" className="presence-toast" onClick={() => open(true)} title="Open version history">
      <Avatar actor={change.actor} />
      <span className="min-w-0 truncate">
        <strong>{actorName(change.actor)}</strong> {operationLabel(change.operation).toLowerCase()}
        {count ? ` · ${count} object${count === 1 ? "" : "s"}` : ""}
      </span>
    </button>
  );
}

/** Avatars of who changed the document recently, newest first. */
export function RecentActors() {
  const activity = useStudio((st) => st.activity);
  const open = useStudio((st) => st.setHistoryOpen);
  const actors = useMemo(() => {
    const seen = new Map<string, RemoteChange>();
    for (const a of activity) {
      const key = `${a.actor?.kind}:${a.actor?.name}`;
      if (!seen.has(key)) seen.set(key, a);
    }
    return [...seen.values()].slice(0, 3);
  }, [activity]);
  if (!actors.length) return null;
  return (
    <button type="button" className="presence-stack hidden shrink-0 items-center sm:flex" aria-label="Recent editors; open version history" onClick={() => open(true)}>
      {actors.map((a) => (
        <span key={a.seq} title={`${actorName(a.actor)} · ${operationLabel(a.operation)} · ${ago(a.at)}`}>
          <Avatar actor={a.actor} size={24} />
        </span>
      ))}
    </button>
  );
}

export function HistoryButton() {
  const shared = useStudio((st) => st.shared);
  const open = useStudio((st) => st.historyOpen);
  const setOpen = useStudio((st) => st.setHistoryOpen);
  if (!shared) return null;
  return (
    <Button variant={open ? "outline" : "ghost"} size="icon" className="size-9" aria-label="Version history" aria-pressed={open} title="Version history" onClick={() => setOpen(!open)}>
      <History size={15} />
    </Button>
  );
}

type Listing = { cursor: number; entries: HistoryEntry[] };

/** Drawer listing the shared document's versions; click one to preview it. */
export function HistoryPanel() {
  const open = useStudio((st) => st.historyOpen);
  const setOpen = useStudio((st) => st.setHistoryOpen);
  const revision = useStudio((st) => st.revision);
  const preview = useStudio((st) => st.versionPreview);
  const previewVersion = useStudio((st) => st.previewVersion);
  const [listing, setListing] = useState<Listing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [, tick] = useState(0);

  useEffect(() => {
    if (!open) return;
    let live = true;
    api
      .history()
      .then((value) => live && (setListing(value), setError(null)))
      .catch((e: unknown) => live && setError(e instanceof Error ? e.message : "Could not read the history."));
    const timer = window.setInterval(() => tick((n) => n + 1), 30000);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [open, revision]);

  if (!open) return null;
  const rows = listing ? listing.entries.map((entry, i) => ({ entry, i })).reverse() : [];
  const first = listing?.entries[0];
  return (
    <aside className="presence-history" aria-label="Version history">
      <header className="flex items-center gap-2 border-b px-3 py-2">
        <History size={15} aria-hidden="true" />
        <strong className="flex-1 text-sm">Version history</strong>
        <Button variant="ghost" size="icon" className="size-8" aria-label="Close version history" onClick={() => setOpen(false)}>
          <X size={15} />
        </Button>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {error && <p className="p-2 text-xs text-destructive">{error}</p>}
        {listing && !rows.length && <p className="p-2 text-xs text-muted-foreground">No saved changes yet. Edits from Studio, the CLI and agents appear here.</p>}
        <ol className="grid gap-1">
          {rows.map(({ entry, i }) => {
            const current = i === listing!.cursor - 1;
            const undone = i >= listing!.cursor;
            const active = preview?.hash === entry.after;
            return (
              <li key={`${i}-${entry.after}`}>
                <button
                  type="button"
                  className={cn("presence-version", active && "presence-version-active", undone && "opacity-55")}
                  onClick={() => void previewVersion(entry, entry.after)}
                >
                  <Avatar actor={entry.actor} />
                  <span className="grid min-w-0 flex-1 text-left">
                    <span className="truncate text-xs font-medium">
                      {operationLabel(entry.operation)}
                      {current && <span className="presence-current">Current</span>}
                      {undone && <span className="presence-undone">Undone</span>}
                    </span>
                    <span className="truncate text-[11px] text-muted-foreground">
                      {actorName(entry.actor)}
                      {entry.actor?.directed_by ? ` for ${entry.actor.directed_by}` : ""} · {ago(entry.timestamp_ms)}
                    </span>
                  </span>
                </button>
              </li>
            );
          })}
          {first && (
            <li>
              <button
                type="button"
                className={cn("presence-version", preview?.hash === first.before && "presence-version-active")}
                onClick={() => void previewVersion({ ...first, operation: "original", actor: undefined }, first.before)}
              >
                <span className="presence-avatar inline-grid size-[22px] shrink-0 place-items-center rounded-full bg-muted text-[10px]" aria-hidden="true">
                  ●
                </span>
                <span className="grid min-w-0 flex-1 text-left">
                  <span className="truncate text-xs font-medium">Original</span>
                  <span className="truncate text-[11px] text-muted-foreground">Before the first recorded change</span>
                </span>
              </button>
            </li>
          )}
        </ol>
      </div>
    </aside>
  );
}

/** Bar shown while a stored version is on the canvas. */
export function VersionPreviewBar() {
  const preview = useStudio((st) => st.versionPreview);
  const end = useStudio((st) => st.endVersionPreview);
  const restore = useStudio((st) => st.restoreVersion);
  useEffect(() => {
    if (!preview) return;
    const onKey = (event: KeyboardEvent) => event.key === "Escape" && end();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [preview, end]);
  if (!preview) return null;
  const { entry } = preview;
  const what = entry.operation === "original" ? "the original version" : `the version after “${operationLabel(entry.operation)}”`;
  return (
    <div className="presence-preview-bar" role="region" aria-label="Version preview">
      <span className="min-w-0 truncate text-xs">
        Viewing {what}
        {entry.actor ? ` by ${actorName(entry.actor)}` : ""} · {ago(entry.timestamp_ms)}
      </span>
      <Button size="sm" className="h-8 gap-1.5" onClick={() => void restore(preview.hash)}>
        <RotateCcw size={14} aria-hidden="true" />
        Restore this version
      </Button>
      <Button variant="outline" size="sm" className="h-8" onClick={end}>
        Back to current
      </Button>
    </div>
  );
}
