# 3D mockups and packaging

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [3D mockups and UVs](#3d-mockups-and-uvs)

## 3D mockups and UVs

Pentool imports GLB, glTF, and OBJ models into ordinary `.pen` scene nodes.
Format v10 supports model import and static rendering; v11 adds linked product
artwork and editable UV islands. Migrations are explicit:

```sh
pentool migrate products.pen --target 10
pentool scene3d products.pen add bottle --model bottle.glb --layer mockups --page front
pentool scene3d products.pen uv bottle show
pentool scene3d products.pen uv bottle project --primitive 0 --projection cylindrical --axis y
pentool migrate products.pen --target 11
pentool mockup list --json
pentool mockup apply products.pen --preset perfume-bottle --id bottle-preview --layer mockups --page products --width 400 --height 600 --dry-run --json
pentool mockup apply products.pen --preset perfume-bottle --id bottle-preview --layer mockups --page products --width 400 --height 600 --if-revision 0123456789abcdef --json
pentool scene3d products.pen form presets
pentool scene3d products.pen form add bottle-preview --form bottle --preset perfume-bottle --layer mockups --page front --width 400 --height 600
pentool scene3d products.pen dependencies
pentool scene3d products.pen render-report bottle-preview
pentool scene3d-cache --server http://127.0.0.1:4711 status
pentool scene3d-cache --server http://127.0.0.1:4711 clear
pentool scene3d products.pen batch --node bottle-preview --node carton-preview --out-dir mockup-renders
# For page-local duplicate IDs, qualify the selector as PAGE::ID.
pentool scene3d products.pen batch --node products::bottle-preview --node alternate::bottle-preview --out-dir mockup-renders
pentool scene3d mockup-queue.json batch-queue --out-dir all-mockups --cancel-file cancel.request
pentool scene3d products.pen uv bottle island add --primitive 0 --id label --uv-set 0 --triangles 12,13,14 --scale 0.8,0.8 --offset 0.1,0
pentool scene3d products.pen uv bottle island set --primitive 0 label --rotate 12
pentool scene3d products.pen uv bottle layout --primitive 0 --out bottle-uv.svg
pentool scene3d products.pen graphic bottle add label-art --source page:front --slot label --mode decal
pentool scene3d products.pen pack sleeve --dieline sleeve --node sleeve-preview --layer mockups --length-mm 120 --width-mm 70 --height-mm 180 --dry-run
pentool scene3d products.pen pack carton --dieline carton --node carton-preview --layer mockups --template straight-tuck --length-mm 120 --width-mm 80 --height-mm 220 --dry-run
pentool scene3d products.pen pack tube --dieline label --node tube-preview --layer mockups --radius-mm 30 --height-mm 140 --overlap-mm 3 --dry-run
pentool scene3d products.pen pack pouch --dieline pouch --node pouch-preview --layer mockups --front-width-mm 120 --gusset-mm 35 --height-mm 180 --dry-run
pentool scene3d products.pen pack bottle-label --dieline bottle-label --node label-preview --layer mockups --radius-mm 30 --height-mm 90 --overlap-mm 3 --dry-run
pentool pack products.pen import imported-carton --svg carton.svg --node imported-preview --layer mockups --dry-run
pentool scene3d products.pen pack carton --dieline mailer --node mailer-preview --layer mockups --template mailer --length-mm 180 --width-mm 120 --height-mm 35 --glue-flap-mm 18 --dry-run
pentool scene3d products.pen pack crease carton front-body-fold --angle-deg -45 --direction mountain --dry-run
pentool scene3d products.pen pack settings carton --board-mm 0.5 --bleed-mm 3 --dry-run
pentool scene3d products.pen pack settings bottle-label --seam-angle-deg 42 --wrap-direction counterclockwise --dry-run
pentool scene3d products.pen pack preview carton --fold 0.75 --out carton-preview.png
pentool scene3d products.pen pack templates
pentool scene3d products.pen pack inspect carton
pentool scene3d products.pen pack preflight carton --min-ppi 300
pentool scene3d products.pen pack export carton --out carton-flat.svg --min-ppi 300
pentool scene3d products.pen pack separate carton --out carton-plates --ppi 300 --bits 8
pentool scene3d products.pen pack vector-separate carton --out carton-vector-plates
pentool pack products.pen export-bundle carton --out dist/carton --min-ppi 300 --ppi 300 --fold 1
pentool pack verify-bundle dist/carton
pentool pack templates
pentool pack products.pen inspect carton
pentool pack products.pen preflight carton --min-ppi 300
pentool serve products.pen
```

`mockup list` prints the offline starter catalog as JSON. `mockup apply` creates
an ordinary editable product form plus its scene node in one validated
transaction; the form record ID is deterministically derived as `<node-id>-form`.
The command supports `--dry-run` and `--if-revision`, and JSON reports are the
default (`--json` is accepted for consistency with other commands).

The `batch-queue` manifest lists up to eight documents and their selected
mockups; document paths are relative to the manifest. It preflights aggregate
work before rendering, stages the whole queue, and atomically publishes one
output directory. Individual render failures are reported per item while the
successful PNGs are published together; a summary reports rendered/failed
counts. Creating the optional cancel marker stops between renders and discards
all staged output. See `docs/scene3d-batch-queue.schema.json` for the manifest
contract and `docs/scene3d-v2.md` for the report behavior.

Packaging preview and SVG export accept `--if-revision REV`; successful JSON
reports include the exact source document revision, while stale guards fail
before creating an output file. `--dry-run` performs the same preview/export
work and reports planned bytes without creating or replacing its destination;
this applies to folded PNG, flat SVG, and PDF/X draft export.
`pentool pack DOC export ID --out proof.pdf --pdfx --ppi 300` creates a
flattened PDF/X-4 draft through the document's declared CMYK output role. Its
technical marks are process-color raster content, not spot separations, and it
is not a certified manufacturing file; keep the SVG for editable cut paths.
SVG dieline import is a bounded subset: explicit millimeter dimensions and
viewBox, named rectangular panels, labeled production paths, and straight
creases with explicit direction and angle. Unsupported SVG geometry is refused.
`pentool pack verify-bundle DIR` checks every hash-listed bundle artifact and
rejects missing, modified, or out-of-bundle files. It also checks that the
cutting map retains its reference-only legend and that each vector spot plate
declares one finishing role matching its manifest ink. It also compares the
map's cut/fold path geometry to `flat.svg`; this verifies handoff integrity,
not manufacturing readiness or certification.

Island triangle numbers are zero-based within a model primitive. Island changes
are stored as overlays; imported model bytes remain unchanged. Studio's 3D
workspace can edit linked artwork and UV islands, and previews/exports use the
Rust reference renderer. Studio can generate simplified sleeve, straight- or
reverse-tuck carton, hinged-lid mailer, stand-up pouch, tube-label, and
cylindrical bottle-label mockups, with a static fold preview for panel nets.
These are visual design nets, not manufacturing dielines. Positive board
thickness adds inner panel faces and exposed cut-edge walls, offsets fold axes
to keep the outer seams connected, and bridges the interior wedge at folded
hinges. That bridge is visual only; score compression, cut/fold allowances,
collision-free structural geometry, and certified production output are not
modeled.
The flat SVG export includes linked artwork and stored technical paths; vector
art stays vector and unsupported source content is embedded as bounded PNG with
an effective-ppi warning. Configured bleed expands the flat export and extends
raster artwork along eligible cut edges; crease edges and wrap seams are
excluded. The editor also supports 3D face selection/art placement and UV island
packing. Broader carton structures, profile-conforming bottle labels, and
manufacturing conformance remain in development; see the [v0.16
roadmap](roadmap/v0.16.0/TASKS.md) and
[v11 3D contract](../docs/scene3d-v2.md) (schema:
[`docs/pen-format-v11.schema.json`](../docs/pen-format-v11.schema.json); upgrade with
`pentool migrate FILE --target 11`) for current limits.
