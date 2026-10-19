import { expect, test } from "bun:test";
import type { Doc, SceneNode } from "./api";
import { dragLayers, sameContent } from "./dragSprite";

const node = (id: string, extra: Record<string, unknown> = {}) => ({ kind: "rect", id, x: 0, y: 0, width: 10, height: 10, ...extra }) as SceneNode;
const grade = { kind: "adjustment", id: "grade", adjustment: "hue-saturation", scope: { kind: "below" } } as SceneNode;
const page = (nodes: SceneNode[][]) =>
  ({
    version: 6,
    pages: [{ id: "p", canvas: { width: 100, height: 50, background: "#ffffff" }, layers: nodes.map((list, i) => ({ id: `l${i}`, nodes: list })) }],
  }) as unknown as Doc;
const ids = (doc: Doc | null) => (doc?.pages?.[0]?.layers ?? []).map((layer) => (layer.nodes ?? []).map((n) => n.id));
const background = (doc: Doc | null) => (doc?.pages?.[0]?.canvas as { background?: string } | undefined)?.background;

test("a top-level object splits into below, itself and above, across layers", () => {
  const parts = dragLayers(page([[node("a"), node("b")], [node("c"), grade]]), "p", "b");
  expect(ids(parts.backdrop)).toEqual([["a"], ["grade"]]);
  expect(ids(parts.sprite)).toEqual([["b"], ["grade"]]);
  expect(ids(parts.above)).toEqual([[], ["c", "grade"]]);
  expect(background(parts.backdrop)).toBe("#ffffff");
  expect(background(parts.sprite)).toBe("none");
  expect(background(parts.above)).toBe("none");
});

test("a grade below the object stays out of the object and the parts above", () => {
  const parts = dragLayers(page([[node("a"), grade, node("b"), node("c")]]), "p", "b");
  expect(ids(parts.backdrop)).toEqual([["a", "grade"]]);
  expect(ids(parts.sprite)).toEqual([["b"]]);
  expect(ids(parts.above)).toEqual([["c"]]);
});

test("the top object has no part above, and clipping keeps the two-part split", () => {
  expect(dragLayers(page([[node("a"), node("b")]]), "p", "b").above).toBeNull();
  const clipped = dragLayers(page([[node("a"), node("b"), node("c", { clipping: { base: "b" } })]]), "p", "b");
  expect(clipped.above).toBeNull();
  expect(ids(clipped.backdrop)).toEqual([["a", "c"]]);
});

test("identical part content maps to one document", () => {
  const first = dragLayers(page([[node("a"), node("b")]]), "p", "b").backdrop;
  const moved = dragLayers(page([[node("a"), node("b", { x: 40 })]]), "p", "b").backdrop;
  expect(sameContent(moved)).toBe(sameContent(first));
});
