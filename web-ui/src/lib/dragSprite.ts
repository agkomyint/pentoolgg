import type { Doc, SceneNode } from "@/lib/api";
import { children, pageOf } from "@/lib/model";

type Holder = { nodes?: SceneNode[]; paths?: SceneNode[]; texts?: SceneNode[] };

function holds(node: SceneNode, id: string): boolean {
  return node.id === id || children(node).some((child) => holds(child, id));
}

/** The list without the node `id`, at any depth. */
function without(list: SceneNode[], id: string): SceneNode[] {
  return list
    .filter((node) => node.id !== id)
    .map((node) => (children(node).length ? { ...node, children: without(children(node), id) } : node));
}

/** Only the node `id` and the groups that contain it, so group transforms still apply. */
function only(list: SceneNode[], id: string): SceneNode[] {
  return list
    .filter((node) => holds(node, id))
    .map((node) => (node.id === id || !children(node).length ? node : { ...node, children: only(children(node), id) }));
}

function rebuild(doc: Doc, pageId: string | undefined, edit: (list: SceneNode[]) => SceneNode[], transparent: boolean): Doc {
  const target = pageOf(doc, pageId);
  if (!Array.isArray(doc.pages) && target) {
    const layers = (target.layers ?? []).map((layer) => {
      const next: Holder & Record<string, unknown> = { ...layer };
      for (const key of ["nodes", "paths", "texts"] as const) {
        const list = layer[key];
        if (Array.isArray(list)) next[key] = edit(list);
      }
      return next;
    });
    return { ...doc, layers, canvas: transparent ? { ...target.canvas, background: "none" } : target.canvas };
  }
  const pages = (doc.pages ?? []).map((page) => {
    if (page !== target) return page;
    const layers = (page.layers ?? []).map((layer) => {
      const next: Holder & Record<string, unknown> = { ...layer };
      for (const key of ["nodes", "paths", "texts"] as const) {
        const list = layer[key];
        if (Array.isArray(list)) next[key] = edit(list);
      }
      return next;
    });
    const canvas = transparent ? { ...page.canvas, background: "none" } : page.canvas;
    return { ...page, layers, canvas };
  });
  return { ...doc, pages } as Doc;
}

/** A whole-page grade: it applies per pixel to whatever is painted below it. */
const pageGrade = (node: SceneNode) =>
  node.kind === "adjustment" && (node.scope as { kind?: string } | undefined)?.kind === "below";

/**
 * Documents for a drag, all rendered at the same size so the object moves with
 * a CSS translate and no cropping math: the page without the node (`backdrop`),
 * the node alone on a transparent page (`sprite`), and, for a top-level node,
 * the nodes painted after it on a transparent page (`above`). With all three,
 * a drag frame only stacks three images, whatever the number of layers.
 *
 * Whole-page grades stay in every part so each part is graded as on the page.
 * The split is a live approximation (edges under a grade, blend modes against
 * the parts); the exact render replaces it as soon as the drag ends. A node
 * that is nested, clipped or clips others keeps the two-part split.
 */
export function dragLayers(doc: Doc, pageId: string | undefined, id: string): { backdrop: Doc; sprite: Doc; above: Doc | null } {
  const page = pageOf(doc, pageId);
  const order = Array.isArray(doc.pages) ? (page?.layers ?? []).flatMap((layer) => (Array.isArray(layer.nodes) ? layer.nodes : [])) : [];
  const at = order.findIndex((node) => node.id === id);
  const clipped = (node: SceneNode | undefined) => node?.clipping !== undefined && node?.clipping !== null;
  if (at < 0 || clipped(order[at]) || clipped(order[at + 1])) {
    return {
      backdrop: rebuild(doc, pageId, (list) => without(list, id), false),
      sprite: rebuild(doc, pageId, (list) => only(list, id), true),
      above: null,
    };
  }
  const position = new Map(order.map((node, index) => [node.id, index]));
  // Positions [from, to) of the page, plus the grades painted over any of them.
  const keep = (from: number, to: number) => (list: SceneNode[]) =>
    list.filter((node) => {
      const p = position.get(node.id) ?? -1;
      return pageGrade(node) ? p > from : p >= from && p < to && node.kind !== "adjustment";
    });
  const content = (from: number, to: number) => order.slice(from, to).some((node) => node.kind !== "adjustment");
  return {
    backdrop: rebuild(doc, pageId, keep(0, at), false),
    sprite: rebuild(doc, pageId, keep(at, at + 1), true),
    above: content(at + 1, order.length) ? rebuild(doc, pageId, keep(at + 1, order.length), true) : null,
  };
}

// The same part content (for example the backdrop after the object moved)
// maps to the same document object, so its finished render is reused.
const parts = new Map<string, Doc>();
export function sameContent(doc: Doc): Doc {
  const key = JSON.stringify(doc);
  const known = parts.get(key);
  if (known) return known;
  parts.set(key, doc);
  if (parts.size > 12) parts.delete(parts.keys().next().value!);
  return doc;
}
