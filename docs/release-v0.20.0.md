# Pentool v0.20.0 — Collaboration

Multiple people and agents can now work on one `.pen` and converge.

- **Operation log and merge** (`docs/collaboration-v1.md`): content-hash op ids, hybrid logical clock, hash-chained append-only log, per-property last-writer-wins, fractional child order, delete-vs-edit and cycle handling, named conflict reports. `pentool collab init|verify|replay|log|strip`.
- **Local-network sync**: `pentool serve --collab`, `pentool collab peer|sync|join` over plain HTTP plus server-sent events; hashed bearer tokens; loopback-only unless `--host` with tokens.
- **Roles enforced by the server**: viewer, commenter, editor, owner; every route is classified and a test fails for unclassified routes.
- **Actor identity and contribution stats**: humans versus agents, `pentool stats DOC [--by actor|kind|page]`, a compact summary embedded in the `.pen` (stripped from packages unless `--include-attribution`).
- **Comments, mentions and review states** stored outside the scene.
- **Presence** (best-effort, in memory only): `pentool collab presence`.
- **Studio**: Collab workspace with collaborators, comments, conflict viewer and stats panel.
- **Evidence**: convergence, hostile-peer, crash-recovery tests and four fuzz targets; stale-lock and temp-file recovery.

Known gaps: no Ed25519 op signing, no stale-base rejection for agents, no `collab compact`, no one-click conflict resolution or cursors in Studio.
