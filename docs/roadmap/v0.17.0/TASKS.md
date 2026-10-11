# v0.17.0 — 3D objects: import, light, render

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
- Models are immutable, content-addressed sources, embedded or referenced by a
  verified relative path, like images and DNGs.
- Import is bounded and hostile-input safe: triangle, vertex, texture,
  material, node-depth and byte limits are checked before allocation. External
  buffers and textures resolve only inside the project root.
- 3D edits are ordinary mutations. Dry run and commit share one planner and
  failed edits leave the document byte for byte unchanged.
- Documents without 3D, and every earlier version, render exactly as before.
- Size budget: the CPU path adds under 2 MiB to the release binary; the `gpu`
  feature has its own recorded size and is excluded from the default gate only
  by an explicit, documented owner decision.

## Model

```json
{"kind": "scene3d", "id": "hero", "x": 0, "y": 0, "width": 800, "height": 600,
 "model": "sha256:…",
 "camera": {"kind": "perspective", "fov_deg": 35, "position": [0, 1, 4],
            "target": [0, 0.5, 0]},
 "lights": [{"kind": "directional", "direction": [-1, -2, -1], "intensity": 3}],
 "environment": {"kind": "color", "value": "#ffffffff"},
 "quality": {"samples": 4, "shadows": true}}
```

- `scene3d` is a rectangle node whose pixels come from rendering the model.
- Materials follow glTF metallic-roughness PBR, with base color, normal,
  metallic-roughness, occlusion and emissive textures.
- Lights: directional, point, spot and a bounded image-based environment.
- Cameras: perspective and orthographic, with explicit units and handedness.
- Transparency uses documented sorting; unsupported glTF extensions are listed in
  the import report instead of being dropped silently.

## Must ship, in order

- [ ] **1. Freeze the 3D specification:** coordinate system, units, handedness,
  material model, supported glTF/OBJ features, limits, and the CPU/GPU parity
  policy above, in `docs/scene3d-v1.md` and a schema.
- [ ] **2. Bounded importers:** GLB/glTF 2.0 and OBJ+MTL with limits, path
  containment, an import report listing unsupported features, and hostile-file
  tests (truncated, cyclic nodes, oversized accessors, traversal URIs).
- [ ] **3. CPU rasterizer:** depth buffer, perspective-correct interpolation,
  backface policy, anti-aliasing, deterministic fixed-order arithmetic with no
  platform `libm` and no fused multiply-add.
- [ ] **4. PBR shading and lights:** metallic-roughness BRDF, normal mapping,
  shadow maps, bounded environment lighting, sRGB/linear policy shared with the
  v0.12.0 working-space renderer.
- [ ] **5. `scene3d` node:** validation, rendering into the shared scene
  pipeline, masks, effects, blend modes, transforms and export to PNG, SVG
  (as an embedded raster) and PDF.
- [ ] **6. Agent commands:** place, move, orbit, frame-to-fit, light and material
  edits, and a compact `scene3d inspect` listing nodes, meshes, materials and
  bounds with stable IDs and pagination.
- [ ] **7. Optional `wgpu` accelerator** behind the `gpu` feature with automatic
  fallback, a parity test against the CPU reference, and a recorded size delta.
- [ ] **8. Editor:** orbit/pan/zoom, light and material panels, import dialog,
  progressive preview, and a clear `accelerated` / `reference` indicator.
- [ ] **9. Packaging, history and recovery:** models travel with packages and
  collected projects; undo/redo and crash recovery cover 3D edits.
- [ ] **10. Security, conformance and performance:** hostile-file corpus,
  cross-platform golden renders, a triangle/texture budget benchmark, and the
  release-size check on every target.

## CLI direction

```sh
pentool scene3d add DOC model.glb --layer art --width 800 --height 600
pentool scene3d camera DOC hero --fov 35 --position 0,1,4 --target 0,0.5,0
pentool scene3d light DOC hero add --kind directional --direction -1,-2,-1
pentool scene3d inspect DOC hero --limit 50
pentool export DOC out.png --renderer reference   # or accelerated
```

## Acceptance targets

- The reference-sphere, textured-cube and a PBR material grid render
  byte-identically on Linux, Windows and Apple Silicon macOS.
- A GPU render of the same scenes stays within the documented tolerance.
- A 100k-triangle model renders within a recorded time budget on the CPU path.
- Every hostile model is refused with a named error and no mutation.

## Explicitly deferred

- Modeling, sculpting, UV unwrapping, rigging and physics.
- Ray tracing and global illumination.
- Animation (v0.19.0) and print-accurate color of 3D output beyond the existing
  v0.12.0 policy.
