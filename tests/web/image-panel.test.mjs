// Dependency-free DOM-stub test for web/image-panel.js. Run: node tests/web/image-panel.test.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

class El {
  constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.value = ''; this.listeners = {}; this.checked = false; this.dataset = {}; this.style = {}; }
  append(...c) { this.children.push(...c); }
  replaceChildren(...c) { this.children = c; if (c[0]?.value !== undefined) this.value = c[0].value; }
  setAttribute(k, v) { this.attrs[k] = v; }
  removeAttribute(k) { delete this.attrs[k]; }
  addEventListener(t, f) { this.listeners[t] = f; }
  removeEventListener() {}
  setPointerCapture() {}
  querySelector(sel) { return sel === '#imageHandles' ? this.children.find((c) => c.id === 'imageHandles') : null; }
  getBoundingClientRect() { return { left: 0, top: 0, width: 400, height: 300 }; }
  click() { return this.onclick?.(); }
  remove() {}
  get viewBox() { return { baseVal: { width: 400, height: 300 } }; }
}

const elements = {};
const ids = ['canvas', 'imageStatus', 'imageSelect', 'imageOps', 'imageOpKind', 'imageOpAdd', 'imageOpParams', 'imageBeforeAfter', 'imageReset', 'imageBake', 'imagePreviewStatus', 'imageFit', 'imageFocalX', 'imageFocalY', 'imageCropX', 'imageCropY', 'imageCropW', 'imageCropH', 'imageOpacity', 'imageApplyFrame', 'historyRefresh', 'imageEditing', 'imageAddForm', 'imageAddPage', 'imageAddLayer', 'imageAddId', 'imageFile', 'imageAllowMigration', 'imageMigrationLabel', 'imageAddSubmit', 'imageAddStatus'];
for (const id of ids) { elements[id] = new El('div'); elements[id].id = id; }

elements.imageSelect.replaceChildren = function (...c) { this.children = c; this.value = c[0]?.value ?? ''; };

const listeners = new Map();
const document = {
  getElementById: (id) => elements[id],
  createElement: (tag) => new El(tag),
  createElementNS: (_ns, tag) => new El(tag),
  importNode: (n) => n,
  addEventListener: (name, fn) => { const list = listeners.get(name) || []; list.push(fn); listeners.set(name,list); },
  dispatchEvent: event => { for (const fn of listeners.get(event.type) || []) fn(event); },
};

const image = { kind: 'image', id: 'hero', asset: 'sha256:abcdef0123456789abcdef', x: 10, y: 10, width: 200, height: 100, fit: 'cover', position: [0.5, 0.5], crop: [0, 0, 1, 1], opacity: 1, operations: [{ id: 'g', kind: 'grayscale', enabled: true, params: {} }] };
const doc = { version: 5, image_assets: { [image.asset]: {} }, pages: [{ id:'page-2',layers: [{ id:'content',nodes: [image] }] }] };
document.addEventListener('pentool-request-document',() => document.dispatchEvent({type:'pentool-document-changed',detail:{document:doc,revision:'r1',shared:true}}));
const posts = [];
const fetch = async (url, init) => {
  if (init?.method === 'POST') posts.push({ url, body: JSON.parse(init.body) });
  return { ok: true, json: async () => ({ document: doc, revision: 'r1', result: {} }), text: async () => '<svg xmlns="http://www.w3.org/2000/svg"/>' };
};

const context = {
  document, fetch, structuredClone, Option: class { constructor(text, value) { this.text = text; this.value = value; } },
  MutationObserver: class { observe() {} }, DOMParser: class { parseFromString() { return { documentElement: { childNodes: [] } }; } },
  confirm: () => true, JSON, Number, Math, Object, console,
  Event: class {constructor(type){this.type=type;}},
  CustomEvent: class {constructor(type,options){this.type=type;this.detail=options.detail;}},
  FileReader: class {readAsDataURL(){this.result='data:image/png;base64,cG5n';this.onload();}},
};
vm.createContext(context);
vm.runInContext(readFileSync(new URL('../../web/image-panel.js', import.meta.url), 'utf8'), context);
await new Promise((r) => setTimeout(r, 20));

// The operation list renders and the frame controls mirror the node.
assert.equal(elements.imageOps.children.length, 1);
assert.equal(elements.imageFit.value, 'cover');
assert.equal(elements.imageOpKind.children.length, 10);

// Applying the frame commits one set-image batch through the shared API.
elements.imageCropW.value = '0.5';
elements.imageFocalX.value = '0.25';
await elements.imageApplyFrame.onclick();
await new Promise((r) => setTimeout(r, 5));
const scene = posts.find((p) => p.url === '/api/scene');
assert.equal(scene.body.operations[0].type, 'set-image');
assert.deepEqual(scene.body.operations[0].crop, [0, 0, 0.5, 1]);
assert.deepEqual(scene.body.operations[0].position, [0.25, 0.5]);
assert.equal(scene.body.revision, 'r1');
assert.equal(scene.body.page, 'page-2');

// Reset removes every operation in one batch after confirmation.
posts.length = 0;
await elements.imageReset.onclick();
await new Promise((r) => setTimeout(r, 5));
assert.deepEqual(posts[0].body.operations.map((o) => o.type), ['image-op-remove']);

// Bake asks for a dry run first, then commits.
posts.length = 0;
await elements.imageBake.onclick();
assert.deepEqual(posts.map((p) => !!p.body.dry_run), [true, false]);

// Invalid parameter JSON is rejected locally without any request.
posts.length = 0;
elements.imageOpParams.value = '{nope';
elements.imageOpAdd.onclick();
assert.equal(posts.length, 0);
assert.match(elements.imageStatus.textContent, /valid JSON/);

// The visible import form preserves input on failures and sends page/layer/revision.
posts.length = 0;
elements.imageFile.files=[{name:'photo.png',size:4}];
elements.imageAddId.value='new-photo';
await elements.imageAddForm.onsubmit({preventDefault(){}});
assert.equal(posts[0].url,'/api/image/add');
assert.equal(posts[0].body.page,'page-2');
assert.equal(posts[0].body.layer,'content');
assert.equal(posts[0].body.revision,'r1');
assert.equal(posts[0].body.data,'cG5n');
assert.equal(posts[0].body.document,undefined);
assert.match(elements.imageAddStatus.textContent,/Added new-photo/);
assert.equal(elements.imageAddSubmit.disabled,false);
posts.length=0;
elements.imageFile.files=[{name:'huge.png',size:33*1024*1024}];
await elements.imageAddForm.onsubmit({preventDefault(){}});
assert.equal(posts.length,0);
assert.equal(elements.imageAddId.value,'new-photo');
assert.match(elements.imageAddStatus.textContent,/32 MiB/);

// Bare serve uses an explicit browser document; it never writes a shared file.
const legacy={version:3,pages:[{id:'local',layers:[{id:'ink'}]}]};
document.dispatchEvent({type:'pentool-document-changed',detail:{document:legacy,shared:false,revision:null}});
elements.imageFile.files=[{name:'photo.png',size:4}];
elements.imageAllowMigration.checked=false;
await elements.imageAddForm.onsubmit({preventDefault(){}});
assert.equal(posts.length,0);
assert.match(elements.imageAddStatus.textContent,/upgrading/);
elements.imageAllowMigration.checked=true;
await elements.imageAddForm.onsubmit({preventDefault(){}});
assert.equal(posts[0].body.document.version,3);
assert.equal(posts[0].body.migrate,true);
assert.equal(posts[0].body.page,'local');

console.log('image-panel: ok');
