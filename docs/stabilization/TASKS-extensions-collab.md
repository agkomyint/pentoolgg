# Tasks: extensions, design systems, collaboration, Studio

Class: PATCH-SAFE (fix in a patch release) or DECISION (needs maintainer decision / feature work, not for
a stabilization patch). Test file names are proposals.

## EXT-001 Collab, Stats and Systems workspaces unreachable (P1, PATCH-SAFE)
- Repro: open Studio; the Workspace select (web-ui/src/App.tsx ~319) lists only the `WORKFLOWS` entries.
- Expected: every workspace in web-ui/src/lib/workspaces.ts with status "available" is selectable.
- Actual: `collab`, `stats`, `systems` are unreachable; their `WorkspaceBody` cases are dead.
- Suspects: App.tsx `WORKFLOWS`, `WorkspaceBody`, store.ts `setWorkspace`.
- Test: web-ui test (or tests/v019_design_ui.rs) `every_available_workspace_is_selectable`.
- Accept: each available workspace reachable from the selector; rebuilt web/studio.html committed.

## EXT-002 Comment author forgery (P1, PATCH-SAFE)
- Repro: POST a comment to the comments API with an arbitrary `actor {kind,id,name}` under a commenter token.
- Expected: identity comes from the token, as for collab ops (`identity_comes_from_the_token_and_forgery_is_refused`).
- Actual: author is the body `actor` (src/comments.rs `actor_from`, `apply`; src/server.rs `comments_apply`).
- Test: tests/comments.rs `comment_author_comes_from_the_token`.
- Accept: with a collab token the body actor is ignored or a mismatch is refused; without collab behavior is unchanged and documented.

## EXT-003 Sync push silently skips ops and advances the cursor (P2, PATCH-SAFE)
- Repro: sync with a local actor different from the token actor; ops by the other actor are never pushed.
- Expected: a skipped op is reported and the cursor does not pass it, or the mismatch is an error.
- Actual: filter `l.op.actor == session_actor.id` then `pushed_to = pushed_to.max(local_head)` (src/collab/sync.rs ~1684-1745).
- Test: tests/collab_sync.rs `push_does_not_skip_foreign_actor_ops_silently`.
- Accept: report lists every skipped op in `push_rejections`; cursor stays behind the first unpushed op.

## EXT-004 Unauthenticated `serve --host 0.0.0.0` (P2, DECISION)
- Repro: `pentool serve --host 0.0.0.0` without `--collab`.
- Expected: refuse or warn loudly on a non-loopback bind without auth.
- Actual: silent bind to all interfaces.
- Suspects: serve argument handling in src/main.rs / src/server.rs.
- Test: tests/server_bind.rs `non_loopback_bind_requires_ack`.
- Accept: maintainer picks refuse vs warn; behavior documented in README.

## EXT-005 Lock/package writes leave `.bak.N` siblings (P2, PATCH-SAFE)
- Repro: run library/package commands that rewrite pentool.lock or .penpkg; look for `NAME.bak.N`.
- Expected: no adjacent backups (AGENTS.md); bounded hidden history.
- Actual: `atomic_new_replace` (src/asset.rs ~371) calls `editing::transactional_write` (src/editing.rs ~160), cap 999; callers library.rs:98, package.rs:583, 679, 757, 972.
- Test: tests/v019_design.rs `library_install_leaves_no_bak_files`.
- Accept: no `.bak.*` files after any of these commands; failure still leaves target byte-identical.

## EXT-006 `spacing` audit rule is a no-op (P2, PATCH-SAFE)
- Repro: `design audit` on a document with off-grid spacing; rule `spacing` never reports.
- Expected: finding, or the rule is not advertised.
- Actual: listed in `RULES` (src/design/audit.rs:21) yet yields nothing.
- Test: tests/v019_design.rs `audit_spacing_rule_reports_or_is_unlisted`.
- Accept: either implemented or removed from RULES/help/docs with an explicit "unsupported" error when requested by name.

## EXT-007 PROTOCOL.md missing about 71 of 86 routes (P2, PATCH-SAFE)
- Repro: compare routes in src/server.rs with docs/PROTOCOL.md (list: scratchpad audit-ext/routes.txt).
- Expected: every route documented (method, auth/role, body, response).
- Test: tests/protocol_docs.rs `every_server_route_is_documented` (parse router, grep doc).
- Accept: test passes; collab routes list roles from `perm::ROUTES`.

## EXT-008 `--values` and `batch` help says inline JSON, takes file path (P3, PATCH-SAFE)
- Repro: `extension run ... --values '{"a":1}'` and `batch doc '[...]'` give os error 123.
- Expected: help matches behavior (or inline JSON accepted).
- Test: tests/v018_extension.rs `values_help_matches_behavior`.
- Accept: help and README examples runnable as written; error says "expects a file path".

## EXT-009 Stale docs/design-systems-v1.md header (P3, PATCH-SAFE)
- Header says no command below shipped; commands shipped in v0.19.0. Update; keep "Not done" list.
- Test: none (docs). Accept: header states shipped status and links Q8.

## EXT-010 `extension run` on legacy documents gives unhelpful error (P3, PATCH-SAFE)
- Repro: `extension run` against logo.pen (v1-v3) yields [unsupported-capability].
- Expected: message names the migration command.
- Test: tests/v018_extension.rs `legacy_doc_error_names_migration`.

## EXT-011 External edit leaves collab log behind with no guidance (P3, PATCH-SAFE)
- Repro: `collab init`, edit the .pen by hand, `collab verify` then `stats`.
- Actual: `matches_document: false`, stats show 0 ops, no hint.
- Expected: warning text tells the user how to catch up.
- Test: tests/collab_evidence.rs `verify_hints_on_external_edit`.

## EXT-012 `collab compact` missing (P2, DECISION)
Known gap (collaboration-v1.md 13a-13c, item 3a). Feature work; log grows unbounded. Needs design decision on snapshot format and verify interaction. Test when built: tests/collab_sync.rs `compact_preserves_verify_and_convergence`.

## EXT-013 Base staleness not enforced (P2, DECISION)
Known gap (item 3a). Ops built on a stale base are accepted. Needs policy (reject vs rebase). Test when built: tests/collab_sync.rs `stale_base_op_is_rejected`.

## EXT-014 No Ed25519 op signing (P2, DECISION)
Known gap (item 3a). Identity rests on bearer tokens only. New dependency review needed per AGENTS.md. Test when built: tests/collab_evidence.rs `unsigned_op_is_refused_when_signing_required`.

## EXT-015 No conflict-resolution command or UI (P2, DECISION)
Known gap (item 7): no `collab resolve`, no one-click resolution in Studio (studio.md). Conflicts are detected and listed only. Test when built: tests/collab_converge.rs `resolve_conflict_keeps_chosen_value_on_all_peers`.

## EXT-016 No live cursors (P3, DECISION)
Known gap; presence exists but cursors do not. Test when built: tests/collab_presence.rs `cursor_position_is_shared`.

## EXT-017 No `summary.json` (P3, DECISION)
Known gap in item 3a notes. Test when built: tests/collab_stats.rs `summary_json_matches_stats`.

## EXT-018 Studio panels never exercised in a live browser (P3, PATCH-SAFE)
studio.md admits it. After EXT-001, do a manual or automated browser pass over Collaboration, Stats, Systems and the v0.19.1 responsive/accessibility items; record the result in docs/studio.md.

## EXT-019 `tree/search --resolve` and `validate --strict` unshipped (P3, DECISION)
Listed under "Not done" in design-systems-v1.md; v0.19.0 boxes stay unticked until built or descoped.

## Architecture tasks 1-8
Not audited in depth; treated as not started. Keep the WASM spike decision record as the gate.
