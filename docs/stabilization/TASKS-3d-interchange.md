# Tasks: 3D and design interchange (from audit-3d-interchange.md)

Hand-off for the fixing agent. Binary audited: installed 0.20.0. Do not run the full
suite while iterating; use `cargo test --test <file> --locked <filter>`.
"Patch-safe" means it fixes documented behavior and can ship in a 0.20.x patch.

## Defects

### X3D-001 (P1) Shipped example fails to export: render buffer limit
- **Status: fixed in 0.20.1.** Reproduced. The example now uses `supersample: 2` (fits the 256 MiB budget, limit unchanged); the regenerated snapshot differs only in edge antialiasing (3518 of 1,080,000 px, 98% on silhouette edges). Test: `tests/v0150_scene3d.rs::every_shipped_example_exports`.
- Repro: `pentool export examples/v015-glass-crystal/glass-crystal.pen out.png`
- Expected: PNG written. Actual: `[limit-exceeded] render buffers need 393120000 bytes; the limit is 268435456` (supersample 3 at 1200x900; `--scale` does not reduce it). Workaround: `scene3d set crystal --supersample 2`.
- Suspect: `MAX_RENDER_BUFFER_BYTES` (256 MiB) in `src/scene3d/render.rs` (line ~18, check ~771), added in commit 0420659.
- Test: new `tests/v0150_scene3d.rs::every_shipped_example_exports` rendering all `examples/**/*.pen` that contain `scene3d`.
- Acceptance: example exports; the limit still protects against hostile sizes; docs name the limit and how to lower it.
- Patch-safe: yes (lower the example's supersample or reduce buffer cost). Raising the limit needs a maintainer decision.

### X3D-002 (P1) v11 gates reject v12 documents
- **Status: fixed in 0.20.1.** Reproduced. All `== Some(11)` gates in `scene3d/{pack,bindings,cli,node}.rs` and `server.rs` accept v11 or newer; messages say "v11 or newer". Test: `tests/v0160_packaging.rs::pack_mutations_accept_v12_documents` (crease, carton, bind on v12, and v11/v12 mockup renders byte-identical). A separate `scene3d batch` v12 case in `v0160_mockup.rs` is still open.
- Repro: `pentool migrate doc.pen --target 12` on a doc with a carton, then `pack carton`, `pack sleeve`, `pack crease`, `pack bind`, `scene3d batch`.
- Expected: accepted (v12 is a superset). Actual: `[unsupported-capability] package mockups require document format v11`, "per-crease editing needs document format v11; upgrade the document first" (misleading), "panel artwork requires document format v11", "batch mockup rendering requires document format v11". SVG gradient import silently migrates a destination to v12, after which packaging edits are refused.
- Suspect: `raw.get("version") ... == Some(11)` in `src/scene3d/pack.rs` (4413, 4806, 4847, 4971, 5119), `bindings.rs` (26, 69, 221, 403, 451), `cli.rs` (1803, 2102), `node.rs` (368, 2377: render-time dependency plan skipped on v12). `require_v11` at `cli.rs:1423` already uses `>=`.
- Test: `tests/v0160_packaging.rs::pack_mutations_accept_v12_documents`, plus a v12 case for `scene3d batch` in `tests/v0160_mockup.rs` and a render of a bound graphic on v12.
- Acceptance: every v11 mutation and render path accepts version >= 11; error text says "format v11 or newer"; v11 and v12 renders of the same scene are byte-identical.
- Patch-safe: yes.

### X3D-003 (P2) Raster-image environment not implemented
- Roadmap v0.15 Model section and task 4 promise a document raster as backdrop and IBL source and show `environment: {kind: color}`. Shipped: sky/ground gradient only.
- Suspect: `src/scene3d` environment model, `docs/scene3d-v1.md`.
- Test (if implemented): `tests/v0150_scene3d.rs::image_environment_lights_scene`.
- Acceptance: either implemented with docs, or the roadmap text is reworded to match shipped behavior.
- Patch-safe: no. Maintainer decision (new feature vs reword).

### X3D-004 (P2) `scene3d uv show` is unbounded and unpaginated
- Repro: `pentool scene3d uv doc.pen hero show` on a 3-primitive model returns about 220 KB with no `limit`, `offset`, `returned`, `has_more`.
- Expected: compact, paginated output per roadmap task 6a and AGENTS.md.
- Suspect: uv subcommand in `src/scene3d/cli.rs` and its inspect module.
- Test: `tests/v0150_scene3d.rs::uv_show_is_paginated` asserting `limit`/`offset`/`returned`/`has_more` and a bound on bytes.
- Acceptance: `--limit/--offset`, default limit, accurate pagination fields; existing JSON fields retained.
- Patch-safe: yes if existing fields are kept (default limit may be a compatibility call; maintainer to confirm).

### X3D-005 (P2) No pinned acceptance goldens for reference sphere, textured cube, PBR grid
- Expected: byte-identical digests pinned in tests (roadmap acceptance, v0.15 task 10).
- Actual: no such fixtures or digest tests found by grep.
- Test: `tests/v0150_scene3d.rs::acceptance_scene_digests` using small fixtures in `docs/fixtures/`.
- Acceptance: digests committed and passing on all 3 CI OSes. Hosted CI is the authority.
- Patch-safe: yes (test only); never refresh a golden blindly.

### X3D-006 (P1) CLI import of SVG with a rectangular clip mask fails
- **Status: fixed in 0.20.1.** Reproduced. `append_imported_document` merges `mask_resources` (dedupe by digest, conflict error, `MAX_MASKS` across the merged document). Test: `tests/v017_interchange.rs::cli_import_of_rectangular_clip_mask_svg_validates`.
- Repro: `pentool new d.pen; pentool import d.pen docs/fixtures/.../rectangular-clip-mask.svg` (also on v4/v10/v11/v12 destinations).
- Expected: imported. Actual: `[missing-mask] node "rectangular-clip-mask-svg-masked-e3962bc3" mask resource sha256:488d... missing`.
- Suspect: `append_imported_document` in `src/interchange.rs` (~601-860) merges `image_assets` and `components` but not `mask_resources`.
- Test: `tests/v017_interchange.rs::cli_import_of_rectangular_clip_mask_svg_validates` (via the CLI path, not `import_design`); also assert mask dedupe on repeated import.
- Acceptance: import succeeds, document validates, PNG export shows the clip; the 256-mask cap is enforced across the merged destination.
- Patch-safe: yes.

### X3D-007 (P1) `.fig` import cannot be listed with `tree` or `search`
- **Status: fixed in 0.20.1.** Reproduced. A found node without drawable geometry reports zero-size bounds at its world origin. Tests: `tests/v017_interchange.rs::figma_fig_import_is_searchable_with_tree_and_search`, unit `scene::empty_group_reports_zero_size_bounds_at_its_origin`.
- Repro: `pentool import w.pen docs/fixtures/figma-weather-now-v15.fig; pentool tree w.pen` (or `search w.pen a`).
- Expected: compact tree. Actual: `Error: node not found: w-figma-instance-476635-9a1ef8c5a4-f8ac316a` (`--limit 1` works, `--limit 5` fails).
- Suspect: `node_bounds_on_page` in `src/scene.rs` (~618-782), `local()` returns None for an empty-children group or an instance with an empty fallback group; called from search/tree (~3921).
- Test: `tests/v017_interchange.rs::figma_fig_import_is_searchable_with_tree_and_search` plus a unit case for an empty group and an empty-fallback instance in `src/scene.rs`.
- Acceptance: tree/search succeed on the fixture; empty groups report zero-size bounds deterministically; pagination fields stay exact.
- Patch-safe: yes (the empty-bounds representation must be chosen consistently with the schema).

### X3D-008 (P2) Missing cross-platform digest for a v11 fixture, v0.16 evidence
- Roadmap tasks 13 and 14 need hosted 3-OS evidence. Add a pinned digest test for a small v11 mockup fixture. Test: `tests/v0160_mockup.rs::v11_fixture_digest_is_pinned`. Patch-safe: yes (test). Task checkboxes stay unticked until CI evidence exists.

### X3D-009 (P2) Folding geometry overlaps (v0.16 task 9)
- `pack carton --template mailer` + `pack preflight` at fold 1: 1 collision (lid vs glue-joint, zero-thickness-intersection). Roadmap already admits it. Straight-tuck at fold 0.5 could not be checked (no CLI to set stored fold state).
- Maintainer decision: fix geometry or keep documented limitation. Keep task 9 unticked.

### X3D-010 (P3) Doc/help drift (v0.16)
- `docs/scene3d-v2.md` and `--help` list 7 presets; 8 exist. `graphic --source` help omits `page:ID` / `node:PAGE:NODE`. README does not mention `migrate --target 11`. `docs/roadmap/v0.16.0/ACCEPTANCE-AUDIT.md` is stale (missing-slot gap, 7 presets, straight-tuck overlap). `v0.16.0/TASKS.md` contains stale candidate commands and truncated text ("Bleed on"). Patch-safe: yes.

### X3D-011 (P3) `dependencies` omits page-sourced graphic edges
- `scene3d dependencies` reports 0 edges for a `page:` source. Document intended semantics or include the edge. Maintainer decision.

### X3D-012 (P3) Roadmap v0.15 CLI direction differs from shipped spellings
- `uv show --limit`, `uv project --mesh --kind --set`, `uv layout --mesh` are not the shipped flags (`add` takes `--model --layer ID`). Update the roadmap or add aliases. Patch-safe for docs.

### X3D-013 (P3) PDF re-import canvas size
- Re-importing an exported PDF into a fresh doc keeps geometry but lands on the destination's default canvas (1200x800 vs original 1000x700). Probably intended; confirm and document.

## Checkboxes to change

- v0.15.0 task 4 (PBR, lights, environment): untick or reword the roadmap, because image-based environment is not implemented (X3D-003).
- v0.15.0 task 5: keep ticked only after X3D-001 is fixed; shipped example does not export today.
- v0.15.0 task 6a: keep ticked only after X3D-004 (pagination).
- v0.15.0 task 10: acceptance goldens lack pinned digests (X3D-005); untick the "cross-platform golden renders" claim until added.
- v0.15.0 task 7: leave ticked but the byte-parity claim is unverified here (installed binary lacks `gpu`); requires a hosted `gpu` build.
- v0.16.0 tasks 8, 8a, 9, 11, 13, 14: leave unticked; evidence matches their own text. Task 9 still has the mailer overlap.
- v0.16.0 ticked tasks involving `pack`/`batch` mutations: they work only on v11 documents; treat as broken on v12 until X3D-002 is fixed.
- v0.17.0 task 2 (SVG import hardening): untick or caveat until X3D-006 is fixed (CLI import of the clip-mask corpus file fails).
- v0.17.0 task 5 (Figma): untick or caveat until X3D-007 is fixed (imported `.fig` cannot be browsed with `tree`/`search`).
- v0.17.0 tasks 1, 3, 4, 6, 9: WORKS per CLI evidence; leave ticked.
- v0.17.0 tasks 7, 8: leave ticked; Figma REST JSON, Sketch symbols/overrides, `--prefix`, and the editor viewer remain UNVERIFIED by this audit.

## Suggested order

X3D-002, X3D-006, X3D-007 and X3D-001 first (all P1, all patch-safe), then X3D-004, X3D-005, X3D-008, then the P3 doc fixes. X3D-003, X3D-009 and X3D-011 need a maintainer decision.
