# Photoshop and Figma reference study

Reviewed 2026-10-17. This document proposes UI direction; it does not mark
implementation or performance acceptance complete. Research used the official
documentation and published interface examples, not hands-on access to either app.

## What the references establish

Photoshop gives separate roles to the application bar, creation tools, document
window, tool options, and panels. Related tools are grouped. Tool settings follow
the active tool; contextual actions follow the task or selection.

Figma separates creation tools, document navigation, canvas, and property editing.
Its current documentation includes navigation for files/pages/layers and assets.
Its UI3 design account describes collapsible/resizable panels, predictable control
positions, and the importance of preserving familiar interaction patterns.
The 2024 UI3 screenshots are historical examples; current documentation describes
additional navigation changes. Do not treat one screenshot as a permanent layout.

Sources:

- [Adobe workspace overview](https://helpx.adobe.com/photoshop/desktop/get-started/learn-the-basics/workspace-overview.html)
- [Figma design file overview](https://help.figma.com/hc/en-us/articles/15297425105303-Explore-design-files)
- [Figma navigation and left sidebar](https://help.figma.com/hc/en-us/articles/360039831974-Explore-the-navigation-bar-and-left-sidebar)
- [Figma's UI3 design decisions](https://www.figma.com/blog/behind-our-redesign-ui3/)

## Recommended direction for Pentool

These are our design judgments, rather than claims about reference internals.

| Area | Responsibility in Pentool |
| --- | --- |
| One top bar | Project/file actions, active tool settings, save and export |
| Left tool rail | Select, transform, vector, text, paint, retouch, navigation; related tools grouped |
| Document navigation | Pages, layers, reusable assets; collapsible and available while inspecting |
| Canvas | Keep the artwork visible while selecting tools and adjusting objects |
| Right inspector | Properties of the current selection; useful document settings when nothing is selected |
| Specialized views | RAW development, book overview and 3D mockups only when the task needs them |
| Status | Saving, processing, actionable errors; avoid internal engine explanations |

The user's one-header preference is a Pentool constraint. Photoshop's separate
options bar is a reference for contextual settings, not a reason to add another row.
Use the existing Studio theme. Preserve offline operation and the single Rust binary.

## Gaps observed in current code

- `App.tsx` still switches stages for Paint, Photo and Packaging. Returning mounts
  those stages again. Preview caching reduces repeated renders but does not preserve
  stage view state or prove mode-switch latency is acceptable.
- The right dock puts Layers and Inspect in mutually exclusive tabs. Selecting an
  object changes the tab automatically, making document navigation disappear.
- Workflow panels mix creation, import, settings and technical actions in long forms.
- Paint and vector creation use separate stages. Unifying their coordinates,
  selection, preview and history needs an explicit implementation plan.
- The workspace dropdown reduces header height but makes common tools less visible.
- Empty states need a visible next action and supported-format guidance.
- Errors such as "Failed to fetch" need operation context and a retry path.

## Order for the next iterations

1. Make the shell stable: preserve the canvas view, selection, panels and relevant
   view state when changing tools; measure actual switching before claiming speed.
2. Keep layers available alongside selection properties; add panel resizing/collapse.
3. Group vector and raster tools in the left rail; expose settings for the active
   tool in the single header. Do not duplicate tool buttons across panels.
4. Reduce the inspector to relevant controls, grouping advanced options behind
   clear disclosures. Replace generic workflow introductions with useful actions.
5. Design explicit entry flows for RAW import, package creation and export, with
   progress, recovery and meaningful empty states.
6. Evaluate logo-to-photo placement, retouching, multi-page layout and packaging
   artwork as complete journeys, including save/reopen and keyboard access.

Measure navigation feedback separately from preview completion. A responsive shell
can show progress while a preview is pending; caching alone is insufficient evidence
of responsive interaction or correct cancellation.
