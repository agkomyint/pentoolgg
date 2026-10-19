# Layer and adjustment model: Photoshop comparison (study, not approved)

Status: draft for maintainer review. Nothing here is implemented or scheduled.
It records how Photoshop's layer stack behaves, how Pentool's compositor behaves
today (verified in `src/composite.rs`, `src/raster/`, `src/mask.rs`,
`src/effects.rs`), and a staged plan to close the gaps that block more tools.

## Why

Retouching tools (heal, clone, dodge/burn, liquify, content-aware fill) and
non-destructive editing all assume one predictable answer to "what pixels does
this layer see, and what does it change?". Photoshop users expect:

1. click an image layer, heal on it (or on an empty layer above it);
2. add an adjustment layer and have it affect everything below;
3. clip an adjustment to one layer with Alt-click;
4. paint a mask to limit any of the above.

Pentool supports parts of 2-4 with different rules, and supports 1 only on
`raster` paint layers. That gap made heal feel broken in Studio.

## Photoshop's model

- **One stack per document.** Layers composite bottom to top. Groups nest.
- **Adjustment layers** apply to the composite of everything below them in the
  same stack, up to the nearest isolating boundary.
- **Groups** are *Pass Through* by default: children, including adjustments,
  act as if the group were not there. A group set to *Normal* (or any other mode)
  is isolated: its children composite on their own, then the result blends
  as one layer, so its adjustments affect only the group's contents.
- **Clipping masks**: a clipped layer is drawn only where its base layer
  (the lowest unclipped layer under it) has alpha. A clipped adjustment affects
  only its base. Clipping is the everyday way to adjust one layer.
- **Masks**: every layer, group and adjustment can carry a pixel mask and a
  vector mask. Both are multiplied together. Masks can be disabled or unlinked,
  and the transform can stay linked to the content or not.
- **Opacity vs Fill**: *Opacity* fades the layer and its layer styles. *Fill*
  fades only the pixels, so a drop shadow stays at full strength.
- **Smart Objects** hold the source (a raster, vector or nested document) and
  transform without loss. **Smart Filters** are a filter stack on a Smart Object
  with one shared filter mask, re-run from the source.
- **Fill layers** (solid, gradient, pattern) are adjustment-like layers with
  content and a mask.
- **Retouching targets the selected layer**, and each tool has a *Sample:
  Current Layer / Current & Below / All Layers* option. The professional
  workflow is to heal onto an empty layer with *All Layers* sampled, which
  leaves the photo untouched. Healing a Smart Object directly is refused with
  a prompt to rasterize.
- **Locks**: transparent pixels, image pixels, position, artboard, and all.
- **Stamp visible / merge** produce flattened pixel layers from the composite.

## Pentool today

| Concept | Pentool | Gap |
| --- | --- | --- |
| Stack | A page has `layers`, each with ordered `nodes`. `render_scene` composites **each page layer on its own** (`renderer.siblings(layer.nodes)`), then lays it over the result with `normal`. | Page layers are always isolated. An adjustment in an upper page layer cannot change content in a lower page layer. Photoshop has no such boundary, so this surprises users. |
| Adjustment layers | `kind: "adjustment"`, 13 adjustments, `opacity`, `blend_mode`, `blend_space`, optional `mask`, `scope`. | Scope is explicit: `below` (earlier siblings in the same list), `group(id)` or `targets(ids)`. `targets` has no Photoshop equivalent. Photoshop expresses the same thing with stack position plus clipping. |
| Groups | `isolation: "pass-through"` or isolated. | Matches Photoshop. Pass-through renders the children twice (once isolated, once over the backdrop), which costs memory budget. |
| Clipping | `clipping: {base}` on a node. Consecutive clipped siblings share the base alpha. | Close to Photoshop. Adjustments can be clipped, but `scope` and `clipping` overlap in meaning. |
| Masks | Raster mask (`resource`), vector mask (`kind: "vector"`), invert. | No separate pixel + vector mask on one node, no "disable mask" flag, no link/unlink of transforms. |
| Opacity / Fill | `opacity` only. | No separate fill opacity, so effects cannot stay at full strength while the pixels fade. |
| Layer styles | `effects`: drop/inner shadow, outer/inner glow, blur, fill overlay. | A reasonable subset. |
| Smart objects / filters | `image` and `photo` nodes keep their source and transform without loss, which works like a Smart Object. | No filter stack (adjustments or blur) attached to one image node with a shared filter mask. Today you clip an adjustment instead. |
| Pixel layers | `kind: "raster"`: tiled, journaled strokes, replayable. Heal, clone, brush and selection work only here. `raster rasterize` turns a node into a raster layer. | Selecting an `image` node and healing does nothing useful. There is no "heal on an empty layer, sample the composite" path. |
| Heal | `src/raster/heal.rs`, ALGORITHM 1, Gauss-Seidel solve, `MAX_REGION` 1024, solve budget 64M. | Sweeps grow with region size, so a 419x408 brush region needs 87.5M and is refused. |
| Locks | `locked` on nodes and layers. | Only all-or-nothing locking. |

## What "stable" should mean before more tools

A tool author should be able to answer these questions from the format alone:

1. **What does a node see?** The composite of everything below it in one page
   stack, cut at isolated groups, the same way as Photoshop.
2. **What does a node change?** Its own pixels, or (for an adjustment) the
   composite below it, narrowed by clipping and masks. Nothing else.
3. **Where do retouching pixels go?** Onto a target raster layer. The source is
   chosen by `sample: current | below | all`, and an image node is never
   mutated in place.
4. **What does it cost?** It must stay inside the existing budgets, with
   algorithms that scale with the region size, not with sweeps × size.

## Staged plan

Each stage is a separate minor release with its own roadmap file, tests and
goldens. None of them belongs in a 0.22.x patch except where noted.

### Stage 0: make today's tools usable (candidate for 0.22.0, needs approval)

- Heal ALGORITHM 2: a multigrid or coarse-to-fine Poisson solve whose work is
  about proportional to the region's pixels, so 1024² fits the budget.
  ALGORITHM 1 stays for replaying existing journals.
- Studio: when heal or clone targets an `image`/`photo` node, stop silently
  failing. Explain why, and offer "Add a retouch layer above" (an empty raster
  layer clipped to that node).

### Stage 1: one page stack (format change, migration)

- New page flag (or document version) for unified stacking: page layers act
  like top-level pass-through groups, so `scope: below` sees lower page layers.
- Older documents keep isolated page layers byte-for-byte. A migration opts
  them in explicitly.
- Goldens: one adjustment over two page layers, rendered both ways.

### Stage 2: retouch onto a layer (sample modes)

- `heal-stroke` and `clone-stroke` gain `sample: current | below | all`. The
  source pixels come from the compositor snapshot below (or including) the
  target raster layer, and the result is written only to that layer.
- Studio defaults heal and clone to "new empty layer, sample all", like the
  usual Photoshop workflow.
- The journal records the sample mode and the source digest so replay stays
  deterministic.

### Stage 3: layer properties parity

- `fill_opacity` separate from `opacity`.
- Granular locks: `pixels`, `transparency`, `position`.
- A node may carry both a pixel mask and a vector mask, with `enabled` and
  `linked` flags.
- Decide whether `scope: targets` stays as an agent convenience that the
  migration expresses as clipping, or is deprecated.

### Stage 4: smart filters

- `filters` on image and photo nodes: an ordered list of adjustments and
  effects with one shared filter mask, rendered from the source.

### Stage 5: Studio layers panel

- One tree per page that shows the order exactly as composited: adjustments
  in place, a clipping indent and arrow, mask thumbnails, and clicking a mask
  to make it the paint target. Alt-click between rows to clip. Each row also
  shows its blend mode, opacity and fill.

## Adjustment layers in Studio (audit, 2026-10-19)

The engine already has Photoshop-style adjustment layers: an `adjustment`
node (v6+) with one of 13 kinds, `params`, `enabled`, `opacity`, `blend_mode`,
`scope`, an optional mask, and clipping. Studio hides almost all of it:

| Photoshop | Pentool engine | Studio today |
| --- | --- | --- |
| Adjustment layer is a row in Layers | `adjustment` node in the tree | Generic row, no icon, kind as text |
| "New adjustment layer" button | `adjustment-add` batch op | Composite panel, raw JSON params |
| Inserted above the selected layer | Any position | Always `page.layers[0]`, ignores selection |
| Properties panel with sliders | Validated params | JSON textarea only |
| Eye toggles the effect | `enabled` | Not exposed |
| Layer mask on the adjustment | `mask` | Attach by mask name only |
| Alt-click to clip to layer below | `clipping: {base}` | Pick a base ID from a list |
| Copy/drag to another image or file | Node clipboard (same doc kind) | Works only v6 to v6; v4 paste fails at save |
| Adjustment presets | `.penpreset` (appearance only, CLI) | None |
| Works in any document | Needs v6 | "Migrate with the CLI, then reload" |

Studio also has three adjustment systems that cannot exchange settings:
Photo develop (catalog, Lightroom-like), image `operations` (per image, close
to Smart Filters), and adjustment nodes (close to Photoshop adjustment layers).

### Stage A: adjustment layers as real layers (Studio only, no format change)

1. One-click upgrade to v6 from Studio when adding the first adjustment, the
   same as the existing v5 offer for images (explicit and undoable).
2. A Layers panel "New adjustment layer" menu (13 kinds) that inserts
   directly above the selection in the same parent, `scope: below`.
3. Adjustment rows: a per-kind icon, an eye bound to `enabled`, blend and
   opacity, a mask thumbnail, a clip arrow and indent.
4. A properties panel that reuses the ImageAdjust sliders and curves editor
   with live preview, and commits one undo step on release.
5. Clip to the layer below: Alt-click between rows or a menu item (`clip-add`
   to the previous sibling); release with the same gesture.
6. Copy, paste and duplicate adjustment rows within and across documents.
   Pasting into a v4/v5 document offers the v6 upgrade instead of failing at
   save.
7. Drag an adjustment row onto an image row: clip it to that image.

### Stage B: moving settings between systems

- "Save adjustment as preset" and an Adjustments library, so one look applies
  to other images and documents.
- Convert both ways between an image's `operations` and adjustment layers
  clipped to it, where the kinds overlap.
- Photo: "Add with develop as adjustment layers" as an alternative to the live
  `photo` node, for users who want to keep editing on the canvas.

Stage A is UI work on existing operations and fits a minor release. Stage B's
preset library may need a format decision. The page-layer isolation (Stage 1)
still limits `scope: below` to one page layer until it lands.

## Open questions for the maintainer

1. Is the Stage 0 heal fix allowed in 0.22.0? It is a new algorithm version,
   not a format field, but it does change output for new strokes.
2. Unified stacking: a page-level flag, or a new `.pen` version?
3. Keep `scope: targets` for agents, or move everything to clipping?
4. Order of Stages 2-5. Stage 2 (retouch layer plus sample-all) gives users the
   most visible win; Stage 1 is the most fundamental.
5. Stage A (adjustment layers in Studio) in 0.22.0, or the next minor?
