# v0.17.0 — Design interchange

Bring work in from, and send work out to, the formats designers already use,
without losing structure where the target format can hold it and without
silently losing anything where it cannot.

## Product contract

- Every import and export produces a **fidelity report**: what mapped exactly,
  what was approximated, and what was dropped, each with the source object ID.
  There are no silent drops.
- Import is bounded and hostile-input safe (archive paths, decompression ratio,
  node count, nesting, image size) and never contacts the network.
- Imports create ordinary `.pen` nodes with stable generated IDs; they do not
  introduce a parallel model.
- Round trips are tested: an exported-then-reimported document keeps the
  properties the fidelity table says are preserved.
- No proprietary SDKs and no system libraries are linked. Parsers are Pentool's
  own Rust code or small audited pure Rust dependencies.

## Formats, in priority order

| Format | Import | Export | Notes |
|---|---|---|---|
| SVG | full static subset | existing | styles, gradients, clip, mask, text-to-outline option |
| PDF (vector) | pages to nodes | existing PDF | text with embedded fonts where licensed |
| PSD | layers, masks, adjustments subset | flattened + layers subset | per v0.13.0 fidelity limits |
| Figma (`.fig` / JSON REST export) | frames, auto layout, components | none | structure mapped to groups and v0.6.24 components |
| Sketch (`.sketch`) | artboards, symbols, shared styles | none | |
| Adobe Illustrator (`.ai`, PDF-compatible) | via the PDF path | via PDF | native `.ai` internals are out of scope |
| IDML | per v0.14.0 | per v0.14.0 | reuse, do not duplicate |

## Must ship, in order

- [ ] **1. Fidelity report schema** and a shared import harness with limits.
- [ ] **2. SVG import hardening** to the full static subset, with a conformance
  corpus.
- [ ] **3. PDF vector import** with font and image extraction rules.
- [ ] **4. PSD import** reusing the v0.13.0 layer model.
- [ ] **5. Figma export-file import** with auto-layout mapped to Pentool layout.
- [ ] **6. Sketch import** with symbols mapped to components.
- [ ] **7. Round-trip tests** and a documented fidelity table per format.
- [ ] **8. CLI and editor entry points,** with a fidelity-report viewer.
- [ ] **9. Security and size evidence,** including hostile archives.

## CLI direction

```sh
pentool import DOC design.fig --report report.json
pentool import DOC icons.svg --prefix icons
pentool export DOC out.psd --layers
```

## Explicitly deferred

- Live sync with Figma/Sketch cloud services, plugins that call vendor SDKs, and
  proprietary prototype or interaction data.
