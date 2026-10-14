# v0.15.0 — 3D objects: import, light, render

Let a `.pen` document hold 3D models. Pentool imports glTF/GLB and OBJ, lets an
agent or a person arrange a model, camera and lights, and renders the scene to
a 2D layer that composes like any other node. This milestone is *import,
arrange, light, render*. It is not a modeler.

This is a minor release: it adds format data, commands and editor features.

## Product contract

- A deterministic CPU renderer is **normative**. The same document, sources and
  engine version produce the same pixels on every release target. Golden tests
  run on the CPU path.
- `wgpu` is the optional accelerator, chosen by the owner on 2026-10-11:
  - it is behind a `gpu` Cargo feature and a runtime switch, never required;
  - with no usable adapter, or with the feature off, Pentool falls back to the
    CPU renderer and says so in its report;
  - GPU output must match the CPU reference within a documented per-channel
    tolerance on the acceptance scenes, and a render never silently mixes the
    two paths;
  - a GPU render is labeled `accelerated` in reports and is never used to
    produce a golden, a package digest or a conformance claim;
  - adapter selection, shader compilation and driver behavior never touch the
    network, and a missing driver never fails document open, validation or CPU
    rendering.
  - v0.15.0 ships the adapter, compute-resolve, readback and fallback boundary;
    geometry and shading remain on the CPU reference path. Diagnostics call the
    hybrid pipeline `reference-raster+wgpu-resolve`; a full GPU rasterizer is not
    a v0.15.0 capability.
- Models are immutable, content-addressed sources, embedded or referenced by a
  verified relative path, like images and DNGs.
- Import is bounded and hostile-input safe: triangle, vertex, texture,
  material, node-depth and byte limits are checked before allocation. External
  buffers and textures resolve only inside the project root.
- 3D edits are ordinary mutations. Dry run and commit share one planner and
  failed edits leave the document byte for byte unchanged.
- Documents without 3D, and every earlier version, render exactly as before.
- Size budget: the owner approved a 60,000,000-byte absolute release-binary gate
  on 2026-10-14 after the default Windows build reached 42,787,328 bytes. The
  default and `gpu` builds still have their sizes recorded separately on every
  release target so growth remains visible.
  Local Windows x64 MSVC release measurements on 2026-10-15 are 42,794,496
  bytes without `gpu` and 46,976,000 bytes with `gpu` (a 4,181,504-byte delta).
  Hosted per-target sizes are recorded by the release workflow.

## Model

```json
{"kind": "scene3d", "id": "hero", "x": 0, "y": 0, "width": 800, "height": 600,
 "model": "sha256:…",
 "camera": {"kind": "perspective", "fov_deg": 35, "position": [0, 1, 4],
            "target": [0, 0.5, 0]},
 "physical_camera": {"focal_length_mm": 50, "sensor_mm": [36, 24],
                     "aperture_f": 4.0, "focus_distance": 4.0,
                     "exposure_ev": 0.0},
 "lights": [{"kind": "directional", "direction": [-1, -2, -1], "intensity": 3}],
 "environment": {"kind": "color", "value": "#ffffffff"},
 "quality": {"samples": 4, "shadows": true}}
```

- `scene3d` is a rectangle node whose pixels come from rendering the model.
- Materials follow glTF metallic-roughness PBR, with base color, normal,
  metallic-roughness, occlusion and emissive textures.
- Lights: directional, point, spot and a bounded image-based environment.
- Cameras: perspective and orthographic, with explicit units and handedness.
- Physical camera (optional, perspective only): focal length and sensor size
  derive the field of view, so a photographer's lens choice is the input rather
  than a bare angle; `aperture_f` and `focus_distance` give depth of field;
  `exposure_ev` scales scene light before tone mapping. Depth of field is a
  bounded, fixed-pattern multi-sample approximation (no ray tracing), is
  deterministic like the rest of the CPU path, and is off unless `aperture_f`
  is set. `fov_deg` and `physical_camera` are two spellings of one lens; setting
  both inconsistently is a validation error.
- A raster image (a photograph from the document) may be the environment
  backdrop and the source of image-based lighting, within the existing bounded
  environment policy.
- UVs are first-class data: each mesh primitive reports its UV sets, island
  count, bounds and overlap/out-of-range coverage. Imported UVs are preserved
  unchanged; Pentool generates UVs only by explicit, deterministic projection
  (below), never by silent rewrite.
- Transparency uses documented sorting; unsupported glTF extensions are listed in
  the import report instead of being dropped silently.

## Must ship, in order

- [x] **1. Freeze the 3D specification:** coordinate system, units, handedness,
  material model, supported glTF/OBJ features, limits, and the CPU/GPU parity
  policy above, in `docs/scene3d-v1.md` and a schema.
- [x] **2. Bounded importers:** GLB/glTF 2.0 and OBJ+MTL with limits, path
  containment, an import report listing unsupported features, and hostile-file
  tests (truncated, cyclic nodes, oversized accessors, traversal URIs).
- [x] **3. CPU rasterizer:** depth buffer, perspective-correct interpolation,
  backface policy, anti-aliasing, deterministic fixed-order arithmetic with no
  platform `libm` and no fused multiply-add.
- [x] **4. PBR shading and lights:** metallic-roughness BRDF, normal mapping,
  shadow maps, bounded environment lighting, sRGB/linear policy shared with the
  v0.12.0 working-space renderer.
- [x] **5. `scene3d` node:** validation, rendering into the shared scene
  pipeline, masks, effects, blend modes, transforms and export to PNG, SVG
  (as an embedded raster) and PDF.
- [x] **6. Agent commands:** place, move, orbit, frame-to-fit, light and material
  edits, and a compact `scene3d inspect` listing nodes, meshes, materials and
  bounds with stable IDs and pagination.
- [x] **6a. UV mapping CLI:** `scene3d uv` inspects UV sets and islands with
  stable IDs and pagination, reports stretch, overlap and out-of-range texels,
  adds or replaces a UV set by explicit projection (planar, box, cylindrical,
  spherical) with documented axes and units, applies a 2D transform (offset,
  scale, rotate) to a set, and exports the UV layout as PNG or SVG so artwork
  can be drawn against it. All edits go through the shared planner (dry run
  matches commit) and never change the source model: UV edits are an
  overlay record on the `scene3d` node keyed by model digest and mesh ID.
- [x] **6b. Physical camera and photographic render:** implement the optional
  `physical_camera` of the Model section (focal length and sensor to field of
  view, deterministic depth of field, exposure), `scene3d camera` flags for it,
  and a framing helper that fits a model to a chosen focal length. Golden
  tests cover the same scene at 24, 50 and 85 mm and with depth of field on
  and off.
- [x] **7. Optional `wgpu` backend boundary** behind the `gpu` feature with
  automatic fallback, a byte-parity test against the CPU reference, and a
  recorded size delta. v0.15.0 uses it for compute resolve and readback, not
  geometry rasterization, as specified in the product contract.
- [x] **8. Editor:** orbit/pan/zoom, light and material panels, import dialog,
  progressive preview, and a clear `accelerated` / `reference` indicator.
- [x] **9. Packaging, history and recovery:** models travel with packages and
  collected projects; undo/redo and crash recovery cover 3D edits.
- [x] **10. Security, conformance and performance:** hostile-file corpus,
  cross-platform golden renders, a triangle/texture budget benchmark, and the
  release-size check on every target.

## CLI direction

```sh
pentool scene3d add DOC model.glb --layer art --width 800 --height 600
pentool scene3d camera DOC hero --fov 35 --position 0,1,4 --target 0,0.5,0
pentool scene3d camera DOC hero --focal-length 50 --sensor 36x24 --aperture 4 --focus-distance 4
pentool scene3d light DOC hero add --kind directional --direction -1,-2,-1
pentool scene3d uv DOC hero show --limit 50
pentool scene3d uv DOC hero project --mesh body --kind box --set uv1
pentool scene3d uv DOC hero layout --mesh body --out body-uv.svg
pentool scene3d inspect DOC hero --limit 50
pentool export DOC out.png --renderer reference   # or accelerated
```

## Acceptance targets

- The reference-sphere, textured-cube and a PBR material grid render
  byte-identically on Linux, Windows and Apple Silicon macOS.
- A GPU render of the same scenes stays within the documented tolerance.
- A 100k-triangle model renders within a recorded time budget on the CPU path.
- `scene3d uv` reports the UV islands of the textured-cube fixture, a box
  projection of the reference sphere is reproducible byte for byte on every
  target, and the exported UV layout rasterizes identically on every target.
- The same scene at 24, 50 and 85 mm focal length changes only perspective, and
  depth of field never changes pixels at the focus distance.
- Every hostile model is refused with a named error and no mutation.

## Explicitly deferred

- Modeling, sculpting, automatic UV unwrapping of arbitrary meshes (seam
  detection and island packing), rigging and physics. Explicit projections and
  UV inspection in task 6a are in scope.
- Ray tracing and global illumination.
- Animation (out of scope for Pentool) and print-accurate color of 3D output beyond the existing
  v0.12.0 policy.
