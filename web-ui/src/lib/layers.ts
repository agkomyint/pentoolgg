// Photoshop-style layer behaviour for Studio: adjustment layers, clipping,
// blend/opacity/fill and masks. Everything here edits the existing v6
// compositing fields; the Rust server stays the validator on render and save.
import type { Doc, SceneNode } from "@/lib/api";
import { collectIds, findNode, pageOf, uid } from "@/lib/model";
import { useStudio } from "@/store";

/** First `.pen` version with adjustment layers, masks, clipping and blend modes. */
export const COMPOSITE_VERSION = 6;

export type AdjustmentSpec = { kind: string; label: string; params: () => Record<string, unknown> };

/** The 13 engine adjustments in Photoshop's menu order, each with neutral, valid params. */
export const ADJUSTMENTS: AdjustmentSpec[] = [
  { kind: "brightness-contrast", label: "Brightness/Contrast", params: () => ({ brightness: 0, contrast: 0 }) },
  { kind: "levels", label: "Levels", params: () => ({ black: 0, white: 255, gamma: 1 }) },
  { kind: "curves", label: "Curves", params: () => ({ points: [[0, 0], [255, 255]] }) },
  { kind: "exposure", label: "Exposure", params: () => ({ stops: 0 }) },
  { kind: "vibrance", label: "Vibrance", params: () => ({ amount: 0 }) },
  { kind: "hue-saturation", label: "Hue/Saturation", params: () => ({ hue: 0, saturation: 0 }) },
  { kind: "color-balance", label: "Color Balance", params: () => ({ red: 0, green: 0, blue: 0 }) },
  { kind: "black-and-white", label: "Black & White", params: () => ({ red: 0.2126, green: 0.7152, blue: 0.0722 }) },
  { kind: "channel-mixer", label: "Channel Mixer", params: () => ({ matrix: [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0]] }) },
  { kind: "invert", label: "Invert", params: () => ({}) },
  { kind: "posterize", label: "Posterize", params: () => ({ levels: 4 }) },
  { kind: "threshold", label: "Threshold", params: () => ({ level: 128 }) },
  { kind: "gradient-map", label: "Gradient Map", params: () => ({ stops: [{ position: 0, rgb: [0, 0, 0] }, { position: 1, rgb: [255, 255, 255] }] }) },
];

export const BLEND_MODES = [
  "normal", "multiply", "screen", "overlay", "darken", "lighten", "color-dodge", "color-burn",
  "hard-light", "soft-light", "difference", "exclusion", "hue", "saturation", "color", "luminosity",
];

/** Node kinds that can carry `content_opacity` (Photoshop's Fill). */
const NO_FILL = new Set(["adjustment", "group"]);

export const adjustmentLabel = (kind: unknown): string =>
  ADJUSTMENTS.find((a) => a.kind === kind)?.label ?? String(kind ?? "Adjustment");

export const isAdjustment = (node: SceneNode | undefined): boolean => node?.kind === "adjustment";

/** True when the node can show a Fill control (content opacity). */
export const hasFill = (node: SceneNode): boolean => !NO_FILL.has(String(node.kind));

/** An adjustment that affects only one layer: Photoshop's clipped adjustment. */
export function adjustmentBase(node: SceneNode): string | null {
  const scope = node.scope as { kind?: string; ids?: unknown[] } | undefined;
  return node.kind === "adjustment" && scope?.kind === "targets" && scope.ids?.length === 1 && typeof scope.ids[0] === "string"
    ? scope.ids[0]
    : null;
}

/** The base layer a node is clipped to, for content and adjustments alike. */
export function clipBaseOf(node: SceneNode): string | null {
  if (node.kind === "adjustment") return adjustmentBase(node);
  const base = (node.clipping as { base?: unknown } | undefined)?.base;
  return typeof base === "string" ? base : null;
}

/**
 * The layer a node at `index` would clip to: the nearest content layer below it,
 * passing over layers already clipped to that same base.
 */
export function clipTarget(siblings: SceneNode[], index: number): string | null {
  const below = siblings[index - 1];
  if (!below) return null;
  const base = clipBaseOf(below);
  if (base) return base;
  return below.kind === "adjustment" ? null : below.id;
}

/**
 * Clip or release `id` (Alt-click between layers). Releasing a layer also releases
 * the clipped layers above it, and clipping a base pulls its own clipped layers
 * into the new group, as in Photoshop. Returns an error message or null.
 */
export function toggleClipIn(siblings: SceneNode[], id: string): string | null {
  const index = siblings.findIndex((n) => n.id === id);
  const node = siblings[index];
  if (!node) return "That layer is not in this stack.";
  const clipped = clipBaseOf(node) !== null;
  if (clipped) {
    for (let i = index; i < siblings.length && clipBaseOf(siblings[i]!) !== null; i++) release(siblings[i]!);
    return null;
  }
  const base = clipTarget(siblings, index);
  if (!base) return index === 0 ? "There is no layer below to clip to." : "Adjustment layers cannot be a clipping base. Move the layer above a picture or shape.";
  if (node.kind === "group" && node.isolation === "pass-through") return "Pass-through groups cannot be clipped. Set the group to Normal first.";
  attach(node, base);
  // Layers that were clipped to this node now belong to the new base.
  for (let i = index + 1; i < siblings.length; i++) {
    const above = siblings[i]!;
    if (clipBaseOf(above) !== node.id) break;
    attach(above, base);
  }
  return null;
}

function attach(node: SceneNode, base: string): void {
  if (node.kind === "adjustment") node.scope = { kind: "targets", ids: [base] };
  else node.clipping = { base };
}

export function release(node: SceneNode): void {
  if (node.kind === "adjustment") node.scope = { kind: "below" };
  else delete node.clipping;
}

/**
 * IDs of layers that belong to a clipping group: clipped to the base the engine
 * would see at their position. Adjustments scoped to some other layer are not members.
 */
export function clipMembers(siblings: SceneNode[]): Set<string> {
  const members = new Set<string>();
  let base: string | null = null;
  for (const node of siblings) {
    const to = clipBaseOf(node);
    if (to !== null && to === base) members.add(node.id);
    else base = node.kind === "adjustment" ? null : node.id;
  }
  return members;
}

/**
 * After a reorder, point every member at the base now below it, or release it
 * when there is none, so the stack stays valid. Photoshop does the same when a
 * base or a clipped layer is dragged elsewhere.
 */
export function settleClipping(siblings: SceneNode[], members: Set<string>): void {
  let base: string | null = null;
  for (const node of siblings) {
    if (members.has(node.id) && base && !(node.kind === "group" && node.isolation === "pass-through")) {
      attach(node, base);
      continue;
    }
    if (members.has(node.id)) release(node);
    if (clipBaseOf(node) !== null && clipBaseOf(node) === base) continue;
    base = node.kind === "adjustment" ? null : node.id;
  }
}

/** Index after `index` and any layers clipped to it, so an insert never splits a clipping group. */
export function afterClipGroup(siblings: SceneNode[], index: number): number {
  const node = siblings[index];
  const base = node ? clipBaseOf(node) ?? node.id : null;
  let at = index + 1;
  while (at < siblings.length && base && clipBaseOf(siblings[at]!) === base) at++;
  return at;
}

/** Next free "Levels 1", "Levels 2"... name on the page. */
export function nextName(doc: Doc, label: string): string {
  const names = new Set<string>();
  const visit = (nodes: SceneNode[] | undefined) =>
    nodes?.forEach((n) => {
      if (typeof n.name === "string") names.add(n.name);
      if (Array.isArray(n.children)) visit(n.children);
    });
  for (const page of doc.pages ?? []) for (const layer of page.layers ?? []) visit(layer.nodes);
  let n = 1;
  while (names.has(`${label} ${n}`)) n++;
  return `${label} ${n}`;
}

export function makeAdjustment(doc: Doc, kind: string): SceneNode {
  const spec = ADJUSTMENTS.find((a) => a.kind === kind);
  if (!spec) throw new Error(`Unknown adjustment ${kind}.`);
  const ids = collectIds(doc);
  let id = uid("adjust");
  while (ids.has(id)) id = uid("adjust");
  return {
    kind: "adjustment",
    id,
    name: nextName(doc, spec.label),
    adjustment: kind,
    params: spec.params(),
    scope: { kind: "below" },
    enabled: true,
    opacity: 1,
    blend_mode: "normal",
    blend_space: "srgb",
  };
}

const state = () => useStudio.getState();

/** Upgrade the open document to the compositing format once, on first use. Undo reverts. */
export async function ensureComposite(): Promise<boolean> {
  const { doc } = state();
  if (!doc) return false;
  if (doc.version >= COMPOSITE_VERSION) return true;
  if (doc.version < 4) {
    state().notify("Adjustment layers need a scene document. Upgrade this legacy file from the File menu first.", true);
    return false;
  }
  await state().upgradeDoc(COMPOSITE_VERSION);
  return (state().doc?.version ?? 0) >= COMPOSITE_VERSION;
}

/**
 * Photoshop's "New adjustment layer": insert directly above the selected layer
 * (above its clipping group) or at the top of the current layer, then select it.
 */
export async function addAdjustment(kind: string): Promise<void> {
  if (!(await ensureComposite())) return;
  const { doc, pageId, layerId, selectedId } = state();
  if (!doc) return;
  const page = pageOf(doc, pageId);
  const anchor = findNode(page, selectedId);
  const layer = anchor?.layer ?? page?.layers?.find((l) => l.id === layerId) ?? page?.layers?.[0];
  if (!layer || layer.locked) return state().notify("Choose an unlocked layer for the adjustment.", true);
  const node = makeAdjustment(doc, kind);
  state().edit("New adjustment layer", (draft, { pageId }) => {
    const found = findNode(pageOf(draft, pageId), selectedId);
    if (found) {
      found.siblings.splice(afterClipGroup(found.siblings, found.siblings.indexOf(found.node)), 0, node);
      return;
    }
    const target = pageOf(draft, pageId)?.layers?.find((l) => l.id === layer.id);
    (target!.nodes ??= []).push(node);
  });
  state().select(node.id);
}

/** Alt-click: clip the layer to the one below it, or release it. */
export function toggleClip(id: string): void {
  const { doc, pageId } = state();
  const found = findNode(pageOf(doc, pageId), id);
  if (!found) return;
  if (found.layer.locked) return state().notify("Unlock the layer first.", true);
  const problem = toggleClipIn(structuredClone(found.siblings), id);
  if (problem) return state().notify(problem, true);
  void (async () => {
    if (!(await ensureComposite())) return;
    state().edit("Clipping mask", (draft, { pageId }) => {
      const again = findNode(pageOf(draft, pageId), id);
      if (again) toggleClipIn(again.siblings, id);
    });
  })();
}

/** Set (or with `undefined` remove) compositing fields on one node as one undo step. */
export function setComposite(id: string, patch: Record<string, unknown>, label = "Layer properties"): void {
  void (async () => {
    if (!(await ensureComposite())) return;
    state().edit(label, (draft, { pageId }) => {
      const found = findNode(pageOf(draft, pageId), id);
      if (!found || found.layer.locked) return;
      for (const [key, value] of Object.entries(patch)) {
        if (value === undefined) delete found.node[key];
        else found.node[key] = value;
      }
    });
  })();
}

/** Live preview of a compositing change while a control is dragged. */
export function previewComposite(id: string, patch: Record<string, unknown>): void {
  const { doc, pageId } = state();
  if (!doc || doc.version < COMPOSITE_VERSION) return;
  const next = structuredClone(doc);
  const found = findNode(pageOf(next, pageId), id);
  if (!found) return;
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) delete found.node[key];
    else found.node[key] = value;
  }
  state().setPreview(next);
}

export function toggleVisible(id: string): void {
  const { doc, pageId } = state();
  const node = findNode(pageOf(doc, pageId), id)?.node;
  if (!node) return;
  state().edit(node.visible === false ? "Show layer" : "Hide layer", (draft, { pageId }) => {
    const found = findNode(pageOf(draft, pageId), id);
    if (!found || found.layer.locked) return;
    if (found.node.visible === false) delete found.node.visible;
    else found.node.visible = false;
  });
}

/**
 * Photoshop's "Create clipping mask from shape": snapshot a vector shape as a
 * named mask and attach it to `target`. The shape stays on the canvas.
 */
export async function maskWithShape(target: string, shape: string): Promise<void> {
  if (!(await ensureComposite())) return;
  const { doc } = state();
  if (!doc) return;
  const masks = (doc as { masks?: Record<string, string> }).masks ?? {};
  let name = `mask-${target}`;
  for (let n = 2; name in masks; n++) name = `mask-${target}-${n}`;
  const { runScene } = await import("@/lib/actions");
  const done = await runScene([
    { type: "mask-create", name, from: `vector:${shape}` },
    { type: "mask-attach", id: target, name },
  ]);
  if (done) state().select(target);
}

/**
 * Resource tables keyed by content digest. Copied nodes carry the entries they
 * reference so a paste into another document renders the same pixels.
 */
const RESOURCE_TABLES = ["image_assets", "mask_resources", "raster_tiles"] as const;
export type Resources = Partial<Record<(typeof RESOURCE_TABLES)[number], Record<string, unknown>>>;

/** Every resource entry the values reference, following masks to their image assets. */
export function referencedResources(doc: Doc, values: unknown[]): Resources {
  const out: Resources = {};
  const tables = doc as Record<string, unknown>;
  const pending = [...values];
  const seen = new Set<string>();
  while (pending.length) {
    const value = pending.pop();
    if (typeof value === "string") {
      if (seen.has(value)) continue;
      seen.add(value);
      for (const name of RESOURCE_TABLES) {
        const table = tables[name] as Record<string, unknown> | undefined;
        if (table && Object.hasOwn(table, value)) {
          (out[name] ??= {})[value] = table[value];
          pending.push(table[value]);
        }
      }
    } else if (Array.isArray(value)) pending.push(...value);
    else if (value && typeof value === "object") pending.push(...Object.values(value));
  }
  return out;
}

/** Add missing resource entries; digests are content-addressed, so existing keys already match. */
export function mergeResources(doc: Doc, resources: Resources | undefined): void {
  const tables = doc as Record<string, unknown>;
  for (const [name, entries] of Object.entries(resources ?? {})) {
    const table = ((tables[name] as Record<string, unknown> | undefined) ??= {});
    for (const [key, value] of Object.entries(entries ?? {})) if (!Object.hasOwn(table, key)) table[key] = value;
    tables[name] = table;
  }
}

const COMPOSITE_KEYS = ["mask", "clipping", "blend_mode", "blend_space", "content_opacity", "effects", "transforms", "isolation"];

/** True when pasted nodes use compositing fields that a v4/v5 document cannot hold. */
export function needsComposite(nodes: SceneNode[]): boolean {
  const visit = (n: SceneNode): boolean =>
    n.kind === "adjustment" || n.kind === "fill" || COMPOSITE_KEYS.some((k) => n[k] !== undefined) || (Array.isArray(n.children) && n.children.some(visit));
  return nodes.some(visit);
}

/**
 * After new IDs are assigned to pasted nodes, point clipping and adjustment
 * scopes at the copies, and release references to layers that were not copied.
 */
export function remapReferences(nodes: SceneNode[], ids: Map<string, string>): void {
  for (const node of nodes) {
    const base = clipBaseOf(node);
    if (base !== null) {
      const next = ids.get(base);
      if (next) attach(node, next);
      else release(node);
    } else if (node.kind === "adjustment") {
      const scope = node.scope as { kind?: string; id?: string; ids?: string[] } | undefined;
      if (scope?.kind === "group") node.scope = ids.has(scope.id ?? "") ? { kind: "group", id: ids.get(scope.id!) } : { kind: "below" };
      if (scope?.kind === "targets") {
        const kept = (scope.ids ?? []).flatMap((t) => (ids.has(t) ? [ids.get(t)!] : []));
        node.scope = kept.length ? { kind: "targets", ids: kept } : { kind: "below" };
      }
    }
    if (Array.isArray(node.children)) remapReferences(node.children, ids);
  }
}
