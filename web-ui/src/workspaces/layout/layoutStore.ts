// Layout workspace state. The document itself lives in the Studio store; this
// holds the composed overview (computed by Rust), overlay toggles and the
// "thread to next frame" mode. Every layout edit goes through the Rust planners
// via `/api/layout` and lands in the Studio store as one undo step.
import { create } from "zustand";
import { layoutOps, type LayoutOp, type LayoutOverview } from "@/lib/api";
import { useStudio } from "@/store";

export type Overlays = { frames: boolean; threads: boolean; overset: boolean; margins: boolean };

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
  overlays: stored<Overlays>("overlays", { frames: true, threads: true, overset: true, margins: false }),
  threadFrom: null,
  indesignKeys: stored("indesign-keys", false),
  setOverlay(key, on) {
    const overlays = { ...get().overlays, [key]: on };
    remember("overlays", overlays);
    set({ overlays });
  },
  setThreadFrom: (threadFrom) => set({ threadFrom }),
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
