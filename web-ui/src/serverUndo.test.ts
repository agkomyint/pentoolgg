import { afterAll, expect, test } from "bun:test";
import type { Doc, SceneNode } from "./lib/api";

const previousWindow = globalThis.window;
Object.defineProperty(globalThis, "window", { configurable: true, value: { matchMedia: () => ({ matches: false }) } });
const { useStudio } = await import("./store");
const previousState = useStudio.getState();
afterAll(() => {
  useStudio.setState(previousState, true);
  if (previousWindow) Object.defineProperty(globalThis, "window", { configurable: true, value: previousWindow });
  else Reflect.deleteProperty(globalThis, "window");
});
const doc = (nodes: SceneNode[]): Doc => ({ version: 5, pages: [{ id: "p", layers: [{ id: "l", nodes }] }] });

test("a clean server acceptance has one normal undo and redo step", () => {
  const before = doc([]);
  const after = doc([{ id: "hero", kind: "image", ai: { provider: "mock", model: "image" } }]);
  useStudio.setState({ doc: after, dirty: false, past: [], future: [], pageId: "p" });
  useStudio.getState().rememberServerChange(before);
  expect(useStudio.getState().past).toHaveLength(1);
  useStudio.getState().undo();
  expect(useStudio.getState().doc).toEqual(before);
  expect(useStudio.getState().dirty).toBe(true);
  useStudio.getState().redo();
  expect(useStudio.getState().doc).toEqual(after);
});

test("an edit made during server acceptance keeps its local undo history", () => {
  const before = doc([]);
  const after = doc([{ id: "hero", kind: "image" }, { id: "local", kind: "rect" }]);
  const past = [JSON.stringify(doc([{ id: "hero", kind: "image" }]))];
  useStudio.setState({ doc: after, dirty: true, past, future: [] });
  useStudio.getState().rememberServerChange(before);
  expect(useStudio.getState().past).toEqual(past);
  expect(useStudio.getState().doc).toEqual(after);
});
