# Audit: roadmap v0.15.0, v0.16.0, v0.17.0 (3D and design interchange)

Auditor: read-mostly agent, 2026-10-18. Binary under test: installed Pentool 0.20.0
(`C:\Users\OS\AppData\Local\Programs\Pentool\0.20.0\pentool.exe`, no `gpu` feature).
Scratch work was done outside the repo. No source, test, or roadmap file was edited.
Severity: P0 data loss or corrupt `.pen`; P1 documented feature broken; P2 wrong
output or missing test/doc/help; P3 polish. Defect IDs refer to `TASKS-3d-interchange.md`.

Summary: WORKS 36, PARTIAL 9, BROKEN 5, UNVERIFIED 8 (see rows).

## v0.15.0 static 3D

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| 1 Spec frozen (`docs/scene3d-v1.md`, schema) | [x] | WORKS | docs exist; v10+ gate and fields match CLI behavior | - |
| 2 GLB/glTF/OBJ importers + hostile corpus | [x] | WORKS | Refused with named errors and no mutation: truncated GLB, junk, `../` URI, `file://` URI, node cycle, oversized accessor, OBJ bad index. OBJ `mtllib ../x` is reported `[unsafe-path]` under `unsupported` and not read | - |
| 3 CPU rasterizer, deterministic | [x] | WORKS | two exports of the same doc: identical sha256; lantern example PNG reproduced byte-identically | - |
| 4 PBR, lights, shadows, environment | [x] | PARTIAL | lights/shadows/materials work. Environment is a sky/ground gradient only; the Model section's raster-image environment/IBL and `environment.kind` JSON are not implemented or documented (X3D-003) | P2 |
| 5 `scene3d` node, PNG/SVG/PDF export | [x] | PARTIAL | `export` to PNG, SVG (1 embedded image), PDF (2.88 MB) works. Shipped example `examples/v015-glass-crystal/glass-crystal.pen` fails: `[limit-exceeded] render buffers need 393120000 bytes; the limit is 268435456` (X3D-001) | P1 |
| 6 Agent commands, `scene3d inspect` | [x] | WORKS | add/frame/light/camera/material/set/inspect/list; undo/redo; dry-run flag | - |
| 6a UV CLI | [x] | PARTIAL | `uv show` has no pagination/limit and returns about 220 KB for a 3-primitive model, no `has_more`/`limit` fields (X3D-004). Roadmap spells `show --limit`, `project --mesh --kind --set`, `layout --mesh`; shipped spellings differ (X3D-012) | P2 |
| 6b Physical camera | [x] | WORKS | focal 24/50/85 give 3 distinct digests; aperture/focus flags accepted; tests in `tests/v0150_scene3d.rs` | - |
| 7 `wgpu` backend, byte-parity vs CPU | [x] | UNVERIFIED | installed binary has no `gpu` feature; `--renderer accelerated` falls back to `reference` with a message (fallback WORKS). Parity not testable here | - |
| 8 Editor (orbit, panels, indicator) | [x] | UNVERIFIED | browser UI not exercised | - |
| 9 Packaging, history, recovery | [x] | WORKS | `linked collect` yields identical render; undo/redo covers 3D edits | - |
| 10 Hostile corpus, goldens, 100k-tri benchmark, size | [x] | PARTIAL | 100k-triangle benchmark 13.96 s vs 30 s budget, digest matches `docs/performance.md`; exe 55,714,304 bytes < 60 MB gate. Acceptance goldens (reference sphere, textured cube, PBR grid) have no pinned byte digest test found (X3D-005) | P2 |
| Accept: 24/50/85 mm and DoF | - | WORKS | tests exist and distinct digests observed | - |
| Accept: 100k-triangle budget | - | WORKS | see task 10 | - |

## v0.16.0 3D in layouts and mockups

Ticked tasks 1-7, 10, 12 were exercised where the CLI allows; unticked tasks 8, 8a, 9,
11, 13, 14 are assessed against their own text.

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| Format v11 bindings/forms/packaging | [x] | PARTIAL | `form add`, `graphic add`, `pack` inspect/preflight/preview/export, cycle detection work on v11 and v12. Mutating commands reject v12 documents (X3D-002) | P1 |
| `pack carton`/`sleeve`/`crease`/`bind` on current format | [x] | BROKEN | on a doc after `migrate --target 12`: `[unsupported-capability] package mockups require document format v11`; `pack crease`: "per-crease editing needs document format v11; upgrade the document first" (misleading); `pack bind`: "panel artwork requires document format v11" | P1 |
| `scene3d batch` on v12 | [x] | BROKEN | "batch mockup rendering requires document format v11" | P1 |
| Silent v12 migration by SVG gradient import | - | PARTIAL | `appending_svg_gradients_migrates_older_destination_to_v12`: a v11 doc with packaging becomes v12 and then its packaging edits are refused (consequence of X3D-002) | P1 |
| 8 / 8a (unticked) | [ ] | UNVERIFIED | accepted as open per the roadmap text; not re-derivable from the CLI | - |
| 9 Folding geometry (unticked) | [ ] | PARTIAL | `pack carton --template mailer` then `pack preflight` at fold 1: needs-review, 1 collision (`zero-thickness-intersection`, lid vs glue-joint) matches the roadmap's admitted pinned overlap. Straight-tuck at fold 1 is clear. Straight-tuck at fold 0.5 is UNVERIFIED (no CLI sets stored `fold_state`) | P2 |
| 11 Studio (unticked) | [ ] | UNVERIFIED | manual QA only, per roadmap | - |
| 13, 14 hosted 3-OS evidence (unticked) | [ ] | UNVERIFIED | `gh run list`: recent main CI and Release v0.20.0 succeeded; no per-OS evidence for the v0.16 digests. No pinned cross-platform digest for a v11 fixture | P2 |
| Presets | [x] | PARTIAL | 8 presets exist; `docs/scene3d-v2.md` and `--help` list 7 (X3D-010) | P3 |
| `dependencies` for page-sourced graphic | [x] | PARTIAL | reports 0 edges for a `page:` source (only node-to-node counted); semantics undocumented | P3 |
| `graphic --source` help | [x] | PARTIAL | help omits the `page:ID` / `node:PAGE:NODE` syntax (X3D-010) | P3 |
| `ACCEPTANCE-AUDIT.md` | n/a | PARTIAL | stale: still claims a missing-slot diagnostic gap (diagnostic works), seven presets (eight), and a straight-tuck pinned overlap (clear at fold 1) | P3 |

## v0.17.0 design interchange

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| 1 Fidelity report schema, limits | [x] | WORKS | every import/export prints `schema_version 1` counts; `--report FILE` writes items | - |
| 2 SVG import: text, transforms, gradients, CSS, inheritance | [x] | WORKS | corpus files editable-text, geometry-transform, gradient-css, gradient-inheritance import and re-export | - |
| 2 SVG import: rectangular clip mask | [x] | BROKEN | `pentool import X rectangular-clip-mask.svg` on destinations of v4, v10, v11, v12: `[missing-mask] node "rectangular-clip-mask-svg-masked-e3962bc3" mask resource sha256:488d... missing`. `append_imported_document` merges image_assets and components but not `mask_resources`. Library test uses `import_design`, not the CLI path (X3D-006) | P1 |
| 3 PDF vector import | [x] | WORKS | export PDF from `examples/logo.pen`, `eye.pen`, `apple-logo.pen`, re-import: all exact, 4-10 nodes, re-export works. `eye` re-render has the same geometry at the same position; canvas is the destination default (1200x800 vs 1000x700), not a content defect | - |
| 4 PSD import | [x] | WORKS | `export eye.pen l.psd --layers` (14 MB), import: 18 exact / 2 approximated / 0 dropped, re-export PNG ok | - |
| 4 PSD export `--layers` vs flat | [x] | WORKS | layers file 14,000,418 B vs flat 2,800,040 B (differ); both report 1 exact / 8 approximated; `--report` items stay omitted unless asked | - |
| 5 Figma `.fig` v15 import | [x] | PARTIAL | `docs/fixtures/figma-weather-now-v15.fig` imports (744 nodes; 196 exact / 157 approximated / 50 dropped) and PNG export works, but the result cannot be listed: `pentool tree w.pen` and `search w.pen a` fail `Error: node not found: w-figma-instance-476635-9a1ef8c5a4-f8ac316a` (`tree --limit 1` works, `--limit 5` fails). Cause: `scene::node_bounds_on_page` returns None for an instance whose fallback group is empty (X3D-007) | P1 |
| 5 Figma REST JSON import | [x] | UNVERIFIED | covered by tests only; not run through the CLI | - |
| 6 Sketch import | [x] | WORKS | hand-built minimal `.sketch` imports with a report | - |
| 6 Sketch symbols/overrides | [x] | UNVERIFIED | no real symbol archive built | - |
| 7 Round-trip tests, fidelity table | [x] | PARTIAL | SVG, PDF, PSD round trips verified; Figma/Sketch persistence only in tests; `docs/design-interchange-v1.md` table exists | - |
| 8 CLI entry points, `--report` | [x] | WORKS | `import DEST SRC --report FILE`, export report in JSON | - |
| 8 `--prefix` | [x] | UNVERIFIED | not exercised | - |
| 8 Editor fidelity viewer | [x] | UNVERIFIED | browser UI not exercised | - |
| 9 Hostile archives | [x] | WORKS | Sketch zip with `../evil.json`: `[unsafe-path] archive entry "../evil.json" is not a safe relative path`; 400 MB entry in a 389 KB zip: `[limit-exceeded] ... exceeds the compression-ratio limit` | - |
| Hostile SVG, PSD, PDF, `.fig` beyond the above | [x] | UNVERIFIED | only the Sketch cases were run | - |
| Import then `tree`/`search` public agent API | AGENTS.md | BROKEN | see Figma `.fig` row | P1 |

## Totals

WORKS 36, PARTIAL 9, BROKEN 5 (v12 gate for pack mutations, v12 gate for `scene3d batch`, SVG mask CLI import, `.fig` tree/search, glass-crystal example), UNVERIFIED 8 (GPU parity, Studio/editor UI x3, hosted CI, straight-tuck fold 0.5, Figma REST CLI, Sketch overrides, `--prefix`). Counts are approximate where one roadmap line was split into several rows.
