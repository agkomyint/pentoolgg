import { useEffect, useRef, useState } from "react";
import { ChevronsDown, ChevronsUp, Plus } from "lucide-react";
import { api, type Doc } from "@/lib/api";
import { ARTWORK_SOURCE_DRAG_TYPE } from "@/lib/artworkDrag";
import { addPage, duplicatePage, movePage, removePage, renamePage } from "@/lib/actions";
import { isScene, pagesOf } from "@/lib/model";
import { cn } from "@/lib/utils";
import { collectIds, useStudio } from "@/store";
import { useLayout } from "@/workspaces/layout/layoutStore";
import { PHOTO_DRAG_TYPE, usePhoto } from "@/workspaces/photoStore";
import { nextId, pageParent } from "@/workspaces/layout/layoutModel";

const PAGE_DRAG_TYPE = "application/x-pentool-page";
const THUMB = 160;

/** Page edits go through the layout engine from format v9 so stories and spreads stay consistent. */
const serverPages = (doc: Doc | null) => !!doc && doc.version >= 9;

/** A page without its own name (the layout engine names new pages by id) reads as "Page N". */
const named = (page: { id: string; name?: string } | undefined) => !!page?.name && page.name !== page.id;
const pageLabel = (page: { id: string; name?: string }, index: number) => (named(page) ? page.name! : `Page ${index + 1}`);

async function addAfter(current: string | undefined): Promise<void> {
  const { doc, setPage } = useStudio.getState();
  if (!doc) return;
  if (!serverPages(doc)) return addPage();
  const page = doc.pages.find((p) => p.id === current);
  const id = nextId("page", collectIds(doc));
  if (await useLayout.getState().run({ type: "page-add", id, after: current, parent: pageParent(page) }, "Add page")) setPage(id);
}

async function duplicate(id: string): Promise<void> {
  const { doc, setPage } = useStudio.getState();
  if (!doc) return;
  setPage(id);
  if (!serverPages(doc)) return duplicatePage();
  const copy = nextId("page", collectIds(doc));
  if (await useLayout.getState().run({ type: "page-duplicate", id, new_id: copy }, "Duplicate page")) setPage(copy);
}

async function remove(id: string): Promise<void> {
  const { doc, setPage } = useStudio.getState();
  if (!doc || doc.pages.length < 2) return;
  const page = doc.pages.find((p) => p.id === id);
  const index = doc.pages.findIndex((p) => p.id === id);
  if (!window.confirm(`Delete ${page ? pageLabel(page, index) : id}? Undo brings it back.`)) return;
  if (!serverPages(doc)) return removePage(id);
  const neighbour = doc.pages[index + 1] ?? doc.pages[index - 1];
  if ((await useLayout.getState().run({ type: "page-remove", id }, "Delete page")) && neighbour) setPage(neighbour.id);
}

async function move(id: string, index: number): Promise<void> {
  const { doc } = useStudio.getState();
  if (!doc) return;
  if (!serverPages(doc)) return movePage(id, index);
  await useLayout.getState().run({ type: "page-move", id, index }, "Move page");
}

/**
 * Page thumbnails rendered by the server, keyed by the page's JSON so an edit
 * re-renders only the page it touched. The current page renders first.
 */
function useThumbnails(doc: Doc | null, current: string | undefined): Record<string, string> {
  const [urls, setUrls] = useState<Record<string, string>>({});
  const keys = useRef(new Map<string, string>());
  // Bumped to retry pages whose render failed, e.g. while preview workers were busy.
  const [retry, setRetry] = useState(0);
  useEffect(() => {
    if (!doc || !isScene(doc)) return;
    let cancelled = false;
    let failed = false;
    let again: number | undefined;
    const timer = window.setTimeout(async () => {
      const pages = pagesOf(doc);
      const order = [...pages].sort((a, b) => Number(b.id === current) - Number(a.id === current));
      for (const page of order) {
        if (cancelled) return;
        const key = `${doc.version}:${JSON.stringify(page)}`;
        if (keys.current.get(page.id) === key) continue;
        try {
          const blob = await api.renderPng(doc, page.id, THUMB);
          if (cancelled) return;
          keys.current.set(page.id, key);
          const url = URL.createObjectURL(blob);
          setUrls((old) => {
            if (old[page.id]) URL.revokeObjectURL(old[page.id]!);
            return { ...old, [page.id]: url };
          });
        } catch {
          failed = true;
        }
      }
      if (failed && !cancelled && retry < 5) again = window.setTimeout(() => setRetry((n) => n + 1), 2000);
      else if (!failed && retry) setRetry(0);
      const live = new Set(pages.map((p) => p.id));
      setUrls((old) => {
        const gone = Object.keys(old).filter((id) => !live.has(id));
        if (!gone.length) return old;
        const next = { ...old };
        for (const id of gone) {
          URL.revokeObjectURL(next[id]!);
          delete next[id];
          keys.current.delete(id);
        }
        return next;
      });
    }, 400);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      window.clearTimeout(again);
    };
  }, [doc, current, retry]);
  return urls;
}

/**
 * The one place to see, add, reorder and manage pages, below the canvas as in
 * Canva and Keynote. Thumbnails can also be dragged onto a mockup artwork slot.
 */
const COMPACT_KEY = "pentool.pagestrip-compact";

function storedCompact(): boolean {
  try {
    return localStorage.getItem(COMPACT_KEY) === "1";
  } catch {
    return false;
  }
}

export function PageStrip() {
  const { doc, pageId, setPage } = useStudio();
  const pages = pagesOf(doc);
  const legacy = !!doc && !Array.isArray(doc.pages);
  const urls = useThumbnails(doc, pageId);
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const [dropAt, setDropAt] = useState<number | null>(null);
  const [photoTarget, setPhotoTarget] = useState<string | null>(null);
  // Compact shows one row of page name chips instead of thumbnails, leaving more room for the canvas.
  const [compact, setCompact] = useState(storedCompact);
  const toggleCompact = () => {
    const next = !compact;
    setCompact(next);
    try {
      localStorage.setItem(COMPACT_KEY, next ? "1" : "0");
    } catch {
      // storage can be unavailable
    }
  };
  const strip = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!menu) return;
    const close = () => setMenu(null);
    window.addEventListener("pointerdown", close);
    window.addEventListener("blur", close);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("blur", close);
    };
  }, [menu]);
  // Keep the current page in view.
  useEffect(() => {
    strip.current?.querySelector<HTMLElement>('[aria-current="page"]')?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [pageId]);

  if (!doc) return null;
  const rename = (id: string) => {
    const page = pages.find((p) => p.id === id);
    const next = window.prompt("Page name", named(page) ? page!.name : "");
    if (next?.trim()) renamePage(id, next.trim());
  };
  const menuPage = menu && pages.find((p) => p.id === menu.id);
  const menuIndex = menu ? pages.findIndex((p) => p.id === menu.id) : -1;

  return (
    <nav aria-label="Pages" className={cn("studio-pagestrip", compact && "studio-pagestrip-compact")}>
      <div ref={strip} className={cn("flex min-w-0 flex-1 gap-2 overflow-x-auto px-3", compact ? "items-center py-1" : "items-end py-2")} role="list"
        onDragOver={(event) => { if (event.dataTransfer.types.includes(PAGE_DRAG_TYPE)) event.preventDefault(); }}
        onDragLeave={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropAt(null); }}
        onDrop={(event) => {
          const id = event.dataTransfer.getData(PAGE_DRAG_TYPE);
          const at = dropAt;
          setDropAt(null);
          if (!id || at === null) return;
          event.preventDefault();
          const from = pages.findIndex((p) => p.id === id);
          void move(id, at > from ? at - 1 : at);
        }}>
        {pages.map((page, index) => {
          const canvas: { width: number; height: number; background?: string } = page.canvas ?? { width: 1200, height: 800 };
          const ratio = canvas.width / Math.max(1, canvas.height);
          const h = compact ? 24 : 56;
          const w = Math.max(28, Math.min(120, h * ratio));
          const label = named(page) ? `${index + 1}. ${page.name}` : pageLabel(page, index);
          const active = page.id === pageId;
          return (
            <div key={page.id} role="listitem" className="relative flex shrink-0 flex-col items-center gap-1"
              onDragOver={(event) => {
                if (!event.dataTransfer.types.includes(PAGE_DRAG_TYPE)) return;
                event.preventDefault();
                const rect = event.currentTarget.getBoundingClientRect();
                setDropAt(event.clientX < rect.left + rect.width / 2 ? index : index + 1);
              }}>
              {dropAt === index && <span aria-hidden="true" className="absolute -left-1.5 top-0 w-0.5 rounded bg-primary" style={{ height: h }} />}
              {dropAt === index + 1 && index === pages.length - 1 && <span aria-hidden="true" className="absolute -right-1.5 top-0 w-0.5 rounded bg-primary" style={{ height: h }} />}
              <button
                type="button"
                draggable={!legacy}
                aria-current={active ? "page" : undefined}
                aria-label={`${pageLabel(page, index)}, page ${index + 1} of ${pages.length}`}
                title={`${pageLabel(page, index)}. Drag to reorder or onto a mockup. Right-click for more.`}
                className={cn("block overflow-hidden rounded border shadow-sm outline-offset-2 transition-shadow hover:shadow-md focus-visible:outline-2 focus-visible:outline-primary", active ? "border-primary ring-2 ring-primary" : "border-border", photoTarget === page.id && "scale-105 ring-2 ring-primary ring-offset-2", compact ? "max-w-[140px] truncate bg-background px-2 text-[11px] leading-none" : "bg-white", compact && active && "font-medium")}
                style={compact ? { height: h } : { width: w, height: h, background: canvas.background ?? "#fff" }}
                onClick={() => setPage(page.id)}
                onDoubleClick={() => !legacy && rename(page.id)}
                onContextMenu={(event) => {
                  if (legacy) return;
                  event.preventDefault();
                  setMenu({ id: page.id, x: event.clientX, y: event.clientY });
                }}
                onKeyDown={(event) => {
                  if (legacy) return;
                  if ((event.key === "Delete" || event.key === "Backspace") && pages.length > 1) {
                    event.preventDefault();
                    event.stopPropagation();
                    void remove(page.id);
                  } else if (event.altKey && (event.key === "ArrowLeft" || event.key === "ArrowRight")) {
                    event.preventDefault();
                    event.stopPropagation();
                    void move(page.id, index + (event.key === "ArrowLeft" ? -1 : 1));
                  } else if (event.key === "F2") {
                    event.preventDefault();
                    rename(page.id);
                  }
                }}
                onDragStart={(event) => {
                  event.dataTransfer.effectAllowed = "copyMove";
                  event.dataTransfer.setData(PAGE_DRAG_TYPE, page.id);
                  event.dataTransfer.setData(ARTWORK_SOURCE_DRAG_TYPE, JSON.stringify({ kind: "page", page: page.id }));
                  event.dataTransfer.setData("text/plain", pageLabel(page, index));
                }}
                onDragEnd={() => setDropAt(null)}
                onDragEnter={(event) => { if (event.dataTransfer.types.includes(PHOTO_DRAG_TYPE)) setPhotoTarget(page.id); }}
                onDragOver={(event) => {
                  if (!event.dataTransfer.types.includes(PHOTO_DRAG_TYPE)) return;
                  event.preventDefault();
                  event.dataTransfer.dropEffect = "copy";
                }}
                onDragLeave={() => setPhotoTarget((current) => (current === page.id ? null : current))}
                onDrop={(event) => {
                  const photos = event.dataTransfer.getData(PHOTO_DRAG_TYPE);
                  setPhotoTarget(null);
                  if (!photos) return;
                  // A photo dropped on a page thumbnail goes in the middle of that page.
                  event.preventDefault();
                  event.stopPropagation();
                  void usePhoto.getState().place(JSON.parse(photos) as string[], { pageId: page.id });
                }}
              >
                {compact ? label : urls[page.id] ? <img src={urls[page.id]} alt="" className="pointer-events-none h-full w-full object-contain" draggable={false} /> : null}
              </button>
              {!compact && <span className={cn("max-w-[120px] truncate text-[10px] leading-none", active ? "font-medium text-foreground" : "text-muted-foreground")}>{label}</span>}
            </div>
          );
        })}
        {!legacy && (
          <div role="listitem" className="flex shrink-0 flex-col items-center gap-1">
            <button type="button" aria-label="Add page" title="Add a page after the current one"
              className={cn("grid place-items-center rounded border", compact ? "size-6" : "size-14", " border-dashed text-muted-foreground hover:border-primary hover:text-primary focus-visible:outline-2 focus-visible:outline-primary")}
              onClick={() => void addAfter(pageId)}>
              <Plus size={compact ? 14 : 18} aria-hidden="true" />
            </button>
            {!compact && <span className="text-[10px] leading-none text-muted-foreground">Add page</span>}
          </div>
        )}
        {legacy && <p className="self-center text-[11px] text-muted-foreground">Single-page legacy document. Upgrade the format (status bar) to add pages.</p>}
      </div>
      <button type="button" aria-pressed={compact} aria-label={compact ? "Show page thumbnails" : "Make the page strip smaller"}
        title={compact ? "Show page thumbnails" : "Make the page strip smaller"}
        className="m-1 grid size-7 shrink-0 place-items-center self-start rounded text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-2 focus-visible:outline-primary"
        onClick={toggleCompact}>
        {compact ? <ChevronsUp size={16} aria-hidden="true" /> : <ChevronsDown size={16} aria-hidden="true" />}
      </button>
      {menu && menuPage && (
        <div role="menu" aria-label={`${pageLabel(menuPage, menuIndex)} options`} className="fixed z-50 grid min-w-44 gap-0.5 rounded-md border bg-card p-1 text-xs shadow-lg"
          style={{ left: Math.min(menu.x, window.innerWidth - 190), top: Math.max(8, menu.y - 200) }}
          onPointerDown={(event) => event.stopPropagation()}>
          {[
            { label: "Add page after", run: () => addAfter(menu.id) },
            { label: "Duplicate", run: () => duplicate(menu.id) },
            { label: "Rename…", run: () => rename(menu.id) },
            { label: "Move left", run: () => move(menu.id, menuIndex - 1), disabled: menuIndex < 1 },
            { label: "Move right", run: () => move(menu.id, menuIndex + 1), disabled: menuIndex >= pages.length - 1 },
            { label: "Delete", run: () => remove(menu.id), disabled: pages.length < 2, danger: true },
          ].map((item) => (
            <button key={item.label} type="button" role="menuitem" disabled={item.disabled}
              className={cn("rounded px-2 py-1.5 text-left hover:bg-accent disabled:opacity-40", item.danger && "text-destructive")}
              onClick={() => {
                setMenu(null);
                void item.run();
              }}>
              {item.label}
            </button>
          ))}
        </div>
      )}
    </nav>
  );
}
