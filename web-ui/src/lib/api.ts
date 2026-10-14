import { noteServerRender, renderLocal } from "@/lib/wasmCore";
// Thin client for the Pentool server. The server stays the single source of
// truth; this file never re-implements document logic.
export type Doc = { version: number; pages: Page[]; [key: string]: unknown };
export type Page = {
  id: string;
  name?: string;
  layers?: Layer[];
  canvas?: { width: number; height: number };
  production?: unknown;
};
export type Layer = {
  id: string;
  name?: string;
  nodes?: SceneNode[];
  paths?: SceneNode[];
  texts?: SceneNode[];
  locked?: boolean;
  visible?: boolean;
};
export type SceneNode = { id: string; kind?: string; name?: string; children?: SceneNode[]; [key: string]: unknown };

export class ApiError extends Error {}

async function check(response: Response): Promise<Response> {
  if (response.ok) return response;
  let message = `${response.status} ${response.statusText}`;
  try {
    const body = await response.json();
    if (body?.error) message = String(body.error);
  } catch {
    // non-JSON error body
  }
  throw new ApiError(message);
}

export async function getJson<T>(url: string): Promise<T> {
  return (await check(await fetch(url))).json();
}

export async function postJson<T>(url: string, body: unknown): Promise<T> {
  const r = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  return (await check(r)).json();
}

export type PreflightIssue = {
  severity: "error" | "warning";
  code: string;
  node?: string;
  message: string;
  fix: string;
};
export type PreflightReport = {
  ok: boolean;
  errors: number;
  warnings: number;
  issues: PreflightIssue[];
  production: unknown;
};
export type PlateListing = {
  width: number;
  height: number;
  plates: { plate: string; spot: boolean; average_coverage_percent: number; max_percent: number }[];
};

export const api = {
  health: () => getJson<{ ok: boolean; version: string }>("/api/health"),
  document: () => getJson<{ revision: string; document: Doc }>("/api/document"),
  migrate: (document: Doc, target: number) =>
    postJson<{ document: Doc; from_version: number; to_version: number }>("/api/migrate", { document, target }),
  undo: () => postJson<unknown>("/api/undo", {}),
  redo: () => postJson<unknown>("/api/redo", {}),
  async renderPng(doc: Doc, page: string | undefined, maxEdge: number): Promise<Blob> {
    const local = await renderLocal(doc, page, maxEdge);
    if (local) return local;
    const started = performance.now();
    const q = new URLSearchParams({ max_edge: String(maxEdge) });
    if (page) q.set("page", page);
    const r = await check(
      await fetch(`/api/render/png?${q}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(doc),
      }),
    );
    const blob = await r.blob();
    noteServerRender(performance.now() - started);
    return blob;
  },
  preflight(opts: { page?: string; min_ppi: number; min_bleed_inches: number; max_total_ink?: number }) {
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(opts)) if (v !== undefined) q.set(k, String(v));
    return getJson<PreflightReport>(`/api/color/preflight?${q}`);
  },
  plates: (page?: string) => getJson<PlateListing>(`/api/color/plates${page ? `?page=${page}` : ""}`),
  plateUrl: (plate: string, page?: string) =>
    `/api/color/plates?plate=${encodeURIComponent(plate)}${page ? `&page=${page}` : ""}`,
  proofUrl: (page?: string) => `/api/color/proof${page ? `?page=${page}` : ""}`,
  fonts: () => getJson<{ faces: { family: string }[] }>("/api/fonts"),
  assets: (query: string) =>
    getJson<{ assets: { id: string; name: string; library: string; version: string }[] }>(
      `/api/assets?query=${encodeURIComponent(query)}&limit=60`,
    ),
  asset: (spec: string) =>
    getJson<{ document: Record<string, unknown>; asset: { name: string } }>(`/api/asset?spec=${encodeURIComponent(spec)}`),
  scene3dStatus: () =>
    getJson<{ active: "reference" | "accelerated"; accelerated_available: boolean; adapter?: string; backend?: string; fallback_reason?: string }>("/api/scene3d/status"),
  scene3dImport: (body: { document: Doc; data: string; format: "glb" | "gltf" | "obj"; id: string; layer: string; page?: string; x: number; y: number; width: number; height: number }) =>
    postJson<{ document: Doc; result: { id: string; model: string; report: unknown } }>("/api/scene3d/import", body),
};

export type LayoutIssue = {
  severity: "error" | "warning";
  code: string;
  message: string;
  action: string;
  page?: string;
  frame?: string;
  story?: string;
  offset?: number;
  node?: string;
};
export type LayoutPreflightReport = {
  ok: boolean;
  errors: number;
  warnings: number;
  issues_total: number;
  truncated: boolean;
  issues: LayoutIssue[];
  composed: { lines: number; stories_overset: number; fonts: number };
};
export type LayoutOverview = {
  stories: {
    id: string;
    paragraphs: number;
    characters: number;
    frames: string[];
    placed: boolean;
    overset: boolean;
    overset_characters: number;
  }[];
  frames: LayoutFrame[];
};

export const layoutApi = {
  preflight(opts: { bleed: number; max_overrides: number; no_alt: boolean }) {
    const q = new URLSearchParams({
      bleed: String(opts.bleed),
      max_overrides: String(opts.max_overrides),
      no_alt: String(opts.no_alt),
    });
    return getJson<LayoutPreflightReport>(`/api/layout/preflight?${q}`);
  },
  overview: () => getJson<LayoutOverview>("/api/layout/overview"),
  exportUrl: (format: "pdf" | "epub", fixed = false) =>
    `/api/layout/export?format=${format}${fixed ? "&fixed=true" : ""}`,
};

export type LayoutFrame = {
  id: string;
  story: string;
  page: string;
  x: number;
  y: number;
  width: number;
  height: number;
  next?: string | null;
  overset: boolean;
};

/** One layout edit or read, run by the Rust planners on the open document. */
export type LayoutOp =
  | { type: "overview" }
  | { type: "preflight"; bleed?: number; max_overrides?: number; no_alt?: boolean }
  | { type: "story"; action: Record<string, unknown> }
  | { type: "frame"; action: Record<string, unknown> }
  | { type: "page-add"; id: string; after?: string; parent?: string }
  | { type: "page-duplicate"; id: string; new_id: string }
  | { type: "page-remove"; id: string; drop_story?: boolean }
  | { type: "page-move"; id: string; index: number }
  | { type: "page-rename"; id: string; name: string }
  | { type: "page-parent"; page: string; parent: string | null }
  | { type: "spread-facing"; spread: string; facing: boolean }
  | { type: "table" | "wrap" | "style" | "grid" | "doc"; action: Record<string, unknown> };

export const layoutOps = {
  run: <T = unknown>(document: Doc, op: LayoutOp) =>
    postJson<{ document?: Doc; result: T }>("/api/layout", { document, op }),
  overview: (document: Doc) =>
    layoutOps.run<LayoutOverview>(document, { type: "overview" }).then((r) => r.result),
  preflight: (document: Doc, opts: { bleed: number; max_overrides: number; no_alt: boolean }) =>
    layoutOps.run<LayoutPreflightReport>(document, { type: "preflight", ...opts }).then((r) => r.result),
  /** Download the open document (unsaved edits included) as tagged PDF or EPUB. */
  async download(doc: Doc, format: "pdf" | "epub", fixed = false) {
    const r = await check(
      await fetch("/api/layout/export", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ document: doc, format, fixed }),
      }),
    );
    const blob = await r.blob();
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `${String(doc.name ?? "layout")}${fixed ? "-fixed" : ""}.${format}`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
  },
};

