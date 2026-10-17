import { noteServerRender, renderLocal, useRenderStats } from "@/lib/wasmCore";
// Thin client for the Pentool server. The server stays the single source of
// truth; this file never re-implements document logic.
export type Doc = { version: number; pages: Page[]; [key: string]: unknown };
export type Page = {
  id: string;
  name?: string;
  layers?: Layer[];
  canvas?: { width: number; height: number };
  production?: unknown;
  dieline_layout?: { dieline: string; coordinates: "mm" };
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

export type FidelityReport = {
  schema_version: number;
  format: string;
  source: string;
  counts: { exact: number; approximated: number; dropped: number };
  items: {
    source_id: string;
    feature: string;
    status: "exact" | "approximated" | "dropped";
    message: string;
    action?: string;
  }[];
  items_omitted: number;
};

export class ApiError extends Error {}

async function check(response: Response): Promise<Response> {
  if (response.ok) return response;
  let message = `${response.status} ${response.statusText}`;
  try {
    const body = await response.json();
    if (typeof body?.error === "string") message = body.error;
    else if (body?.error?.message) {
      // Structured design error: {code, message, action?}.
      message = `${body.error.message}${body.error.action ? ` (${body.error.action})` : ""}`;
    }
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

export async function postBytes<T>(url: string, bytes: Blob, headers: Record<string, string> = {}): Promise<T> {
  const r = await fetch(url, { method: "POST", headers, body: bytes });
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
  scene3d_ppi?: {
    node: string;
    width_px: number;
    height_px: number;
    placed_width: number;
    placed_height: number;
    effective_ppi: { x: number; y: number; units_per_inch: number };
  }[];
  scene3d_ppi_omitted?: number;
};
export type PlateListing = {
  width: number;
  height: number;
  plates: { plate: string; spot: boolean; average_coverage_percent: number; max_percent: number }[];
};

export type Scene3dUvIsland = {
  id: string;
  uv_set: number;
  triangle_count: number;
  uv_bounds: { min: [number, number]; max: [number, number] };
  mirrored: boolean;
  texture_resolution_px: [number, number];
  texel_density_px_per_model_unit: number | null;
  below_minimum_texel_density: boolean | null;
};
export type Scene3dUvTexelDensity = {
  uv_set: number;
  texture_resolution_px: [number, number];
  texel_density_px_per_model_unit: number | null;
  below_minimum_texel_density: boolean | null;
  model_area_unit: "model_unit";
};
export type Scene3dUvDiscontinuityReport = {
  uv_set: number;
  count: number;
  analyzed_triangles: number;
  analysis_truncated: boolean;
  details_truncated: boolean;
  render_segments_truncated: boolean;
  edges: { triangles: number[]; occurrence_count: number; segments_truncated: boolean; uv_segments: [[[number, number], [number, number]]][] }[];
};
export type Scene3dUvPrimitive = {
  primitive: number;
  name?: string;
  triangles: number;
  vertices: number;
  uv_sets: number;
  texel_density: Scene3dUvTexelDensity[];
  uv_discontinuities: Scene3dUvDiscontinuityReport[];
  islands?: Scene3dUvIsland[];
  island_overlap_candidates?: [string, string][];
  island_overlap_analysis_truncated?: boolean;
};

// Studio edits replace the document object. Reuse previews only for the same
// immutable snapshot, page, size and renderer setting; failed renders can retry.
const previewCache = new WeakMap<Doc, Map<string, Promise<Blob>>>();

export const api = {
  health: () => getJson<{ ok: boolean; version: string }>("/api/health"),
  document: () => getJson<{ revision: string; document: Doc }>("/api/document"),
  migrate: (document: Doc, target: number) =>
    postJson<{ document: Doc; from_version: number; to_version: number }>("/api/migrate", { document, target }),
  undo: () => postJson<unknown>("/api/undo", {}),
  redo: () => postJson<unknown>("/api/redo", {}),
  async renderPng(doc: Doc, page: string | undefined, maxEdge: number): Promise<Blob> {
    let cache = previewCache.get(doc);
    if (!cache) {
      cache = new Map();
      previewCache.set(doc, cache);
    }
    const key = JSON.stringify([page, maxEdge, useRenderStats.getState().enabled]);
    const cached = cache.get(key);
    if (cached) return cached;
    const request = (async () => {
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
    })();
    cache.set(key, request);
    if (cache.size > 4) cache.delete(cache.keys().next().value!);
    void request.catch(() => {
      if (cache.get(key) === request) cache.delete(key);
    });
    return request;
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
  scene3dRenderReport: (body: { document: Doc; page: string; node: string }) =>
    postJson<{ node: string; page: string; document_revision: string; local: true; render: { format: "pentool-scene3d-render-report-v1"; output: { format: "png"; width_px: number; height_px: number; bytes: number }; effective_ppi: { x: number; y: number; units_per_inch: number } | null; renderer_requested: "reference" | "accelerated"; renderer_used: "reference" | "accelerated"; cache_hit: boolean } }>("/api/scene3d/render-report", body),
  scene3dImport: (body: { document: Doc; data: string; format: "glb" | "gltf" | "obj"; id: string; layer: string; page?: string; x: number; y: number; width: number; height: number }) =>
    postJson<{ document: Doc; result: { id: string; model: string; report: unknown } }>("/api/scene3d/import", body),
  scene3dForm: (body: { document: Doc; action: "add" | "set"; form_id: string; preset?: string; kind?: string; parameters?: Record<string, unknown>; material_slots?: string[]; node_id?: string; layer?: string; page?: string; placement?: [number, number, number, number] }) =>
    postJson<{ document: Doc; result: { node?: string; form: string; vertices: number; triangles: number } }>("/api/scene3d/form", body),
  scene3dFold: (body: { document: Doc; dieline: string; fold_state: number }) =>
    postJson<{ document: Doc; result: { dieline: string; fold_state: number } }>("/api/scene3d/fold", body),
  scene3dCreaseAngle: (body: { document: Doc; dieline: string; crease: string; angle_deg: number }) =>
    postJson<{ document: Doc; result: { dieline: string; crease: string; previous_angle_deg: number; angle_deg: number } }>("/api/scene3d/crease-angle", body),
  scene3dCreaseDirection: (body: { document: Doc; dieline: string; crease: string; direction: "mountain" | "valley" }) =>
    postJson<{ document: Doc; result: { dieline: string; crease: string; previous_direction: string; direction: "mountain" | "valley" } }>("/api/scene3d/crease-direction", body),
  scene3dPackSettings: (body: { document: Doc; dieline: string; board_mm?: number; bleed_mm?: number; seam_angle_deg?: number; wrap_direction?: "clockwise" | "counterclockwise" }) =>
    postJson<{ document: Doc; result: { dieline: string; previous_board_mm: number; board_mm: number; previous_bleed_mm: number; bleed_mm: number; previous_seam_angle_deg: number | null; seam_angle_deg: number | null; previous_wrap_direction: string | null; wrap_direction: string | null } }>("/api/scene3d/pack/settings", body),
  scene3dPackPreflight: (body: { document: Doc; dieline: string; min_ppi?: number }) =>
    postJson<{ result: { id: string; template?: string; preflight_status: "mockup-ready" | "needs-review"; manufacturing_certified: false; panel_count: number; panel_art_bound: number; panels_without_artwork: string[]; fold_hinge_checks: { kind: "hinge" | "wrap-seam"; panels: string[]; gap_mm: number; tolerance_mm: number; joined: boolean }[]; fold_collision_report: { status: "checked-clear" | "potential-overlaps"; fold_state: number; pairs_tested: number; collision_count: number; collisions: { panels: string[]; kind: "zero-thickness-intersection" }[]; collisions_omitted: number; physical_thickness_tested: false } | null; rasterized_panels: string[]; rasterized_ppi: { panel: string; effective_ppi: number }[]; bleed_synthesized: { panel: string; bleed_mm: number; raster_ppi: number; pixel_width: number; pixel_height: number; cut_edges: string[]; method: string }[]; safe_zone_checks: { panel: string; inset_mm: number | null; within_safe_zone: boolean }[]; wrap_seam_checks: { panel: string; seam_angle_deg: number; direction: string; overlap_mm: number; touches_start_edge: boolean; touches_end_edge: boolean; touches_seam: boolean; method: string }[]; pdfx_output_role: { configured: boolean; space: string | null; profile: string | null; message: string | null }; minimum_ppi: number; warnings: string[]; production_blockers: string[] } }>("/api/scene3d/pack/preflight", body),
  scene3dPackFlatPreview: (body: { document: Doc; dieline: string; min_ppi?: number }) =>
    postJson<{ svg: string; report: { dieline: string; artwork_panels: string[]; rasterized_panels: string[]; rasterized_ppi: { panel: string; effective_ppi: number }[]; bleed_synthesized: { panel: string; bleed_mm: number; raster_ppi: number; pixel_width: number; pixel_height: number; cut_edges: string[]; method: string }[]; minimum_ppi: number; warnings: string[]; manufacturing_certified: false } }>("/api/scene3d/pack/flat-preview", body),
  scene3dPackFacePreview: (body: { document: Doc; page: string; node: string; panel: string }) =>
    postJson<{ node: string; page: string; panel: string; png_base64: string; max_edge_px: number }>("/api/scene3d/pack/face-preview", body),
  async scene3dPackExportBundle(body: { document: Doc; dieline: string; min_ppi?: number; ppi?: number; fold?: number }) {
    const response = await check(await fetch("/api/scene3d/pack/export-bundle", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }));
    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = "pentool-packaging-bundle.zip";
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  },
  scene3dSleeve: (body: { document: Doc; dieline_id: string; node_id: string; layer: string; page?: string; length_mm: number; width_mm: number; height_mm: number; board_mm: number; bleed_mm: number; placement: [number, number, number, number] }) =>
    postJson<{ document: Doc; result: { node: string; dieline: string; panels: number; fold_state: number } }>("/api/scene3d/sleeve", body),
  scene3dCarton: (body: { document: Doc; dieline_id: string; node_id: string; layer: string; page?: string; template: "straight-tuck" | "reverse-tuck" | "mailer"; length_mm: number; width_mm: number; height_mm: number; board_mm: number; bleed_mm: number; glue_flap_mm: number; placement: [number, number, number, number] }) =>
    postJson<{ document: Doc; result: { node: string; dieline: string; template: string; panels: number; fold_state: number } }>("/api/scene3d/carton", body),
  scene3dTubeLabel: (body: { document: Doc; dieline_id: string; node_id: string; layer: string; page?: string; radius_mm: number; height_mm: number; bleed_mm: number; overlap_mm?: number; placement: [number, number, number, number] }) =>
    postJson<{ document: Doc; result: { node: string; dieline: string; template: string; panels: number; fold_state: number } }>("/api/scene3d/tube-label", body),
  scene3dPouch: (body: { document: Doc; dieline_id: string; node_id: string; layer: string; page?: string; front_width_mm: number; gusset_mm: number; height_mm: number; bleed_mm: number; placement: [number, number, number, number] }) =>
    postJson<{ document: Doc; result: { node: string; dieline: string; template: string; panels: number; fold_state: number } }>("/api/scene3d/pouch", body),
  scene3dBottleLabel: (body: { document: Doc; dieline_id: string; node_id: string; layer: string; page?: string; radius_mm: number; height_mm: number; bleed_mm: number; overlap_mm?: number; placement: [number, number, number, number] }) =>
    postJson<{ document: Doc; result: { node: string; dieline: string; template: string; panels: number; fold_state: number } }>("/api/scene3d/bottle-label", body),
  scene3dPanelArt: (body: { document: Doc; dieline: string; panel: string; source?: { kind: "page" | "node"; page: string; node?: string } }) =>
    postJson<{ document: Doc; result: { dieline: string; panel: string; artwork: unknown } }>("/api/scene3d/panel-art", body),
  scene3dPanelArtTransform: (body: { document: Doc; dieline: string; panel: string; scale: [number, number]; offset: [number, number]; rotate: number }) =>
    postJson<{ document: Doc; result: { dieline: string; panel: string; artwork_transform: { scale: [number, number]; offset: [number, number]; rotate: number } } }>("/api/scene3d/panel-art-transform", body),
  scene3dPanelSafeZone: (body: { document: Doc; dieline: string; panel: string; inset_mm: number }) =>
    postJson<{ document: Doc; result: { dieline: string; panel: string; safe_zone_mm: number } }>("/api/scene3d/panel-safe-zone", body),
  scene3dPathRole: (body: { document: Doc; dieline: string; node: string; role: "cut" | "crease" | "perforation" | "bleed" | "safe-zone" | "glue-flap" }) =>
    postJson<{ document: Doc; result: { dieline: string; path: string; node: string; role: string; spot: string } }>("/api/scene3d/path-role", body),
  scene3dPathCrease: (body: { document: Doc; dieline: string; node: string; crease: string; angle_deg: number; direction: "mountain" | "valley" }) =>
    postJson<{ document: Doc; result: { dieline: string; crease: string; node: string; line_mm: [[number, number], [number, number]]; angle_deg: number; direction: "mountain" | "valley" } }>("/api/scene3d/path-crease", body),
  scene3dUvPreview: (document: Doc, node: string, primitive: number, size = 1024, textureResolution = 2048, minimumDensity?: number) =>
    postJson<{ node: string; model: string; primitive: number; material_slot: string | null; svg: string; png_base64: string; report: { primitives: Scene3dUvPrimitive[] } }>("/api/scene3d/uv/preview", { document, node, primitive, size, texture_resolution: textureResolution, minimum_density: minimumDensity }),
  scene3dUvModelPreview: (body: { document: Doc; page: string; node: string; primitive: number; island?: string }) =>
    postJson<{ node: string; page: string; primitive: number; island: string | null; png_base64: string; max_edge_px: number }>("/api/scene3d/uv/model-preview", body),
  scene3dUvFacePick: (body: { document: Doc; page: string; node: string; primitive?: number; x: number; y: number }) =>
    postJson<{ node: string; page: string; primitive: number | null; panel: string | null; triangle: number | null; island: string | null; material_slot: string | null; uv: [number, number] | null }>("/api/scene3d/uv/face-pick", body),
  scene3dUvPack: (body: { document: Doc; page: string; node: string; primitive: number; padding: number }) =>
    postJson<{ document: Doc; result: { node: string; page: string; primitive: number; padding: number; islands: number } }>("/api/scene3d/uv/pack", body),
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

