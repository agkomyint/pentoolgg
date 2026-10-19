import { expect, test } from "bun:test";

// A stand-in worker: it records each job and answers when the test says so.
type Job = { id: number; page: string; scale: number };
const posted: Job[] = [];
let instance: FakeWorker | null = null;
class FakeWorker {
  onmessage: ((event: { data: unknown }) => void) | null = null;
  constructor() {
    instance = this;
  }
  postMessage(job: Job) {
    posted.push(job);
  }
}
const g = globalThis as Record<string, unknown>;
g.Worker = FakeWorker;
g.location ??= { href: "http://127.0.0.1:7878/" };
g.document ??= {};
const { renderLocal, Superseded } = await import("./wasmCore");

const doc = { version: 6, pages: [{ id: "p", canvas: { width: 1000, height: 500 } }] };
const answer = (id: number) =>
  instance!.onmessage!({ data: { id, ok: true, png: new Blob(["png"]), pixels: null, width: 1, height: 1 } });
const tick = () => new Promise((r) => setTimeout(r, 0));

test("renders run one at a time and a newer canvas render replaces a waiting one", async () => {
  const first = renderLocal(doc, "p", 500, "canvas");
  const thumb = renderLocal(doc, "p", 120);
  const stale = renderLocal(doc, "p", 768, "canvas").catch((error: unknown) => error);
  const latest = renderLocal(doc, "p", 1000, "canvas");
  await tick();
  // Only the first job reached the worker; the main thread never renders.
  expect(posted.map((j) => j.scale)).toEqual([0.5]);
  expect(await stale).toBeInstanceOf(Superseded);
  answer(posted[0]!.id);
  expect(await first).toBeInstanceOf(Blob);
  await tick();
  answer(posted[1]!.id);
  expect(await thumb).toBeInstanceOf(Blob);
  await tick();
  expect(posted.map((j) => j.scale)).toEqual([0.5, 0.12, 1]);
  answer(posted[2]!.id);
  expect(await latest).toBeInstanceOf(Blob);
});

test("a missing module falls back to the server from then on", async () => {
  const pending = renderLocal(doc, "p", 500);
  await tick();
  instance!.onmessage!({ data: { id: posted.at(-1)!.id, ok: false, unavailable: true } });
  expect(await pending).toBeNull();
  const count = posted.length;
  expect(await renderLocal(doc, "p", 500)).toBeNull();
  expect(posted.length).toBe(count);
});
