# Pentool v0.22.0 (unreleased)

Minor release. No new `.pen` fields or CLI changes. Version 6 validation accepts
one more stack shape (clipped adjustments inside a clipping group, below).

## Studio

- Live document: an open Studio tab now updates by itself when the served file
  changes, whether the edit came from the CLI, an agent, another tab or a
  collaboration peer. Unsaved edits in the tab are merged with the incoming
  change instead of being overwritten, and there is no "Changed on disk" banner.
- Photo: an "Add to canvas" button in the Photo toolbar and panel places the
  selected photos, with their edits, on the active layer and opens Design.
  Several selected photos are placed with a small offset. When it is disabled,
  its tooltip says why. Later Photo edits update the placed photo.
- Photo: a project with no photos yet opens with an empty grid instead of
  `[missing-resource] the document has no photography catalog`, so the first
  JPEG, PNG or DNG can be imported from Studio.

- Photo: the Layers dock has a Photos tab with a thumbnail strip of the
  catalog. Drag thumbnails onto the canvas to place them where they are
  dropped, or onto a page thumbnail to place them in the middle of that page.
  Double-click a thumbnail to add it to the current page.
- Photo: placing a photo on a page that had not been saved yet failed with
  `[missing-resource] page page-N not found`. Studio now saves the canvas first.
- Pages: the page strip at the bottom has a compact mode (the chevron button on
  the right) that shows page name chips instead of thumbnails. Studio remembers
  the choice.

### Layers, adjustment layers and masks

- Layers panel: per-kind icons, an eye on every row, a clip arrow and indent for
  clipped layers, an underlined clipping base, an `fx` badge and a mask chip.
  A bar above the list sets the selected layer's blend mode (16 modes, plus
  Pass through for groups), Opacity and Fill. A toolbar below it adds an
  adjustment layer, clips, groups, duplicates and deletes. The right-click menu
  adds Create/Release clipping mask, Mask with selected shape, Invert mask and
  Delete mask.
- New adjustment layer (13 kinds: levels, curves, brightness/contrast, hue/saturation,
  vibrance, exposure, color balance, black & white, channel mixer, gradient map,
  invert, posterize, threshold) inserts directly above the selected layer,
  outside its clipping group, and is selected. A version 4 or 5 document is
  upgraded to version 6 first, as one undoable step.
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

## Engine

- A clipped adjustment (scope `targets` naming only the clipping base) no
  longer ends the clipping group, so layers above it can stay clipped to the
  same base, as in Photoshop. Hiding a clipped adjustment no longer releases the
  layers clipped above it at render time. Documents that use this shape fail
  validation in 0.21.x and earlier.

## Server

- New `GET /api/document/events` server-sent event stream that reports the shared
  document's revision on connect and on every change. Works with or without
  `--collab` (view access required when collaboration is on). See
  `docs/PROTOCOL.md`.
- Error responses (`400 {"error": ...}`) now include the cause chain, so a
  failed `/api/scene` batch says why, for example
  `operation 0 failed (page default, layer -, group -, object base): [missing-mask] mask nope missing`
  instead of only the operation context.
