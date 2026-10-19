# Pentool v0.22.0 — Layers and live documents

Layers in Studio now work the way Photoshop users expect: real adjustment
layers with proper controls, clipping masks, layer masks, blend modes, and
copy and paste of all of it between documents. An open Studio tab also follows
the file live, shows who changed what, and keeps a version history you can
restore from.

No new `.pen` fields, CLI commands or dependencies. Version 6 validation
accepts one more stack shape (a clipped adjustment inside a clipping group, see
Engine); documents that use it do not validate in 0.21.x and earlier.

## Highlights

- **Adjustment layers as real layers.** Add any of 13 adjustments from the
  Layers panel, edit them with sliders, curves and gradients, clip them to one
  layer or let them affect everything below, and copy them into another image.
- **Clipping masks like Photoshop.** Alt-click or Alt-drop to clip; dragging a
  layer into or out of a clipping group joins or releases it.
- **Layer masks.** Mask a layer with a shape and edit density, feather and
  invert.
- **Live documents.** Changes from the CLI, an agent, another tab or a
  collaborator appear in an open tab without reloading, and your unsaved edits
  are kept.
- **Version history.** See recent editors, preview any stored version and
  restore it as an undoable step.
- **Photo to canvas.** A Photos strip in the Layers dock places photos, with
  their edits, by drag and drop.

## Studio

### Layers, adjustment layers and masks

- Layers panel: per-kind icons, an eye on every row, a clip arrow and indent for
  clipped layers, an underlined clipping base, an `fx` badge and a mask chip.
  A bar above the list sets the selected layer's blend mode (16 modes, plus
  Pass through for groups), Opacity and Fill. A toolbar below it adds an
  adjustment layer, clips, groups, duplicates and deletes. The right-click menu
  adds Create/Release clipping mask, Mask with selected shape, Invert mask and
  Delete mask.
- New adjustment layer (13 kinds: levels, curves, brightness/contrast,
  hue/saturation, vibrance, exposure, color balance, black & white, channel
  mixer, gradient map, invert, posterize, threshold) inserts directly above the
  selected layer, outside its clipping group, and is selected. A version 4 or 5
  document is upgraded to version 6 first, as one undoable step.
- Properties shows real controls for each kind instead of a JSON field: sliders,
  a curves editor (click to add, drag, double-click to remove), gradient stops
  and channel mixer outputs, with live preview and one undo step per change.
  "Affects" chooses all layers below or only the layer below (clipped). A Mask
  section edits density, feather, invert and link.
- Clipping works as in Photoshop: Alt-click a row, or Alt-drop a layer onto
  another, to clip it to the layer below. Releasing a clipped layer releases
  the ones above it. Dragging a layer into a clipping group joins it, dragging
  one out releases it, and moving a base re-clips its layers to the layer now
  below them, so reordering never leaves an invalid stack.
- Copy and paste keep adjustment and clipped layers working across documents:
  pasted IDs are remapped together with the scopes and clipping that refer to
  them, referenced images and mask resources travel with the copy, and pasting
  into a version 4 or 5 document upgrades it instead of failing at save.
  Adjustments paste in place without the usual offset. Duplicating a clipping
  base keeps its clipped layers attached to the original.

### Live document, presence and history

- An open Studio tab updates by itself when the served file changes, whether
  the edit came from the CLI, an agent, another tab or a collaboration peer.
  Unsaved edits in the tab are merged with the incoming change instead of being
  overwritten; there is no "Changed on disk" banner to dismiss.
- Remote changes are pointed out on the canvas with the editor's name and
  colour, a short activity toast, and avatars of recent editors in the header.
- A Version history panel lists recent changes with who made them, previews
  any stored version, and restores it as a new step that can be undone.

### Photo and pages

- An "Add to canvas" button in the Photo toolbar and panel places the selected
  photos, with their edits, on the active layer and opens Design. Several
  photos are placed with a small offset; when the button is disabled its
  tooltip says why. Later Photo edits update the placed photo.
- The Layers dock has a Photos tab with a thumbnail strip of the catalog. Drag
  thumbnails onto the canvas to place them where they are dropped, or onto a
  page thumbnail to place them in the middle of that page. Double-click a
  thumbnail to add it to the current page.
- The page strip at the bottom has a compact mode (the chevron on the right)
  that shows page name chips instead of thumbnails. Studio remembers the choice.

## Engine

- A clipped adjustment (scope `targets` naming only the clipping base) no
  longer ends the clipping group, so layers above it can stay clipped to the
  same base, as in Photoshop. Hiding a clipped adjustment no longer releases the
  layers clipped above it at render time.

## Server

- New `GET /api/document/events` server-sent event stream that reports the
  shared document's revision on connect and on every change. Works with or
  without `--collab` (view access required when collaboration is on).
- New `GET /api/history/version` and `POST /api/history/restore` for previewing
  and restoring stored versions. See `docs/PROTOCOL.md`.
- Error responses (`400 {"error": ...}`) now include the cause chain, so a
  failed `/api/scene` batch says why, for example
  `operation 0 failed (page default, layer -, group -, object base): [missing-mask] mask nope missing`
  instead of only the operation context.

## Fixes

- Photo: placing a photo on a page that had not been saved yet failed with
  `[missing-resource] page page-N not found`. Studio now saves the canvas first.
- Photo: a project with no photos yet opens with an empty grid instead of
  `[missing-resource] the document has no photography catalog`, so the first
  JPEG, PNG or DNG can be imported from Studio.
- Studio: reordering a clipped layer or a clipping base could leave a stack that
  failed to save with `invalid-clipping`.
- `tree`, `search` and geometry output round bounds to 3 decimals, so values
  such as `28.799999999999997` are reported as `28.8`.
- `paint-plan -` refuses stdin with a clear `[invalid-request]` message instead
  of a confusing missing-file error.

## Not in this release

- Painting directly on a layer mask (masks come from a shape, an alpha or an
  image), one combined layer stack per page, and the healing fix described in
  `docs/roadmap/architecture/layer-model.md` need a format or output decision
  and are planned for a later minor release.
