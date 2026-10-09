// Photographer studio: grid/filmstrip, loupe, compare, culling, histogram,
// clipping/gamut/mask overlays, develop panels, masks, crop and copy/sync.
// No develop math or document writes in JS: every preview is rendered by the
// Rust pipeline and every edit is one guarded Rust transaction (/api/photo/*).
(() => {
  const $ = id => document.getElementById(id);
  const studio = $('photoStudio');
  if (!studio) return;
  const p3 = matchMedia('(color-gamut: p3)').matches;
  const state = {
    photos: [], selected: [], current: null, detail: null, revision: null,
    view: 'grid', overlay: 'none', crop: null, cropping: false, brush: null,
    copied: null, generation: 0, thumbs: new Map(), busy: false,
  };
  const SLIDERS = [
    ['Tone', [['tone.exposure', 'Exposure', -5, 5, 0.05], ['tone.contrast', 'Contrast', -100, 100, 1], ['tone.highlights', 'Highlights', -100, 100, 1], ['tone.shadows', 'Shadows', -100, 100, 1], ['tone.whites', 'Whites', -100, 100, 1], ['tone.blacks', 'Blacks', -100, 100, 1]]],
    ['Presence', [['presence.texture', 'Texture', -100, 100, 1], ['presence.clarity', 'Clarity', -100, 100, 1], ['presence.dehaze', 'Dehaze', -100, 100, 1], ['presence.vibrance', 'Vibrance', -100, 100, 1], ['presence.saturation', 'Saturation', -100, 100, 1]]],
    ['Detail', [['detail.sharpening.amount', 'Sharpening', 0, 150, 1], ['detail.noise.luminance', 'Luminance NR', 0, 100, 1], ['detail.noise.color', 'Color NR', 0, 100, 1]]],
    ['Geometry', [['geometry.rotate', 'Rotate', -45, 45, 0.1], ['geometry.vertical', 'Vertical', -100, 100, 1], ['geometry.horizontal', 'Horizontal', -100, 100, 1], ['lens.distortion', 'Distortion', -100, 100, 1]]],
    ['Effects', [['effects.vignette.amount', 'Vignette', -100, 100, 1], ['effects.grain.amount', 'Grain', 0, 100, 1]]],
  ];
  const LABELS = ['none', 'red', 'yellow', 'green', 'blue', 'purple'];
  const GROUPS = ['white_balance', 'tone', 'presence', 'curves', 'hsl', 'grading', 'monochrome', 'detail', 'lens', 'geometry', 'crop', 'effects', 'calibration', 'local'];
  const el = (tag, props = {}, ...children) => { const node = Object.assign(document.createElement(tag), props); node.append(...children); return node; };
  const notify = message => { $('photoStatus').textContent = message; };
  const variant = () => $('photoVariant').value || 'master';
  const get = (object, path) => path.split('.').reduce((value, key) => value?.[key], object);

  async function json(response) {
    const value = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(value.error || `Request failed (${response.status}).`);
    return value;
  }
  const post = (url, body) => fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }).then(json);
  async function run(action) {
    if (state.busy) return;
    state.busy = true; studio.setAttribute('aria-busy', 'true');
    try { await action(); } catch (error) { notify(`${error.message} Nothing was changed; adjust and retry.`); }
    finally { state.busy = false; studio.removeAttribute('aria-busy'); }
  }

  // Every edit carries the revision last read, so a concurrent CLI or agent
  // change is refused instead of overwritten.
  async function edit(op, message) {
    const result = await post('/api/photo/edit', { edit: op, revision: state.revision });
    state.revision = result.revision;
    notify(message || 'Saved. Undo restores the previous settings.');
    await refresh();
    return result;
  }

  async function loadCatalog() {
    const query = encodeURIComponent($('photoQuery').value.trim());
    const data = await json(await fetch(`/api/photo/catalog?query=${query}&limit=500`));
    state.photos = data.photos; state.revision = data.revision;
    state.selected = state.selected.filter(id => state.photos.some(p => p.id === id));
    if (!state.photos.some(p => p.id === state.current)) state.current = state.photos[0]?.id ?? null;
    if (state.current && !state.selected.length) state.selected = [state.current];
    renderGrid();
    $('photoCount').textContent = `${data.matches} photo${data.matches === 1 ? '' : 's'}${data.has_more ? ' (first 500)' : ''}`;
    if (state.current) await loadDetail(); else { state.detail = null; $('photoLoupe').replaceChildren(el('p', { className: 'text-help', textContent: 'No photos match. Add DNGs with `pentool raw add`.' })); }
  }

  async function loadDetail() {
    const data = await json(await fetch(`/api/photo/detail?photo=${encodeURIComponent(state.current)}`));
    state.detail = data; state.revision = data.revision;
    const variants = data.photo.variants.map(v => v.id);
    const keep = [variant(), $('photoCompareB').value];
    for (const [select, wanted] of [[$('photoVariant'), keep[0]], [$('photoCompareB'), keep[1]]]) {
      select.replaceChildren(...data.photo.variants.map(v => new Option(v.name ? `${v.name} (${v.id})` : v.id, v.id)));
      select.value = variants.includes(wanted) ? wanted : variants[0];
    }
    $('photoSnapshots').replaceChildren(...(data.photo.snapshots || []).map(s => new Option(s.name ? `${s.name} (${s.id})` : s.id, s.id)));
    $('photoKeywords').textContent = (data.photo.keywords || []).join(', ') || 'No keywords';
    renderPanels(); renderMasks();
    await renderLoupe();
  }

  async function refresh() {
    state.thumbs.delete(state.current);
    await loadCatalog();
  }

  // Previews are requested from Rust and tagged for the chosen space.
  async function preview(photo, variantId, edge, overlay = 'none', uncropped = false) {
    const space = $('photoSpace').value;
    const data = await post('/api/photo/preview', { photo, variant: variantId, edge, space, overlay, uncropped });
    return { url: `data:image/png;base64,${data.png}`, report: data.report };
  }

  function stars(rating) { return '★'.repeat(rating) + '☆'.repeat(5 - rating); }
  function renderGrid() {
    const grid = $('photoGrid');
    grid.replaceChildren(...state.photos.map(photo => {
      const image = el('img', { alt: photo.id, loading: 'lazy' });
      const cell = el('button', { className: `photo-cell${state.selected.includes(photo.id) ? ' selected' : ''}${photo.id === state.current ? ' current' : ''}${photo.pick === 'reject' ? ' rejected' : ''}`, title: `${photo.id} · ${photo.width}×${photo.height}` },
        image, el('span', { className: 'photo-meta' }, el('span', { textContent: stars(photo.rating) }), el('span', { className: `photo-label label-${photo.label}`, textContent: photo.pick === 'pick' ? '⚑' : photo.pick === 'reject' ? '✕' : '' })),
        el('span', { className: 'photo-id', textContent: photo.id }));
      cell.dataset.id = photo.id;
      cell.setAttribute('aria-pressed', String(state.selected.includes(photo.id)));
      cell.addEventListener('click', event => select(photo.id, event));
      cell.addEventListener('dblclick', () => setView('loupe'));
      thumbnail(photo.id, image);
      return cell;
    }));
  }

  // Thumbnails are developed one at a time and cached until an edit.
  const queue = [];
  let draining = false;
  function thumbnail(id, image) {
    if (state.thumbs.has(id)) { image.src = state.thumbs.get(id); return; }
    queue.push([id, image]); drain();
  }
  async function drain() {
    if (draining) return; draining = true;
    while (queue.length) {
      const [id, image] = queue.shift();
      try {
        if (!state.thumbs.has(id)) state.thumbs.set(id, (await preview(id, 'master', 320)).url);
        image.src = state.thumbs.get(id);
      } catch (error) { image.alt = `${id}: ${error.message}`; }
    }
    draining = false;
  }

  function select(id, event) {
    if (event?.shiftKey && state.current) {
      const ids = state.photos.map(p => p.id);
      const [a, b] = [ids.indexOf(state.current), ids.indexOf(id)].sort((x, y) => x - y);
      state.selected = ids.slice(a, b + 1);
    } else if (event?.ctrlKey || event?.metaKey) {
      state.selected = state.selected.includes(id) ? state.selected.filter(x => x !== id) : [...state.selected, id];
    } else state.selected = [id];
    state.current = id;
    run(loadDetail).then(renderGrid);
  }

  function setView(view) {
    state.view = view;
    studio.dataset.view = view;
    for (const button of studio.querySelectorAll('[data-view]')) button.setAttribute('aria-pressed', String(button.dataset.view === view));
    if (view !== 'grid') run(renderLoupe);
  }

  async function renderLoupe() {
    if (!state.current || state.view === 'grid') return;
    const generation = ++state.generation;
    const loupe = $('photoLoupe');
    const edge = Math.min(4096, Math.ceil(Math.max(loupe.clientWidth, loupe.clientHeight) * devicePixelRatio) || 1600);
    const uncropped = state.cropping || !!state.brush;
    const overlay = state.brush && $('photoMaskOverlay').checked ? `mask:${state.brush}` : state.overlay;
    notify('Developing…');
    if (state.view === 'compare') {
      const half = Math.ceil(edge / 2);
      const [a, b] = await Promise.all([preview(state.current, variant(), half, state.overlay), preview(state.current, $('photoCompareB').value, half, state.overlay)]);
      if (generation !== state.generation) return;
      loupe.replaceChildren(figure(a, variant()), figure(b, $('photoCompareB').value));
      histogram(a.report);
    } else {
      const shot = await preview(state.current, variant(), edge, overlay, uncropped);
      if (generation !== state.generation) return;
      const image = el('img', { src: shot.url, alt: `${state.current} ${variant()}`, draggable: false });
      const canvas = el('canvas', { className: 'photo-draw' });
      const frame = el('div', { className: 'photo-frame' }, image, canvas);
      loupe.replaceChildren(frame);
      image.addEventListener('load', () => {
        // Fit the loupe (enlarging small previews) and size the drawing layer to match.
        const scale = Math.min((loupe.clientWidth - 16) / image.naturalWidth, (loupe.clientHeight - 16) / image.naturalHeight);
        image.style.width = `${Math.max(1, Math.floor(image.naturalWidth * scale))}px`;
        image.style.height = `${Math.max(1, Math.floor(image.naturalHeight * scale))}px`;
        canvas.width = image.clientWidth; canvas.height = image.clientHeight; drawCrop(canvas);
      });
      attachPointer(canvas);
      histogram(shot.report);
    }
    notify(`${state.current} · ${variant()} · ${$('photoSpace').selectedOptions[0].text}`);
  }
  function figure(shot, name) { return el('figure', { className: 'photo-compare' }, el('img', { src: shot.url, alt: name }), el('figcaption', { textContent: name })); }

  // Histogram and clipping indicators come from the preview's stage-11 report.
  function histogram(report) {
    const canvas = $('photoHistogram'), context = canvas.getContext('2d');
    const { r, g, b, bins } = report.delivery.histogram;
    const peak = Math.max(1, ...r.slice(1, -1), ...g.slice(1, -1), ...b.slice(1, -1));
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.globalCompositeOperation = 'lighter';
    for (const [values, color] of [[r, '#ff4040'], [g, '#40ff40'], [b, '#4080ff']]) {
      context.fillStyle = color;
      values.forEach((count, i) => {
        const height = Math.min(1, count / peak) * canvas.height;
        context.fillRect(i * canvas.width / bins, canvas.height - height, canvas.width / bins, height);
      });
    }
    context.globalCompositeOperation = 'source-over';
    $('photoClipHigh').classList.toggle('active', report.highlight_clipped_pixels > 0);
    $('photoClipLow').classList.toggle('active', report.shadow_clipped_pixels > 0);
    $('photoClipHigh').title = `${report.highlight_clipped_pixels} pixels with a channel at maximum`;
    $('photoClipLow').title = `${report.shadow_clipped_pixels} pixels with a channel at zero`;
    $('photoGamutInfo').textContent = `${report.delivery.out_of_gamut_pixels} of ${report.delivery.pixels} pixels outside ${report.delivery.space} (mapped perceptually) · ${report.width}×${report.height} of ${report.developed_width}×${report.developed_height}`;
  }

  function renderPanels() {
    const develop = state.detail.photo.variants.find(v => v.id === variant())?.develop || {};
    const host = $('photoDevelop');
    host.replaceChildren(...SLIDERS.map(([title, sliders]) => el('details', { open: title === 'Tone' }, el('summary', { textContent: title }),
      ...sliders.map(([path, label, min, max, step]) => {
        const value = get(develop, path) ?? 0;
        const output = el('output', { textContent: value });
        const input = el('input', { type: 'range', min, max, step, value, title: 'Double-click to reset' });
        input.addEventListener('input', () => { output.textContent = input.value; });
        input.addEventListener('change', () => {
          const set = { [path]: Number(input.value) };
          // Grain needs a seed so it is reproducible; keep any existing one.
          if (path === 'effects.grain.amount' && get(develop, 'effects.grain.seed') === undefined) set['effects.grain.seed'] = 1;
          run(() => edit({ op: 'develop', photo: state.current, variant: variant(), set }, `${label} set to ${input.value}.`));
        });
        input.addEventListener('dblclick', () => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), unset: [path] }, `${label} reset.`)));
        return el('label', { className: 'photo-slider' }, el('span', { textContent: label }), input, output);
      }))));
    const wb = develop.white_balance || {};
    $('photoTemperature').value = wb.temperature ?? 5500;
    $('photoTint').value = wb.tint ?? 0;
    const crop = develop.crop?.rect;
    $('photoCropInfo').textContent = crop ? `Crop ${crop.map(v => Number(v).toFixed(3)).join(', ')}` : 'No crop';
  }

  function renderMasks() {
    const develop = state.detail.photo.variants.find(v => v.id === variant())?.develop || {};
    const list = develop.local || [];
    $('photoMaskList').replaceChildren(...list.map(adjustment => {
      const exposure = el('input', { type: 'range', min: -4, max: 4, step: 0.05, value: adjustment.params?.exposure ?? 0 });
      const enabled = el('input', { type: 'checkbox', checked: adjustment.enabled !== false });
      const show = el('button', { type: 'button', textContent: state.brush === adjustment.id ? 'Stop editing' : 'Edit / paint' });
      const replace = changes => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), set: { local: list.map(a => a.id === adjustment.id ? { ...a, ...changes(a) } : a) } }));
      exposure.addEventListener('change', () => replace(a => ({ params: { ...a.params, exposure: Number(exposure.value) } })));
      enabled.addEventListener('change', () => replace(() => ({ enabled: enabled.checked })));
      show.addEventListener('click', () => { state.brush = state.brush === adjustment.id ? null : adjustment.id; state.cropping = false; renderMasks(); run(renderLoupe); });
      const remove = el('button', { type: 'button', textContent: 'Delete' });
      remove.addEventListener('click', () => { if (state.brush === adjustment.id) state.brush = null; run(() => edit({ op: 'develop', photo: state.current, variant: variant(), set: { local: list.filter(a => a.id !== adjustment.id) } }, `Removed ${adjustment.id}.`)); });
      return el('div', { className: 'photo-mask' }, el('strong', { textContent: `${adjustment.name || adjustment.id} · ${adjustment.mask.components.map(c => c.kind).join(' + ')}` }),
        el('label', {}, enabled, ' Enabled'), el('label', { className: 'photo-slider' }, el('span', { textContent: 'Exposure' }), exposure), el('div', { className: 'photo-row' }, show, remove));
    }));
    $('photoBrushHint').hidden = !state.brush;
  }

  function newMask(kind) {
    const develop = state.detail.photo.variants.find(v => v.id === variant())?.develop || {};
    const list = develop.local || [];
    let n = list.length + 1;
    while (list.some(a => a.id === `${kind}-${n}`)) n++;
    const id = `${kind}-${n}`;
    const exposure = Number($('photoMaskExposure').value);
    if (kind === 'brush') { state.brush = id; state.pendingBrush = { id, params: { exposure } }; state.cropping = false; notify(`Paint on the photo to create ${id}.`); run(renderLoupe); return; }
    const component = kind === 'radial'
      ? { kind: 'radial', mode: 'add', center: [0.5, 0.5], radius: [0.25, 0.25], feather: 50 }
      : { kind: 'linear', mode: 'add', start: [0.5, 0], end: [0.5, 0.5] };
    run(() => edit({ op: 'develop', photo: state.current, variant: variant(), set: { local: [...list, { id, mask: { components: [component] }, params: { exposure } }] } }, `Added ${id}.`));
  }

  // Crop and brush strokes are drawn on the uncropped frame, normalized 0–1.
  function attachPointer(canvas) {
    let points = null, start = null;
    const at = event => { const box = canvas.getBoundingClientRect(); return [Math.min(1, Math.max(0, (event.clientX - box.left) / box.width)), Math.min(1, Math.max(0, (event.clientY - box.top) / box.height))]; };
    canvas.addEventListener('pointerdown', event => {
      if (!state.cropping && !state.brush) return;
      canvas.setPointerCapture(event.pointerId);
      if (state.cropping) start = at(event); else points = [at(event)];
    });
    canvas.addEventListener('pointermove', event => {
      if (start) { const [x, y] = at(event); state.crop = [Math.min(start[0], x), Math.min(start[1], y), Math.abs(x - start[0]), Math.abs(y - start[1])]; constrain(); drawCrop(canvas); }
      if (points) { const p = at(event); points.push(p); const context = canvas.getContext('2d'); context.fillStyle = 'rgba(255,64,64,0.5)'; const r = Number($('photoBrushSize').value) / 100 * Math.max(canvas.width, canvas.height) / 2; context.beginPath(); context.arc(p[0] * canvas.width, p[1] * canvas.height, r, 0, Math.PI * 2); context.fill(); }
    });
    canvas.addEventListener('pointerup', () => {
      start = null;
      if (!points) return;
      const samples = points.map(([x, y]) => ({ x, y })); points = null;
      const op = { op: 'paint', photo: state.current, variant: variant(), samples, size: Number($('photoBrushSize').value) / 100, erase: $('photoBrushErase').checked, brush: { hardness: 0.3 } };
      if (state.pendingBrush?.id === state.brush) op.create = state.pendingBrush; else op.adjustment = state.brush;
      run(async () => { await edit(op, 'Stroke painted.'); state.pendingBrush = null; });
    });
  }
  function constrain() {
    const ratio = $('photoCropAspect').value;
    if (!state.crop || ratio === 'free') return;
    const [w, h] = ratio.split(':').map(Number);
    const frame = state.detail; const aspect = (w / h) / ((frame.width || 1) / (frame.height || 1));
    state.crop[3] = Math.min(1 - state.crop[1], state.crop[2] / aspect);
    state.crop[2] = state.crop[3] * aspect;
  }
  function drawCrop(canvas) {
    const context = canvas.getContext('2d');
    context.clearRect(0, 0, canvas.width, canvas.height);
    if (!state.cropping || !state.crop) return;
    const [x, y, w, h] = state.crop.map((v, i) => v * (i % 2 ? canvas.height : canvas.width));
    context.fillStyle = 'rgba(0,0,0,0.55)';
    context.fillRect(0, 0, canvas.width, canvas.height);
    context.clearRect(x, y, w, h);
    context.strokeStyle = '#b8f34a'; context.lineWidth = 2; context.strokeRect(x, y, w, h);
    context.strokeStyle = 'rgba(255,255,255,0.4)'; context.lineWidth = 1;
    for (const t of [1 / 3, 2 / 3]) { context.beginPath(); context.moveTo(x + w * t, y); context.lineTo(x + w * t, y + h); context.moveTo(x, y + h * t); context.lineTo(x + w, y + h * t); context.stroke(); }
  }

  // Culling keys work in grid and loupe; typing in a field is never captured.
  function keys(event) {
    if (studio.hidden || event.target.matches('input, textarea, select')) return;
    // The canvas editor underneath must not react while the studio is open.
    event.stopImmediatePropagation();
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'z' && !event.shiftKey) { event.preventDefault(); $('photoUndo').click(); return; }
    if (event.ctrlKey || event.metaKey || event.altKey) return;
    const ids = state.photos.map(p => p.id), at = ids.indexOf(state.current);
    const rate = fields => { event.preventDefault(); run(() => edit({ op: 'rate', selection: state.selected.join(','), ...fields }, `Updated ${state.selected.length} photo(s).`)); };
    const key = event.key;
    if (/^[0-5]$/.test(key)) rate({ rating: Number(key) });
    else if (/^[6-9]$/.test(key)) rate({ label: LABELS[Number(key) - 5] });
    else if (key === 'p' || key === 'P') rate({ pick: 'pick' });
    else if (key === 'x' || key === 'X') rate({ pick: 'reject' });
    else if (key === 'u' || key === 'U') rate({ pick: 'none' });
    else if ((key === 'ArrowRight' || key === 'ArrowLeft') && ids.length) { event.preventDefault(); select(ids[(at + (key === 'ArrowRight' ? 1 : ids.length - 1)) % ids.length]); }
    else if (key === 'g' || key === 'G') setView('grid');
    else if (key === 'e' || key === 'E' || key === 'Enter') setView('loupe');
    else if (key === 'c' || key === 'C') setView('compare');
    else if (key === 'j' || key === 'J') { state.overlay = state.overlay === 'clipping' ? 'none' : 'clipping'; $('photoOverlay').value = state.overlay; run(renderLoupe); }
    else if (key === 'Escape') { if (state.cropping || state.brush) { state.cropping = false; state.brush = null; renderMasks(); run(renderLoupe); } else close(); }
  }

  function open() { studio.hidden = false; setView(state.view); run(loadCatalog); }
  function close() { studio.hidden = true; }

  function build() {
    $('photoSpace').value = p3 ? 'display-p3' : 'srgb';
    $('photoOpen').addEventListener('click', open);
    $('photoClose').addEventListener('click', close);
    window.addEventListener('keydown', keys, true);
    for (const button of studio.querySelectorAll('[data-view]')) button.addEventListener('click', () => setView(button.dataset.view));
    $('photoSearch').addEventListener('submit', event => { event.preventDefault(); run(loadCatalog); });
    $('photoSpace').addEventListener('change', () => { state.thumbs.clear(); run(loadCatalog); });
    $('photoOverlay').addEventListener('change', () => { state.overlay = $('photoOverlay').value; run(renderLoupe); });
    $('photoClipHigh').addEventListener('click', () => { state.overlay = 'clipping'; $('photoOverlay').value = 'clipping'; run(renderLoupe); });
    $('photoClipLow').addEventListener('click', () => { state.overlay = 'clipping'; $('photoOverlay').value = 'clipping'; run(renderLoupe); });
    $('photoVariant').addEventListener('change', () => { state.brush = null; renderPanels(); renderMasks(); run(renderLoupe); });
    $('photoCompareB').addEventListener('change', () => run(renderLoupe));
    $('photoAutoTone').addEventListener('click', () => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), auto_tone: true }, 'Auto tone applied.')));
    $('photoAsShot').addEventListener('click', () => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), white_balance: { mode: 'as-shot' } }, 'White balance as shot.')));
    $('photoAutoWb').addEventListener('click', () => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), white_balance: { mode: 'suggest' } }, 'Suggested white balance applied.')));
    $('photoWbApply').addEventListener('click', () => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), white_balance: { mode: 'temperature', temperature: Number($('photoTemperature').value), tint: Number($('photoTint').value) } }, 'White balance set.')));
    $('photoCropStart').addEventListener('click', () => {
      const develop = state.detail?.photo.variants.find(v => v.id === variant())?.develop || {};
      state.cropping = !state.cropping; state.brush = null; state.crop = develop.crop?.rect ? [...develop.crop.rect] : [0, 0, 1, 1];
      $('photoCropStart').textContent = state.cropping ? 'Cancel crop' : 'Crop…';
      run(renderLoupe);
    });
    $('photoCropApply').addEventListener('click', () => {
      if (!state.cropping || !state.crop || state.crop[2] <= 0 || state.crop[3] <= 0) { notify('Drag a crop rectangle on the photo first.'); return; }
      const [x, y] = state.crop.slice(0, 2).map(v => Math.round(v * 10000) / 10000);
      const rect = [x, y, Math.floor(Math.min(state.crop[2], 1 - x) * 10000) / 10000, Math.floor(Math.min(state.crop[3], 1 - y) * 10000) / 10000];
      const aspect = $('photoCropAspect').value;
      state.cropping = false; $('photoCropStart').textContent = 'Crop…';
      run(() => edit({ op: 'develop', photo: state.current, variant: variant(), set: { 'crop.rect': rect, 'crop.aspect': aspect === 'free' ? 'free' : aspect } }, 'Crop applied.'));
    });
    $('photoCropClear').addEventListener('click', () => run(() => edit({ op: 'develop', photo: state.current, variant: variant(), unset: ['crop'] }, 'Crop removed.')));
    $('photoCropAspect').addEventListener('change', () => { constrain(); renderLoupe(); });
    for (const kind of ['radial', 'linear', 'brush']) $(`photoMask-${kind}`).addEventListener('click', () => newMask(kind));
    $('photoMaskOverlay').addEventListener('change', () => run(renderLoupe));
    $('photoCopy').addEventListener('click', () => { state.copied = `${state.current}/${variant()}`; $('photoCopied').textContent = `Copied ${state.copied}`; });
    $('photoGroups').replaceChildren(...GROUPS.map(group => el('label', {}, el('input', { type: 'checkbox', value: group, checked: !['crop', 'local'].includes(group) }), ` ${group}`)));
    $('photoSync').addEventListener('click', () => {
      if (!state.copied) { notify('Copy settings from a photo first.'); return; }
      const to = $('photoSyncTo').value.trim() || state.selected.filter(id => `${id}/master` !== state.copied).join(',');
      if (!to) { notify('Select target photos (ctrl/shift-click) or type IDs or a query.'); return; }
      const groups = [...$('photoGroups').querySelectorAll('input:checked')].map(input => input.value);
      run(() => edit({ op: 'sync', source: state.copied, to, groups, auto_per_photo: $('photoAutoPerPhoto').checked }, `Synced ${groups.length} group(s) from ${state.copied}.`).then(() => state.thumbs.clear()));
    });
    $('photoVariantAdd').addEventListener('click', () => {
      const id = $('photoNewId').value.trim(); if (!id) { notify('Type an ID for the new variant.'); return; }
      run(async () => { await edit({ op: 'variant', photo: state.current, id, from: variant() }, `Variant ${id} created from ${variant()}.`); $('photoVariant').value = id; renderPanels(); renderMasks(); await renderLoupe(); });
    });
    $('photoSnapshotAdd').addEventListener('click', () => {
      const id = $('photoNewId').value.trim(); if (!id) { notify('Type an ID for the snapshot.'); return; }
      run(() => edit({ op: 'snapshot', photo: state.current, variant: variant(), id }, `Snapshot ${id} saved.`));
    });
    $('photoRestore').addEventListener('click', () => {
      const snapshot = $('photoSnapshots').value; if (!snapshot) { notify('This photo has no snapshots.'); return; }
      run(() => edit({ op: 'restore', photo: state.current, snapshot }, `Restored ${snapshot}.`));
    });
    $('photoKeywordForm').addEventListener('submit', event => {
      event.preventDefault();
      const words = $('photoKeywordInput').value.split(',').map(w => w.trim()).filter(Boolean);
      if (!words.length) return;
      const remove = event.submitter?.value === 'remove';
      run(() => edit({ op: 'keyword', selection: state.selected.join(','), [remove ? 'remove' : 'add']: words }, `${remove ? 'Removed' : 'Added'} ${words.join(', ')}.`));
    });
    $('photoUndo').addEventListener('click', () => run(async () => { await json(await fetch('/api/undo', { method: 'POST' })); state.thumbs.clear(); await loadCatalog(); notify('Undone.'); }));
    let resize = null;
    addEventListener('resize', () => { clearTimeout(resize); resize = setTimeout(() => { if (!studio.hidden) run(renderLoupe); }, 300); });
  }
  build();
})();
