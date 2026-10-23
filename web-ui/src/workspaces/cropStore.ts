import { create } from "zustand";
import type { SceneNode } from "@/lib/api";
import type { Matrix, Rect } from "@/lib/model";

/** Crop window as fractions of the source: [x, y, width, height]. */
export type CropWindow = [number, number, number, number];

/** Aspect presets for the canvas crop tool; `null` is free-form. */
export const CROP_RATIOS: { label: string; ratio: number | null }[] = [
  { label: "Free", ratio: null },
  { label: "1:1", ratio: 1 },
  { label: "4:3", ratio: 4 / 3 },
  { label: "3:2", ratio: 3 / 2 },
  { label: "16:9", ratio: 16 / 9 },
  { label: "3:4", ratio: 3 / 4 },
  { label: "2:3", ratio: 2 / 3 },
  { label: "9:16", ratio: 9 / 16 },
];

type CropState = {
  /** Image node being cropped on the canvas, or null when the tool is off. */
  nodeId: string | null;
  /** The crop rectangle in page units while it is being dragged. */
  draft: Rect | null;
  /** Width / height lock, or null for free-form. */
  ratio: number | null;
  start: (nodeId: string, draft: Rect) => void;
  setDraft: (draft: Rect) => void;
  setRatio: (ratio: number | null) => void;
  stop: () => void;
};

export const useCrop = create<CropState>((set) => ({
  nodeId: null,
  draft: null,
  ratio: null,
  start: (nodeId, draft) => set({ nodeId, draft, ratio: null }),
  setDraft: (draft) => set({ draft }),
  setRatio: (ratio) => set({ ratio }),
  stop: () => set({ nodeId: null, draft: null, ratio: null }),
}));

const round = (v: number, places = 100) => Math.round(v * places) / places;

/** The node's translation to page space; null when it is rotated, scaled or skewed. */
export function pureOffset(world: Matrix): { dx: number; dy: number } | null {
  const near = (a: number, b: number) => Math.abs(a - b) < 1e-9;
  return near(world[0], 1) && near(world[1], 0) && near(world[2], 0) && near(world[3], 1)
    ? { dx: world[4], dy: world[5] }
    : null;
}

/** Why the canvas crop tool cannot edit this image, or null when it can. */
export function cropBlocker(node: SceneNode, world: Matrix): string | null {
  if (node.kind !== "image") return "Only image nodes can be cropped on the canvas.";
  if (String(node.fit ?? "contain") !== "fill") return "Canvas crop needs Fit: fill, so the frame and the cropped pixels match.";
  if (!pureOffset(world)) return "Canvas crop does not support rotated or scaled images; reset the transform first.";
  return null;
}

export type Limits = { l: number; t: number; r: number; b: number };

/** The furthest the crop rectangle may reach before it would leave the source image, in page units. */
export function cropLimits(frame: Rect, crop: CropWindow, offset: { dx: number; dy: number }): Limits {
  const [cx, cy, cw, ch] = crop;
  const x = frame.x + offset.dx;
  const y = frame.y + offset.dy;
  return {
    l: x - (cx * frame.width) / cw,
    t: y - (cy * frame.height) / ch,
    r: x + frame.width + ((1 - cx - cw) * frame.width) / cw,
    b: y + frame.height + ((1 - cy - ch) * frame.height) / ch,
  };
}

export type CropHandle = "nw" | "n" | "ne" | "e" | "se" | "s" | "sw" | "w";

/** Drag `handle` to `p`, keeping the rectangle inside `limits` and optionally at `ratio` (width / height). */
export function dragCrop(box: Rect, handle: CropHandle, p: { x: number; y: number }, limits: Limits, ratio: number | null): Rect {
  const MIN = 8;
  const dirX = handle.includes("w") ? -1 : handle.includes("e") ? 1 : 0;
  const dirY = handle.includes("n") ? -1 : handle.includes("s") ? 1 : 0;
  const l = box.x;
  const t = box.y;
  const r = box.x + box.width;
  const b = box.y + box.height;
  const mx = (l + r) / 2;
  const my = (t + b) / 2;
  let w = dirX > 0 ? p.x - l : dirX < 0 ? r - p.x : box.width;
  let h = dirY > 0 ? p.y - t : dirY < 0 ? b - p.y : box.height;
  const maxW = dirX > 0 ? limits.r - l : dirX < 0 ? r - limits.l : 2 * Math.min(mx - limits.l, limits.r - mx);
  const maxH = dirY > 0 ? limits.b - t : dirY < 0 ? b - limits.t : 2 * Math.min(my - limits.t, limits.b - my);
  if (ratio) {
    if (dirX) h = w / ratio;
    else w = h * ratio;
    w = Math.max(w, MIN);
    h = w / ratio;
    const scale = Math.min(1, maxW / w, maxH / h);
    w *= scale;
    h = w / ratio;
  } else {
    w = Math.min(Math.max(w, Math.min(MIN, maxW)), maxW);
    h = Math.min(Math.max(h, Math.min(MIN, maxH)), maxH);
  }
  const x = dirX > 0 ? l : dirX < 0 ? r - w : mx - w / 2;
  const y = dirY > 0 ? t : dirY < 0 ? b - h : my - h / 2;
  return { x: round(x), y: round(y), width: round(w), height: round(h) };
}

/** Largest rectangle of `ratio` centered in `box`. */
export function fitRatio(box: Rect, ratio: number): Rect {
  let width = box.width;
  let height = width / ratio;
  if (height > box.height) {
    height = box.height;
    width = height * ratio;
  }
  return { x: round(box.x + (box.width - width) / 2), y: round(box.y + (box.height - height) / 2), width: round(width), height: round(height) };
}

/**
 * The frame (node units) and source window a finished crop rectangle maps to. The pixels
 * stay where they are on the page; only the visible window around them changes.
 */
export function applyCrop(frame: Rect, crop: CropWindow, offset: { dx: number; dy: number }, draft: Rect): { frame: Rect; crop: CropWindow } {
  const [cx, cy, cw, ch] = crop;
  const x0 = frame.x + offset.dx;
  const y0 = frame.y + offset.dy;
  const six = (v: number) => Math.round(v * 1e6) / 1e6;
  const nx = Math.min(1, Math.max(0, cx + ((draft.x - x0) / frame.width) * cw));
  const ny = Math.min(1, Math.max(0, cy + ((draft.y - y0) / frame.height) * ch));
  const nw = Math.min(1 - nx, Math.max(1e-6, (draft.width / frame.width) * cw));
  const nh = Math.min(1 - ny, Math.max(1e-6, (draft.height / frame.height) * ch));
  return {
    frame: { x: round(draft.x - offset.dx), y: round(draft.y - offset.dy), width: draft.width, height: draft.height },
    crop: [six(nx), six(ny), six(nw), six(nh)],
  };
}
