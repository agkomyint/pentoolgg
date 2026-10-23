# Pentool v0.27.0

A focused feature release on selection, cutout and crop, so an agent (or a person
in Studio) can take a photo, select a subject, remove the background, crop, and
build a mood board. No new dependencies, no model, no network; all algorithms are
deterministic. Guide: [Selecting and cutting out](selecting-and-cutting-out.md).

## New

- **Wand for photographs:** `--sample` seed averaging, `--gradient` neighbor-step
  flooding and `--leash` on `select-wand` and `fill`. A noisy gradient sky selects
  as one region (about 0.99 IoU on the synthetic test) where tolerance alone
  leaked or stopped partway.
- **`raster DOC select-refine`:** snaps a rough selection to image edges with a
  deterministic geodesic path race, `--band`, `--core` and `--fg`/`--bg` hints.
- **`raster DOC cutout ID --id NEW`:** one step from a box, polygon, wand or stored
  selection to a new transparent layer, working on raster layers and image nodes
  without touching the source. `--refine`, `--invert`, `--shrink`, `--feather`,
  `--decontaminate`.
- **Shared dispatcher:** `POST /api/raster` and `raster batch` now accept
  `select-refine`, `select-modify`, `select-save/load/delete`, `lift`,
  `move-pixels`, `transform-pixels` and `paste`.
- **Selection queries:** `raster-selection` (active or saved), `invert`, `grow`,
  `shrink` and `feather`; `selection crop --node ID` crops one image.
- **Studio:** wand sample/gradient, selection modifier buttons, Refine edge, and
  Copy/Cut to layer in the Raster panel.

## Also new

- **Lab wand distance:** `--perceptual` on `select-wand`, `fill` and `cutout` measures
  colour difference in CIE Lab (so lightness matters more than the largest channel).
- **`select-modify smooth-edge`:** rounds a jagged outline but keeps a soft edge.
- **`raster DOC delete-pixels ID`:** erases the selected pixels (also in batch and
  `/api/raster`).
- **Studio canvas crop tool:** drag eight handles, aspect presets, Enter/Esc; the
  pixels stay put and the frame and crop window change as one undoable edit. Works
  on image nodes with Fit: fill and no rotation or scale. "Delete selected" and
  "Smooth edge" buttons in the Raster panel.

## Not included

- A `cutout` mask source, `--preview` on cutout (use the global `--preview`), moving
  or transforming the selection outline in Studio, keyboard nudge, and learned
  subject detection.
- `cutout` is not in the shared dispatcher; it needs the document's folder to bake
  image nodes.

## Compatibility

Existing flags, JSON fields and `.pen` files behave as before; wand defaults are
unchanged. The new `decontaminated_pixels`, `undecided_pixels` and similar fields
are additive.
