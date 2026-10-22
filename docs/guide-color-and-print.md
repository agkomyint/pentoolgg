# Color management and print

[Documentation](README.md) · [Project home](../README.md)

Command examples assume the named documents, layers, and input assets exist.
Use `pentool <command> --help` for the full argument reference. Format upgrades
are explicit; see each linked feature contract for supported versions and limits.

## On this page

- [ICC profile tools](#icc-profile-tools)

## ICC profile tools

Inspect a local ICC profile and validate a requested transform without changing
a document:

```sh
pentool color profile inspect docs/fixtures/print-CGATS21_CRPC7.icc --limit 10 --json
pentool color profile validate docs/fixtures/print-CGATS21_CRPC7.icc --direction pcs-to-device --intent relative-colorimetric --json
```

Inspection returns the original SHA-256, bounded description/copyright metadata,
paginated tag headers, and supported intent/direction paths with diagnostics.
`--offset` and `--limit` apply to tags; limits are 1–256. ICC version is reported
as `icc_version`; the report's `version` identifies its JSON contract.
Validation checks only the requested profile path and refuses a missing or
unsupported path. It does not certify ICC or PDF/X conformance. These commands
are offline and read only.

Explicitly migrate an existing document to v8 before importing profiles:

```sh
pentool migrate campaign.pen --target 8 --dry-run
pentool migrate campaign.pen --target 8
pentool migrate campaign.pen --target 11 --dry-run
pentool color profile import campaign.pen docs/fixtures/print-rgb-linear-v4.icc --name rgb-probe --redistribution allowed --dry-run
pentool color profile import campaign.pen docs/fixtures/print-rgb-linear-v4.icc --name rgb-probe --redistribution allowed
pentool color profile list campaign.pen --limit 10 --json
pentool color profile show campaign.pen --name rgb-probe --limit 10 --json
pentool color profile embed campaign.pen --name rgb-probe --dry-run
pentool color profile collect campaign.pen --dry-run --json
pentool color profile remove campaign.pen --name rgb-probe --dry-run --json
```

Import stores the original bytes by SHA-256 without assigning a role or changing
appearance. Mutations support `--dry-run` and `--if-revision`. Optional
`--external profiles/press.icc` verifies an existing document-relative copy;
`embed` makes its verified bytes self-contained. Redistribution defaults to
`unknown`; use a declaration supported by the profile's license. Downgrading
refuses retained profiles. See the [v8 catalog contract](../docs/professional-color-v1.md#implemented-catalog-foundation)
for supported operations, limits, and validation.

`collect` embeds every ICC catalog entry in one transaction. Sharing collection
requires declared `allowed` redistribution; `--local-only` permits a private
copy while preserving restricted/unknown declarations. Existing `.penpkg`
packaging requires embedded, redistributable ICC dependencies and verifies its
profile audit table. `remove` deletes an unused profile and all aliases; a paint
reference anywhere in the document rejects removal. External source files stay
in place. These commands support dry runs, revision guards and undo.

Configure document conversion targets without changing stored paint samples:

```sh
pentool color role list campaign.pen --json
pentool color role assign campaign.pen working --to '{"space":"display-p3:linear"}' --dry-run --json
pentool color profile import campaign.pen docs/fixtures/print-CGATS21_CRPC7.icc --name press-coated --redistribution allowed
pentool color role assign campaign.pen output --to '{"space":"icc-cmyk","profile":"press-coated"}' --intent relative-colorimetric --json
pentool color role evaluate campaign.pen output --color '{"space":"srgb","channels":[0.2,0.4,0.7]}' --json
pentool color paints convert campaign.pen --role output --include-shared --dry-run --json
pentool color role clear campaign.pen output --dry-run --json
```

Roles are `working`, `proof`, `display` and `output`. Assignment validates required
ICC paths and stores profile hashes; it supports dry runs, revision guards and
undo. Working/display default to linear sRGB/sRGB. Proof/output require explicit
settings. Role conversion uses its stored intent, so `--role` conflicts with an
explicit `--intent` or other destination flags. Profile removal refuses assigned
dependencies. See the [color management reference](professional-color-v1.md) for role-aware
rendering and production export support and limitations.

Inspect visible paint samples through a verified CMYK output profile:

```sh
pentool inspect ink campaign.pen --profile profile:press-coated --tac-limit 300 --small-text-points 8 --units-per-inch 72 --limit 100 --json
```

The read-only report gives profile-derived C/M/Y/K channels, TAC, black and neutral
classifications, and small-text warnings. Omit `--profile` to use the document's
assigned output role and intent. The thresholds are explicit inputs; `--page`,
`--offset`, and `--limit` select a bounded page of records. Each record describes
a declared visible paint or gradient stop, not rendered area or a separation plate.

Evaluate a native color sample without editing the document:

```sh
pentool color evaluate docs/fixtures/print-v8.pen --color '{"space":"display-p3","channels":[1,0,0],"alpha":0.4}' --to '{"space":"srgb"}' --json
pentool color evaluate campaign.pen --color '{"space":"icc-rgb","profile":"rgb-probe","channels":[0.2,0.4,0.7]}' --to '{"space":"lab-d50"}' --intent relative-colorimetric --json
pentool color separate campaign.pen --out plates   # C,M,Y,K + spot plate PNGs and separations.json
pentool color pdfx campaign.pen campaign-x4.pdf --ppi 300   # flattened CMYK PDF/X-4 draft, unvalidated
pentool color preflight campaign.pen --min-ppi 150   # structured print issues with stable codes
pentool color preflight campaign.pen --max-total-ink 300   # flag pixels above 300% total ink
pentool color pdfx campaign.pen out.pdf --recipe magazine-print.json   # recipe supplies intent/ppi; flags override
pentool color package campaign.pen --out pkg && pentool color package-verify pkg   # PDF/X draft, plates, profiles, manifest
```

The report includes physical D50 XYZ, original and converted channels, alpha,
clipping, verified profile hashes, and selected ICC paths. Named RGB and ICC
device channels use [0,1]; Lab uses physical L*, a*, b*. Alpha defaults to 1 and
is preserved separately. See the [sample grammar](../docs/color-value-v1.schema.json).
V8 vector/background paints, solid fills, gradient stops and effect colors
can also retain these native values. See the small
[native paint fixture](../docs/fixtures/print-v8-native.pen). Ordinary exports
evaluate them into an sRGB8 preview while preserving stored channels. Sample evaluation does not assign or convert objects. See the
[color management reference](professional-color-v1.md) for supported separation
and print-delivery paths and their limits.

Interpolate numeric samples in an explicit channel space, with separate alpha:

```sh
pentool color interpolate docs/fixtures/print-v8.pen --from '{"space":"srgb","channels":[0,0,0]}' --to '{"space":"srgb","channels":[1,1,1]}' --space '{"space":"srgb:linear"}' --position 0.5 --json
```

Endpoints convert once using the selected intent. Same-profile CMYK retains all
four device channels; Lab retains physical values. Reports include converted
endpoints, clipping and profile hashes. This read-only sampler does not change
scene gradients or composite alpha.

V8 fill gradients can also declare a native interpolation space and intent:

```sh
pentool export docs/fixtures/print-v8-gradient.pen native-gradients.png
```

The fixture compares linear RGB, physical Lab and an authored CMYK numerical
probe. Native channels interpolate before preview conversion, with separate
straight alpha. Existing `"space":"srgb"` and `"space":"linear"` gradients
retain their released behavior. See the [native gradient contract](../docs/professional-color-v1.md#implemented-native-fill-gradients)
for JSON, required ICC paths and limits. These exports are RGB previews.

Assign or convert the selected page's stored vector paints with an atomic edit:

```sh
pentool color paints assign campaign.pen --to '{"space":"display-p3"}' --dry-run --json
pentool color paints convert campaign.pen --to '{"space":"lab-d50"}' --dry-run --json
pentool color profile import campaign.pen docs/fixtures/print-CGATS21_CRPC7.icc --name press-coated --redistribution allowed
pentool color paints convert campaign.pen --to '{"space":"icc-cmyk","profile":"press-coated"}' --intent relative-colorimetric --include-shared --json
pentool color paints convert campaign.pen --legacy-srgb --all-pages --include-shared --dry-run --json
```

Assignment keeps channel numbers and alpha; it requires equal channel counts.
Conversion changes channels through the requested transform and reports clipping.
`--legacy-srgb` explicitly quantizes to legacy sRGB8 hex colors. Shared styles and
vector mask snapshots require `--include-shared`; their other page users can be
affected. Locked paints or locked shared-resource users reject the whole edit.
Use global `--page ID` or `--all-pages` for scope. `--offset`/`--limit` paginate the
change report, while the entire selected scope is planned. Remove `--dry-run` to
commit; `--if-revision` and undo are supported. These commands convert stored
paints, including hidden ones; raster pixels and document profile roles are not
part of this operation.
