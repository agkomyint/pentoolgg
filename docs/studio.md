# Pentool Studio (web-ui)

`web-ui/` is the TypeScript source of the new browser UI: Vite, React 18,
Tailwind CSS 4 and shadcn-style components (Radix primitives, `cva`), with
Zustand for state. It is served at `/` (and `/studio`) and is the only browser editor; the earlier
vanilla-JS editor (`/classic`) was retired once its photo and raster panels were ported.

## Product properties

- `cargo build` never needs Node or Bun. `bun run build` produces one
  self-contained `web/studio.html`, which is committed and embedded in the
  binary (`include_str!`). Rebuild and commit it whenever `web-ui/src` changes.
- The server stays the source of truth. Studio uses the embedded WebAssembly
  renderer for supported v6-v9 previews, including raster tiles, and falls back
  to `/api/render/png` when local rendering is unavailable. Raster edits still
  commit through `/api/raster`; the browser only draws the in-progress stroke.
  Release builds embed the renderer built for `wasm32-unknown-unknown`.
- One application bar holds file actions, a workspace selector, page selection,
  active drawing/brush settings, save, and export. The left rail keeps vector,
  paint, and retouch tools together. The right dock has Properties, Tools, and
  Assets above a separate layer tree, so selecting an object keeps layers visible.
  Both the dock width and layer area can be resized with a pointer or arrow keys.
- Visited canvas stages remain mounted. Hidden stages pause their global shortcuts
  and preview requests; Design retains its viewport, and Design/Paint share zoom
  and page framing. They still have separate interaction surfaces. An unchanged
  document snapshot can reuse a bounded preview cache; editing creates a new
  snapshot. This cache does not change exported pixels or the saved format.
- Workspaces map to the roadmap (`web-ui/src/lib/workspaces.ts`): Design,
  Images (0.7), Composite (0.8), Photo (0.11), Print (0.12), Paint (0.13),
  Layout (0.14), 3D (0.15). A workspace whose format or CLI is
  not implemented says so and lists what the roadmap promises; it has no
  controls that cannot work.
- Serving an existing project opens its editor directly. A bare server opens a
  welcome screen with continue, `.pen` open, and common canvas presets. The Home control returns there without closing the current
  document; choosing a preset creates a canvas at that pixel size.

## Develop

```sh
pentool serve artwork.pen --port 7878      # terminal 1
cd web-ui && bun install && bun run dev    # terminal 2, proxies /api to :7878
bun run build                              # typecheck + refresh web/studio.html
```

## Status

Editing (document JSON edited in the browser, validated by Rust on render and
save, one undo step per edit): select/move/resize, pen (bezier, close, finish),
rectangle, ellipse, line, text, hand/pan/zoom, delete/duplicate/group/ungroup,
z-order, layers (visibility, lock, rename, add, delete), pages, canvas size and
background, paste/drop images (`/api/image/add`), embed fonts, import `.pen`,
SVG / PDF / PSD / Sketch / Figma REST JSON / supported Figma `.fig` imports with
an in-panel fidelity report,
asset library search, property inspector (geometry, fill, stroke, radii, text),
raw node JSON, a scene-batch console, Images (operations stack), Composite
(blend, opacity, adjustments, masks, clips; needs format v6), Print (preflight,
plates, soft proof), New/Open/Save (shared file or `.pen` download), current-page
PNG/SVG export, document PDF/EPUB export, dark mode.

The welcome screen, grouped workflow navigation, contextual Design modes, Paint
tool rail, and tabbed inspector make the existing capabilities easier to find
within one project. Photo, raster painting, and 3D retain specialized stages
where their tools need a different interaction surface. This redesign does
not change `.pen` object structure or turn CLI-only operations into UI actions.

Print preflight also lists every placed 3D mockup's rendered pixel dimensions
and effective X/Y PPI. Selecting a resolution row selects that mockup on the
page; below-threshold rows also appear as ordinary actionable preflight issues.

Photo (DNG, JPEG and PNG import from the Photo workflow into a shared `pentool serve PROJECT.pen`
document, up to 64 MiB per file; JPEG/PNG use the rendered-image editing path, while camera white balance,
camera profiles and RAW-specific controls require DNG; catalog grid,
loupe, compare, culling keys, histogram and clipping overlays, develop sliders,
white balance, crop, radial/linear/brush masks, copy/sync, variants, snapshots, keywords) and Paint
(raster brush, eraser, clone, heal, quick mask, marquee/lasso/wand selections, rulers, guides, snap,
navigator) workspaces use the same `/api/photo/*` and `/api/raster` endpoints as before.
Known limits: hit-testing is bounding-box based; during a drag only the
selection box moves and the rendered preview updates on release. The PDF
workflow exports tagged book/layout PDFs; use `pentool export x.pen x.pdf` for
the direct vector document exporter. Larger DNG sources can
be imported with `pentool raw add`. Design interchange is
currently partial. Active SVG elements, event handlers, foreign namespaces, and
external references are rejected; unsupported static features are reported as
dropped, and supported import subsets are listed in
[`design-interchange-v1.md`](design-interchange-v1.md). Figma auto-layout is
materialized as fixed positions; only legacy unwrapped Figma `fig-kiwi` v15
Deflate files and REST JSON are supported. ZIP/Zstandard, FigJam, and other
`.fig` versions are rejected. PDF import includes classic cross-reference tables,
plain/Flate xref streams and bounded object-stream references, vector paths,
simple one-byte `Tj`/`TJ` text runs, permitted embedded TrueType fonts, bounded
Flate/JPEG images and predictors, page quarter-turn rotation, DeviceCMYK
approximations, and matching soft/color-key masks; inline images, hybrid or
incremental xrefs, transparency groups, calibrated color, and other masks remain
unsupported. PSD import supports bounded raster layers/folders, root-level
clipping alpha, raster masks with zero, relative-position, and invert flags, and
a small legacy RGB adjustment subset; other adjustment/effect layers and mask
layouts remain unsupported. PSD export writes flattened or named full-canvas
raster layers. Sketch symbols/shared-style fidelity remains limited. Figma component snapshots
and instance links are retained with materialized fallback artwork. Unique
`TEXT` component properties named `<layer>#text` are applied to matching fallback
text nodes; other property types and live override propagation remain
unsupported. Studio does not expose component override editing.

### Extensions (0.18)

The Extensions workspace and the Ctrl/Cmd+K palette list the project's installed
sandboxed extensions with their approved capabilities and limits, then preview
(dry run) or run a command against the served document through
`GET /api/extensions` and `POST /api/extensions/run` (guarded by the document
revision, one undoable step). Installing and approving capabilities stay CLI
actions (`pentool extension add --approve`). Running is disabled while local
edits are unsaved. Not yet exercised in a live browser. See `docs/extensions-v1.md`.

### Design systems (0.19)

The Systems workspace has four tabs over the served document: Tokens (type filter,
resolved swatch, local/clean/override state, dependent counts, pagination),
Libraries (document, `pentool.lock` and installed package state), Templates (the
templates in the served folder, a parameter form built from `design template
check`, Preview and Fill) and Audit (findings by severity, near-match toggle and
"Apply fix plan"). Every view calls `/api/design/*`, which builds the same command
the CLI parses and runs it through `design::cli::run`: `GET /api/design/tokens`,
`/library/status`, `/audit`, `/templates`, `/template/check`, and
`POST /api/design/template/fill`. Errors keep the structured
`{error:{code,message,action}}` shape. Fill requires the document `revision`
(a missing or stale one is refused before any write), writes only a new
project-relative `.pen` file and never modifies the template; paths with `..` or
absolute paths are rejected. "Apply fix plan" sends the audit's operations to the
existing `/api/scene` route (revision guard, one undoable transaction). Library
install/update, brand apply, token editing and template thumbnails are not in Studio
yet (CLI only). The HTTP routes are covered by tests; the panel itself was built
and type-checked but not exercised in a live browser.

### Paint plan authoring (0.13)

The Paint workspace's "Plan passes" section authors an ordered paint plan: path or
pressure-sample editors, color, brush size and hardness, per-pass regions,
protected regions and palettes, and pass ordering. An overlay shows regions,
protected areas and the changed bounds returned by each pass (amber when a
measured warning applies). "Preview plan" and "Commit" both call
`POST /api/paint/plan`, so the report is the authoritative Rust output, never a
browser estimate; commit is one undoable step guarded by the plan hash.
"Score preview against goal" calls `POST /api/paint/score`. "Palette from canvas"
and "Sample point" read the rendered canvas through `POST /api/paint/palette` and
add colors to the pass palette; "Compare with reference" uploads a reference image
to `POST /api/paint/compare` and shows the metrics with a difference overlay;
"Crop preview" uses `POST /api/paint/preview`; "Show field vectors" draws a stroke
field on the plan overlay via `POST /api/paint/field`; the per-pass Material list
comes from `POST /api/paint/media` and sets `brush.media`. Plans appear as
normal editable passes, not a chat surface, and the panel states that every pass is
generated work and which warnings are measured versus heuristic.

## Design system and shell

Studio uses a compact neutral editor theme with one application bar, a left
creative tool rail, the central canvas, and a resizable inspector/layers dock.
Properties follow the selected object. With nothing selected, they show document
settings. Tools contains the active task's controls; Assets contains import and
library actions. B selects Brush, E selects Eraser, and V returns from Paint to
vector selection. These shortcuts leave text fields and specialized photo/3D
keyboard interactions alone.

Use the panel toggle in the header to collapse the dock. On narrow screens it
opens as an overlay; extra file actions move into the compact menu. Tool feedback,
errors, zoom, and page position appear in a single status bar.

Tokens live in `web-ui/src/index.css` as oklch custom properties (`background`,
`card`, `input`, `muted`, `border`, `primary`, `accent`, `destructive`,
`warning`, `success`); components use the Tailwind names, never raw colors.
Base font size is 14px for density. Shared primitives are in
`web-ui/src/components/ui/kit.tsx`: `Section`, `Row`, `NumberField`,
`Segmented`, `Check`, `Kbd`, `Empty`, plus `Button`, `Badge` and `Tip`.

### Layout workspace

The Layout module edits `.pen` v9 layout through `POST /api/layout`
(`{document, op}`); the browser never measures text. Composition, overset and
preflight come from `GET/POST /api/layout/{overview,preflight,export}`. The
optional InDesign-style keys are toggled in Layout > View.

### 3D workspace

The 3D module can create and edit bounded box/card/device-frame, gusseted
pouch, cylinder, open-tube, and revolved bottle/jar forms in millimeters. Its
editable starters include perfume bottles, cosmetic jars, tubes, shipping
cartons, product cards, a simple device-body mockup, and a conceptual stand-up
pouch. Form changes go through
`POST /api/scene3d/form`, where the shared Rust generator and document validator
check the complete candidate before Studio records it as one undoable edit.
Bottle profiles expose editable radius/height control points; the base and total
height endpoints stay anchored, and invalid point ordering is rejected without
changing the document. The profile graph supports pointer dragging and arrow-key
editing of interior points, with numeric fields retained for precise values.
Select a point to reveal its tangent handle; dragging the handle or using arrow
keys edits the bounded local radius/height slope. Point or tangent gestures each
commit once after Rust validates the complete profile. Product forms
require document format v11. The 3D module
also imports embedded GLB, glTF, and OBJ files through
`POST /api/scene3d/import`; the Rust importer applies format and resource limits
before returning an undoable document replacement. Selected `scene3d` nodes
expose orbit, target pan, zoom, light-rig, material-override, and linked surface
artwork controls. Artwork can link a page or drawable node, target a material
slot, and configure decal/fill mode, UV/planar projection, fit, tile wrapping,
normalized source crop, blend, opacity, scale, offset, and rotation. The center
stage pairs the selected model's UV layout with a bounded, CPU-rendered 3D
viewport. Imported models and generated product forms share the same UV
workspace, renderer, and editable document overlay. Selecting an island
highlights its triangles in a temporary, uncached render. A page-context toggle
opens the ordinary editable page canvas without losing the target model. Select
a page object there and drag its accessible “Drag selected art to UV” handle
onto the UV layout to create a linked decal on the selected primitive's actual
material slot; the drop point sets the decal center. Existing form controls
remain available as a keyboard-friendly alternative and for fills, crop/fit,
repeat/mirror, and slot selection. Locked source layers disable the drag handle;
locked target layers, unavailable slots, and stale/locked sources report an
inline status instead of silently dropping the artwork. The model preview is
bounded to a 512px longest edge. Users can apply supported automatic
projections, create islands from triangle indices, and pack islands with
configurable padding through
`POST /api/scene3d/uv/pack`, select an island, and edit its scale/flip, offset,
and rotation. Packing returns a validated document replacement as one undoable
action. Source model bytes remain unchanged, and UV edits use ordinary v11
document data. Studio does not implement a second 3D renderer. The UV header
selects a square texture resolution and optional minimum density; per-set and
selected-island density is shown as pixels per model unit, with below-target
warnings when requested. Possible shared-edge UV discontinuities are called
out in the report and highlighted in red in the SVG preview; very large
primitives can report that bounded seam analysis was truncated. Existing
UV-projected decals appear as movable rectangles on the layout. Drag to
reposition, or use the center, scale, rotation fields, or arrow keys on the
outlined decal for keyboard-accessible placement; release records one undoable
document edit and refreshes the adjacent 3D preview. Clicking the 3D preview
picks the nearest visible face and its UV island; “Place art on face” adds a
linked decal at that hit with the face's actual material slot. Both imported
models and generated forms support this path. Both SVG and server-rendered PNG
layouts are downloadable.
Preview rendering remains progressive through the debounced native renderer.
Selected v11 mockups can also enable an explicit ground plane, edit its
color/footprint/offset, and choose an opaque floor or transparent shadow catcher.
The catcher exports direct-light shadow occlusion over transparency; keep scene
shadows enabled and the background transparent when compositing the PNG.
The panel reports the active reference renderer plus any explicit accelerator
fallback reason from `GET /api/scene3d/status`.

The panel can create an open sleeve, straight-/reverse-tuck carton, or hinged-
lid mailer mockup from length, width, height, board, and bleed values; carton
and mailer creation also takes a glue-flap width. `POST /api/scene3d/sleeve` and
`POST /api/scene3d/carton` each build the linked dieline and preview node as one
validated document change. The carton is a simplified visual net with four
body panels, closure lids, tuck tongues, and dust flaps. The mailer has a base,
four walls, a hinged lid, and a side glue joint. Board and bleed are metadata
only; these outputs are design previews, not production dielines. The mailer
does not yet model lock tabs or material thickness. Generated carton and mailer
cut plates trace closed exposed outlines, keep shared non-crease flap edges as
through-cut slits, and leave stored crease edges off the cut plate.

Generated packages also include a linked flat-layout page in the same `.pen`
document. Choose **Edit flat layout page** in the 3D inspector to open it in the
normal design canvas. Its panel, safe-zone, cut, and crease shapes remain
ordinary editable nodes; stable metadata IDs resolve changes back into flat
exports, preflight, and the folded preview. Linked artwork continues to use its
source page and the paired flat-preview placement controls.
To turn a newly drawn path on the layout page into a production mark, select
the path and choose its role in the Design panel. `POST /api/scene3d/path-role`
validates the selected path and assigns a stable path ID, canonical spot-plate
name, and matching role color in the layout canvas. Cut, crease, perforation, bleed,
safe-zone, and glue-flap marks then flow through the same flat SVG, preflight,
vector plates, and print bundle.
An ordinary path can also be assigned the **crease** role for freeform scoring;
it appears on the separate blue Crease plate but does not create or alter a
structural hinge in the folded mockup. Structural folds still require the
validated shared-panel-edge operation below.
Safe-zone is a review guide, never a cut. To turn an authored straight path into
a structural fold, give it a stable ID, target angle, and mountain/valley
direction in the Design panel. `POST /api/scene3d/path-crease` accepts it only
when its endpoints exactly match a shared edge of two panels; freeform, partial,
or transformed paths remain ordinary artwork and cannot drive the fold solver.
The resulting hinge updates the paired flat/folded model and appears in the
vector and raster Crease finishing plates, even when no separate crease-role
path was stored.
Selecting a panel node on that page also selects its stable folded-face ID;
**View folded mockup** returns to the linked scene node with the same face
selected for the 3D highlight.

The same creator offers a five-panel stand-up-pouch mockup (`front width`,
`gusset depth`, `height`) and a cylindrical bottle-label band (`label-zone
radius`, `height`, `overlap`) through `POST /api/scene3d/pouch` and
`POST /api/scene3d/bottle-label`. These are validated, atomic document changes.
The pouch omits zipper/seal and film behavior; the bottle label uses a constant
radius and does not conform to tapered bottle profiles. Neither is certified
manufacturing geometry.

For a selected dieline-backed object, the 3D inspector also shows a 0–100%
package fold control. Dragging previews a throw-away document through the Rust
renderer; releasing sends the candidate through `POST /api/scene3d/fold` for
Rust validation, then commits one undoable `fold_state` edit. The inspector
also lets you choose a crease, change its target angle, and switch between
mountain and valley folding. Studio sends those edits through the validated
crease APIs; each successful edit is one undoable document change and updates
the folded preview without changing the shared fold amount. The shared slider
previews every crease proportionally toward its own target. This first solver supports
connected rectangular panel trees with matching shared-edge creases. A positive
board thickness adds inner panel faces and exposed cut-edge walls, and offsets
each folded hinge axis by half the stored thickness to keep exterior seams
connected. A simple untextured bridge fills each folded interior hinge wedge;
it does not simulate score compression. Allowances and validated structural
behavior are not modeled. Collision findings remain a limited screen, not
structural validation.

The same inspector lets you edit stored board thickness and print bleed after
creation. Changes use `POST /api/scene3d/pack/settings`; bleed updates flat
export bounds, while thickness drives the panel surfaces, edge walls, hinge-axis
offset, and limited overlap preflight. The equivalent CLI is
`pentool pack DOC settings DIELINE --board-mm MM --bleed-mm MM`; either value
may be changed alone, and both support dry-run and revision-guarded document
transactions.
For tube and bottle-label dielines, the same settings action also accepts
`--seam-angle-deg DEG` and `--wrap-direction clockwise|counterclockwise`.
Studio exposes those fields in the package inspector; changing them updates the
flat seam marker and the folded cylinder together. Seam angles are bounded to
-360..=360 degrees, and wrap settings are rejected for non-cylindrical nets.

The inspector's **Preview flat** action uses
`POST /api/scene3d/pack/flat-preview` with the current document, dieline ID, and
the selected print-raster PPI threshold (300 by default). Rust returns a bounded SVG from the same exporter used by
the CLI, including linked artwork and resolution warnings. Drag linked artwork
within its panel to reposition it; the dashed box previews the placement while
dragging, and release stores an undoable shared flat/fold transform through
`POST /api/scene3d/panel-art-transform`. The flat workspace also provides
panel-local size and 15-degree rotation buttons for linked artwork; each click
uses that same validated route and commits one undoable change. Scale, center,
and rotation fields provide precise keyboard-accessible edits. The stable panel
ID identifies the same folded face. The panel's stored UV rectangle is composed
with its normalized artwork transform for the folded surface binding, while the
flat SVG keeps the transform in panel-local millimeters; changing only the UV
atlas rectangle should preserve the visible art on the corresponding face.
In the 3D workspace, clicking a visible folded face now uses the
shared CPU camera ray to select its panel and highlight the flat counterpart.
The flat dieline view also assigns a linked page or drawable node directly to
the selected panel; Rust validates the binding and returns the updated document
through the same transactional panel-art route used by the inspector.
Pages and drawable nodes can also be dragged from the Layers outline or Design
panel onto a flat panel. The target highlights during the drag and reports the
link result inline; the source picker remains available as a keyboard-friendly
alternative. The selected flat panel also exposes its safe-area inset in
millimeters, using the same validated, undoable document mutation as the 3D
inspector; the guide and preflight warning refresh from that saved value. When
artwork is linked, exact normalized X/Y scale and center plus degree rotation
fields are available beside the quick transform buttons, so placement can be
entered numerically without leaving the dieline view. Selected artwork also
shows direct resize and rotate handles on the flat panel; drag to preview and
release to save one edit. The 42 px handles support arrow-key resizing and
rotation as well as pointer gestures.
For a linked panel, **Edit source artwork** switches to that source's ordinary
Studio page/canvas (selecting the linked node when the source is a node), so
vector and text edits continue to use normal `.pen` tools and remain live-linked
to the packaging preview. Return to the package through the 3D workspace and
select its scene node again.
Selecting the same panel in the flat workspace (or from the panel selector)
also requests a transient CPU-rendered highlight of that panel's folded faces
through `POST /api/scene3d/pack/face-preview`. The preview is bounded and does
not change or dirty the document.
This placement surface is not yet a general editable 2D canvas.

The packaging preflight report also reports whether the document has an ICC
CMYK output role for the flattened PDF/X draft. Studio displays the Rust
planner's result and points to the import/assignment commands when it is
missing; it does not infer print readiness in the browser. The report also measures the actual folded mesh's shared
hinge endpoints (and cylindrical wrap seam) against a scale-aware tolerance.
It scans non-adjacent rectangular face interiors at the current fold amount and
lists potential zero-thickness intersections by panel pair. Edge-only contact
is ignored. This is a geometric design check, not a physical board/film
thickness, gap, or structural simulation.

Each panel can store a safe-area inset in millimeters through
`POST /api/scene3d/panel-safe-zone`. The flat preview draws this guide; preflight
checks the rotated bounds of the linked source viewport against it. This is
conservative guidance, not pixel-content analysis or print certification. Stored
bleed expands flat SVG/PDF page dimensions. Panel guides, linked artwork, and
Studio's selectable panel regions share the same bleed-offset origin. Eligible
rectangular cut edges use bounded nearest-edge bleed; an explicitly associated
closed cut path clips panel artwork and synthesizes nearest-contour bleed in the
flat export. Crease edges and wrap seams remain excluded for rectangular-edge
bleed. A simple closed panel contour also shapes the folded face using the same
panel coordinates and must preserve complete shared fold edges; contours with
holes are rejected rather than silently approximated. Production certification
and board-thickness structural compensation are still not provided.

Each package face has a panel-art selector for linking a page or drawable node.
Changing a selector uses `POST /api/scene3d/panel-art`; Rust validates the
candidate dependency graph before Studio records one undoable document change.
The link stays editable in the source document and renders onto that panel's
material slot.

The packaging inspector's “Run preflight” action calls
`POST /api/scene3d/pack/preflight` with the current document, dieline ID, and the
selected print-raster PPI threshold (300 PPI by default). It is read-only and reports unlinked panels, missing cut or
crease paths, linked raster resolution, and configured safe areas. The report
is a mockup/design check only: it never certifies manufacturing readiness and
lists blockers including solid-board/cut-fold allowance compensation, structural validation,
and the fact that PDF/X is flattened process CMYK while technical spot plates
are separate outputs. It also counts closed subpaths in each production path
and warns when a cut-role path has no closed contour, which often indicates a
missing outer through-cut outline; intentional open slits are still allowed.
It also checks stored production paths for self-intersections, so an edited
bow-tie cut contour is flagged before exporting plates. Excessively complex
paths fail with a bounded-work diagnostic rather than being silently skipped.

Use **Export print bundle** in the flat dieline workspace to download a ZIP of
the current document snapshot. The server uses the shared packaging exporter;
the flattened PDF/X draft requires a verified CMYK output profile assigned to
the document. Studio shows this prerequisite when it is missing; import and
assign a licensed profile from the CLI before exporting, for example:

```sh
pentool color profile import artwork.pen press.icc --name press --redistribution allowed
pentool color role assign artwork.pen output --to '{"space":"icc-cmyk","profile":"press"}' --intent relative-colorimetric
```

set **Print raster PPI** before exporting (72–1200, default 300). This value is
used both to rasterize technical finishing plates and to set the linked-image
resolution target in preflight/flat preview; vector-compatible artwork stays
vector. Choosing a higher target does not add detail to a low-resolution source:
the affected panel is still reported below target. The selected value is applied
to the exported bundle's plates and image sampling.
the bundle contains `flat.svg`, a color-keyed `CUTTING-MAP.svg` for visual
review, a flattened process-CMYK `print-draft.pdf`,
`folded-preview.png`, vector spot plates, grayscale technical plates,
`manifest.json`, and `PRINT-GUIDE.txt`. The PDF does not contain cut or crease
plates: `CUTTING-MAP.svg` is only a visual reference; send the named SVG spot plates as separate cutter/finisher separations
and follow the included guide: the red Cut line / `CutContour` is a through-cut,
while the Fold / `Crease` lines are scoring operations, not through-cuts. The
reference map draws valley folds as solid blue and mountain folds as dashed
violet; check the folded preview to confirm the assembled direction.
The folded PNG is an assembled-look review only; it is not a production proof.
Bundle verification checks listed artifact hashes and paths, confirms the
cutting map retains its reference-only production legend, and checks each
vector spot plate has one role under its declared ink. It also checks that the
map's production path geometry matches `flat.svg`. It does not verify
structural or manufacturing conformance.
The flat view's SVG and legend map technical marks to their display styles: red
cut, solid blue valley fold, dashed violet mountain fold, purple perforation, orange
bleed, green safe zone, and cyan glue flap. Safe-zone guides are explicitly not
cut paths, and only marks present in the current dieline are drawn. Each production operation must use its own spot
ink: export refuses a dieline that assigns one ink name to multiple roles, such
as cut and crease, rather than merging distinct finishing instructions onto a
single plate.
