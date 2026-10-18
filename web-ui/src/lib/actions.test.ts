import { describe, expect, test } from "bun:test";
import { findNode, nodeWorldTransform, pageOf } from "./model";
import type { Doc } from "./api";

// The store reads the colour scheme at import; give it a minimal window.
(globalThis as { window?: unknown }).window ??= { matchMedia: () => ({ matches: false }) };
const { moveInStack } = await import("./actions");
const { useStudio } = await import("@/store");

const sceneDoc = () => ({
  version: 11,
  name: "Stack",
  pages: [{
    id: "p",
    canvas: { width: 400, height: 300, background: "#ffffff" },
    layers: [
      { id: "l1", nodes: [
        { kind: "rect", id: "a", x: 10, y: 10, width: 20, height: 20 },
        { kind: "group", id: "g", transform: [2, 0, 0, 2, 100, 50], children: [{ kind: "rect", id: "b", x: 0, y: 0, width: 5, height: 5 }] },
      ] },
      { id: "l2", nodes: [{ kind: "rect", id: "c", x: 0, y: 0, width: 5, height: 5 }] },
      { id: "locked", locked: true, nodes: [] },
    ],
  }],
}) as unknown as Doc;

const load = (doc: Doc) => useStudio.setState({ doc, pageId: "p", past: [], future: [], selectedIds: [], selectedId: null });
const page = () => pageOf(useStudio.getState().doc, "p");
const world = (id: string) => nodeWorldTransform(findNode(page(), id)!);

describe("moveInStack", () => {
  test("nesting into a scaled, moved group keeps the object where it was painted", () => {
    load(sceneDoc());
    const before = world("a");
    moveInStack("a", { node: "g", where: "into" });
    const found = findNode(page(), "a")!;
    expect(found.parent?.id).toBe("g");
    const after = world("a");
    after.forEach((v, i) => expect(v).toBeCloseTo(before[i]!, 9));
    expect(useStudio.getState().past).toHaveLength(1);
  });

  test("moving out of a group to another layer bakes the group transform", () => {
    load(sceneDoc());
    const before = world("b");
    moveInStack("b", { node: "c", where: "above" });
    const layer = page()!.layers![1]!;
    expect(layer.nodes!.map((n) => n.id)).toEqual(["c", "b"]);
    world("b").forEach((v, i) => expect(v).toBeCloseTo(before[i]!, 9));
  });

  test("a group cannot be moved into its own child, and locked layers refuse drops", () => {
    load(sceneDoc());
    const doc = JSON.stringify(useStudio.getState().doc);
    moveInStack("g", { node: "b", where: "below" });
    moveInStack("a", { layer: "locked" });
    expect(JSON.stringify(useStudio.getState().doc)).toBe(doc);
  });

  test("legacy objects reorder only within their own list", () => {
    load({ version: 1, name: "L", canvas: { width: 10, height: 10 }, layers: [
      { id: "x", paths: [{ id: "p1", d: "M0 0" }, { id: "p2", d: "M1 1" }] },
      { id: "y", paths: [] },
    ] } as unknown as Doc);
    useStudio.setState({ pageId: undefined });
    moveInStack("p1", { node: "p2", where: "above" });
    const doc = useStudio.getState().doc!;
    expect(doc.layers![0]!.paths!.map((n) => n.id)).toEqual(["p2", "p1"]);
    const before = JSON.stringify(doc);
    moveInStack("p1", { layer: "y" });
    expect(JSON.stringify(useStudio.getState().doc)).toBe(before);
  });
});
