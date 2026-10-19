import { create } from "zustand";
import { api, type Doc, type HistoryActor, type HistoryEntry } from "@/lib/api";
import { streamEvents } from "@/lib/collab";
import { changedNodeIds, mergeDocs } from "@/lib/merge";
import { clone, collectIds, findNode, pageOf, pagesOf, uid, type Appearance, type Tool } from "@/lib/model";

export type CanvasView = { kind: "document" } | { kind: "image"; url: string; label: string };

/** A change another writer (an agent, the CLI, a teammate) saved to the shared document. */
export type RemoteChange = {
  seq: number;
  /** Changed node IDs on the page this tab shows. */
  ids: string[];
  pageId: string | undefined;
  actor: HistoryActor | null;
  operation: string | null;
  at: number;
};

/** A stored version shown on the canvas instead of the document. */
export type VersionPreview = { hash: string; entry: HistoryEntry; doc: Doc };

type Mutator = (doc: Doc, ctx: { pageId: string | undefined }) => void;

type State = {
  doc: Doc | null;
  /** Throw-away document rendered instead of `doc` while a control is being dragged. */
  preview: Doc | null;
  setPreview: (doc: Doc | null) => void;
  /** Last document known to be on disk (shared) or opened (standalone). */
  base: Doc | null;
  revision: string | null;
  /** Newest change another writer made; drives the canvas highlight and agent cursor. */
  remoteChange: RemoteChange | null;
  /** Recent remote changes, newest first. */
  activity: RemoteChange[];
  historyOpen: boolean;
  setHistoryOpen: (open: boolean) => void;
  versionPreview: VersionPreview | null;
  /** Show a stored version on the canvas (read-only) until `endVersionPreview` or an edit. */
  previewVersion: (entry: HistoryEntry, hash: string) => Promise<void>;
  endVersionPreview: () => void;
  /** Make a stored version current, as a new undoable step in the shared history. */
  restoreVersion: (hash: string) => Promise<void>;
  /** Mark the next server-side changes as this tab's own, so they are not shown as remote activity. */
  expectOwnChange: () => void;
  /** Adopt the served document now, merging unsaved edits (as a live update does). */
  pull: () => Promise<void>;
  shared: boolean;
  dirty: boolean;
  version: string | null;
  pageId: string | undefined;
  layerId: string | undefined;
  selectedId: string | null;
  /** Every selected object; `selectedId` is the primary (last picked) one. */
  selectedIds: string[];
  /** Object under the pointer on the canvas or in the layers list. */
  hoverId: string | null;
  setHover: (id: string | null) => void;
  /** Stable panel ID shared by the packaging stage and its inspector. */
  selectedPackagePanel: string | null;
  setSelectedPackagePanel: (id: string | null) => void;
  tool: Tool;
  look: Appearance;
  /** Right-hand panel; "3d" and "photo" also open their focus stage. */
  workspaceId: string;
  /** Paint tools act on the main canvas instead of the vector tools. */
  paintMode: boolean;
  view: CanvasView;
  /** Zoom as a multiple of "fit page"; `scale` is the resulting display scale (1 = 100%). */
  zoom: number;
  scale: number;
  dark: boolean;
  busy: boolean;
  message: { text: string; error: boolean } | null;
  past: string[];
  future: string[];
  load: () => Promise<void>;
  /** Apply an edit to a copy of the document and record one undo step. */
  edit: (label: string, mutate: Mutator) => void;
  /** Replace the whole document with a server-produced one (one undo step). */
  replaceDoc: (doc: Doc, label?: string) => void;
  newDoc: (width?: number, height?: number) => Promise<void>;
  /** Upgrade the open document to a newer format version (undoable, not saved until Save). */
  upgradeDoc: (target?: number) => Promise<void>;
  openDoc: (doc: Doc) => void;
  save: () => Promise<void>;
  undo: () => void;
  redo: () => void;
  setPage: (id: string) => void;
  setLayer: (id: string) => void;
  /** Replace the selection, or with `toggle` add/remove one object (Shift-click). */
  select: (id: string | null, mode?: "replace" | "toggle") => void;
  selectMany: (ids: string[]) => void;
  setTool: (tool: Tool) => void;
  setLook: (look: Partial<Appearance>) => void;
  setWorkspace: (id: string) => void;
  setView: (view: CanvasView) => void;
  setZoom: (zoom: number) => void;
  setScale: (scale: number) => void;
  setPaintMode: (on: boolean) => void;
  toggleDark: () => void;
  notify: (text: string, error?: boolean) => void;
};

function initialDark(): boolean {
  try {
    const stored = localStorage.getItem("pentool-theme");
    if (stored) return stored === "dark";
  } catch {
    // storage can be unavailable
  }
  return !window.matchMedia("(prefers-color-scheme: light)").matches;
}

const HISTORY_LIMIT = 100;
/** Newest `.pen` format the server can migrate to (`pentool migrate --target 11`). */
export const LATEST_FORMAT = 11;

function blankDoc(width = 1200, height = 800): Doc {
  return {
    format: "pentool",
    version: 4,
    name: "Untitled",
    pages: [
      {
        id: "page-1",
        name: "Page 1",
        canvas: { width, height, background: "#ffffff" },
        layers: [{ id: "layer-1", name: "Layer 1", visible: true, locked: false, nodes: [] }],
      },
    ],
    styles: {},
    components: [],
    fonts: [],
  } as unknown as Doc;
}

export const useStudio = create<State>((set, get) => {
  const firstLayer = (doc: Doc, pageId: string | undefined) => pageOf(doc, pageId)?.layers?.[0]?.id;
  let pendingLoad: Promise<void> | null = null;
  let saving = false;
  let following = false;
  let changeSeq = 0;
  // Until this time, changes on disk come from this tab's own server-side edits.
  let ownUntil = 0;
  // A revision event that arrived mid-save or mid-load, replayed afterwards.
  let deferred: { latest: string; change: HistoryEntry | null } | null = null;
  // Adopt the document on disk when another writer (the CLI, an agent, a peer
  // or another tab) changed it. Unsaved edits are merged, not discarded: the
  // remote document becomes the new base and this tab's edits stay on top.
  const followDisk = async (latest: string, change: HistoryEntry | null) => {
    const state = get();
    if (!state.shared || latest === state.revision) return;
    if (saving || pendingLoad) {
      deferred = { latest, change };
      return;
    }
    let loaded: { revision: string; document: Doc };
    try {
      loaded = await api.document();
    } catch {
      return;
    }
    const now = get();
    if (!now.shared || !now.doc || loaded.revision === now.revision) return;
    if (saving || pendingLoad) {
      deferred = { latest: loaded.revision, change };
      return;
    }
    const remote = loaded.document;
    const doc = now.dirty && now.base ? mergeDocs(now.base, now.doc, remote) : remote;
    const dirty = now.dirty && JSON.stringify(doc) !== JSON.stringify(remote);
    const page = pageOf(doc, now.pageId);
    const ids = page ? collectIds({ ...doc, pages: [page] } as Doc) : new Set<string>();
    const selectedIds = now.selectedIds.filter((id) => ids.has(id));
    const remoteChange: RemoteChange = {
      seq: ++changeSeq,
      ids: changedNodeIds(pageOf(now.doc, now.pageId), page),
      pageId: page?.id,
      actor: change?.actor ?? null,
      operation: change?.operation ?? null,
      at: Date.now(),
    };
    const activity = Date.now() < ownUntil ? {} : { remoteChange, activity: [remoteChange, ...now.activity].slice(0, 20) };
    // Local undo steps describe the document before the merge; replaying them would undo the remote change.
    set({ past: [], future: [], selectedIds, selectedId: selectedIds.at(-1) ?? null, ...activity });
    adopt(doc, { base: clone(remote), revision: loaded.revision, dirty });
  };
  const replayDeferred = () => {
    const next = deferred;
    deferred = null;
    if (next) void followDisk(next.latest, next.change);
  };
  const follow = () => {
    if (following) return;
    following = true;
    const connect = () => {
      streamEvents(
        "/api/document/events",
        (name, data) => {
          if (name !== "revision") return;
          try {
            const event = JSON.parse(data) as { revision: string; change?: HistoryEntry };
            void followDisk(event.revision, event.change ?? null);
          } catch {
            // ignore a malformed frame
          }
        },
        // The server ends streams periodically; reconnecting reports the current revision.
        () => window.setTimeout(connect, 2000),
      );
    };
    connect();
  };
  const adopt = (doc: Doc, extra: Partial<State> = {}) => {
    const page = pageOf(doc, get().pageId);
    const layer = page?.layers?.find((l) => l.id === get().layerId)?.id ?? page?.layers?.[0]?.id;
    // A version being previewed stays on the canvas until it is closed.
    const version = "versionPreview" in extra ? extra.versionPreview : get().versionPreview;
    set({ doc, preview: version?.doc ?? null, pageId: page?.id, layerId: layer, ...extra });
  };
  return {
    doc: null,
    preview: null,
    setPreview: (preview) => set({ preview }),
    base: null,
    revision: null,
    remoteChange: null,
    activity: [],
    historyOpen: false,
    setHistoryOpen: (historyOpen) => set({ historyOpen }),
    versionPreview: null,
    async previewVersion(entry, hash) {
      try {
        const version = await api.historyVersion(hash);
        const versionPreview = { hash, entry, doc: version.document };
        const page = pageOf(version.document, get().pageId);
        set({ versionPreview, preview: version.document, ...(page ? {} : { pageId: pagesOf(version.document)[0]?.id }) });
      } catch (error) {
        get().notify(error instanceof Error ? error.message : "Could not load that version.", true);
      }
    },
    endVersionPreview: () => set({ versionPreview: null, preview: null }),
    expectOwnChange: () => {
      ownUntil = Date.now() + 4000;
    },
    pull: () => followDisk("", null),
    async restoreVersion(hash) {
      if (get().dirty && !window.confirm("Restoring replaces your unsaved changes. Continue?")) return;
      try {
        await api.restoreVersion(hash);
        // Adopt the restored file now rather than waiting for the live stream.
        const loaded = await api.document();
        set({ past: [], future: [], dirty: false, versionPreview: null, selectedId: null, selectedIds: [] });
        adopt(loaded.document, { base: clone(loaded.document), revision: loaded.revision, versionPreview: null });
        get().notify("Version restored. It is a new step in the history, so you can undo it there.");
      } catch (error) {
        get().notify(error instanceof Error ? error.message : "Could not restore that version.", true);
      }
    },
    shared: false,
    dirty: false,
    version: null,
    pageId: undefined,
    layerId: undefined,
    selectedId: null,
    selectedIds: [],
    selectedPackagePanel: null,
    setSelectedPackagePanel: (selectedPackagePanel) => set({ selectedPackagePanel }),
    hoverId: null,
    setHover: (hoverId) => { if (get().hoverId !== hoverId) set({ hoverId }); },
    tool: "select",
    look: { fill: "#6d28d9", stroke: "#111827", strokeWidth: 3 },
    workspaceId: "design",
    paintMode: false,
    view: { kind: "document" },
    zoom: 1,
    scale: 1,
    dark: initialDark(),
    busy: false,
    message: null,
    past: [],
    future: [],
    load() {
      // React Strict Mode intentionally replays mount effects in development.
      // Coalesce overlapping requests so a slower duplicate cannot adopt the
      // document again after the user has already started interacting.
      if (pendingLoad) return pendingLoad;
      set({ busy: true });
      pendingLoad = (async () => {
        try {
          const health = await api.health();
          let shared = true;
          let document: Doc;
          let revision: string | null = null;
          try {
            const loaded = await api.document();
            document = loaded.document;
            revision = loaded.revision;
          } catch {
            shared = false;
            document = get().doc ?? blankDoc();
          }
          set({ past: [], future: [], dirty: false, versionPreview: null, selectedId: null, selectedIds: [] });
          adopt(document, { base: clone(document), revision, shared, version: health.version, message: null, view: { kind: "document" } });
          if (shared) follow();
        } catch (error) {
          get().notify(error instanceof Error ? error.message : "Could not load the document.", true);
        } finally {
          set({ busy: false });
        }
      })().finally(() => {
        pendingLoad = null;
        replayDeferred();
      });
      return pendingLoad;
    },
    edit(label, mutate) {
      const { doc, pageId, past } = get();
      if (!doc) return;
      const next = clone(doc);
      mutate(next, { pageId });
      const nextPast = [...past, JSON.stringify(doc)].slice(-HISTORY_LIMIT);
      adopt(next, { past: nextPast, future: [], dirty: true, versionPreview: null });
      void label;
    },
    replaceDoc(next) {
      const { doc, past } = get();
      if (!doc) return;
      adopt(next, { past: [...past, JSON.stringify(doc)].slice(-HISTORY_LIMIT), future: [], dirty: true });
    },
    async newDoc(width = 1200, height = 800) {
      let doc = blankDoc(width, height);
      let note = "New document. Use Save to download it as .pen.";
      try {
        // Start at the newest format so images, compositing, photo and print work without a manual upgrade.
        const upgraded = await api.migrate(doc, LATEST_FORMAT);
        doc = upgraded.document;
        note = `New document (format v${upgraded.to_version}). Use Save to download it as .pen.`;
      } catch {
        // server unreachable: fall back to the base scene format
      }
      set({ past: [], future: [], dirty: false, shared: false, base: null, revision: null, versionPreview: null, activity: [], remoteChange: null, selectedId: null, selectedIds: [], pageId: undefined, layerId: undefined });
      adopt(doc, { base: clone(doc) });
      get().notify(note);
    },
    async upgradeDoc(target = LATEST_FORMAT) {
      const { doc } = get();
      if (!doc) return;
      try {
        const upgraded = await api.migrate(doc, target);
        get().replaceDoc(upgraded.document);
        get().notify(`Upgraded to format v${upgraded.to_version}. Save to keep it; Undo reverts.`);
      } catch (error) {
        get().notify(error instanceof Error ? error.message : "Could not upgrade the document.", true);
      }
    },
    openDoc(doc) {
      set({ past: [], future: [], dirty: false, shared: false, base: null, revision: null, versionPreview: null, activity: [], remoteChange: null, selectedId: null, selectedIds: [], pageId: undefined, layerId: undefined });
      adopt(doc, { base: clone(doc) });
      get().notify("Opened as a standalone document. Use Save to download it.");
    },
    async save() {
      const { doc, shared, revision, base } = get();
      if (!doc) return;
      try {
        if (shared) {
          saving = true;
          let r: Response;
          let body: { revision?: string; error?: string };
          try {
            r = await fetch("/api/document", {
              method: "PUT",
              headers: { "content-type": "application/json" },
              body: JSON.stringify({ revision, base, document: doc }),
            });
            body = (await r.json()) as { revision?: string; error?: string };
          } finally {
            saving = false;
          }
          // Our own save comes back on the stream with this revision and is skipped.
          if (r.ok && body.revision) set({ revision: body.revision });
          replayDeferred();
          if (!r.ok) throw new Error(body.error ?? "Save failed.");
          set({ revision: body.revision ?? revision, base: clone(doc), dirty: false });
          get().notify("Saved to the shared document.");
        } else {
          const blob = new Blob([JSON.stringify(doc, null, 2)], { type: "application/json" });
          const a = document.createElement("a");
          a.href = URL.createObjectURL(blob);
          a.download = `${String(doc.name ?? "untitled")}.pen`;
          a.click();
          setTimeout(() => URL.revokeObjectURL(a.href), 1000);
          set({ dirty: false });
          get().notify(".pen file downloaded.");
        }
      } catch (error) {
        get().notify(error instanceof Error ? error.message : "Save failed.", true);
      }
    },
    undo() {
      const { doc, past, future } = get();
      const last = past.at(-1);
      if (!doc || !last) return;
      adopt(JSON.parse(last) as Doc, { past: past.slice(0, -1), future: [...future, JSON.stringify(doc)], dirty: true });
    },
    redo() {
      const { doc, past, future } = get();
      const next = future.at(-1);
      if (!doc || !next) return;
      adopt(JSON.parse(next) as Doc, { past: [...past, JSON.stringify(doc)], future: future.slice(0, -1), dirty: true });
    },
    setPage: (pageId) => {
      const doc = get().doc;
      set({ pageId, layerId: doc ? firstLayer(doc, pageId) : undefined, selectedId: null, selectedIds: [], view: { kind: "document" } });
    },
    setLayer: (layerId) => set({ layerId }),
    select: (id, mode = "replace") => {
      const { doc, pageId, selectedIds } = get();
      let ids = id ? [id] : [];
      if (mode === "toggle" && id) ids = selectedIds.includes(id) ? selectedIds.filter((x) => x !== id) : [...selectedIds, id];
      const selectedId = ids.at(-1) ?? null;
      const found = selectedId && doc ? findNode(pageOf(doc, pageId), selectedId) : null;
      set({ selectedId, selectedIds: ids, ...(found ? { layerId: found.layer.id } : {}) });
    },
    selectMany: (ids) => {
      const { doc, pageId } = get();
      const selectedId = ids.at(-1) ?? null;
      const found = selectedId && doc ? findNode(pageOf(doc, pageId), selectedId) : null;
      set({ selectedId, selectedIds: ids, ...(found ? { layerId: found.layer.id } : {}) });
    },
    // A vector tool leaves painting; the paint options give way to Properties.
    setTool: (tool) => set({ tool, paintMode: false, ...(get().workspaceId === "paint" ? { workspaceId: "design" } : {}) }),
    setLook: (look) => set({ look: { ...get().look, ...look } }),
    setWorkspace: (id) => set({ workspaceId: id === "canvas" ? "design" : id, view: { kind: "document" } }),
    setView: (view) => set({ view }),
    setZoom: (zoom) => set({ zoom: Math.min(64, Math.max(0.05, zoom)) }),
    setScale: (scale) => { if (get().scale !== scale) set({ scale }); },
    setPaintMode: (paintMode) => set({ paintMode }),
    toggleDark() {
      const dark = !get().dark;
      try {
        localStorage.setItem("pentool-theme", dark ? "dark" : "light");
      } catch {
        // ignore
      }
      set({ dark });
    },
    notify: (text, error = false) => {
      const message = { text, error };
      set({ message });
      // Info fades like a toast; an error stays until something replaces it.
      if (!error) window.setTimeout(() => get().message === message && set({ message: null }), 6000);
    },
  };
});

export { collectIds, uid };
