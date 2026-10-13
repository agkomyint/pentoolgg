# Static 3D (scene3d v1, document version 10)

A `scene3d` node is a rectangle whose pixels come from rendering an imported
model with Pentool's deterministic CPU rasterizer. The render is a normal
raster layer: it composes, exports (PNG, SVG with an embedded raster, PDF) and
packages like any other node. Schema: `pen-format-v10.schema.json`.

Version 10 is version 9 plus the `scene3d` node kind and the top-level
`models` map. `pentool migrate --target 10` adds it; `--target 9` is refused
while any scene3d node or model exists, naming the first one.

## Space and units

Right-handed, Y up, meters (glTF convention). The camera looks down its local
-Z. `azimuth` orbits about Y (degrees), `elevation` tilts above the horizon.
UV origin is the texture's top-left corner (glTF); `v` grows downward in
exported layouts.

## Models

`models` maps `sha256:<hex>` of the **source file bytes** to a record:

```json
{"format": "glb|gltf|obj",
 "storage": {"kind": "embedded", "data": "<base64>"}
          | {"kind": "external", "path": "assets/model.gltf"},
 "uv": {"*": {"projection": "box", "scale": [2, 2]}}}
```

The digest is verified on every load. External paths are document-relative,
must stay inside the document directory, and sidecar buffers/textures resolve
under the same containment. Embedded models cannot reference other files; use
`--external` for `.gltf`/`.obj` with sidecars.

### Importer scope and limits

glTF 2.0 / GLB: triangle lists, strips and fans (points and lines are reported,
not drawn); PBR metallic-roughness, base color / metallic-roughness / normal /
occlusion / emissive textures; node hierarchies with TRS or matrices; sparse
accessors and `extensionsRequired` are refused. OBJ: `v vt vn f o g usemtl
mtllib`, negative indices, polygon fans; MTL `Kd Ke d Ns map_Kd`.

Limits (`LIMITS`): 256 MiB file, 2M vertices, 1M triangles, 4096 primitives,
1024 materials, 64 textures, 20k nodes, depth 32, 20k instances, 256 MiB of
decoded texture. Anything over a limit fails with `[limit-exceeded]` and no
document mutation. No network access ever happens.

## Node

| field | meaning |
| --- | --- |
| `model` | digest key into `models` |
| `x y width height` | layout rectangle (render edge ≤ 4096 px after `pixel_ratio`) |
| `camera` | `azimuth` `elevation` `fov` (vertical, degrees) `projection` `height` (orthographic) or explicit `eye`/`target`/`up`/`near`/`far`; omitted = frame the model |
| `physical_camera` | `focal_length_mm`, `sensor_mm` [w,h] (default 36x24), `aperture_f`, `focus_distance`, `exposure_ev`, `samples` |
| `lights` | up to 16 of `directional`, `point`, `spot`; default is a two-light key/fill rig |
| `environment` | `sky`, `ground`, `intensity` gradient lighting and reflections |
| `background` | `#rrggbb[aa]` or `none` (transparent) |
| `exposure_ev` | scene exposure, stops |
| `supersample` | 1–4 (default 2) |
| `shadows`, `shadow_map_size` | shadow maps on (default) and 64–2048 texels |

### Physical camera

`focal_length_mm` and the sensor height give the vertical field of view
(`2·atan(h / 2f)`); the model is re-framed for the chosen lens, so changing
24 → 50 → 85 mm changes perspective only. Setting `camera.fov` and a
physical camera that disagree by more than 0.01° is a validation error, and a
physical camera on an orthographic camera is refused.

Depth of field is on only when `aperture_f` is set. Aperture diameter is
`focal_length / f-number` in world meters. The renderer takes `samples`
(default 16, max 64) positions on a fixed golden-angle disc, all aimed at the
same point on the focus plane, and averages the renders in premultiplied
space; points at `focus_distance` stay sharp. Total work is bounded
(`samples × pixels × supersample² ≤ 256 Mi`).

### Shadows

Directional lights use an orthographic map fitted to the scene, spot lights a
perspective map over the cone, point lights a six-face cube. Lookups use a
fixed 3x3 percentage-closer filter with a texel-scaled normal offset. Blended
materials do not cast shadows. All maps together are limited to 16 Mi texels.

## UV overlays

`pentool scene3d DOC NODE uv show` reports, per primitive, the UV sets, UV
bounds, vertices outside 0–1, degenerate triangles and unit-square coverage.
`uv project` records an overlay under `models[digest].uv[primitive|*]`;
the importer output is re-projected on every load, so the source bytes and
digest never change. Planar maps along `--axis` (default z); box picks the
dominant axis per triangle; cylindrical and spherical wrap around `--axis`
(default y) from the bounds center, with seam triangles lifted to avoid
smearing. Non-planar projections duplicate vertices (≤ 3M). The transform is
rotate about (0.5, 0.5), then scale, then offset. `uv layout --out
file.svg|png` writes the wireframe.

## Determinism

The CPU path is normative: no platform math library (own sin/cos/atan/exp/ln),
fixed traversal order, fixed sample patterns. Renders are cached by a hash of
every pixel-affecting field plus the model digest and UV overlay.

## CLI

```sh
pentool scene3d DOC add hero --model hero.glb --layer art --width 800 --height 600
pentool scene3d DOC camera hero --azimuth 35 --elevation 20
pentool scene3d DOC camera hero --focal-length 85 --sensor 36x24 --aperture 2 --focus-distance 3
pentool scene3d DOC light hero add --kind spot --position 0,4,0 --direction 0,-1,0 --outer-deg 35
pentool scene3d DOC set hero --background none --shadow-map-size 2048
pentool scene3d DOC uv hero project --projection box --scale 2,2
pentool scene3d DOC uv hero layout --primitive 0 --out hero-uv.svg
pentool scene3d DOC inspect hero
```

Every edit is a transaction: `--dry-run` plans, `--if-revision` guards, a
failure leaves the file byte-identical.
