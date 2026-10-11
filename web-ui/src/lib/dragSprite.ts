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

/**
 * Two documents for a drag: the page without the node (the backdrop) and the
 * node alone on a transparent page (the sprite). Both render at the same size,
 * so the sprite is moved with a CSS translate and no cropping math.
 */
export function dragLayers(doc: Doc, pageId: string | undefined, id: string): { backdrop: Doc; sprite: Doc } {
  return {
    backdrop: rebuild(doc, pageId, (list) => without(list, id), false),
    sprite: rebuild(doc, pageId, (list) => only(list, id), true),
  };
}
