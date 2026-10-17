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

test("a failed preview can retry instead of caching the error", async () => {
  let calls = 0;
  globalThis.fetch = (async () => { if (++calls === 1) throw new TypeError("Failed to fetch"); return new Response(new Uint8Array([137, 80, 78, 71])); }) as typeof fetch;
  const doc = document();
  await expect(api.renderPng(doc, "page", 768)).rejects.toThrow("Failed to fetch");
  await api.renderPng(doc, "page", 768);
  expect(calls).toBe(2);
});
