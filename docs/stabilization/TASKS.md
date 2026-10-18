# Stabilization queue (0.20.x)

Merged from the five audits in this folder (2026-10-18, run against the installed
0.20.0 binary). Detail for each ID (repro, expected vs actual, suspected files,
regression test to write, acceptance criteria) lives in the matching
`TASKS-<area>.md`; evidence lives in `audit-<area>.md`.

The audits were done by agents. **Verify each defect by reproducing it before
fixing** (AGENTS.md bug-fix recipe, step 2). If it does not reproduce, record that
in the task file and close it.

Rules for every wave: patch-safe only, one regression test per fix, fix in the
shared code path, no new features. Items marked DECISION need the maintainer
before any code. One patch release per wave.

## Wave 1, release 0.20.1: P1 patch-safe

| ID | Defect | Area file |
| --- | --- | --- |
| CORE-001 | `import` and `add` fail on default `pentool new` v4 documents (misleading "no canvas") | TASKS-core |
| X3D-002 | `pack carton/sleeve/crease/bind` and `scene3d batch` check `version == 11`, reject v12 | TASKS-3d-interchange |
| X3D-006 | SVG import with a clip mask fails `[missing-mask]`; `mask_resources` not merged in `append_imported_document` | TASKS-3d-interchange |
| X3D-007 | `.fig` import: `tree`/`search` fail "node not found" (`node_bounds_on_page` on empty fallback group) | TASKS-3d-interchange |
| X3D-001 | Shipped glass-crystal example cannot export (393 MB buffers vs 256 MiB limit); fix the example or the budget logic, not the limit blindly | TASKS-3d-interchange |
| EXT-001 | Collab, Stats and Systems workspaces unreachable from the Studio selector | TASKS-extensions-collab |
| EXT-002 | Comment author taken from request body `actor`; derive from the token | TASKS-extensions-collab |
| RASTER-001 | Gradient stop key `offset` (documented) rejected; only `position` works | TASKS-raster |

Status: all eight fixed in 0.20.1 (`docs/release-v0.20.1.md`); notes per item in the area files.

## Wave 2, release 0.20.2: P2 patch-safe

CORE-002 (tight bounds ignore stroke), CORE-004 (corrupt preview cache served ok,
incomplete key), CORE-005 (`migrate --target 4` rejects docs with attached
instances), CORE-006 (package verify accepts trailing bytes), CORE-008
(`instance update` lacks `--if-revision`), CORE-011 (invalid override value
accepted), CORE-014 (false `geometry-changed` plans), EXT-003 (sync push skips ops
and advances `pushed_to`; check for real data loss first), EXT-005 (`.bak.N`
siblings), EXT-006 (`spacing` audit rule no-op), EXT-007 (PROTOCOL.md missing about
71 of 86 routes), X3D-004 (`scene3d uv show` unpaginated), X3D-005 and X3D-008
(missing pinned goldens/digests), X3D-009 (folding overlap), PRINT-002 (PDF/X
self-declares conformance: reword), PRINT-005 (preflight ok vs pdfx/package
refusal), PRINT-011 (normative `layout-v9-valid.pen` cannot render), RASTER-002
(image assets cannot be created/indexed), RASTER-009 (missing referenced test).

## Wave 3, release 0.20.3: P3, docs and help drift

CORE-012, CORE-013, EXT-008 to EXT-011, X3D-010 to X3D-013, PRINT-006 to PRINT-010,
PRINT-012, PRINT-013, RASTER-004 to RASTER-008, and roadmap checkbox corrections
listed at the bottom of each `TASKS-*.md` (tick or untick only with the evidence
cited there). RASTER-003 is unreproduced: try to reproduce, otherwise close.

## Needs a maintainer decision (no code until answered)

| ID | Question |
| --- | --- |
| EXT-004 | `serve --host 0.0.0.0` without `--collab` has no auth: refuse, warn, or leave? |
| EXT-012 to EXT-017 | Collab gaps: compact, stale-base rejection, signing, conflict-resolution command/UI, live cursors, `summary.json`. Which are in the 0.20.x scope? |
| EXT-019 | `tree/search --resolve` and `validate --strict` are promised but unshipped |
| PRINT-001 | Studio print panel lost Classic features (gamut warning, PDF/X link, intent/BPC, overlays): restore in a patch or declare a minor? |
| PRINT-003, PRINT-004 | Soft proof and spot inks have no CLI |
| X3D-003 | Raster-image environment lighting not implemented |
| CORE-003, CORE-007, CORE-010, CORE-015 | CLI surface promised by roadmap but absent (`add` placement flags, cache/trust CLI, alias tokens, tracked local edits) |
| CORE-009 | 100k-path peak RSS target unmet |

Recommendation for the roadmap: where an unshipped CLI was promised but is a new
capability, change the roadmap text and untick the box rather than adding the
feature in a patch.

## Unverified areas (an auditor could not or did not run these)

Studio panels in a live browser (EXT-018), GPU parity, hosted per-OS evidence,
Figma REST JSON, Sketch overrides, v0.6.0 lock/registry/governance, v0.6.2 layout
and history prune, v0.6.24 items 15-20, architecture tasks 1-8.

## Process notes

- Two auditors ran `taskkill`/`Stop-Process` on every `pentool.exe`; any
  server-related result in `audit-extensions-collab.md` and `audit-raster.md` may
  be affected. Re-run those repros in isolation when fixing.
- Do not let agents kill processes by image name; use the PID they started.
