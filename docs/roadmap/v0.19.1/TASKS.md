# v0.19.1 — Studio workflow redesign

This user-directed release is an exception to the patch-only bug-fix policy:
the user chose v0.19.1 for a Studio UI redesign after v0.19.0 had shipped.
This release changes the browser experience while preserving the `.pen` format
and existing CLI behavior. Small server endpoints may be added where the UI
cannot otherwise complete an existing workflow.

## Product goal

Make Pentool feel like one project environment for vector design, image
compositing, raster painting, photography, page layout, packaging mockups, and
print preparation. Help people start from a useful canvas or open a project,
then make the next action discoverable without presenting every subsystem at
once. Keep existing tools honest about what is implemented and which actions
still require the CLI.

The document remains one editable `.pen` project. Photography, pixel painting,
and 3D keep dedicated stages where their interaction models need them; their
project context, navigation, page selection, history, inspector, and export
actions remain part of the Studio shell.

## Current code and user-facing gaps

- The former shell placed nine separate workspace buttons beside New/Open,
  Save, Undo/Redo, PNG/SVG export, reload, and theme actions in one row.
- The right dock stacked Properties, Layers, and a large workspace panel. Users
  had to scroll through unrelated panels to reach the controls for the current
  task.
- The left rail only exposed vector tools; paint tools were all in the right
  panel even though painting is a primary creative action.
- Creating a document used a fixed 1200 × 800 canvas, with no entry point that
  helped users open or resume a project or choose a common size.
- Photo catalog operations could be used in Studio after import, but importing
  DNGs and placing the developed result onto a design page required CLI work.
- PDF/EPUB export endpoints existed but were not available from the main Studio
  export control.
- UI/UX and responsive web output remain governed by the v0.20 roadmap. This
  release must not introduce a competing responsive design model.

## User journeys

1. Start from a welcome screen, resume the loaded project, open `.pen`, or make
   a canvas from a social, poster, screen, A4, or custom pixel size.
2. Create a logo or poster with vector tools, bring in supported artwork or
   images, combine them, and switch to raster tools when pixel retouching is
   needed.
3. Import a DNG photo shoot, cull and develop a photo, place the result on a
   project page, and continue designing without leaving the project.
4. Build a multi-page book or editorial layout, inspect layout status, and
   export PDF/EPUB from the same document.
5. Design a package surface, link project artwork to a 3D mockup, inspect the
   render, and prepare print output with preflight.
6. Move among the project workflows while retaining the same document, pages,
   layers, selection, undo history, and save/export actions.

## Implementation plan

- [x] **1. Project entry:** welcome screen, continue current document, open a
  `.pen` file, canvas presets, bounded custom size, and return to start without
  discarding the in-memory document.
- [x] **2. Information architecture:** group the primary areas into Design,
  Photo, Layout, Packaging, Print, and Extensions in one workspace selector.
  Keep one application bar, with contextual settings for the active tool. All implemented features
  remain reachable and labels describe the actual capability.
- [x] **3. Contextual editing shell:** replace the stacked right dock with
  Properties, Tools, and Assets above an independently visible, resizable layer
  tree. Keep vector and raster tools together in the left rail. Preserve visited
  stages and suspend inactive shortcuts and preview requests.
- [x] **4. Photo to design path:** add bounded DNG/JPEG/PNG import to the served
  project and an action to place a developed photo on the active page. Both
  operations use revision guards and atomic history; reject invalid/stale
  requests without modifying the source. Rendered JPEG/PNG sources support
  global tone, presence, color, detail and effects edits; DNG retains camera
  white balance, camera profiles and RAW controls.
- [x] **5. Publishing actions:** expose current-page PNG/SVG and document
  PDF/EPUB export in one accessible menu. Keep the Print workflow's preflight,
  plates, and proof actions discoverable.
- [ ] **6. Responsive and accessible behavior:** preserve visible focus, keyboard
  navigation, honest loading/empty/error states, and usable layouts at 375,
  768, and 1280 px.
- [ ] **7. Evidence and release:** update Studio and protocol docs, verify the
  UI/build, run focused server tests and CI/release gates, and inspect the
  packaged artifacts before publishing.

## v0.19.1 acceptance

- The start screen offers continue, `.pen` open, common canvas presets, and
  validated custom dimensions from 1 to 10,000 pixels per side.
- One application bar exposes the task areas through a workspace selector and
  settings for the active tool, without stacked navigation rows.
- The inspector presents Properties, Tools, and Assets above a separate layer
  tree. Selecting an object exposes its properties without hiding the layer tree.
  Panel width and layer area support pointer and keyboard resizing.
- Vector, paint, and retouch tools remain available together in the left rail.
  Design and Paint use the same page framing and zoom; specialized interaction
  surfaces remain separate. Hidden stages do not handle editing shortcuts.
- Photo UI imports DNG files up to 64 MiB into a shared served project and can
  place the developed variant on the active page. Both operations are
  revision-guarded, undoable, and leave the file unchanged on failure.
- The export menu exposes PNG, SVG, tagged PDF, and EPUB from unsaved document
  state where the existing API supports it.
- Existing `.pen` structure, CLI operations, workspace-specific capabilities,
  and explicit partial-feature limitations remain intact.
- Studio and API documentation match the implemented workflows. The embedded
  `web/studio.html` matches source, and CI passes before a release tag is made.
