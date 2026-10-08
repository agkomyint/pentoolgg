// No appearance math or document writes in JS: Rust is the shared planner/renderer.
(() => {
  const $ = id => document.getElementById(id);
  const panel = $('compositePanel');
  const state = { document: null, revision: null, nodes: [], busy: false, controller: null, generation: 0, job: null };
  function cancel() { state.generation++; state.controller?.abort(); if (state.job) fetch('/api/composite/cancel',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({job:state.job})}).catch(() => {}); }
  const modes = ['normal','multiply','screen','overlay','darken','lighten','color-dodge','color-burn','hard-light','soft-light','difference','exclusion','hue','saturation','color','luminosity'];
  $('compositeBlend').replaceChildren(...modes.map(mode => new Option(mode, mode)));
  const operations = ['adjustment-add','adjustment-set','adjustment-enable','adjustment-disable','adjustment-remove','fill-add','fill-set','node-move','effect-add','effect-set','effect-enable','effect-disable','effect-move','effect-remove','transform-add','transform-set','transform-metadata','transform-enable','transform-disable','transform-move','transform-remove','mask-create','mask-delete','selection-save','selection-crop'];
  $('compositeOperationKind').replaceChildren(...operations.map(kind => new Option(kind, kind)));
  const notify = message => { $('compositeStatus').textContent = message; };
  const collect = nodes => { for (const node of nodes || []) { state.nodes.push(node); collect(node.children); } };
  const current = () => state.nodes.find(node => node.id === $('compositeNode').value);
  const page = () => $('compositePage').value;
  const requiredNode = () => { const node = current(); if (!node) throw new Error('Select a node first.'); return node; };
  async function responseJSON(response) {
    const value = await response.json();
    if (!response.ok) throw new Error(value.error || 'Request rejected. Review settings and retry.');
    return value;
  }
  async function run(action) {
    if (state.busy) return;
    state.busy = true; panel.setAttribute('aria-busy', 'true');
    const buttons = [...panel.querySelectorAll('button')].filter(button => button.id !== 'compositeCancel');
    const disabled = buttons.map(button => button.disabled); buttons.forEach(button => { button.disabled = true; });
    notify('Working… Your source pixels remain unchanged.');
    try { await action(); } catch (error) { notify(error.name === 'AbortError' ? 'Preview cancelled.' : `${error.message} Your input is retained; correct it and retry.`); }
    finally { state.busy = false; panel.removeAttribute('aria-busy'); buttons.forEach((button, index) => { button.disabled = disabled[index]; }); }
  }
  async function load() {
    const data = await responseJSON(await fetch('/api/document'));
    state.document = data.document; state.revision = data.revision;
    if (data.document.version < 6) { state.nodes = []; $('compositeNode').replaceChildren(); throw new Error('Compositing requires a shared v6 document. Run migrate --target 6 first.'); }
    const previous = page();
    $('compositePage').replaceChildren(...data.document.pages.map(p => new Option(p.name || p.id, p.id)));
    if (data.document.pages.some(p => p.id === previous)) $('compositePage').value = previous;
    loadNodes(); notify('Document loaded. Changes commit as one undoable Rust transaction.');
  }
  function loadNodes() {
    const previous = $('compositeNode').value; state.nodes = [];
    for (const layer of state.document.pages.find(p => p.id === page()).layers) collect(layer.nodes);
    $('compositeNode').replaceChildren(...state.nodes.map(node => new Option(`${node.id} · ${node.kind}`, node.id)));
    if (state.nodes.some(n => n.id === previous)) $('compositeNode').value = previous;
    $('compositeMask').replaceChildren(...Object.keys(state.document.masks || {}).sort().map(name => new Option(name, name)));
    sync();
  }
  function sync() {
    const node = current(); if (!node) { $('compositeStacks').textContent = 'No nodes. Add a fill or adjustment using the operation form.'; return; }
    $('compositeId').value = node.id; $('compositeBlend').value = node.blend_mode || 'normal';
    $('compositeSpace').value = node.blend_space || 'srgb'; $('compositeOpacity').value = node.opacity ?? 1;
    $('compositeContent').value = node.content_opacity ?? 1; $('compositeContent').disabled = node.kind === 'adjustment';
    $('compositeIsolation').value = node.isolation || 'isolated'; $('compositeIsolation').disabled = node.kind !== 'group';
    $('compositeStacks').textContent = JSON.stringify({ adjustment: node.adjustment, params: node.params, scope: node.scope, fill: node.fill, effects: node.effects || [], transforms: node.transforms || [], mask: node.mask, clipping: node.clipping }, null, 2);
  }
  async function commit(operations) {
    if (!state.document || state.document.version < 6) throw new Error('Load a shared v6 document first.');
    await responseJSON(await fetch('/api/scene', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ page: page(), revision: state.revision, operations }) }));
    await load(); document.dispatchEvent(new Event('pentool-composite-changed')); await preview(false); notify('Changes saved. Undo is available in document history.');
  }
  async function preview(before) {
    if (!state.document) throw new Error('Load a document first.');
    cancel(); state.controller = new AbortController(); const generation = ++state.generation; state.job = `panel-${crypto.randomUUID()}`;
    const doc = structuredClone(state.document);
    if (before) {
      const strip = value => { if (!value || typeof value !== 'object') return; if ('enabled' in value) value.enabled = false; delete value.mask; for (const [key, child] of Object.entries(value)) if (key !== 'mask_resources') strip(child); };
      strip(doc);
    }
    const response = await fetch(`/api/render/svg?page=${encodeURIComponent(page())}&max_edge=1024&job=${state.job}`, { method: 'POST', signal: state.controller.signal, headers: { 'content-type': 'application/json' }, body: JSON.stringify(doc) });
    if (!response.ok) { await responseJSON(response); return; }
    const svg = new DOMParser().parseFromString(await response.text(), 'image/svg+xml').documentElement;
    if (generation !== state.generation) return;
    $('canvas').setAttribute('viewBox',svg.getAttribute('viewBox')); $('canvas').replaceChildren(...[...svg.childNodes].map(n => document.importNode(n,true)));
    notify(`${before ? 'Before' : 'After'} · approximate 1024 px proxy. Export remains authoritative.`);
  }
  const form = (id, action) => { $(id).onsubmit = event => { event.preventDefault(); run(action); }; };
  form('compositeAppearance', () => { const node = requiredNode(); const settings = { blend_mode: $('compositeBlend').value, blend_space: $('compositeSpace').value, opacity: Number($('compositeOpacity').value) }; if (node.kind !== 'adjustment') settings.content_opacity = Number($('compositeContent').value); if (node.kind === 'group') settings.isolation = $('compositeIsolation').value; return commit([{ type: 'composite-set', id: node.id, settings }]); });
  form('compositeOperation', () => {
    const type = $('compositeOperationKind').value; const settings = JSON.parse($('compositeSettings').value);
    const operation = { type, id: $('compositeId').value, op_id: $('compositeOpId').value, settings };
    if (type.startsWith('transform-')) Object.assign(operation, settings);
    if (type.startsWith('mask-')) { operation.name = operation.id; operation.from = settings.from; }
    if (type.startsWith('selection-')) { operation.name = operation.id; operation.query = settings.query; }
    if (type === 'node-move') operation.index = settings.index;
    return commit([operation]);
  });
  form('compositeMaskForm', () => commit([{ type: 'mask-attach', id: requiredNode().id, name: $('compositeMask').value, settings: { density: Number($('compositeDensity').value), feather: Number($('compositeFeather').value), invert: $('compositeInvert').checked, linked: $('compositeLinked').checked } }]));
  $('compositeDetach').onclick = () => run(() => commit([{ type:'mask-detach',id:requiredNode().id }]));
  form('compositeClipForm', () => commit([{ type:'clip-add',id:requiredNode().id,base:$('compositeBase').value }]));
  $('compositeUnclip').onclick = () => run(() => commit([{ type:'clip-remove',id:requiredNode().id }]));
  form('compositeAnalyzeForm', async () => {
    const query = $('compositeQuery').value.trim();
    const result = await responseJSON(await fetch('/api/composite/analyze', { method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({ document:state.document,page:page(),scope:$('compositeScope').value,query:query ? JSON.parse(query) : null,compare:true }) }));
    const histogram = result.analysis.histogram.luminosity; const max = Math.max(1,...histogram);
    const path = document.createElementNS('http://www.w3.org/2000/svg','path'); path.setAttribute('d',histogram.map((v,i) => `M${i},80V${80-v/max*78}`).join('')); path.setAttribute('stroke','currentColor'); $('compositeHistogram').replaceChildren(path);
    const { histogram: _, ...summary } = result.analysis; $('compositeAnalysis').textContent = JSON.stringify({ ...summary,comparison:result.comparison },null,2); notify('Analysis complete. The document was not modified.');
  });
  $('compositeDependencies').onclick = () => run(async () => { const report = await responseJSON(await fetch('/api/composite/dependencies')); $('compositeDependencyResult').textContent=JSON.stringify(report,null,2); notify(report.portable ? 'All dependencies are portable and verified.' : 'Missing or changed links found. Relink with verified bytes.'); });
  form('compositeLinkedForm',() => commit([{type:`linked-${$('compositeAssetAction').value}`,asset:$('compositeAssetDigest').value,path:$('compositeAssetPath').value || undefined}]));
  $('compositeRefresh').onclick = () => run(load); $('compositePage').onchange = loadNodes; $('compositeNode').onchange = sync;
  $('compositeBefore').onclick = () => run(() => preview(true)); $('compositeAfter').onclick = () => run(() => preview(false));
  $('compositeCancel').onclick = cancel;
  document.addEventListener('pentool-shared-loaded', () => run(load));
  run(load);
})();
