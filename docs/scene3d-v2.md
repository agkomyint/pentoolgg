# Product forms, surface graphics, and dielines (scene3d v2, document v11)

This is the v0.16.0 extension to [static 3D v1](scene3d-v1.md). It adds editable parametric product forms, source-linked surface graphics, editable UV island overlays, and packaging dielines with folded 3D views. The document envelope is format v11; see [pen-format-v11.schema.json](pen-format-v11.schema.json). Version 10 remains unchanged. Upgrade explicitly with `pentool migrate FILE --target 11`; downgrade to v10 is refused while product forms, dielines, graphics, or editable UV islands remain.

The format stores design intent, not only a flattened preview. Artwork remains editable and is referenced by stable IDs. Parametric meshes and rendered pixels are deterministic derived results. A dependency graph is validated before render or mutation; cycles are rejected.

## Compatibility and identity

- v11 retains all v10 fields and their meaning. A v10 document is never silently upgraded.
- A v11 `scene3d` node has exactly one of `model` (imported model digest), `product_form` (key in `product_forms`), or `dieline` (key in `dielines`). Camera, lights, layout rectangle, background, and render quality remain node properties.
- Page IDs are part of page/node source references because node IDs are page-scoped. Material slot IDs are stable machine identifiers; display names are not.
- `product_forms` and `dielines` are document-level maps. `surface_graphics` are stored on the consuming `scene3d` node. Editable UV overlays stay under `models[digest].uv`.
- Mutations use shared transaction/history. A failure leaves the document byte-identical. Unknown extension data remains preserved.

## Explicit ground plane

A v11 `scene3d` node may set `ground_plane` to `{ "enabled": true, "color": "#d8d5d0", "size_multiplier": 2.5, "offset": 0 }`. It creates an opaque, rough horizontal scene object under the model bounds. Footprint is the model-bounds diagonal times `size_multiplier` (1..=4); `offset` (0..=0.25) lowers the plane by that fraction of the same diagonal, plus a tiny separation epsilon. The default is disabled, so existing v10/v11 renders do not change. It participates in the shared CPU shadow maps and in cache identity; render reports return the setting. Studio can enable and edit the setting. Setting `mode` to `shadow-catcher` instead produces only direct-light occlusion over transparency; `opacity` (0..=1, default 0.65) scales shadow opacity. Catcher color is the shadow tint (default floor color); no opaque floor is emitted. Shadows must be enabled and the document background should remain transparent for compositing. This does not change the document's flat print layout.

## Parametric forms

`product_forms` is keyed by stable form ID. Each record has a generator version, kind, and bounded parameters. Initial forms are `box` (length/width/height and corner bevel), `pouch` (width/height/gusset depth with independent printable regions), `cylinder`/`tube` (height, outer/inner diameter, radial segments), and `lathe-bottle` (height, radial segments, ordered `[radius_mm,height_mm]` profile control points from base to mouth). Dimensions and profile coordinates use millimeters. Profiles must be finite, bounded, ordered, and non-self-intersecting. Bottle control points are deterministically interpolated into four clamped Hermite rings per control interval; axial ring heights remain strictly ordered and radii stay within each neighboring control pair. Optional `profile_tangents` stores one bounded radius-mm/height-mm slope per control point; omitted tangents retain automatic curve behavior. Geometry, material slots, and default UVs are generated deterministically. A bottle may include `cap_mm: {"radius_mm": ..., "height_mm": ..., "offset_mm": 0}`; `offset_mm` defaults to zero for older documents and shifts the cap vertically from the bottle mouth, bounded so the cap stays above the bottle base and within the supported model range. The cap skirt and crown meshes use the `cap` material slot, separate from the bottle body for material editing and surface graphics (but still part of the same product-form model). `scene3d DOC form presets` lists seven editable procedural starters: `perfume-bottle`, `cosmetic-jar`, `cosmetic-tube`, `shipping-carton`, `product-card`, `device-frame`, and `stand-up-pouch`. Studio offers the same seven choices in its product-form creator. Catalog entries include recipe digest, license and attribution, named material slots and normalized UV rectangles, camera framing, default lighting, optional ground/shadows, and minimum preview resolution; the digest identifies the procedural recipe, not an external model asset. Their ordinary form parameters can be changed with `form set`. The carton starter is a visual box mockup, not a folding carton dieline; the pouch is a simplified sealed gusset form, not manufacturing CAD.

This is focused product-form authoring, not a general mesh modeler. Arbitrary shapes continue to use imported immutable glTF/GLB or OBJ; users can still edit supported materials, UV overlays, and surface graphics on those models.

### 3D artwork in print output

Linked surface graphics are rasterized into the deterministic CPU mockup before
they enter page composites, print PDFs, or PDF/X. Spot paints use their declared
portable fallback color for those preview pixels; the spot ink identity and
separate plate are not propagated through the 3D surface. CMYK separation treats
the rendered mockup as process color and reports that flattening. Print preflight
follows visible linked artwork and warns when spot semantics will be flattened;
a strict recipe may promote that warning to an error. Packaging dieline cut,
crease, and other production-role plates remain separate vector/spot outputs,
but do not include spot channels painted onto a folded 3D mockup. Use the flat
artwork/production paths when a production spot plate is required. This is a
print-output limitation, not a limitation on editing the linked source artwork.

Lathed product forms may declare `label_band_mm: [low, high]`. The bounds must coincide with profile-ring heights; geometry is split deterministically so the `label-band` material slot owns only faces within that interval. Its catalog UV rectangle is `[0, low/height, 1, (high-low)/height]`. Bevelled box faces are projected along the dominant face-normal axis and normalized independently into 0..1 UV space. The v0.16.0 starter regression generates every recipe and verifies each declared slot has faces whose UV bounds match the catalog rectangle.

## Editable UV overlays

The v10 UV projection record remains valid. A v11 primitive overlay may also contain `islands`, each with a stable ID, source UV set, triangle membership, and 2D scale/rotation/offset transform. A triangle may belong to at most one editable island per primitive/UV set. Effective UVs apply the island transform after source or generated UVs. Island transforms pivot around the mean of the island's triangle corners; negative scale flips an axis. Coordinates are normalized; `(0,0)` is texture top-left and `v` grows down. `clamp`, `repeat`, or `mirror` is explicit, never inferred from out-of-range UVs.

UV edits never rewrite imported source bytes. Automatic planar/box/cylindrical/spherical projection remains available. `scene3d uv island add|set|list|remove` edits triangle membership and transforms transactionally; `scene3d uv show` reports effective bounds, mirrored transforms, and triangle-interior overlap pairs. Exact overlap analysis is capped at 1,000,000 triangle-pair tests per primitive and reports when truncated. UV export visualizes the same mapping used by rendering. Limits: 4096 islands per model overlay, 1,000,000 triangle references per model, 256 graphics per 3D node, and 16,384 px texture edge, in addition to v10 model/render limits.

Studio's UV workspace pairs the layout with a CPU-rendered preview capped at a 512px longest edge for imported models and generated product forms. Product-form UV overlays are optional per-form `uv` maps keyed by primitive index (or `*`); absent overlays preserve generator UVs. Selecting an editable island highlights its member triangles in a transient render; that preview bypasses the PNG cache and does not modify the document. The page-context toggle returns to the normal editable canvas. UV SVG, PNG, density, overlap, and seam data share the effective coordinates.

## Source-linked surface graphics

Each `scene3d.surface_graphics[]` record contains a stable `id`, source, target material slot, mode, projection/island, transform, fit/crop, repeat behavior, opacity/blend, and optional supported material response. Sources are `{ "kind":"page", "page":"front" }` or `{ "kind":"node", "page":"front", "node":"brand-mark" }`.

Current v11 CPU export support includes linked page/node sources, stored UV or procedural planar, box, cylindrical, spherical, and triplanar projection (procedural axes x/y/z; triplanar uses world-space normal-weighted projections and requires no source UVs), clamp/repeat/mirror addressing, normal/multiply/screen/overlay/darken/lighten blend, crop, contain/cover/stretch fit, decal placement scale/offset/rotation, opacity, and material-slot targeting. An optional `island` targets only the triangles owned by that stable UV island on UV set 0; the overlay geometry is restricted to those triangles. An absent/unknown island fails with an actionable validation error. Per-graphic `material` supports bounded metallic, roughness, and opacity overrides on the generated overlay material; the top-level graphic opacity and material opacity multiply. Studio's mockup panel exposes island ID and material-response fields. Blend operations are composited in linear light after shading. Source artwork is rendered through Pentool's regular page/node renderer, including its existing transforms, clipping, visibility, and color pipeline; v11 has no independent source color-role or output-intent override. `fill` covers the slot UV texture; its optional scale sets 1..=16 horizontal/vertical tile counts (at most 256 total), and offset sets normalized tile phase. Mirror alternates tile orientation. Projection modes beyond those listed fail validation with an explicit `unsupported-capability` error instead of silently approximating them. The CPU renderer preflights framebuffer plus resolved-image storage and refuses renders exceeding 256 MiB; depth-of-field preflight includes its accumulator as well as one framebuffer/image pair. The in-process PNG cache is capped at 64 MiB with FIFO eviction. Its key combines render settings and graphic records with a digest of resolved linked-art pixels, so source edits cannot reuse a stale mockup. Bound art is still rasterized to establish that digest before lookup. Rust callers can explicitly evict results matching a changed source page or node with `node::evict_png_cache_source`. Shared document commits and history restores automatically evict changed direct sources and walk linked page/node graphics and dieline panel dependencies transitively, including uncached intermediate mockups; model, product-form, and dieline catalog edits also invalidate owners and downstream consumers. Working-role changes invalidate linked mockups, while proof-role-only changes and unbound renders retain their cache entries. The in-process poisoned-lock path is tested as a cache miss, and an end-to-end server test confirms cache entries disappear on restart without changing rendered output; no persisted cache-corruption format exists. `pentool scene3d DOC render-report NODE` runs the deterministic render path and emits the exact PNG dimensions/bytes, effective X/Y PPI, renderer requested/used (including accelerated fallback), cache-hit status, and document revision without modifying the document. Effective PPI uses the containing page's `production.units_per_inch` (default 72) and the scene3d node's placed width and height. Rust callers can use `render_png_report_in_page` for the same per-call report. The server also exposes this report at `POST /api/scene3d/render-report` with `{document,page,node}`; Studio's typed client exposes the same endpoint. The local server exposes cache counters through `GET /api/scene3d/cache` (also included in `/api/scene3d/status`) and clears disposable results through `DELETE /api/scene3d/cache`. GPU preview, explicit source-color-role management, and full workspace editing are also outstanding.

Bindings can be managed transactionally from the CLI:

```sh
pentool scene3d DOC graphic bottle-preview add front-label --source page:label-art --slot label-band --mode decal --fit contain --scale 0.8,0.3 --offset 0.5,0.48
pentool scene3d DOC graphic bottle-preview set front-label --rotate 8 --opacity 0.9
pentool scene3d DOC graphic bottle-preview list
pentool scene3d DOC graphic bottle-preview remove front-label
```

Node selectors use `--source node:<page-id>:<node-id>`. Add/set/remove accept the standard top-level `--dry-run` and `--if-revision` options. Lists are read-only. Final document validation resolves source IDs and slots before a commit; invalid edits leave the source file unchanged.

`decal` mode places a localized graphic on a surface; `fill` maps artwork across a named material region. Both remain linked to their source. Graphics can move, rotate, scale, crop, repeat, or mirror; supported graphic material response is limited to documented matte/glossy/metallic parameters, not arbitrary shaders. The target is a stable primitive/material slot and optionally a UV island. Entries composite in array order with the shared compositor's opacity/blend semantics.

Sources render with normal transforms, clipping, visibility, and the existing Pentool color pipeline. A page source excludes its consuming node. A node source resolves in its own page. There is no v11-specific color role, profile, or intent field: changing color behavior requires editing the source document through the established color-management features. Missing references/slots/islands, unsupported texture paths, and dependency cycles fail before mutation with an actionable structured diagnostic. Cache identity includes render settings, graphic source/placement records, and the digest of resolved linked-art pixels; model storage/UV identity is already included in the model key. The current cache is disposable and process-local, with server inspection/clear endpoints, per-render hit reporting, and hit/miss/FIFO-eviction counters. It reuses identical mockup results after unrelated document edits. Shared commits and history restores invalidate changed direct sources and follow linked page/node and dieline panel dependencies transitively, including uncached intermediate mockups; model/product-form/dieline catalog edits and working-role changes invalidate linked consumers. Proof-role-only changes do not evict these renders. Dedicated restart/corruption coverage remains open.

## Dielines and folded packaging

Generated dielines also create a linked `layout_page` containing normal v4
rectangle, text, and path nodes. `page.dieline_layout` declares the dieline ID
and millimeter coordinates; node metadata (`dieline_panel_id`,
`dieline_panel_label`,
`dieline_safe_zone_panel`, `dieline_path_id`, and `dieline_crease_id`) maps
editable shapes back to stable dieline records. The shared resolver applies
page transforms and reads those nodes before flat export, preflight, and folded
rendering, so edits flow through the same panel/path model. Legacy dielines
without `layout_page` retain their stored geometry unchanged. Studio's **Edit
flat layout page** opens this ordinary canvas. Its initial canvas and trim
bounds include production paths and structural crease endpoints as well as the
panel union, so extended cut/fold geometry is visible in the normal editor.
Linked artwork itself remains
referenced from its source page and is placed through the flat-preview controls.

`dielines` is keyed by stable ID and stores template kind, millimeter units, dimensions, board thickness, named panels, creases, and production paths. Panel flat rectangles and crease/path coordinates are in the dieline's millimeter plane; UV rectangles are normalized with top-left origin and downward-growing `v`. A panel contains a stable ID, flat-layout rectangle, folded face/material slot, normalized UV rectangle, optional artwork reference, and optional `safe_zone_mm` inset measured inward from its edges. The inset must be finite, non-negative, and less than half the panel's shortest side. Creases have stable IDs, flat line geometry, target angle in degrees, and `mountain`/`valley` direction; `fold_state` in `[0,1]` scales every stored target angle from flat to target, with the child-panel side determining the signed rotation. Paths use roles `cut`, `crease`, `perforation`, `bleed`, `safe-zone`, or `glue-flap`; an optional `panel_id` on a cut path associates one closed contour path to the stable panel whose artwork trim it defines. The association must reference an existing panel and its contour bounds must remain within that panel. Existing unassociated paths remain valid. A panel-associated cut path must flatten to one simple, non-holed contour with at most 2048 vertices at 0.1 mm tolerance and preserve every complete shared fold edge. Flat export clips linked panel artwork to the contour and synthesizes nearest-contour bleed within the same 16-megapixel aggregate bound as rectangular bleed. The folded panel mesh is triangulated from that same contour and maps its UVs proportionally within the canonical panel rectangle. Hinge checks use the contour's shared-edge vertices, and the bounded fold-collision screen operates on mesh triangle pairs. Positive board thickness creates panel interior faces and exposed cut-edge walls, offsets fold axes to keep outer surfaces joined, and adds an untextured bridge across the inner wedge of each folded hinge; it does not alter flat cut geometry. The hinge bridge is a visual closure only, not a model of score compression. Board/fold allowances and manufacturing compensation are not modeled. Glue-flap allowance remains metadata. Production roles can map to dedicated spot separations.

The dieline is the source of truth: panel ID ↔ flat region ↔ folded face ↔ UV rectangle is one mapping, not duplicated data. Flat art remains ordinary editable `.pen` content. Studio panel selectors and `scene3d DOC pack bind DIELINE --panel PANEL --source page:PAGE` link artwork without copying it; omit `--source` to clear the binding. A panel may also store `artwork_transform` with normalized `scale: [sx, sy]`, `offset: [cx, cy]` (artwork center in panel coordinates), and `rotate` in degrees. The default is `[1,1]`, `[0.5,0.5]`, `0`; scales must be in `(0,1]`, the unrotated rectangle must stay within the panel, and rotation is bounded to ±360°. The same transform is applied by the flat SVG exporter and the folded CPU preview, so placement does not drift between those views. It changes placement only; it does not alter source artwork or panel UV mapping. Studio edits it through validated, undoable document changes. Source visibility, existence, and dependency cycles are validated before save. In the 3D workspace, pointer positions are converted through the selected node's parent transform and passed to the shared CPU camera-ray triangle picker; the nearest hit returns its primitive and stable dieline panel ID, which selects the flat counterpart. The same `/api/scene3d/uv/face-pick` route continues to support primitive-constrained UV selection for imported models. The current CPU fold solver supports rectangular, connected, tree-shaped panel nets only: every joined panel pair must share a complete edge and have a matching crease. `fold_state` is a normalized 0..1 multiplier on each crease's `angle_deg`; direction and which side of the crease the child occupies determine rotation sign. Valley folds move the child away from the root panel's printed (+Z) side; the generated package exterior normals and linked art are regression-tested in the assembled CPU preview. Studio's fold slider previews this shared multiplier, while per-crease target angles can be changed in the inspector or with `pack crease DIELINE CREASE --angle-deg ANGLE`. Both edits use validated, undoable document changes. Folding is a static design control, not a timeline. Positive board thickness creates panel interior faces and exposed cut-edge walls; a visual, untextured bridge closes each folded interior hinge wedge while half-thickness hinge offsets preserve the outer seam. Score compression, fold allowances, and closed structural validation remain unimplemented. Packaging preflight checks current-fold-state face intersections and a limited plane-distance clearance screen; angled board clearance is not modeled.

The schema catalogs straight/reverse-tuck carton, sleeve, mailer, cylinder/tube, pouch, and bottle-label template identifiers. Studio generates a four-panel open sleeve, visual straight-/reverse-tuck carton, or a seven-panel hinged-lid mailer. The carton net has four body panels, a glue-flap region, two closure lids, two tuck tongues, and four dust flaps, with cut/crease/glue-flap paths and a stable panel-to-face/UV mapping. The mailer net has a base, four walls, lid, and side glue joint. These templates use the bounded rectangular tree-net fold solver. Positive board thickness adds inner panel faces and exposed cut-edge walls, bridges folded interior hinge wedges, and offsets folded hinge axes; flat cut paths do not change. Hinge bridges are visual surfaces, not score compression, and fold allowances are not calculated. Stored bleed expands the flat output and linked artwork is edge-extended beyond non-crease cut edges; fold edges and tube wrap seams are excluded. This visual bleed treatment does not offset cut paths or constitute production-grade CAD. Packaging preflight has a bounded zero-thickness collision scan and a limited board-thickness plane-distance clearance screen; angled gaps, structural behavior, and manufacturing certification remain unvalidated. A constrained labeled, rectangular-net SVG import is implemented; production-ready template generation is not. DXF/ArtiosCAD/Esko import belongs to v0.17. Arbitrary mesh unfolding is out of scope.

Studio's **Preview flat** action sends the current document to
`POST /api/scene3d/pack/flat-preview`. The Rust exporter returns a bounded SVG
with linked artwork and any stored safe-area inset guides. Artwork can be dragged within its panel to adjust its
normalized center; release stores the change through
`POST /api/scene3d/panel-art-transform` and refreshes the shared flat preview.
Keyboard users can select a panel and use the scale, center, and rotation
fields. This remains a placement surface, not a general editable 2D `.pen`
canvas; panel selection is not yet synchronized from 3D face picking.

The v11 dieline contract bounds stored board thickness to 0..=100 mm. Positive
thickness drives inner panel faces, exposed cut-edge walls, folded-hinge interior
bridges, and half-thickness fold-axis offsets in the preview; it does not change
flat cut paths. The bridge does not model score compression, and no manufacturing
allowance or certification is implied.

## CLI direction

The top-level `mockup` command is the concise entry point for product starters.
`mockup list` prints the licensed offline recipe catalog as JSON;
`mockup apply DOC --preset ID --id NODE --layer LAYER` creates an ordinary v11
product-form record (ID `NODE-form`) and its scene3d placement through the same
transaction, dry-run, and revision-guard path as `scene3d DOC form add`.
Placement defaults to `(0,0,400,400)` and `--page` is optional.

The current v0.16 packaging commands create visual sleeve, tuck-carton, mailer, and tube-label mockups, import a bounded labeled SVG subset, list supported templates, inspect stored dielines (including per-panel artwork transforms and safe-area insets), run preflight, and export flat SVGs with linked artwork and configured safe-area guides. Preflight parses stored production paths, checks their bounds against the panel envelope plus bleed, reports disconnected panel nets, and screens current-fold-state intersections plus limited board-thickness plane-distance clearance. Positive thickness adds panel interiors, exposed cut-edge walls, and visual bridges across folded interior hinge wedges; these bridges do not model score compression. Flat cut/fold allowances, comprehensive gap validation, engineering strength, and manufacturing certification are not provided. Commands are available under both `pentool scene3d DOC pack ...` and the shorter `pentool pack DOC ...` namespace; `pentool pack templates` needs no document. PDF/X remains a flattened process-CMYK draft. `pack separate` exports technical-role plates as grayscale PNGs; `pack export-bundle` combines the flat SVG, folded PNG preview, flattened PDF/X draft, grayscale plates, editable vector SVG plates grouped by spot ink, and a relative-path manifest into one failure-atomic directory. The flat SVG bounds now include production paths and structural crease endpoints, so cut geometry cannot silently land outside its declared print page. `pentool pack verify-bundle DIR` checks each hash-listed artifact against its manifest, rejects missing/modified files and paths escaping the bundle, requires the cutting map's reference-only production legend, compares map cut/fold path geometry to `flat.svg`, and compares normalized vector spot-plate geometry to the corresponding flat-dieline role after bleed translation. It also verifies that flat production geometry stays inside its page and every vector plate retains the flat layout's physical millimeter page size and matching `viewBox`, along with one finishing role under its declared ink. It reports handoff integrity only—not structural or manufacturing readiness. The vector plates preserve millimeter dimensions and source path/role IDs. Global `--dry-run` performs the same plan and render checks but removes its temporary staging output and reports `complete: false`. This bundle is a review handoff, not certified manufacturing data. Keep the implemented spelling synchronized across CLI help and public docs:

```sh
pentool mockup list --json
pentool mockup apply DOC --preset perfume-bottle --id bottle-front --layer mockups --page products --width 640 --height 720 --dry-run --json
pentool scene3d DOC pack sleeve --dieline sleeve --node sleeve-preview --layer mockups --length-mm 120 --width-mm 70 --height-mm 180 --board-mm 0.5 --bleed-mm 3 --dry-run --json
pentool scene3d DOC pack carton --dieline carton --node carton-preview --layer mockups --template straight-tuck --length-mm 120 --width-mm 80 --height-mm 220 --board-mm 0.5 --bleed-mm 3 --glue-flap-mm 15 --dry-run --json
pentool scene3d DOC pack tube --dieline label --node tube-preview --layer mockups --radius-mm 30 --height-mm 140 --dry-run --json
pentool pack DOC import imported-carton --svg carton.svg --node imported-preview --layer mockups --dry-run --json
pentool scene3d DOC pack bind sleeve --panel front --source page:front --dry-run --json
pentool scene3d DOC pack bind sleeve --panel front --dry-run --json
pentool scene3d DOC pack crease carton front-body-fold --angle-deg -45 --dry-run --json
pentool scene3d DOC pack preview carton --fold 0.75 --out carton-preview.png
pentool scene3d DOC pack templates
pentool scene3d DOC pack inspect carton
pentool scene3d DOC pack preflight carton --min-ppi 300
pentool scene3d DOC pack export carton --out carton-flat.svg --min-ppi 300
pentool pack DOC export carton --out carton-proof.pdf --pdfx --ppi 300
pentool pack DOC separate carton --out carton-separations --ppi 300 --bits 8
pentool pack DOC export-bundle carton --out dist/carton --min-ppi 300 --ppi 300 --fold 1
pentool pack verify-bundle dist/carton
pentool pack templates
pentool pack DOC inspect carton
pentool pack DOC preflight carton --min-ppi 300
```

`pack tube` creates a one-panel cylindrical label. Its flat panel width is
`2π × radius_mm + overlap_mm` (the optional overlap is capped at 25% of the
circumference). The same stored radius, seam angle, direction, and overlap drive
the flat layout and deterministic 3D surface. The tail is drawn as a slightly
raised overlay on the wrap start; the flat SVG highlights the extra printable
strip. Preflight warns when conservative transformed artwork bounds touch
either seam edge so that alignment can be checked. Tube ends are intentionally
open and are not structural packaging geometry.

`pack import ID --svg FILE.svg` accepts only UTF-8 SVG with explicit `mm`
width/height and a matching `viewBox`. Panels must be named positive-size
rectangles (`data-panel`); production paths need `data-id` and a supported
`data-role`. Crease paths must use straight M/L segments and explicitly declare
`data-direction="mountain|valley"` plus `data-angle-deg`. It rejects CSS,
transforms, hidden geometry, foreign namespaces, external references, curves in
creases, and unlabeled/unsupported drawing. The import is bounded to 4 MiB and
uses the existing rectangular tree-net solver; it is not a general SVG or CAD
importer.

`pack preflight` reports unbound panels, missing stored cut/crease paths,
rasterized artwork resolution, missing safe-area insets, artwork bounds that
enter configured safe areas, the configured ICC CMYK output role required by
the flattened PDF/X bundle, and known manufacturing blockers as JSON. The
`pdfx_output_role` object reports `configured`, destination `space` and
`profile`, plus a corrective `message` when the role is missing or not ICC
CMYK; it is distinct from dieline geometry status. Bounds
use the linked source viewport's rotated rectangle, not transparent pixels or
visible vector ink, so these checks are conservative guidance. Its
`mockup-ready`/`needs-review` status is not a print-production pass: board
compensation, synthesized bleed/safe zones, comprehensive physical-thickness
clearance, and structural validation remain unavailable. The folded preview
includes a bounded zero-thickness intersection scan and, when board thickness
is positive, a board-thickness clearance screen: overlapping face interiors
whose plane-distance range stays within the stored board thickness are review
warnings. Angled clearances and crease compression are not modeled.

`pack preview DIELINE --fold 0..1 --out FILE.png` renders the linked package
node with the CPU reference renderer at a temporary fold amount. It writes a
static PNG without changing the `.pen` document and refuses to overwrite that
document.
Preview and flat export also accept the standard `--if-revision REV` guard;
they reject stale document snapshots before rendering or writing and include
the source revision in their JSON report. Global `--dry-run` still performs the
same render/export plan and reports its byte count, but does not create or
replace the requested output; its report sets `dry_run: true` and
`complete: false`.

`pack export DIELINE --out FILE.svg` exports panel guides plus stored cut,
crease, perforation, bleed, safe-zone, and glue-flap paths in millimeters.
Bound page/node artwork is included; vector-compatible sources remain vector,
while unsupported source content is embedded as a CPU-rendered PNG. The JSON
report identifies rasterized panels, effective ppi, warnings below
`--min-ppi` (default 300), and per-panel synthesized bleed resolution/edges.
Linked artwork is rasterized only for bleed synthesis, then its nearest edge
pixels are extended by the stored bleed amount on cut edges. Crease edges and
tube wrap seams do not receive bleed. The bleed raster uses
`max(300, --min-ppi)` ppi and is bounded to 16 megapixels aggregate; exports
exceeding that bound fail with an actionable limit error. The edge extension is
a design aid and is not certified manufacturing output.
`--pdfx --out FILE.pdf` routes the same flat SVG plan through the shared PDF/X-4
exporter using the document's CMYK output role and intent. This is a flattened
process-CMYK draft: cut, crease, perforation, and safe-zone marks are rasterized
into the image, not emitted as spot separations. A CMYK output profile is
required, independent conformance validation remains `not-run`, and the report
explicitly sets `manufacturing_certified` to false. Use the SVG for editable
technical paths; do not send this draft as a cut-path production file.
`--dry-run` applies to both SVG and PDF/X exports and preserves any existing
destination file.
`pack separate DIELINE --out DIR --ppi 300 --bits 8` writes deterministic
grayscale PNG spot plates for stored production paths and panel safe-zone
guides, plus a hash-bearing `separations.json`. Each spot remains on its own
overprinting plate; process plates are blank. Existing output directories are
refused. The export is not a production pass: it does not validate structural
geometry or compensate for board thickness.
`pack vector-separate DIELINE --out DIR` writes editable SVG plates, one per
spot ink, preserving millimeter page dimensions and source path IDs/roles. It
shares the canonical dieline paths and safe-zone guides with raster separation.
Global `--dry-run` executes validation and plate generation in a temporary
directory, then removes it without publishing the destination.

```sh
pentool migrate DOC --target 11
pentool scene3d DOC form presets
pentool scene3d DOC form add bottle-preview --form bottle --preset perfume-bottle --layer mockups --page products --width 400 --height 600
pentool scene3d DOC dependencies
pentool scene3d DOC render-report bottle-preview
pentool scene3d-cache --server http://127.0.0.1:4711 status
pentool scene3d-cache --server http://127.0.0.1:4711 clear
pentool scene3d DOC batch --node bottle-preview --node carton-preview --out-dir mockup-renders
pentool scene3d DOC form add perfume --form perfume-bottle --kind lathe-bottle --parameters '{"height_mm":180,"radial_segments":64,"profile_mm":[[24,0],[26,12],[26,105],[23,120],[11,132],[11,169],[8,180]]}' --slots glass-body,label-band --layer mockups --page products
pentool scene3d DOC form set perfume-bottle --parameters '{"height_mm":190,"radial_segments":64,"profile_mm":[[24,0],[26,12],[26,110],[23,130],[11,142],[11,179],[8,190]]}'
pentool export DOC mockup.png
```

Mutations support `--dry-run` and `--if-revision`. Reports include changed IDs, source/model digests, slot/panel, output dimensions, effective ppi, renderer/cache data, and warnings; they do not inline pixels.

`scene3d-cache status|clear` talks only to a running HTTP loopback Pentool server
(`--server` defaults to `http://127.0.0.1:4711`). It reports the process-local
cache state or clears its entries and counters; external hosts and HTTPS URLs
are rejected. It never reads or changes a document.

`scene3d DOC batch --node ID ... --out-dir DIR` renders selected v11 scene3d
nodes in the shared dependency planner's deterministic order. Unqualified IDs
must be unique across pages; use `PAGE::ID` to select page-local duplicates.
Multi-page batches are supported. Each output filename is deterministic for the selected
set and scene order; the JSON summary includes per-file SHA-256, byte count,
render report, and total pixels/bytes. The command caps batches at 32 nodes,
64 million output pixels, and 256 MiB of PNG data. It renders and validates
every selected item before staging files, then atomically renames a new
same-parent output directory into place. Existing output directories are
refused. Global `--dry-run` performs the same validation/render planning but
does not publish; its summary has `dry_run: true` and `complete: false`. A
failed item therefore publishes no partial batch and never changes the source
document.

`scene3d MANIFEST batch-queue --out-dir DIR` renders across documents using a
versioned JSON queue manifest. Document paths resolve relative to the manifest;
each job selects nodes with the same ID or `PAGE::ID` syntax used by `batch`.
The queue is bounded to eight documents, 32 total mockups, 64 million output
pixels, 256 million supersampled samples, and a conservative five-byte-per-pixel
PNG estimate before rendering. It stages
all document outputs under one same-parent temporary directory and publishes
the final directory with one rename after the queue finishes. `--dry-run`
does all validation/render planning without publishing. `--cancel-file PATH`
checks for a user-created marker between renders; cancellation discards the
entire staged queue. Individual render failures are isolated: each item reports
`status: "rendered"` with its PNG metadata or `status: "failed"` with a
selector and error. Successful PNGs are published together even when some
items fail. The summary reports `requested`, `rendered`, `failed`, `complete`,
and `partial`; `complete` is false when any item failed. Manifest, document,
revision, aggregate-limit, cancellation, staging, or publication errors still
abort the queue without publishing its staged directory.

Manifest shape (also described by `scene3d-batch-queue.schema.json`):

```json
{
  "format": "pentool-scene3d-batch-queue-v1",
  "jobs": [
    { "document": "front.pen", "nodes": ["products::bottle", "products::carton"] },
    { "document": "back.pen", "nodes": ["mockups::bottle"] }
  ]
}
```

## Schema and fixtures

`pen-format-v11.schema.json` keeps the v10 envelope and adds bounded schemas for forms, graphics, UV islands, and dielines. Schema validation covers shape and scalar limits; Rust validation remains authoritative for ID resolution, graph cycles, slot membership, triangle ownership, geometry, paths, render work, and byte limits. The Rust UV loader validates island IDs/transforms, UV-set and triangle references, and exclusive triangle ownership before applying corner-split transforms; source model bytes remain untouched. Minimal fixtures cover a perfume bottle with linked label, a carton with panel/face mapping, and invalid source, duplicate triangle ownership, profile, and cycle cases.

Editable UV CLI examples:

```sh
pentool scene3d DOC uv MODEL show --resolution 2048 --minimum-density 1000
pentool scene3d DOC uv MODEL project --primitive 0 --projection box
pentool scene3d DOC uv MODEL island add --primitive 0 --id front --uv-set 0 --triangles 0,1 --scale 0.8,0.8 --offset 0.1,0 --rotate 0
pentool scene3d DOC uv MODEL island set --primitive 0 front --scale -0.8,0.8
pentool scene3d DOC uv MODEL island list --primitive 0
pentool scene3d DOC uv MODEL island pack --primitive 0 --padding 0.01
pentool scene3d DOC uv MODEL island remove --primitive 0 front
pentool scene3d DOC uv MODEL layout --primitive 0 --out front-uv.svg
```

Island triangle indices refer to the zero-based triangle order of the imported primitive. Island transforms apply after any stored primitive projection and are used by ordinary model previews, exports, and linked surface artwork through the shared loader.
`island pack` deterministically shelf-packs islands independently per UV set,
preserves each island's orientation and mirror state, and applies one uniform
scale per island to fit the unit square. `--padding` is a normalized UV-space
gap on the outer edge and between island bounding boxes (default `0.01`, range
`0..0.25`). It is a transactional edit, honors `--dry-run` and `--if-revision`,
and refuses degenerate or un-packable islands without changing the document.
`uv ... show --resolution N` reports area-weighted texel density per UV set and
editable island as pixels per model unit, with `N` the width and height of a
square texture (default 2048, maximum 16384). OBJ units are user-defined and
the value is intentionally not mislabeled as pixels/mm or pixels/inch. Passing
`--minimum-density N` marks islands below the requested threshold with
`below_minimum_texel_density`; without a threshold, that field is `null`, so
densities are shown for inspection but are not classified as inadequate.
The same report includes possible UV discontinuities: shared geometric edges
whose neighboring triangles assign different UVs. The SVG and PNG layout
exports highlight both UV-side segments in red. Since coincident but separate
shells can produce the same signature, the diagnostic is labeled possible, not
an automatic topology repair. Analysis is capped at 250,000 triangles per
primitive and detailed edge output at 2,048 edges; truncation is explicit in
the report (`analysis_truncated`, `details_truncated`, and
`render_segments_truncated`). Highlight drawing is also capped at 4,096 UV
segments and 16 million PNG line samples per layout.

## Stand-up pouch mockup

`pentool scene3d DOC pack pouch` creates a linked five-panel visual net: front,
back, left/right gussets, and a bottom gusset. `--front-width-mm`, `--gusset-mm`,
and `--height-mm` define its proportions; `--bleed-mm` sets the stored print
bleed. The net has stable panel IDs, face slots, UV rectangles, a cut outline,
and four crease records, and folds through the shared deterministic CPU tree-net
solver. It is a simplified box-bottom bag mockup, not a production pouch
pattern: zipper, formed seals, film thickness, seal allowance, and forming strain
are not modeled, and it is not manufacturing-certified.

```sh
pentool scene3d products.pen pack pouch --dieline pouch --node pouch-preview --layer mockups --front-width-mm 120 --gusset-mm 35 --height-mm 180 --dry-run
```

`pentool scene3d DOC pack bottle-label` creates a single-panel label at a
measured cylindrical label-zone radius. The dieline width is the circumference
plus optional overlap; the shared 64-segment wrap generator preserves its seam
and UV mapping in the folded preview. This is useful for a cylindrical bottle
band, but does not follow a tapered or compound lathed bottle profile and is
not a manufacturing-ready label pattern.

```sh
pentool scene3d products.pen pack bottle-label --dieline bottle-label --node label-preview --layer mockups --radius-mm 30 --height-mm 90 --overlap-mm 3 --dry-run
```

`pack preflight` includes `fold_hinge_checks` and `fold_collision_report`. For
rectangular panel nets it reports the maximum shared-edge endpoint gap in
millimeters for every panel hinge at the stored fold amount; for tube and
bottle-label surfaces it reports the wrap-seam gap. `joined` compares the
measured gap with a size-scaled tolerance. The collision report checks every
panel pair at that fold amount with a bounded triangle-intersection scan;
edge-only contact is ignored. Coplanar overlapping faces with opposite outward
normals count as face contact, while same-facing overlaps and non-coplanar
crossings are reported. For positive board thickness, panel pairs are projected
onto a common plane and flagged when their interiors overlap and the full face's
plane-distance range is strictly less than one board thickness; exact one-
thickness separation is face contact, not positive-volume overlap, and
coincident opposing faces remain contact here too. Generated tuck tongues now
fold toward the carton interior. Both tuck cartons now layer their dust flaps
under the lids: each flap is narrower than its wall by 1.5 board thicknesses per
side, hinged along a partial wall edge with a matching crease, and folds slightly
past square (about 1 degree, or enough for twice the board thickness) so its free
edge sits below the closing lid. Both cartons are clear at flat and fully folded
states under this bounded screen. At intermediate fold amounts the straight-tuck
dust flaps and lids still sweep through each other in transit because one shared
fold amount drives every crease. This limited screen does not model angled
clearance, crease compression, full solid-board geometry, or structural
fitness, and a clear result is not manufacturing certification.
