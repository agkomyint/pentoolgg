# Layout and publishing

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [InDesign interchange](#indesign-interchange)
- [Data merge and adaptive layout](#data-merge-and-adaptive-layout)
- [Books](#books)

## InDesign interchange

Pentool reads and writes InDesign Markup (IDML), Adobe's published XML
interchange format. Native `.indd`/`.indb` files are refused with instructions
to export IDML from InDesign.

```sh
pentool idml-import mag.idml mag.pen --report fidelity.json
pentool idml-export mag.pen mag.idml --report export.json
pentool idml-export mag.pen mag.idml --allow-approximation --overwrite
```

The import report lists every `supported`, `approximated`, `carried` and
`unsupported` element; unknown data is kept under `extension.idml` and written
back on export. Export is deterministic and fails (writing nothing) when a
feature has no IDML equivalent unless `--allow-approximation` is given. The
supported subset and known gaps are in `docs/idml-mapping-v1.md`.

## Data merge and adaptive layout

```sh
pentool layout-merge card.pen --data people.csv --out cards.pen --image-field photo=pic
pentool layout-merge labels.pen --data a.csv --out out.pen --records-per-page 4 --grid 2x2 --cell 200x100
pentool layout-adapt poster.pen --page p1 --width 600 --height 800 --rule scale --uniform
pentool layout-alt poster.pen create p1 p1-tablet --width 768 --height 1024
pentool layout-alt poster.pen sync
pentool layout-preflight book.pen --bleed 9
pentool layout-export book.pen book.pdf        # tagged PDF, real text, bookmarks, links
pentool layout-export book.pen book.epub       # EPUB 3 reflowable; add --fixed for pre-paginated
```

Merge fills `{{field}}` placeholders and image frames from CSV, TSV or JSON
without changing the template and reports estimated overflow per record.
Alternate layouts share linked stories; `sync` never overwrites local edits
unless `--force`. See `docs/layout-v1.md` section 11.1.

## Books

A `.penbook` is readable JSON that lists member `.pen` documents in order.
Operations plan across every member first and write all or none; an
interrupted run is rolled back by the next command.

```sh
pentool book guide.penbook init --name Guide --member 01-intro.pen --member 02-body.pen
pentool book guide.penbook status
pentool book guide.penbook renumber --dry-run
pentool book guide.penbook sync --styles --swatches
pentool book guide.penbook toc --style h1 --style h2
pentool book guide.penbook index --rule contiguous
pentool book guide.penbook preflight --print
pentool book guide.penbook package dist/guide
pentool book guide.penbook export guide.pdf --ppi 300
```

Members must stay inside the book directory and be version 9 documents for
layout operations. `export` writes one flattened PDF/X-4 draft with no
conformance claim; EPUB export arrives with the publishing exports.
