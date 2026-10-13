// Layout workspace state. The document itself lives in the Studio store; this
// holds the composed overview (computed by Rust), overlay toggles and the
// "thread to next frame" mode. Every layout edit goes through the Rust planners
// via `/api/layout` and lands in the Studio store as one undo step.
import { create } from "zustand";
import { layoutOps, type LayoutOp, type LayoutOverview } from "@/lib/api";
import { useStudio } from "@/store";

export type Overlays = { frames: boolean; threads: boolean; overset: boolean; margins: boolean; baseline: boolean; hidden: boolean };

type LayoutState = {
  overview: LayoutOverview | null;
  loading: boolean;
  /** Why the overview could not be composed (validation error from Rust). */
  error: string | null;
  overlays: Overlays;
  /** Frame waiting for a click on its successor. */
  threadFrom: string | null;
  /** InDesign-style single-key shortcuts (V, T, Page Up/Down). */
  indesignKeys: boolean;
  setOverlay: (key: keyof Overlays, on: boolean) => void;
  setThreadFrom: (id: string | null) => void;
  setIndesignKeys: (on: boolean) => void;
  /** Text waiting to be dropped by the next canvas click (loaded cursor). */
  loaded: { name: string; text: string } | null;
  setLoaded: (loaded: { name: string; text: string } | null) => void;
  /** Create a story from the loaded text and a frame at the page point. */
  placeLoaded: (at: { x: number; y: number }) => Promise<string | null>;
  refresh: () => Promise<void>;
  /** Run one planner op on the open document; returns the planner report. */
  run: <T = Record<string, unknown>>(op: LayoutOp, label: string) => Promise<T | null>;
};

function stored<T>(key: string, fallback: T): T {
  try {
    const value = localStorage.getItem(`pentool-layout:${key}`);
    return value === null ? fallback : (JSON.parse(value) as T);
  } catch {
    return fallback;
  }
}
function remember(key: string, value: unknown) {
  try {
    localStorage.setItem(`pentool-layout:${key}`, JSON.stringify(value));
  } catch {
    // storage can be unavailable
  }
}

export const useLayout = create<LayoutState>((set, get) => ({
  overview: null,
  loading: false,
  error: null,
  overlays: { frames: true, threads: true, overset: true, margins: false, baseline: false, hidden: false, ...stored<Partial<Overlays>>("overlays", {}) },
  threadFrom: null,
  indesignKeys: stored("indesign-keys", false),
  setOverlay(key, on) {
    const overlays = { ...get().overlays, [key]: on };
    remember("overlays", overlays);
    set({ overlays });
  },
  setThreadFrom: (threadFrom) => set({ threadFrom }),
  loaded: null,
  setLoaded: (loaded) => set({ loaded }),
  async placeLoaded(at) {
    const { loaded, run } = get();
    const studio = useStudio.getState();
    if (!loaded || !studio.doc) return null;
    const stories = new Set(((studio.doc.layout as { stories?: { id: string }[] } | undefined)?.stories ?? []).map((s) => s.id));
    const base = loaded.name.replace(/\.[^.]*$/, "").replace(/[^A-Za-z0-9_-]+/g, "-") || "story";
    let story = base;
    for (let i = 2; stories.has(story); i += 1) story = `${base}-${i}`;
    set({ loaded: null });
    if (!(await run({ type: "story", action: { type: "add", id: story, text: loaded.text } }, "Place text"))) return null;
    const frame = `${story}-frame`;
    const page = studio.doc.pages.find((p) => p.id === studio.pageId);
    const w = page?.canvas?.width ?? 612;
    const h = page?.canvas?.height ?? 792;
    const width = Math.min(300, Math.max(60, w - at.x));
    const height = Math.min(360, Math.max(60, h - at.y));
    const ok = await run(
      { type: "frame", action: { type: "add", id: frame, story, page: studio.pageId, layer: studio.layerId, x: at.x, y: at.y, width, height } },
      "Add text frame",
    );
    if (ok) useStudio.getState().select(frame);
    return ok ? frame : null;
  },
  setIndesignKeys(on) {
    remember("indesign-keys", on);
    set({ indesignKeys: on });
  },
  async refresh() {
    const { doc } = useStudio.getState();
    if (!doc || doc.version < 9) return set({ overview: null });
    set({ loading: true });
    try {
      set({ overview: await layoutOps.overview(doc), error: null });
    } catch (error) {
      set({ overview: null, error: error instanceof Error ? error.message : "Could not compose the layout." });
    } finally {
      set({ loading: false });
    }
  },
  async run(op, label) {
    const studio = useStudio.getState();
    if (!studio.doc) return null;
    try {
      const response = await layoutOps.run<Record<string, never>>(studio.doc, op);
      if (response.document) studio.replaceDoc(response.document, label);
      void get().refresh();
      return response.result as never;
    } catch (error) {
      studio.notify(error instanceof Error ? error.message : `${label} failed.`, true);
      return null;
    }
  },
}));
