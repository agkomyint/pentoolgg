// Image operation panel. Every committed change goes through the shared Rust
// services (/api/scene batch operations and /api/image/bake), never client-side.
(() => {
  const $ = (id) => document.getElementById(id);
  const KINDS = ['crop', 'resize', 'rotate', 'brightness-contrast', 'levels', 'curves', 'hue-saturation', 'blur', 'sharpen', 'grayscale'];
  const state = { doc: null, revision: null, images: [], shared: false };
  let comparing = false;

  const collect = (nodes, out, page) => {
    for (const node of nodes || []) {
      if (node.kind === 'image') out.push(page ? {...node, _page: page, _value: JSON.stringify([page,node.id])} : node);
      collect(node.children, out, page);
    }
    return out;
  };
  const notify = (message) => { $('imageStatus').textContent = message; };
  const current = () => state.images.find((n) => n._value === $('imageSelect').value);

  async function load() {
    const response = await fetch('/api/document');
    if (!response.ok) return state.doc ? loadDocument({document:state.doc,revision:state.revision,shared:state.shared}) : notify('Start a document to add an image.');
    const data = await response.json();
    loadDocument({...data,shared:true});
  }
  function loadDocument(data) {
    state.doc = data.document;
    state.revision = data.revision;
    state.shared = data.shared;
    $('imageBake').disabled = !state.shared;
    $('imageBake').title = state.shared ? 'Bake through document history' : 'Save .pen and serve that file before baking';
    state.images = [];
    $('imageEditing').hidden = true;
    loadImportTargets();
    if (![5,6,7].includes(state.doc.version)) { $('imageSelect').replaceChildren(); return notify('Add an image above. Allow the v5 upgrade to enable raster images.'); }
    for (const page of state.doc.pages) for (const layer of page.layers) collect(layer.nodes, state.images, page.id);
    $('imageEditing').hidden = !state.images.length;
    const select = $('imageSelect');
    const previous = select.value;
    select.replaceChildren(...state.images.map((n) => new Option(`${n._page} / ${n.id} (${n.asset.slice(0, 15)}…)`, n._value)));
    if (state.images.some((n) => n._value === previous)) select.value = previous;
    notify(state.images.length ? `${state.images.length} image(s). Source pixels are never modified.` : 'No images yet. Choose a file above and select Add image.');
    renderOps();
    syncFrame();
  }

  function importPages() {
    return state.doc?.pages || [{ id:'page-1', layers:state.doc?.layers || [] }];
  }
  function loadImportTargets() {
    const pages = importPages();
    const previous = $('imageAddPage').value;
    $('imageAddPage').replaceChildren(...pages.map(p => new Option(p.name || p.id,p.id)));
    if (pages.some(p => p.id === previous)) $('imageAddPage').value = previous;
    loadImportLayers();
    $('imageMigrationLabel').hidden = state.doc.version >= 5;
    $('imageAddSubmit').disabled = false;
    $('imageAddStatus').textContent = 'Choose a file to add an undoable image layer.';
  }
  function loadImportLayers() {
    const previous = $('imageAddLayer').value;
    const layers = importPages().find(p => p.id === $('imageAddPage').value)?.layers || [];
    $('imageAddLayer').replaceChildren(...layers.map(layer => {
      const option = new Option(`${layer.name || layer.id}${layer.locked ? ' (locked)' : ''}`,layer.id);
      option.disabled = layer.locked === true;
      return option;
    }));
    if (layers.some(l => l.id === previous && !l.locked)) $('imageAddLayer').value = previous;
    else $('imageAddLayer').value = layers.find(l => !l.locked)?.id || '';
  }
  $('imageAddPage').onchange = loadImportLayers;
  $('imageAddForm').onsubmit = async event => {
    event.preventDefault();
    const button = $('imageAddSubmit');
    if (button.disabled) return;
    const status = $('imageAddStatus');
    const input = $('imageFile');
    try {
      if (!state.doc) throw new Error('Serve a .pen document first, then reload.');
      const file = input.files[0];
      if (!file) throw new Error('Choose a PNG, JPEG or WebP file.');
      if (file.size > 32 * 1024 * 1024) throw new Error('Choose an image smaller than 32 MiB.');
      if (state.doc.version < 5 && !$('imageAllowMigration').checked) throw new Error('Allow upgrading to v5 below, or migrate this document with the CLI first.');
      button.disabled = true; button.textContent = 'Adding image…';
      $('imageAddForm').setAttribute('aria-busy','true');
      status.textContent = 'Validating and embedding the image locally…';
      input.removeAttribute('aria-invalid');
      const data = await new Promise((resolve,reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result).split(',')[1]);
        reader.onerror = () => reject(new Error('Could not read the file. Choose it again.'));
        reader.readAsDataURL(file);
      });
      const id = $('imageAddId').value.trim();
      const page = $('imageAddPage').value;
      const response = await fetch('/api/image/add',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({id,page,layer:$('imageAddLayer').value,data,revision:state.revision || '',migrate:$('imageAllowMigration').checked,...(!state.shared ? {document:state.doc} : {})})});
      const result = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(result.error || 'Import failed; reload the shared document and retry.');
      if (result.local) document.dispatchEvent(new CustomEvent('pentool-local-document',{detail:{document:result.document,page}}));
      else await load();
      $('imageSelect').value = JSON.stringify([page,id]); renderOps(); syncFrame();
      input.value = '';
      status.textContent = `Added ${id}. Source pixels are unchanged. Undo restores the document.`;
      if (!result.local) document.dispatchEvent(new Event('pentool-composite-changed'));
    } catch (error) {
      status.textContent = `${error.message} Nothing was changed.`;
      input.setAttribute('aria-invalid','true');
    } finally {
      button.disabled = false; button.textContent = 'Add image';
      $('imageAddForm').removeAttribute('aria-busy');
    }
  };

  function syncFrame() {
    const node = current();
    if (!node) return;
    $('imageFit').value = node.fit;
    [$('imageFocalX').value, $('imageFocalY').value] = node.position || [0.5, 0.5];
    const crop = node.crop || [0, 0, 1, 1];
    $('imageCropX').value = crop[0]; $('imageCropY').value = crop[1];
    $('imageCropW').value = crop[2]; $('imageCropH').value = crop[3];
    $('imageOpacity').value = node.opacity ?? 1;
    drawHandles();
  }

  function button(text, onclick, disabled = false) {
    const b = document.createElement('button');
    b.textContent = text;
    b.disabled = disabled;
    b.onclick = onclick;
    return b;
  }

  function renderOps() {
    const root = $('imageOps');
    root.replaceChildren();
    const node = current();
    if (!node) return;
    if (!state.doc.image_assets?.[node.asset]) {
      const p = document.createElement('p');
      p.textContent = 'Source missing. Re-add the file with `pentool image add`; the operation stack is preserved.';
      root.append(p);
    }
    (node.operations || []).forEach((op, index, all) => {
      const row = document.createElement('div');
      const toggle = document.createElement('input');
      toggle.type = 'checkbox';
      toggle.checked = op.enabled !== false;
      toggle.title = 'Enable operation';
      toggle.onchange = () => commit([{ type: toggle.checked ? 'image-op-enable' : 'image-op-disable', id: node.id, op_id: op.id }]);
      const label = document.createElement('span');
      label.textContent = ` ${index + 1}. ${op.kind} ${JSON.stringify(op.params || {})} `;
      row.append(
        toggle,
        label,
        button('↑', () => commit([{ type: 'image-op-move', id: node.id, op_id: op.id, index: index - 1 }]), index === 0),
        button('↓', () => commit([{ type: 'image-op-move', id: node.id, op_id: op.id, index: index + 1 }]), index === all.length - 1),
        button('×', () => commit([{ type: 'image-op-remove', id: node.id, op_id: op.id }])),
      );
      root.append(row);
    });
  }

  async function commit(operations) {
    const response = await fetch('/api/scene', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ operations, page: current()?._page, revision: state.revision,...(!state.shared ? {document:state.doc} : {}) }),
    });
    const result = await response.json().catch(() => ({}));
    if (!response.ok) return notify(result.error || 'The operation was rejected; nothing was changed.');
    if (result.local) document.dispatchEvent(new CustomEvent('pentool-local-document',{detail:{document:result.document,page:current()?._page}}));
    else {await load();document.dispatchEvent(new Event('pentool-composite-changed'));}
    document.getElementById('historyRefresh')?.click();
  }

  async function setPreview(stripped) {
    comparing = stripped;
    const node = current();
    if (!node || !state.doc) return;
    const doc = structuredClone(state.doc);
    if (stripped) {
      for (const page of doc.pages) for (const layer of page.layers) {
        if (page.id !== node._page) continue;
        const strip = (nodes) => { for (const n of nodes || []) { if (n.id === node.id) n.operations = []; strip(n.children); } };
        strip(layer.nodes);
      }
    }
    const response = await fetch(`/api/render/svg?max_edge=1024&page=${encodeURIComponent(node._page)}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(doc),
    });
    if (!response.ok) return notify('Preview failed; the authoritative document is unchanged.');
    const parsed = new DOMParser().parseFromString(await response.text(), 'image/svg+xml').documentElement;
    $('canvas').replaceChildren(...[...parsed.childNodes].map((n) => document.importNode(n, true)));
    $('imagePreviewStatus').textContent = stripped
      ? 'Preview: BEFORE (approximate proxy, operations hidden)'
      : 'Preview: AFTER (approximate proxy; export is authoritative)';
  }

  $('imageSelect').onchange = () => { renderOps(); syncFrame(); };
  $('imageApplyFrame').onclick = () => {
    const node = current();
    if (!node) return notify('Select an image first.');
    const n = (id) => Number($(id).value);
    commit([{
      type: 'set-image', id: node.id, fit: $('imageFit').value, opacity: n('imageOpacity'),
      position: [n('imageFocalX'), n('imageFocalY')],
      crop: [n('imageCropX'), n('imageCropY'), n('imageCropW'), n('imageCropH')],
    }]);
  };
  $('imageOpKind').replaceChildren(...KINDS.map((k) => new Option(k, k)));
  $('imageOpAdd').onclick = () => {
    const node = current();
    if (!node) return notify('Select an image first.');
    let params;
    try { params = JSON.parse($('imageOpParams').value || '{}'); } catch { return notify('Parameters must be valid JSON.'); }
    commit([{ type: 'image-op-add', id: node.id, op: $('imageOpKind').value, params }]);
  };
  $('imageBeforeAfter').onclick = () => setPreview(!comparing);
  $('imageReset').onclick = () => {
    const node = current();
    if (!node || !(node.operations || []).length) return;
    if (!confirm(`Remove all ${node.operations.length} operations from ${node.id}? Undo restores them.`)) return;
    commit(node.operations.map((op) => ({ type: 'image-op-remove', id: node.id, op_id: op.id })));
  };
  $('imageBake').onclick = async () => {
    const node = current();
    if (!node) return;
    const post = (extra) => fetch('/api/image/bake', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ id: node.id, page: node._page, revision: state.revision, ...extra }),
    });
    const plan = await post({ dry_run: true });
    const predicted = await plan.json().catch(() => ({}));
    if (!plan.ok) return notify(predicted.error || 'Bake is not possible.');
    if (!confirm(`Bake ${node.id} into a new PNG?\n${JSON.stringify(predicted.result).slice(0, 300)}\nUndo restores the operation stack.`)) return;
    const done = await post({});
    if (!done.ok) return notify((await done.json().catch(() => ({}))).error || 'Bake failed; nothing changed.');
    await load();
  };

  // Direct-manipulation handles drawn over the frame: focal dot, crop move/resize.
  const NS = 'http://www.w3.org/2000/svg';
  let overlayBusy = false;
  const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));

  function drawHandles() {
    const canvas = $('canvas');
    const node = current();
    canvas.querySelector('#imageHandles')?.remove();
    if (!node || !canvas.viewBox?.baseVal?.width) return;
    const g = document.createElementNS(NS, 'g');
    g.id = 'imageHandles';
    const crop = [Number($('imageCropX').value), Number($('imageCropY').value), Number($('imageCropW').value), Number($('imageCropH').value)];
    const focal = [Number($('imageFocalX').value), Number($('imageFocalY').value)];
    const unit = canvas.viewBox.baseVal.width / Math.max(1, canvas.getBoundingClientRect().width);
    const make = (tag, attrs) => {
      const el = document.createElementNS(NS, tag);
      for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
      g.append(el);
      return el;
    };
    const rect = make('rect', {
      x: node.x + crop[0] * node.width, y: node.y + crop[1] * node.height,
      width: crop[2] * node.width, height: crop[3] * node.height,
      fill: 'none', stroke: '#2563eb', 'stroke-width': unit, 'stroke-dasharray': `${4 * unit} ${3 * unit}`, style: 'cursor:move',
      'pointer-events': 'all',
    });
    const corner = make('rect', {
      x: node.x + (crop[0] + crop[2]) * node.width - 4 * unit, y: node.y + (crop[1] + crop[3]) * node.height - 4 * unit,
      width: 8 * unit, height: 8 * unit, fill: '#fff', stroke: '#2563eb', 'stroke-width': unit, style: 'cursor:nwse-resize',
    });
    const dot = make('circle', {
      cx: node.x + focal[0] * node.width, cy: node.y + focal[1] * node.height, r: 5 * unit,
      fill: '#f59e0b', stroke: '#fff', 'stroke-width': unit, style: 'cursor:crosshair',
    });
    const toUnit = (e) => {
      const box = canvas.getBoundingClientRect();
      return [
        clamp((((e.clientX - box.left) * canvas.viewBox.baseVal.width) / box.width - node.x) / node.width, 0, 1),
        clamp((((e.clientY - box.top) * canvas.viewBox.baseVal.height) / box.height - node.y) / node.height, 0, 1),
      ];
    };
    const drag = (el, update) => {
      el.addEventListener('pointerdown', (down) => {
        down.preventDefault();
        down.stopPropagation();
        el.setPointerCapture(down.pointerId);
        const start = toUnit(down);
        const origin = [...crop];
        const move = (e) => { update(toUnit(e), start, origin); overlayBusy = true; drawHandles(); overlayBusy = false; };
        const up = () => {
          el.removeEventListener('pointermove', move);
          el.removeEventListener('pointerup', up);
          $('imageApplyFrame').click();
        };
        el.addEventListener('pointermove', move);
        el.addEventListener('pointerup', up);
      });
    };
    drag(dot, ([x, y]) => { $('imageFocalX').value = x; $('imageFocalY').value = y; });
    drag(rect, ([x, y], [sx, sy], [cx, cy, cw, ch]) => {
      $('imageCropX').value = clamp(cx + x - sx, 0, 1 - cw);
      $('imageCropY').value = clamp(cy + y - sy, 0, 1 - ch);
    });
    drag(corner, ([x, y], _s, [cx, cy]) => {
      $('imageCropW').value = clamp(x - cx, 0.01, 1 - cx);
      $('imageCropH').value = clamp(y - cy, 0.01, 1 - cy);
    });
    canvas.append(g);
  }

  new MutationObserver(() => { if (!overlayBusy && !$('canvas').querySelector('#imageHandles')) drawHandles(); })
    .observe($('canvas'), { childList: true });

  document.addEventListener('pentool-document-changed',event => loadDocument(event.detail));
  document.dispatchEvent(new Event('pentool-request-document'));
})();
