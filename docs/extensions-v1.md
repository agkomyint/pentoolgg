# Pentool extensions v1

Extensions are sandboxed WebAssembly modules that read a document and propose
edits. Every effect goes through the same scene batch planner and the same
transaction as `pentool batch`: dry run, revision guard, atomic write, one
history entry, rollback on any failure. An extension can do nothing the CLI
cannot.

Status: API revision `1`, implemented in v0.18.0 for **v4+ scene documents**.
Legacy v1-v3 documents are refused with a clear error (migrate them first).
Verified locally on Windows only; Linux and macOS evidence comes from CI.

## Engine decision

The engine is [`wasmi`](https://github.com/wasmi-labs/wasmi) 2.x, a pure Rust
WebAssembly interpreter, with features `std`, `validate`, `stable`,
`deterministic` and `portable-dispatch`, host-only (not compiled for the WASM
renderer target).

Why a WebAssembly interpreter rather than an embedded scripting language:

- the sandbox is the VM: a module has no ambient authority, only the imports we
  grant, and we grant exactly one;
- CPU limits are deterministic *fuel*, not wall-clock timers, so the same input
  stops at the same instruction on every machine;
- authors can write extensions in Rust, C, Zig, AssemblyScript and others, and the
  `.wasm` is content-addressable, which fits the package model;
- no Node.js, Python or system interpreter is needed.

Alternatives considered: a JIT runtime (`wasmtime`) was rejected by reasoning
only (large binary, JIT code generation conflicts with the "small, auditable,
deterministic" goal); embedded script interpreters (Rhai, Boa, mlua) were
rejected because their sandboxes are weaker and their resource limits are
harder to make deterministic. Only `wasmi`'s size was measured, below.

`portable-dispatch` is required: without it debug builds overflow the native
stack on wasm loops.

Raising `rust-version` from 1.85 to **1.86** was required by `wasmi` 2.

### Binary-size budget

The CI release-binary budget is 60 MB. Like-for-like Windows x64 release builds
(`cargo build --release --locked`, same machine and toolchain):

| Build | Bytes |
| --- | --- |
| v0.16.1 source (`9bf4a13`) | 42,043,392 |
| with `wasmi` and the extension host | 43,572,224 |
| Delta | +1,528,832 (+3.6%) |

On the merged v0.18.0 tree (v0.17.1 plus extensions, version 0.18.0) the same
Windows x64 release build is **45,156,864 bytes**, still under the budget. No
v0.17.1 baseline was built, so the 0.17 share of that growth is not separated.

The second figure was measured before the Studio routes and panel were added;
those add little to the Rust binary (the embedded `studio.html` grew by a few
KB). CI's `binary-size` jobs will report the Linux and macOS figures.

## Threat model

Assumed hostile: the extension module, its manifest, its arguments' effect on
its own control flow, and the `.penext` archive.

Not in scope: a malicious *user* (they own the machine), side channels, and
bugs in `wasmi` itself (mitigated by keeping the surface to one import and
pinning `Cargo.lock`).

| Threat | Mitigation |
| --- | --- |
| Reads or writes files, spawns processes, reaches the network, reads the clock or randomness | The only permitted import is `pentool.call`. Any other import (including WASI) makes the module fail before it runs. |
| Edits without consent | `document.edit` is a manifest capability that `extension add` shows and requires `--approve` for. Approval is recorded with the package and entry hashes and re-checked on every run. |
| Edits a locked layer or object | The host compares the batch result with the document and rejects any change under a locked layer or node. |
| Infinite loop or heavy compute | Fuel: `limits.cpu_ms x 100,000` units (default 5 s, max 60 s). Each host call also costs 1,000 units. Exhaustion traps; nothing is written. |
| Memory exhaustion | `limits.memory_mb` (default 64, max 256) enforced by a resource limiter that traps on a failed grow. |
| Deep recursion | wasm stack overflow traps inside the VM; the host process survives. |
| Call floods | At most 10,000 host calls per run. |
| Oversized data | Module 8 MiB, request 1 MiB, response 4 MiB, result 1 MiB, 100 log lines of 1 KiB, tree/search page limit clamped to 200. |
| Out-of-bounds guest pointers | Every pointer and length is bounds-checked; a bad one traps the run. |
| Tampered archive or install | `.penext` carries a sha256 digest index; `extension verify` and `extension add` check it offline. `extension run` re-hashes the installed entry module. |
| Archive path tricks, symlinks, zip bombs | Unsafe paths and symlinks are rejected; at most 256 files and 32 MiB. |
| Partial writes | Edits run on an in-memory copy; one `commit_value` happens at the end. A trap, non-zero status, or failed batch leaves the file byte-for-byte unchanged. |

## Package format

`pentool extension pack DIR OUT.penext` builds a deterministic zip:
`extension.json`, `files.json` (sha256 per file), the entry module and any
other files in `DIR`. The same input always yields the same bytes.

Manifest (`extension.json`, unknown fields and duplicate keys are rejected):

```json
{"name": "stamp", "version": "1.0.0", "api": 1,
 "description": "Add a labelled badge to the page.",
 "capabilities": ["document.read", "document.edit"],
 "entry": "main.wasm",
 "limits": {"cpu_ms": 2000, "memory_mb": 16},
 "commands": [{"id": "stamp", "title": "Add a badge"}]}
```

Capabilities: `document.read`, `document.edit` (requires `document.read`).
There are no others; there is no way to ask for the network or the filesystem.

Packages are ordinary files, so `pentool package sign` and
`verify-signature` work on a `.penext` for publisher signatures.

## CLI

```sh
pentool extension pack examples/extensions/stamp/package stamp.penext
pentool extension verify stamp.penext             # offline: digests, manifest, module
pentool extension add stamp.penext                # prints capabilities, installs nothing
pentool extension add stamp.penext --approve      # installs into .pentool/extensions/stamp
pentool extension list
pentool extension run stamp art.pen --command stamp --args '{"text":"SALE"}' --dry-run
pentool extension run stamp art.pen --args '{"text":"SALE"}' --if-revision <sha>
pentool extension remove stamp
```

`--page` selects the page, as everywhere. All commands print JSON. A run
reports `changed`, `change` (revision before/after, as other mutations), the
applied `changes`, `result`, `logs`, `calls` and `fuel_used`. `pentool undo`
reverts a run in one step.

## Guest ABI

The module exports `memory` and `run() -> i32` (0 = success; any other status
aborts and writes nothing) and imports a single function:

```
pentool.call(req_ptr, req_len, out_ptr, out_cap) -> i32
```

The request is UTF-8 JSON `{"method": "...", "params": {...}}`. The response is
`{"ok": true, "result": ...}` or `{"ok": false, "error": {"code", "message"}}`.
A non-negative return is the number of bytes written to `out_ptr`. A negative
return is `-needed`: the response did not fit, so call again with `req_len = 0`
and a larger buffer to read the same pending response.

`examples/extensions/sdk` is a small Rust helper for this ABI.

## Methods (frozen for api 1)

| Method | Capability | Params | Result |
| --- | --- | --- | --- |
| `api.info` | none | | api, extension, version, capabilities, dry_run, revision |
| `args` | none | | `{command, args, event}` as given to `extension run` |
| `log` | none | `{message}` | null (bounded) |
| `result` | none | `{value}` | null; becomes `result` in the run report |
| `document.info` | read | | version, revision, pages, layers (`id`, `locked`, `visible`), edited |
| `document.tree` | read | `{page?, kind?, layer?, offset?, limit?}` | the CLI `tree` JSON (paginated) |
| `document.search` | read | `{query, page?, kind?, layer?, offset?, limit?}` | the CLI `search` JSON (paginated) |
| `document.get` | read | `{id, page?}` | `{page, layer, locked, node}` or error `not-found` |
| `document.plan` | edit | `{operations, page?}` | `{would_change, changes}`; never mutates |
| `document.apply` | edit | `{operations, page?}` | `{changes}`; later reads see the change; nothing is written until `run` returns 0 |

`operations` is the `pentool batch` operation list. Error codes:
`capability-denied`, `unknown-method`, `malformed-request`, `invalid-operation`,
`operation-failed`, `not-found`, `limit-exceeded`.

Events (selection changed, document opened) are read-only: the caller may pass
an event with `--event JSON` and the module reads it from `args`. Studio does
not send events yet.

## Determinism

Given the same document, arguments and module, a run produces the same edits:
the interpreter is built with `wasmi`'s `deterministic` feature, there is no
clock or randomness import, limits are fuel, not time, and the host methods
return deterministic JSON. A test runs the same extension twice and compares
documents.

## Editor

Studio has an **Extensions** workspace and a Ctrl/Cmd+K palette. They list
installed extensions with their approved capabilities and limits and run them
against the served document (`GET /api/extensions`, `POST /api/extensions/run`
with the document revision). Installing and approving capabilities remain CLI
actions. Running is disabled while there are unsaved local edits, because the
run writes the served file.

The HTTP endpoints are covered by an integration test. The Ctrl/Cmd+K palette
was driven in headless Edge on Windows against a served document (open palette,
run "Add a badge", nodes `stamp-bg` and `stamp-label` appear in the file). The
Extensions workspace tab was also driven there (open the tab, Preview reports
"preview only, nothing written"); the only failed request was `/favicon.ico`.
Other browsers and hands-on use have not been exercised.
The `revision` sent by Studio is the document revision from
`GET /api/document`; a stale or missing one is rejected before anything runs.

## Known limits

- Batch operations only: an extension cannot do anything `scene::apply_batch_at`
  does not support.
- No marketplace, network-capable extensions, or native plugins (deferred).
- Edits to a locked layer or node are rejected by the extension host. v0.18.0
  also fixed plain `pentool batch`, whose `put-shape`, `put-text` and
  `put-path` previously ignored layer locks.
