# Pentool v0.15.0

Pentool 0.15.0 adds static 3D import, arrangement, lighting and rendering to
editable `.pen` documents. It remains an offline, single-binary application and
keeps the deterministic CPU renderer normative.

## Highlights

- Import bounded GLB/glTF 2.0 and OBJ/MTL models with content-addressed assets,
  containment checks and explicit unsupported-feature reports.
- Place `scene3d` nodes in the ordinary scene graph and export them through PNG,
  SVG and PDF composition.
- Configure perspective, orthographic and physical cameras, deterministic depth
  of field, exposure, directional/point/spot lights, shadows, PBR materials and
  bounded environments.
- Inspect and edit scenes from the CLI, including framing, camera orbit, lights,
  material overrides and deterministic UV inspection/projection/layout export.
- Edit 3D scenes in Studio with import, camera, light and material controls plus
  progressive preview.
- Package and collect model sidecars with digest verification, history and
  recovery coverage.
- Optionally compile `wgpu` support with `--features gpu`. In this release the
  GPU path provides adapter selection, compute resolve, byte-identical readback
  and explicit fallback; geometry and shading still use the CPU reference
  rasterizer. On Windows x64 MSVC, the local release binaries measured
  42,794,496 bytes by default and 46,976,000 bytes with `gpu` enabled.

## Compatibility and limits

- Document format version 10 adds `scene3d` nodes and the top-level `models`
  table. Earlier document versions remain readable and render unchanged.
- Pentool is not a modeler: sculpting, arbitrary automatic UV unwrapping,
  rigging, physics, ray tracing and animation are outside this release.
- CPU output is the only source of goldens, digests and conformance claims.

See `docs/scene3d-v1.md` for the frozen format and rendering contract.
