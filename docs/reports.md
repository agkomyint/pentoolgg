# Reports: multiple sources, citations and charts

`pentool report` turns a small JSON spec plus CSV/TSV/JSON data files into a
readable `.pen` report. It exists so agents and people can write a data report
without placing every rectangle and text baseline by hand.

```sh
pentool report example                       # a runnable spec and CSV
pentool report data regions.csv              # columns, numeric ranges, sample rows
pentool report check spec.json --strict      # unknown ids, missing columns, uncited numbers
pentool report build spec.json --out report.pen
pentool export report.pen report.pdf
```

No new dependency, no network, no model. The output is an ordinary v4 document:
open it in Studio, edit it, export PNG/SVG/PDF. Building is deterministic: the
same spec and data give the same bytes.

## Spec

| Field | Meaning |
| --- | --- |
| `title`, `subtitle` | Report name (document name, footer) and optional lead line. |
| `page` | `{width, height}` in px, default 1200 x 1600. |
| `theme` | `accent`, `ink`, `muted`, `background`, `card` (`#RRGGBB`), `font`. |
| `sources` | Every source a number can come from (see below). |
| `pages` | `[{title?, blocks: [...]}]`. Content flows down the page and continues on a new `(cont.)` page when a block does not fit. |
| `sources_page` | Append the generated Sources page (default `true`). |

### Sources

`{id, title, publisher?, url?, date?, kind?, note?, data?, format?, delimiter?}`

- `kind` is how much to trust the number: `observed`, `survey`, `forecast` or
  `other`. It is shown as a coloured chip on the Sources page.
- `data` is a CSV/TSV/JSON file relative to the spec (no absolute paths, no
  `..`). Charts and tables read it with `"source": "<id>"`.

Citations are numbered `[1]`, `[2]` in order of first use. Each data block also
prints a `Source: publisher, date (kind)` caption, and the final page lists every
cited source with its URL.

### Blocks

| `type` | Fields |
| --- | --- |
| `heading` | `text`, `cite[]` |
| `text` | `text`, `cite[]`, `size` |
| `callout` | `text`, `cite[]` |
| `stats` | `items: [{value, label, source?}]` (up to 4 per row) |
| `bar`, `column`, `segmented` | `source` (data) or `values: [{label, value}]`, `label`, `value` (column names), `title`, `cite[]`, `prefix`, `suffix`, `decimals`, `sort` (`desc`/`asc`/`none`), `limit` |
| `table` | `source`, `columns[]`, `title`, `limit`, `cite[]` |
| `spacer` | `height` |

Numbers in CSV cells may carry `$`, `,` and `%` (`$1,234.5`, `12%`).
Charts need nonnegative values; use a table for signed data.

## Checks and limits

`check` (and `build --strict`) warn about: stats or charts with no source, and
sources that are never cited. Errors name the block: unknown source id, missing
column, non-numeric cell (with the row number), data path escaping the spec
directory, too many points (60 per chart), blocks (2000), pages (200).

`build` refuses to replace an existing file without `--overwrite`; replacing goes
through the normal history, so `pentool undo` restores the old report.
`--dry-run` plans and reports without writing.
