import { describe, expect, test } from "bun:test";
import type { Doc, SceneNode } from "./api";
import { findNode, pageOf } from "./model";

// The store reads the colour scheme at import; give it a minimal window.
(globalThis as { window?: unknown }).window ??= { matchMedia: () => ({ matches: false }) };
const layers = await import("./layers");
const { copySelectionText, duplicateSelected, moveInStack, pasteSelectionText } = await import("./actions");
const { useStudio } = await import("@/store");

const rect = (id: string, extra: Record<string, unknown> = {}): SceneNode => ({ kind: "rect", id, x: 0, y: 0, width: 10, height: 10, ...extra });
const adjust = (id: string, scope: unknown = { kind: "below" }): SceneNode => ({
  kind: "adjustment", id, adjustment: "invert", params: {}, scope, enabled: true, opacity: 1, blend_mode: "normal", blend_space: "srgb",
});

const doc = (nodes: SceneNode[], extra: Record<string, unknown> = {}) => ({
  format: "pentool",
  version: 6,
  pages: [{ id: "p", canvas: { width: 100, height: 100 }, layers: [{ id: "l", nodes }] }],
  ...extra,
}) as unknown as Doc;

const load = (d: Doc, selected: string | null = null) =>
  useStudio.setState({ doc: d, pageId: "p", layerId: "l", past: [], future: [], selectedIds: selected ? [selected] : [], selectedId: selected });
const stack = () => pageOf(useStudio.getState().doc, "p")!.layers![0]!.nodes!;

describe("clipping", () => {
  test("Alt-click clips content to the layer below and adjustments by scope", () => {
    const nodes = [rect("photo"), adjust("curves"), rect("tint")];
    expect(layers.toggleClipIn(nodes, "curves")).toBeNull();
    expect(nodes[1]!.scope).toEqual({ kind: "targets", ids: ["photo"] });
    // A clipped adjustment passes the base on to the layer above it.
    expect(layers.clipTarget(nodes, 2)).toBe("photo");
  });

  test("an adjustment cannot be a clipping base", () => {
    const nodes = [rect("photo"), adjust("curves"), rect("tint")];
    expect(layers.toggleClipIn(nodes, "tint")).toContain("cannot be a clipping base");
    expect(nodes[2]!.clipping).toBeUndefined();
  });

  test("releasing a clipped layer releases the clipped layers above it", () => {
    const nodes = [rect("base"), rect("a", { clipping: { base: "base" } }), rect("b", { clipping: { base: "base" } }), rect("top")];
    expect(layers.toggleClipIn(nodes, "a")).toBeNull();
    expect(nodes.map((n) => layers.clipBaseOf(n))).toEqual([null, null, null, null]);
  });

  test("clipping a base moves its clipped layers to the new base", () => {
    const nodes = [rect("base"), rect("mid"), rect("over", { clipping: { base: "mid" } })];
    expect(layers.toggleClipIn(nodes, "mid")).toBeNull();
    expect(nodes.map((n) => layers.clipBaseOf(n))).toEqual([null, "base", "base"]);
  });

  test("an insert after a base goes above its whole clipping group", () => {
    const nodes = [rect("base"), rect("a", { clipping: { base: "base" } }), adjust("adj", { kind: "targets", ids: ["base"] }), rect("top")];
    expect(layers.afterClipGroup(nodes, 0)).toBe(3);
    expect(layers.afterClipGroup(nodes, 3)).toBe(4);
  });
});

describe("reordering keeps clipping valid", () => {
  const bases = () => stack().map((n) => `${n.id}:${layers.clipBaseOf(n) ?? "-"}`);

  test("dragging a clipped layer out of its group releases it", () => {
    load(doc([rect("base"), rect("a", { clipping: { base: "base" } }), rect("top")]));
    moveInStack("a", { node: "top", where: "above" });
    expect(bases()).toEqual(["base:-", "top:-", "a:-"]);
  });

  test("dropping a layer inside a clipping group joins it", () => {
    load(doc([rect("base"), rect("a", { clipping: { base: "base" } }), rect("b", { clipping: { base: "base" } }), rect("top")]));
    moveInStack("top", { node: "a", where: "above" });
    expect(bases()).toEqual(["base:-", "a:base", "top:base", "b:base"]);
  });

  test("moving the base away re-clips its layers to the layer now below them", () => {
    load(doc([rect("floor"), rect("base"), rect("a", { clipping: { base: "base" } })]));
    moveInStack("base", { node: "floor", where: "below" });
    expect(bases()).toEqual(["base:-", "floor:-", "a:floor"]);
    moveInStack("floor", { node: "a", where: "above" });
    expect(bases()).toEqual(["base:-", "a:base", "floor:-"]);
  });

  test("Alt-drop clips an adjustment to the image it lands on", () => {
    load(doc([rect("photo"), rect("top"), adjust("grade")]));
    moveInStack("grade", { node: "photo", where: "above" }, true);
    expect(bases()).toEqual(["photo:-", "grade:photo", "top:-"]);
  });

  test("a clipped adjustment moved within its group stays clipped", () => {
    load(doc([rect("base"), rect("a", { clipping: { base: "base" } }), adjust("grade", { kind: "targets", ids: ["base"] })]));
    moveInStack("grade", { node: "base", where: "above" });
    expect(bases()).toEqual(["base:-", "grade:base", "a:base"]);
  });

  test("Alt-drop does not clip in a document without compositing", () => {
    load(doc([rect("photo"), rect("top")], { version: 4 }));
    moveInStack("top", { node: "photo", where: "above" }, true);
    expect(stack()[1]!.clipping).toBeUndefined();
  });
});

describe("new adjustment layer", () => {
  test("inserts directly above the selected layer with a numbered name", async () => {
    load(doc([rect("bottom"), rect("top")]), "bottom");
    await layers.addAdjustment("levels");
    const nodes = stack();
    expect(nodes.map((n) => n.kind)).toEqual(["rect", "adjustment", "rect"]);
    expect(nodes[1]).toMatchObject({ name: "Levels 1", adjustment: "levels", params: { black: 0, white: 255, gamma: 1 }, scope: { kind: "below" } });
    expect(useStudio.getState().selectedId).toBe(nodes[1]!.id);
  });

  test("every kind starts with params the engine accepts", () => {
    for (const spec of layers.ADJUSTMENTS) expect(spec.params()).toBeObject();
    expect(layers.ADJUSTMENTS).toHaveLength(13);
  });

  test("a v4 document is upgraded once before the first adjustment", async () => {
    const calls: number[] = [];
    load(doc([rect("a")], { version: 4 }));
    const upgradeDoc = useStudio.getState().upgradeDoc;
    useStudio.setState({
      upgradeDoc: async (target?: number) => {
        calls.push(target ?? 0);
        useStudio.setState({ doc: { ...useStudio.getState().doc!, version: 6 } });
      },
    });
    await layers.addAdjustment("invert");
    useStudio.setState({ upgradeDoc });
    expect(calls).toEqual([6]);
    expect(stack().at(-1)!.kind).toBe("adjustment");
  });
});

describe("copy and paste", () => {
  test("adjustments paste above the selection, in place, with references remapped", () => {
    load(doc([rect("photo"), adjust("inv", { kind: "targets", ids: ["photo"] })]));
    useStudio.setState({ selectedIds: ["photo", "inv"], selectedId: "inv" });
    const text = copySelectionText()!;
    load(doc([rect("other"), rect("top")]), "other");
    expect(pasteSelectionText(text)).toBe(true);
    const nodes = stack();
    expect(nodes.map((n) => n.kind)).toEqual(["rect", "rect", "adjustment", "rect"]);
    // The pasted adjustment follows its pasted base, not the original ID.
    expect(nodes[2]!.scope).toEqual({ kind: "targets", ids: [nodes[1]!.id] });
  });

  test("a lone clipped adjustment pasted elsewhere affects everything below", () => {
    load(doc([rect("photo"), adjust("inv", { kind: "targets", ids: ["photo"] })]), "inv");
    const text = copySelectionText()!;
    load(doc([rect("other")]), "other");
    pasteSelectionText(text);
    expect(stack()[1]!.scope).toEqual({ kind: "below" });
  });

  test("referenced images and masks travel with the copy", () => {
    const mask = { kind: "raster", asset: "sha256:m" };
    const source = doc([rect("photo", { mask: { resource: "sha256:r", invert: false, density: 1, feather: 0, linked: true } })], {
      image_assets: { "sha256:m": { media_type: "image/png" }, "sha256:unused": {} },
      mask_resources: { "sha256:r": mask },
    });
    load(source, "photo");
    const text = copySelectionText()!;
    load(doc([], { image_assets: {} }));
    pasteSelectionText(text);
    const target = useStudio.getState().doc as unknown as Record<string, Record<string, unknown>>;
    expect(Object.keys(target.image_assets!)).toEqual(["sha256:m"]);
    expect(target.mask_resources).toEqual({ "sha256:r": mask });
  });

  test("duplicating a clipping base keeps its clipped layers attached", () => {
    load(doc([rect("base"), rect("a", { clipping: { base: "base" } }), rect("top")]), "base");
    duplicateSelected();
    const nodes = stack();
    expect(nodes.map((n) => layers.clipBaseOf(n))).toEqual([null, "base", null, null]);
    expect(findNode(pageOf(useStudio.getState().doc, "p"), "a")!.siblings.indexOf(nodes[1]!)).toBe(1);
  });
});
