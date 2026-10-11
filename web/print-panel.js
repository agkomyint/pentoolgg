// Print preflight panel. All checks run in Rust; this file only displays the report.
(() => {
  const $ = id => document.getElementById(id);
  const status = $('printStatus');
  const list = $('printIssues');
  let busy = false;
  let production = null;
  let overprintNodes = [];
  const guides = $('printGuides');
  const NS = 'http://www.w3.org/2000/svg';
  // Draw the resolved media, bleed, trim and safe rectangles over the plate or
  // proof image. Coordinates are canvas units, so the image (rendered at the
  // canvas aspect ratio) and the guides scale together.
  function drawGuides() {
    const on = $('printGuidesToggle').checked && production && production.canvas && !$('printImage').hidden;
    guides.hidden = !on;
    guides.replaceChildren();
    if (!on) return;
    guides.setAttribute('viewBox', `0 0 ${production.canvas.width} ${production.canvas.height}`);
    guides.setAttribute('preserveAspectRatio', 'none');
    if ($('printOverprintToggle').checked) {
      for (const node of overprintNodes) {
        if (node.width == null || node.height == null) continue;
        const rect = document.createElementNS(NS, 'rect');
        rect.setAttribute('x', node.x); rect.setAttribute('y', node.y);
        rect.setAttribute('width', node.width); rect.setAttribute('height', node.height);
        rect.setAttribute('fill', '#f59e0b'); rect.setAttribute('fill-opacity', '0.35');
        rect.setAttribute('stroke', '#b45309'); rect.setAttribute('vector-effect', 'non-scaling-stroke');
        const title = document.createElementNS(NS, 'title');
        title.textContent = `overprint: ${node.id}`;
        rect.append(title);
        guides.append(rect);
      }
    }
    for (const [name, color, dash] of [['media', '#888888', ''], ['bleed', '#e11d48', '4 3'], ['trim', '#2563eb', ''], ['safe', '#16a34a', '2 2']]) {
      const box = production[name];
      if (!box) continue;
      const rect = document.createElementNS(NS, 'rect');
      rect.setAttribute('x', box.x); rect.setAttribute('y', box.y);
      rect.setAttribute('width', box.width); rect.setAttribute('height', box.height);
      rect.setAttribute('fill', 'none'); rect.setAttribute('stroke', color);
      rect.setAttribute('vector-effect', 'non-scaling-stroke'); rect.setAttribute('stroke-width', '1.5');
      if (dash) rect.setAttribute('stroke-dasharray', dash);
      const title = document.createElementNS(NS, 'title');
      title.textContent = `${name}: ${box.width} x ${box.height} at ${box.x}, ${box.y}`;
      rect.append(title);
      guides.append(rect);
    }
  }
  $('printGuidesToggle').onchange = drawGuides;
  $('printOverprintToggle').onchange = drawGuides;
  function show(report) {
    list.replaceChildren(...report.issues.map(issue => {
      const item = document.createElement('li');
      item.dataset.severity = issue.severity;
      if (issue.node) {
        item.tabIndex = 0;
        item.title = 'Select this object';
        const go = () => document.dispatchEvent(new CustomEvent('pentool-select-node', { detail: { id: issue.node } }));
        item.onclick = go;
        item.onkeydown = event => { if (event.key === 'Enter') go(); };
      }
      item.textContent = `${issue.severity.toUpperCase()} ${issue.code}${issue.node ? ` (${issue.node})` : ''}: ${issue.message} Fix: ${issue.fix}`;
      return item;
    }));
    status.textContent = report.ok
      ? `Ready: ${report.warnings} warning(s), no blocking errors.`
      : `Blocked: ${report.errors} error(s) and ${report.warnings} warning(s).`;
    production = report.production || null;
    overprintNodes = report.overprint_nodes || [];
    drawGuides();
    $('printGeometry').textContent = report.production
      ? JSON.stringify(report.production, null, 2)
      : 'This page declares no production geometry.';
  }
  async function run() {
    if (busy) return;
    busy = true; $('printRun').disabled = true; status.textContent = 'Running preflight...';
    try {
      const query = new URLSearchParams({ min_ppi: $('printMinPpi').value, min_bleed_inches: $('printMinBleed').value });
      const response = await fetch(`/api/color/preflight?${query}`);
      const value = await response.json();
      if (!response.ok) throw new Error(value.error || 'Preflight was rejected.');
      show(value);
    } catch (error) {
      status.textContent = error.message || 'Preflight failed.';
    } finally {
      busy = false; $('printRun').disabled = false;
    }
  }
  $('printRun').onclick = run;
  const view = $('printViewStatus');
  const image = $('printImage');
  let objectUrl = null;
  async function showImage(url, label) {
    view.textContent = `Rendering ${label}...`;
    try {
      const response = await fetch(url);
      if (!response.ok) throw new Error((await response.json()).error || 'Request was rejected.');
      const blob = await response.blob();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
      objectUrl = URL.createObjectURL(blob);
      image.src = objectUrl; image.hidden = false;
      drawGuides();
      view.textContent = `${label} (numeric view, not a contract proof).`;
    } catch (error) {
      image.hidden = true;
      drawGuides();
      view.textContent = error.message || 'Could not render the view.';
    }
  }
  $('printPlates').onclick = async () => {
    view.textContent = 'Separating plates...';
    try {
      const response = await fetch('/api/color/plates');
      const value = await response.json();
      if (!response.ok) throw new Error(value.error || 'Separation was rejected.');
      const select = $('printPlate');
      select.replaceChildren(...value.plates.map(plate => {
        const option = document.createElement('option');
        option.value = plate.plate;
        option.textContent = `${plate.plate}${plate.spot ? ' (spot)' : ''} - ${plate.average_coverage_percent.toFixed(1)}% avg, ${plate.max_percent.toFixed(0)}% max`;
        return option;
      }));
      select.disabled = false;
      view.textContent = `${value.plates.length} plate(s) at ${value.width}x${value.height}.`;
      await showImage(`/api/color/plates?plate=${encodeURIComponent(select.value)}`, `Plate ${select.value}`);
    } catch (error) {
      view.textContent = error.message || 'Separation failed.';
    }
  };
  $('printPlate').onchange = event => showImage(`/api/color/plates?plate=${encodeURIComponent(event.target.value)}`, `Plate ${event.target.value}`);
  const proofUrl = base => ($('printIntent').value ? `${base}?intent=${encodeURIComponent($('printIntent').value)}` : base);
  $('printIntent').onchange = () => {
    const intent = $('printIntent').value;
    $('printPdfx').href = `/api/color/pdfx?ppi=300${intent ? `&intent=${encodeURIComponent(intent)}` : ''}`;
  };
  $('printProof').onclick = () => showImage(proofUrl('/api/color/proof'), 'Soft proof');
  $('printGamut').onclick = () => showImage(proofUrl('/api/color/gamut'), 'Gamut warning (red = outside the output profile)');
})();
