// Editor operations. Structural edits are local JSON edits that the Rust server
// validates on every render and on save; heavy work (images, import, fonts,
// scene batches) always goes through the shared Rust endpoints.
import { api, postJson, type Doc, type FidelityReport, type SceneNode } from "@/lib/api";
import {
  addToLayer,
  affinePath,
  apply,
  invert,
  multiply,
  nodeMatrix,
  type Matrix,
  children,
  clone,
  collectIds,
  findNode,
  isScene,
  layerNodes,
  makeNode,
  nodeKind,
  pageOf,
  uid,
  type Found,
  type Tool,
} from "@/lib/model";
import { useStudio } from "@/store";

const state = () => useStudio.getState();
const fail = (error: unknown, fallback: string) =>
  state().notify(error instanceof Error ? error.message : fallback, true);

function freshId(doc: Doc, prefix: string): string {
  const ids = collectIds(doc);
  let id = uid(prefix);
  while (ids.has(id)) id = uid(prefix);
  return id;
}

function currentLayer(doc: Doc, pageId: string | undefined, layerId: string | undefined) {
  const page = pageOf(doc, pageId);
  return page?.layers?.find((l) => l.id === layerId) ?? page?.layers?.[0];
}

/** Add a new object built by a drawing tool and select it. */
export function addObject(
  tool: Exclude<Tool, "select" | "hand">,
  geometry: Parameters<typeof makeNode>[4],
): string | null {
  const { doc, pageId, layerId, look } = state();
  if (!doc) return null;
  const layer = currentLayer(doc, pageId, layerId);
  if (!layer || layer.locked || layer.visible === false) {
    state().notify("Choose a visible, unlocked layer first.", true);
    return null;
  }
  const id = freshId(doc, tool === "pen" || tool === "line" ? "path" : tool);
  state().edit(`Add ${tool}`, (draft) => {
    const target = currentLayer(draft, pageId, layerId)!;
    const made = makeNode(tool, id, isScene(draft), look, geometry);
    addToLayer(target, made.node, made.bucket);
  });
  state().select(id);
  return id;
}

export function deleteSelected(): void {
  const { selectedId, selectedIds, pageId } = state();
  if (!selectedId) return;
  const ids = selectedIds.length ? selectedIds : [selectedId];
  state().edit("Delete", (draft) => {
    for (const id of ids) {
      const found = findNode(pageOf(draft, pageId), id);
      if (!found || found.layer.locked) continue;
      found.siblings.splice(found.siblings.indexOf(found.node), 1);
    }
  });
  state().select(null);
}

export function duplicateSelected(): void {
  const { selectedId, pageId, doc } = state();
  if (!selectedId || !doc) return;
  const next = freshId(doc, "copy");
  state().edit("Duplicate", (draft) => {
    const found = findNode(pageOf(draft, pageId), selectedId);
    if (!found || found.layer.locked) return;
    const copy = clone(found.node);
    const seen = collectIds(draft);
    const reassign = (node: SceneNode, root: boolean) => {
      let id = root ? next : uid("copy");
      while (seen.has(id)) id = uid("copy");
      seen.add(id);
      node.id = id;
      children(node).forEach((child) => reassign(child, false));
    };
    reassign(copy, true);
    // Offset so the copy is visibly distinct from the original.
    const m = Array.isArray(copy.transform) ? (copy.transform as number[]) : [1, 0, 0, 1, 0, 0];
    if (typeof found.node.kind === "string" || Array.isArray(copy.transform)) {
      copy.transform = [m[0], m[1], m[2], m[3], (m[4] ?? 0) + 16, (m[5] ?? 0) + 16];
    } else if (typeof copy.x === "number") {
      copy.x += 16;
      copy.y = Number(copy.y ?? 0) + 16;
    } else {
      copy.transform = [1, 0, 0, 1, 16, 16];
    }
    found.siblings.splice(found.siblings.indexOf(found.node) + 1, 0, copy);
  });
  state().select(next);
}

export function reorderSelected(direction: "front" | "forward" | "backward" | "back"): void {
  const { selectedId, pageId } = state();
  if (!selectedId) return;
  state().edit("Reorder", (draft) => {
    const found = findNode(pageOf(draft, pageId), selectedId);
    if (!found || found.layer.locked) return;
    const list = found.siblings;
    const from = list.indexOf(found.node);
    const to = { front: list.length - 1, back: 0, forward: Math.min(list.length - 1, from + 1), backward: Math.max(0, from - 1) }[direction];
    list.splice(from, 1);
    list.splice(to, 0, found.node);
  });
}

export function groupSelected(): void {
  const { selectedId, pageId, doc } = state();
  if (!selectedId || !doc) return;
  if (!isScene(doc)) {
    state().notify("Grouping needs a scene document (v4+). Use File > Upgrade to v4.", true);
    return;
  }
  const id = freshId(doc, "group");
  state().edit("Group", (draft) => {
    const found = findNode(pageOf(draft, pageId), selectedId);
    if (!found || found.layer.locked) return;
    const index = found.siblings.indexOf(found.node);
    found.siblings.splice(index, 1, { kind: "group", id, transform: [1, 0, 0, 1, 0, 0], children: [found.node] });
  });
  state().select(id);
}

export function ungroupSelected(): void {
  const { selectedId, pageId } = state();
  if (!selectedId) return;
  state().edit("Ungroup", (draft) => {
    const found = findNode(pageOf(draft, pageId), selectedId);
    if (!found || found.node.kind !== "group" || found.layer.locked) return;
    const m = (Array.isArray(found.node.transform) ? found.node.transform : [1, 0, 0, 1, 0, 0]) as number[];
    const kids = children(found.node).map((kid) => {
      const k = Array.isArray(kid.transform) ? (kid.transform as number[]) : [1, 0, 0, 1, 0, 0];
      // child' = group * child, so the painted position is preserved.
      kid.transform = [
        m[0]! * k[0]! + m[2]! * k[1]!,
        m[1]! * k[0]! + m[3]! * k[1]!,
        m[0]! * k[2]! + m[2]! * k[3]!,
        m[1]! * k[2]! + m[3]! * k[3]!,
        m[0]! * k[4]! + m[2]! * k[5]! + m[4]!,
        m[1]! * k[4]! + m[3]! * k[5]! + m[5]!,
      ];
      return kid;
    });
    found.siblings.splice(found.siblings.indexOf(found.node), 1, ...kids);
  });
  state().select(null);
}

export function renameNode(id: string, name: string): void {
  const { pageId } = state();
  state().edit("Rename", (draft) => {
    const found = findNode(pageOf(draft, pageId), id);
    if (found) found.node.name = name;
  });
}

export function updateNode(id: string, change: (node: SceneNode, found: Found, scene: boolean) => void): void {
  const { pageId } = state();
  state().edit("Edit", (draft) => {
    const found = findNode(pageOf(draft, pageId), id);
    if (found && !found.layer.locked) change(found.node, found, isScene(draft));
  });
}

export function replaceNodeJson(id: string, source: string): void {
  try {
    const parsed = JSON.parse(source) as SceneNode;
    if (!parsed || typeof parsed !== "object" || typeof parsed.id !== "string") throw new Error("A node needs a string id.");
    const { pageId } = state();
    state().edit("Replace node", (draft) => {
      const found = findNode(pageOf(draft, pageId), id);
      if (found) found.siblings.splice(found.siblings.indexOf(found.node), 1, parsed);
    });
    state().select(parsed.id);
  } catch (error) {
    fail(error, "Invalid JSON.");
  }
}

// ---- layers and pages ----

export function addLayer(): void {
  const { doc, pageId } = state();
  if (!doc) return;
  const id = freshId(doc, "layer");
  state().edit("Add layer", (draft) => {
    const page = pageOf(draft, pageId)!;
    const scene = isScene(draft);
    const count = page.layers?.length ?? 0;
    const layer = { id, name: `Layer ${count + 1}`, visible: true, locked: false, ...(scene ? { nodes: [] } : { paths: [], texts: [] }) };
    (page.layers ??= []).push(layer);
  });
  state().setLayer(id);
}

export function patchLayer(id: string, patch: { name?: string; visible?: boolean; locked?: boolean }): void {
  const { pageId } = state();
  state().edit("Layer", (draft) => {
    const layer = pageOf(draft, pageId)?.layers?.find((l) => l.id === id);
    if (layer) Object.assign(layer, patch);
  });
}

export function removeLayer(id: string): void {
  const { pageId } = state();
  state().edit("Remove layer", (draft) => {
    const page = pageOf(draft, pageId);
    if (!page?.layers || page.layers.length <= 1) return;
    const layers = page.layers.filter((l) => l.id !== id);
    if (Array.isArray(draft.pages)) page.layers = layers;
    else draft.layers = layers;
  });
  state().select(null);
}

export function addPage(): void {
  const { doc } = state();
  if (!doc) return;
  const id = freshId(doc, "page");
  const layerId = freshId(doc, "layer");
  state().edit("Add page", (draft) => {
    const source = pageOf(draft, state().pageId)!;
    const scene = isScene(draft);
    draft.pages.push({
      id,
      name: `Page ${draft.pages.length + 1}`,
      canvas: { ...(source.canvas ?? { width: 1200, height: 800 }), background: "#ffffff" },
      layers: [{ id: layerId, name: "Layer 1", visible: true, locked: false, ...(scene ? { nodes: [] } : { paths: [], texts: [] }) }],
    } as never);
  });
  state().setPage(id);
}

export function duplicatePage(): void {
  const { doc, pageId } = state();
  if (!doc) return;
  const copyId = freshId(doc, "page");
  state().edit("Duplicate page", (draft) => {
    const source = pageOf(draft, pageId);
    if (!source) return;
    const copy = clone(source);
    const seen = collectIds(draft);
    const rename = (nodes: SceneNode[]) => {
      for (const node of nodes) {
        let id = uid("n");
        while (seen.has(id)) id = uid("n");
        seen.add(id);
        node.id = id;
        rename(children(node));
      }
    };
    copy.id = copyId;
    copy.name = `${source.name ?? source.id} copy`;
    for (const layer of copy.layers ?? []) {
      layer.id = uid("layer");
      rename(layerNodes(layer));
    }
    draft.pages.splice(draft.pages.indexOf(source) + 1, 0, copy);
  });
  state().setPage(copyId);
}

export function renamePage(id: string, name: string): void {
  state().edit("Rename page", (draft) => {
    const page = draft.pages.find((p) => p.id === id);
    if (page) page.name = name;
  });
}

export function removePage(id: string): void {
  const { doc } = state();
  if (!doc || doc.pages.length <= 1) return;
  const index = doc.pages.findIndex((p) => p.id === id);
  state().edit("Remove page", (draft) => {
    draft.pages = draft.pages.filter((p) => p.id !== id);
  });
  state().setPage(doc.pages[index === 0 ? 1 : index - 1]!.id);
}

export function movePage(id: string, index: number): void {
  const { doc } = state();
  const from = doc?.pages.findIndex((p) => p.id === id) ?? -1;
  if (!doc || from < 0) return;
  const to = Math.max(0, Math.min(doc.pages.length - 1, index));
  if (to === from) return;
  state().edit("Move page", (draft) => {
    const [page] = draft.pages.splice(from, 1);
    draft.pages.splice(to, 0, page!);
  });
}

export function resizeCanvas(width: number, height: number, background?: string): void {
  const { pageId } = state();
  state().edit("Canvas", (draft) => {
    const page = pageOf(draft, pageId);
    if (!page) return;
    const canvas = { ...(page.canvas ?? {}), width: Math.max(1, Math.min(16384, Math.round(width))), height: Math.max(1, Math.min(16384, Math.round(height))), ...(background ? { background } : {}) };
    if (Array.isArray(draft.pages)) page.canvas = canvas as never;
    else draft.canvas = canvas;
  });
}

// ---- server-backed operations ----

export async function fileToBase64(file: Blob): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

/** Add a PNG/JPEG/WebP as an embedded image through the Rust image service. */
export async function addImage(file: Blob, label = "image"): Promise<void> {
  const { doc, pageId, layerId } = state();
  if (!doc) return;
  if (file.size > 32 * 1024 * 1024) {
    state().notify("Images are limited to 32 MiB.", true);
    return;
  }
  const layer = currentLayer(doc, pageId, layerId);
  if (!layer || layer.locked) {
    state().notify("Choose an unlocked layer for the image.", true);
    return;
  }
  const migrate = doc.version < 5;
  if (migrate && !window.confirm("Adding an image upgrades this document to format v5. Continue?")) return;
  try {
    const id = freshId(doc, "image");
    const result = await postJson<{ document: Doc }>("/api/image/add", {
      document: doc,
      id,
      page: pageOf(doc, pageId)?.id,
      layer: layer.id,
      data: await fileToBase64(file),
      migrate,
    });
    state().replaceDoc(result.document);
    state().select(id);
    state().notify(`Added ${label}. Save to keep it.`);
  } catch (error) {
    fail(error, "Could not add the image.");
  }
}

export async function runScene(operations: unknown[]): Promise<boolean> {
  const { doc, pageId } = state();
  if (!doc) return false;
  try {
    const result = await postJson<{ document: Doc; changes: unknown[] }>("/api/scene", {
      document: doc,
      page: pageId,
      operations,
    });
    state().replaceDoc(result.document);
    state().notify(`${result.changes.length} operation(s) applied.`);
    return true;
  } catch (error) {
    fail(error, "Scene operation failed.");
    return false;
  }
}

export async function importPen(
  source: unknown,
  options: { x: number; y: number; scale: number; rotation: number; prefix: string; expand: boolean },
): Promise<void> {
  const { doc, pageId } = state();
  if (!doc) return;
  try {
    const result = await postJson<{ document: Doc; summary: { layers: Record<string, string> } }>("/api/import", {
      document: doc,
      source,
      options: {
        destination_page: pageId,
        source_page: null,
        prefix: options.prefix || null,
        x: options.x,
        y: options.y,
        scale: options.scale,
        rotation: options.rotation,
        expand_canvas: options.expand,
      },
    });
    state().replaceDoc(result.document);
    state().notify(`${Object.keys(result.summary.layers).length} layer(s) imported.`);
  } catch (error) {
    fail(error, "Import failed.");
  }
}

export async function importDesignFile(
  file: File,
  prefix = "",
): Promise<FidelityReport | undefined> {
  const { doc, pageId } = state();
  if (!doc) return;
  if (file.size > 32 * 1024 * 1024) {
    state().notify("Design imports are limited to 32 MiB in Studio.", true);
    return;
  }
  const suffix = file.name.toLowerCase();
  let format: "svg" | "pdf" | "psd" | "sketch" | "figma";
  if (suffix.endsWith(".svg")) format = "svg";
  else if (suffix.endsWith(".pdf")) format = "pdf";
  else if (suffix.endsWith(".psd")) format = "psd";
  else if (suffix.endsWith(".sketch")) format = "sketch";
  else if (suffix.endsWith(".fig")) format = "figma";
  else if (suffix.endsWith(".figma.json") || suffix.endsWith(".json")) {
    let exported: unknown;
    try {
      exported = JSON.parse(await file.text());
    } catch {
      state().notify("This JSON file is not a readable Figma export.", true);
      return;
    }
    const document = (exported as { document?: { type?: string } } | null)?.document;
    const rootType = (exported as { type?: string } | null)?.type;
    if (document?.type !== "DOCUMENT" && rootType !== "DOCUMENT") {
      state().notify("Choose a Figma REST JSON export (DOCUMENT tree).", true);
      return;
    }
    format = "figma";
  } else {
    state().notify("Studio supports SVG, PDF, PSD, Sketch archives, Figma .fig v15 files, and Figma JSON imports.", true);
    return;
  }
  try {
    const result = await postJson<{ document: Doc; summary: unknown; report: FidelityReport }>("/api/import", {
      document: doc,
      source: await fileToBase64(file),
      source_name: file.name,
      format,
      options: {
        destination_page: pageId,
        source_page: null,
        prefix: prefix || null,
        x: 0,
        y: 0,
        scale: 1,
        rotation: 0,
        expand_canvas: false,
      },
    });
    state().replaceDoc(result.document);
    const { exact, approximated, dropped } = result.report.counts;
    state().notify(`Imported ${file.name}: ${exact} exact, ${approximated} approximated, ${dropped} dropped.`);
    return result.report;
  } catch (error) {
    fail(error, "Could not import this design.");
    return;
  }
}

export async function embedFont(file: File): Promise<void> {
  const { doc } = state();
  if (!doc) return;
  if (file.size > 6 * 1024 * 1024) {
    state().notify("Font file is too large (6 MiB limit).", true);
    return;
  }
  try {
    const result = await postJson<{ document: Doc }>("/api/font", {
      document: doc,
      id: freshId(doc, "font"),
      data: await fileToBase64(file),
    });
    state().replaceDoc(result.document);
    state().notify("Font embedded in the document.");
  } catch (error) {
    fail(error, "Could not embed the font.");
  }
}

export async function insertAsset(spec: string): Promise<void> {
  const { doc, pageId } = state();
  if (!doc) return;
  try {
    const source = await api.asset(spec);
    const result = await postJson<{ document: Doc }>("/api/import", {
      document: doc,
      source: source.document,
      options: {
        destination_page: pageId,
        source_page: (source.document as { asset?: { entry_page?: string } }).asset?.entry_page ?? null,
        prefix: `asset-${Date.now().toString(36)}`,
        x: 0,
        y: 0,
        scale: 1,
        rotation: 0,
        expand_canvas: false,
      },
    });
    state().replaceDoc(result.document);
    state().notify(`${source.asset.name} added as a copy.`);
  } catch (error) {
    fail(error, "Could not add the asset.");
  }
}

export async function exportAs(kind: "png" | "svg"): Promise<void> {
  const { doc, pageId } = state();
  if (!doc) return;
  try {
    const q = new URLSearchParams();
    if (pageId && isScene(doc)) q.set("page", pageId);
    const r = await fetch(`/api/render/${kind}?${q}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(doc),
    });
    if (!r.ok) throw new Error(((await r.json()) as { error?: string }).error ?? "Export failed.");
    const blob = await r.blob();
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `${String(doc.name ?? "artwork")}-${pageId ?? "page"}.${kind}`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    state().notify(`${kind.toUpperCase()} exported.`);
  } catch (error) {
    fail(error, "Export failed.");
  }
}

export function nodeLabel(node: SceneNode, layer?: Parameters<typeof nodeKind>[1]): string {
  const kind = nodeKind(node, layer);
  return `${kind} ${typeof node.name === "string" ? node.name : node.id}`;
}

/** Move a node by a page-space delta, in whatever representation it uses. */
export function moveNodeBy(id: string, dx: number, dy: number): void {
  updateNode(id, (node, f, scene) => {
    const kind = nodeKind(node, f.layer);
    if (!scene && !Array.isArray(node.transform) && kind === "path" && typeof node.d === "string") {
      node.d = affinePath(node.d, [1, 0, 0, 1, dx, dy]);
      return;
    }
    if (!scene && kind === "text" && !Array.isArray(node.transform)) {
      node.x = Number(node.x ?? 0) + dx;
      node.y = Number(node.y ?? 0) + dy;
      return;
    }
    const inv = invert(f.world);
    const [ox, oy] = inv ? apply(inv, 0, 0) : [0, 0];
    const [px, py] = inv ? apply(inv, dx, dy) : [dx, dy];
    const m = nodeMatrix(node);
    node.transform = [m[0], m[1], m[2], m[3], m[4] + (px - ox), m[5] + (py - oy)];
  });
}

/** Apply a page-space matrix (scale, rotate, translate) to one node. */
export function transformNodePage(id: string, page: Matrix): void {
  updateNode(id, (node, f, scene) => {
    const kind = nodeKind(node, f.layer);
    const plain = page[1] === 0 && page[2] === 0;
    if (!scene && !Array.isArray(node.transform) && plain) {
      if (kind === "path" && typeof node.d === "string") {
        node.d = affinePath(node.d, page);
        return;
      }
      if (kind === "text") {
        node.x = Number(node.x ?? 0) * page[0] + page[4];
        node.y = Number(node.y ?? 0) * page[3] + page[5];
        node.font_size = Math.max(1, Number(node.font_size ?? 16) * Math.abs(page[3]));
        return;
      }
    }
    const inv = invert(f.world);
    if (!inv) return;
    node.transform = multiply(multiply(inv, multiply(page, f.world)), nodeMatrix(node));
  });
}
