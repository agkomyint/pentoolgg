# Libraries, design systems, and collaboration

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [Comments and review states](#comments-and-review-states)
- [Extensions](#extensions)
- [Design systems and brand kits](#design-systems-and-brand-kits)
- [Open design libraries and packages](#open-design-libraries-and-packages)
- [Collaboration operation log](#collaboration-operation-log)

## Comments and review states

Threads, `@mentions` and review states live in a sidecar next to the document
(`.pentool/comments/<name>.json`). Commenting never changes the `.pen` bytes,
its revision, rendering or export. The contract is
[`docs/collaboration-comments.md`](../docs/collaboration-comments.md).

```sh
pentool comment add logo.pen "Tighten this, @ana" --object title --author Ana --actor-id ana
pentool comment add logo.pen "Check the corner" --x 12 --y 8
pentool comment reply logo.pen t-1 "Done" --kind agent --actor-id bot-1 --author Painter --model claude --directed-by ana
pentool comment review logo.pen t-1 requested        # none | requested | approved | changes-requested
pentool comment resolve logo.pen t-1 --if-version 2  # refuses a stale thread version
pentool comment reopen logo.pen t-1
pentool comment list logo.pen --status open --limit 20 --json
pentool comment list logo.pen --thread t-1           # one thread with every comment
```

The shared server exposes the same operations as `GET /api/comments` (filters
and `offset`/`limit`) and `POST /api/comments` (`action` of `add`, `reply`,
`resolve`, `reopen` or `review`).

## Extensions

Sandboxed WebAssembly extensions read a document and propose edits through the
same planner and transaction as `pentool batch` (dry run, revision guard, locks,
rollback, one undo step). They get no filesystem, network, clock or randomness,
and run under fixed CPU and memory limits. Capabilities are declared in the
manifest and shown before install. See [docs/extensions-v1.md](../docs/extensions-v1.md).

```sh
pentool extension pack examples/extensions/stamp/package stamp.penext
pentool extension verify stamp.penext
pentool extension add stamp.penext                 # shows capabilities, installs nothing
pentool extension add stamp.penext --approve
pentool extension run stamp logo.pen --args '{"text":"SALE"}' --dry-run
pentool extension list
pentool extension remove stamp
```

## Design systems and brand kits

`pentool design` inspects tokens, packs token libraries as `.penpkg` files and
vendors them into documents (`docs/design-systems-v1.md`). A library may carry a
brand kit in `design_library.brand`: named colors and type (by token ID), logo
asset references with usage limits, spot-ink and ICC profile references, and
advisory usage rules. No `.pen` format bump.

```sh
pentool design library init ./brand-lib acme/brand
pentool design library pack ./brand-lib brand-1.0.0.penpkg
pentool design brand apply brand-1.0.0.penpkg doc.pen --dry-run
pentool design brand apply brand-1.0.0.penpkg doc.pen
pentool design brand show doc.pen
```

`brand apply` vendors only the tokens the kit names (plus alias targets) and sets
`design.brand` in one transaction. A different local brand kit is a `conflict`,
never overwritten. `pack` refuses ICC profiles whose redistribution is
`restricted` or `unknown` unless `--allow-restricted-local` is given.

A library can also ship templates: add template documents (with `asset` metadata) under
`assets/` and list them in the library block as
`"templates": {"social/post": "assets/social-post.pen"}`. `pack` checks every listed
template with `design template check`; the template's hash is covered by the package
manifest, and `inspect`/`verify`/`pack` report `templates` and `template_names`.

### Templates

A template is an ordinary v4+ `.pen` document with a `design.template`
declaration of typed parameters (`text number bool color token asset`), each
mapped to `{object, property}` targets. `fill` writes a new document and never
touches the template.

```sh
pentool design template check post.pen
pentool design template list ./templates --limit 20
pentool design template fill post.pen --values values.json --output launch.pen --dry-run
pentool design template fill post.pen --values values.json --output launch.pen --if-revision <sha256>
```

Fill many rows at once (JSON array or CSV, at most 10,000 rows; one output per row,
all rows validated before anything is written, existing outputs never overwritten):

```sh
pentool design template fill post.pen --data rows.csv --output-dir ./cards --dry-run
pentool design template fill post.pen --data rows.csv --output-dir ./cards
```

`values.json` is `{"headline": "Launch day", "accent": "color.brand"}`. Missing
parameters use their defaults. Every problem (unknown name, constraint failure,
missing required value, locked or missing target) is reported at once as
`invalid-input`, and a failed fill writes nothing. The `changes` list is paginated
with `--offset/--limit` (`matches returned offset limit has_more`).

### Audit and fix plans

`design audit` is read-only: it compares literal paint, text and corner values with
the document's own tokens, reports unresolved or invalid token references, and
evaluates the brand kit's usage rules (`contrast`, `forbidden_pairs`, `logo`,
`fonts`).

```sh
pentool design audit doc.pen --limit 50
pentool design audit doc.pen --all-pages --rules color,refs --offset 50
pentool design audit doc.pen --fix-plan > audit.json
pentool design audit doc.pen --fail-on error      # exit status 3 when errors exist
```

Findings are sorted in document order and paginated (`matches returned offset
limit has_more`; default `--limit 50`, max 500; `summary` counts the whole result).
`--fix-plan` adds `plan.operations`, plain `bind-token` batch operations for the
exact matches on the returned page (`--fix-nearest` adds deltaE <= 2 matches);
locked nodes are listed under `plan.skipped`. Apply a plan with the existing batch
command (use `--page` for the plan's `page`):

```sh
pentool batch doc.pen ops.json --dry-run     # ops.json is the plan.operations array
pentool batch doc.pen ops.json
```

The batch operations `{"type":"bind-token","id":"hero","property":"fill","token":"color.accent"}`
and `{"type":"set-token","name":"space.md","token_type":"spacing","value":16}` are
available to any batch file.

### Token files (import and export)

Tokens move in and out of a document as a native `pentool.tokens` JSON file
(`{"schema":1,"tokens":{"color.accent":{"type":"color","value":"#F59E0B"}}}`) or as
the strict one-to-one subset of W3C design tokens (`$type`/`$value`). Import is
validated in full before any write, uses the same planner for `--dry-run` and
commit, and never overwrites a differing local token unless `--on-collision
overwrite` is given (default `fail` reports every conflict). Unsupported W3C
features (`$extensions`, `rem`, gradients, color objects, ...) are rejected with
`unsupported-token-construct` and a JSON pointer, never approximated.

```sh
pentool design token export doc.pen --output tokens.json
pentool design token export doc.pen --format w3c --output tokens.w3c.json
pentool design token import other.pen tokens.json --dry-run
pentool design token import other.pen tokens.json --on-collision skip
pentool design library export brand-1.0.0.penpkg --format w3c --skip-unmappable --output brand.w3c.json
```

See `docs/design-systems-v1.md` section 10 and "Implementation notes: token files".

## Open design libraries and packages

Reusable assets remain ordinary editable `.pen` documents. Save a document, page,
layer, object list, or rectangular selection, then register its folder as a local
library:

```sh
pentool asset create design.pen assets/card.pen --id ui/card --name "Card" --layer card
pentool asset create design.pen assets/icons.pen --id icons/navigation --name "Navigation icons" --rect 20 20 400 240
pentool library add ./assets --name project-assets
pentool library refresh project-assets
pentool explore card --category layout
pentool asset preview project-assets/ui/card --output card.png
```

Place an editable copy or an offline-safe instance. Instances retain materialized
content, so documents continue to render when the source library is unavailable.
`add` and `import` place into legacy v1-v3 documents; in a v4+ scene graph (the
`pentool new` default) reuse artwork as a component instead (`pentool group DOC promote GROUP --component NAME`,
then `group DOC instantiate`); both commands say so rather than failing obscurely.

```sh
pentool add poster.pen project-assets/ui/card --mode copy --at 100 80
pentool add poster.pen project-assets/ui/card --mode instance --at 400 80
pentool instance poster.pen list
pentool instance poster.pen detach instance-1
```

Versioned libraries use deterministic, data-only `.penpkg` archives. Filesystem
registries are fully functional static registries and can be hosted or mirrored by
ordinary file synchronization; the protocol does not depend on an official server.

```sh
pentool package init ./open-ui --name open-design/ui
pentool package pack ./open-ui --output open-ui-0.1.0.penpkg
pentool package verify open-ui-0.1.0.penpkg
pentool package keygen ./publisher
pentool package sign open-ui-0.1.0.penpkg --key publisher.key
pentool package verify open-ui-0.1.0.penpkg --signature open-ui-0.1.0.penpkg.sig.json --public-key publisher.pub
pentool package publish open-ui-0.1.0.penpkg --registry ./registry
pentool registry search ./registry open-design
pentool package install open-design/ui@0.1.0 --registry ./registry
pentool lock verify
pentool lock sync --offline
```

Installation verifies the archive and asset hashes, writes `pentool.lock`, and
activates the installed package in the project asset index. Package versions are
immutable. See the open, host-independent [asset and package protocol](../docs/PROTOCOL.md).

Instance changes are always reviewed and explicit:

```sh
pentool instance design.pen updates instance-1 --source card-v2.pen
pentool instance design.pen update instance-1 --source card-v2.pen --dry-run
pentool instance design.pen update instance-1 --source card-v2.pen
pentool instance design.pen rollback instance-1
```

Expose stable asset properties without editing JSON by hand:

```sh
pentool asset create design.pen assets/chrome.pen --id ai/chrome --name Chrome --layer content \
  --property 'counter=text:counter-label.content'
pentool asset property add assets/chrome.pen ai/chrome active \
  --target active-segment --field visible --default true
pentool asset property list assets/chrome.pen ai/chrome --json
pentool asset property validate assets/chrome.pen ai/chrome
```

Property definitions may also be supplied with `--schema property.json`. Mutating
property commands support `--dry-run` and `--if-revision`.

## Collaboration operation log

Collaboration is opt in per document. `collab init` starts a hash-chained,
append-only operation log under `.pentool/collab/`; from then on every
mutation made through the shared transaction path (CLI commands, `batch`,
`undo`/`redo`/`restore`) is also recorded as intent-level operations stamped
with an actor. Without `collab init`, nothing changes and no files are added.
Replay rebuilds the document from the log; any delivery order of the same
operations converges to the same bytes, and conflicts are reported by name
(`overwritten-edit`, `edit-on-deleted`, `duplicate-id`, `reparent-cycle`,
`parent-deleted`, `locked`, `limit-exceeded`).

```sh
pentool collab init artwork.pen
pentool batch artwork.pen ops.json --actor-kind agent --actor-id design-bot --directed-by alice
pentool collab log artwork.pen --limit 20
pentool collab verify artwork.pen
pentool collab replay artwork.pen --out replayed.pen
```

The default actor is a human taken from the OS user. Agents pass the global
`--actor-kind agent --actor-id ID --directed-by HUMAN [--actor-name NAME]`
flags or the `PENTOOL_ACTOR_*` environment variables. `collab log` is paginated
(`matches`, `returned`, `offset`, `limit`, `has_more`; `--actor ID` filters).
`collab replay` never modifies the document. See `docs/collaboration-v1.md`.

### Sync, tokens and roles

`pentool serve DOC --collab` adds `/api/collab/*` (plain HTTP plus SSE, no new
dependency) and enforces roles on every route. Roles are `viewer`, `commenter`,
`editor` and `owner`; only the owner issues or revokes tokens and changes roles.
The server binds to loopback; a non-loopback `--host` is refused until a token
exists and `--allow-insecure-lan` accepts plain HTTP on a trusted network. The
host's own loopback browser acts as owner; anything else needs a token in an
`Authorization: Bearer` header. Tokens are random 256-bit secrets shown once;
only a SHA-256 hash, the role, actor identity and expiry are stored (under
`.pentool/collab/`, never in the `.pen`).

```sh
# owner
pentool collab init artwork.pen
pentool --actor-id bob collab token create artwork.pen --role editor --name bob-laptop --expires 7d
pentool serve artwork.pen --collab

# collaborator (token from PENTOOL_COLLAB_TOKEN or --token)
pentool collab join copy.pen --peer http://127.0.0.1:4711 --token ptc_...
pentool --actor-id bob batch copy.pen ops.json        # work offline
pentool collab peer list copy.pen
pentool collab sync copy.pen --peer http://127.0.0.1:4711 --token ptc_...

# owner housekeeping
pentool collab token list artwork.pen
pentool collab token revoke artwork.pen TOKEN_ID
```

`collab sync` pulls the operations it lacks, then pushes its own; it is
idempotent, resumable after an interruption and never changes the document when
a step fails. The server derives each operation's actor from the token and
rejects operations by anyone else.

Contribution stats are derived only from the verified log, per actor, kind
(human or agent) or page, with exact shares in basis points:

```sh
pentool stats artwork.pen --by kind
pentool stats artwork.pen --by actor --json --limit 20
```

A compact `collab` summary is kept inside the `.pen` and is stripped from
packages unless `pentool package pack --include-attribution` is given (rendered exports never contain it)
(`pentool collab strip artwork.pen` removes it).

Presence (who is connected, their page, selection and cursor) is best-effort and
held in memory only: it is never written to the document, history or log, and
expires about 15 seconds after a participant's last update.

```sh
pentool collab presence copy.pen --peer http://127.0.0.1:4711 --token ptc_...
pentool collab presence copy.pen --peer http://127.0.0.1:4711 --token ptc_... --json
```
