// PEN-010-001: the Layers panel must not assume v3 `paths`/`texts` arrays on scene layers.
// Run: node tests/web/layers-v6.test.mjs [path/to/app.js]
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = readFileSync(process.argv[2] || 'web/app.js', 'utf8');
const line = source.split('\n').find((l) => l.includes('const kind=o.kind||'));
assert.ok(line, 'object-row kind/stack line not found');
const statement = line.trim().replace(/^const /, 'var ').replace(/;$/, '');
const index = (layer, o) => new Function('layer', 'o', `${statement}; return {kind, stack, index};`)(layer, o);

// A v6 scene layer has only `nodes` (no paths/texts): every node kind must work.
const scene = { id: 'content', nodes: [] };
for (const kind of ['group', 'rect', 'text', 'raster', 'image', 'path']) {
  const result = index(scene, { id: kind, kind });
  assert.equal(result.stack.length, 0, kind);
  assert.equal(result.index, -1, kind);
}
// Legacy v3 layers keep their stacks.
const legacy = { paths: [{ id: 'a' }], texts: [{ id: 't' }] };
assert.equal(index(legacy, legacy.paths[0]).index, 0);
assert.equal(index(legacy, legacy.texts[0]).kind, 'text');
console.log('layers-v6 ok');
