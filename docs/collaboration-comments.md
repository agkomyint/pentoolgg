# Comments, mentions and review states (v0.20.0 task 4)

Comments are review data, not design data. They are stored outside the scene
graph and never change rendering, export, the `.pen` bytes or its revision.

## Storage

- Sidecar: `<document dir>/.pentool/comments/<safe-name>.json`, where the safe
  name replaces every character outside `[A-Za-z0-9._-]` with `_` (the same
  rule as `.pentool/history`).
- Schema 1: `{ "schema", "next_thread", "next_comment", "threads": [...] }`.
  Thread ids are `t-N` and comment ids are `c-N`, allocated from the counters
  under a lock, so ids are deterministic, unique and never reused.
- Writes take an inter-process lock and use the shared atomic writer. A
  malformed, unsupported or oversized sidecar is reported as
  `[invalid-input]` or `[limit-exceeded]` and left untouched.

## Model

```json
{
  "id": "t-1",
  "target": { "page": "page-1", "object": "title", "point": [12.0, 8.0] },
  "status": "open",
  "review_state": "none",
  "version": 1,
  "comments": [{
    "id": "c-1",
    "author": { "kind": "human", "id": "ana", "name": "Ana" },
    "body": "Tighten this @bot-1",
    "mentions": ["bot-1"],
    "created_ms": 0
  }]
}
```

- `target` fields are all optional; an empty target comments on the document.
  A given `page` or `object` must exist when the thread is created
  (`[not-found]`). An object named together with a page must be on that page.
- `status` is `open` or `resolved`. `review_state` is `none`, `requested`,
  `approved` or `changes-requested`.
- `author.kind` is `human` or `agent`. Agents may record `model` and
  `directed_by` (the directing human); humans may not.
- Mentions are `@name` tokens (`A-Za-z0-9_.-`, trailing `.`/`-` trimmed),
  deduplicated and sorted. An `@` inside a word (an email address) is not a
  mention.
- `version` starts at 1 and increases on every change to the thread. Mutating
  commands accept `--if-version N` (`if_version` in JSON); a mismatch is
  `[conflict]` and changes nothing. Repeating a state that already holds is a
  successful no-op with `"changed": false` and no version bump.

## Dangling targets

Deleting an object never deletes its threads. `list` marks a thread whose page
or object no longer exists with `"dangling": true` and reports the document-wide
`dangling` count. A dangling thread can still be replied to, reviewed and
resolved.

## Limits

Checked before any write: comment body 16 KiB, 32 mentions per comment, 500
comments per thread, 10,000 threads, 16 MiB sidecar, list `limit` 1..=1000
(default 50). Violations are `[limit-exceeded]`.

## Listing

`pentool comment list` and `GET /api/comments` return threads in creation order
with `matches`, `returned`, `offset`, `limit` and `has_more` describing the
emitted window exactly, plus `total_threads` and `dangling`. Filters: `status`,
`review_state`, `page`, `object`, `mention`, `thread`. Rows are compact (no
bodies) unless `--comments` is given or one `--thread` is selected.

## Not covered here

Permissions (who may comment), authentication of authors and sync between
peers belong to later v0.20.0 tasks. Until then the actor is as asserted by the
caller and is not authenticated.
