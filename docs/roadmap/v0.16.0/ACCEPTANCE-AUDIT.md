# v0.16.0 acceptance audit (read-only, no build run)

Scope: tests/v0160_{bindings,migration,mockup,packaging,uv}.rs, tests/v0150_scene3d.rs,
src/scene3d/*.rs unit tests, src/server.rs browser_* tests. All v0160_*/v0150 files are
mounted by tests/suites/suite_12.rs:42-55. Tests were not executed here; status is from reading.
Line numbers are the `fn` line. B=v0160_bindings.rs, P=v0160_packaging.rs, M=v0160_migration.rs,
U=v0160_uv.rs, K=v0160_mockup.rs, N=src/scene3d/node.rs, PK=src/scene3d/pack.rs, S=src/server.rs.

## Acceptance targets

1. Saved doc with page+node bindings reopens identically; source edit updates mockup; unrelated pixels unchanged.
   PARTIAL-to-COVERED. B:3690 graphic_cli_add_set_list_remove_is_transactional (undo/redo/reload exact data);
   B:2065 generated_perfume_form_renders_on_the_cpu_path (unrelated page/node edits keep pixels at ~2091-2120,
   source-fill edit changes pixels ~2128-2143); N:3135 committed_source_changes_evict_only_page_and_node_consumers.
   Gap: no single test does save -> reopen -> edit source -> compare, and node-source (not only page-source) reopen equality is not separately asserted.

2. Perfume bottle: create, resize profile, body/cap materials, linked label; move label in 3D <-> UV island; source change updates both.
   PARTIAL. Create/profile/cap: B:8, B:248, B:307, B:342, B:390, B:3491 product_form_cli_add_is_transactional_and_renderable.
   Label+render: B:2065. 3D label drag: S:4305 browser_uv_face_pick..., graphic CLI B:3690.
   Gap: no test asserts "moving the decal changes UV placement AND moving the island changes visible placement" as one scenario.
   Studio drag/handles are only manual-QA claims in TASKS.md (task 5b/11), no automated browser test in repo.

3. Import arbitrary model, select UV islands, overlay/projection, linked decal/fill, deterministic seam/overlap/density diagnostics.
   COVERED (CLI/Rust). U:225 editable_uv_islands_are_transactional_and_survive_reload; U:143 uv_show_cli_reports_triangle_interior_overlap...;
   U:19 uv_island_pack_...; src/scene3d/uv.rs:1435 texture_density..., :1471 uv_discontinuities..., :1601 island_overlap_uses_triangle_interiors...,
   :1647 island_overlap_budget...; bindings.rs:2592 uv_island_surface_graphic_duplicates_only_owned_triangles; B:3690 (fill+decal);
   S:4305/S:4357 (island pick/highlight). Decal/fill on an *imported glTF/OBJ* is exercised via v0150 uv_project (tests/v0150_scene3d.rs:817).

4. Missing source / slot / invalid UV / cycle -> stable diagnostic, bytes unchanged.
   COVERED for source/cycle/UV; PARTIAL for missing slot. B:1948, B:1968, B:2009, B:2034; M:121 normative_v11_invalid_cases_are_rejected_without_mutating_the_fixture;
   bindings.rs:2673/2724; U:225 and uv.rs:1569 (bad island indices); byte-preservation in B:1163, B:1194, B:3690.
   Gap: no test named for an unknown material slot on `graphic add` (grep found none asserting a slot diagnostic) -- verify or add.

5. Cache hit/miss/invalidation keyed on every output field; bounded; disposable.
   COVERED. N:2842 png_cache_key_tracks_every_scene_and_output_setting..., N:3041 (byte-budget eviction), N:3083 (poisoned lock),
   N:3096/3135/3182/3213/3231 (targeted invalidation), tests/v0150_scene3d.rs:88 (restart discards cache), cli.rs:546 (cache CLI loopback-only).
   Note: cache is in-process only, so "disposable/rebuildable" is trivially true.

6. Same fixture + size -> same digest on all release targets.
   NOT COVERED as a test. Only same-machine determinism tests (N:3000 renders_a_cube_deterministically_and_caches, B:1313/B:1471 batch hash equality).
   No pinned PNG digest for a v11 fixture in any test; docs/performance.md:295-316 records digests for benchmark runs only, Windows-measured.
   Cross-platform equality depends on hosted CI matrix, which has no digest assertion. Needs a pinned-digest test run on 3 OS.

7. Effective ppi correct; preflight threshold; PDF/X and separations include 3D with documented color.
   COVERED. B:1194 (render-report), B:1249 print_preflight_reports_scene3d_ppi_after_final_transform_and_threshold, P:1911 (working space+intent),
   P:734 (PDF/X fallback+intents), P:1980 product_form_mockup_is_included_in_process_color_separations, P:438 strict spot, P:1867 stack order, B:733 PDF/X draft.

8. Every built-in template has stable panel/face/UV map; no unintended gaps/overlaps flat/intermediate/folded; panel<->face selection both ways.
   PARTIAL. Gaps: PK:6834 every_builtin_..._at_minimum_dimensions_and_fold_states, PK:8028 all_builtin_rectangular_panel_hinges..., PK:8098, PK:7886.
   Overlaps: PK:6957 flat nets no collisions, PK:6988 collisions stable across preview states, PK:7035 -- which PINS straight-tuck as having two unresolved
   dust-flap/lid overlaps (needs-review). So "no unintended overlaps" is explicitly not met for straight-tuck. Tube/bottle-label are cylinder meshes
   (PK:8210) and are not in the overlap sweeps. Selection: S:4865 face-pick -> panel, S:4890 panel -> highlight (pixel diff). UV map: P:539, P:616.
   Not proven: UV-rect stability across all seven templates (no table-driven test of panel/face/UV identity for every template).

9. Cylindrical label: reproducible seam, measured circumference, printable bleed, consistent 3D/flat.
   COVERED (as visual mockup). PK:8210 tube_label_maps_its_print_width_to_a_closed_cylindrical_surface, P:202 cylindrical_wrap_settings_update_the_shared_flat_and_folded_mapping_atomically,
   PK:7296/7338/7380 bleed (cut edges only, not seams), B:543 SVG round-trip. Bottle-label does not conform to tapered bottles (documented).

10. Apparel carton: edit flat, view folded, export with print lines separated.
    COVERED for export; PARTIAL for "edit flat". P:1469 packaging_print_bundle_separates_cut_and_crease_guides_from_artwork, B:444, B:897, P:315 (layout path role -> plate),
    PK:6728 editable_layout_page_changes_the_shared_dieline_geometry, P:539. "shirt-carton" fixture id used B:449+. Flat editing of artwork is by linked source page, not on the dieline page (TASKS 8a says so).

11. Handoff: folded vs flat compare; separate cut/crease/perforation; bleed/safe distinct; plainly draft/reference-only.
    COVERED. P:1469 (CutContour/Crease/Perforation plates, CUTTING-MAP reference-only legend, PRINT-GUIDE, verify-bundle tamper rejections), B:897, P:1117 (ink-name merge refusal).
    Draft labelling: manifest `manufacturing_certified:false` asserted in bundle tests.

12. Panel artwork update -> folded preview; flat export keeps bleed, separates roles; package verifies and lists dependencies.
    COVERED/PARTIAL. PK:7749 carton_exterior_panel_overlay_is_visible_in_cpu_render, B:2400, P:616, PK:7296 (bleed), P:1469 (verify-bundle). "reports all external/embedded
    dependencies" -- no test found asserting a dependency list in the bundle manifest; confirm manifest fields.

13. Batch output names/order/JSON/bytes deterministic.
    COVERED. B:1313 (sha256 per item, order), B:1559, B:1761, B:1837 (atomic cancel), B:1471 (two runs equal).

## Open tasks: can they be ticked? (draft/reference-only output is allowed by acceptance text)

Task 8 (parametric geometry). Recommend: DO NOT TICK yet; ticking is defensible only after narrowing the checkbox text.
 Blockers: (a) "unsupported non-developable shapes / impossible panel combos / self-intersections validated before mutation" -- panel overlap and SVG
 self-intersection are covered (B:2034, B:543, P:1315), but no explicit non-developable refusal test found; (b) text lists bottle-label wrap "conforms to profile" gap
 and states "Other non-rectangular templates ... remain unimplemented"; (c) text itself says "visual mockups, not production dielines". Deferred section does not
 defer these. Honest tick = reword to "visual/draft parametric templates" and cite PK:6834.

Task 8a (dielines as design surfaces). Recommend: NOT YET. Remaining per its own text: "general freeform manufacturing crease editing remains open",
 direct artwork editing on the dieline page (art stays in paired source page), profile-conforming bottle labels, tube packaging ends, production pouch construction.
 Apparel/T-shirt box is satisfied by shirt-carton fixture. Could tick if editing-of-art-on-dieline-page is declared out of scope in the text.

Task 9 (flat/folded sync). Recommend: NOT TICKABLE. Verify "no face gaps/overlaps across all built-in templates" is demonstrably false for straight-tuck (PK:7035 pins 2 overlaps;
 TASKS.md:798-812 admits it). Needs thickness-aware flap layering fix or an acceptance rewrite that scopes straight-tuck out as `needs-review`. Everything else
 (S:4865, S:4890, PK:6988, P:539) is present.

Task 9a (unified UV/print mapping). Recommend: MOSTLY TICKABLE for tube/bottle-label and rectangular panels, after removing the dangling sentence "Bleed on" (TASKS.md:861-862,
 text truncates mid-sentence) . Remaining gaps: "complete dieline/UV export parity" (TASKS.md:875) -- there is no test that UV-layout export (`uv layout`) and flat dieline export read
 the same stored map for a dieline-backed node; seam-crossing warning exists only as a conservative bounds check (P:202 area), multi-subpath/hole cut contours rejected.

Task 10 (packaging CLI/print export). Recommend: TICKABLE as draft/reference handoff once stale text is fixed. Implemented: templates, import, bind, crease, settings, preview, preflight,
 export (--pdfx), export-bundle, verify-bundle, separate, vector-separate; dry-run + --if-revision (P:1355, P:975); failure-atomicity (P:1469, B:897).
 Remaining nits: candidate command block (TASKS.md:897-905) still shows `pack create`, `pack preview DOC carton --out`, and says "not an existing CLI promise" -- actual verbs are
 `pack sleeve|carton|pouch|tube|bottle-label`, no `pack create`; deterministic output names are for bundle only. "Editable dieline import, production validation, certified output remain open" (TASKS.md:1000) conflicts with the Explicitly-deferred list; reword.

Tasks 11, 13, 14 stay open per own text: no automated browser tests (Studio claims are manual Chromium QA), no hosted 3-OS CI evidence, no tagged release artifact.

## Docs / help sync findings

- CLI names verified against src/main.rs:2113-2150, 7107-7160 and src/scene3d/cli.rs: `scene3d DOC <graphic ID|form|pack|uv ID|add|camera|...>`, `scene3d-cache`, `mockup list|apply`, `pack [DOC] <action>` all exist. README:78-120 spellings match
  the clap grammar (pack templates without DOC handled at cli.rs:917; verify-bundle at :908).
- STALE: preset count. cli.rs `form add --preset` help (cli.rs ~line 187) and docs/scene3d-v2.md:21 list SEVEN starters, TASKS.md 5a/6 say EIGHT; src/scene3d/form.rs:37 has
  `cylinder-container` which is missing from the help text and from the v2 doc/Studio claim. Decide the number and sync help + docs + README (task 6 "every one of the eight" test is K:221).
- STALE: TASKS.md task 6 candidate commands (`texture bind`, `form create`, `uv project --kind/--region/--set`, `graphic add --region`, `mockup apply ... --x --y`) are not the shipped spelling
  (`graphic ID add --slot`, `form add --preset`, `uv project --projection`, panel art via `pack bind`). Text says the final grammar is frozen; the old block should be removed or labelled historical.
- README:104 `pentool pack products.pen import ...` vs README:99-115 use `scene3d ... pack`; both valid, but README:118 `pentool pack templates` followed by :119 `pentool pack DOC inspect` is fine. No `pentool pack --help` test in tests/cli_help.rs (grep: no scene3d/pack/mockup there) -- help examples unguarded.
- README:137 and :136 reference `docs/scene3d-batch-queue.schema.json` (exists). README does not mention `pentool migrate FILE --target 11` or pen-format-v11 at all.
- docs/pen-format-v11.schema.json: referenced from docs/scene3d-v2.md:3 and :290 only. NOT referenced from README.md, docs/pen-format-v4.md, docs/pen-format.md, docs/studio.md, or docs/PROTOCOL.md (grep). Recommend adding a README link next to the v11 3D contract link (README:178).
- docs/scene3d-v2.md:53 ends "GPU preview, explicit source-color-role management, and full workspace editing are also outstanding." -- fine, but the same paragraph is a ~4 KB single line; consider splitting.
- TASKS.md structural defects: task 9a text truncated "Bleed on" at lines 861-862 (sentence never finishes); task 5b has a dangling "`island pack` command" fragment at line 360; task 8 header mixes status/history.
- ci/release smoke: .github/workflows/release.yml:140-148 covers dependencies, render-report, pack preflight/preview/export/export-bundle/verify-bundle (bindings + mockup render + packaging preview + flat export: satisfied for task 14 smoke requirement).
- ci.yml has only `benchmark --scene3d` (ci.yml:73); no gpu-feature or scene3d-specific job beyond generic tests.
