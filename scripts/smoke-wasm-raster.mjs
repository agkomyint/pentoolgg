import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const [wasmPath, documentPath] = process.argv.slice(2);
assert(wasmPath && documentPath, "usage: node scripts/smoke-wasm-raster.mjs CORE.wasm DOCUMENT.pen");

const { instance } = await WebAssembly.instantiate(await readFile(wasmPath), {});
const core = instance.exports;
const encoder = new TextEncoder();

function render(document) {
  const docBytes = encoder.encode(JSON.stringify(document));
  const pageBytes = new Uint8Array();
  const docPtr = core.alloc(docBytes.length);
  const pagePtr = core.alloc(pageBytes.length);
  new Uint8Array(core.memory.buffer, docPtr, docBytes.length).set(docBytes);
  try {
    assert.equal(core.render(docPtr, docBytes.length, pagePtr, 0, 1), 0, "WASM render rejected the raster document");
    const width = core.result_width();
    const height = core.result_height();
    return { width, height, pixels: new Uint8Array(core.memory.buffer, core.result_ptr(), core.result_len()).slice() };
  } finally {
    core.dealloc(docPtr, docBytes.length);
    core.dealloc(pagePtr, 0);
  }
}

const document = JSON.parse(await readFile(documentPath, "utf8"));
const withRaster = render(document);
const withoutRasterDocument = structuredClone(document);
const nodes = withoutRasterDocument.pages[0].layers.flatMap((layer) => layer.nodes ?? []);
const rasterIndex = nodes.findIndex((node) => node.kind === "raster");
assert.notEqual(rasterIndex, -1, "fixture must contain a painted raster layer");
for (const layer of withoutRasterDocument.pages[0].layers) {
  layer.nodes = (layer.nodes ?? []).filter((node) => node.kind !== "raster");
}
delete withoutRasterDocument.raster_tiles;
const withoutRaster = render(withoutRasterDocument);
assert.equal(withRaster.width, withoutRaster.width);
assert.equal(withRaster.height, withoutRaster.height);
assert.notDeepEqual(withRaster.pixels, withoutRaster.pixels, "painted raster pixels must affect the WASM render");
process.stdout.write(`WASM raster render passed (${withRaster.width}x${withRaster.height})\n`);
