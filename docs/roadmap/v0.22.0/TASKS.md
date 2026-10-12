# v0.22.0 — Hosted Pentool (self-hostable service)

Make it practical to run Pentool as a multi-user service. The product stays one
offline-capable Rust binary: hosting is a deployment mode, not a different
product, and every feature still works locally without an account.

## Product contract

- Isolation first: one workspace per user or team, with a separate process or
  container per workspace. `pentool serve` itself gains authentication, but the
  default posture of the binary remains local-only and unauthenticated on
  `127.0.0.1`, unchanged.
- Authentication is required whenever `serve` binds a non-loopback address; it
  refuses to start otherwise unless an explicit, loudly named flag is passed.
- All storage stays file based and portable. A workspace can be exported as a
  collected project and opened on a laptop with no loss.
- Quotas (storage, CPU time, memory, concurrent exports) are enforced and
  reported, and a quota failure leaves documents unchanged.
- Hosting adds no hidden network access to open, validate or render.
- The hosted layer is optional: a deployment guide and container image are
  provided, but the single binary remains the supported artifact.

## Must ship, in order

- [ ] **1. Threat model and deployment guide** (`docs/hosting-v1.md`): path
  containment review of every API route, untrusted-file handling, secrets.
- [ ] **2. Authentication** (tokens and an OIDC-compatible mode) and the
  non-loopback refusal rule.
- [ ] **3. Workspaces:** per-user storage roots with strict containment and
  quota accounting.
- [ ] **4. Process supervisor mode:** start, idle-stop and wake a workspace
  instance on demand, with resource limits.
- [ ] **5. Audit log and usage metrics** in a compact, machine-readable form.
- [ ] **6. Container image and reference reverse-proxy configuration.**
- [ ] **7. Backup, export and restore** of workspaces as verified packages.
- [ ] **8. Evidence:** isolation tests, a hostile-tenant suite, load test and
  recorded memory per idle and active workspace.

## Explicitly deferred

- Billing, a managed commercial offering and a public marketplace. These are
  business decisions outside the repository.
- Multi-tenant single-process serving (many users inside one process).
