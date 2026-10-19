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
  pageBounds as pageBoundsOf,
  pageOf,
  uid,
  unite,
  type Found,
  type Tool,
} from "@/lib/model";
import { LATEST_FORMAT, useStudio } from "@/store";
import { afterClipGroup, clipBaseOf, clipMembers, mergeResources, needsComposite, referencedResources, release, remapReferences, settleClipping, COMPOSITE_VERSION, type Resources } from "@/lib/layers";

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
  if (selectionRoots().length > 1) return duplicateSelectionBy(16, 16);
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
    const at = found.siblings.indexOf(found.node);
    if (copy.kind === "adjustment") {
      // Adjustments have no position; the copy sits directly above the original.
      found.siblings.splice(at + 1, 0, copy);
      return;
    }
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
    // A copy of a clipping base goes above its clipped layers, so the group stays intact.
    found.siblings.splice(clipBaseOf(copy) ? at + 1 : afterClipGroup(found.siblings, at), 0, copy);
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
  const ids = selectionRoots();
  // Grouping keeps stacking order, so every member must share one parent list.
  const page = pageOf(doc, pageId);
  const lists = new Set(ids.map((x) => findNode(page, x)?.siblings));
  if (lists.size > 1) {
    state().notify("Objects in different layers or groups cannot be grouped together. Select siblings.", true);
    return;
  }
  state().edit("Group", (draft) => {
    const found = ids.map((x) => findNode(pageOf(draft, pageId), x)).filter((f): f is Found => !!f && !f.layer.locked);
    const list = found[0]?.siblings;
    if (!list) return;
    const members = list.filter((n) => found.some((f) => f.node === n));
    const top = list.indexOf(members.at(-1)!);
    list.splice(top, 1, { kind: "group", id, transform: [1, 0, 0, 1, 0, 0], children: members });
    for (const m of members.slice(0, -1)) list.splice(list.indexOf(m), 1);
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

/**
 * Move an object above or below another one in the layers list, into a group,
 * or to the top of a layer. Scene objects may change layer or group; their
 * parent transforms are baked in so nothing moves on the page. Legacy objects
 * only reorder within their own list.
 */
/** Reorder a node. With `clip` (Alt-drop) it also joins the clipping group it lands in. */
export function moveInStack(id: string, target: { node: string; where: "above" | "below" | "into" } | { layer: string }, clip = false): void {
  const { doc, pageId } = state();
  const page = pageOf(doc, pageId);
  const from = findNode(page, id);
  if (!doc || !page || !from) return;
  const scene = isScene(doc);
  const to = "node" in target ? findNode(page, target.node) : null;
  const layer = "layer" in target ? page.layers?.find((l) => l.id === target.layer) : to?.layer;
  if (!layer || ("node" in target && !to) || (to && to.node === from.node)) return;
  if (from.layer.locked || layer.locked) {
    state().notify("That layer is locked. Unlock it to rearrange its objects.", true);
    return;
  }
  // A group cannot go inside itself.
  for (let f = to; f?.parent; f = findNode(page, f.parent.id)) {
    if (f.parent === from.node) return;
  }
  if (to && "node" in target && target.where === "into" && to.node.kind !== "group") return;
  const sameList = to && "node" in target && target.where !== "into" ? to.siblings === from.siblings : false;
  if (!scene && !sameList) {
    state().notify("Legacy (v1-v3) objects can only be reordered within their layer. Upgrade the format to move them between layers.", true);
    return;
  }
  state().edit("Reorder", (draft) => {
    const p = pageOf(draft, pageId);
    const src = findNode(p, id);
    if (!src) return;
    const dst = "node" in target ? findNode(p, target.node) : null;
    const dstLayer = "layer" in target ? p?.layers?.find((l) => l.id === target.layer) : dst?.layer;
    if (!dstLayer) return;
    const into = dst && "node" in target && target.where === "into";
    const list = into ? children(dst.node) : dst ? dst.siblings : (dstLayer.nodes ?? []);
    const world = into ? multiply(dst.world, nodeMatrix(dst.node)) : dst ? dst.world : ([1, 0, 0, 1, 0, 0] as Matrix);
    const members = new Set([...clipMembers(src.siblings), ...clipMembers(list)]);
    src.siblings.splice(src.siblings.indexOf(src.node), 1);
    // Keep the painted position when the parent transform changes.
    if (scene && (src.parent || into || dst?.parent)) {
      const back = invert(world);
      if (back) src.node.transform = multiply(back, multiply(src.world, nodeMatrix(src.node)));
    }
    if (!dst || into) list.push(src.node);
    else list.splice(list.indexOf(dst.node) + ("node" in target && target.where === "above" ? 1 : 0), 0, src.node);
    if (into && !Array.isArray(dst.node.children)) dst.node.children = list;
    if (!dst && !Array.isArray(dstLayer.nodes)) dstLayer.nodes = list;
    if (!scene) return;
    // Like Photoshop: dropping inside a clipping group joins it, dragging out releases.
    const at = list.indexOf(src.node);
    const below = list[at - 1];
    const above = list[at + 1];
    const stays = members.has(id) && !!below && (members.has(below.id) || below.id === clipBaseOf(src.node));
    const join = (clip && draft.version >= COMPOSITE_VERSION) || stays || (above && members.has(above.id));
    if (join) members.add(id);
    else if (members.delete(id)) release(src.node);
    settleClipping(list, members);
    if (src.siblings !== list) settleClipping(src.siblings, members);
  });
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

function moveFound(node: SceneNode, f: Found, scene: boolean, dx: number, dy: number): void {
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
}

function transformFound(node: SceneNode, f: Found, scene: boolean, page: Matrix): void {
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
}

/** Selected IDs without any object whose ancestor group is also selected (so nothing moves twice). */
export function selectionRoots(): string[] {
  const { doc, pageId, selectedId, selectedIds } = state();
  const ids = selectedIds.length ? selectedIds : selectedId ? [selectedId] : [];
  const page = pageOf(doc, pageId);
  const inside = new Set<string>();
  for (const id of ids) {
    const found = findNode(page, id);
    if (found) collectChildIds(found.node, inside);
  }
  return ids.filter((id) => !inside.has(id));
}

function collectChildIds(node: SceneNode, into: Set<string>): void {
  for (const kid of children(node)) {
    into.add(kid.id);
    collectChildIds(kid, into);
  }
}

/** Move a node by a page-space delta, in whatever representation it uses. */
export function moveNodeBy(id: string, dx: number, dy: number): void {
  moveNodesBy([id], dx, dy);
}

/** Move several nodes by one page-space delta as a single undo step. */
export function moveNodesBy(ids: string[], dx: number, dy: number): void {
  const { pageId } = state();
  state().edit("Move", (draft) => {
    const scene = isScene(draft);
    for (const id of ids) {
      const found = findNode(pageOf(draft, pageId), id);
      if (found && !found.layer.locked) moveFound(found.node, found, scene, dx, dy);
    }
  });
}

/** Apply a page-space matrix (scale, rotate, translate) to one node. */
export function transformNodePage(id: string, page: Matrix): void {
  transformNodesPage([id], page);
}

/** Apply one page-space matrix to several nodes as a single undo step. */
export function transformNodesPage(ids: string[], page: Matrix): void {
  const { pageId } = state();
  state().edit("Transform", (draft) => {
    const scene = isScene(draft);
    for (const id of ids) {
      const found = findNode(pageOf(draft, pageId), id);
      if (found && !found.layer.locked) transformFound(found.node, found, scene, page);
    }
  });
}

/** Copy the selection in place, move the copies by a delta, and select them (Alt+drag). */
export function duplicateSelectionBy(dx: number, dy: number): void {
  const { doc, pageId } = state();
  const ids = selectionRoots();
  if (!doc || !ids.length) return;
  const made: string[] = [];
  state().edit("Duplicate", (draft) => {
    const seen = collectIds(draft);
    const scene = isScene(draft);
    for (const id of ids) {
      const found = findNode(pageOf(draft, pageId), id);
      if (!found || found.layer.locked) continue;
      const copy = clone(found.node);
      const reassign = (node: SceneNode) => {
        let next = uid("copy");
        while (seen.has(next)) next = uid("copy");
        seen.add(next);
        node.id = next;
        children(node).forEach(reassign);
      };
      reassign(copy);
      found.siblings.splice(found.siblings.indexOf(found.node) + 1, 0, copy);
      moveFound(copy, { ...found, node: copy }, scene, dx, dy);
      made.push(copy.id);
    }
  });
  state().selectMany(made);
}

/** Prefix that marks Studio objects on the system clipboard. */
export const CLIPBOARD_MARK = "pentool-objects:";
type ClipEntry = { node: SceneNode; bucket: "nodes" | "paths" | "texts" };
type ClipData = { scene: boolean; version?: number; entries: ClipEntry[]; resources?: Resources };
let pasteCount = 0;

/**
 * Clipboard text for the selection: the root objects with their parent group
 * transforms baked in, so a paste lands where the copy was, plus the images,
 * masks and tiles they reference so they paste into another document intact.
 */
export function copySelectionText(): string | null {
  const { doc, pageId } = state();
  const page = pageOf(doc, pageId);
  const entries: ClipEntry[] = [];
  for (const id of selectionRoots()) {
    const found = findNode(page, id);
    if (!found) continue;
    const node = clone(found.node);
    if (found.parent && node.kind !== "adjustment") node.transform = multiply(found.world, nodeMatrix(found.node));
    entries.push({ node, bucket: !isScene(doc) && nodeKind(found.node, found.layer) === "text" ? "texts" : !isScene(doc) ? "paths" : "nodes" });
  }
  if (!entries.length || !doc) return null;
  pasteCount = 0;
  const data: ClipData = { scene: isScene(doc), version: doc.version, entries, resources: referencedResources(doc, entries.map((e) => e.node)) };
  return CLIPBOARD_MARK + JSON.stringify(data);
}

/**
 * Add clipboard objects with fresh IDs and select them. Adjustment layers go
 * directly above the selected layer, like Photoshop; other objects go on top
 * of the current layer, stepped so repeated pastes stay visible.
 */
export function pasteSelectionText(text: string): boolean {
  if (!text.startsWith(CLIPBOARD_MARK)) return false;
  const { doc } = state();
  if (!doc) return true;
  let data: ClipData;
  try {
    data = JSON.parse(text.slice(CLIPBOARD_MARK.length));
  } catch {
    state().notify("The clipboard holds damaged Studio objects.", true);
    return true;
  }
  if (data.scene !== isScene(doc)) {
    state().notify("Objects copied from a legacy (v1-v3) document cannot be pasted into a scene document, or the reverse.", true);
    return true;
  }
  const nodes = data.entries.map((e) => e.node);
  if (isScene(doc) && doc.version < COMPOSITE_VERSION && needsComposite(nodes)) {
    // Adjustment layers, masks and blend modes need the compositing format.
    void state().upgradeDoc(Math.max(COMPOSITE_VERSION, Math.min(data.version ?? COMPOSITE_VERSION, LATEST_FORMAT))).then(() => {
      if ((state().doc?.version ?? 0) >= COMPOSITE_VERSION) pasteData(data);
    });
    return true;
  }
  pasteData(data);
  return true;
}

function pasteData(data: ClipData): void {
  const { doc, pageId, layerId, selectedId } = state();
  if (!doc) return;
  const anchor = findNode(pageOf(doc, pageId), selectedId);
  const layer = anchor?.layer ?? currentLayer(doc, pageId, layerId);
  if (!layer || layer.locked || layer.visible === false) {
    state().notify("Choose a visible, unlocked layer to paste into.", true);
    return;
  }
  const inPlace = data.entries.every((e) => e.node.kind === "adjustment");
  // Each repeated paste of the same copy steps further, so copies stay visible.
  const offset = inPlace ? 0 : 16 * ++pasteCount;
  const made: string[] = [];
  state().edit("Paste", (draft) => {
    mergeResources(draft, data.resources);
    const seen = collectIds(draft);
    const scene = isScene(draft);
    const renamed = new Map<string, string>();
    const copies = data.entries.map(({ node, bucket }) => {
      const copy = clone(node);
      const reassign = (n: SceneNode) => {
        let next = uid("copy");
        while (seen.has(next)) next = uid("copy");
        seen.add(next);
        renamed.set(n.id, next);
        n.id = next;
        children(n).forEach(reassign);
      };
      reassign(copy);
      return { copy, bucket };
    });
    remapReferences(copies.map((c) => c.copy), renamed);
    // Scene pastes land above the selected layer (and its clipping group). Positioned
    // objects only do so at the top level, where their page transform still applies.
    const found = scene ? findNode(pageOf(draft, pageId), selectedId) : null;
    const above = found && (inPlace || !found.parent) ? found : null;
    let at = above ? afterClipGroup(above.siblings, above.siblings.indexOf(above.node)) : 0;
    const target = above?.layer ?? pageOf(draft, pageId)?.layers?.find((l) => l.id === layer.id) ?? currentLayer(draft, pageId, layerId)!;
    for (const { copy, bucket } of copies) {
      if (above) above.siblings.splice(at++, 0, copy);
      else addToLayer(target, copy, bucket);
      if (offset && copy.kind !== "adjustment") {
        const list = above?.siblings ?? (Array.isArray(target.nodes) ? target.nodes : target[bucket === "texts" ? "texts" : "paths"]!);
        moveFound(copy, { node: copy, layer: target, siblings: list, parent: null, world: [1, 0, 0, 1, 0, 0] }, scene, offset, offset);
      }
      made.push(copy.id);
    }
  });
  state().selectMany(made);
}

export type Align = "left" | "hcenter" | "right" | "top" | "vcenter" | "bottom" | "hspace" | "vspace";

/** Align to the selection's bounds (or the page for one object); distribute needs three. */
export function alignSelected(how: Align): void {
  const { doc, pageId } = state();
  const page = pageOf(doc, pageId);
  const items = selectionRoots()
    .map((id) => findNode(page, id))
    .map((f) => (f ? { id: f.node.id, box: pageBoundsOf(f) } : null))
    .filter((x): x is { id: string; box: NonNullable<ReturnType<typeof pageBoundsOf>> } => !!x?.box);
  if (!page || !items.length) return;
  const vertical = how === "top" || how === "vcenter" || how === "bottom" || how === "vspace";
  const deltas = new Map<string, number>();
  if (how === "hspace" || how === "vspace") {
    if (items.length < 3) return state().notify("Distribute needs three or more objects.", true);
    const pos = (b: (typeof items)[number]["box"]) => (vertical ? b.y : b.x);
    const size = (b: (typeof items)[number]["box"]) => (vertical ? b.height : b.width);
    const sorted = [...items].sort((a, b) => pos(a.box) - pos(b.box));
    const first = sorted[0]!.box;
    const last = sorted.at(-1)!.box;
    const total = sorted.reduce((sum, item) => sum + size(item.box), 0);
    const gap = (pos(last) + size(last) - pos(first) - total) / (sorted.length - 1);
    let at = pos(first);
    for (const item of sorted) {
      deltas.set(item.id, at - pos(item.box));
      at += size(item.box) + gap;
    }
  } else {
    const area = items.length > 1
      ? items.reduce((u, i) => unite(u, i.box), null as ReturnType<typeof unite> | null)!
      : { x: 0, y: 0, width: Number(page.canvas?.width ?? 0), height: Number(page.canvas?.height ?? 0) };
    for (const { id, box } of items) {
      const d = {
        left: area.x - box.x,
        hcenter: area.x + area.width / 2 - (box.x + box.width / 2),
        right: area.x + area.width - (box.x + box.width),
        top: area.y - box.y,
        vcenter: area.y + area.height / 2 - (box.y + box.height / 2),
        bottom: area.y + area.height - (box.y + box.height),
      }[how];
      deltas.set(id, d);
    }
  }
  state().edit("Align", (draft) => {
    const scene = isScene(draft);
    for (const [id, d] of deltas) {
      const found = findNode(pageOf(draft, pageId), id);
      if (found && !found.layer.locked && d) moveFound(found.node, found, scene, vertical ? 0 : d, vertical ? d : 0);
    }
  });
}

/** Select every object on the visible, unlocked layers of the current page. */
export function selectAllOnPage(): void {
  const { doc, pageId } = state();
  const ids = (pageOf(doc, pageId)?.layers ?? [])
    .filter((l) => l.visible !== false && !l.locked)
    .flatMap((l) => (Array.isArray(l.nodes) ? l.nodes : [...(l.paths ?? []), ...(l.texts ?? [])]).map((n) => n.id));
  state().selectMany(ids);
}

/** View commands the canvas registers while it is mounted, for menus and the command palette. */
export const canvasView: { fit?: () => void; actual?: () => void; selection?: () => void; zoomBy?: (factor: number) => void } = {};
