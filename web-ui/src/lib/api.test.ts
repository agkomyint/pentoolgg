import { afterEach, expect, test } from "bun:test";
import { api, type Doc } from "./api";

const originalFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = originalFetch; });
const document = (): Doc => ({ version: 4, pages: [{ id: "page", canvas: { width: 20, height: 20 }, layers: [] }] });

test("reuses an unchanged preview and coalesces overlapping mode-switch requests", async () => {
  let calls = 0;
  globalThis.fetch = (async () => { calls++; return new Response(new Uint8Array([137, 80, 78, 71])); }) as typeof fetch;
  const doc = document();
  const [a, b] = await Promise.all([api.renderPng(doc, "page", 768), api.renderPng(doc, "page", 768)]);
  expect(a).toBe(b);
  expect(calls).toBe(1);
  await api.renderPng(doc, "page", 768);
  expect(calls).toBe(1);
  await api.renderPng({ ...doc }, "page", 768);
  await api.renderPng(doc, "page", 1024);
  await api.renderPng(doc, "other-page", 768);
  expect(calls).toBe(4);
});

// The server runs at most four preview jobs and answers "[limit-exceeded]" past that.
const busyServer = (limit: number) => {
  let running = 0;
  let peak = 0;
  globalThis.fetch = (async () => {
    if (running >= limit) {
      return new Response(JSON.stringify({ error: "[limit-exceeded] preview workers busy; cancel or retry" }), { status: 429 });
    }
    peak = Math.max(peak, ++running);
    await new Promise((r) => setTimeout(r, 5));
    running--;
    return new Response(new Uint8Array([137, 80, 78, 71]));
  }) as typeof fetch;
  return () => peak;
};

test("a burst of previews stays under the server's worker limit", async () => {
  const peak = busyServer(4);
  const docs = Array.from({ length: 8 }, document);
  await Promise.all(docs.map((doc) => api.renderPng(doc, "page", 768)));
  expect(peak()).toBeLessThanOrEqual(3);
});

test("a preview retries while the server's workers are busy", async () => {
  busyServer(0);
  const pending = api.renderPng(document(), "page", 768);
  setTimeout(() => busyServer(4), 30);
  expect((await pending).size).toBe(4);
});

test("a failed preview can retry instead of caching the error", async () => {
  let calls = 0;
  globalThis.fetch = (async () => { if (++calls === 1) throw new TypeError("Failed to fetch"); return new Response(new Uint8Array([137, 80, 78, 71])); }) as typeof fetch;
  const doc = document();
  await expect(api.renderPng(doc, "page", 768)).rejects.toThrow("Failed to fetch");
  await api.renderPng(doc, "page", 768);
  expect(calls).toBe(2);
});

test("postJson gives up on a hung server with a clear error", async () => {
  const real = globalThis.fetch;
  globalThis.fetch = ((_u: unknown, init?: RequestInit) => new Promise((_res, rej) => {
    if (init?.signal?.aborted) return rej(init.signal.reason);
    init?.signal?.addEventListener("abort", () => rej(init.signal!.reason));
  })) as typeof fetch;
  const realTimeout = AbortSignal.timeout;
  AbortSignal.timeout = () => AbortSignal.abort(new DOMException("t", "TimeoutError"));
  try {
    const { postJson } = await import("./api");
    await expect(postJson("/api/x", {})).rejects.toThrow(/did not answer/);
  } finally {
    globalThis.fetch = real;
    AbortSignal.timeout = realTimeout;
  }
});
