// Raster canvas panel. Pixels are only ever changed by POST /api/raster (the shared
// Rust engine). Everything drawn here - cursor outline, live stroke preview, stabilizer
// lag line, clone source marker, quick mask and selection edges - is a disposable overlay.
(() => {
  const $ = (id) => document.getElementById(id);
  const state = { doc: null, page: null, layers: [], zoom: 1, rotate: 0, stroke: null, busy: false, source: null, selection: null, panX: 0, panY: 0, guides: [], lasso: [], tool: 'brush', pointer: null, generation: 0, mask: false };
  const view = $('rasterView'), overlay = $('rasterOverlay'), image = $('rasterImage');
  const ctx = overlay.getContext('2d');
  const notify = (m) => { $('rasterStatus').textContent = m; };
  const layer = () => state.layers.find((n) => n.id === $('rasterLayer').value);
  const collect = (nodes, out) => { for (const n of nodes || []) { if (n.kind === 'raster') out.push(n); collect(n.children, out); } return out; };

  function loadDocument(detail) {
    state.doc = detail.document;
    state.page = detail.page || detail.document?.pages?.[0]?.id;
    state.layers = [];
    if (state.doc?.version >= 6) {
      for (const p of state.doc.pages) if (p.id === state.page) for (const l of p.layers) collect(l.nodes, state.layers);
    }
    const previous = $('rasterLayer').value;
    $('rasterLayer').replaceChildren(...state.layers.map((n) => new Option(`${n.id} (${n.width}x${n.height})`, n.id)));
    if (state.layers.some((n) => n.id === previous)) $('rasterLayer').value = previous;
    $('rasterEditing').hidden = !state.layers.length;
    notify(state.layers.length ? 'Pick a tool and paint on the preview.' : 'No raster layers. Create one with `pentool raster doc.pen add`.');
    if (state.layers.length) refresh();
  }

  async function refresh() {
    const generation = ++state.generation;
    const response = await fetch(`/api/render/png?page=${encodeURIComponent(state.page)}&max_edge=1024`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(state.doc) });
    if (generation !== state.generation || !response.ok) return;
    const url = URL.createObjectURL(await response.blob());
    image.onload = () => { URL.revokeObjectURL(url); fit(); drawNavigator(); };
    image.src = url;
  }

  // The preview is a proxy of the page; map it onto the selected layer's pixels.
  function geometry() {
    const node = layer();
    const canvas = state.doc?.pages.find((p) => p.id === state.page)?.canvas;
    if (!node || !canvas || !image.naturalWidth) return null;
    return { node, sx: image.naturalWidth / canvas.width, sy: image.naturalHeight / canvas.height, lx: node.x || 0, ly: node.y || 0 };
  }
  function fit() { overlay.width = image.naturalWidth; overlay.height = image.naturalHeight; applyView(); drawOverlay(); }
  function applyView() {
    view.style.transform = `translate(${state.panX}px, ${state.panY}px) rotate(${state.rotate}deg) scale(${state.zoom})`;
    $('rasterZoomLabel').textContent = `${Math.round(state.zoom * 100)}% / ${state.rotate}°`;
    drawNavigator();
  }

  // View aids (rulers, guides, snapping, navigator) never touch the document.
  const SNAP = 6;
  function snap(p) {
    const g = geometry();
    if (!g || !$('rasterSnap').checked) return p;
    let { x, y } = p;
    const grid = +$('rasterGrid').value;
    if (grid > 0) { x = Math.round(x / grid) * grid; y = Math.round(y / grid) * grid; }
    for (const guide of state.guides) {
      if (guide.axis === 'x' && Math.abs(p.x - guide.at) <= SNAP) x = guide.at;
      if (guide.axis === 'y' && Math.abs(p.y - guide.at) <= SNAP) y = guide.at;
    }
    return { ...p, x, y, px: (x + g.lx) * g.sx, py: (y + g.ly) * g.sy };
  }
  function drawRulers(g) {
    ctx.save(); ctx.font = '10px sans-serif'; ctx.lineWidth = 1;
    ctx.fillStyle = 'rgba(255,255,255,.85)'; ctx.fillRect(0, 0, overlay.width, 12); ctx.fillRect(0, 0, 12, overlay.height);
    ctx.fillStyle = '#000'; ctx.strokeStyle = '#000';
    for (let x = 0; x <= g.node.width; x += 10) {
      const px = (g.lx + x) * g.sx; const long = x % 50 === 0;
      ctx.beginPath(); ctx.moveTo(px, long ? 0 : 7); ctx.lineTo(px, 12); ctx.stroke();
      if (long) ctx.fillText(String(x), px + 2, 9);
    }
    for (let y = 0; y <= g.node.height; y += 10) {
      const py = (g.ly + y) * g.sy; const long = y % 50 === 0;
      ctx.beginPath(); ctx.moveTo(long ? 0 : 7, py); ctx.lineTo(12, py); ctx.stroke();
      if (long) ctx.fillText(String(y), 1, py - 2);
    }
    ctx.restore();
  }
  function drawGuides(g) {
    ctx.save(); ctx.strokeStyle = '#00c8ff'; ctx.lineWidth = 1;
    for (const guide of state.guides) {
      ctx.beginPath();
      if (guide.axis === 'x') { const px = (g.lx + guide.at) * g.sx; ctx.moveTo(px, 0); ctx.lineTo(px, overlay.height); }
      else { const py = (g.ly + guide.at) * g.sy; ctx.moveTo(0, py); ctx.lineTo(overlay.width, py); }
      ctx.stroke();
    }
    ctx.restore();
  }
  function drawNavigator() {
    const nav = $('rasterNavigator'), nctx = nav.getContext('2d');
    nctx.clearRect(0, 0, nav.width, nav.height);
    if (!image.naturalWidth) return;
    const scale = Math.min(nav.width / image.naturalWidth, nav.height / image.naturalHeight);
    const w = image.naturalWidth * scale, h = image.naturalHeight * scale;
    nctx.drawImage(image, 0, 0, w, h);
    // The stage shows 1/zoom of the page, centered on the pan offset.
    const stage = view.parentElement.getBoundingClientRect();
    const k = w / (view.offsetWidth || w);
    const vw = Math.min(w, stage.width / state.zoom * k), vh = Math.min(h, stage.height / state.zoom * k);
    const cx = w / 2 - state.panX / state.zoom * k, cy = h / 2 - state.panY / state.zoom * k;
    nctx.strokeStyle = '#ff375f'; nctx.lineWidth = 2; nctx.strokeRect(cx - vw / 2, cy - vh / 2, vw, vh);
  }

  // Client point -> layer pixel, through zoom and rotation.
  function toLayer(event) {
    const g = geometry();
    if (!g) return null;
    const rect = overlay.getBoundingClientRect();
    const a = state.rotate * Math.PI / 180;
    const dx = event.clientX - (rect.left + rect.width / 2), dy = event.clientY - (rect.top + rect.height / 2);
    const ux = (dx * Math.cos(a) + dy * Math.sin(a)) / state.zoom, uy = (-dx * Math.sin(a) + dy * Math.cos(a)) / state.zoom;
    const px = ux * overlay.width / (rect.width / state.zoom) + overlay.width / 2;
    const py = uy * overlay.height / (rect.height / state.zoom) + overlay.height / 2;
    return { x: px / g.sx - g.lx, y: py / g.sy - g.ly, px, py };
  }

  const brush = () => ({ kind: $('rasterKind').value, size: +$('rasterSize').value, hardness: +$('rasterHardness').value, flow: +$('rasterFlow').value, smoothing: +$('rasterStabilizer').value });

  function drawOverlay() {
    ctx.clearRect(0, 0, overlay.width, overlay.height);
    const g = geometry();
    if (!g) return;
    if ($('rasterRulers').checked) drawRulers(g);
    drawGuides(g);
    if (state.mask) { ctx.fillStyle = 'rgba(255,0,0,.3)'; ctx.fillRect(g.lx * g.sx, g.ly * g.sy, g.node.width * g.sx, g.node.height * g.sy); }
    if (state.selection) {
      const s = state.selection;
      ctx.save(); ctx.setLineDash([4, 3]); ctx.strokeStyle = '#0a84ff'; ctx.lineWidth = 1;
      ctx.strokeRect((g.lx + s.x) * g.sx, (g.ly + s.y) * g.sy, s.width * g.sx, s.height * g.sy); ctx.restore();
    }
    if (state.source) {
      const x = (g.lx + state.source.x) * g.sx, y = (g.ly + state.source.y) * g.sy;
      ctx.strokeStyle = '#ff9f0a'; ctx.beginPath(); ctx.moveTo(x - 8, y); ctx.lineTo(x + 8, y); ctx.moveTo(x, y - 8); ctx.lineTo(x, y + 8); ctx.stroke();
    }
    if (state.lasso.length) {
      ctx.strokeStyle = '#0a84ff'; ctx.beginPath();
      state.lasso.forEach((p, i) => (i ? ctx.lineTo(p.px, p.py) : ctx.moveTo(p.px, p.py))); ctx.stroke();
    }
    const s = state.stroke;
    if (s?.samples) {
      ctx.strokeStyle = 'rgba(0,0,0,.5)'; ctx.lineWidth = Math.max(1, brush().size * g.sx); ctx.lineCap = 'round'; ctx.beginPath();
      s.preview.forEach((p, i) => (i ? ctx.lineTo(p.px, p.py) : ctx.moveTo(p.px, p.py))); ctx.stroke();
      // Stabilizer feedback: the engine smooths with this lag, so show where the dab will land.
      ctx.strokeStyle = '#30d158'; ctx.lineWidth = 1; ctx.beginPath(); ctx.moveTo(s.lag.px, s.lag.py); ctx.lineTo(s.last.px, s.last.py); ctx.stroke();
    }
    if (state.pointer && !state.stroke && ['brush', 'eraser', 'clone', 'heal', 'quickmask'].includes(state.tool)) {
      const r = Math.max(1, brush().size * g.sx / 2);
      ctx.lineWidth = 1; ctx.strokeStyle = '#fff'; ctx.beginPath(); ctx.arc(state.pointer.px, state.pointer.py, r + 1, 0, 7); ctx.stroke();
      ctx.strokeStyle = '#000'; ctx.beginPath(); ctx.arc(state.pointer.px, state.pointer.py, r, 0, 7); ctx.stroke();
    }
  }

  async function call(action, args) {
    if (state.busy) return;
    state.busy = true; view.setAttribute('aria-busy', 'true');
    try {
      const response = await fetch('/api/raster', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ document: state.doc, page: state.page, id: layer().id, action, args }) });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error || 'Request rejected.');
      state.doc = body.document;
      document.dispatchEvent(new CustomEvent('pentool-local-document', { detail: { document: body.document, page: state.page } }));
      notify(`${action}: done. Save .pen to keep it; Undo restores the previous state.`);
    } catch (e) {
      notify(e.message);
    } finally {
      state.busy = false; view.removeAttribute('aria-busy'); refresh();
    }
  }

  overlay.addEventListener('pointerdown', (event) => {
    let p = toLayer(event);
    if (!p || state.busy) return;
    if ($('rasterRulers').checked && (p.py < 12 || p.px < 12)) {
      // A click on a ruler drops a guide: top ruler -> horizontal, left ruler -> vertical.
      state.guides.push(p.py < 12 ? { axis: 'y', at: Math.round(p.y) } : { axis: 'x', at: Math.round(p.x) });
      drawOverlay();
      return;
    }
    p = snap(p);
    overlay.setPointerCapture(event.pointerId);
    const tool = state.tool;
    if (tool === 'source') { state.source = { x: p.x, y: p.y }; call('set-clone-source', { x: p.x, y: p.y }); return; }
    if (tool === 'wand') { call('select-wand', { x: Math.max(0, Math.round(p.x)), y: Math.max(0, Math.round(p.y)), tolerance: +$('rasterTolerance').value, mode: $('rasterMode').value }); return; }
    if (tool === 'lasso') { state.lasso = [p]; state.stroke = { lasso: true }; return; }
    if (tool === 'marquee') { state.stroke = { marquee: p }; return; }
    state.stroke = { samples: [{ x: p.x, y: p.y, pressure: event.pressure || 0.5 }], preview: [p], last: p, lag: p };
    drawOverlay();
  });
  overlay.addEventListener('pointermove', (event) => {
    const p = (() => { const q = toLayer(event); return q && snap(q); })();
    if (!p) return;
    state.pointer = p;
    const s = state.stroke;
    if (s?.samples) {
      s.samples.push({ x: p.x, y: p.y, pressure: event.pressure || 0.5 });
      const w = +$('rasterStabilizer').value;
      s.lag = { px: s.lag.px * w + p.px * (1 - w), py: s.lag.py * w + p.py * (1 - w) };
      s.preview.push(s.lag); s.last = p;
    } else if (s?.lasso) {
      state.lasso.push(p);
    } else if (s?.marquee) {
      const m = s.marquee;
      state.selection = { x: Math.min(m.x, p.x), y: Math.min(m.y, p.y), width: Math.abs(p.x - m.x), height: Math.abs(p.y - m.y) };
    }
    drawOverlay();
  });
  overlay.addEventListener('pointerleave', () => { state.pointer = null; drawOverlay(); });
  overlay.addEventListener('pointerup', async () => {
    const s = state.stroke;
    state.stroke = null;
    if (!s) return;
    const tool = state.tool;
    const common = { brush: brush(), color: $('rasterColor').value, seed: 0 };
    if (s.samples) {
      const action = { brush: 'stroke', eraser: 'stroke', clone: 'clone', heal: 'heal', quickmask: 'quickmask' }[tool];
      await call(action, { ...common, samples: s.samples, blend: tool === 'eraser' ? 'erase' : 'normal', erase: tool === 'quickmask' && $('rasterMode').value === 'subtract' });
    } else if (s.lasso && state.lasso.length >= 3) {
      await call('select-lasso', { points: state.lasso.map((p) => [p.x, p.y]), mode: $('rasterMode').value });
    } else if (s.marquee && state.selection?.width > 0 && state.selection?.height > 0) {
      await call('select-marquee', { ...state.selection, shape: $('rasterShape').value, mode: $('rasterMode').value });
    }
    state.lasso = []; state.selection = null; drawOverlay();
  });

  $('rasterTool').addEventListener('change', () => { state.tool = $('rasterTool').value; state.mask = state.tool === 'quickmask'; drawOverlay(); });
  $('rasterLayer').addEventListener('change', refresh);
  $('rasterZoomIn').addEventListener('click', () => { state.zoom = Math.min(8, state.zoom * 1.25); applyView(); });
  $('rasterZoomOut').addEventListener('click', () => { state.zoom = Math.max(0.1, state.zoom / 1.25); applyView(); });
  $('rasterRotate').addEventListener('click', () => { state.rotate = (state.rotate + 15) % 360; applyView(); });
  $('rasterReset').addEventListener('click', () => { state.zoom = 1; state.rotate = 0; state.panX = 0; state.panY = 0; applyView(); });
  for (const id of ['rasterRulers', 'rasterSnap', 'rasterGrid']) $(id).addEventListener('input', drawOverlay);
  $('rasterClearGuides').addEventListener('click', () => { state.guides = []; drawOverlay(); });
  $('rasterNavigator').addEventListener('pointerdown', (event) => {
    const rect = $('rasterNavigator').getBoundingClientRect();
    const u = (event.clientX - rect.left) / rect.width - 0.5, v = (event.clientY - rect.top) / rect.height - 0.5;
    // Center the clicked spot: pan opposite to its offset from the page center.
    state.panX = -u * (view.offsetWidth || 0) * state.zoom; state.panY = -v * (view.offsetHeight || 0) * state.zoom;
    applyView();
  });
  $('rasterClearSelection').addEventListener('click', () => call('select-clear', {}));
  document.addEventListener('pentool-document-changed', (event) => loadDocument(event.detail));
  document.dispatchEvent(new Event('pentool-request-document'));
})();
