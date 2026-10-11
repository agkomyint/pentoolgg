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
    const q = new URLSearchParams({ max_edge: String(maxEdge) });
    if (page) q.set("page", page);
    const r = await check(
      await fetch(`/api/render/png?${q}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(doc),
      }),
    );
    return r.blob();
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
};
