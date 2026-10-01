# v0.2.0 — Editable typography and sharp edges

Release highlights and compatibility notes.

- Real editable text objects, CLI create/update/remove, and browser text tool.
- Configurable sharp/round/beveled stroke joins, flat/round/square caps, miter limit, browser controls, and geometry-preserving CLI path styling.
- Font family, size, weight, italic, color, alignment, spacing, and multiline leading.
- Bundled Atkinson Hyperlegible in four styles; installed font discovery and custom TTF/OTF embedding.
- Native browser-free text shaping, PNG rendering, live SVG with embedded fonts, and optional outlined SVG export.
- Text bounds and affine transforms, including atomic whole-layer path/text transforms and layer locks.
- Version 2 documents with version 1 reading compatibility and retained extension fields.
- Revision-token shared saves avoid browser integer/float comparison conflicts while rejecting stale edits.
- Typography demo, license attribution, and typography regression tests.

Limitations: explicit line breaks only; no rich text, automatic wrapping,
variable-font axes, or text-on-path. SVG consumer support for embedded fonts
varies; use outlined SVG when exact portability matters. Fonts must cover the
characters used; the bundled font does not cover every writing system.

Release workflow tests and builds Windows, Linux, Intel macOS, and Apple Silicon
macOS, then creates a draft release with archives and SHA256 checksums. Publishing
requires the release tag and review of the resulting CI artifacts.
