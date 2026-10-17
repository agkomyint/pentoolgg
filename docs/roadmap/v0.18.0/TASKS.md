# v0.18.0 — Scripting and extension API

Give people and agents a safe way to extend Pentool: sandboxed scripts that read
documents and propose edits through the same planner every command uses.

## Product contract

- Extensions can only do what the CLI can do: every effect is a transaction
  through the shared planner, with dry run, revision guards, locks and rollback.
- Sandboxed by default: no filesystem outside the project, no network, no
  process spawning, and bounded CPU time and memory. Capabilities are explicit,
  declared in a manifest and shown to the user before first run.
- Deterministic: a script given the same document and inputs yields the same
  edit plan. Clocks and randomness are unavailable unless seeded explicitly.
- No Node.js or system interpreter is required. The script engine is embedded
  Rust (a small, audited, pure Rust interpreter or WASM runtime), chosen in the
  specification task against the binary-size budget.
- Extensions are versioned, content-addressed packages that verify offline,
  using the existing `package` mechanism.

## Model

```json
{"name": "align-to-grid", "version": "1.0.0", "api": 1,
 "capabilities": ["document.read", "document.edit"],
 "entry": "main.wasm", "limits": {"cpu_ms": 5000, "memory_mb": 64}}
```

- The API exposes the same compact, paginated tree and search results as the
  CLI, plus plan, apply and dry-run calls.
- Events (selection changed, document opened) are read-only notifications.

## Must ship, in order

- [ ] **1. Choose the engine and freeze the API,** with a binary-size budget and
  a threat model in `docs/extensions-v1.md`.
- [ ] **2. Sandbox and limits** with tests for escape attempts and resource
  exhaustion.
- [ ] **3. Document API:** read, search, plan, apply, dry-run, all through the
  shared transaction path.
- [ ] **4. Manifest, capability prompts, packaging and offline verification.**
- [ ] **5. CLI:** `extension add|list|run|remove` with JSON results.
- [ ] **6. Editor:** extension panel and command palette integration.
- [ ] **7. Examples and conformance tests,** including a hostile-script suite.

## Explicitly deferred

- A public marketplace, network-capable extensions, and native (non-sandboxed)
  plugins.

## Status

All seven items are implemented on branch `feat/v0.18.0-extensions` (see
`docs/extensions-v1.md`) with passing local tests on Windows (full suite,
Clippy and rustfmt clean). Checkboxes stay open until the three-OS CI run
provides cross-platform evidence. Studio (item 6) has not been exercised in a
live browser, and a like-for-like binary-size delta against v0.16.1 is still
to be read from CI's `binary-size` jobs.
