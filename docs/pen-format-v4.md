# `.pen` format version 4: ordered scene graph

Version 4 replaces each layer's separate `paths` and `texts` arrays with one
ordered `nodes` array. Reading v1–v3 never upgrades a file. `pentool migrate`
performs the explicit conversion; `pentool migrate --target 3` flattens v4 for
older consumers.

Nodes render back-to-front in array order. A group owns an ordered `children`
array. Node IDs are unique within a page, including nested children, component
definitions, and instances. Maximum group depth is 64; reparenting must reject
cycles. Deleting a group deletes its descendants unless `ungroup` is explicitly
requested.

Every node has a local affine `transform` in SVG matrix order
`[a,b,c,d,e,f]`. Its world transform is the parent world transform multiplied by
its local transform. Bounds are the axis-aligned world-space bounds after this
composition, including stroke extents. Reordering changes only sibling z-order.
Clipping is an optional node reference evaluated in the clipped node's parent
coordinate space; a clip cannot reference itself or its descendants.

Supported node kinds are `group`, `rect`, `ellipse`, `line`, `path`, `text`, and
`instance`. A rectangle with positive `radius_x` or `radius_y` is a rounded
rectangle; equal ellipse radii describe a circle. Style-bearing nodes contain a
`style` object. Each property can contain a named `ref` and a literal `fallback`;
missing references render the fallback and remain diagnosable.

Component definitions are document-level ordered records containing a root group.
Instances identify a definition and store an offline-safe fallback group plus
property overrides. This extends the v0.5 identity/update contract rather than
requiring a source at render time.

Unknown fields at document, page, layer, node, style, component, and instance
levels must survive load/edit/save. Extensions must not alter rendering unless a
later format version adopts them.

Migration from v1–v3 preserves page/layer order. Each layer's paths become path
nodes followed by its text nodes, matching the legacy renderer's defined order.
Legacy IDs and transforms are retained. Flattening recursively applies transforms,
resolves styles to fallbacks, expands primitives/groups/instances to v3 paths and
texts, and fails rather than dropping unsupported clipping or text-box semantics.

The normative machine schema is [`pen-format-v4.schema.json`](pen-format-v4.schema.json)
and the minimal fixture is [`fixtures/v4-scene.pen`](fixtures/v4-scene.pen).
