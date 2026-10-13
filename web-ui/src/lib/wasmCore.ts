import { create } from "zustand";

/**
 * Browser build of the Pentool renderer (`wasm/`). It is a live preview of the
 * same document; exports and files stay on the native renderer. When the module
 * is missing, unsupported content appears, or a render fails, callers fall back
 * to the server (`/api/render/png`), so Studio never depends on it.
 */
type Exports = {
  memory: WebAssembly.Memory;
  alloc(len: number): number;
  dealloc(ptr: number, len: number): void;
  render(doc: number, docLen: number, page: number, pageLen: number, scale: number): number;
  result_ptr(): number;
  result_len(): number;
  result_width(): number;
  result_height(): number;
  error_ptr(): number;
  error_len(): number;
};

export type RenderSource = "wasm" | "server";

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

let loading: Promise<Exports | null> | null = null;

async function load(): Promise<Exports | null> {
  try {
    const response = await fetch("/core/pentool_core.wasm");
    if (!response.ok) return null;
    const { instance } = await WebAssembly.instantiateStreaming(response, {}).catch(async () =>
      WebAssembly.instantiate(await (await fetch("/core/pentool_core.wasm")).arrayBuffer(), {}),
    );
    return instance.exports as unknown as Exports;
  } catch {
    return null;
  }
}

function core(): Promise<Exports | null> {
  loading ??= load().then((x) => {
    note({ available: x !== null });
    return x;
  });
  return loading;
}

const encoder = new TextEncoder();

function write(x: Exports, text: string): [number, number] {
  const bytes = encoder.encode(text);
  const ptr = x.alloc(bytes.length);
  new Uint8Array(x.memory.buffer, ptr, bytes.length).set(bytes);
  return [ptr, bytes.length];
}

/** Formats the local renderer understands (the compositing documents). */
export const localVersion = (version: number) => version >= 6 && version <= 9;

/**
 * Render one page locally. Returns null when local rendering is off, not
 * available, or failed, so the caller can ask the server.
 */
export async function renderLocal(
  doc: { version: number; pages?: { id: string; canvas?: { width: number; height: number } }[] },
  page: string | undefined,
  maxEdge: number,
): Promise<Blob | null> {
  if (!useRenderStats.getState().enabled || !localVersion(doc.version)) return null;
  const x = await core();
  if (!x) return null;
  const selected = doc.pages?.find((p) => p.id === page) ?? doc.pages?.[0];
  const longest = Math.max(selected?.canvas?.width ?? 0, selected?.canvas?.height ?? 0);
  if (!longest) return null;
  const scale = Math.min(1, maxEdge / longest);
  const started = performance.now();
  const [dp, dl] = write(x, JSON.stringify(doc));
  const [pp, pl] = write(x, page ?? "");
  try {
    if (x.render(dp, dl, pp, pl, scale) !== 0) return null;
    const width = x.result_width();
    const height = x.result_height();
    const pixels = new Uint8ClampedArray(x.memory.buffer, x.result_ptr(), x.result_len()).slice();
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    canvas.getContext("2d")!.putImageData(new ImageData(pixels, width, height), 0, 0);
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
    if (blob) note({ last: { source: "wasm", ms: Math.round(performance.now() - started), width, height } });
    return blob;
  } catch {
    return null;
  } finally {
    x.dealloc(dp, dl);
    x.dealloc(pp, pl);
  }
}

/** Record a server render so the status bar can compare both paths. */
export function noteServerRender(ms: number) {
  note({ last: { source: "server", ms: Math.round(ms), width: 0, height: 0 } });
}
