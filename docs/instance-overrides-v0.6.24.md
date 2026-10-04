# Instance overrides and trustworthy updates (v0.6.24)

This document defines the v0.6.24 instance-update contract. It is normative for
newly created instance records; older records without a stored materialization
hash are treated as unverifiable and must not be updated silently.

## Instance record

An attached instance records:

- stable instance, asset, page, and materialized layer IDs;
- source asset version and content hash;
- placement transform and visibility;
- `base_layers`, the exact accepted materialization of the source revision before
  per-instance overrides;
- `materialized_hash`, the hash of the accepted materialized layers after declared
  overrides;
- override values keyed by stable exposed-property name;
- the accepted exposed-property definitions, so reusing a key for a different
  source object or property path is detected as a semantic retarget;
- bounded rollback history containing the preceding layers, positions, hashes,
  source identity, and base snapshot.

The current materialized layers are hashed before every update plan. A mismatch
with `materialized_hash` is an `untracked-local-edit` conflict. Legacy records that
have no accepted hash are also unverifiable and therefore conflict rather than
claiming that the local state is clean.

## Exposed-property schema

Asset metadata stores exposed properties under `asset.properties`:

```json
{
  "label": {
    "type": "text",
    "label": "Card label",
    "default": "Ready",
    "targets": [
      { "object": "label", "property": "content" }
    ],
    "constraints": { "max_length": 80 }
  }
}
```

Property names are stable public identifiers. Renaming or removing one is a
breaking asset change unless the update supplies an explicit mapping.

Each target contains a stable source object ID and property path. Initial supported
paths are:

- `content` for text;
- `fill` and `stroke` colors;
- `visible` as a boolean;
- `stroke_width`, `opacity`, `width`, and `height` as finite numbers;
- `style_ref` as a design-token reference.

Definitions may include a human label, default value, and constraints appropriate
to their type. An update is compatible only when the property still exists, every
target path remains supported, and the override value can be represented by the
target type. Asset packages carry this metadata as ordinary deterministic JSON.

## Planning and conflicts

Update planning compares:

1. `base_layers` and the recorded source hash;
2. current materialized layers and declared overrides;
3. the incoming verified asset and its exposed-property definitions.

Plans report source versions/hashes, accepted and current materialization hashes,
whether local content changed, retained overrides, and structured conflicts.
Supported conflict resolution names are `keep-local`, `take-source`, `detach`, and
`map-target`; no resolution is selected implicitly.

Compatible overrides are reapplied after the incoming source is materialized and
before the transaction commits. A removed property, missing target, incompatible
type, retargeted stable key, or undeclared local edit blocks the update. Dry runs
and commits use the same plan and neither path mutates the document on conflict.

## Mutation protection

Generic editing commands cannot mutate attached instance internals. The shared
transaction boundary compares every materialized instance layer before committing
an ordinary edit and rejects differences with guidance to use an exposed property
or detach the instance. This covers object, text, path, shape, group, layer, batch,
and future commands that use the transaction engine. Existing documents with
earlier untracked changes retain a second update-time materialization-hash guard:
an undeclared difference blocks replacement instead of being overwritten.
