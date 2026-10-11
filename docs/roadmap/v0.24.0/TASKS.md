# v0.24.0 — Collaboration

Let several people and agents work on one document safely, without giving up
the single binary, readable `.pen` files and atomic history.

## Product contract

- The `.pen` document remains the source of truth. Collaboration is a protocol
  over operations, not a different storage format.
- Edits are operations with stable IDs and revision guards. Concurrent edits
  merge deterministically or are rejected with a named conflict; nothing is
  silently overwritten, and locks are respected.
- Works offline: a peer can work disconnected and reconcile later. No central
  service is required to open or save a document.
- Identity and access are explicit. A session shows who may read, comment or
  edit, and every operation records its author.
- Bounded: operation size, rate, session size and history are limited, and
  limits are checked before work starts.
- Comments and presence data are separate from the scene graph and never change
  rendering or export.

## Must ship, in order

- [ ] **1. Specify the operation log,** merge rules and conflict reporting in
  `docs/collaboration-v1.md`, with a threat model.
- [ ] **2. Operation log in the transaction path,** with deterministic replay
  and a property-based convergence test.
- [ ] **3. Local-network sync** between instances (`serve --collab`), with
  authentication tokens and explicit peers.
- [ ] **4. Comments, mentions and review states** stored outside the scene.
- [ ] **5. Presence and cursors,** best-effort and non-persistent.
- [ ] **6. Permissions** (viewer, commenter, editor) enforced by the server.
- [ ] **7. Editor:** collaborator list, comments panel, conflict resolution UI.
- [ ] **8. Evidence:** convergence fuzzing, hostile-peer tests, and recovery
  tests for crash during sync.

## Explicitly deferred

- A hosted relay or account system (see v0.25.0), end-to-end encrypted rooms,
  and real-time co-editing of a single text run at keystroke granularity.
