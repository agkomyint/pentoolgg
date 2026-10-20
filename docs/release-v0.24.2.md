# Pentool 0.24.2 (internal, unreleased)

Patch notes. Not published: the next public release is 0.25.0.

## Fixes

- Parent-page items now paint in PNG, SVG, PDF and Studio renders. They were
  resolved by `parent resolve` but never drawn. They render beneath the page's
  own layers, honour overrides and detaches, and hidden parent layers.
- `pentool spread <doc> facing <spread> true|false` accepts its value. The
  argument was parsed as a flag, so facing could not be turned on.

## Known gaps (not patches)

- Placed tables are not laid out or drawn by the renderer; only their story
  text is. Table layout needs a decision before work starts.
- Paragraph styles have no colour property.
