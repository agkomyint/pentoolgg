import { create } from "zustand";

/**
 * Browser build of the Pentool renderer (`wasm/`). It is a live preview of the
 * same document; exports and files stay on the native renderer. When the module
 * is missing, unsupported content appears, or a render fails, callers fall back
 * to the server (`/api/render/png`), so Studio never depends on it.
 *
 * The module runs in a dedicated worker: a heavy page (many blurred layers)
 * can take seconds to render, and on the main thread that froze every click
 * and drag until it finished.
 */
export type RenderSource = "wasm" | "server";

type Job = { id: number; doc: string; page: string; scale: number };
type Reply =
  | { id: number; ok: true; png: Blob | null; pixels: ArrayBuffer | null; width: number; height: number }
  | { id: number; ok: false; unavailable: boolean };

/**
 * Worker body. It is serialized with `toString()` into a Blob URL, so it must
 * not refer to anything outside itself.
 */
function workerMain(wasmUrl: string) {
  type Exports = {
    memory: WebAssembly.Memory;
    alloc(len: number): number;
    dealloc(ptr: number, len: number): void;
    render(doc: number, docLen: number, page: number, pageLen: number, scale: number): number;
    result_ptr(): number;
    result_len(): number;
    result_width(): number;
    result_height(): number;
  };
  const scope = self as unknown as {
    onmessage: ((event: MessageEvent<Job>) => void) | null;
    postMessage(message: Reply, transfer?: Transferable[]): void;
  };
  const encoder = new TextEncoder();
  let core: Promise<Exports | null> | null = null;
  const load = async (): Promise<Exports | null> => {
    try {
      const response = await fetch(wasmUrl);
      if (!response.ok) return null;
      const { instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {});
      return instance.exports as unknown as Exports;
    } catch {
      return null;
    }
  };
  const write = (x: Exports, text: string): [number, number] => {
    const bytes = encoder.encode(text);
    const ptr = x.alloc(bytes.length);
    new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
    return [ptr, bytes.length];
  };
  scope.onmessage = async ({ data: job }) => {
    const x = await (core ??= load());
    if (!x) return scope.postMessage({ id: job.id, ok: false, unavailable: true });
    const [dp, dl] = write(x, job.doc);
    const [pp, pl] = write(x, job.page);
    try {
      if (x.render(dp, dl, pp, pl, job.scale) !== 0) return scope.postMessage({ id: job.id, ok: false, unavailable: false });
      const width = x.result_width();
      const height = x.result_height();
      const pixels = new Uint8ClampedArray(x.memory.buffer, x.result_ptr(), x.result_len()).slice();
      if (typeof OffscreenCanvas !== "undefined") {
        const canvas = new OffscreenCanvas(width, height);
        canvas.getContext("2d")!.putImageData(new ImageData(pixels, width, height), 0, 0);
        const png = await canvas.convertToBlob({ type: "image/png" });
        return scope.postMessage({ id: job.id, ok: true, png, pixels: null, width, height });
      }
      scope.postMessage({ id: job.id, ok: true, png: null, pixels: pixels.buffer, width, height }, [pixels.buffer]);
    } catch {
      scope.postMessage({ id: job.id, ok: false, unavailable: false });
    } finally {
      x.dealloc(dp, dl);
      x.dealloc(pp, pl);
    }
  };
}

type Stats = {
  enabled: boolean;
  available: boolean | null;
  last: { source: RenderSource; ms: number; width: number; height: number } | null;
  setEnabled: (on: boolean) => void;
};

const KEY = "pentool-local-render";
const stored = () => {
  try {
    return localStorage.getItem(KEY) !== "0";
  } catch {
    return true;
  }
};

export const useRenderStats = create<Stats>((set) => ({
  enabled: stored(),
  available: null,
  last: null,
  setEnabled: (enabled) => {
    try {
      localStorage.setItem(KEY, enabled ? "1" : "0");
    } catch {
      // storage can be unavailable
    }
    set({ enabled });
  },
}));

const note = (patch: Partial<Stats>) => useRenderStats.setState(patch);

/** A queued render that a newer one in the same lane replaced before it started. */
export class Superseded extends Error {
  constructor() {
    super("superseded by a newer render");
  }
}

type Waiting = { job: Job; lane?: string; resolve: (reply: Reply) => void; reject: (error: Error) => void };

let worker: Worker | null | undefined;
let running = false;
let unavailable = false;
let nextId = 0;
const queue: Waiting[] = [];
const replies = new Map<number, (reply: Reply) => void>();

function start(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    const wasm = new URL("/core/pentool_core.wasm", location.href).href;
    const source = `(${workerMain.toString()})(${JSON.stringify(wasm)});`;
    worker = new Worker(URL.createObjectURL(new Blob([source], { type: "text/javascript" })));
    worker.onmessage = ({ data }: MessageEvent<Reply>) => {
      replies.get(data.id)?.(data);
      replies.delete(data.id);
    };
  } catch {
    worker = null;
  }
  return worker;
}

/**
 * The worker renders one page at a time; the rest wait here. The page on
 * screen (the `canvas` lane) goes first, ahead of thumbnails and drag parts.
 */
function pump() {
  if (running) return;
  const first = queue.findIndex((waiting) => waiting.lane === "canvas");
  const next = queue.splice(Math.max(first, 0), 1)[0];
  const w = next && start();
  if (!next) return;
  if (!w) return next.resolve({ id: next.job.id, ok: false, unavailable: true });
  running = true;
  replies.set(next.job.id, (reply) => {
    running = false;
    next.resolve(reply);
    pump();
  });
  w.postMessage(next.job);
}

/**
 * Formats the local renderer understands: the compositing documents, v6
 * through the newest (v12). It renders these byte-identically to the server.
 */
export const localVersion = (version: number) => version >= 6 && version <= 12;

/**
 * Render one page locally, off the main thread. Returns null when local
 * rendering is off, not available, or failed, so the caller can ask the
 * server. Renders that share a `lane` replace each other while waiting: only
 * the newest waiting one runs, and the others reject with `Superseded`.
 */
export async function renderLocal(
  doc: { version: number; pages?: { id: string; canvas?: { width: number; height: number } }[] },
  page: string | undefined,
  maxEdge: number,
  lane?: string,
): Promise<Blob | null> {
  if (!useRenderStats.getState().enabled || !localVersion(doc.version) || unavailable) return null;
  if (typeof Worker === "undefined" || typeof location === "undefined") return null;
  const selected = doc.pages?.find((p) => p.id === page) ?? doc.pages?.[0];
  const longest = Math.max(selected?.canvas?.width ?? 0, selected?.canvas?.height ?? 0);
  if (!longest) return null;
  const job: Job = { id: ++nextId, doc: JSON.stringify(doc), page: page ?? "", scale: Math.min(1, maxEdge / longest) };
  const started = performance.now();
  const reply = await new Promise<Reply>((resolve, reject) => {
    if (lane !== undefined) {
      for (let i = queue.length - 1; i >= 0; i--) {
        if (queue[i]!.lane === lane) queue.splice(i, 1)[0]!.reject(new Superseded());
      }
    }
    queue.push({ job, lane, resolve, reject });
    pump();
  });
  if (!reply.ok) {
    if (reply.unavailable) {
      unavailable = true;
      note({ available: false });
    }
    return null;
  }
  let blob = reply.png;
  if (!blob && reply.pixels) {
    const canvas = document.createElement("canvas");
    canvas.width = reply.width;
    canvas.height = reply.height;
    canvas.getContext("2d")!.putImageData(new ImageData(new Uint8ClampedArray(reply.pixels), reply.width, reply.height), 0, 0);
    blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  }
  if (blob) note({ available: true, last: { source: "wasm", ms: Math.round(performance.now() - started), width: reply.width, height: reply.height } });
  return blob;
}

/** Record a server render so the status bar can compare both paths. */
export function noteServerRender(ms: number) {
  note({ last: { source: "server", ms: Math.round(ms), width: 0, height: 0 } });
}
