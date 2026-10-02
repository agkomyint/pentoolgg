# v0.6.2 — Design authoring above raw paths

Make Pentool practical for everyday design construction, not only low-level SVG
path scripting. A designer or agent should be able to create a card, move it as one
unit, recolor a document through named tokens, lay objects out, wrap text, and undo
any write without hand-authoring path arcs or hundreds of shell commands.

v0.6.2 builds on the v0.6.1 renderer-performance work. New abstractions must lower
to the same deterministic, offline renderer and must not weaken v0.5/v0.6 asset,
instance, package, or lock guarantees.

## Product principles

- High-level editing is the primary interface; raw SVG paths remain an escape hatch.
- One logical operation is one transaction, one history entry, and at most one
  backup, regardless of which CLI command initiated it.
- Groups, styles, primitives, and text boxes are stored as semantic document data,
  not conventions encoded in object names.
- Every mutating command supports dry run, revision guards, structured output, and
  deterministic behavior.
- Existing v1–v3 documents continue to open and render. Migration is explicit,
  tested, reversible, and never silently discards data.
- CLI, Rust library, browser editor, and package assets use the same scene and
  mutation services.

## Must ship, in order

- [ ] **1. Specify an ordered scene graph and migration contract.** Publish the
  next `.pen` schema before adding commands. Replace separate layer-level path and
  text collections with an ordered node model capable of containing:

  - groups and nested groups with stable IDs and transforms;
  - rectangle, rounded rectangle, ellipse/circle, line, path, and text nodes;
  - style references with local fallback values;
  - component definitions and instances compatible with the v0.5 asset model;
  - extension fields that survive load/edit/save round trips.

  Define local versus world transforms, bounds, z-order, clipping, deletion,
  reparenting, cycle prevention, ID uniqueness, and maximum nesting depth. Provide
  JSON Schema, normative fixtures, v1–v3 readers, an explicit `pentool migrate`, and
  a flatten/export path for older consumers. Do not silently upgrade a file merely
  by reading it.

- [ ] **2. Centralize all writes in one transaction engine.** Route `path put`,
  `text put`, layer geometry, object editing, batch, page import, styles, groups,
  and every new command through one mutation API. Each operation validates before
  commit, supports `--dry-run` and `--if-revision`, writes atomically, and produces
  a structured change summary. A failed operation leaves the document byte-for-byte
  unchanged.

- [ ] **3. Add semantic shape primitives.** Implement rectangle, rounded rectangle,
  ellipse/circle, and line nodes with measured bounds and normal styling:

  ```sh
  pentool shape card.pen rrect card-bg --layer content \
    --x 80 --y 80 --width 360 --height 220 --radius 24 \
    --fill '#111827'
  pentool shape card.pen circle avatar --layer content \
    --cx 136 --cy 144 --radius 32 --fill '#22D3EE'
  pentool shape card.pen line divider --layer content \
    --x1 104 --y1 184 --x2 416 --y2 184 --stroke '#334155'
  ```

  Accept conventional aliases (`w`/`width`, `h`/`height`, `r`/`radius`) in JSON
  APIs while keeping one canonical serialized form. Geometry operations must edit
  primitive parameters when possible and convert to a path only through an explicit
  command.

- [ ] **4. Add groups and everyday components.** Provide create, add/remove child,
  ungroup, move, rotate, scale, duplicate, rename, reorder, and bounds inspection:

  ```sh
  pentool group card.pen create card-1 --children card-bg,title,icon
  pentool group card.pen move card-1 --dx 40 --dy 0
  pentool group card.pen duplicate card-1 --id card-2 --dx 400
  ```

  Allow a group to be promoted into a local component definition and placed as an
  instance. Reuse v0.5 source identity, fallback snapshots, overrides, detach, and
  update semantics instead of inventing an incompatible component system.

- [ ] **5. Let batch create and compose complete scenes.** Add `put-shape`,
  `put-path`, `put-text`, `create-group`, `set-style`, and reparent operations to
  batch. One JSON request must be able to construct a full slide or screen in one
  validated transaction. Support temporary aliases so later operations in the same
  batch can refer to objects created earlier. Report the failing operation index,
  page, layer, group, and object ID without committing a partial result.

- [ ] **6. Add document styles and design tokens.** Store named color, stroke,
  typography, spacing, radius, and shadow tokens in the document. Nodes reference
  tokens while retaining explicit fallback values for robust rendering. Provide
  list, create, set, rename, delete, usage, detach, and replace commands:

  ```sh
  pentool style app.pen set color.accent --type color --value '#F59E0B'
  pentool style app.pen usage color.accent
  pentool style app.pen replace color.accent color.brand --dry-run
  ```

  Token edits update every reference without rewriting every node. Detect aliases
  and cycles, define missing-token behavior, preserve package/instance fallbacks,
  and include style dependencies in asset/package manifests.

- [ ] **7. Add scoped bulk replacement.** Support direct values as well as named
  styles, with page, layer, group, object-type, visibility, and selection scopes:

  ```sh
  pentool replace app.pen --fill '#22D3EE' --to '#F59E0B' --dry-run
  pentool replace app.pen --page dashboard --layer cards \
    --stroke '#334155' --to '#475569'
  ```

  Preview matched IDs and before/after counts. Require an explicit `--all-pages`
  for document-wide changes and never alter embedded assets or detached fallback
  snapshots unless separately requested.

- [ ] **8. Add alignment, distribution, and bounds snapping.** Implement left,
  horizontal-center, right, top, vertical-center, bottom, equal horizontal/vertical
  distribution, fixed-gap distribution, and snapping to another object's bounds.
  Operate on objects or groups, support key-object and selection-bounds modes, and
  define behavior for transformed, rotated, locked, hidden, and zero-size nodes.
  Return computed bounds and applied deltas in dry-run output.

- [ ] **9. Add real text boxes.** Support point text and bounded text with width,
  optional height, wrapping, overflow policy, horizontal alignment, vertical
  alignment, line height, and explicit anchor modes (`baseline`, `top`, `center`,
  `bottom`). Document in `--help` that existing `y` is a baseline. Bounds, layout,
  SVG, PNG, PDF, browser display, and hit testing must share the same shaping and
  line-breaking result.

- [ ] **10. Unify history, backups, and undo.** Store project-local history under a
  hidden directory such as `.pentool/history/`, not beside the `.pen` file. Add:

  ```sh
  pentool history app.pen
  pentool undo app.pen
  pentool redo app.pen
  pentool restore app.pen --revision <id>
  pentool history prune app.pen --keep 50 --dry-run
  ```

  Use content hashes, bounded retention, atomic restoration, and inter-process
  locking. Every mutating command follows the same policy. Migrating legacy
  `.bak.N` files is explicit; never delete them automatically.

- [ ] **11. Make CLI failures agent-safe.** Add a global `--json` mode with stable
  error codes, command context, page/layer/group/object identity, nearest-ID
  suggestions, and actionable hints. Expected user errors must not print Rust
  backtraces. Treat a closed stdout pipe as normal termination, including
  `pentool tree ... | head`. Add cross-platform SIGPIPE/broken-pipe tests.

  Make negative numeric values parse naturally without requiring a `--` separator.
  Audit every command's help text for coordinate meaning, defaults, scope, output,
  backup/history behavior, and runnable examples.

- [ ] **12. Add multi-page export and document comparison.** Implement:

  ```sh
  pentool export deck.pen ./exports --all-pages --format png
  pentool export deck.pen deck.pdf --all-pages
  pentool diff old.pen new.pen --visual --json
  ```

  Multi-page output uses stable, collision-safe filenames and an atomic staging
  directory. PDF preserves vectors and searchable text when fonts permit, embeds or
  outlines fonts deterministically, and has explicit page sizing. Structural diff
  reports pages, groups, nodes, styles, geometry, text, and instance changes;
  optional visual diff produces bounded artifacts.

- [ ] **13. Add optional compact serialization.** Keep human-readable JSON as the
  default. Add `pentool format --compact` and `--pretty`, producing semantically
  equivalent deterministic files. Never make compact output a hidden side effect of
  editing. Measure size and parse-time changes on the 257-object feedback fixture
  and large benchmark fixtures.

- [ ] **14. Complete browser parity and authoring documentation.** Expose groups,
  primitives, styles, layout, text boxes, history, and diff through shared Rust
  services and the browser UI. Publish task-oriented guides for building a card,
  creating a reusable component, recoloring through tokens, laying out a grid,
  undoing a transaction, and generating a multi-page PDF.

## Acceptance targets

- Construct the 257-object feedback design in at most **five CLI invocations** using
  one scene batch plus optional style/layout/export operations, rather than roughly
  260 invocations.
- Move, rotate, duplicate, recolor, and undo one complete card without enumerating
  its path/text/icon children.
- Replace an accent color throughout a document with one dry-runnable command, and
  accomplish the same result by updating one named token.
- Create rounded rectangles and circles without supplying SVG path data.
- Lay out a three-by-three card grid without manually calculating every coordinate.
- Wrap text identically in browser, SVG, PNG, and PDF outputs.
- Every write command creates exactly one discoverable history entry; undo and redo
  restore byte-valid documents, and failed writes create none.
- Expected CLI errors emit no backtrace, JSON errors are machine-parseable, piping
  into a short reader does not panic, and negative values require no separator.
- Existing v1–v3 fixtures render identically before and after migration within the
  established visual tolerance.
- Compact serialization reduces the 257-object fixture by at least **25%** without
  changing document semantics.

## Out of scope

- Real-time multiplayer collaboration.
- A general constraint solver, responsive web-layout engine, or CSS compatibility.
- Arbitrary executable plugins, scripts, or package hooks.
- Silent migration, silent token rebinding, or silent component updates.
- Making PDF the canonical document format.

## Acceptance demo

Build a nine-card, multi-page design from semantic shapes, bounded text, named
styles, groups, and a reusable local component. Do it in five or fewer CLI calls,
then recolor the theme, align and distribute the cards, move and duplicate one card,
inspect the structural/visual diff, undo and redo the change, and export every page
to SVG, PNG, and one vector PDF. Repeat after migrating an existing v3 document and
prove that rendering, package assets, instance fallbacks, history, and structured
errors remain deterministic across Windows, Linux, and macOS.
