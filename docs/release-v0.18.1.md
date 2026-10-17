# Pentool v0.18.1

This user-requested release is an exception to the patch-only bug-fix policy:
it adds the first slice of the Studio workflow redesign after v0.18.0 had
already shipped. Future releases should use the documented semantic versioning
policy.

## Studio project entry

- New startup welcome screen with Continue, Open `.pen`, and canvas creation.
- Canvas presets for social square, poster, screen, and A4 page, plus custom
  dimensions from 1 to 10,000 pixels per side.
- Home action returns to the welcome screen while keeping the current project
  in memory. Unsaved edits are protected before replacing the document.
- Workflow redesign scope and remaining tasks are tracked in
  [`roadmap/v0.18.1/TASKS.md`](roadmap/v0.18.1/TASKS.md).

The existing editor workspaces and project formats are unchanged. This is the
entry-flow foundation; workspace consolidation, shared canvas behavior, and
cross-project asset flows remain future work.
