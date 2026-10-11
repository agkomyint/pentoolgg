import { create } from "zustand";
import { api, type Doc } from "@/lib/api";
import { clone, collectIds, findNode, pageOf, uid, type Appearance, type Tool } from "@/lib/model";

export type CanvasView = { kind: "document" } | { kind: "image"; url: string; label: string };

type Mutator = (doc: Doc, ctx: { pageId: string | undefined }) => void;

type State = {
  doc: Doc | null;
  /** Throw-away document rendered instead of `doc` while a control is being dragged. */
  preview: Doc | null;
  setPreview: (doc: Doc | null) => void;
  /** Last document known to be on disk (shared) or opened (standalone). */
  base: Doc | null;
  revision: string | null;
  shared: boolean;
  dirty: boolean;
  version: string | null;
  pageId: string | undefined;
  layerId: string | undefined;
  selectedId: string | null;
  tool: Tool;
  look: Appearance;
  workspaceId: string;
  view: CanvasView;
  zoom: number;
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
  newDoc: () => void;
  openDoc: (doc: Doc) => void;
  save: () => Promise<void>;
  undo: () => void;
  redo: () => void;
  setPage: (id: string) => void;
  setLayer: (id: string) => void;
  select: (id: string | null) => void;
  setTool: (tool: Tool) => void;
  setLook: (look: Partial<Appearance>) => void;
  setWorkspace: (id: string) => void;
  setView: (view: CanvasView) => void;
  setZoom: (zoom: number) => void;
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
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

const HISTORY_LIMIT = 100;

function blankDoc(): Doc {
  return {
    format: "pentool",
    version: 4,
    name: "Untitled",
    pages: [
      {
        id: "page-1",
        name: "Page 1",
        canvas: { width: 1200, height: 800, background: "#ffffff" },
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
  const adopt = (doc: Doc, extra: Partial<State> = {}) => {
    const page = pageOf(doc, get().pageId);
    const layer = page?.layers?.find((l) => l.id === get().layerId)?.id ?? page?.layers?.[0]?.id;
    set({ doc, preview: null, pageId: page?.id, layerId: layer, ...extra });
  };
  return {
    doc: null,
    preview: null,
    setPreview: (preview) => set({ preview }),
    base: null,
    revision: null,
    shared: false,
    dirty: false,
    version: null,
    pageId: undefined,
    layerId: undefined,
    selectedId: null,
    tool: "select",
    look: { fill: "#6d28d9", stroke: "#111827", strokeWidth: 3 },
    workspaceId: "design",
    view: { kind: "document" },
    zoom: 1,
    dark: initialDark(),
    busy: false,
    message: null,
    past: [],
    future: [],
    async load() {
      set({ busy: true });
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
        set({ past: [], future: [], dirty: false, selectedId: null });
        adopt(document, { base: clone(document), revision, shared, version: health.version, message: null, view: { kind: "document" } });
      } catch (error) {
        get().notify(error instanceof Error ? error.message : "Could not load the document.", true);
      } finally {
        set({ busy: false });
      }
    },
    edit(label, mutate) {
      const { doc, pageId, past } = get();
      if (!doc) return;
      const next = clone(doc);
      mutate(next, { pageId });
      const nextPast = [...past, JSON.stringify(doc)].slice(-HISTORY_LIMIT);
      adopt(next, { past: nextPast, future: [], dirty: true });
      void label;
    },
    replaceDoc(next) {
      const { doc, past } = get();
      if (!doc) return;
      adopt(next, { past: [...past, JSON.stringify(doc)].slice(-HISTORY_LIMIT), future: [], dirty: true });
    },
    newDoc() {
      const doc = blankDoc();
      set({ past: [], future: [], dirty: false, shared: false, base: null, revision: null, selectedId: null, pageId: undefined, layerId: undefined });
      adopt(doc, { base: clone(doc) });
      get().notify("New document. Use Save to download it as .pen.");
    },
    openDoc(doc) {
      set({ past: [], future: [], dirty: false, shared: false, base: null, revision: null, selectedId: null, pageId: undefined, layerId: undefined });
      adopt(doc, { base: clone(doc) });
      get().notify("Opened as a standalone document. Use Save to download it.");
    },
    async save() {
      const { doc, shared, revision, base } = get();
      if (!doc) return;
      try {
        if (shared) {
          const r = await fetch("/api/document", {
            method: "PUT",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ revision, base, document: doc }),
          });
          const body = (await r.json()) as { revision?: string; error?: string };
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
      set({ pageId, layerId: doc ? firstLayer(doc, pageId) : undefined, selectedId: null, view: { kind: "document" } });
    },
    setLayer: (layerId) => set({ layerId }),
    select: (selectedId) => {
      const { doc, pageId } = get();
      const found = selectedId && doc ? findNode(pageOf(doc, pageId), selectedId) : null;
      set({ selectedId, ...(found ? { layerId: found.layer.id } : {}) });
    },
    setTool: (tool) => set({ tool }),
    setLook: (look) => set({ look: { ...get().look, ...look } }),
    setWorkspace: (workspaceId) => set({ workspaceId, view: { kind: "document" } }),
    setView: (view) => set({ view }),
    setZoom: (zoom) => set({ zoom: Math.min(8, Math.max(0.1, zoom)) }),
    toggleDark() {
      const dark = !get().dark;
      try {
        localStorage.setItem("pentool-theme", dark ? "dark" : "light");
      } catch {
        // ignore
      }
      set({ dark });
    },
    notify: (text, error = false) => set({ message: { text, error } }),
  };
});

export { collectIds, uid };
