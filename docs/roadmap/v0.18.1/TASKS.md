# v0.18.1 — Studio workflow redesign (release exception)

This is a user-directed exception to the repository's patch-only bug-fix policy:
the user requested the Studio redesign to ship as v0.18.1 after v0.18.0 had
already been published. Future releases should return to the documented version
policy. This milestone starts the redesign; it does not promise a complete
replacement of the editor shell.

## Product goal

Make it clear how to start or resume a project, then make existing vector,
image, paint, photography, layout, print, and static 3D capabilities feel like
parts of one project workflow. Keep `.pen` as the editable project, preserve
existing command and file behavior, and do not imply that unfinished tools are
complete.

## Current code and user-facing gaps

- `web-ui/src/App.tsx` loads the served document directly into an editor with a
  9-workspace horizontal switcher, a left vector toolbox, and a contextual
  right dock. There is no start or project-selection screen.
- `web-ui/src/store.ts` creates a fixed 1200 × 800 blank canvas. The existing
  New/Open controls are small toolbar actions; new users cannot choose a
  canvas preset before entering the editor.
- Photo, Paint, and 3D replace the central stage. Design, Images, Composite,
  Print, Layout, and Extensions each have their own dock panel. This is a
  useful foundation, but the modes and stage changes make related work feel
  disconnected.
- Import capabilities already exist in the Design panel and file-drop path,
  but the start flow does not help a user choose a project type or locate those
  workflows.
- UI/UX and responsive web output must align with the v0.20 roadmap's shared
  responsive design model rather than introducing a competing layout system.

## User journeys to support end to end

1. Create a canvas from a common screen, poster/social, or print page size;
   enter a custom pixel size; save as `.pen`.
2. Open an existing `.pen` project and resume without losing its pages, assets,
   layers, layout, or 3D objects.
3. Import vector artwork, photos, or supported design files into an existing
   project; keep placed assets reusable across pages and compositions.
4. Design a brand mark or poster, reuse the artwork in image composites and
   social variants, and export the required formats.
5. Import a photo shoot, select and develop images, retouch or composite them,
   then place the result in a book, layout, or print document.
6. Build a book from written content, illustrations, and images; manage pages,
   text flow, preflight, and export.
7. Design packaging artwork, create or import a static package form, apply the
   design to its surfaces, inspect the mockup, and prepare print output.
8. Create responsive web and UI/UX artwork with reusable assets and inspect
   supported screen sizes, following v0.20's roadmap contract.

## Work plan

- [x] **1. Project entry:** welcome screen, continue current document, open a
  `.pen` file, choose common canvas presets, edit custom dimensions, and return
  to the start screen from the editor.
- [ ] **2. Information architecture:** group the many workspaces by user intent;
  keep every implemented capability discoverable and clearly mark partial or
  planned functionality. Retain keyboard and narrow-window access.
- [ ] **3. Shared canvas workflow:** define which stages can stay on the shared
  canvas and when a specialized Photo, Paint, Layout, or 3D stage is necessary.
  Keep selection, layers, undo, save, and project context consistent.
- [ ] **4. Project asset flow:** make import, reuse, placement, and replacement
  understandable across pages and modes; clearly report format limitations.
- [ ] **5. Workflow presets:** add task-oriented starters only where the current
  document model and export path can carry the promised workflow end to end.
- [ ] **6. Responsive and accessible behavior:** keyboard navigation, visible
  focus, useful empty/loading/error states, and usable 375/768/1280 px layouts.
- [ ] **7. Evidence:** document the supported user journeys and limitations;
  run focused UI checks and the normal CI/release gates before shipping.

## Acceptance for the v0.18.1 first slice

- Startup presents a usable welcome screen while retaining the loaded document
  so the user can continue, open a `.pen`, or create a new canvas.
- Canvas presets create the selected dimensions; custom dimensions are bounded
  to positive values no greater than 10,000 pixels per side.
- The Home control returns to the start screen without discarding the current
  in-memory document.
- Existing editor open, save, and workspace behavior remains available.
- Studio documentation explains the scope and clearly identifies the remaining
  workflow redesign tasks.
