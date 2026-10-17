# v0.19.0 — Design systems, brand kits and templates

Make reusable design decisions first class: tokens, brand kits and templates
that documents reference, update and audit.

## Product contract

- Tokens (color, type, spacing, radius, shadow) are named, versioned values in a
  library that documents reference by ID. Resolving a token is deterministic.
- Updating a library never silently rewrites local overrides (the v0.6.24
  component-override rules apply); the planner reports what would change.
- Libraries are packages: content-addressed, verified offline, installable and
  lockable with the existing `lock` mechanism.
- Brand kits can carry ICC/spot-ink references from v0.12.0 so print colors are
  stated once.
- Templates are ordinary `.pen` documents with declared parameters. Filling a
  template is a transaction with a dry run.

## Must ship, in order

- [x] **1. Specify token and library schema,** resolution order and override
  semantics in `docs/design-systems-v1.md`.
- [x] **2. Token resolution** in the shared scene path with cycle rejection.
- [x] **3. Library packages,** locks, update planning and diff reports.
- [x] **4. Brand kit:** colors, type, logos, ink references and usage rules.
- [x] **5. Template parameters and fill command,** with validation and limits.
- [ ] **6. Audit command:** find off-token colors, fonts and spacing, with fixes
  proposed as a plan.
- [ ] **7. Editor:** token panel, library browser, template gallery.
- [x] **8. Import/export of token files** (a documented JSON format, and
  W3C design tokens where it maps exactly).

## Acceptance targets

- Updating a token changes exactly its dependents and nothing else.
- An audit of a large document is bounded, paginated and deterministic.

## Explicitly deferred

- Cloud-hosted shared libraries and permissioned design-system registries.

## Evidence (v0.19.0)

CI run 39030789006 on PR #202 passed on Linux, Windows and Apple Silicon macOS
(tests, browser-core, binary-size, policy, studio-build). Items 1-5 and 8 are ticked.

Left unticked on purpose:

- **6. Audit:** color, font, radius, reference and brand-rule checks and `--fix-plan`
  are done; spacing audits are not (nodes have no gap/padding property), and
  `tree/search --resolve` and `validate --strict` are not built.
- **7. Editor:** token, library-status, template and audit views exist and the HTTP
  routes are tested, but the panel was not exercised in a browser and has no token
  editing, library install/update/diff or conflict resolution.
