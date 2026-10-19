import { expect, test } from "bun:test";

// The studio store reads the colour scheme when it is created.
Object.assign(globalThis, { window: { matchMedia: () => ({ matches: false }) } });
const { photoApi } = await import("@/lib/photo");
const { useStudio } = await import("@/store");
const { usePhoto } = await import("./photoStore");

test("placing on an unsaved page saves the canvas before the server places", async () => {
  const calls: string[] = [];
  const doc = { format: "pentool", version: 4, pages: [{ id: "page-3", canvas: { width: 800, height: 600 }, layers: [{ id: "layer-1", nodes: [] }] }] };
  useStudio.setState({
    doc: doc as never,
    pageId: "page-3",
    shared: true,
    dirty: true,
    save: async () => {
      calls.push("save");
      useStudio.setState({ dirty: false });
    },
    pull: async () => {},
    notify: () => {},
  } as never);
  Object.assign(photoApi, {
    catalog: async () => (calls.push("catalog"), { revision: "r1", photos: [] }),
    place: async (body: { page?: string }) => (calls.push(`place ${body.page}`), { revision: "r2", node: "photo-1" }),
  });
  await usePhoto.getState().place(["photo-a"], { pageId: "page-3" });
  expect(calls).toEqual(["save", "catalog", "place page-3"]);
});

test("a failed save stops the placement", async () => {
  const calls: string[] = [];
  useStudio.setState({ dirty: true, save: async () => void calls.push("save"), notify: () => {} } as never);
  Object.assign(photoApi, { place: async () => (calls.push("place"), { revision: "r3", node: "photo-2" }) });
  await usePhoto.getState().place(["photo-a"], { pageId: "page-3" });
  expect(calls).toEqual(["save"]);
});
