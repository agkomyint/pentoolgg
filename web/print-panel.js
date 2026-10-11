// Print preflight panel. All checks run in Rust; this file only displays the report.
(() => {
  const $ = id => document.getElementById(id);
  const status = $('printStatus');
  const list = $('printIssues');
  let busy = false;
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
      view.textContent = `${label} (numeric view, not a contract proof).`;
    } catch (error) {
      image.hidden = true;
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
  $('printProof').onclick = () => showImage('/api/color/proof', 'Soft proof');
})();
