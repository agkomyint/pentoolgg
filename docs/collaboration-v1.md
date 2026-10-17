# Pentool collaboration protocol, version 1

Status: **specification only** (v0.20.0 roadmap task 1). Nothing in this document
is implemented until the phase that owns it in section 13 ships. Where this text
and released behavior disagree, released behavior and the `.pen` schemas win.

Collaboration lets several people and software agents edit one document without
giving up the single binary, readable `.pen` JSON, stable IDs or atomic history.

## 1. Principles

1. **The `.pen` document is the source of truth.** Collaboration is a protocol
   over *operations*, not a second storage format. Any peer can always write the
   current state as an ordinary `.pen` file that older tools open.
2. **No silent loss.** Concurrent edits either merge deterministically or produce
   a *named conflict* (section 5). A losing edit is always recoverable.
3. **Offline first.** A peer works disconnected and reconciles later. No central
   service is needed to open, edit, save or render. A hosted relay is out of scope.
4. **Explicit identity and access.** Every operation records an actor; every
   session states who may read, comment or edit; the server, not the client,
   decides both.
5. **Bounded.** Operation size, rate, log length, session size and peer count are
   limited, and limits are checked before parsing or applying anything.
6. **Presence and comments never touch the scene.** They are stored and
   transmitted separately and cannot change rendering or export.
7. **Humans and agents are peers.** Agents use the same protocol as people.
8. **Deterministic.** The same set of operations produces byte-identical
   documents on every peer, independent of arrival order.

Non-goals for v1: a hosted relay or accounts, end-to-end encrypted rooms,
keystroke-granularity co-editing of one text run, and any time-based media.

## 2. Existing foundations this builds on

| Existing mechanism | Where | How collaboration uses it |
| --- | --- | --- |
| Whole-document revision = SHA-256 of the file bytes | `transaction::revision` | Becomes the *base revision* for an operation batch; stays the guard for non-collab writes. |
| Validate, check revision, history-record, atomic write | `transaction::commit_bytes` | The only place a collab batch is committed; there is no second write path. |
| Content-addressed snapshots with undo/redo cursor | `history.rs` | Stays the local recovery history. The op log (section 3.5) is a separate, hidden, append-only record. |
| Batch of scene operations with `page`, `revision`, `dry_run` | `POST /api/scene`, `scene::apply_batch_at` | Local op planner. A collab operation's `payload` is a scene operation the same planner already understands. |
| Stable, page-unique node IDs, ordered `nodes`/`children` | `pen-format-v4.md` | Merge addresses objects by ID and children by sibling order. |
| Layer `locked`, attached-instance protection | `scene::ensure_layer_unlocked`, `transaction::protect_attached_instances` | Collaboration respects both (section 4.6). |
| Ed25519 signing | `ed25519-dalek` (package signatures) | Reused for operation-log signatures (section 7). |
| Unknown fields survive load/edit/save | `pen-format-v4.md` | Collaboration metadata in the document lives in a namespaced extension (section 3.6). |

## 3. Operation model

### 3.1 Operation

An operation is one atomic, intent-level change authored by one actor.

```json
{
  "schema": 1,
  "id": "op_01HZ…",
  "actor": "act_7f3a…",
  "hlc": {"wall_ms": 1760000000000, "counter": 3, "peer": "peer_ab12"},
  "base": "sha256:…",
  "page": "page-1",
  "kind": "set-property",
  "targets": ["rect-12"],
  "payload": {"path": "style/fill/fallback", "value": "#ff3366"},
  "deps": ["op_01HY…"]
}
```

| Field | Meaning |
| --- | --- |
| `id` | `"op_" + hex(sha256(canonical_bytes(op minus id and sig))[..16])`. Content-derived, so identical ops dedupe and replay is idempotent. |
| `actor` | Stable actor ID (section 7.1). Set or overwritten by the accepting server from the authenticated token, never trusted from the client. |
| `hlc` | Hybrid logical clock (section 3.2). |
| `base` | Document revision (`transaction::revision`) the author observed. Used for rejection (section 6) and for diagnostics, not for merge ordering. |
| `page` | Page the operation applies to. Page selection never leaks to other pages. |
| `kind` | One of the operation classes in section 3.3. |
| `targets` | IDs of the nodes, pages, layers or records the operation reads or writes. Needed for conflict detection and permission checks without applying the operation. |
| `payload` | Kind-specific, validated, size-limited data. |
| `deps` | Optional causal predecessors. An operation applies only after all `deps` are applied. Missing deps keep the operation *pending* (bounded; section 10). |
| `sig` | Optional on the wire, mandatory in the stored log (section 7.2). |

Unknown fields on an operation are preserved and ignored. An unknown `kind` is
rejected with `unsupported-op` and never applied partially.

### 3.2 Hybrid logical clock

Each peer has a persistent random `peer` ID and a hybrid logical clock:
`(wall_ms, counter, peer)`. The clock advances on every local op and on every
received op (`wall_ms = max(local_wall, received_wall, now)`, counter bumped on
ties), so causally later operations always order after earlier ones even with
skewed wall clocks. A received `wall_ms` more than `MAX_CLOCK_SKEW_MS` (default
10 minutes) ahead of the receiver's clock is rejected (`clock-skew`), preventing
a peer from winning every future conflict by posting a far-future timestamp.

**Total order.** Operations compare lexicographically by
`(wall_ms, counter, actor, op_id)`. This order is total, identical on every peer,
and is used only to break ties between *concurrent* operations. It never reorders
causally related ones.

### 3.3 Operation classes

Every kind maps to one merge class. The kinds reuse the existing scene operation
vocabulary; collaboration adds no new geometry semantics.

| Class | Existing scene operations | Addressed by |
| --- | --- | --- |
| **Property set** | `set-property`, `set-style`, `bind-token`, `set-token`, text content/style edits, transform edits | `(page, object id, property path)` |
| **Create** | `rect`, `ellipse`, `line`, `path`, `text`, `put-shape`, `create-group`, `instantiate-component`, `add-page`, image/photo/raster add | new object ID |
| **Delete** | node/page/layer removal | object ID |
| **Structure** | `reparent`, reorder, `group`/`ungroup`, layer add/reorder | `(parent id, child id, position key)` |
| **Document record** | component promotion, token catalog, profile catalog, styles | record ID in its catalog |

Operations are **intent-level**: "set `style/fill/fallback` of `rect-12`", not
"replace the page". An operation that replaces a whole subtree (for example a
bulk import) is a Create of many objects and is subject to the size limits.

### 3.4 Canonical serialization and hashing

An operation's canonical bytes are its JSON with object keys sorted
lexicographically (recursively), no insignificant whitespace, numbers in the
shortest round-trip form already used by `serde_json` with `float_roundtrip`,
and `sig` and `id` omitted. `id` and the log chain hash (section 3.5) are
SHA-256 over these bytes. Two implementations must produce the same bytes for
the same logical operation; this is covered by shared fixtures
(section 12).

### 3.5 Operation log

Each collab-enabled document has a hidden, append-only log under
`.pentool/collab/<safe-name>/` (the same containment rules as `history.rs`):

```text
.pentool/collab/<safe-name>/
  log.jsonl          one accepted operation per line, in apply order
  head               {"seq": N, "hash": "sha256:…"}
  actors.json        actor registry (section 7.1)
  summary.json       bounded contribution summary (section 7.4)
  pending.jsonl      operations waiting for deps (bounded)
  conflicts.jsonl    open and resolved conflict records (section 5)
```

Each log line is `{seq, op, applied_at_revision, prev}` where `prev` is the hash
of the previous line, forming a hash chain. `head` commits the chain tip. The
log is *derivable* but not *discardable*: deleting it loses attribution and
conflict history, never document content. A document without a log opens and
renders normally; `serve --collab` starts a new chain at the current revision.

`applied_at_revision` is the document revision after the operation, which lets a
peer or an auditor verify replay: replaying the chain from a snapshot must yield
the recorded revisions.

### 3.6 Document-side metadata

The `.pen` file gains at most one optional top-level extension object,
`"collab": {"schema": 1, "head": "sha256:…", "summary": {…}}`, which carries the
chain head and the bounded contribution summary (section 7.4) so stats survive
copying the file without its hidden directory. It is metadata only: it never
changes rendering or export, unknown fields inside it survive, and a document
without it is valid. Exporting it (to PDF, SVG, PNG or a package) is **off by
default** because it can identify people (section 7.5).

## 4. Merge rules

Merge is a pure function of the set of accepted operations: replaying them in
the total order of section 3.2, with the rules below, yields identical bytes on
every peer.

### 4.1 Property set: last writer wins, per property

Two operations conflict when they write the same `(page, object id, property
path)` and neither causally precedes the other (neither lists the other in its
transitive `deps` nor has a higher HLC from a peer that had seen it). The winner
is the operation that is greater in the total order; the loser is **not
discarded**: it is recorded as an `overwritten-edit` conflict (section 5) carrying
its value, so it can be restored.

Operations on *different* property paths of the same object, or on different
objects, never conflict and both apply. This is the same granularity at which
design editors resolve concurrent edits: a fill change and a position change to
one object by two people both survive.

A property path is a JSON-pointer-like path into the node (for example
`transform`, `style/fill/fallback`, `radius_x`). Array-valued properties (such as
`transform`, which is six numbers, or a path's `d`) are a single atomic value;
v1 does not merge inside them.

### 4.2 Ordering of children: fractional keys

The array order of `nodes` and `children` is the z-order. To make concurrent
insert and reorder commute, each child carries an ephemeral **position key** in
the log (not stored in the `.pen` file): a base-62 fractional index strictly
between its neighbours' keys. Replay sorts siblings by `(key, creator HLC,
object id)` and writes the resulting order into the ordinary array. Two peers
inserting at the same slot get distinct keys by appending a peer-derived suffix,
so neither insert is lost and order is deterministic. The keys are rebuilt from
the array when a peer first joins (a snapshot), so existing documents need no
migration.

### 4.3 Create

Creating an object ID that already exists with different content is a
`duplicate-id` conflict. The earlier create (total order) wins; the later is
**rejected at the boundary** and the author is told the existing object. To make
this rare, clients generate IDs as `kind-` plus a random suffix, and the server
suggests a fresh ID in the rejection. Re-sending an identical create is
idempotent.

### 4.4 Delete versus edit

Deleting an object (or page, layer) writes a tombstone. A *causally concurrent*
edit to a deleted object does not resurrect it: **the delete wins**, and each
edit is preserved as an `edit-on-deleted` conflict carrying the edit's value so
the object can be restored with the edit applied. Deleting a group deletes its
descendants (`pen-format-v4.md`); an edit to a descendant is handled the same
way. A Create or reparent into a concurrently deleted parent is rejected as
`parent-deleted`.

### 4.5 Reparent and cycles

Reparenting follows `pen-format-v4.md`: cycles and depth over 64 are rejected.
Concurrent reparents can create a cycle that neither author could see (A moves X
under Y while B moves Y under X). Replay applies reparents in total order; a
reparent whose application would create a cycle or exceed depth 64 is skipped and
recorded as a `reparent-cycle` conflict, leaving the object where the earlier
operation put it. The check is deterministic because it runs on the replayed
state in a fixed order.

### 4.6 Locks and protected content

Existing protections are enforced **at apply time on every peer**, not only at
the origin:

- An edit to a node in a locked layer is rejected (`locked`). Locking and
  unlocking are themselves operations; an edit concurrent with a lock is ordered
  by the total order and, if it lands after the lock, is rejected and reported.
- Layers owned by an attached instance cannot be edited except through the
  instance operations (`transaction::protect_attached_instances`); the same
  check runs on replay.
- A claim (section 6.3) is a lock with an owner and expiry; it blocks other
  *actors* but not its owner.

### 4.7 Document records

Component promotion, token and style catalogs and similar records merge as
records keyed by ID with the property-set and delete rules above. Format
migration is **not a collaborative operation**: a migration changes the format
version and is rejected while a collab session is active (`migration-blocked`);
it must be done before or after the session through the explicit CLI command.

### 4.8 Why not Yjs, Automerge or Loro for v1

Those libraries are excellent CRDTs for text and general JSON. We do not adopt
one for v1 because:

- **Source of truth.** A CRDT library stores its own document state; `.pen` would
  become a projection of a second format, violating principle 1 and the
  one-readable-JSON promise.
- **Single small binary and WASM.** The core must stay one Rust binary that also
  compiles to WASM; a large dependency is a measured cost against an agreed
  benefit.
- **Our data fits a simpler model.** A scene graph is a tree of objects with
  property bags; per-property last-writer-wins plus fractional ordering plus
  tombstones is sufficient, small, auditable and deterministic.
- **Named conflicts.** Our contract requires surfacing a lost edit; generic CRDTs
  converge by silently choosing a winner.
- **Text co-editing is deferred.** Keystroke-level merging of one text run is the
  main thing those libraries add, and it is explicitly out of v1 scope. A text
  object is a property value; two concurrent edits to the same text content
  conflict as in 4.1.

**Escape hatch.** The operation vocabulary and the merge function sit behind one
module boundary (`collab::merge`). If text co-editing is later required, a Rust
CRDT (`loro` or `yrs`) can be used for *only* the text-content property: the
document stores plain text, the CRDT state lives in the hidden log directory, and
the replay function emits the resolved string. No change to `.pen` is needed.

## 5. Conflicts

A conflict is a first-class, persisted record, so nothing is silently
overwritten.

```json
{
  "schema": 1,
  "id": "cf_…",
  "code": "overwritten-edit",
  "page": "page-1",
  "object": "rect-12",
  "property": "style/fill/fallback",
  "winning_op": "op_…",
  "losing_op": "op_…",
  "winning_actor": "act_…",
  "losing_actor": "act_…",
  "losing_value": "#ff3366",
  "winning_value": "#3366ff",
  "status": "open",
  "restore": {"command": "pentool collab resolve DOC cf_… --keep losing"}
}
```

| Code | Raised when | Default outcome |
| --- | --- | --- |
| `overwritten-edit` | Concurrent writes to one property (4.1) | Greater op applies; loser kept in the record. |
| `edit-on-deleted` | Edit concurrent with a delete (4.4) | Delete applies; edit kept. |
| `duplicate-id` | Create of an existing ID (4.3) | Earlier create stands; later rejected. |
| `reparent-cycle` | Concurrent reparent would loop (4.5) | Earlier reparent stands; later skipped. |
| `parent-deleted` | Create/move into a deleted parent | Rejected. |
| `locked` | Edit lands on a locked layer/claim (4.6) | Rejected. |
| `stale-base` | Batch `base` no longer current (section 6.1) | Rejected with fresh state. |
| `permission-denied` | Actor lacks the role (section 9) | Rejected. |
| `limit-exceeded` | Any bound in section 10 | Rejected before work. |

Status is `open`, `resolved` or `dismissed`. **Resolving** a conflict is itself an
ordinary operation (for example re-applying `losing_value`), authored by the
resolver, so it replicates, is attributed and is undoable. Conflicts reference
operations by ID and carry the values needed to restore without the full log, so
they survive log compaction (section 7.4).

CLI and API results use the repository error style: the message names the
operation, page, object and corrective action, for example
`[conflict] set-property style/fill/fallback on rect-12 (page-1) lost to op_… by
act_…; run pentool collab resolve …`. A rejected batch changes nothing: the
document is byte-for-byte unchanged and no log line is written.

## 6. Agents and concurrent writers

Agents are ordinary peers, but they run unattended and fast, so the protocol adds
explicit rules rather than relying on good behavior.

### 6.1 Base-revision rejection with fresh state

A *non-collab* write (CLI or `POST /api/scene`) keeps the existing contract: it
carries `revision`, and a mismatch fails with `revision mismatch` and no change.
A collab batch carries `base`; if `base` is not the current revision *and* the
batch's `targets` were touched by operations after `base`, the server rejects it
as `stale-base` and returns, in the same response:

- the current `revision`,
- the changed objects among `targets` (compact: ID, kind, the changed properties),
- the IDs and actors of the operations that changed them.

The agent re-plans against that state and resends. If no target was touched since
`base`, the batch applies (rebased) even though the document revision moved. This
keeps agents from retry-storming on unrelated edits while never letting them
clobber a change they have not seen.

### 6.2 Intent-level operations

Agents send small, targeted operations ("set fill of `rect-12`"), never a
rewritten document. A single batch is limited (section 10) and one successful
batch is one log entry group and one undoable step.

### 6.3 Claims

An actor can **claim** objects, a layer or a page region for a bounded time
(`claim` operation: targets, `ttl_ms` up to 10 minutes, renewable). A claim is an
advisory-plus-enforced lock built on the existing lock check: other actors' edits
to claimed targets are rejected with `locked` and the claimant is named. Claims
expire on their own, are listed in the collaborator view, and can be broken by an
editor with the `owner` role (recorded as an operation). Agents are expected to
claim before multi-step edits; the planner's `--claim` option does it
automatically.

### 6.4 Agent-agent and human-agent

Nothing changes between combinations: the same rules, the same conflicts, the
same stats. A human may always override an agent's claim; an agent may not
override a human's.

## 7. Identity, attribution and statistics

### 7.1 Actors

```json
{
  "id": "act_7f3a…",
  "kind": "agent",
  "display_name": "Layout bot",
  "tool": "claude-code",
  "model": "claude-sonnet-5-5",
  "directed_by": "act_91c0…",
  "created_at_ms": 1760000000000
}
```

`kind` is `human` or `agent`. `directed_by` is mandatory for agents and names the
human actor on whose behalf they act; it must resolve to a `human` actor in the
same registry. `model` and `tool` are free text, length-limited, and informational
only: they carry no authority. The registry (`actors.json`) is part of the log
state and replicates.

An actor ID is `"act_" + hex(sha256(public_key)[..16])` for the actor's Ed25519
key (section 7.2), so identity is self-certifying and not chosen by the client.

### 7.2 Unforgeable attribution

- **Server-assigned actor.** The accepting server derives `actor` from the
  authenticated token (section 9); an `actor` field in an incoming operation is
  ignored if it differs. A peer cannot author as someone else.
- **Signatures.** Each accepting peer signs `hash(op canonical bytes ‖ seq ‖
  prev)` with its own Ed25519 key when appending to the log; the log line stores
  `sig` and the signer's key ID. Operations relayed between peers keep the
  originating peer's signature. A line that fails verification is rejected
  (`bad-signature`).
- **Hash chain.** Each line includes the previous line's hash, so reordering,
  removing or editing history changes `head`. `head` is also written into the
  document's `collab` block.
- **Keys.** Keys are generated locally on first collab use and stored in the
  user's configuration directory (the `directories` crate, as for existing keys),
  never in the document, the log or a package. Key rotation adds a new actor
  linked by a signed statement from the old key.

Attribution is *honest* rather than *secret*: it proves which key authored what,
not that a human was at a keyboard; `directed_by` is a claim of the directing
actor's token holder and is shown as such.

### 7.3 Contribution statistics

Stats are computed **only** from the verified operation log (plus the bounded
summary of compacted history). They are never self-reported and never trusted
from a peer. Counts per actor:

| Metric | Definition |
| --- | --- |
| `operations` | Accepted operations (excluding rejected, pending, resolved-conflict bookkeeping). |
| `share` | `operations / total accepted operations`, reported as a percentage with fixed rounding (largest remainder, so shares sum to exactly 100). |
| `objects_created` | Objects whose Create was authored by the actor. |
| `objects_modified` | Distinct objects that received at least one property or structure change from the actor after creation (own creations excluded). |
| `properties_changed` | Distinct `(object, property)` pairs written by the actor. |
| `net_authored` | Objects the actor created that still exist in the current document. |
| `surviving_properties` | `(object, property)` pairs whose *current* value was last written by the actor. |

**Surviving-content attribution.** For each existing object, stats record the
creator and, per property, the last effective writer. An object an agent created
and a human later restyled therefore counts as `objects_created` for the agent and
`surviving_properties` for both. A deleted object counts toward `operations` but
not `net_authored`.

**Totals.** `by kind` aggregates `human` and `agent` actors; `directed_by` rolls an
agent's work into a second report, "by directing human", so human-plus-agent
contribution can be read both ways.

`pentool stats DOC [--by actor|kind|page] [--json]` reports these deterministically:
the same log yields byte-identical JSON on every platform, ordered by actor ID.

### 7.4 Survival of history compaction

The log can be compacted (`pentool collab compact`) to bound disk use. Compaction
folds the oldest operations into `summary.json`, which stores, per actor and
page, the metric counters above plus the per-object creator and per-property
last-writer map for *surviving* objects. It is itself hash-chained to the retained
log, so the summary cannot be edited without breaking `head`. Stats computed
before and after compaction are identical (tested). The summary is bounded:
at most `MAX_ACTORS` actors and `MAX_SUMMARY_OBJECTS` surviving-object records;
beyond the object bound, per-property attribution degrades to per-object, and the
report states that precision was reduced.

### 7.5 Privacy

Attribution can identify people, so: display names are optional and user-chosen;
the `collab` block, actor registry and stats are **never written into exports**
(PNG, SVG, PDF, PSD, packages) unless the user passes an explicit
`--include-attribution`; sharing a `.pen` file shares the `collab` block, which
the CLI warns about once per document, and `pentool collab strip DOC` removes it.

## 8. Sync

### 8.1 Topology

There is no required central service. Each peer holds the full log and document.
`pentool serve --collab` extends the existing server (bound to `127.0.0.1` by
default) with a sync endpoint; peers are **explicit**: a list of
`--peer URL` entries and/or accepted incoming connections. There is no discovery
protocol in v1 (no multicast, no registry). A "host" is simply a peer others
connect to; any peer can play that role, and the host has no extra authority over
merge results.

### 8.2 Transport

HTTP/1.1 on the existing axum server:

| Route | Purpose |
| --- | --- |
| `POST /api/v1/collab/hello` | Exchange protocol version, peer ID, actor, `head`, role granted. |
| `POST /api/v1/collab/ops` | Submit a bounded batch of operations (returns accepted, pending, rejected with conflict records). |
| `GET /api/v1/collab/ops?since=SEQ&limit=N` | Pull accepted operations after a sequence, paginated (`matches`, `returned`, `offset`, `limit`, `has_more` exact, as for tree/search). |
| `GET /api/v1/collab/stream` | Optional server-sent events for live ops, presence and comments; polling works without it. |
| `GET /api/v1/collab/state` | Snapshot (document bytes + `head`) for a new or far-behind peer. |

No WebSocket or new network dependency is required for v1. Everything is plain
HTTP/JSON using the existing server and `reqwest` (host builds only). The
shared-core / WASM split in the architecture milestone is unaffected: merge and
log code are pure and `no_std`-friendly, networking lives only in `sync`.

### 8.3 Reconciliation

1. Peers exchange `head` and sequence numbers via `hello`.
2. Each side sends the operations the other lacks (`since`), bounded by size and
   count; pagination repeats until caught up.
3. The receiver verifies signatures, chain linkage and per-op validity, buffers
   operations whose `deps` are missing (bounded), and applies the rest in total
   order through the shared transaction path.
4. Both peers converge to the same revision and the same conflict set. A peer
   that is too far behind (log truncated by compaction) receives `state` (a
   snapshot) plus the newer operations.

Re-sending an operation is harmless (content-derived ID, idempotent apply).
Delivery order and duplication do not change the result.

### 8.4 Crash safety

Appending an operation and updating the document is one recoverable unit:

1. validate and plan in memory;
2. append the log line to `log.jsonl.tmp`, fsync;
3. commit the document through `transaction::commit_bytes` (atomic write plus
   history entry);
4. rename the log line into place, update `head`.

On startup, `recover_interrupted_write` (existing) restores the document, then
the collab layer reconciles the log: a log line with no matching document
revision is discarded (the operation was never applied), and a document revision
with no log line is detected by chain verification and re-derived from history.
A crash during sync can only lose *un-acknowledged* operations, which the peer
resends.

## 9. Permissions, comments and presence

### 9.1 Roles

| Role | May |
| --- | --- |
| `viewer` | Read document, log, stats; see presence. |
| `commenter` | Viewer plus create/resolve comments and mentions. |
| `editor` | Commenter plus submit scene operations, create claims. |
| `owner` | Editor plus manage tokens, break claims, compact, change roles. |

Roles are carried by the **token** and enforced by the server on every request,
including relayed operations from other peers (a peer's relayed operation is
checked against the *originating* actor's role). Clients display roles but never
decide them. A request without sufficient role fails with `permission-denied`
before any parsing beyond the header.

### 9.2 Tokens

Tokens are random 256-bit secrets, shown once at creation. The server stores only
a SHA-256 hash, a role, an optional actor binding and an expiry. They are passed
in an `Authorization: Bearer` header, never in a URL, a document, a log, a
package or a lockfile, and are redacted from all diagnostics. `serve --collab`
refuses to start on a non-loopback address without `--token-file` and, by
default, refuses plain HTTP off loopback unless `--allow-insecure-lan` is given
(documented as LAN-only; TLS termination is the user's responsibility in v1).
`pentool collab token create|list|revoke` manage tokens; revocation takes effect
on the next request.

### 9.3 Comments and review states

Comments, mentions and review states (`open`, `approved`, `changes-requested`) are
stored in a separate file, `<doc>.pencomments` (or the hidden collab directory),
as an append-only list keyed by object ID and optional page-space anchor. They are
not in the scene graph, never change rendering or export, and survive object
edits (an anchor to a deleted object becomes "orphaned", not lost). Mentions
reference actor IDs. They replicate with the same signed-log mechanism but are a
separate stream (`comment` ops), so a viewer-with-comment-role cannot write
scene operations.

### 9.4 Presence

Presence (cursor, selection, viewport, current page) is best-effort and
**non-persistent**: sent over the stream, held in memory with a short TTL, never
written to the log, the document or history, rate-limited, and dropped on
overflow. Presence cannot affect the document in any way.

## 10. Limits

All limits are checked before parsing or applying work and fail with
`[limit-exceeded]` naming the limit. Values are defaults; a server can lower but
not raise them above the hard ceilings.

| Limit | Default | Hard ceiling |
| --- | --- | --- |
| Single operation, canonical bytes | 256 KiB | 1 MiB |
| Operations per batch | 500 | 10,000 (matches `apply_batch_at`) |
| Batch request bytes | 4 MiB | 16 MiB |
| Operations per actor per second | 50 | 500 |
| Pending (dep-missing) operations | 1,000 | 10,000 |
| Pending age | 10 min | 1 h |
| Log length before compaction advised | 100,000 ops | 1,000,000 ops |
| Log directory size | 256 MiB | 1 GiB |
| Peers per session | 16 | 64 |
| Actors in registry | 256 | 1,024 |
| Active claims | 256 | 2,048 |
| Claim `ttl` | 10 min | 1 h |
| Comments per object / per doc | 1,000 / 50,000 | – |
| Presence updates per peer per second | 10 | 30 |
| Allowed clock skew | 10 min | 1 h |
| Group depth (existing) | 64 | 64 |
| Surviving-object records in summary | 100,000 | 1,000,000 |

`GET` listings are paginated with exact `matches/returned/offset/limit/has_more`.

## 11. Threat model

Assets: document integrity, attribution truthfulness, availability of the editor,
user secrets (tokens, keys), user filesystem.

| Threat | Mitigation |
| --- | --- |
| **Forged actor** (peer claims to be another) | Server-assigned actor from token; per-op Ed25519 signature; actor ID derived from the key; `bad-signature` rejection. |
| **Retroactive tampering** with history/stats | Hash chain + `head` in the document; summary chained to log; tamper detected on open and by `collab verify`. |
| **Replay** of a captured op | Content-derived ID makes replay idempotent; `deps`/`seq` and expiry-bound tokens bound re-injection; replay of a revoked actor's old op is accepted only if it was already in the log. |
| **Oversized or malicious ops** | Limits (section 10) checked before parsing; strict schema, `deny_unknown_fields` on required parts; payload validated by the same scene planner and `validate_value`; no code execution anywhere in ops. |
| **Clock abuse** (far-future HLC to win all conflicts) | `MAX_CLOCK_SKEW_MS` rejection; ordering only breaks ties between concurrent ops, so it cannot override causally later edits. |
| **Flooding / DoS** | Per-actor and per-peer rate limits, bounded pending queue, bounded batches, bounded log, presence dropped on overflow. |
| **Hostile peer sends divergent state** | Peers verify and re-derive state from signed ops; a peer's claimed document revision is never trusted; mismatch triggers resync from verified log. |
| **Privilege escalation** (viewer writes) | Role in token; server-side check per request and per relayed op against the originating actor; comments and scene ops are separate op streams. |
| **Path traversal** (op or comment names a path) | Ops carry IDs, not paths; image/resource payloads use existing `resource::safe_relative_path` and size limits; log and token files live under the contained `.pentool/collab/<safe-name>` using the same name sanitizing as `history.rs`. |
| **Archive/package smuggling** | Collab state is never part of a package unless explicitly requested; existing package path checks unchanged. |
| **Crash mid-sync / mid-commit** | Section 8.4: write-ahead log line, atomic document commit, recovery on start, unacknowledged ops resent. |
| **Token leak** | Tokens stored hashed, shown once, header-only, redacted in logs, expirable and revocable; loopback by default; insecure LAN explicit. |
| **Key theft** | Keys outside the document; rotation by signed statement; compromised key's actor can be revoked and its future ops rejected. |
| **Information disclosure via exports** | Attribution not exported by default (7.5); presence never persisted. |
| **Conflicting protections** (edit through lock/instance) | Locks and attached-instance protection enforced at apply time on every peer. |
| **Split brain / divergent histories** | Deterministic total order and conflict records; two histories that share an ancestor always converge; unrelated histories are refused (`unrelated-history`) rather than force-merged. |

Residual risks, stated plainly: v1 transport on a LAN is unencrypted unless the
user adds TLS; attribution proves key control, not human presence; a malicious
`editor` can still make bad but valid edits (mitigated by claims, conflict
records, history and undo, not prevented).

## 12. Conformance and tests

A conforming implementation demonstrates:

1. **Canonical bytes and IDs** are identical across Linux, Windows and macOS for
   shared fixtures in `docs/fixtures/collab/` (small, deterministic, tracked).
2. **Property-based convergence.** For random sets of operations and random
   delivery orders, partitions and duplicates, every peer ends with a
   byte-identical `.pen` and the same conflict set. Properties covered:
   commutativity of concurrent ops on different properties, determinism of the
   tie-break, idempotence of replay, associativity of merging log segments.
3. **Merge-rule tests** for each rule in section 4: LWW tie-break by HLC then
   actor, fractional-key insertion at the same slot, delete vs edit, create
   duplicates, reparent cycles, depth limit, lock and claim interplay, attached
   instance protection, migration blocked.
4. **No silent loss.** Every operation that does not take effect has a conflict
   record or a rejection; a test asserts `accepted + rejected + pending = sent`.
5. **Atomicity.** A failed batch leaves the document byte-identical and writes no
   log line; dry run and commit use the same planner and report compatible results.
6. **Permissions.** Matrix test of role × operation class × direct/relayed.
7. **Hostile peers.** Forged actor, bad signature, truncated or reordered chain,
   oversized and malformed ops, far-future clocks, flooding, path-like IDs,
   duplicate IDs, replayed ops, unrelated histories.
8. **Crash recovery.** Fault injection at each step of section 8.4 (kill before
   log fsync, between log and document, after document before `head`) followed by
   reopen and resync; the result equals a crash-free run.
9. **Stats.** Deterministic byte-identical output; shares sum to exactly 100;
   unchanged across compaction; surviving-content attribution on the
   agent-creates / human-restyles case; not influenced by self-reported fields.
10. **Compatibility.** A document with `collab` metadata opens in tools that
    ignore it; a document without it works; unknown future `schema` fails clearly
    and without mutation; render/export output is byte-identical with and without
    the collab block and with and without comments/presence.
11. **Limits.** Each limit in section 10 has a boundary test and a message test.

Fuzz targets (run by the existing weekly fuzz workflow, not a push gate): the
operation parser, the log reader, the merge function, and the sync request
handlers.

## 13. Implementation map

New modules are pure where possible so they can later move into the shared core
crate. Existing paths are extended, not duplicated.

| Phase (TASKS item) | Modules / changes | Exit evidence |
| --- | --- | --- |
| **1. Spec** | `docs/collaboration-v1.md` (this document), `docs/fixtures/collab/` (canonical-bytes fixtures) | Reviewed spec; threat model signed off. |
| **2. Operation log in transaction path** | `src/collab/op.rs` (types, canonical bytes, HLC), `src/collab/log.rs` (append, chain, verify, recovery), `src/collab/merge.rs` (replay + rules, conflicts); hook in `transaction::commit_bytes` behind `collab` mode; `pentool collab log|verify|replay` | Replay determinism; property-based convergence test; crash tests for the local log. |
| **3. Local-network sync** | `src/collab/sync.rs` (hello/ops/state/stream), `serve --collab`, `--peer`, token store; `/api/v1/collab/*` routes in `server.rs` | Two-process convergence over loopback; token and loopback-default tests; reconcile after offline edits. |
| **3a. Actors and stats** | `src/collab/actor.rs` (registry, keys, signatures), `src/collab/stats.rs`, `pentool stats DOC [--by actor\|kind\|page] [--json]`, summary + compaction | Determinism, compaction-stability and forgery tests; editor stats panel contract. |
| **4. Comments and reviews** | `src/comments.rs`, `.pencomments`, `pentool comment …`, `comment` op stream | Comments never change render/export bytes; anchor survival tests. |
| **5. Presence** | `src/collab/presence.rs` (in memory, TTL, rate limit) over the stream | Not persisted; no effect on document bytes; overflow behavior. |
| **6. Permissions** | `src/collab/perm.rs` (roles, per-request and relayed-op checks) | Role matrix tests; revoked-token tests. |
| **7. Editor** | `web-ui/` collaborator list, comments panel, conflict resolution UI, stats panel; `bun run build` refreshes `web/studio.html` | Studio tests; conflict resolve round trip via `collab resolve`. |
| **8. Evidence** | Convergence fuzzing, hostile-peer suite, crash-during-sync suite, 3-OS CI; `docs/collaboration-v1.md` conformance checklist ticked | CI green on Linux, Windows, Apple Silicon macOS. |

Cross-cutting: README and `--help` examples for every new command; a schema for
the operation, conflict and stats JSON (`docs/collaboration-v1.schema.json`)
added with phase 2; `AGENTS.md` roadmap section updated when each phase ships;
version chosen at release time under the versioning policy (collaboration is a
backward-compatible MINOR feature).

## 13a. Phase 2 implementation notes (operation log)

What shipped in `src/collab/` for roadmap item 2, and where it differs from the
sections above. Later phases close these gaps.

- **Op kinds.** `set-property`, `create`, `delete`, `move` are native. A fifth
  kind, `scene`, carries a scene batch (`payload.operations`) and is expanded at
  replay by running `scene::apply_batch_at` and diffing the result into native
  operations, so there is one mutation vocabulary. Targets are object ids;
  `payload.level` (`d` document, `p` page, `l` layer, `n` node) selects the
  namespace, since layer and node ids are only unique per level and page.
- **Local recording.** The transaction hook replays the log, adds a
  `act_external` catch-up entry if the document was edited outside the
  transaction path, derives native operations for the new document, and
  requires the replayed result to equal the written document; otherwise the
  mutation fails with `[collab-divergence]` and the document is not modified.
- **Files.** `genesis.pen`, `log.jsonl`, `head`, `actors.json`, `peer`, `lock`,
  and a transient `append.tmp` (write-ahead stage, recovered by revision).
  There is no `summary.json`, `pending.jsonl` or `conflicts.jsonl`: conflicts and
  pending operations are derived by replay, which is a pure function of the
  operation set.
- **Actors.** The id is `act_` plus a digest of `kind:handle`; Ed25519 keys and
  `sig` verification arrive with the actor and stats phase. There is no
  `collab` block in the `.pen` file yet.
- **Sibling order.** Fractional keys order siblings; exact key ties are broken by
  `(key, creation HLC, id)` rather than a peer suffix.
- **Extra codes.** Conflicts also use `parent-deleted`, `duplicate-id` and
  `limit-exceeded`; rejection-only codes are `invalid-op`, `unknown-target`,
  `migration-blocked` and `unsupported-op`.
- **Limits.** 100,000 operations per log (no compaction yet), 10,000 operations
  per local mutation, 256 KiB per operation, 256 actors, group depth 64. Only
  version 4 documents can be recorded; migration is not a collaborative operation.
- **Not yet enforced.** `base` staleness checks, the clock-skew check at the
  local hook (the check exists in `hlc.rs`), claims, permissions and sync.

## 13b. Phase 3 implementation notes (sync, tokens, roles)

`src/collab/{sync,token,perm}.rs`; tests in `tests/collab_sync.rs`. Deviations
from sections 8 to 10:

- **Transport.** Plain HTTP plus SSE (`GET /api/collab/events`, one `head`
  event per new log head). No WebSocket. TLS is not terminated by Pentool;
  a non-loopback bind needs an active token and `--allow-insecure-lan`.
- **Routes.** `GET session|genesis|events|ops`, `POST ops|claim|release`, and
  owner-only `GET|POST tokens`, `POST tokens/revoke|role`. Pages of `ops` are
  cursor based on the log sequence (`since`, `next`) and bounded by count and
  bytes. `POST ops` returns `accepted`, `duplicate`, `rejected` (named codes
  such as `actor-mismatch`, `clock-skew`, `unknown-actor`, `locked`,
  `invalid-op`), `pending`, `merge_rejected` and `conflicts`.
- **Reconcile.** A pull batch and a push batch are each one write-ahead unit
  (log staging, document + history commit, log finalize). A pull page moves the
  peer cursor (`peers.json`) only after it is durable. Relayed operations are
  stored on pull but only a peer's own operations are pushed.
- **Tokens.** `ptc_<id>_<64 hex>`; hash `sha256("pentool-collab-token-v1:" +
  secret)` in `tokens.json` (0600 on unix), compared in constant time over all
  records. The actor is fixed at creation from the `--actor-*` flags.
- **Roles.** `perm::ROUTES` classifies every route; an unclassified route or
  method is denied, and a test scans `server.rs` and `sync.rs` for registrations.
  Read-only POST routes (renders, analysis) are viewer-level; extension run and
  registry install are owner-only. Mutations by token holders are attributed to
  the token's actor through a request-scoped override guarded by a mutex.
  Comment authorship is not yet derived from the token (item 4 follow-up).
- **Local owner.** A tokenless request is the owner only when the server is
  bound to loopback, the peer address and `Host` are loopback and no forwarding
  header is present.
- **Claims.** Stored in `claims.json`; the transaction hook rejects local
  edits that touch another actor's claim (humans may override an agent's claim).
  TTL default 10 min (ceiling 1 h), 256 claims (ceiling 2,048), 64 targets each.
- **Peers.** `peers.json`, at most 16 (ceiling 64); URLs are `http(s)://host[:port][/prefix]`
  with no credentials, query, fragment, dot segments or escapes.
- **Limits.** Batch 500 ops / 4 MiB, 50 ops per second per token, 4 event
  streams per token and 64 in total, pending 1,000. Not yet enforced: pending
  age expiry and compaction.
- **Presence and UI.** Not part of this phase: item 5 can add its routes to
  `perm::ROUTES` (viewer-level) and reuse `Ctx` and the SSE slot limiter; item 7
  reads `GET /api/collab/session` for `role` and `may`.

## 13c. Evidence and invariants

`tests/collab_evidence.rs` (fast, seeded, no network) and four `cargo-fuzz`
targets back the guarantees above. Each invariant names the test that proves it.

| Id | Invariant | Test |
| --- | --- | --- |
| C1 | 3-5 peers converge through partitions, duplicate and reordered delivery | `simulated_peers_converge_through_partitions_and_reordering` |
| C2 | Delivery order never changes the result (concurrent reparent/delete) | `concurrent_reparents_and_deletes_converge_in_both_orders` |
| H1 | Oversized operations are rejected before state is read | `hostile_oversized_fields_are_rejected` |
| H2 | A batch over the count limit is refused whole | `hostile_oversized_batch_is_refused_before_any_work` |
| H3 | Huge identifiers are rejected | `hostile_huge_ids_are_rejected` |
| H4 | Deep nesting is bounded | `hostile_deep_nesting_is_bounded` |
| H5 | Duplicate ids keep one node and report `duplicate-id` | `hostile_duplicate_ids_keep_one_node` |
| H6 | Ids differing by unicode normalization resolve deterministically (ids compare byte-wise, no normalization) | `hostile_unicode_normalization_collisions_are_deterministic` |
| H7 | Non-finite numbers never reach the document; the batch is rejected whole (`[invalid-merge]`) | `hostile_non_finite_numbers_are_rejected` |
| H8 | Locked layers reject edits, including `set-property` (spec 4.6) | `hostile_edits_on_locked_layers_are_not_applied` |
| H9 | Instance-protected layers reject direct edits | `hostile_edits_on_instance_protected_layers_are_not_applied` |
| H10 | Cycle-forming reparent is rejected (`reparent-cycle`) | `hostile_cycle_forming_reparent_is_rejected` |
| H11 | Forged or unknown actors are rejected | `hostile_forged_and_unknown_actors_are_rejected` |
| H12 | Clocks more than 10 minutes ahead are rejected | `hostile_far_future_clocks_are_rejected` |
| H13 | Replaying integrated operations is a no-op | `hostile_replay_of_old_operations_is_a_no_op` |
| D1-D5 | A crash at every durability boundary (staging, history store, history manifest, document write, actors, torn line, lines, head) recovers to either the old or the new document, a verified chain, and reruns to the uninterrupted bytes | `crash_at_every_durability_boundary_recovers_to_the_uninterrupted_result` |
| D6 | Repeated crashes still converge | `repeated_crashes_still_converge` |
| D7 | A failed commit aborts the staged record | `failed_commit_leaves_no_staged_record` |
| D8 | A torn tail is cut; corruption inside the committed prefix is refused | `torn_log_tail_is_cut_but_committed_corruption_is_not` |
| L1 | A lock older than 60 s is reclaimed | `stale_locks_are_reclaimed` |
| L2 | A fresh lock is still honoured | `fresh_locks_are_still_honoured` |
| L3 | Orphaned `*.PID.tmp` / `.stale-PID` files are swept under the held lock | `orphaned_temp_files_are_swept_under_the_lock` |
| F1 | The fuzz entry points never panic on seeded input | `fuzz_entry_points_never_panic_on_seeded_input` |

### Stale-lock policy

`src/stale.rs` is shared by the collaboration log and document history. A lock
file older than `STALE_AFTER` (60 s) is reclaimed by renaming it aside and
retrying; fresh locks fail with a clear error. Temp files matching
`X.<digits>.tmp` are swept at age 0 while the lock is held, since every writer
holds it. `atomic_write` also removes `.NAME.pentool-tmp-*` siblings older than
60 s. Residual risks: a writer paused for more than 60 s while holding the lock
can lose it; history debris from a crashed writer remains until the next
history write.

### Fuzzing

Targets `collab_op`, `collab_merge`, `collab_log` and `collab_sync_request`
(`fuzz/fuzz_targets/`) call `pentool::collab::evidence`. They run in the weekly
`fuzz.yml` workflow, not in the push gate.

### Bugs found by this work

- `set-property` on a node ignored the layer lock (spec 4.6); `merge.rs`
  now checks it. Regression: H8.
- A torn final log line from a crash blocked recovery; it is now cut.
- Dead-process lock files blocked history and the log forever; now reclaimed.
- Orphan temp files accumulated after crashes; now swept.
- Hardening without a failing-before test: the manifest and log atomic writes
  rename over the old file instead of remove-then-rename (the window is
  between two syscalls and cannot be hooked).

## 14. Open questions for review

1. **Transport.** Is plain HTTP plus SSE enough for v1, or do we want WebSocket
   (a new dependency) for lower latency presence?
2. **Roles.** Is a fourth role (`owner`) acceptable, or should owner be implied by
   possession of the token-management capability on the host only?
3. **Claims.** Is a 10-minute maximum claim TTL right for long agent tasks?
4. **Compaction default.** Should compaction run automatically at the advised log
   length, or only on explicit command?
5. **Attribution in the file.** Is carrying the bounded summary in the `.pen`
   `collab` block acceptable given the privacy trade-off, or should it be
   sidecar-only by default?
6. **Text content.** Is "text content is one LWW property" acceptable for v1
   given keystroke co-editing is deferred?
