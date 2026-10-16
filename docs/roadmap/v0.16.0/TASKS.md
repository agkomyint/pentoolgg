# v0.16.0 — 3D product and packaging design

Turn the static 3D capability from v0.15.0 into a product-design and packaging
workflow. A designer can create supported product/package forms, control their
UVs, wrap editable `.pen` artwork around surfaces, preview materials and lighting
in 3D, and prepare a corresponding flat print-and-cut handoff. The workflow must
make three things easy to inspect together: the assembled package in 3D, the
full-scale printable flat artwork with bleed, and the separate vector paths
that indicate where to cut, crease, or perforate. Example jobs include a
perfume bottle with a curved label, a tube or jar, a folding carton for a
T-shirt, and graphics applied to an imported custom model.

This is a feature milestone. It extends the v0.15.0 `scene3d` node, model
catalog, renderer, and UV overlay; it does not create a second document model or
replace the deterministic CPU reference renderer. It is not a general sculpting
or free-form mesh-modeling application: users can author supported parametric
forms and UV/wrap designs, and import glTF/GLB or OBJ for other shapes.
Because v0.15.0's 3D contract is document format v10, these additions use a
new document format v11. v10 remains unchanged; migration to v11 is explicit,
and downgrade to v10 refuses documents that contain v11-only data.

The interaction goal is a simple, self-contained product studio: choose or make
a form, drag a graphic onto it, position/rotate/scale the graphic on the surface,
edit UV placement when needed, and inspect the result under controllable
materials and lights. Adobe Dimension's documented graphics workflow is a
reference for direct placement and decal-versus-fill behavior; Pentool keeps
its own data model, offline workflow, and print/dieline connection. See
[Adobe's material and graphics workflow](https://helpx.adobe.com/dimension/desktop/using/apply-materials-images-to-models.html)
and [UV projection overview](https://helpx.adobe.com/dimension/desktop/features/material-projections.html).

## Product contract

- A 3D object remains an ordinary `.pen` scene node. Live artwork bindings,
  model sources, mockup settings, and dieline metadata are versioned document
  data and survive open, save, undo, redo, package, and migration.
- A texture binding references a page or node by stable ID. It does not copy or
  flatten the source artwork into the document. The source page/node remains
  editable and its changes update dependent mockups.
- Dependencies are explicit and acyclic. Cycles, missing IDs, missing material
  slots, incompatible source kinds, and invalid UV mappings fail before a
  mutation or render; they never become blank or stale mockups silently.
- The same source document, model assets, renderer version, and settings produce
  deterministic output on the reference renderer. An optional accelerator may
  improve preview speed but never defines exports, print separations, goldens,
  cache identity, or conformance results.
- A mockup render is resolution-aware. Its report includes output pixel size,
  effective ppi at placed size, renderer path, cache status, and any warnings.
  Print preflight uses the effective ppi at final placement, not a nominal
  texture size.
- Surface artwork has two distinct modes: a localized decal placed on a
  surface, or a fill that covers/repeats across a material region. The user can
  move, rotate, scale, crop, repeat, mirror, and set surface/material response
  without baking the source artwork into an uneditable image.
- Supported parametric forms are editable by dimensions/profile after creation.
  General mesh topology editing, sculpting, and arbitrary retopology are not
  implied by UV authoring.
- Ordinary 2D documents and existing 3D documents without new bindings render
  exactly as before. Documents stay offline-capable; no cloud model or preset
  service is required.
- Imported model and preset licenses are recorded and inspectable. Presets use
  embedded or verified project-relative assets with bounded size and safe paths.

## Data model

- A binding identifies the consuming `scene3d` node, material slot, source
  (`page` or `node` stable ID), crop/fit, color role/intent, and UV overlay
  reference. One source may feed multiple slots; each slot has at most one
  active source binding.
- A surface graphic is a first-class editable layer with a stable ID, source
  page/node, target model/material region, projection mode (`uv` or supported
  procedural projection), transform, crop/tile settings, opacity/blend, and
  optional graphic material response (matte, glossy, metallic, or supported
  material parameters). Decal mode is localized to a surface region; fill mode
  maps to the full material region and can repeat/mirror. It must remain linked
  to source artwork so edits propagate.
- UV data records the source UV set or a Pentool-authored overlay, island
  identity, transform, seam/overlap diagnostics, and texture resolution. The
  same UV record drives viewport display, render sampling, UV-layout export,
  and print/dieline mapping; there is no separate preview-only mapping.
- Parametric form data is bounded and explicit: box/carton panels, cylinder and
  tube, lathed bottle/jar profiles, and supported label-wrap surfaces. Dimensions
  and profile points are stored as editable parameters; generated mesh and UVs
  are deterministic derived data. Imported arbitrary models remain immutable
  sources except for material, UV overlay, and graphic placement edits.
- A source may not depend directly or indirectly on the `scene3d` node that
  consumes it. Page bindings include the page's ordinary 2D content and exclude
  the consumer node itself. Node bindings resolve in the node's containing page
  and include its normal ancestor transforms and clipping.
- Render-cache keys include source content digest, model digest, UV overlay,
  material slot, color settings, camera, lights, renderer version, and requested
  resolution. Cache entries are bounded by bytes and count, evict deterministically,
  and can be rebuilt from document data.
- A packaging dieline contains a stable dieline ID, explicit units (millimeters
  for built-in print templates), parametric dimensions, named panels, and
  labeled vector paths/regions for cut, crease, perforation, bleed, safe zone,
  and glue flap. Its panel ID ↔ dieline region ↔ folded face ↔ UV rectangle
  mapping is stored once and referenced by both views.
- Fold angle is stored per crease with documented positive direction and range.
  Board thickness and glue-flap allowance participate in panel geometry so
  faces meet without overlaps or gaps at the closed/folded target angle.
- User-authored `.pen` structure is preserved. Applying a mockup or packaging
  template adds ordinary nodes and versioned metadata through the shared
  transaction/history path; it does not overwrite placed art or silently
  reparent existing content.

## Must ship, in order

- [x] **1. Freeze the v11 binding and dieline contracts.** Keep the v10
  contract/schema unchanged. Specify form parameters and generator identity,
  editable UV island ownership/transforms, graphic sources/slots/decal-vs-fill,
  dependency ordering and cycle errors, cache inputs, material/color behavior,
  and downgrade refusal. Specify dieline units, panel/face/UV identity,
  fold-angle direction, thickness, bleed, and safe-zone semantics. Update
  `docs/scene3d-v2.md` and `docs/pen-format-v11.schema.json`; add a minimal valid
  product/packaging fixture and focused invalid cases. Record v10 compatibility
  and v11 migration behavior. Frozen in `docs/scene3d-v2.md` and
  `docs/pen-format-v11.schema.json`; v10 schema and behavior remain unchanged.
  Fixtures and migration tests cover valid product/packaging documents, invalid
  references/profiles/cycles, explicit upgrade, safe empty downgrade, and
  refusal for every v11-only data category (forms, dielines, graphics, editable
  UV islands).

- [x] **2. Build a shared dependency planner.** Resolve page/node sources and
  their ancestors; detect missing/hidden/locked/unsupported sources and cycles;
  produce a deterministic dependency graph and render order. Use the same
  planner for preview, export, dry-run, CLI mutation, and batch rendering.
  A rejected plan must leave document bytes unchanged and explain the source,
  consumer, slot, and corrective action.

  Initial implementation is in `scene3d::bindings`: v11 validation resolves
  page/node IDs, rejects missing sources and cycles, tracks inherited group and
  layer visibility/locks for direct node bindings, and returns a deterministic
  dependency-first order. The CPU export path now rasterizes page/node source
  content, excludes a same-page consumer, applies crop/fit/UV decal placement,
  rotation and opacity to named slots, and bounds overlay allocation. Binding
  renders bypass the stale-prone PNG cache. UV, planar, box, cylindrical,
  spherical-axis, and normal-weighted world-space triplanar projections (which
  work without source UVs),
  repeat/clamp/mirror texture addressing, fill tile counts, and linear-light
  blend modes are implemented. `scene3d ... graphic add/set/list/
  remove` now manages stable page/node selectors and placement options through
  transactional validation/history (including dry-run and revision guards).
  The Studio 3D panel also authors linked page/node graphics and their current
  decal/fill controls. Node-source filtering now retains ancestor groups and
  transforms plus sibling clipping-base dependencies; a pixel-level CPU
  regression verifies the clip and excludes unrelated siblings. Direct v11
  preview/export also invokes this planner before loading/rendering a model.
  Missing page/node sources and binding cycles now report the source, consumer,
  graphic or dieline panel, material slot, and a corrective action; regressions
  cover both direct graphics and panel artwork cycles. `scene3d DOC
  dependencies` emits the validated graph, binding reasons, node dependencies,
  and deterministic render order as compact JSON; the command is read-only.
  The shared render path now has a structured report result carrying exact PNG
  dimensions/bytes, requested and actually used renderer (including GPU
  fallback), and per-call cache-hit status; cache entries retain the actual
  renderer identity so cached reports stay truthful. `scene3d ...
  render-report NODE` exposes this report as read-only JSON and includes the
  document revision. The server also exposes this report through
  `POST /api/scene3d/render-report`, and Studio has a typed client for it.
  Effective PPI is computed from actual output pixels, placed node dimensions,
  and the containing page's `production.units_per_inch` (default 72). Batch
  rendering now consumes the same dependency-first plan for selected v11
  mockups. Every projection in the frozen v11 schema is rendered and pixel-
  tested: UV, planar, box, cylindrical, spherical-axis, and normal-weighted
  world-space triplanar (which does not require source UVs). Per-graphic
  metallic, roughness, and opacity material response reaches the CPU overlay
  material. A stable `island` ID restricts a linked graphic to the UV-set-0
  triangles owned by that island; unknown IDs fail before render, and Studio
  exposes island and material-response controls. The v11 schema defines no
  additional projection modes; adding one later requires a new contract change.
  `scene3d MANIFEST batch-queue` runs bounded jobs across documents
  from a versioned JSON manifest, preflights aggregate work, supports a
  cancellation marker, isolates individual render failures in its result, and
  atomically publishes successful outputs together; manifest, revision,
  cancellation, aggregate-limit, and publication errors still abort the queue.
  Regressions exercise shared planning across validation, render, dry-run,
  transactional CLI, and both batch paths, plus missing/hidden/locked sources,
  inherited node context, cycles, deterministic order, and byte-for-byte
  rollback. The complete schema-defined v11 dependency/projection contract is
  implemented.

- [x] **3. Implement live material bindings.** Add, inspect, update, and remove
  bindings through one shared Rust implementation. Support page and node
  sources, explicit fit modes (`contain`, `cover`, `stretch`), crop bounds, and
  named material slots. Honor node/page transforms, clipping, visibility,
  color role, and output resolution. Undo/redo and save/reopen preserve the
  exact binding. Changing unrelated document content must not change the
  rendered mockup.

  Implemented in the shared v11 bindings planner and renderer: page/node sources,
  contain/cover/stretch, crop, named slots, inherited transforms and clipping,
  visibility/lock validation, and the existing page working-color pipeline are
  used by CPU preview/export. Source rasterization is bounded to a 4096px longest
  edge, then mapped at the mockup output resolution. Transactional CLI
  add/set/list/remove supports dry-run and revision guards; integration tests
  verify exact undo/redo/reopen data and that editing a sibling mockup leaves
  selected output pixels unchanged. Binding, fit/crop, projection, blend, and
  source-edit pixel behavior is covered in `suite_12` (115 tests pass locally).
  A separate per-binding color-role override is deliberately not part of v11;
  sources inherit the established document/page color behavior.

- [x] **4. Add bounded deterministic render caching.** Key cached surfaces by
  every appearance-affecting input in the data contract. A source edit
  invalidates only its transitive consumers; a camera, model, UV, material, or
  color edit invalidates only that mockup. Enforce byte/work limits before
  allocation, deterministic eviction, cache inspection/clear commands, and
  correct behavior after process restart or cache corruption. Cache is
  disposable and never changes document semantics.

  Initial implementation: scene3d PNG results use an in-process 64 MiB byte
  budget with FIFO eviction. The key includes node render settings and graphic
  records plus a digest of the resolved linked-art pixels, which prevents
  stale mockups when source artwork changes. The linked source is currently
  rasterized before lookup to compute that digest. Cache keys make unchanged
  mockups reusable across unrelated document edits; obsolete keyed entries are
  retained only until FIFO eviction. `/api/scene3d/cache` reports entry/byte
  bounds and hit, miss, and eviction counters and clears entries/counters with
  DELETE; `/api/scene3d/status` includes those counters. Individual render
  reports expose cache hits. The Rust cache records each render's consumer and
  direct page/node sources; `scene3d::node::evict_png_cache_source` provides
  targeted eviction, and the shared commit plus history restore paths now
  invalidate changed page/node consumers. Transaction invalidation walks
  linked page/node graphics and dieline panel sources transitively, including
  uncached intermediate mockups; edits to model, product-form, and dieline
  catalog records invalidate their scene3d owners and downstream consumers.
  Working-role changes invalidate linked page/node consumers but preserve
  unbound renders; unrelated proof-role changes keep their cache entries.
  A poisoned in-process cache lock is covered as a cache miss, leaving the
  regular renderer path available. An end-to-end server regression warms the
  cache through `POST /api/scene3d/render-report`, confirms a cache hit, restarts
  the process, confirms zero entries/bytes, then confirms the deterministic
  output is unchanged after the new-process miss. The cache is disposable and
  process-local; no persisted cache-corruption format exists. The CPU render
  path now preflights framebuffer plus resolved-image bytes (256 MiB maximum),
  including the depth-of-field accumulator before allocation. A regression now
  changes every scene/output setting individually and pins linked-art digest,
  renderer identity, and stable non-rendering page metadata behavior in the
  appearance key. `pentool scene3d-cache status|clear --server URL` inspects
  and clears a running loopback server's process-local cache without touching a
  document; HTTPS and non-loopback destinations are refused. Remaining evidence
  includes the release-build cache performance audit using
  `pentool benchmark --scene3d-cache`, recorded in `docs/performance.md`.
  Restart and poisoned-lock behavior are tested. Resolving linked-art pixels
  before lookup remains a deliberate correctness cost because it supplies the
  exact source-content key; the cache is process-local, so no persisted
  corruption format exists to recover.

- [x] **5. Define and implement mockup presets.** Ship licensed, documented
  presets for box, cylinder/tube, bottle, bag/pouch, card, and a simple device
  frame. Each preset must document model digest/license, named material slots,
  UV orientation/rectangles, allowed source binding, camera framing, default
  lights, ground/shadow behavior, and minimum useful render resolution. Include
  shadow catcher and ground plane as explicit optional scene objects/settings,
  not hidden renderer magic. Presets must work offline and be inspectable.

  Initial catalog progress: the offline `scene3d form presets` output now carries
  a stable digest of each procedural recipe, its project license/attribution,
  material slots and normalized UV rectangles, default camera framing, lighting,
  ground/shadow defaults, and minimum preview resolution. The editable product
  form picker adds a closed cylinder, thin product card, simple device-body
  frame, and gusseted stand-up pouch with separately printable front/back/gusset/seal
  regions. These are procedural recipes, not imported meshes; per-face preset
  UV rectangles are now checked against generated triangle/material assignments
  for every catalog starter, and regression coverage checks each preset's license,
  recipe digest, binding policy, UV orientation/regions, camera, lighting,
  ground/shadow defaults, and minimum resolution. Bottle and jar label-band faces are split from
  their lathed bodies at profile-ring boundaries and receive the cataloged UV
  sub-rectangle; bevelled polygon UVs are projected and normalized per face.
  v11 scene nodes can opt into an explicit
  ground plane, choosing opaque floor or transparent direct-shadow catcher, with
  bounded opacity, size, color, and below-object offset settings. It receives
  shadows, participates in cache identity, appears in render reports, and is
  editable in Studio. It is disabled by default and unavailable to v10 documents.

- [x] **5a. Add editable product forms.** Provide guided creation/editing for
  boxes and cartons, cylinders/tubes, and revolved bottle/jar profiles. A bottle
  profile exposes a bounded set of dimensions and curve/control points for
  body, shoulder, neck, and base; the generated surface and default UV layout
  update deterministically when the profile changes. Users can add a cap as a
  separate material region/object. Preserve imported arbitrary mesh support;
  do not imply free-form sculpting. Include starter perfume bottle, cosmetic
  jar, tube, and shipping/carton forms with editable parameters and clear
  dimensional units.

  Initial Rust geometry generators now produce deterministic CPU-renderable
  box/carton meshes (including bevel), cylinders, open tubes, and lathed bottle
  profiles from bounded millimeter parameters. `scene3d form add/set` creates
  and edits shared form records transactionally; document validation generates
  each catalog form under an aggregate vertex budget. Studio now creates forms
  on the active layer and edits dimensions/profile points through a server
  endpoint that validates the full candidate before returning it for one undo
  step. The profile base and final-height endpoints are anchored. An optional
  bottle `cap_mm` now creates stable skirt/crown meshes on the separate `cap`
  material slot and is covered by CPU rendering. Studio can add/remove interior
  profile points and toggle/edit cap radius, height, and vertical offset through
  the validated form API. Legacy `cap_mm` records default the offset to zero;
  validation bounds the cap above the bottle base and within the model range.
  Studio now includes a scaled radius/height profile graph: interior control
  points can be dragged or adjusted by arrow keys, while numeric inputs remain
  available for precise values; pointer gestures submit one validated update.
  The CPU generator interpolates the axial profile with four clamped Hermite
  samples per control interval, preserving strictly ordered heights and bounded
  radii; the graph displays the same sampled curve.
  The CLI now lists and instantiates eight bounded, editable procedural
  starters (perfume bottle, cosmetic jar, tube, closed cylinder, shipping
  carton, product card, simple device frame, gusseted pouch); tests generate
  each mesh and exercise transactional CLI and server creation. Studio presents the same choices and
  clarifies that the carton is a visual box mockup and pouch is conceptual.
  The cap is still a separately material-editable region within the form model,
  not an independently selectable scene object. The profile graph exposes a
  selected-point tangent handle backed by optional bounded `profile_tangents`;
  each pointer gesture and keyboard slope edit commits through validated form
  replacement. Rust regressions cover determinism, geometry changes, and bad
  tangent counts/ranges. Isolated Chromium browser QA on the v11 product fixture
  now verifies a focused profile point responds to an ArrowRight edit (radius
  26 → 26.5 mm) and the Studio Undo action restores 26 mm. The regenerated
  perfume-bottle preview stayed visible with no page errors. Isolated Chromium
  on the v11 fixture now verifies the guided perfume-bottle preset flow end to
  end: create on the active layer, load the generated 3D preview, change an
  interior profile radius (27 → 28 mm), remove and restore the cap material
  region and its controls, and drag a tangent handle to commit a new slope
  (0.075 → 0.430). The run completed without page errors or failed network
  requests. A second isolated browser pass created all seven catalog starters
  and each of the five custom form kinds (bottle/jar, box, pouch, cylinder, and
  tube), exposing the editable dimensions panel without alerts or page errors.
  Browser edits also commit box length (120 → 125 mm), cylinder diameter
  (70 → 72 mm), tube inner diameter (64 → 65 mm), and pouch gusset depth
  (65 → 67 mm), with no alerts or page errors. The focused CLI update regression
  verifies an invalid form edit leaves the file byte-identical, and the Studio
  bottle-profile regression verifies Undo restores the prior edit. CPU tests
  confirm deterministic generated UVs and that moving a profile control point
  regenerates those UVs deterministically. The separate `cap` material region
  is the supported cap-separation contract; independent scene-node selection
  is not required. Existing imported arbitrary-mesh support remains intact.

- [x] **5b. Implement editable UV layout and surface wrapping.** Provide a UV
  workspace with the model viewport and 2D UV canvas side by side. Selecting a
  face/material region highlights its UV island; selecting an island highlights
  its 3D surface. Support automatic box/cylinder/planar projections, seam and
  island inspection, move/rotate/scale/flip, island packing with padding,
  texture resolution/texel-density display, overlap warnings, and UV-layout
  SVG/PNG export. Users can apply linked source art as a movable/resizable
  surface decal or as a full-region fill with repeat/mirror and crop/fit
  controls. Edits in the 2D artwork, UV canvas, or 3D placement view remain
  synchronized and undoable. Seam discontinuities, mirrored/back-facing UVs,
  and low-resolution texture regions are visible before export.

  Partial implementation: the imported-model loader applies v11 island triangle
  ownership and corner-split scale/rotate/offset/flip transforms. Transactional
  `scene3d uv ... island add/set/list/remove` commands validate model references
  before commit. Studio now has a side-by-side page/model canvas and native UV
  layout with primitive projection, island creation from triangle indices,
  selection, transforms/flips, SVG export, and a Rust-rendered PNG download
  using the same effective UV coordinates and seam highlights. The transactional
  model viewport renders through the shared CPU renderer with a 512px edge cap;
  selecting an island highlights its exact face triangles in a transient,
  uncached 3D render, and a page-context toggle returns to the editable canvas.
  A server regression verifies the highlight changes pixels without mutating the
  document.
   `island pack` command now shelf-packs each UV set into the unit square with
  normalized padding, preserving island orientation and mirror state; tests
  cover dry-run safety, bounds, padding, and non-overlap. `uv ... show` now
  reports area-weighted texel density per UV set and editable island for a
  requested square texture resolution, using pixels per source model unit;
  island overlap diagnostics now use triangle-interior intersection tests rather
  than bounding boxes, with a one-million triangle-pair budget and an explicit
  truncation field. A regression verifies separated polygons with overlapping
  AABBs are not falsely reported. Studio distinguishes true overlaps from a
  bounded/incomplete analysis.
  optional `--minimum-density` marks low-density islands without assuming OBJ
  units are millimeters. Studio now passes the selected resolution/threshold
  through the UV preview endpoint and displays per-set and selected-island
  warnings. UV reports now detect shared geometric edges with mismatched UVs;
  SVG and PNG layouts highlight both sides in red, with explicit bounds on
  analysis and report size. Studio calls out possible discontinuities and
  whether analysis was truncated. The UV stage can select an existing linked
  UV decal, drag it on the layout, and commit its normalized scale/center/angle
  as one document history edit; number fields provide keyboard placement and
  the adjacent 3D preview rerenders from the saved document state. Clicking the
  3D viewport casts a camera-aligned ray against transformed model geometry,
  accounts for front-face culling and primitive occlusion, then selects the
  editable UV island owning the nearest hit triangle. Unassigned faces and
  misses are reported without mutating the document; a server regression covers
  hit-to-island mapping and input immutability. The face-pick endpoint also
  loads generated product forms and returns interpolated hit UVs with the actual
  material slot. Dieline-backed scene nodes are now ray-pickable through that
  same camera and nearest-hit path; the stable panel ID is returned and a
  folded-face click in Studio selects its matching flat panel. Conversely,
  selecting the flat panel requests a bounded, transient CPU render highlighting
  all of its folded primitives; a server regression verifies preview pixels
  change without document mutation. Studio's
  explicit “Place art on face” mode creates a linked
  decal centered at that hit coordinate and bound to that primitive's slot;
  imported models and generated forms are supported for this placement path.
  A selected decal can also be dragged across visible hits in its current
   material slot; start/end surface UVs become one undoable offset edit. The
   same UV workspace can edit overlays on parametric product forms as well as
   imported models; UV preview, face picking, and island packing resolve the
   generated form and persist UV data on its form record. A server regression
   covers generated-form UV overlay preview. Studio can now
  add a linked page or node source directly from the UV workspace as a UV decal
  or repeating fill, with source crop, contain/cover/stretch fit, clamp/repeat/
  mirror wrapping, editable tile counts/phase, and stable graphic IDs. The
   selected primitive's actual material slot is returned by the bounded UV
   preview endpoint, avoiding guessed slot names. Direct canvas drag/drop is
   implemented in Studio: the UV workspace retains its target model while page
   artwork is selected, exposes an accessible drag handle for the selected page
   object, and accepts it on the UV layout to create a linked decal at the drop
   point on the selected material slot. The existing form-based link controls
   remain available as a keyboard-friendly alternative. The Studio typecheck and
  production bundle pass. Isolated Chromium QA now verifies the procedural
  perfume form's 3D UV preview endpoint returns successfully and displays its
  bottle preview; dragging `label-name` from the artwork page onto an uncovered
  part of the UV canvas creates linked graphic `uv-label-name-art` on that
  target while retaining the source page context. Both UV preview endpoints
  returned HTTP 200 with no browser errors. Isolated Chromium at desktop
  viewport also verifies UV-decal pointer resize and rotation commit, 40×40 px
  transform targets, keyboard scale/rotation, and Escape cancellation without
  browser errors. Isolated Chromium also applies cylindrical projection,
  assigns a triangle island, confirms island selection highlights the matching
  3D triangles, and flips its U transform from +1 to -1 without alerts, failed
  requests, or page errors. An isolated two-island imported-model fixture with
  intersecting triangle interiors also renders the accessible warning naming
  `overlap-left ↔ overlap-inner` in Chromium, without page or network errors.
  All 13 Rust UV-core tests pass, covering projection, seam, density, overlap,
  layout rendering, island transforms, and analysis bounds. A CLI-level
  regression now verifies the overlap pair in structured `uv show` output and
  byte-identical document preservation. The desktop UV
  milestone is complete; the 375 px shell overflow remains tracked under wider
  Studio acceptance in task 13.
- [x] **6. Add product-design and mockup CLI with stable JSON reports.** Provide
  bounded form create/edit, UV inspect/project/transform/layout/export, graphic
  decal/fill placement, preset list/apply, bind, inspect, render, and remove
  operations. Dry-run and commit use
  the same planner; mutations support revision guards and transactional undo.
  Reports identify created/changed node IDs, bindings, source digests, slots,
  cache hits/misses, renderer, dimensions, effective ppi, warnings, and output
  paths without embedding pixels. Freeze exact spelling and argument grammar in
  this task before implementation. Candidate command shape:

  ```sh
  pentool mockup list --json
  pentool mockup apply DOC --preset perfume-bottle --id bottle-front --layer mockups --page products --x 120 --y 80 --width 640 --height 720 --dry-run --json
  pentool scene3d DOC texture bind bottle-front --slot label --source page:front
  pentool scene3d DOC texture bind bottle-front --slot cap --source node:cap-art
  pentool scene3d DOC form create perfume-bottle --id bottle --height-mm 180 --body-diameter-mm 52
  pentool scene3d DOC uv bottle project --kind cylinder --region label --set uv1
  pentool scene3d DOC graphic add bottle --source page:front --region label --mode decal
  pentool scene3d DOC uv bottle layout --region label --out label-uv.svg
  pentool scene3d DOC inspect bottle-front --json
  pentool render DOC --page front --node bottle-front --scale 2 --json
  ```

  The final CLI contract is JSON by default; `--json` remains an accepted global
  compatibility flag. `mockup list` needs no document and reports offline
  procedural recipes. `mockup apply DOC` requires a preset, stable scene node ID,
  and target layer; page and placement are optional, with a default 400×400
  placement at the origin. The generated form record ID is `<node-id>-form`.
  Apply uses the shared transactional form planner, so dry-run and commit have
  the same report and `--if-revision` protects the write. This entry point and
  grammar are implemented in CLI help and README; regressions also check the
  no-flag 400×400 origin defaults and dry-run/commit report equivalence. The
  form `set` regression now independently verifies dry-run byte preservation
  and equality of its geometry report with the committed edit. Its focused test
  passes. A panel-bind CLI regression now covers node-source linking, clear,
  dry-run/commit report equivalence, and stale revision rejection with
  byte-preservation assertions. The graphic CLI regression now also creates
  and removes a repeating fill with mirror wrap, cover fit, crop, tile scale,
  and phase, then renders the bound fill through `render-report`, in addition
  to its decal CRUD/undo/reload path. The
  lower-level form, UV,
  graphic, inspect, render, packaging, and bind operations retain their existing
  `scene3d DOC <area> <action>` spelling; the unified task remains incomplete
  until all operations, preset application checks, reports, and print workflow
  gates are audited. A CLI regression now applies every one of the eight listed
  starters through `mockup apply` and verifies that each reports stable node and
  form IDs plus nonempty generated geometry; render-report coverage separately
  pins renderer identity, output dimensions, cache state, and effective PPI.
  The printed Clap grammar was compared against the README and v0.16 command
  reference; the tested `graphic`, `form`, `uv`, `pack`, `mockup`, and
  `render-report` spellings match. The v0.16 CLI milestone is complete. Color
  semantics and print preflight policy remain in task 7, and manufacturing
  limits remain in the packaging tasks.

- [x] **7. Carry 3D through color and print.** Render bound textures through the
  document's managed color roles and intent; preserve deterministic reference
  output. Include 3D results in composite renders, spot/CMYK separations, print
  PDFs, and PDF/X draft output according to the existing print contract. Report
  effective ppi from rendered pixel dimensions and final placement; preflight
  warns/errors at the same configurable thresholds as other raster content.
  Test overprint, transparency, blend order, profiles, and fallback behavior.

  Partial implementation: the shared float compositor now accepts `scene3d` as
  a bounded raster surface, so managed-color PDF/X flattening includes the
  rendered mockup; the deterministic CPU renderer remains authoritative. CMYK
  separations rasterize scene3d as process color and report
  `flattened_scene3d_objects`; their JSON now warns that linked spot inks and
  vector overprint are not retained. Raster/3D nodes declaring overprint,
  traps, or style-level spot paints fail explicitly instead of silently
  ignoring vector-only plate declarations. Regression coverage checks process
  coverage, the warning, and overprint refusal; source-texture spot semantics
  themselves are still not retained. Print preflight now follows visible linked
  page/node sources and packaging-panel sources transitively, reports the
  consuming mockup when linked spot paint will flatten, and honors strict spot
  policy as an error; the traversal is capped at 100,000 nodes and reports an
  incomplete audit rather than claiming a clean result.
  Separation deliberately rejects partial opacity and non-normal blend modes
  until their plate interactions have explicit semantics, with a regression for
  the partial-alpha diagnostic and `multiply` rejection; PDF/X flattening does
  honor scene3d opacity, which is covered by a pixel-output regression. The render report already
  computes effective PPI from rendered dimensions and final placement. A focused
  integration test verifies visible CMYK plate coverage, the report count, and
  structurally valid opaque and translucent PDF/X output. Effective-PPI
  preflight now uses the renderer's exact raster-dimension calculation and the
  accumulated affine placement; the report lists per-axis PPI and warns below
  the existing `--min-ppi` threshold. Regression coverage checks threshold
  behavior, pixel ratio, rotation, and scale. A CPU pixel regression verifies
  that a 3D mockup and vector art honor document stack order, and a new pixel
  assertion pins `multiply` blending against a saturated underlay instead of
  silently reverting to normal. A linked-spot CPU-render regression confirms
  that changing a source paint's declared sRGB fallback changes mockup pixels;
  strict preflight separately reports that spot semantics flatten in 3D. An
  end-to-end PDF/X regression varies only that linked fallback and verifies
  the flattened output changes while both exports remain structurally valid;
  requesting perceptual rather than the document's relative-colorimetric intent
  also changes the managed 3D PDF/X output. That linked-fallback regression now
  exercises all four ICC intents (perceptual, relative colorimetric, saturation,
  absolute colorimetric), verifies the reported intent and PDF structure, and
  confirms saturation/absolute output differs from the relative baseline.
  A working-color regression now compares a rendered 3D pixel against the
  explicit sRGB-to-Display-P3 linear conversion under the declared
  relative-colorimetric intent. The linked-spot PDF/X regression additionally
  proves that equivalent sRGB and linear Display-P3 working roles produce the
  same deterministic CMYK output under one output profile and intent. It now
  also verifies that a linked spot paint's imported ICC-CMYK fallback affects
  flattened PDF/X output and that a missing fallback profile fails with its
  identity instead of silently using the ink alternate. The imported ICC-CMYK
  fallback is also exercised under saturation and absolute-colorimetric intents,
  with structurally valid output and reported intent assertions. Product policy
  is explicit in `docs/scene3d-v2.md`: linked spot paints use their portable
  fallback in the rasterized mockup, never propagate as 3D spot plates;
  packaging technical plates remain separate. This is covered by fallback-pixel
  and strict spot preflight regressions. The acceptance audit now covers all
  four ICC intents for sRGB spot fallback, relative/saturation/absolute intent
  handling for imported ICC-CMYK fallback, invalid ICC fallback refusal,
  style-level spot/overprint/trap refusal during separation, partial-alpha and
  non-normal blend refusal, PDF/X opacity retention, document z-order, profile-
  managed working-space pixels, process-plate contribution, per-axis effective
  PPI after transforms, and strict linked-spot policy. These behaviors and the
  deliberate process-flattening limits are documented and covered by the
  v0.16 packaging and binding integration tests.

- [ ] **8. Define parametric packaging geometry.** Implement a first-class
  dieline model and deterministic fold solver. Built-in templates: tuck-end
  cartons (straight-tuck and reverse-tuck), sleeve, mailer box, cylinder/tube,
  pouch, and bottle-label wrap. Inputs include length, width, height, board
  thickness, glue-flap width, bleed, and per-crease angle. Validate minimum
  dimensions, impossible panel combinations, self-intersections, invalid fold
  ranges, and unsupported non-developable shapes before mutation. SVG dieline
  import accepts only labeled supported paths/regions; ambiguous labels fail
  with actionable diagnostics. A focused import regression now verifies that
  two overlapping labeled panels receive a panel-specific diagnostic and that
  the failed candidate leaves the source document unchanged. Labeled SVG
  production paths now receive a bounded segment-intersection check before the
  candidate is committed; bow-tie contours and crossing subpaths are rejected
  atomically, while the generated-net SVG round-trip preserves shared corner
  endpoints.

  Partial foundation: v11 stores bounded dieline panels, UV rectangles, crease
  records, paths, and artwork source references. Rust rejects overlapping flat
  panels and unresolved sources. `scene3d::pack` currently folds only connected
  rectangular tree nets with matching shared-edge creases; it emits thin CPU
  mesh faces and applies the normalized `fold_state`. A separate cylindrical
  tube-label mapping subdivides its measured circumference into a deterministic
  64-segment surface with a stored seam angle/direction and shared flat UV map.
  Other non-rectangular templates, comprehensive physical thickness/gap checks,
  richer SVG import, and production geometry remain unimplemented. A seven-panel hinged-lid mailer net is now generated through
  this solver, but it does not model lock tabs. The folded solver offsets each
  hinge axis by half the stored board thickness so adjacent exterior panel
  surfaces remain connected. Positive thickness also generates inner panel
  faces and exposed cut-edge walls, with generated vertex, triangle, and
  primitive totals checked against shared renderer limits; linked art remains on
  exterior material slots. A simple untextured bridge closes each folded
  interior hinge wedge, but does not model score compression. Cut/fold
  allowances, closed structural validation, and production certification remain
  unimplemented.
  v11 bounds stored board thickness to 0..=100 mm, matching template creation
  and the post-creation settings API; invalid over-limit documents are covered
  by the normative invalid-case manifest.

  Current template implementation: `scene3d::pack::sleeve_dieline` generates a
  four-panel rectangular sleeve; `tuck_carton_dieline` generates straight- and
  reverse-tuck visual nets with four body panels, lids, tuck tongues, dust
  flaps, and glue-flap/cut/crease paths. The same carton helper generates a
  seven-panel mailer net with a base, four walls, hinged lid, and side glue
  joint. Carton and mailer cut paths now derive closed exposed outlines from
  their panel unions, omit shared edges identified as creases, and retain
  shared non-crease edges as open through-cut slits. Fold-sweep regressions
  verify generated cut paths contain closed contours, tuck slits remain, and
  no cut segment overlaps a stored crease. `pouch_dieline` now generates a five-panel simplified stand-up-pouch
  net (front/back, two side gussets, and bottom gusset) with cut/crease paths;
  it uses the shared rectangular tree-net fold solver and is available through
  `pack pouch` with dry-run, rendering, and template-list coverage. A shared
  regression also confirms every built-in template, including tube and
  bottle-label wraps, has a finite, parseable primary cut path with a closed
  trim contour; this checks drawing data, not cutter conformance. This is a
  visual box-bottom bag mockup only: zipper/seal geometry, film thickness, and
  forming strain are not modeled. Pouch now joins the supported built-in
  connected-net preflight set. Geometry regressions check all five panel
  dimensions over flat/intermediate/folded states and explicitly verify each of
  its four hinge seams stays joined at five fold amounts. A shared regression
  now discovers every shared edge in straight/reverse-tuck cartons, sleeve,
  mailer, and pouch nets and verifies both endpoints remain coincident at five
  fold amounts. These regressions prove hinge continuity. Bounded hinge
  resolution now rejects multiple crease records claiming the same panel edge
  instead of silently choosing the first angle/direction, and rejects crease
  marks that do not lie on any panel edge; focused tests preserve older marks
  on exposed panel boundaries. The solver now maps valley folds away from the
  root panel's printed side so folded carton walls have outward-facing normals;
  a CPU render regression verifies panel artwork remains visible on an
  assembled carton, and a geometry regression checks outward normals for all
  four body walls. Bounded
  zero-thickness triangle-interior detection and a board-thickness clearance
  screen are included in packaging preflight for the generated current-fold-
  state mesh. The thickness screen projects faces onto a common plane and
  reports overlapping interiors whose vertex plane distances stay within
  one stored board thickness; it skips edge-only contact and does not simulate
  angled clearance, crease compression, closed hinge geometry, or manufacturing
  behavior. `/api/scene3d/sleeve` and
  `/api/scene3d/carton` validate and add each dieline plus preview node in one
  candidate document. The shared fold solver checks the rectangular tree net.
  Bleed participates in the print layout; board thickness adds interior panel
  faces and exposed cut-edge walls and offsets folded exterior hinge axes, but
  does not alter flat cut geometry. Interior hinge bridges are visual rather
  than score-compression geometry; cutting/fold allowances and manufacturing
  certification are not provided. These remain
  visual mockups, not production dielines.

  CLI foundation: `pentool scene3d DOC pack sleeve` and `pack carton` create the
  same generated v11 mockups through shared pack helpers and transaction path.
  `--dry-run` is failure-atomic; `scene3d list` reports model, product-form, and
  dieline-backed nodes. `pack export DIELINE --out FILE.svg` writes a bounded,
  atomic SVG with panel guides, stored production-role paths, and linked page/node
  artwork. Vector-compatible sources remain vector; unsupported content uses a
  reported CPU-rendered PNG fallback with effective-ppi warnings. It does not
  synthesize bleed. Template listing and `pack inspect DIELINE` return
  structured JSON for sleeve, tuck-carton, mailer, and tube-label templates and stored
  panel/crease/path
  metadata. `pack preflight DIELINE` reports missing artwork, missing
  cut/crease roles, rasterized effective ppi, and non-production blockers; it
  is not a production pass. A constrained SVG import and flattened process-CMYK
  PDF/X draft and a deterministic PNG spot-plate export are implemented below;
  structural production validation remains unimplemented. SVG import accepts absolute millimeter
  dimensions/viewBox, named panel rectangles, labeled production roles, and
  straight crease M/L paths with explicit mountain/valley and angle metadata;
  it adds the dieline plus preview node in one guarded transaction and rejects
  unsupported or ambiguous geometry without mutating the source. A regression
  now pins rejection of transforms on child SVG panel elements, which otherwise
  could be silently interpreted at untransformed coordinates.

- [ ] **8a. Make dielines usable as design surfaces.** Provide a 2D dieline
  editing view where panels have stable IDs and can receive linked page/node
  artwork. Support ordinary vector/text editing plus panel assignment, panel
  transforms, bleed/safe-zone guides, cut/crease/perforation roles, and label
  wrap geometry. Keep this view a normal `.pen` page/scene, not a parallel
  proprietary document. Include package types for retail cartons (including
  apparel/T-shirt boxes), sleeves, mailers, tubes, pouches, and bottle labels.

  Partial foundation: the generated packages expose one artwork source selector
  per panel in the 3D inspector. `POST /api/scene3d/panel-art` and
  `scene3d DOC pack bind` share stable panel IDs and dependency validation;
  page/node art stays linked and flows to the folded material slot. Per-panel
  normalized scale, center, and rotation are now stored once and consumed by
  both flat export and folded UV graphics; Studio exposes these values for the
  selected panel and supports dragging linked art to reposition it in the flat
  preview, with keyboard-accessible scale/center/rotation controls. The preview
  renders per-panel safe-area insets, and preflight checks transformed artwork
  bounds against them conservatively. The paired flat view now also exposes
  panel-local scale-down/up and 15-degree rotation controls beside the artwork
  selector; each uses the same validated mutation and one undoable document edit.
  Flat panel selection can now link or clear
  a page/node source in place through the same validated server mutation used by
  the inspector. The flat panel also accepts the same typed page/node drag
  payload as the Layers outline and Design panel, with drop highlighting and
  inline success/stale-source feedback; the selector remains the keyboard path.
  Embedded-Chrome QA created a carton and dropped a real page source onto its
  front panel; the picker reflected the new link with no page errors. The
  selected panel also exposes the validated safe-area inset directly in the
  flat workspace, alongside its refreshed guide and preflight feedback.
  Chrome interaction verified the inset editor sends the shared safe-zone
  request (HTTP 200), updates the field to 4 mm, and reports completion inline.
  Exact X/Y scale, center, and rotation fields now sit beside those quick
  controls and commit through the shared panel-transform planner.
  Chrome QA changed front-panel horizontal scale from 1.0 to 0.9 (HTTP 200)
  and verified Ctrl+Z restored 1.0 in the flat control.
  Selected linked art now has 42 px resize and rotate handles on the flat panel;
  drag previews the transform and release commits one undoable change, while
  arrow keys provide keyboard adjustment. Chrome QA dragged the resize handle
  (HTTP 200, scale 1.0 → 0.83) and verified Ctrl+Z restored 1.0 without page
  errors. A follow-up confirmed both handles render at 42 × 42 CSS pixels and
  Shift+Right on the rotate handle commits a 15° edit that Ctrl+Z reverses.
  A linked panel can jump to its normal `.pen` source page/canvas,
  selecting a linked node for ordinary vector/text editing. The separate 3D
  package can be reopened by selecting its scene node. Generated packages now
  also have a linked `layout_page` built from ordinary editable `.pen` panel,
  safe-zone, cut, and crease nodes; edits resolve back through stable IDs for
  validation, flat export, preflight, and folded rendering. The generated
  canvas and trim now include production-path bounds and structural crease
  endpoints as well as panel bounds, so authored cut/fold geometry is not
  clipped outside the normal `.pen` editing surface. Legacy dielines without a
  layout page preserve their stored geometry. This completes the
  editable-page foundation, but direct artwork editing/placement remains in the
  paired preview/source page rather than on the dieline page. Selecting a panel
  on the normal editable layout page now lets a user promote a selected path to
  a stable cut, crease, perforation, bleed, safe-zone, or glue-flap production path via
  a validated Rust API. The same path enters flat SVG and vector spot plates.
  A selected untransformed two-point path can also become a structural crease
  when its endpoints exactly match a shared edge between two panels; the user
  supplies a stable ID, target angle, and mountain/valley direction. This
  updates the shared fold model and crease finishing output. Freeform, partial,
  or transformed paths are rejected as structural folds. Ordinary paths can
  independently take the production `crease` role for freeform scoring without
  changing the structural fold solver; flat SVG and vector Crease plates retain
  those paths while suppressing only paths composed entirely of structural
  hinge lines. A regression promotes a curved path and verifies both exports.
  A focused regression proves a new cut role reaches the exported CutContour
  plate and another proves
  a validated authored hinge reaches folded geometry. Structural crease
  records missing a separate production path are also included in vector and
  raster Crease plates; regression coverage removes crease paths from a valid
  fixture and verifies the generated raster Crease plate remains populated.
  General freeform
  manufacturing crease editing remains open. Selecting a panel
  on the layout page now updates the shared stable panel selection; a keyboard-
  accessible **View folded mockup** action returns to its scene node and keeps
  that selection for the folded-face highlight. Sleeve, tuck-carton, and mailer generation now use the
  linked panel/artwork model; the tube-label template supports one cylindrical
  wrap panel with measured circumference. A one-panel bottle-label wrap now
  uses an explicit measured cylindrical label-zone radius and the same seam/UV
  mapping. It does not conform to tapered bottle profiles. Full tube packaging,
  production-grade pouch construction (including seals/zipper/film behavior),
  and profile-conforming bottle-label wrapping remain open; both pouch and
  bottle-label outputs are visual mockups.

- [ ] **9. Keep flat dieline and folded view in sync.** Derive folded faces and
  UV rectangles from the single panel map in the data model, using the same
  `scene3d uv` overlay record as ordinary models. Implement a deterministic
  face-ID/picking buffer: selecting a 3D face selects its panel; selecting a
  2D panel highlights its folded face. Artwork binds to panels as live page/node
  sources. Changing a panel, fold angle, dimensions, or board thickness updates
  only affected geometry/texture results. Verify no face gaps/overlaps across
  all built-in templates at flat, intermediate, and folded angles.

  Partial foundation: a `scene3d` node can reference a dieline; the Rust render
  path derives a rectangular panel net with material slots and preserved UVs.
  The Studio fold slider previews in the render pipeline and commits a
  document-level fold value as one undo step. The inspector exposes
  transactional target-angle and mountain/valley direction editors per crease
  through shared Rust planners. `pack crease` accepts `--direction
  mountain|valley` independently of `--angle-deg`. Studio now requests a bounded flat
  SVG with linked artwork from the shared Rust exporter and lets the user select
   a stable panel ID to highlight its flat region. Folded-view face picking now
  selects the matching flat panel, and selecting a flat panel requests a
  bounded render that visibly highlights its folded face; a pixel regression
  verifies the highlighted output differs from baseline without mutating the
  document. Studio places that local render inside the selected node's
  transformed coordinate space without applying the node position twice; a
  browser regression pass confirmed the selected folded face is visible in the
  paired workspace at desktop and narrow viewport sizes. Editable panel nodes
  now live on a linked ordinary `.pen` page and
  selection carries to the folded mockup. A new geometry-parity regression
  edits a shared hinge and both adjoining panel bounds on that editable page,
  then verifies the resolved map changes folded mesh vertices and flat print
  panel dimensions together. Complete no-gap/no-overlap geometric
  conformance remains open; collision checking is bounded, with a limited
  thickness-clearance screen that is not manufacturing validation. The screen
  now reports a board-thickness conflict when folded panel faces overlap after
  projection to a common plane and the full face's plane-distance range stays
  within the stored board thickness. Synthetic tests cover conflict,
  clearance, edge-only contact, zero-thickness mode, and structured report
  output; built-in zero-thickness fold sweeps remain deterministic. A local invariant sweep verifies
  finite coordinates and unchanged panel edge lengths at flat, quarter,
  midpoint, three-quarter, and full fold for all four supported package nets.
  The bounded zero-thickness collision scan now checks hinged panel pairs too;
  the triangle-interior predicate still ignores edge-only hinge contact. A
  synthetic folded-hinge crossing regression and a flat-net false-positive
  sweep cover that distinction for the built-in cartons, sleeve, mailer, and
  pouch. Collision-report structure, pair bounds, and deterministic results
  are also exercised at five preview fold states for every rectangular built-in
  net. A packaging preflight regression found that generated tuck tongues used
  the same fold direction on both scores and landed on the carton exterior;
  their score direction now folds them toward the interior for both carton
  variants. The collision scan now treats overlapping coplanar faces with
  opposite outward normals as contact (including during the thickness screen),
  and treats an exact one-board-thickness separation as contact rather than
  positive-volume overlap, while retaining same-facing overlap and non-coplanar
  crossing detection.
  The straight-tuck dust-flap/lid overlaps are resolved by thickness-aware
  flap layering. Dust flaps are inset by 1.5 board thicknesses per side (so a
  tilted flap stays inside the inner wall faces), hinge along a partial wall
  edge that needs a matching crease (partial contact without one is still not a
  hinge), fold slightly past square so the free edge sits below the lid, and
  both left and right flaps now fold toward the interior (the left flaps
  previously pointed outward). The lid fold and board-compensated opening are
  unchanged, and shared-surface hinge continuity regressions pass; the hinge
  check interpolates partial-edge endpoints. A regression preflights
  straight-tuck, reverse-tuck, sleeve, mailer, and pouch at fold 0, 0.5, and 1:
  all are clear except two pinned, documented cases. Straight-tuck at fold 0.5
  shows only dust-flap/lid pairs because one shared fold amount drives every
  crease (flaps do not fold before lids in transit). The mailer lid overlaps its
  side glue joint once folded; the mailer net is outside this fix. This is not
  a physical-board simulation or manufacturing conformance result.
  Folded-state collision accuracy and geometric conformance remain open.

- [ ] **9a. Unify packaging UVs and print mapping.** Every flat panel's artwork
  has one canonical mapping to its 3D face and UV rectangle. A cylindrical
  bottle/tube label uses a documented seam and wrap direction; the flat label
  width corresponds to the measured surface circumference at the specified
  wrap position, with overlap/gap allowance explicit. Include a seam preview,
  trim/bleed, safe area, and a warning when artwork crosses a seam or falls
  below required print resolution. UV-layout export and flat dieline export
  must come from the same stored mapping.

  Partial foundation: folded mockup binding and flat SVG export now read the
  panel's scale, normalized center, and rotation through the same transform
  decoder; a focused regression asserts the decoded values reach the folded
  surface graphic while the generated-carton print test pins the corresponding
  flat placement. Folded binding now composes that panel-local transform into
  the stored `uv_rect` (the flat SVG remains panel-local); a rendered comparison
  verifies that remapping the UV atlas rectangle preserves the artwork's
  position on the corresponding folded face within raster sampling tolerance.
  Flat SVG export now places linked page/node artwork inside
  each panel from the same `rect_mm` map used by the folded mesh. Vector-
  representable sources stay vector; unsupported content uses bounded CPU PNG
  fallback, with rasterized-panel IDs, effective ppi, and configurable
  `--min-ppi` warnings. Tube labels now store radius, seam angle, and wrap
  direction; their flat width is validated against `2π × radius` and subdivided
  into 64 UV-mapped cylinder segments from that same stored record. Per-panel
  safe-area guides and conservative transformed-artwork bounds checks now
  exist. Tube flat SVG/preview now marks both sides of the seam with its stored
  angle and direction; preflight reports a conservative bounds warning when
  transformed linked artwork touches either seam edge. An explicit overlap
  allowance is stored on the tube map and adds a marked flat tail mapped onto a
  raised overlap strip on the folded cylinder. Studio and `pack settings` now
  update seam angle (-360..=360°) and wrap direction transactionally for
  cylindrical labels; the shared flat seam marker and generated folded mesh
  consume the same stored values. Non-wrap dielines reject these options.
  Regression coverage verifies geometry and flat-marker changes plus
  byte-preserving refusal of invalid range/template edits. Browser QA selected
  a generated tube label, changed seam angle and direction through the inspector,
  and observed two successful settings responses with no page errors. At 390×844
  the controls stayed within the viewport with no horizontal overflow; the
  preview completed after its bounded render request. Flat SVG/PDF export now
  expands
  its page by the stored bleed amount and synthesizes linked-art bleed by
  nearest-edge pixel extension at `max(300, --min-ppi)` on non-crease cut
  edges; fold edges and the tube seam are excluded. The edge-extended raster
  is bounded to 16 megapixels aggregate, reported per panel, and shared by the
  flat SVG and flattened PDF/X draft. Raster and vector technical spot plates
  use the same expanded page origin and dimensions. Cut-edge masks, fold-edge
  exclusions, and edge pixels have deterministic regression coverage. This
  is a design aid, not certified trapping or manufacturing output. Bleed on
  An optional `panel_id` on a cut path now explicitly associates one closed
  contour with an existing panel; validation rejects non-cut associations,
  duplicate per-panel associations, open paths, and contour bounds outside the
  panel. Flat artwork is clipped to the associated contour and bleed is
  synthesized from the nearest contour pixels under the existing aggregate
  raster bound; the vector cut plate retains the same stable path ID. The same
  simple contour is ear-clipped into the folded panel mesh with UV coordinates
  mapped through the canonical rectangle. Curved contours flatten at 0.1 mm
  tolerance and are capped at 2048 vertices; multiple subpaths/holes and
  self-intersections fail clearly. A CPU render regression confirms the folded
  silhouette changes with the cut path. Hinge checks resolve endpoints on the
  triangulated contour, and folded collision/board-clearance preflight now scans
  bounded triangle pairs rather than assuming four-corner faces. Complete
  dieline/UV export parity and structural manufacturing validation remain open.

- [ ] **10. Add packaging CLI and print export.** Provide template listing,
  dieline create/import, panel binding, fold preview, inspect/preflight, and
  export. Flat exports preserve full bleed and place cut, crease, perforation,
  and safe-zone graphics on documented spot-color/separation roles. Export a
  PDF/X draft and a preview image from the same planned document state. Add
  `pentool pack` with stable JSON, dry-run, revision guards, deterministic
  output names, containment checks, and failure-atomic behavior. Candidate
  command shape:

  User outcome/acceptance: a designer must be able to review the assembled
  package in the folded preview, then hand a printer a full-scale flat layout
  whose through-cut, crease/fold, and perforation operations are distinguishable
  as separate vector finishing plates. Include trim/bleed and safe-area guidance
  without presenting the reference-only visual cutting map as machine cutter
  data. The flat artwork and folded appearance must derive from the same panel,
  UV, and dieline state so the designer can catch placement mistakes before
  printing. Clearly label draft/conceptual templates and unsupported material,
  thickness, or assembly behavior; never imply cutter or manufacturing
  certification without validated geometry and an explicit conformance test.

  ```sh
  pentool pack templates --json
  pentool pack create DOC --template straight-tuck --id carton --layer packaging --length-mm 120 --width-mm 80 --height-mm 220 --board-mm 0.5 --dry-run --json
  pentool pack bind DOC carton --panel front --source page:front
  pentool scene3d DOC pack crease carton front-body-fold --angle-deg 90 --dry-run --json
  pentool pack preview DOC carton --fold 0.75 --out preview.png
  pentool pack preflight DOC carton --min-ppi 300 --json
  pentool pack export DOC carton --out dist/carton --pdfx --json
  ```

  Final flags and command nesting are frozen before implementation; examples
  above are a concrete direction for review, not an existing CLI promise.

  Implemented command subset: `scene3d DOC pack templates`, `inspect`,
  `preflight`, flat SVG `export`, and static PNG `preview --fold 0..1`.
  Preview renders the linked node through the CPU reference path, is failure-
  atomic, leaves the source document unchanged, and refuses to target the input
  document path. Transactional sleeve/carton creation, panel binding, and
  per-crease angle edits, and `pack tube` label creation are also available.
  Tube labels map a single flat panel whose width is `2π × radius + overlap`
  onto a deterministic segmented cylindrical surface; the optional overlap is
  rendered as a raised tail on the wrap start, and no tube ends are modeled.
  `pentool pack` now aliases the implemented templates, inspect, preflight,
  preview, export, binding, creation, and crease actions through the shared
  scene3d planner. Preview and SVG export now accept `--if-revision`, reject
  stale snapshots before writing, and report the source revision in JSON.
  Folded PNG preview, flat SVG export, and flattened PDF/X export now honor
  `--dry-run`: they execute the same render/export plan and report byte size,
  revision, `dry_run: true`, and `complete: false` without publishing output.
  CLI regression coverage verifies preview and SVG dry-runs preserve both the
  destination and source document, then verifies committed output byte counts.
  `pack export --pdfx --out FILE.pdf` now rasterizes the exact flat SVG plan,
  places it in a temporary print page, and calls the shared PDF/X-4 exporter
  with the document's CMYK output role/intent. This is explicitly a flattened
  process-color draft: technical paths are rasterized, no spot separations are
  emitted, and independent conformance validation remains `not-run`. Stored
  bleed expands the flat SVG/PDF media area, contributes edge-extended linked-
  art pixels at cut edges, and is represented as the PDF trim inset/bleed
  value; crease edges and tube seams are not extended. Technical spot plates
  use the same expanded origin and dimensions.
  `pack export-bundle` now atomically delivers the flat SVG, folded CPU PNG
  preview, flattened PDF/X draft, separate grayscale PNG technical-role plates,
  editable vector SVG spot plates grouped by canonical ink, and a relocatable
  manifest keyed to the source revision. The vector plates retain millimeter
  dimensions and stable source path/role IDs; they and the raster plates are
  derived from the same stored dieline paths. `pack vector-separate` also
  exports these SVG plates independently and supports a no-publication
  `--dry-run`. Bundle export refuses existing destinations and leaves the
  source document unchanged. A regression now proves the full bundle dry-run
  reports planned artifacts without publishing them, honors a matching
  revision, and rejects a stale revision before export. Its flattened PDF/X
  and raster plate staging now
  discard live dieline ownership before replacing preview nodes, so a generated
  carton whose owner is on the proof page still produces a valid bundle. An
  end-to-end regression exports and verifies that case. When the general
  process-separation surface would exceed its all-plates-in-memory budget,
  packaging raster plates now rasterize one vector spot ink at a time and stage
  it before continuing. A fixed aggregate-pixel work bound still applies;
  requested raster PPI is not silently reduced. The 300-PPI `pack separate`
  regression checks the output dimensions and grayscale pixels. `pack verify-bundle DIR` validates all hash-listed
  files against the manifest, bounds file reads, and rejects traversal and
  symlink escapes. It also requires the cutting map's reference-only legend
  and validates each vector plate's declared ink and single finishing role;
  this verifies handoff integrity, not structural or manufacturing conformance. The packaging
  integration suite exports and verifies a real bundle, asserting separate
  role-labeled `CutContour`, `Crease`, and `Perforation` vector plates beside
  the flat dieline, folded preview, and PDF/X draft, while confirming the source
  document is unchanged. An authored tear line is included in the same test to
  prove perforation paths survive as their own operation plate. Both vector and raster plate exporters now refuse one spot-ink name
  reused across distinct packaging roles, preventing cut and crease paths from
  being merged into an ambiguous finishing plate; a regression covers both
  paths and confirms vector export publishes nothing on rejection. The bundle
  also includes a checksum-listed `CUTTING-MAP.svg` visual review sheet with a
  color legend for cut, crease, perforation, bleed, safe-zone, and glue-flap
  guides, explicitly marked as reference-only and separate from the
  machine-usable spot plates. Bundle verification checks that this guidance is
  intact even if its hash is recomputed, and confirms the map's production
  path IDs/roles/geometry match `flat.svg`. It also compares normalized vector-
  plate geometry to the matching flat production role after bleed translation,
  splitting combined SVG subpaths so deduplicated structural creases compare
  correctly, and rejects a plate whose physical size or viewBox differs from
  the flat artwork. Flat page bounds now include production paths and structural
  crease endpoints, preventing cut geometry from falling outside the declared
  print page. Regression coverage recomputes hashes after altering cut guidance,
  moving a cut path on the review map or machine plate, and changing a
  `CutContour` path to a crease; all are rejected. It includes
  a checksum-listed `PRINT-GUIDE.txt` that
  identifies the artwork and spot-plate artifacts, distinguishes through-cut,
  fold, and perforation guides from the non-cutting safe zone, and explains the
  PDF's spot-plate and manufacturing-certification limitations. The guide now
  leads with the folded preview as the assembled-appearance check, then
  separates the flat-layout/proof review from vector finishing operations and
  explicitly says the PDF alone is not a cutting file; the handoff asks the
  provider to verify scale, material, bleed, and cutter settings. The guide
  now records the exact millimeter media dimensions from the exported flat SVG,
  the requested finishing-plate PPI and linked-art warning target, and tells
  the printer to use actual size rather than fit-to-page scaling. A regression
  compares its reported dimensions to the bundle's SVG. Studio presents the
  same workflow as three explicit steps: inspect the folded mockup, print the
  artwork PDF at 100%, then send the separate vector operation plates to
  finishing; it states that the PDF is not a cutter file and that conceptual
  templates are not manufacturing-certified. This is a design handoff, not an
  invented cut/assembly recipe: structural and
  manufacturing certification remain out of scope until validated. Editable
  dieline import, production validation, and certified manufacturing output
  remain open. `pack preflight` parses stored production paths, reports bounds
  and element diagnostics against the panel envelope plus bleed, and checks
  edge-connected panel components for built-in rectangular nets. It also counts
  closed subpaths per production path and warns when a cut-role path has no
  closed contour, while allowing intentional open slit paths; a regression
  verifies the warning and structured per-path count. Preflight also
  applies the same bounded self-intersection check used by SVG import to stored
  production paths, catching malformed edits made later on the ordinary layout
  page; per-path reports and geometry status expose crossings, and overly
  complex paths fail at the configured work bound. A bow-tie cut regression
  verifies the diagnostic. It then
  measures every generated hinge's endpoint gap at the current fold amount, the
  wrap seam for tube/bottle-label templates, and bounded zero-thickness
  intersections between folded panel faces, including hinge-adjacent pairs;
  Studio surfaces failed gaps
  beside each affected panel. Collision findings are potential overlaps in the
  selected fold state, not a physical-board collision simulation. Thickness-
  related gaps and engineering behavior remain unvalidated.
  Generated tuck cartons keep structural crease records canonical on their
  editable flat page; the redundant aggregate crease path is intentionally not
  mirrored as an editable production path. Preflight now recognizes those
  structural records as the fold operation source, matching the flat SVG and
  vector-plate exporters instead of reporting a false missing-crease warning.
  A regression verifies all 12 folds survive layout resolution and appear on
  the separate Crease vector plate. This resolves only the preflight mismatch;
  broader geometry acceptance remains open.

- [ ] **11. Build the Studio mockup and packaging workflow.** Add a Mockups
  workspace that:
  - lists presets with small previews, required material slots, and license;
  - creates a mockup without replacing the active page or existing artwork;
  - lets users drag a page/node from the document tree onto a named slot and
    shows clear bound/unbound/error states;
  - exposes fit/crop, camera presets, exposure, lights, ground/shadow, and
    preview quality without hiding advanced values;
  - shows effective ppi and whether the preview is reference or accelerated;
  - supports deterministic refresh, inspectable source selection, and undo.

  Add direct manipulation in the 3D viewport: drag artwork onto a target
  surface, then use visible handles to translate, rotate, and scale it. Clearly
  distinguish decal from fill/repeat mode. Add editable material controls for
  supported base color, roughness, metallic, opacity/translucency, and surface
  response without promising full arbitrary shader authoring. Provide a live
  UV editor panel with island selection, transform handles, seam/overlap
  diagnostics, and resolution/texel-density feedback.

  Add a Packaging workspace with linked Flat and Folded views, panel/face
  selection in either view, editable dimensions/thickness/bleed/safe zone,
  per-crease fold controls, panel-to-art assignment, and visible cut/crease
  legend. The fold slider animates only the static geometry between flat and
  target fold states; it is not a document timeline or exported animation.
  Provide preflight issues beside the relevant panel, not only in a global
  status string. UI changes must use server APIs/shared planners, not a
  second browser-side geometry or renderer implementation.

  Initial packaging UI: the 3D panel creates a four-panel sleeve, simplified
  straight-/reverse-tuck carton, or hinged-lid mailer from editable
  length/width/height/board/bleed inputs (plus carton/mailer glue-flap width).
  The server validates the whole
  candidate, Studio selects the resulting preview node, and its linked fold
  slider provides a visual closed/open state. The creator also offers the
  five-panel pouch and measured-radius bottle-label wrap through dedicated Rust
  planners, with front-width/gusset and radius/overlap inputs respectively.
  These are explicitly simplified visual mockups. Full carton variants, flat
  editing, and production checks remain open. A panel-art
  selector can link one page or node to each generated package face; the artwork
  can be changed in the source document and the folded render updates through
  the existing Rust binding path. The selected-package inspector also renders
  the flat dieline through a bounded server endpoint and highlights the
  selected panel ID. In the 3D workspace, selecting a dieline-backed object now
  opens a paired Flat/Folded stage: the active page canvas shows the folded
  package, while the flat net comes from the Rust preview endpoint. Selecting a
  flat panel or its stable-ID folded-face control synchronizes the inspector.
  Dragging artwork within a linked flat panel now updates the shared Rust
  panel-art transform and therefore the folded face too; a dashed transform
  box previews the placement during the gesture. The flat panel controls also
  nudge linked-art scale and rotation through that shared transform route;
  keyboard-accessible placement fields remain in the inspector. General 2D
  canvas editing remains open.

  Current Studio foundation: the 3D inspector exposes a global fold-state
  preview for selected dieline-backed objects, with live Rust-rendered preview
  during drag and one undoable value on release. It also exposes a selected
  crease's target-angle and mountain/valley direction editors through shared
  transactional Rust planners. Stored board thickness and print bleed can now
  also be edited after creation in the inspector or `pack settings`; the
  validated update preserves panel artwork and production paths, and bleed
  changes the flat export bounds immediately.
  A stable-ID folded-face selector now drives the same panel selection and flat
  dieline highlight as the flat-panel controls; this is a navigation list, not
  3D surface hit-testing. The dedicated linked Flat / Folded workspace now
  supports pointer placement for linked artwork. In the UV/model viewport,
  “Place art on face” maps a pointer hit to the nearest visible triangle's
  interpolated UV and material slot, then creates an editable linked decal on
  that surface; the API supports imported models and generated product forms.
  A selected decal can also be dragged across visible hits in its current
  material slot; start/end surface UVs become one undoable offset edit. This
  still lacks live transform handles. The Layers outline now exposes draggable
  page and drawable-node sources, and Linked artwork repeats those sources beside
  the destination cards; dropping either onto a graphic slot relinks it through
  the normal document edit path. Activating a source chip links it to the first
  slot for keyboard users; each slot's source picker remains available for
  precise selection. The source shelf filters and renders at most 80 drag chips,
  with an explicit no-match state for large or narrowed source lists. Embedded-
  Chrome interaction verified keyboard page activation, page and node drops onto
  a slot, filtered lookup, the selected graphic source, 42 px drag-source hit
  targets, and success/empty feedback without browser or failed-request errors.
  Camera review now also has front, back, left, right, top, and three-quarter
  presets. Each writes azimuth/elevation in one undoable node edit, preserves
  the existing FOV and target, and removes stale explicit eye/clip values that
  would override the orbit. Embedded-Chrome checks verified Back values,
  unchanged FOV, and Undo restoring the prior camera with no browser errors.
  The model viewport now also offers direct scale-up/down and 15-degree
  rotation controls for its selected UV decal. Scale changes keep the decal
  within UV bounds; every control uses the shared linked-art edit path and is
  undoable. Studio typecheck and production embed build pass. Projected
  on-surface drag handles and arbitrary 2D canvas editing remain outstanding.
  The UV map now adds direct selected-island move, scale, and rotate handles;
  pointer gestures preview the bounds and commit through the same undoable island
  transform fields used by the Rust renderer. Keyboard arrows offer equivalent
  movement, scale, and rotation adjustments. Typecheck and embedded production
  build pass. Browser interaction against the perfume-form fixture verified
  pointer move/scale/rotate on a one-triangle island, Undo restoring the prior
  rotation, Escape cancelling an in-flight move, and arrow-key movement. At
  390×844 all three handles remain visible at 40×40 CSS pixels, the document
  fits the viewport, and no browser errors were reported. This signs off those
  UV-island gestures only; projected on-surface decal handles and broader Studio
  acceptance remain open.
  The UV workspace can drag selected page-canvas
   artwork onto its layout as a linked decal while retaining the target model;
   browser-level interaction evidence is tracked under task 5b. It also previews
   parametric product forms and stores editable island transforms on the form
   record's optional `uv` map; missing maps continue to use generated UVs.
  The paired packaging view also marks panels with linked-art, safe-zone,
  cylindrical seam, or below-minimum raster-resolution preflight findings;
  activating a marked panel exposes the full issue text to assistive technology.
  The flat dieline workspace now offers an in-app `Export print bundle` action
  for the current document snapshot. The server reuses the CLI bundle planner,
  packages its integrity-checked artifacts into a bounded ZIP download, and the
  workspace identifies the PDF as artwork, vector spot plates as cut/fold/
  perforation guides, and the PNG as folded review. The bundle includes a
  checksum-listed print guide; it remains an uncertified draft.
  Follow-up browser QA linked the `Label artwork` page to the carton front,
  verified it appeared in the flat panel, downloaded the UI bundle at 300 PPI,
  confirmed the SVG retained the linked-panel artwork, and passed all eight
  artifact checks with `pack verify-bundle`. Raster finishing plates now
  stream per spot ink at the requested resolution under an aggregate pixel-work
  bound rather than failing on the multi-plate working-set estimate. The
  paired Studio PPI control is bounded at 1200 to match the bundle rasterizer's
  export limit, while applying the same value to preview and preflight so a
  UI-selected resolution cannot fail only at export.
  `pack preflight` now reports the PDF/X output-role configuration from the
  shared Rust planner. Studio uses that report to warn before export when the
  document lacks an ICC CMYK output role and points to profile import/role
  assignment; the setup is documented in `docs/studio.md`. A fresh fixture reproduced the
  prerequisite: flat preview and preflight worked, bundle export refused the
  unconfigured output role, then succeeded after importing the licensed
  fixture profile and assigning it. The local HTTP API returned a 15-file ZIP;
  its extracted bundle passed `pack verify-bundle`, reported
  `manufacturing_certified: false`, and left the source document unchanged.
  The embedded page returned HTTP 200 and contained the warning and CLI hints.
  This covers the API and embedded asset, not browser interaction or visual QA.
  The flat view now also shows a color-coded production legend for cut, crease,
  perforation, bleed, safe-zone, and glue-flap marks, using the same role colors
  as the Rust SVG exporter; safe-zone is explicitly labeled as non-cutting.
  The shared flat SVG now encodes stored fold direction in the paired design
  preview and exported handoff: valley creases are solid blue and mountain
  creases are dashed violet. The reference-only cutting map, its legend, and
  bundled print guide explain those styles and direct users to check the folded
  preview. Machine-facing spot plates are unchanged. Bundle
  verification requires both direction labels, and the print-bundle integration
  test verifies the rendered mountain path style while preserving the separate
  cut and crease operations. This improves handoff readability; it does not
  certify fold order, material behavior, or cutter compatibility.
  Browser QA against an isolated v11 fixture verified the selected package opens
  the paired views, the folded canvas fills its allocated pane (the wrapper now
  uses a flex column so its child canvas receives the pane height), panel arrow
  navigation moves focus and folded-face selection without nudging or dirtying
  the document, “Edit source artwork” opens the linked source page, and a
  failed flat-preview request exposes a working retry. At 1440×900 and
  1024×768, both views remained visible without page-level overflow. Remaining
  Studio acceptance—including the full mockup control matrix, UV gesture
  browser coverage, and general 2D dieline editing—has not been signed off.
  Follow-up Chrome QA on the rebuilt embedded Studio at 1440×900 verified the
  carton opens both the folded render and flat SVG, linked panel source changes
  and scale/center edits reach the server planners, and preview APIs return
  without browser errors. The module switcher now gives icon-only buttons
  accessible names and 40 px mobile hit areas; a 390 px pass can reach 3D by
  its accessible name without document-level horizontal overflow. A responsive
  layout now moves the dock below the workspace on narrow screens and keeps it
  collapsed until the user opens Panels; the packaging pair stacks and scrolls
  vertically instead of sharing space with the dock. Chrome checks at 390×844
  and 768×1024 verified both full-width views, opening/closing the mobile dock,
  and no page-level horizontal overflow. At 1280×800, the original side-dock
  layout remains and the two package views fit side by side. Broader mobile
  editor acceptance and the full Studio control matrix remain open. A subsequent focused desktop
  pass exercised the stable `front`/`back` panel controls: arrow-key navigation
  selected the matching face, and assigning a page to the selected panel returned
  the expected v11 document from `/api/scene3d/panel-art` (HTTP 200). In that
  same run the background canvas PNG request timed out once while flat preview
  and packaging preflight returned successfully; this is an unresolved runtime
  reliability observation, not a signed-off failure or acceptance pass. On the
  rebuilt embedded app, an unloaded 1280×800 repeat returned HTTP 200 for every
  observed PNG, face-preview, flat-preview, and preflight request, with no failed
  requests or browser errors. The earlier timeout was not reproduced; keep the
  complete render matrix in acceptance. The
  packaging integration suite passed 136/137 tests on its first run, with the
  unrelated scene3d render-cache restart test timing out; that test passed when
  rerun alone. The full Studio workflow and reliable-render acceptance remain
  open. A follow-up Playwright pass against the Vite Studio and release backend
  at 375×844 and 768×844 found that the flat-view controls below the artwork
  picker were clipped when the panel dock was open. The stacked package views
  now size to their content on narrow viewports while retaining the flexible
  side-by-side desktop layout. After the fix, scale, center, rotation, and
  source-edit controls scroll fully into the workspace above the dock at both
  widths, with no horizontal overflow, failed requests, or browser errors; the
  1280×844 side-by-side view remains intact. This closes that specific mobile
  clipping defect only; broader mobile editor and full Studio acceptance remain
  open.

- [x] **12. Add bounded batch rendering.** Render many source pages/documents
  through one mockup and many mockups from one source with stable, collision-
  checked names. Bound queue length, total pixels, document count, and work;
  support cancellation without partial output being reported as complete.
  Return compact per-item results and a deterministic summary. Failed items
  must not corrupt successful outputs or the source documents.

  Initial `scene3d DOC batch --node ID ... --out-dir DIR` support renders up
  to 32 selected v11 mockups in shared planner order, bounds total output to 64
  million pixels and 256 MiB of PNGs, reports deterministic file hashes and
  per-item render metadata, rejects ambiguous IDs and existing output
  directories, and publishes the staged directory with one same-parent rename.
  Global `--dry-run` performs the same planning/render checks without
  publishing. A failed render leaves no output directory and never modifies
  the document.
  Batch selectors now accept `PAGE::ID` so two page-local nodes with the same ID
  can be queued together; an unqualified duplicate ID fails with a corrective
  selector hint. `scene3d MANIFEST batch-queue --out-dir DIR` supports up to
  eight document jobs and 32 mockups per queue, with 64 million aggregate
  output pixels, 256 million supersampled samples, and a conservative
  five-byte-per-pixel PNG estimate before rendering. A `--cancel-file PATH`
  marker is checked between renders; cancellation and queue-level errors remove
  staged outputs and leave the final directory unpublished. Individual render
  failures are returned per item while successful PNGs are published together.
  The report distinguishes complete and partial results; regression coverage
  exercises a missing external model alongside a successful item. A subprocess
  regression now creates the cancellation marker only after the first PNG has
  been staged, then verifies that no final output is published and the staging
  tree is removed.

- [ ] **13. Prove compatibility, safety, and performance.** Add migration and
  round-trip tests; golden/pixel tests on the CPU reference renderer; semantic
  tests for binding graph and exact invalidation; hostile SVG/model/package
  tests; malformed/cyclic/missing-reference tests; limit and rollback tests;
  and cross-platform CLI/Studio coverage. Test all templates at minimum,
  midpoint, and folded states. Record release binary sizes with default and
  `gpu` features, cache/work bounds, representative render timings, and the
  reason for every golden pixel change. Keep old 3D fixtures byte/visual stable
  where the contract requires it.

  Geometry evidence now includes the five-fold-state hinge/dimension matrix for
  rectangular nets and a lower-bound dimension sweep at flat, midpoint, and
  folded states for all seven built-in carton, sleeve, mailer, pouch, tube-label,
  and bottle-label templates. The minimum-dimension sweep for the five
  rectangular nets now also checks every face dimension and shared hinge seam
  at flat, midpoint, and fully folded states. Refreshed Windows optimized
  builds measured 47,493,632 bytes by default and 51,678,208 bytes with `gpu`;
  both pass the 60,000,000-byte budget. The refreshed GPU resolve smoke on an
  NVIDIA RTX 3050 Ti matched CPU-reference output exactly. Current release-build
  cache timings, the 100k-triangle CPU sample, and digests are recorded in
  `docs/performance.md`. The normative v11
  invalid-case manifest is now executed by an integration regression against
  the valid base fixture; it checks missing references, duplicate UV-island
  triangle ownership, invalid product profiles, and binding cycles. The
  manifest's expected diagnostic classes and JSON pointers were corrected to
  match the actual fixture and validator. The current Windows working tree
  passes `cargo test --all-targets --locked` (including the latest packaging
  and print-color tests), `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo fmt --check`, and `cargo check --all-targets --features gpu --locked`.
  The concurrency-sensitive private-cache writer regression passed 20 repeated
  Windows runs after one failure in the first all-target run; the complete
  all-target run passed on retry. The current release profile built into an
  isolated target because the default release executable is held open by the
  running Studio server. Its tree/search and v11 packaging preflight, folded
  preview, and flat SVG export smoke commands all passed. This verifies the
  local Windows build and CLI only, not hosted release artifacts.
  After print-page bounds and bundle verification were tightened, the Windows
  release profile rebuilt successfully and a fixture with an imported CMYK
  output profile exported and verified a complete bundle: all eight artifacts
  passed, the folded preview rendered on the CPU reference path, and the flat
  SVG and vector cut/fold plates shared a 730 × 310 mm page. This remains local
  Windows evidence only. After adding transactional cylindrical seam settings,
  the complete locked test run passed (423 library tests and all 12 integration
  suites), the Studio seam-control flow passed browser QA at desktop and
  390×844 mobile sizes, and the optimized Windows release binary passed tree,
  search, and tube-wrap settings/inspect smokes. The current `gpu` all-target
  check, formatting, and Clippy also pass. This is still local Windows evidence;
  no hosted three-platform result is claimed.
  After adding bundle dry-run and stale-revision regression coverage, the
  complete Windows `cargo test --all-targets --locked -- --test-threads=1`
  passed: 423 library tests and all 12 integration suites. Test temporary
  files were redirected to the ignored `target` directory on D: because C:
  had no free space; the isolated packaging and cache-restart failures from
  the earlier parallel run all passed individually and in this serialized
  run. Warning-denied Clippy, formatting, and the all-target `gpu` feature
  check also pass. This remains local Windows evidence only.
  A fresh rerun of the current worktree initially exhausted the Windows paging
  file while Cargo compiled test targets concurrently; rerunning with both
  Cargo build jobs and test threads set to one passed all 423 library tests and
  all 12 integration suites (including the 150-test packaging/Studio suite).
  The current embedded Studio also passes `bun run typecheck` and `bun run
  build`; the build regenerated `web/studio.html` from the checked-in UI source.
  `cargo fmt --check`, `git diff --check`, and warning-denied all-target Clippy
  pass. This is local Windows evidence; hosted cross-platform CI and browser
  acceptance remain outstanding.
  Three unrelated benchmark tests are intentionally ignored by the all-target
  test command. Hosted cross-platform CI and browser-level Studio acceptance
  remain outstanding.

- [ ] **14. Document and release evidence.** Publish the binding, preset,
  dieline, print, CLI, cache, license, and Studio contracts. Synchronize schema,
  help, README examples, fixtures, and user-facing examples. Record which
  preset source assets are embedded and their license. Do not mark complete
  until Linux, Windows, and Apple Silicon macOS CI passes and all release smoke
  cases cover binding, mockup render, packaging preview, and flat export.

  Release-workflow smoke coverage now copies the v11 packaging fixture and runs
  dependency inspection, a linked perfume mockup render report, packaging
  preflight, folded PNG preview, flat SVG export, and—after importing a local
  CMYK output profile—the complete print-bundle export and verification on every
  release target. The local Windows release-profile build passed the bundle
  export and verifier for all eight artifacts, but does not replace tagged-
  artifact or hosted matrix evidence. The expanded command sequence, including
  local profile import and output-role assignment, passed end-to-end against
  the current Windows build; hosted Linux/Windows/macOS runs remain required.
  A fresh isolated Windows release-profile build (using the exact current
  worktree) then passed the v4 tree/search smoke, `--features gpu` all-target
  check, and the v11 carton preflight/0.75-fold PNG/flat SVG CLI smokes. The
  same release binary verified the linked-art print bundle: all eight artifacts
  passed, including the reference-only cut map, print guide, folded preview,
  print draft, flat SVG, and three role-separated vector plates. The package
  correctly reports `manufacturing_certified: false`. A bare generated-carton
  preflight surfaced five current-fold panel intersections and returned
  `needs-review`; physical carton validation remains open. This is local
  Windows evidence only. A fresh default release-profile build of the current
  worktree then completed in 13m28s at 47,631,360 bytes and passed v4 tree and
  search plus v11 packaging preflight, folded preview, and flat SVG export
  smokes. The hand-authored fixture's preflight correctly remains
  `needs-review`: it has a cut contour outside the panel envelope, an unbound
  panel, and no safe-area inset; this smoke is not a production-conformance
  pass. The README's 3D/packaging summary was synchronized
  with current behavior: it now describes eligible cut-edge raster bleed,
  implemented face selection and UV island packing, and clearly distinguishes
  those capabilities from still-open structural and manufacturing validation.
  Tag-triggered releases are staged as drafts for artifact/release-note review;
  hosted three-platform CI and a real tagged artifact run remain required.

## Acceptance targets

- A saved document with page and node bindings reopens identically; editing the
  source updates the dependent mockup without changing unrelated mockup pixels.
- A parametric perfume bottle can be created, resized by changing its profile,
  given separate body/cap materials, and wrapped with a linked label. Moving the
  label in the 3D view updates its UV placement; moving the UV island updates
  its visible surface placement; changing the source page updates both.
- A user can import an arbitrary supported model, inspect/select its UV islands,
  add a UV overlay or supported projection, and place linked decal/fill artwork
  with deterministic seam, overlap, and texture-resolution diagnostics.
- A missing source, missing slot, invalid UV, or dependency cycle fails with a
  stable structured diagnostic and leaves the original document byte-for-byte
  unchanged.
- Cache hit/miss and invalidation tests prove every output-affecting field is
  keyed and unrelated edits do not invalidate the result. Cache remains bounded
  and disposable.
- The same fixture rendered at the same requested output size by the CPU
  reference produces the same digest on all supported release targets.
- Placed mockups report correct effective ppi and pass/fail preflight at the
  configured threshold. PDF/X draft and separations include bound 3D content
  with the documented color behavior.
- Every built-in packaging template has a stable panel/face/UV map. Flat,
  intermediate, and folded geometry has no unintended gaps or overlaps;
  selecting a panel or face selects its exact counterpart in the other view.
- A cylindrical label wrapping a bottle/tube has a reproducible seam, measured
  circumference mapping, printable bleed, and consistent 3D/flat output. A
  carton for an apparel product can be edited flat, viewed folded, and exported
  with print production lines separated from artwork.
- A packaging handoff lets a designer compare the folded appearance with the
  flat print layout, then identify the exact vector through-cut, crease, and
  perforation paths as separate finishing operations. Bleed and safe-area
  guidance remain distinct from cut paths. The handoff must say plainly when
  its geometry is draft/reference-only and must not claim that a printer or
  cutting machine can use it without further validation.
- Updating artwork on a dieline panel updates the folded preview; exporting the
  flat dieline preserves bleed and separates cut/crease/perforation roles as
  documented. A packaged result passes its own verification and reports all
  external/embedded dependencies.
- Batch render output names, ordering, per-item JSON, and successful output
  bytes are deterministic for a fixed input set and reference renderer.

## Explicitly deferred

- Cloth, fluid, soft-body simulation, physics, and photoreal ray tracing.
- Automatic unfolding of arbitrary imported 3D meshes into manufacturing
  dielines. Built-in parametric forms have authored UVs; supported imported
  models can have UVs inspected and overlaid/projected, but cannot be promised
  to unfold into a structurally correct package.
- Structural engineering certification (compression strength, material yield,
  shipping/drop testing) and manufacturing vendor certification.
- Non-developable doubly-curved packaging that cannot flatten without
  distortion. Cylinders, cones, tubes, and other developable shapes are in
  scope only where the template can define a valid flat panel map.
- General-purpose polygon modeling, sculpting, retopology, rigging, and
  unrestricted manual UV seam painting. v0.16 provides focused parametric
  product forms plus UV island placement/projection tools; arbitrary geometry
  continues to come from imported models.
- DXF, ArtiosCAD, and Esko dieline interchange; this belongs to v0.17.0 design
  interchange.
- Cloud preset catalogs, account-backed assets, live web collaboration, and
  external render services.
- Time-based animation or video export. The fold slider is an editor preview
  control only; the saved deliverable is a static folded render plus the flat
  print layout.
