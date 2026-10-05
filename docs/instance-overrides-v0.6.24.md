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

## CLI authoring

Create an asset and expose common properties without editing JSON:

```sh
pentool asset create chrome.pen --id ai/chrome --rect content \
  --property "counter=text:counter-label.content" \
  --property "active=visibility:active-segment.visible"
pentool asset property add chrome.pen ai/chrome accent \
  --target frame --field fill --default "#2563eb" --label "Accent"
pentool asset property list chrome.pen ai/chrome --json
pentool asset property inspect chrome.pen ai/chrome accent
pentool asset property usage chrome.pen ai/chrome accent
pentool asset property validate chrome.pen ai/chrome
```

`set`, `rename`, and `remove` use the same asset/property position and support
`--dry-run` plus `--if-revision`. Complete definitions can be supplied with
`--schema FILE`. Invalid targets include a nearest-ID suggestion.

## Bulk update and resolution

```sh
pentool instance update deck.pen --all --asset ai/chrome --source chrome-1.3.pen --dry-run
pentool instance update deck.pen --all --asset ai/chrome --source chrome-1.3.pen --to 1.3.0
pentool --page page-3 instance update deck.pen --all --asset ai/chrome --source chrome-1.3.pen
pentool instance update deck.pen card-3 --source chrome-1.3.pen --resolutions fixes.json
```

Bulk filters cover asset, page, parent group, current version/hash, package, and
stale-only selection. The default is all-or-nothing. `--continue-on-conflict`
updates only clean instances, lists every skipped instance, and still produces
one hidden-history entry. A resolution file is an array (or an object containing
`resolutions`) with `instance`, `action`, and optional `property`, `target`, and
`field` keys. Supported actions are `keep-local`, `take-source`, `detach`, and
`map-target`.

Plans report the preserved page, parent/index metadata, source transition,
object classifications, override outcomes, local edits, conflicts, available
resolutions, affected pages, and transaction result. Dry-run and commit share the
same planner snapshot.

## Bounds, IDs, history, and recovery

`asset create` infers tight visible bounds, including strokes, transforms, text,
primitives, and nested groups. Hidden content is excluded unless
`--include-hidden` is passed. `--canvas-bounds` is the only way to request the
whole canvas; empty content requires an explicit `--rect`.

Materialized child IDs use `<instance>/<source-id>` semantics with canonical
escaping and collision suffixes only when necessary. `tree` and `search` expose
the source and materialized IDs together.

Place, set, detach, update, bulk update, and rollback use `.pentool/history`.
One command creates at most one history entry and no adjacent `.bak.N` files.
Use `pentool history FILE`, `pentool undo FILE`, and `pentool redo FILE` for
recovery. A bulk update can be undone byte-for-byte.
