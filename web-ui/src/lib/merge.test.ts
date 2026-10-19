import { describe, expect, test } from "bun:test";
import { changedNodeIds, merge3 } from "./merge";
import type { Page } from "./api";

const node = (id: string, x: number, extra: Record<string, unknown> = {}) => ({ id, kind: "rect", x, ...extra });

describe("merge3", () => {
  test("keeps an agent's change and this tab's change to different nodes", () => {
    const base = { nodes: [node("a", 0), node("b", 0)] };
    const local = { nodes: [node("a", 5), node("b", 0)] };
    const remote = { nodes: [node("a", 0), node("b", 9)] };
    expect(merge3(base, local, remote)).toEqual({ nodes: [node("a", 5), node("b", 9)] });
  });

  test("adds nodes from both sides after their neighbours", () => {
    const base = { nodes: [node("a", 0)] };
    const local = { nodes: [node("a", 0), node("mine", 1)] };
    const remote = { nodes: [node("theirs", 2), node("a", 0)] };
    const merged = merge3(base, local, remote) as { nodes: { id: string }[] };
    expect(merged.nodes.map((n) => n.id)).toEqual(["theirs", "a", "mine"]);
  });

  test("lets the local edit win a conflict and honours deletes", () => {
    const base = { nodes: [node("a", 0), node("gone", 0)], name: "x" };
    const local = { nodes: [node("a", 5), node("gone", 0)], name: "x" };
    const remote = { nodes: [node("a", 7)], name: "y" };
    expect(merge3(base, local, remote)).toEqual({ nodes: [node("a", 5)], name: "y" });
  });
});

describe("changedNodeIds", () => {
  test("names new and edited nodes, not their unchanged groups", () => {
    const page = (nodes: unknown[]) => ({ id: "p", layers: [{ id: "l", nodes }] }) as unknown as Page;
    const before = page([node("g", 0, { children: [node("c", 0)] }), node("same", 0)]);
    const after = page([node("g", 0, { children: [node("c", 3)] }), node("same", 0), node("new", 1)]);
    expect(changedNodeIds(before, after)).toEqual(["c", "new"]);
  });
});
