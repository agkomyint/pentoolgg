import type { Doc, Page, SceneNode } from "./api";
import { children, layerNodes } from "./model";

type Obj = Record<string, unknown>;
type Keyed = Obj & { id: string };

const isObj = (value: unknown): value is Obj => !!value && typeof value === "object" && !Array.isArray(value);
const same = (a: unknown, b: unknown) => a === b || JSON.stringify(a) === JSON.stringify(b);
const keyed = (value: unknown): value is Keyed[] =>
  Array.isArray(value) &&
  value.every((item) => isObj(item) && typeof item.id === "string") &&
  new Set(value.map((item) => (item as Keyed).id)).size === value.length;

/**
 * Three-way merge of JSON documents: what this tab started from (`base`), its
 * unsaved edits (`local`) and what another writer saved (`remote`). Objects
 * merge per key and arrays of objects with unique `id`s merge per item, so an
 * agent's change to one node and this tab's change to another both survive.
 * Where both sides changed the same value, the local edit wins: it is what the
 * person is looking at, and Save will send it on top of the remote document.
 */
export function merge3(base: unknown, local: unknown, remote: unknown): unknown {
  if (same(local, base)) return remote;
  if (same(remote, base) || same(local, remote)) return local;
  if (isObj(local) && isObj(remote)) {
    const b = isObj(base) ? base : {};
    const out: Obj = {};
    for (const key of new Set([...Object.keys(remote), ...Object.keys(local)])) {
      const value = merge3(b[key], local[key], remote[key]);
      if (value !== undefined) out[key] = value;
    }
    return out;
  }
  if (keyed(local) && keyed(remote)) return mergeList(keyed(base) ? base : [], local, remote);
  return local;
}

function mergeList(base: Keyed[], local: Keyed[], remote: Keyed[]): Keyed[] {
  const index = (list: Keyed[]) => new Map(list.map((item) => [item.id, item]));
  const b = index(base);
  const l = index(local);
  const r = index(remote);
  // Keep this tab's ordering only when it reordered something; else follow the remote.
  const shared = (list: Keyed[]) => list.map((item) => item.id).filter((id) => b.has(id) && l.has(id) && r.has(id));
  const localMoved = !same(shared(local), shared(base));
  const primary = (localMoved ? local : remote).map((item) => item.id);
  const secondary = (localMoved ? remote : local).map((item) => item.id);
  const order = [...primary];
  const placed = new Set(order);
  secondary.forEach((id, i) => {
    if (placed.has(id)) return;
    // An item only one side has goes right after its predecessor on that side.
    const at = i > 0 ? order.indexOf(secondary[i - 1]!) : -1;
    order.splice(at + 1, 0, id);
    placed.add(id);
  });
  const out: Keyed[] = [];
  for (const id of order) {
    const item = merge3(b.get(id), l.get(id), r.get(id));
    if (item !== undefined) out.push(item as Keyed);
  }
  return out;
}

/** Merge a remote save into a tab's document; see {@link merge3}. */
export function mergeDocs(base: Doc, local: Doc, remote: Doc): Doc {
  return merge3(base, local, remote) as Doc;
}

/**
 * IDs of the nodes on `after` that are new or whose own properties differ from
 * `before`. A changed child does not mark its group, so highlights stay tight.
 */
export function changedNodeIds(before: Page | undefined, after: Page | undefined): string[] {
  if (!after) return [];
  const own = (node: SceneNode) => JSON.stringify({ ...node, children: undefined, nodes: undefined });
  const previous = new Map<string, string>();
  const index = (nodes: SceneNode[], into: (node: SceneNode) => void) => {
    for (const node of nodes) {
      into(node);
      index(children(node), into);
    }
  };
  for (const layer of before?.layers ?? []) index(layerNodes(layer), (node) => previous.set(node.id, own(node)));
  const changed: string[] = [];
  for (const layer of after.layers ?? []) {
    index(layerNodes(layer), (node) => {
      if (previous.get(node.id) !== own(node)) changed.push(node.id);
    });
  }
  return changed;
}
