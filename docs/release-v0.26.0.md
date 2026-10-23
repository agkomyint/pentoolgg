# Pentool v0.26.0

A stabilization release: about 70 bug fixes, each with a regression test, plus a
small set of CLI conveniences approved with them. No new `.pen` fields or
dependencies. Several commands print less or report more than before; see
"Output changes" if you parse their JSON.

## Output changes

- `import` no longer prints the full `fidelity.items` array. Without a written
  `--report` it prints `items_omitted` and a `report_hint` (#204).
- `linked embed` and `relink` results omit the embedded payload (#173).
- `--json` error `message` now carries the full cause chain, not only the outermost
  context (#150).
- `paint-plan` rejects unknown pass args and missing required args at parse time;
  `fill` passes now require `x` and `y` (#132).
- `paint-palette` ignores pixels with alpha below 16, and `share` is a fraction of
  the scope (#235, #143). `paint-score` luma matches the protocol (#133).
- `serve` answers unknown routes and wrong methods with JSON problem bodies (#35).
- `package-verify` reports files not listed in the manifest as `unlisted` (#28).
- `image analyze` focal suggestion uses pixel centers (#275, #386).
- `paint-diff` reports `layers_added`, `layers_removed` and journal entry counts
  (#181). `stroke-field` accepts the same 1-512 `cell` range as
  `paint-constraints preview` (#180).

## New

- `export DOC -` with `--format` streams SVG/PNG/PDF to stdout (#52).
- `batch` accepts inline JSON, `@file` or stdin operations (#89).
- `new` takes `--background`, `--name` and `--if-revision` with `--overwrite`
  (#48, #387); `diff --visual --out` (#51); `fonts` lists embedded faces (#53).
- `info --revision` and a history `revision` field (#54, #83).
- `object set` addresses grouped children by ID (#13); asset property `set` works
  without `--target` (#65).
- `scene3d`: `--dry-run`/`--if-revision` accepted after any subcommand, documented
  `INPUT`/`ID` and camera flags; `inspect` reports product forms instead of
  panicking (#213, #148).
- Studio: layer context menu moves a layer forward or backward (#392); AI
  generation shows live progress and Studio requests time out instead of freezing.

## Fixes

- Paint protocol: scopes distinguish layer, group and node (#134); constrained
  strokes record mask hashes (#140); `protect_score` (#142); empty-mask preview
  and unequal-size scoring are rejected (#135, #141); id/description changes show
  in plan diffs (#138).
- `object set` validates colors and path data on v3 documents (#19, #18).
- Errors name the fix: compositing needs `migrate --target 6` (#8); missing-file,
  unsafe-path and ICC errors keep their codes and hints (#207, #73, #355, #241);
  `analyze` names a malformed `--samples` or `--query` (#42).
- Studio: stale "unsaved" state, tablet-width panel overlap, mask operations and
  AI run state fixes (#305, #400, #282, #374).
- Many more CLI, library, package, photo and documentation corrections; see the
  commit log since v0.25.0.

## Known gaps

Still open: Ed25519 operation signing and stale-base rejection for agents,
`--cell 0` handling (#180), remaining scene3d flag help (#148), and the issues
tracked on GitHub.
