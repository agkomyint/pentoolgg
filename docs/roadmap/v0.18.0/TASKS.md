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
- [ ] **5. Packaging design:** a dieline is a first-class vector structure
  (cut, crease, perforation and bleed line roles with explicit units). A folded
  3D preview is derived from it: panels become faces, creases become hinges with
  a fold-angle parameter, and each panel's artwork is a live texture binding
  (task 1). Built-in templates: folding carton (tuck-end, straight-tuck,
  reverse-tuck), sleeve, mailer box, cylinder/tube, pouch and bottle label wrap.
  Paperboard thickness and glue flaps are modeled so panels meet correctly.
- [ ] **5a. One data model, two synced views.** The dieline is the source of
  truth; panel ID ↔ 3D face ↔ dieline region is a single mapping in the `.pen`.
  Each panel's UVs are its rectangle on the dieline, so the artwork is one flat
  texture the folded model samples. Picking is exact: the 3D viewport renders an
  ID buffer, so clicking a face selects and highlights the matching 2D panel and
  selecting a 2D panel highlights its face. Edits in either view update the
  other. Dielines are parametric (L × W × H, board thickness) and carry bleed,
  safe-zone and glue-flap regions that preflight checks (text or logos outside
  the safe zone, artwork short of bleed).
- [ ] **6. Packaging output:** artwork exports back to the flat dieline sheet
  with correct bleed, spot colors for cut and crease (separations), and PDF/X
  draft; `pentool pack` CLI to build, fold-preview and export from a template
  plus artwork, with stable JSON output.
- [ ] **7. Editor:** mockup gallery, drag-artwork-to-slot, camera presets, and a
  fold slider showing the flat-to-folded transition for packaging.
- [ ] **8. Batch rendering:** many artworks through one mockup with stable output
  names and a bounded work queue.
- [ ] **9. Evidence:** goldens, size check, hostile cases, docs.

## Acceptance targets

- Changing the source artwork changes only the affected mockup pixels and the
  render cache invalidates exactly the dependents.
- A mockup exported to a PDF/X draft passes preflight with correct effective ppi.
- Editing a panel on the flat dieline updates the folded 3D preview; the flat
  export keeps cut and crease lines on their own separations and the folded
  panels meet without gaps for every built-in template.

## Explicitly deferred

- Cloth, fluid or soft-body simulation, and photoreal ray tracing.
- Automatic dieline generation from arbitrary 3D meshes (unfolding); only
  parametric templates and imported dielines ship first.
- Structural engineering checks (compression strength, material yield).
- Non-developable (doubly curved) packaging that cannot unfold flat without
  distortion; cylinders, cones and tubes (developable) are in scope.
- DXF/ArtiosCAD/Esko dieline import ships with interchange (v0.20.0); the first
  release imports SVG dielines with labeled panels and fold lines.
