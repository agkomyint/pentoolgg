# Audit: extensions, design systems, collaboration, architecture, Studio

Auditor: read-mostly subagent. Binary under test: installed pentool v0.20.0. Repros ran on temp copies
outside the repo. Nothing was built; no source, test or roadmap file was edited.

Verdicts: WORKS (exercised or covered by a passing test and matches the promise), PARTIAL (works with a
documented or found gap), BROKEN (does not do what is promised), UNVERIFIED (not exercised).
Severity: P0 data loss/security hole, P1 feature unreachable or integrity risk, P2 wrong/silent behavior,
P3 polish/docs.

## Summary

WORKS 14, PARTIAL 9, BROKEN 3, UNVERIFIED 6 (architecture tasks 1-8 counted as one UNVERIFIED row).

## v0.18.0 scripting and extensions

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| Install/list/inspect `.penext` | done | WORKS | Lifecycle run on docs/fixtures/v4-scene.pen; tests/v018_extension.rs | - |
| Sandboxed WASM run via shared planner and transaction | done | WORKS | `extension run` produced batch ops through the planner; dry run and commit agree; failed run left doc unchanged | - |
| Fuel/limits and approval hash | done | WORKS | Covered in tests/v018_extension.rs; approval mismatch refused | - |
| Legacy (v1-v3) document with extension | n/a | PARTIAL | `extension run` on legacy logo.pen returns [unsupported-capability]; message does not tell the user to migrate | P3 |
| CLI help for `--values` / `batch` | documented as inline JSON | BROKEN | `--values '{json}'` and `batch '[json]'` fail with os error 123 (they take file paths). Help text says otherwise | P3 |

## v0.19.0 / v0.19.1 design systems, brand kits, templates

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| Tokens (`design token`) | done | WORKS | Live flow; tests/v019_design.rs | - |
| Library packaging, `pentool.lock` | done | PARTIAL | Works, but lock writes go through `transactional_write` and leave `NAME.bak.N` siblings (src/asset.rs:371, src/editing.rs:160), against the no-adjacent-backups rule | P2 |
| Brand kits | done | WORKS | Live flow | - |
| Templates fill/validate | done | WORKS | Fill validation exercised, bad values rejected | - |
| `design audit` rules | done | PARTIAL | `spacing` rule is listed in RULES (src/design/audit.rs:21) but never yields a finding; documented only as Q8 | P2 |
| `--fix-plan` bind-token ops | done | WORKS | Plan applied via batch | - |
| `tree/search --resolve`, `validate --strict` | open (v0.19.0 unticked) | UNVERIFIED | Listed under "Not done" in docs/design-systems-v1.md; not shipped | P3 |
| v0.19.1 responsive / accessibility items | unticked | UNVERIFIED | Not exercised; need a live browser | P3 |
| docs/design-systems-v1.md header | accurate | BROKEN | Header says no command below has shipped, which is stale | P3 |

## v0.20.0 collaboration

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| Hash-chained op log, init, verify | done | WORKS | `collab init`, `collab verify` ok; tests/collab_evidence.rs | - |
| Two-instance sync (HTTP+SSE) and convergence | done | WORKS | Two loopback instances converged; tests/collab_converge.rs, collab_sync.rs | - |
| Named conflicts (`overwritten-edit`) | done | WORKS | Conflict detected and listed after concurrent edit | - |
| Token roles viewer/commenter/editor/owner, revoke | done | WORKS | Role matrix script across routes; `permission_matrix_is_enforced_on_every_route` | - |
| Hostile peer containment | done | WORKS | hostile.py repro plus `hostile_peer_is_contained` | - |
| Claims and release | done | WORKS | Exercised live | - |
| Presence | done | WORKS | tests/collab_presence.rs and live | - |
| Stats (human/agent shares, by kind/page/actor) | done | WORKS | `stats --by kind|page` live, basis points consistent; tests/collab_stats.rs | - |
| Stats after an external hand edit | n/a | PARTIAL | After editing the .pen outside Pentool, `verify` reports `matches_document: false` and stats show 0 ops; no guidance to catch up | P3 |
| Comments | done | BROKEN | Author comes from the request body `actor` (src/comments.rs `actor_from` ~671, `apply` ~680; src/server.rs ~4404). A client can post as any identity | P1 |
| Sync push of local ops | done | PARTIAL | src/collab/sync.rs ~1684-1745 filters `op.actor == session_actor.id`, skips others silently, and still advances `pushed_to`; skipped ops never retried | P2 |
| Crash during sync resumes | done | WORKS | `crash_during_sync_resumes_to_the_same_bytes` | - |
| `serve --host 0.0.0.0` without `--collab` | n/a | PARTIAL | Binds all interfaces with no auth; no warning | P2 |
| Item 3a: `collab compact` | open | UNVERIFIED | Not implemented (collaboration-v1.md 13a-13c) | P2 |
| Item 3a: stale-base rejection | open | UNVERIFIED | Base staleness not enforced | P2 |
| Item 3a: Ed25519 signing | open | UNVERIFIED | Not implemented | P2 |
| Item 7: `collab resolve` / conflict UI | open | UNVERIFIED | Not implemented; studio.md says one-click resolution is missing | P2 |
| Cursors | open | UNVERIFIED | Not implemented (studio.md) | P3 |
| `summary.json` | open | UNVERIFIED | Not implemented | P3 |

## Studio

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| Every workspace in workspaces.ts has a panel | yes | BROKEN | `collab`, `stats`, `systems` have panels in `WorkspaceBody`, but the selector (web-ui/src/App.tsx ~319) renders only `WORKFLOWS` (~51-62); no other path calls `setWorkspace` to them. Unreachable | P1 |
| web/studio.html matches fresh `bun run build` | yes | WORKS | Fresh LF output equals the HEAD blob (a CRLF difference on disk is a checkout artifact); file restored | - |
| Panels exercised in a live browser | stated not done | UNVERIFIED | studio.md admits it; not exercised here | P3 |
| Each src/server.rs route documented in docs/PROTOCOL.md | yes | BROKEN | About 71 of 86 routes are not mentioned; list in scratchpad audit-ext/routes.txt | P2 |

## docs/roadmap/architecture/TASKS.md

| Item | Claimed | Verdict | Evidence | Sev |
|---|---|---|---|---|
| Tasks 1-8 (WASM spike, core crate, /api/v1, live canvas) | not started | UNVERIFIED | Not deeply checked; AGENTS.md says no crate split before the spike decision record. Treat as not started | - |
