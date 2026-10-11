# v0.18.0 — 3D in layouts and mockups

Make the v0.17.0 3D renderer useful in real documents: place models in page
layouts, build product mockups, and carry 3D results through color management,
print and packaging.

## Product contract

- 3D output is a raster node inside the existing scene graph. Print, PDF/X,
  separations and preflight treat a rendered 3D node like any raster image, with
  the same resolution and color rules.
- Rendering is resolution-aware: the node reports its effective ppi and
  preflight checks it against the page's output.
- Mockups are documents, not a special mode: a model, a camera, lights and a
  `texture` binding to ordinary page content.
- Deterministic on the reference renderer; the accelerator never defines output.
- Documents without mockups render as before.

## Model

- A `texture` binding maps a page or node (live, by reference) onto a named
  material slot of a model, so editing the artwork updates the mockup.
- Presets: box, cylinder, bottle, bag, card, device frame, with documented UV
  conventions and licenses.
- Shadow catcher and ground plane as explicit, optional scene objects.

## Must ship, in order

- [ ] **1. Specify live texture bindings,** dependency ordering and cycle
  rejection, in `docs/scene3d-v1.md`.
- [ ] **2. Implement bindings** with deterministic invalidation and a bounded
  render cache keyed by source content digest.
- [ ] **3. Built-in mockup presets** and a CLI to apply artwork to a preset.
- [ ] **4. Color and print path:** managed colors for textures, 3D nodes in
  separations and PDF/X output with effective-ppi preflight.
- [ ] **5. Editor:** mockup gallery, drag-artwork-to-slot, camera presets.
- [ ] **6. Batch rendering:** many artworks through one mockup with stable output
  names and a bounded work queue.
- [ ] **7. Evidence:** goldens, size check, hostile cases, docs.

## Acceptance targets

- Changing the source artwork changes only the affected mockup pixels and the
  render cache invalidates exactly the dependents.
- A mockup exported to a PDF/X draft passes preflight with correct effective ppi.

## Explicitly deferred

- Cloth, fluid or soft-body simulation, and photoreal ray tracing.
