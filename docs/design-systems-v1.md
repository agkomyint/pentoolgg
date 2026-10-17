# Design systems, brand kits and templates (v1)

Status: normative specification for roadmap v0.19.0 (`docs/roadmap/v0.19.0/TASKS.md`).
This document defines contracts; it does not claim that any command below has
shipped. The roadmap checkboxes remain the completion record. Where this spec and
released behavior disagree, released behavior and `docs/pen-format-v4.schema.json`
win until this spec is deliberately amended.

Key words MUST / SHOULD / MAY are used in the RFC 2119 sense.

## 0. Existing foundations this spec extends

| Concern | Existing code or doc | Reused as |
| --- | --- | --- |
| Named document tokens | `src/style.rs`, `src/styles.rs` (layout styles are a different store, `layout.styles`), document key `styles` | the **document token store**: `styles.<name> = {"type","value"}`; types `color stroke typography spacing radius shadow`; alias form `value: {"ref": other}`; alias cycles rejected by `style::validate_aliases` |
| Token reference | node `style.<prop> = {"ref","fallback"}` (`docs/pen-format-v4.md`); `scene::with_resolved_tokens` copies the token value into `fallback` for every render path | the **only** reference syntax; this spec adds none |
| Token CLI | `pentool style FILE list/set/usage/rename/delete/detach/replace` (`StyleArgs` in `src/main.rs`); batch op `set-style` (`src/scene.rs`) | token mutation surface, same transaction path |
| Component updates and overrides | `docs/instance-overrides-v0.6.24.md`, `src/instance.rs` (`update_plan`, `update_resolved`, `update_bulk`, three-way base/current/incoming, `keep-local` `take-source` `detach` `map-target`), instance override `style_ref` target in `src/asset.rs` | the **override and conflict model** for token updates (section 4) |
| Packages | `src/package.rs`: `.penpkg` zip, `pentool.package.json` (`schema: 1`, `assets{path,hash}`), `sha256:` hashes via `asset::hash_bytes`, ed25519 `sign`/`verify_signature`, `pentool.lock` (`name, version, source, package_hash`), `lock verify`, `lock sync --offline`, install under `.pentool/packages/<name>/<version>` | the **library container** (section 5) |
| Asset IDs | `asset::validate_id` (slash-separated segments of `[A-Za-z0-9._-]`, 200 bytes max) | package and library names |
| Print color | `src/color/spot.rs` (`Ink`, spot library, `package_table`, `validate_package_table`), `src/color/catalog.rs` (ICC catalog `color_management.profiles`, `sha256:` identities, redistribution policy), `docs/color-value-v1.schema.json`, `docs/professional-color-v1.md` | **ink and profile references** in brand kits (section 6) |
| Batch planner | `scene::apply_batch_at` (dry run, in-memory candidate, all-or-nothing, `$alias` IDs), shared transaction and `.pentool/history` | all mutation, template fill and audit fixes |
| Extensions | `docs/extensions-v1.md`, `src/extension/` (`document.search`, `document.plan`, `document.apply`) | later scripted audits: the audit result and fix plan are plain batch JSON (section 9) |

Current format: `scene::LATEST_VERSION` is 12 (`docs/pen-format-v12.md`). Unknown
fields at document, page, layer, node, style, component and instance level survive
load/edit/save (`docs/pen-format-v4.md`).

## 1. Token model

### 1.1 Types and value schemas

The canonical type names are the existing ones. The token-file alias `type`
(roadmap wording) is accepted on import and means `typography`.

| Type | `value` | Rules |
| --- | --- | --- |
| `color` | a color string (`none`, `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`; validated by `scene::is_valid_color`), **or** a native color object per `docs/color-value-v1.schema.json` (`space`, `channels`, optional `profile`, `alpha`; spot form references an ink ID) | native objects require v8+ color support in the target document (section 8); otherwise `unsupported-capability` |
| `typography` | object: `font` (string, required), `size` (number, > 0, <= 10000), optional `weight` (integer 1-1000), `italic` (bool), `leading` (number > 0), `tracking` (number), `axes` (object of finite numbers) | keys are a subset of the character/paragraph style keys in `src/styles.rs`; unknown keys are rejected in library tokens |
| `spacing` | finite number, 0 to 1,000,000 (document units) | |
| `radius` | finite number >= 0, or `[rx, ry]` of two such numbers | |
| `shadow` | array (1-8) of `{dx, dy, blur (>= 0), spread, color}`; `color` is a `color` value; numbers finite, bounded +-100,000 | |
| `stroke` | existing document type, kept for compatibility; allowed in documents, not in library v1 | |

Any token value MAY instead be an alias `{"ref": "<other token>"}` of the **same
type**. A cross-type alias is rejected (`token-type-mismatch`).

### 1.2 Naming and IDs

A **token ID** is the key in `styles`. For tokens that originate in a library or a
token file the ID MUST match:

```
^[a-z][a-z0-9-]*(\.[a-z0-9][a-z0-9-]*){0,7}$     length 1..=128
```

The first segment SHOULD equal the type group (`color.accent`, `type.heading.1`,
`space.md`, `radius.card`, `shadow.raised`). Group/type agreement is lint, not
validation. IDs are stable: renaming is an explicit operation (`style rename`,
which already rewrites references) and in a library a rename is a breaking change
(section 1.3). Legacy documents may contain IDs outside this grammar; they remain
valid and usable but cannot be exported to a library or token file until renamed
(`invalid-token-id` names the offender and the nearest valid form).

### 1.3 Versions

- A **token** has no version of its own. Its identity for change detection is the
  **value hash** `sha256:` + hex of canonical JSON (keys sorted, no insignificant
  whitespace, numbers as emitted by `serde_json`) of `{"type","value"}`.
- A **library** has a semantic version (`MAJOR.MINOR.PATCH`) equal to its package
  version. Rules the publisher tool MUST enforce when packing against a previous
  version (`--previous FILE`, optional): removing or renaming a token, changing
  its type, or narrowing a typography/shadow shape requires a MAJOR bump; adding a
  token or changing a value requires at least MINOR; metadata-only changes PATCH.
  Violations are a pack error, not a warning, when `--previous` is supplied.

## 2. Where tokens live and how documents reference them

1. **Document store (authoritative at render time).** The existing top-level
   `styles` object. A document is always self-sufficient: rendering, validation
   and export never read a library, the network, or `.pentool/`. This keeps the
   offline-safe property of instances (`pen-format-v4.md`).
2. **Library (source of truth for shared values).** A package asset whose
   document contains a `design_library` block (section 5). Installing a library
   into a document **vendors** the chosen tokens into `styles` with provenance.
3. **Reference syntax** is unchanged: `style.<property> = {"ref": "color.accent",
   "fallback": <literal>}`. `ref` names a key of the document's own `styles`. There
   is no `library:` URI scheme, no cross-document reference, and no network lookup.

### 2.1 Additive token record keys

A vendored token carries optional keys (ignored by older readers per the unknown
field rule):

```json
"color.accent": {
  "type": "color",
  "value": "#F59E0B",
  "source": {
    "library": "acme/brand", "version": "1.2.0",
    "token": "color.accent",
    "hash": "sha256:...",          // value hash accepted from the library (the merge base)
    "package_hash": "sha256:..."   // package the base came from
  }
}
```

`source.hash` is the three-way **base**. A token without `source` is local.
`source` is written only by `design library install/update`; `style set` on a
sourced token keeps `source` untouched, so the current value hash differing from
`source.hash` **is** the local override marker (section 4). Recorded `source`
MUST NOT contain filesystem paths.

### 2.2 Resolution order and fallback (normative)

Resolving `ref: R` on a node property, for render, tree, search, audit and export:

1. Look up `styles[R]` in the same document. Absent: **unresolved**.
2. If its `value` is an alias `{ref: R2}`, repeat from step 1 with `R2`, maximum
   alias depth **8** (existing code has no depth cap; this spec adds one).
3. The first non-alias value is the resolved value. It MUST pass the value schema
   of the token's declared type, otherwise **invalid**.
4. Unresolved or invalid: use the literal `fallback` of the reference. Render
   continues; the condition is reported by `validate --strict`/`audit` as
   `unresolved-token` or `invalid-token`. A missing fallback with an unresolved
   token is a validation error (the schema already requires `fallback`).
5. Never consult anything outside the document. Resolution is a pure function of
   the document bytes, so it is deterministic across platforms.

`scene::with_resolved_tokens` is the single implementation point; it MUST be
extended (not duplicated) to apply steps 2-4, and the tree/search `style_refs`
output gains `resolved` per ref only on request (`--resolve`), to keep default
output compact.

### 2.3 Cycles, depth, count

- Alias cycles are rejected at validation and at every mutation, before write:
  error `token-cycle`, shape in section 2.4. This strengthens the existing
  `style alias cycle` error without removing it (message text prefix is kept).
- Limits: alias depth 8; tokens per document 5,000 (`styles` today is unbounded);
  tokens per library 5,000; token ID 128 bytes; any single token value 16 KiB
  canonical JSON; token source records 5,000. Exceeding a limit is
  `limit-exceeded` naming the limit, the observed count and the corrective action.
  Existing documents over the new document limit open read-only for tokens: they
  validate, but `design`/`style set` refuse to add more.

### 2.4 Error shape

Every `design` command failure, and every validation failure introduced here, is
`{"error": {"code", "message", "operation", "page", "token", "path", "action"}}`
(absent fields omitted), with codes `token-cycle`, `token-type-mismatch`,
`invalid-token-id`, `invalid-token-value`, `unknown-token`, `limit-exceeded`,
`unsupported-capability`, `conflict`, `hash-mismatch`, `unsafe-path`,
`invalid-input`. Cycle example:

```json
{"error":{"code":"token-cycle","operation":"design token set","token":"color.a",
 "path":["color.a","color.b","color.a"],
 "message":"token alias cycle: color.a -> color.b -> color.a",
 "action":"point one of the aliases at a literal value"}}
```

## 3. Mutation rules

Every token mutation (`style set/rename/delete/detach/replace`, `design ...`) is
planned in memory on a clone, validated (`scene::validate`, alias cycle, limits),
and committed by the shared transaction with `--dry-run` and `--if-revision`,
producing one hidden-history entry. A failed command leaves the file
byte-for-byte unchanged. Delete of a referenced token is refused (existing
behavior). Locked layers/nodes are respected whenever a plan rebinds node styles.

## 4. Override semantics and update planning

A vendored token is in exactly one state, computed per token from three values:
**base** (`source.hash`), **current** (hash of the document's value), **incoming**
(hash of the new library value):

| base vs current | incoming vs base | Outcome (`status`) |
| --- | --- | --- |
| current == base | incoming == base | `unchanged` |
| current == base | incoming != base | `changed` (value is replaced) |
| current != base | incoming == base | `kept-override` (local value preserved; `source.hash` unchanged) |
| current != base | incoming != base, incoming == current | `converged` (`source.hash` advances, value untouched) |
| current != base | incoming != base, incoming != current | `conflict` |
| token absent in library | | `removed` (see below) |
| token absent in document | token in library | `added` |
| type differs, or incoming type incompatible with existing node bindings | | `conflict` (`retargeted`) |

- `removed` and any `conflict` **block the commit** unless a resolution is given;
  resolutions reuse the v0.6.24 vocabulary: `keep-local` (keep the value, advance
  `source.hash` so it stays an override), `take-source` (adopt incoming),
  `detach` (drop `source`, token becomes local), and `map-target` (for a removed
  token, `to` names the replacement token; node refs are rewritten as `style
  replace` does). No resolution is chosen implicitly. A resolutions file is an
  array of `{"token","action","to"?}`. `--continue-on-conflict` updates only
  clean tokens, lists every skipped one, and still produces one history entry
  (v0.6.24 bulk semantics).
- Aliases follow the same table per token. A changed token's **dependents** are
  nodes whose resolved chain includes it (direct refs and aliases of it) and the
  tokens aliasing it. Nothing else is touched: the planner computes the set of
  tokens to write and the commit writes exactly that set. Node style properties,
  geometry, overrides and unknown fields are never rewritten by a library update
  (only `map-target` rewrites node `ref` strings, and only for that token's
  dependents).
- Instance overrides (`style_ref`) are untouched; a removed token that an
  override still names is a `conflict` with `dependents` listing instances.
- Dry run and commit call the same `plan_update(document, library_snapshot,
  resolutions)`; the commit applies the returned `writes`. A plan is a pure
  function of document bytes + verified library bytes + resolutions.

### 4.1 Plan JSON (`design library update --dry-run`, also the committed report)

```json
{
 "library":"acme/brand","from":"1.1.0","to":"1.2.0",
 "package_hash":"sha256:...","dry_run":true,"would_change":true,
 "counts":{"added":1,"removed":0,"changed":2,"kept-override":1,"converged":0,"conflict":0,"unchanged":40},
 "returned":4,"offset":0,"limit":200,"has_more":false,
 "tokens":[
  {"token":"color.accent","status":"changed","type":"color",
   "base":"sha256:..","current":"sha256:..","incoming":"sha256:..",
   "from":"#F59E0B","to":"#D97706",
   "dependents":{"nodes":["p1/hero-bg"],"tokens":["color.cta"],"count":2}}
 ],
 "conflicts":[],"resolutions_needed":[],
 "writes":["color.accent"],
 "change":{"revision_before":"..","revision_after":null}
}
```

`tokens` is sorted by token ID (bytewise), `unchanged` entries omitted unless
`--all`; pagination fields describe exactly the emitted `tokens`; `limit` default
100, max 1000; `dependents.nodes` lists at most 20 IDs (sorted `page/id`) with the
true `count`.

## 5. Library packages

A library is an ordinary `.penpkg` (`package.rs`, `PackageManifest` schema 1)
containing one asset document (`assets/<name>.pen`, hashed by the existing
`PackageAsset.hash`). The asset document is a minimal v4 scene whose top level is:

```json
{"version":4,"pages":[...minimal...],
 "design_library":{"schema":1,"name":"acme/brand","version":"1.2.0",
   "description":"","license":"",
   "tokens":{"color.accent":{"type":"color","value":"#D97706","description":"CTA"}},
   "brand":{...section 6...},"templates":{...section 7...}}}
```

This needs **no manifest change and no package schema bump**: `design_library`
rides in the asset document (unknown top-level fields survive), `name`/`version`
MUST equal the manifest `name`/`version` (checked at pack and verify), and the
package already provides content addressing, signatures and dependencies.

- **Content addressing.** Identity is `package_hash` (`sha256:` of the `.penpkg`
  bytes, as in `pentool.lock`) and the per-asset hash. Token value hashes
  (section 1.3) give per-token identity. `pack` is deterministic (same input,
  same bytes), as for `.penpkg` today.
- **Offline verification.** `design library verify FILE` = `package verify` plus:
  `design_library` schema, name/version agreement, every token valid (types,
  IDs, alias closure with no cycle, limits), brand/template blocks valid. Never
  touches the network. Signatures: `package sign`/`verify-signature` unchanged;
  an update may require `--require-signature --public-key KEY`.
- **Install.** `design library install PKG DOC.pen [--tokens GLOB...] [--prefix P]`
  verifies the package, installs it with `package::install` (project
  `.pentool/packages`, `pentool.lock` entry as today), then vendors tokens into
  the document in one transaction (all tokens by default; selected tokens pull
  their alias closure). `--prefix` is rejected in v1 (open question Q3); an ID
  collision with an existing *local* token is `conflict` (no silent overwrite).
- **Lock integration.** `pentool.lock` is unchanged. `lock verify` and `lock sync
  --offline` therefore already cover libraries. A document records the library in
  `document.design.libraries = {"acme/brand":{"version","package_hash"}}`
  (additive, informational, unknown to older readers). `design library status
  DOC` compares this record, `pentool.lock`, and installed packages and reports
  `ok|stale|missing|hash-mismatch` per library, offline.
- **Update.** `design library update DOC --library acme/brand --to 1.2.0` (or
  `--package FILE`) resolves a verified, locked package only; it never downloads
  unless the caller already passes the existing registry flags of `package
  install`. It produces the plan of section 4.1; without `--dry-run` it commits
  and rewrites the `design.libraries` record and the lock entry together in one
  history entry.
- **Diff report.** `design library diff A B` (two packages or one package vs a
  document) compares two `design_library` blocks with the same status vocabulary
  (`added removed changed`, plus `type-changed`) and the semver verdict
  (`major|minor|patch|none` required vs `declared`); sorted, paginated as 4.1.

## 6. Brand kit

`design_library.brand` (also valid in a plain document at `design.brand`):

```json
{"colors":{"primary":{"token":"color.accent","usage":"CTAs, links","forbidden_on":["color.dark"]}},
 "type":{"heading":{"token":"type.heading.1"}},
 "logos":[{"id":"logo/primary","asset":"logo-primary","variants":["full","mark"],
           "min_width":24,"clear_space":0.5,"forbidden_backgrounds":["color.accent"]}],
 "inks":{"spot":[{"id":"ink/pantone-1655","token":"color.accent"}],
         "profiles":[{"role":"output","profile":"sha256:...","redistribution":"allowed"}]},
 "rules":[{"id":"min-contrast","kind":"contrast","min":4.5}]}
```

- Brand entries **refer to tokens by ID**; colors are stated once as tokens. Spot
  colors are `color` tokens whose value is a native spot object naming an ink ID.
- Spot inks reuse `color::spot::Ink` records (id, name, alias, alternate,
  provenance, redistribution), carried in the library like `PackageAsset.spot_inks`
  today and checked with `spot::validate_package_table`.
- ICC profiles reuse the v0.12.0 catalog identity (`sha256:` over original
  bytes). A library declares them; redistribution `restricted`/`unknown` blocks
  packing unless `--allow-restricted-local` is passed, matching existing package
  policy. Installing a brand kit into a document uses `color::catalog` import so
  the profile appears in `color_management.profiles` with unchanged provenance.
  Native color tokens require the document to be at the color-management
  version; otherwise the install fails `unsupported-capability`, never silently
  degrading to the alternate sRGB.
- Logos are asset references (`asset::validate_id`), not embedded bytes in the
  brand block.
- **Usage rules** are declarative data consumed by `audit` (section 9). v1 rule
  kinds: `contrast` (WCAG ratio between a text color token and its page
  background), `forbidden_pairs` (token pairs that must not be used as fill on
  stroke/text), `logo` (min_width, clear_space, forbidden_backgrounds), `fonts`
  (allowed typography tokens only). Rules are advisory: they never block rendering
  or writes; only `audit --fail-on` turns findings into an exit status.

### 6.1 Implemented behavior (`pentool design brand`)

Implemented in `src/design/brand.rs`; no format bump.

- **Validation** (`brand::validate_block`, run by `design library pack`/`verify`,
  `brand apply` and `brand show`): unknown keys are rejected at every level. Entry
  names are `[a-z0-9_-]{1,64}`; at most 256 colors/type entries/spot inks and 64
  logos, profiles and rules; `usage` is at most 512 bytes. `colors.*.token` and
  `forbidden_on`, logo `forbidden_backgrounds`, `inks.spot[].token`, and
  `contrast`/`forbidden_pairs` tokens must exist and be `color`; `type.*.token` and
  `fonts.allowed` must be `typography` (`unknown-token`, `token-type-mismatch`,
  otherwise `invalid-brand`, each with the JSON-pointer `path`). Logo `id` and
  `asset` use `asset::validate_id`; `min_width > 0`, `clear_space >= 0`. Profiles
  need `role` (`output|proof|display|working`), a `sha256:<64 hex>` identity and
  `redistribution` (`allowed|restricted|unknown`). Rule kinds are `contrast`
  (`min` 1..21), `forbidden_pairs` (`pairs`), `logo`, `fonts` (`allowed`).
- **`design brand show DOC`** prints `present`, `counts`, colors and type with the
  resolved token value/status, `logos`, `inks.spot` (resolved) and
  `inks.profiles` (each with `state` `present|missing` against the document's
  `color_management.profiles`), `rules`, and `libraries`. A document without a
  brand prints `present:false`. Not paginated: every section is bounded above.
- **`design brand apply LIB DOC [--dry-run] [--if-revision] [--project]`**
  verifies the package, vendors exactly the tokens the kit references (plus alias
  closure, with the section 4 conflict/override planning and report), sets
  `design.brand` and marks `design.libraries.<name>.brand = true`, in one
  transaction with the lock entry. A pre-existing `design.brand` that differs and
  was not applied from this library is `conflict` and nothing changes. A later
  `library update` preserves the `brand` marker (and the selection of tokens).
- **`design library pack --allow-restricted-local`**: without it, a brand kit
  with a `restricted`/`unknown` profile fails `restricted-profile`.
- **Deviations from section 6.** ICC profile and spot `Ink` records are
  *references only*: `brand apply` does not import profile bytes through
  `color::catalog` or copy `Ink` records (a package here has no profile bytes);
  it reports each profile as `present|missing` in the target document so the user
  can import it with the existing color commands. Native (spot/ICC) color tokens
  still fail with `unsupported-capability` on documents below the color-management
  version, via the shared `apply_plan` check. Rules are stored and validated only;
  evaluating them is the audit item (roadmap 6).

## 7. Templates

A template is an ordinary `.pen` document (any supported version) plus a
`design.template` declaration (additive):

```json
"design":{"template":{"schema":1,"name":"social/post","version":"1.0.0",
 "parameters":{
  "headline":{"type":"text","label":"Headline","default":"Title",
              "targets":[{"object":"head","property":"content"}],
              "constraints":{"max_length":80}},
  "accent":{"type":"token","token_type":"color","default":"color.accent",
            "targets":[{"object":"bar","property":"fill"}]},
  "photo":{"type":"asset","targets":[{"object":"hero","property":"image"}]},
  "show_badge":{"type":"bool","default":false,
              "targets":[{"object":"badge","property":"visible"}]}}}}
```

- Parameter names follow the property-name rule of v0.6.24 (stable public
  identifiers; `^[a-z][a-z0-9_]*$`, <= 64 bytes). Types: `text`, `number`,
  `bool`, `color`, `token` (value is an existing token ID of `token_type`), `asset`
  (relative path within the document directory, same containment rules as other
  document-relative paths). `targets` use the v0.6.24 `{object, property}` grammar
  and property paths (`content fill stroke visible stroke_width opacity width
  height style_ref`, plus `image` for image nodes). Constraints: `max_length`,
  `min`, `max`, `enum` (<= 64 values), `required`.
- **Limits:** <= 64 parameters; <= 16 targets each; value <= 64 KiB (text) /
  finite numbers; <= 10,000 fill rows (`--data` batch fill, below).
- **Validation (at `design template check`):** targets exist and are not on locked
  layers, property paths are supported for the node kind, defaults satisfy the
  constraints, `token` parameters name tokens that exist (or are present in the
  declared libraries), parameter names unique.
- **Fill.** `design template fill TEMPLATE.pen --values values.json --output
  OUT.pen [--dry-run] [--if-revision SHA]`. Values are `{param: value}`; missing
  parameters use defaults; a missing required one, an unknown name, or a
  constraint failure is `invalid-input` listing every problem (all problems
  reported, not the first). Fill is planned as a **batch operation list** against
  the template (reusing `apply_batch_at`) so dry run and commit use the same
  planner. The output is a new file (written atomically); the template is
  never modified. The result `{"output","would_change","parameters":{name:{"applied":..,"targets":..}},"changes":[...]}`
  reports applied changes; `--in-place` is not offered in v1. The output keeps
  `design.template` with `filled_from` `{name, version, content_hash}` and the
  accepted values so a later fill/diff is reproducible.
- Template gallery contents come from `design_library.templates` (name -> path of
  a template document inside the package, hash covered by the package manifest).

### 7.1 Implemented behavior (`pentool design template`)

- `design template check DOC [--page P]` validates the declaration (the same
  parser used by fill) and reports every problem in one `invalid-template` error.
  Targets are resolved on the selected page (default first page); a target on a
  locked layer or inside a locked group is a problem, as is a property the node
  kind does not support. Supported pairs: `content` (text), `fill` (rect, ellipse,
  path, text), `stroke` and `stroke_width` (rect, ellipse, line, path), `width`
  and `height` (rect, image), `visible` and `style_ref` (any node), `opacity`
  (any node, composited versions only), `image` (image node, composited versions
  only). The parameter type decides the allowed properties: `text`->`content`,
  `number`->`stroke_width|opacity|width|height`, `bool`->`visible`,
  `color`->`fill|stroke`, `token`->`fill|stroke` (color tokens, written as a
  `{ref, fallback}` fill) or `style_ref` (other token types), `asset`->`image`.
- `design template fill DOC --values FILE --output OUT [--page P] [--dry-run]
  [--if-revision SHA] [--offset N --limit N]`. Fill is planned as `set-property`
  batch operations run through `scene::apply_batch_at` on an in-memory copy, then
  the candidate is validated; dry run and commit share that planner, and the
  output is written atomically only after everything succeeded. `--output` may
  not be the template. Result keys: `ok dry_run output would_change
  template{name,version,content_hash} parameters{name:{applied,targets}} changes
  matches returned offset limit has_more revision_before output_revision`;
  `changes` is the paginated batch change list (`--limit` 1..=1000, default 100).
- The output keeps `design.template` and adds `filled_from` `{name, version,
  content_hash}` and `values` (the accepted values after defaults).
- Asset parameters read a document-relative file (safe-path rules, <= 64 MiB) and
  embed it as an image asset.
- `design template list DIR` lists `*.pen` files in a directory that carry a
  `design.template` (sorted by file name, paginated, <= 5,000 files scanned).
- Not in this slice: `--data` multi-row batch fill, templates inside library
  packages (`design_library.templates`), `--in-place`.

## 8. File-format impact

**No `.pen` version bump and no migration are required.**

- Token store, references, fallbacks: already v4+ (`styles`, `{ref,fallback}`).
- `source` on tokens, `design.libraries`, `design.brand`, `design.template`,
  `design_library`: purely additive fields in positions where "unknown fields ...
  must survive load/edit/save" (`pen-format-v4.md`) and `additionalProperties:
  true` already holds in `docs/pen-format-v4.schema.json`. They do not alter
  rendering (the rule for extension fields), so older Pentool renders the same
  pixels; resolution tightening in 2.2 only affects previously-invalid input.
- The minor-release rule in AGENTS.md ("new backward-compatible features")
  applies: crate version becomes `0.19.0` at release time, `LATEST_VERSION`
  stays 12.
- Required doc/schema work: add a `docs/pen-format-v4.schema.json` `$defs` for
  `design`, a `docs/design-library-v1.schema.json`, and the token file schema
  (section 10). Unknown `design.*` keys inside the namespace are rejected by
  Pentool validation (reserved namespace) while keys outside it remain free.
- Exception, explicit: native ICC/spot color tokens need the document to be at
  the version that supports native colors (`color::has_v8`); that is a
  capability check at install/set time, not a new version.
- Downgrade: v4 -> v3 flattening already resolves styles to fallbacks; `design.*`
  and `source` are dropped by that flatten and the report MUST list them as
  `dropped-extension`. No silent loss.

If the lead later wants render-affecting bindings for `radius`/`shadow`/`typography`
tokens (open question Q1), that is the point a version bump (13) would become
necessary.

## 9. Audit

`pentool design audit DOC [--page ID | --all-pages] [--rules color,font,spacing,brand] [--library NAME]
[--offset N --limit N] [--fix-plan]`.

**Off-token definitions** (evaluated on resolved values, nodes in document order
per page, nested children depth-first):

| Check | Finding code | A property is off-token when |
| --- | --- | --- |
| color | `off-token-color` | a fill, stroke, text color, gradient stop or shadow color is a literal (no `ref`, or `ref` unresolved/invalid) **and** its value is not exactly equal to any `color` token value (exact match -> finding carries `suggest: <token>`, `exact: true`; nearest by deltaE76 in Lab with deltaE <= 2.0 -> `exact: false`) |
| font | `off-token-font` | a text node's (font, size, weight, italic) is not equal to the value of any `typography` token, ignoring token keys it does not set |
| spacing | `off-token-spacing` | auto-layout/layout `gap`, `padding`, margins or story spacing numeric literals not equal to any `spacing` token value (only where such a property exists in the node/layout schema) |
| radius | `off-token-radius` | rect radius literals not equal to any `radius` token value |
| refs | `unresolved-token`, `invalid-token`, `unused-token` | reference cannot resolve; token fails schema; token has no dependents (`unused` is info, not an error) |
| brand | `brand-contrast`, `brand-logo`, `brand-pair`, `brand-font` | rule from section 6 violated |

`none`/transparent fills and values inside a component/instance **fallback group**
are skipped (instances are audited by their exposed properties, not internals,
consistent with mutation protection). Hidden or locked nodes are included and
carry `hidden`/`locked` flags. Pixels are never sampled.

**Output (paginated, deterministic):**

```json
{"document":"a.pen","revision":"sha256:..","scope":{"pages":["p1"]},
 "matches":137,"returned":50,"offset":0,"limit":50,"has_more":true,
 "summary":{"off-token-color":90,"off-token-font":30,"off-token-spacing":17},
 "findings":[{"code":"off-token-color","severity":"warn","page":"p1","id":"hero-bg",
   "path":"style.fill","value":"#f59e0c","suggest":"color.accent","exact":false,"delta_e":0.4}]}
```

- Order: findings sorted by (page index, depth-first node order, property path
  bytewise, code). Node order is the document order, so output depends only on
  document bytes. `summary` counts the full result, not the page.
- Bounded: default `--limit 50`, max 500; scan cost is one pass O(nodes + findings)
  with token lookup tables built once (hash maps keyed by canonical value); the
  nearest-token search is capped at 256 candidate tokens per type (if a document
  has more, only exact matches are suggested and the output says
  `"nearest":"skipped"`). `--limit 0` is rejected. Page selection limits the scan
  to that page (AGENTS.md page rule); `--all-pages` is explicit.
- `--fail-on warn|error` sets exit status 3 when matches of that severity exist;
  default exit 0. Errors are `unresolved-token`/`invalid-token`; others `warn`.

**Fix proposals as a plan.** `--fix-plan` adds `"plan":{"operations":[...]}` where
operations are exactly `pentool batch` operations (also valid input to the
extension host `document.plan`), pagination-consistent with the findings
returned. Because no batch operation currently rebinds a node property to a
token, the implementation adds two additive ops to `scene::apply_batch_at`:
`bind-token {id, property, token}` (sets `style.<property>.ref`, keeps `fallback`
as the token's current value) and `set-token {name, token_type, value}` (typed
JSON value; `set-style` stays string-valued for compatibility). Only
`exact: true` findings produce fixes; near matches are listed without ops unless
`--fix-nearest` is passed. A plan is never applied by `audit`; apply it with
`pentool batch DOC --file plan.json --dry-run` then without. Applying the plan
changes only the nodes/properties named in its operations.

Scripting: an extension (v0.18.0) runs `document.search`/`document.get` and
`document.plan` with the same op list; no new host method is needed.

## 10. Token file import/export

### 10.1 Native format (`pentool.tokens`, schema 1)

```json
{"schema":1,"tokens":{
  "color.accent":{"type":"color","value":"#F59E0B","description":"CTA"},
  "color.cta":{"type":"color","value":{"ref":"color.accent"}},
  "type.body":{"type":"typography","value":{"font":"Inter","size":16,"weight":400}},
  "space.md":{"type":"spacing","value":16},
  "radius.card":{"type":"radius","value":8},
  "shadow.raised":{"type":"shadow","value":[{"dx":0,"dy":2,"blur":8,"spread":0,"color":"#00000033"}]}}}
```

It is the `styles` object plus `schema` and optional `description`. Deterministic
export: tokens sorted by ID, 2-space indent, trailing newline, `\n` line endings.
Import validates everything (types, IDs, alias closure, cycles, limits) before
writing; `design token import FILE DOC.pen [--dry-run] [--if-revision SHA]
[--on-collision fail|skip|overwrite]` default `fail`. Export: `design token export DOC.pen
[--format native|w3c] [--output FILE]` (stdout if no output; library export
`design library export PKG`).

### 10.2 W3C design tokens (format-module draft, `$type`/`$value`)

Mapped **one-to-one** (anything else is rejected; no lossy guessing):

| W3C | Native |
| --- | --- |
| group nesting `{"color":{"accent":{...}}}` | dotted ID `color.accent` |
| `$type: "color"` with `$value` hex string `#rrggbb[aa]` (or `#rgb[a]`) | `color` string |
| `$type: "dimension"` with `$value` `"<n>px"` in a group whose first segment is `space`/`spacing`/`size` | `spacing` number (px = document unit) |
| same, first segment `radius` | `radius` number |
| `$type: "fontFamily"`, `fontWeight`, `dimension` for size composed in a `$type: "typography"` object `{fontFamily, fontSize, fontWeight, lineHeight?, letterSpacing?}` | `typography`; fontFamily arrays import only the first entry **if** it is the only entry, otherwise rejected |
| `$type: "shadow"` object/array with `offsetX offsetY blur spread color` in px | `shadow` (`inset: true` rejected) |
| alias `"$value": "{color.accent}"` | `{ref}` (same-type only) |
| `$description` | `description` |

Rejected with `unsupported-token-construct` naming the JSON pointer and the
construct: `$extensions` (kept only if `--keep-extensions`, stored under
`description`-sibling `extensions`, never interpreted), `$deprecated`, `$root`,
non-px units (`rem`, `em`), `number`/`duration`/`cubicBezier`/`gradient`/`border`/
`transition`/`strokeStyle` types, composite alias pieces (a `{ref}` inside a
typography sub-field), color objects (`colorSpace`/`components`, Display P3 etc.
unless the target is native color with identical meaning, which v1 refuses),
`lineHeight` as a unitless number (ambiguous against absolute leading), and
resolver/theme files. Export to `w3c` fails the same way for native-only tokens
(spot, ICC, `stroke`, 2-value radius) unless `--skip-unmappable`, which lists the
skipped IDs. Round trip native -> w3c -> native is byte-identical for the mapped
subset (tested).

## 11. CLI surface

All commands print JSON (`--json` is implicit as in `package`, `instance`),
support `--page` where page-scoped, and mutations support `--dry-run` and
`--if-revision`. New top-level `design` group; `style` and `package` keep working.

| Command | Result (top-level keys) |
| --- | --- |
| `design token list DOC [--type T] [--source LIB\|local\|override] [--resolve] [--offset --limit]` | `matches returned offset limit has_more tokens[{token,type,value,source?,state(local\|clean\|override),dependents:int}]` |
| `design token resolve DOC ID` | `token type value chain[] status(resolved\|unresolved\|invalid) fallback_used` |
| `design token import / export` | section 10; `{imported[],skipped[],would_change,change}` |
| `design library init/pack/verify/inspect/export` | `{ok,package_hash,name,version,tokens:int,templates:int,warnings[]}` |
| `design library install/update/status/diff` | section 5 and 4.1 |
| `design brand show DOC` / `design brand apply LIB DOC` | brand block with resolved values; apply vendors the kit's referenced tokens and `design.brand` (transaction, 6.1) |
| `design template check/fill/list` | section 7; list is paginated like tree |
| `design audit` | section 9 |

Exit codes follow the existing CLI (0 ok, 1 error); `audit --fail-on` adds 3.

### Studio (sketch)

- **Tokens panel:** list/filter by type, state badges (local/clean/override),
  edit via `style set` semantics, usage count (`dependents`), "revert to library
  value" (=`take-source`).
- **Library browser:** installed/available (from `pentool.lock` and
  `.pentool/packages`), diff and update-plan preview using `library diff/update
  --dry-run`, resolve conflicts per token.
- **Template gallery:** thumbnails (rendered by Rust), parameter form generated
  from the declaration, Preview = `fill --dry-run`.
- All actions call `pentool serve` endpoints under the existing revision guard;
  no logic duplicated in the browser. Editing is disabled while unsaved local
  edits exist, as for extensions (`docs/extensions-v1.md`).

## 12. Acceptance tests (must pass; names are the planned test names)

1. `tokens_update_changes_exactly_dependents`: document with 20 tokens, 60 nodes
   referencing a subset, aliases, and an instance override; update one library
   token; byte-compare the file: only the changed token record and (via
   resolution) nodes whose resolved chain includes it differ in *rendered* pixels
   and in `design token list` `value`; every other byte, node style and
   unknown field is identical; `writes` equals `["color.accent"]`.
2. `tokens_update_keeps_local_override_and_reports`: base/current/incoming table
   rows each produce their status; a conflict blocks commit with the file
   unchanged; `--continue-on-conflict` updates only clean tokens in one history
   entry; `undo` restores byte-for-byte.
3. `dry_run_equals_commit_plan`: plan JSON from `--dry-run` equals the report of
   the real run except `dry_run` and `change`.
4. `alias_cycle_rejected`, `alias_depth_limit`, `token_count_limit`: error codes
   and `path` array as in 2.4; file unchanged.
5. `resolution_is_offline_and_deterministic`: rendering and `token resolve`
   with `.pentool/` absent and the network disabled; identical output on repeated
   runs and across the CI matrix (digest assertion).
6. `library_pack_is_deterministic_and_verified`: two packs equal bytes; tampered
   asset byte fails `verify` with `hash-mismatch`; `lock sync --offline` fails
   cleanly when missing; signature round trip.
7. `audit_bounded_paginated_deterministic`: a 20,000-node document; `matches`,
   `returned`, `offset`, `limit`, `has_more` exact for pages 0..n and concatenated
   pages equal one unpaged run; same bytes on repeated runs; `limit > 500`
   rejected; runtime/allocation bound asserted (no clone of the document per
   finding); `--page` scans only that page.
8. `audit_fix_plan_applies_exactly`: applying the plan through `batch` changes
   only the nodes listed; re-audit has zero `exact` findings.
9. `template_fill_dry_run_matches_commit`, `template_fill_reports_all_errors`,
   `template_fill_never_modifies_template`.
10. `w3c_roundtrip_subset` and `w3c_rejections` (each rejected construct above
    yields `unsupported-token-construct` with a pointer).
11. `format_unknown_fields_survive`: documents with `design.*` and `source`
    survive edit/save and render identically in the previous release behavior
    (golden pixel digest unchanged).
12. Studio smoke: token edit and library update preview exercised on the
    served document (documented as Windows-only if no other OS available).

CI additions: update README CLI examples, `--help` examples, fixture
`docs/fixtures/design-library-valid.penpkg` + an invalid-cycle fixture, schemas.

## 13. Implementation order and files per roadmap item

1. Spec (this file) - `docs/design-systems-v1.md`, README link, roadmap tick.
2. **Token resolution** - `src/style.rs` (limits, same-type aliases, depth cap,
   error shape), `src/scene.rs` (`with_resolved_tokens`, validation, `bind-token`
   and `set-token` batch ops, tree `--resolve`), new `src/tokens.rs` (value
   schemas, hashing, resolution chain), `src/main.rs` (`design token`), tests
   `tests/v019_tokens.rs`.
3. **Library packages** - new `src/design_library.rs` (block schema, verify,
   plan_update, diff), `src/package.rs` (pack/verify hook for `design_library`),
   `src/instance.rs` (only to share the resolution enum, no behavior change),
   `src/main.rs`, `tests/v019_library.rs`.
4. **Brand kit** - new `src/brand.rs`, `src/color/spot.rs` and
   `src/color/catalog.rs` (reuse, no change expected), `src/package.rs`.
5. **Templates** - new `src/template.rs`, `src/scene.rs` (plan via batch),
   `src/asset.rs` (reuse property grammar), `tests/v019_template.rs`.
6. **Audit** - new `src/audit.rs`, `src/scene.rs` (walk helper), `src/color/value.rs`
   (deltaE76 for suggestions), `tests/v019_audit.rs`.
7. **Editor** - `src/server.rs` (endpoints), `web-ui/src/lib/api.ts`,
   `web-ui/src/lib/actions.ts`, new `web-ui/src/workspaces/DesignSystemPanel.tsx`
   (and gallery), `web/studio.html` rebuilt (`bun run build`), `docs/studio.md`.
8. **Token files** - new `src/token_file.rs`, `src/main.rs`,
   `tests/v019_token_file.rs`.
Each item also updates `README.md`, `docs/pen-format-v4.schema.json` (`design`
`$defs`) and the roadmap checkbox only when tests, docs and CI evidence exist.

## 14. Open questions (decisions the code did not settle)

- **Q1: render-time binding of non-color tokens.** Only color tokens have a clear
  render site (`style.fill`/`stroke`). `typography`, `spacing`, `radius`,
  `shadow` have no node property that consumes a `ref` today (rect radius is
  `radius_x/radius_y`; layout styles are separate). This spec makes them
  audit/instance-override/layout-style targets only, so no format bump. Binding
  them into rendering is a v13 question.
- **Q2: legacy token names.** Existing free-form names (spaces, uppercase) are
  grandfathered; confirm they may not be exported (spec) rather than auto-slugged.
- **Q3: `--prefix` / namespaces** for installing two libraries with colliding IDs;
  v1 refuses collisions. Alternative: namespace `acme.color.accent` in the ID.
- **Q4: token `type` vs `typography`.** Keep the existing `typography` name, with
  W3C/roadmap `type` as import alias only?
- **Q5: `design` CLI group vs extending `style`/`package`/`instance`.** Spec uses
  a new group to keep help discoverable; check conflicts with `LayoutStyle`.
- **Q6: alias depth cap 8 and 5,000 tokens** are new limits on a currently
  unbounded `styles`; confirm no real documents exceed them (fixtures in
  `examples/` should be scanned).
- **Q7: signed libraries mandatory?** Spec: optional with `--require-signature`.
- **Q8: where audit spacing applies** depends on which layout/auto-layout
  properties exist in v4-v12 nodes; the exact property list must be enumerated
  from `src/layout.rs` and `src/scene.rs` when implementing (not audited here).
- **Q9: delta E threshold** (2.0 CIE76) and 256-candidate cap are proposals.
- **Q10: native-color (spot/ICC) tokens in W3C export** are refused; confirm.

## Implementation deviations (items 2-4, v0.19.0)

Where the shipped behavior differs from the sections above, the shipped behavior
is normative:

- `design library update` requires `--package FILE`. Resolving `--library NAME
  --to VERSION` against the lock or `.pentool/packages` is not implemented;
  `--library` and `--to` only assert the package's name and version.
- The lock file and the document are not one transaction. `package::install`
  writes `pentool.lock` first, then the document is committed through the shared
  transaction; if the commit fails the previous lock bytes are restored (or the
  lock removed if it did not exist). A crash between the two steps can leave a
  lock entry the document does not record; `design library status` reports it.
- Depth (8), count (5,000) and value-size limits are checked **on write** (token
  mutation, install, update, pack). Opening a document checks only alias cycles,
  so a document already over a limit still opens, resolves and renders.
- A token removed from the library and resolved with `keep-local` or `detach`
  becomes a local token (its `source` record is dropped); it is no longer tracked
  by the library.
- Brand kit: see 6.1.

## Decisions on open questions (lead, v0.19.0)

- Q1: no render-time binding for typography, spacing, radius or shadow in v1. They are audit, instance override and layout-style targets only. This keeps the format at the current version (no bump). Render binding is deferred.
- Q2: legacy free-form token names are grandfathered and not exportable; no auto-slugging.
- Q3: ID collisions between two libraries are refused in v1; no namespacing or `--prefix`.
- Q4: `typography` is canonical; `type` is an import alias only.
- Q5: new `pentool design` command group; existing `style`, `package` and `instance` behavior is unchanged.
- Q6: the new limits (depth 8, 5,000 tokens) are enforced on write and on library install; a document already over a limit still opens and renders, with a warning from `design audit`. Check `examples/` and fixtures while implementing.
- Q7: signatures stay optional (`--require-signature` opts in).
- Q8: spacing and gap properties for audit are enumerated at implementation time from `src/layout.rs` and the node model, and listed in the spec once settled.
- Q9: deltaE76 threshold 2.0 and the 256-candidate cap are accepted as proposed.
- Q10: spot and ICC color tokens are refused in W3C export with a clear error.

## Implementation notes: audit (item 6, v0.19.0)

Shipped in `src/design/audit.rs`; the shipped behavior is normative where it
differs from section 9.

- Checks implemented: `color` (style `fill`/`stroke` literals; exact match gives
  `suggest`+`exact:true`, else nearest by deltaE76 <= 2.0 gives `exact:false`,
  `delta_e`; above 256 color tokens only exact matches are suggested and the
  report says `"nearest":"skipped"`), `font` (text nodes vs `typography` tokens,
  only keys the token sets are compared; a matching node yields no finding),
  `radius` (rect `radius_x`/`radius_y`; a match yields no finding), `refs`
  (`unresolved-token`, `invalid-token` on nodes and on tokens, `unused-token` as
  `info`), and `brand` (`brand-contrast`: text fill vs opaque page background;
  `brand-pair`: node fill/stroke, or text fill/page background; `brand-font`;
  `brand-logo`: `min_width` and `forbidden_backgrounds` for image nodes whose
  `id` or `asset` equals the logo's). A category with no tokens of that type is
  skipped instead of flagging every literal.
- `spacing` is accepted by `--rules` but yields nothing: v4-v12 nodes have no
  gap/padding property (Q8). `clear_space` and shadow/gradient colors are not
  audited. Instance internals are not walked.
- Document-level token findings (no page) follow the page findings. `--library`
  limits suggestions and `unused-token` to tokens vendored from that library.
- `--fail-on warn|error` exits 3; `severities` gives the error/warn/info totals.
- `--fix-plan` plans only the returned page of findings. Batch applies to one
  page, so `plan.page` names it and `plan.other_pages` lists further pages with
  fixes (re-run with `--page`). Only fill/stroke have render-time token binding,
  so font and radius findings never produce operations.
- Batch ops `bind-token {id, property: fill|stroke, token}` (resolved `color`
  token; keeps the token value as fallback; refuses locked layers/groups) and
  `set-token {name, token_type, value}` (typed value, keeps `source`, validated
  by `tokens::check_write`) are additive; no format change.
- Not done: `tree/search --resolve` and `validate --strict` (no top-level
  `validate` command exists; the audit reports the same unresolved/invalid
  conditions).

## Implementation notes: Studio (item 7, v0.19.0)

- `GET /api/design/tokens|library/status|audit|templates|template/check` and
  `POST /api/design/template/fill` in `src/server.rs` construct the same
  `DesignCommand` values the CLI uses and call `design::cli::run`; nothing is
  reimplemented. Reads run under the document gate.
- Fill takes `{template?, values, output, page?, revision, dry_run?}`. For the
  shared document the Studio revision is checked with `require_revision` and the
  file's byte revision is then pinned as `--if-revision`; another project file
  needs its byte revision. `output` must be a relative `.pen` path inside the
  served project; `..`, absolute paths and the template itself are refused.
- Audit stays read-only. Applying its fix plan reuses `POST /api/scene`.
- Scope cut: the Studio panel is read/preview plus fill and fix-plan apply. Token
  editing, library install/update/diff, thumbnails and conflict resolution are
  not in the panel; the sketch above remains the target for those.
- Not tested in a live browser; `web/studio.html` is rebuilt with `bun run build`.
