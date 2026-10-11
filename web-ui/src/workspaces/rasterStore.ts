import { create } from "zustand";

export type RasterTool = "brush" | "eraser" | "clone" | "heal" | "source" | "quickmask" | "marquee" | "lasso" | "wand";
export type Guide = { axis: "x" | "y"; at: number };

type State = {
  layerId: string | null;
  tool: RasterTool;
  kind: string;
  size: number;
  hardness: number;
  flow: number;
  stabilizer: number;
  color: string;
  mode: string;
  shape: string;
  tolerance: number;
  zoom: number;
  rotate: number;
  panX: number;
  panY: number;
  rulers: boolean;
  snap: boolean;
  grid: number;
  guides: Guide[];
  busy: boolean;
  status: string;
  set: (patch: Partial<State>) => void;
  resetView: () => void;
};

// View aids (rulers, guides, snapping, zoom) live in the browser only and never
// touch the document; pixels change solely through POST /api/raster.
export const usePaint = create<State>((set) => ({
  layerId: null,
  tool: "brush",
  kind: "hard-round",
  size: 24,
  hardness: 0.8,
  flow: 1,
  stabilizer: 0,
  color: "#000000",
  mode: "replace",
  shape: "rect",
  tolerance: 32,
  zoom: 1,
  rotate: 0,
  panX: 0,
  panY: 0,
  rulers: false,
  snap: false,
  grid: 0,
  guides: [],
  busy: false,
  status: "Pick a tool and paint on the preview.",
  set: (patch) => set(patch),
  resetView: () => set({ zoom: 1, rotate: 0, panX: 0, panY: 0 }),
}));
