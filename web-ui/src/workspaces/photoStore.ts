import { create } from "zustand";
import { LABELS, getPath, photoApi, type Develop, type PhotoDetail, type PhotoSummary, type PreviewReport, type Shot } from "@/lib/photo";

export type PhotoView = "grid" | "loupe" | "compare";
export type Overlay = "none" | "clipping" | "gamut";

const p3 = typeof matchMedia === "function" && matchMedia("(color-gamut: p3)").matches;

// Thumbnails are developed one at a time and cached until an edit.
const thumbs = new Map<string, string>();
const waiting: { id: string; done: (url: string | null) => void }[] = [];
let draining = false;
async function drain(space: () => string) {
  if (draining) return;
  draining = true;
  while (waiting.length) {
    const job = waiting.shift()!;
    try {
      if (!thumbs.has(job.id)) {
        const shot = await photoApi.preview({ photo: job.id, variant: "master", edge: 320, space: space(), overlay: "none", uncropped: false });
        thumbs.set(job.id, shot.url);
      }
      job.done(thumbs.get(job.id) ?? null);
    } catch {
      job.done(null);
    }
  }
  draining = false;
}

type State = {
  photos: PhotoSummary[];
  selected: string[];
  current: string | null;
  detail: PhotoDetail | null;
  revision: string | null;
  query: string;
  count: string;
  view: PhotoView;
  overlay: Overlay;
  space: "srgb" | "display-p3";
  variant: string;
  compareB: string;
  shots: { name: string; shot: Shot }[];
  report: PreviewReport | null;
  cropping: boolean;
  crop: [number, number, number, number] | null;
  cropAspect: string;
  brush: string | null;
  pendingBrush: { id: string; params: { exposure: number } } | null;
  brushSize: number;
  brushErase: boolean;
  maskExposure: number;
  copied: string | null;
  status: string;
  busy: boolean;
  generation: number;
  box: { w: number; h: number } | null;
  set: (patch: Partial<State>) => void;
  run: (work: () => Promise<void>) => Promise<void>;
  loadCatalog: () => Promise<void>;
  importFiles: (files: File[]) => Promise<void>;
  loadDetail: () => Promise<void>;
  renderLoupe: (box?: { w: number; h: number }) => Promise<void>;
  edit: (op: Record<string, unknown>, message?: string) => Promise<void>;
  select: (id: string, mode?: "range" | "toggle") => void;
  setView: (view: PhotoView) => void;
  thumbnail: (id: string) => Promise<string | null>;
  clearThumbs: () => void;
  undo: () => Promise<void>;
  developOf: () => Develop;
};

export const usePhoto = create<State>((set, get) => ({
  photos: [],
  selected: [],
  current: null,
  detail: null,
  revision: null,
  query: "",
  count: "",
  view: "grid",
  overlay: "none",
  space: p3 ? "display-p3" : "srgb",
  variant: "master",
  compareB: "master",
  shots: [],
  report: null,
  cropping: false,
  crop: null,
  cropAspect: "free",
  brush: null,
  pendingBrush: null,
  brushSize: 8,
  brushErase: false,
  maskExposure: 0.5,
  copied: null,
  status: "",
  busy: false,
  generation: 0,
  box: null,
  set: (patch) => set(patch),
  developOf: () => get().detail?.photo.variants.find((v) => v.id === get().variant)?.develop ?? {},

  async run(work) {
    if (get().busy) return;
    set({ busy: true });
    try {
      await work();
    } catch (error) {
      set({ status: `${error instanceof Error ? error.message : "Request failed."} Nothing was changed; adjust and retry.` });
    } finally {
      set({ busy: false });
    }
  },

  async loadCatalog() {
    const data = await photoApi.catalog(get().query.trim());
    const selected = get().selected.filter((id) => data.photos.some((p) => p.id === id));
    let current = get().current;
    if (!data.photos.some((p) => p.id === current)) current = data.photos[0]?.id ?? null;
    if (current && !selected.length) selected.push(current);
    set({
      photos: data.photos,
      revision: data.revision,
      selected,
      current,
      count: `${data.matches} photo${data.matches === 1 ? "" : "s"}${data.has_more ? " (first 500)" : ""}`,
    });
    if (current) await get().loadDetail();
    else set({ detail: null, shots: [], report: null, status: "No photos yet. Import DNG, JPEG or PNG photos from the Photo workflow panel." });
  },

  async importFiles(files) {
    if (get().busy || files.length === 0) return;
    set({ busy: true, status: `Importing ${files.length} photo${files.length === 1 ? "" : "s"}…` });
    const imported: string[] = [];
    const errors: string[] = [];
    const used = new Set(get().photos.map((photo) => photo.id));
    let revision = get().revision;
    try {
      if (!revision) {
        await get().loadCatalog();
        revision = get().revision;
      }
      if (!revision) throw new Error("Could not read the shared photo catalog. Open a project with `pentool serve PROJECT.pen` and retry.");
      for (const file of files) {
        if (file.size === 0 || file.size > 64 * 1024 * 1024) {
          errors.push(`${file.name}: Studio accepts DNG, JPEG and PNG photos up to 64 MiB each.`);
          continue;
        }
        const stem = file.name.replace(/\.[^.]+$/, "").normalize("NFKD").replace(/[\u0300-\u036f]/g, "");
        const base = stem.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^[^A-Za-z0-9]+/, "").slice(0, 64) || "photo";
        let id = base;
        let suffix = 2;
        while (used.has(id)) id = `${base.slice(0, Math.max(1, 62 - String(suffix).length))}-${suffix++}`;
        used.add(id);
        try {
          const result = await photoApi.import(file, id, revision);
          revision = result.revision;
          imported.push(result.result.photo);
        } catch (error) {
          const message = error instanceof Error ? error.message : "Import failed.";
          errors.push(`${file.name}: ${message}`);
          if (message.includes("[conflict]")) break;
        }
      }
      if (imported.length) {
        set({ current: imported[imported.length - 1]!, selected: [imported[imported.length - 1]!], revision });
        await get().loadCatalog();
      }
      set({ status: errors.length ? `${imported.length} imported; ${errors.length} could not be imported. ${errors[0]}` : `Imported ${imported.length} photo${imported.length === 1 ? "" : "s"}.` });
    } catch (error) {
      set({ status: error instanceof Error ? error.message : "Could not import photos. Check the project and retry." });
    } finally {
      set({ busy: false });
    }
  },

  async loadDetail() {
    const current = get().current;
    if (!current) return;
    const detail = await photoApi.detail(current);
    const ids = detail.photo.variants.map((v) => v.id);
    const keep = (id: string) => (ids.includes(id) ? id : (ids[0] ?? "master"));
    set({ detail, revision: detail.revision, variant: keep(get().variant), compareB: keep(get().compareB) });
    if (get().view !== "grid") await get().renderLoupe();
  },

  async renderLoupe(given) {
    const s = get();
    const box = given ?? s.box ?? undefined;
    if (!s.current || s.view === "grid") return;
    const generation = s.generation + 1;
    set({ generation, status: "Developing…" });
    const dpr = typeof devicePixelRatio === "number" ? devicePixelRatio : 1;
    const edge = Math.min(4096, Math.ceil(Math.max(box?.w ?? 0, box?.h ?? 0) * dpr) || 1600);
    const uncropped = s.cropping || !!s.brush;
    const overlay = s.brush ? `mask:${s.brush}` : s.overlay;
    const base = { photo: s.current, space: s.space, uncropped };
    if (s.view === "compare") {
      const half = Math.ceil(edge / 2);
      const [a, b] = await Promise.all([
        photoApi.preview({ ...base, variant: s.variant, edge: half, overlay: s.overlay, uncropped: false }),
        photoApi.preview({ ...base, variant: s.compareB, edge: half, overlay: s.overlay, uncropped: false }),
      ]);
      if (generation !== get().generation) return;
      set({ shots: [{ name: s.variant, shot: a }, { name: s.compareB, shot: b }], report: a.report });
    } else {
      const shot = await photoApi.preview({ ...base, variant: s.variant, edge, overlay });
      if (generation !== get().generation) return;
      set({ shots: [{ name: s.variant, shot }], report: shot.report });
    }
    set({ status: `${s.current} · ${s.variant} · ${s.space === "srgb" ? "sRGB" : "Display P3"}` });
  },

  async edit(op, message) {
    const result = await photoApi.edit(op, get().revision);
    set({ revision: result.revision, status: message ?? "Saved. Undo restores the previous settings." });
    const current = get().current;
    if (current) thumbs.delete(current);
    await get().loadCatalog();
  },

  select(id, mode) {
    const s = get();
    let selected = [id];
    if (mode === "range" && s.current) {
      const ids = s.photos.map((p) => p.id);
      const [a, b] = [ids.indexOf(s.current), ids.indexOf(id)].sort((x, y) => x - y) as [number, number];
      selected = ids.slice(a, b + 1);
    } else if (mode === "toggle") {
      selected = s.selected.includes(id) ? s.selected.filter((x) => x !== id) : [...s.selected, id];
    }
    set({ selected, current: id, brush: null, cropping: false });
    void s.run(() => get().loadDetail());
  },

  setView(view) {
    set({ view });
    if (view !== "grid") void get().run(() => get().renderLoupe());
  },

  thumbnail(id) {
    const hit = thumbs.get(id);
    if (hit) return Promise.resolve(hit);
    return new Promise((resolve) => {
      waiting.push({ id, done: resolve });
      void drain(() => get().space);
    });
  },
  clearThumbs: () => thumbs.clear(),

  async undo() {
    await photoApi.undo();
    thumbs.clear();
    await get().loadCatalog();
    set({ status: "Undone." });
  },
}));

export { LABELS, getPath };
