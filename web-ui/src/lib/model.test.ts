import { describe, expect, test } from "bun:test";
import { apply, collectIds, invert, nodeWorldTransform, pageOf, pagesOf, type Found } from "./model";
import { dragLayers } from "./dragSprite";
import type { Doc } from "./api";

describe("legacy canvas compatibility", () => {
  const makeLegacy = () => ({ version: 1, name: "Legacy", canvas: { width: 400, height: 300, background: "#ffffff" }, layers: [{ id: "layer", paths: [{ id: "a", d: "M 0 0 L 20 20" }, { id: "b", d: "M 5 5 L 10 10" }] }] }) as unknown as Doc;
  test("exposes the original canvas and layer references without migrating the file", () => {
    const doc = makeLegacy();
    const before = JSON.stringify(doc);
    const page = pageOf(doc, undefined)!;
    expect(page.canvas === doc.canvas).toBe(true);
    expect(page.layers === doc.layers).toBe(true);
    expect(pagesOf(doc)).toHaveLength(1);
    expect(collectIds(doc).has("a")).toBe(true);
    expect(JSON.stringify(doc)).toBe(before);
    expect(doc.pages).toBeUndefined();
  });
  test("drag previews isolate artwork without adding pages or altering the source", () => {
    const doc = makeLegacy();
    const before = JSON.stringify(doc);
    const { backdrop, sprite } = dragLayers(doc, "legacy-canvas", "a");
    expect(pageOf(backdrop, undefined)!.layers![0]!.paths!.map(n => n.id)).toEqual(["b"]);
    expect(pageOf(sprite, undefined)!.layers![0]!.paths!.map(n => n.id)).toEqual(["a"]);
    expect(sprite.pages).toBeUndefined();
    expect(sprite.version).toBe(1);
    expect((sprite.canvas as { background: string }).background).toBe("none");
    expect(JSON.stringify(doc)).toBe(before);
  });
  test("future formats are not treated as a legacy canvas", () => {
    expect(pageOf({ ...makeLegacy(), version: 999 }, undefined)).toBeUndefined();
  });
});

describe("nodeWorldTransform", () => {
  test("includes the raster node transform beneath transformed groups", () => {
    const found = {
      node: { id: "paint", kind: "raster", transform: [0, 2, -2, 0, 7, 11] },
      world: [1.5, 0, 0, 1.5, 20, 30],
    } as unknown as Pick<Found, "node" | "world">;
    const world = nodeWorldTransform(found);
    const pagePoint = apply(world, 15, 25);
    const restored = apply(invert(world)!, ...pagePoint);
    expect(restored[0]).toBeCloseTo(15);
    expect(restored[1]).toBeCloseTo(25);
    expect(pagePoint).toEqual([-44.5, 91.5]);
  });
});
