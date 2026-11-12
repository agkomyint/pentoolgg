# Architecture decision record: `pentool-core` crate split

Status: accepted (maintainer request, roadmap item 2 of
`docs/roadmap/architecture/TASKS.md`). Behavior-preserving; no format, CLI or
JSON change.

## Context

The `pentool` library already compiled to `wasm32-unknown-unknown` (the
`pentool-wasm` browser build) by gating host-only modules with
`#[cfg(not(target_arch = "wasm32"))]`. That gating shows where the seam is: no
module other than `server`, `ai` and `extension` depends on those three, and
the networked half of `collab` is only reached from them. The spike required by roadmap item 1 is therefore already measured by
the shipping `web/pentool_core.wasm`.

## Decision

Two crates in one Cargo workspace:

| Crate | Path | Contains |
| --- | --- | --- |
| `pentool-core` | `crates/pentool-core` | Document and scene model, validation, migration, geometry, text, color, raster, photo, image, composite, the deterministic CPU renderer, PDF/PSD/IDML/SVG import and export, transactions, history, libraries and packages, design systems, and the collaboration operation log (`collab::{op,log,merge,claims,hlc,stats}`). |
| `pentool` | `.` | The `pentool` binary, the HTTP server and Studio embedding (`server`), AI, extensions (`wasmi`), and the networked collaboration code (`collab::{sync,presence,perm,token,evidence}`). |

`pentool` re-exports every core module under its old path, so `pentool::scene`,
`pentool::collab::content_revision` and the rest keep working. Integration
tests, fuzz targets and `pentool-wasm` need no source change.

## Invariants

- Output bytes do not change. `pentool-core` inherits the workspace version so
  `env!("CARGO_PKG_VERSION")` (photo metadata, render-cache keys) is identical.
- The CPU renderer stays normative; `gpu` is forwarded from `pentool` to
  `pentool-core` and stays optional.
- `pentool-core` must never depend on `pentool`. Anything it needs from the
  binary is a sign the code belongs in the core or behind a trait.
- `cargo test`, `cargo clippy --all-targets` and `cargo check --all-targets`
  from the repository root cover both crates (`default-members`).

## Known debt (not fixed by this split)

The core still contains host-only code behind `cfg(not(target_arch = "wasm32"))`
and uses `std::fs` widely. Moving file, process and network access fully into
the binary is a later, behavior-changing step. Remaining host dependencies in
the core: `reqwest` and `tokio` (`scene3d::cli`, `scene3d::node`, `package`),
`walkdir` (`design::library`, `library`, `package`), `directories` (`library`),
`ed25519-dalek` (`package`), `rand_core` (`linked`, `benchmark`, `package`) and
`wgpu`/`pollster` (`scene3d::gpu`, feature `gpu`). All are `cfg(not(wasm32))`.
`library` and `package` stayed in the core because `design` and `scene3d` depend
on them; separating a pure engine from the network-capable asset layer is a
follow-up.

## Moved while splitting

- `collab::claims` (claims and `check_claims`) moved out of `collab::sync` because
  `collab::begin`, which every mutation path calls, must honour claims. `sync`
  re-exports it, so `pentool::collab::sync::check_claims` still resolves.
- A handful of `pub(crate)` items became `pub` because the server, AI and
  collaboration-sync code now reach them across the crate boundary.

## Status

Roadmap item 2 stays unticked until the three-OS CI run is green for this change.
