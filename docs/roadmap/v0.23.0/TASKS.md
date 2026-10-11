# v0.23.0 — Web and UX output

Turn designs into responsive web output: layout rules in the document, and
HTML/CSS export that reflects them, from the same offline binary.

## Product contract

- Responsive behavior is data in the `.pen` document (constraints, auto layout,
  breakpoints), not a separate export-time guess.
- HTML/CSS export is deterministic, self-contained and readable, with stable
  class names derived from node IDs and tokens (v0.22.0).
- Text stays text, images stay images, vectors export as inline SVG. Where a
  construct has no faithful web equivalent, the export report says so and falls
  back to a raster with a named reason.
- Accessibility is checked, not assumed: roles, alt text, reading order and
  contrast problems are reported by a bounded audit.
- No runtime framework, bundler or network access is required.

## Must ship, in order

- [ ] **1. Specify constraints, auto layout and breakpoints** in the format.
- [ ] **2. Layout solver** shared by the renderer, the editor and the exporter.
- [ ] **3. HTML/CSS exporter** with a fidelity report and golden tests.
- [ ] **4. Accessibility audit** command and report.
- [ ] **5. Interaction metadata** (links, states) as declarative data only.
- [ ] **6. Editor:** breakpoint switcher and responsive preview.
- [ ] **7. Evidence:** browser-rendered comparison against the Rust renderer
  within a documented tolerance, and size checks.

## Explicitly deferred

- Scripted behavior and component frameworks (React/Vue code generation),
  server-side rendering, and any animation export (time-based media is out of
  scope for Pentool).
