# Pentool documentation

[Back to Pentool](../README.md)

New here? Start with the [installation and quick start](../README.md#get-started).
The guides below contain the longer examples previously included in the project README.

## Guides

| Topic | Command examples | Detailed reference |
| --- | --- | --- |
| Drawing, text, fonts, and pages | [Drawing guide](guide-drawing.md) | [Scene authoring](authoring-v0.6.2.md) |
| Object discovery, safe editing, previews, and HTTP rendering | [Agent and CLI guide](guide-agents.md) | [Protocol](PROTOCOL.md) |
| Images, DNG photography, brushes, and paint plans | [Image and painting guide](guide-images-and-paint.md) | [Images](image-workflow.md) · [Photography](photography-v1.md) · [Raster painting](raster-paint-v1.md) |
| Optional AI image generation and editing | [AI guide](ai.md) | [v0.25.0 release](release-v0.25.0.md) |
| Page layout, data merge, books, and InDesign interchange | [Publishing guide](guide-publishing.md) | [Layout](layout-v1.md) · [IDML mapping](idml-mapping-v1.md) |
| ICC profiles, color roles, separations, and print exports | [Color and print guide](guide-color-and-print.md) | [Color management](professional-color-v1.md) |
| 3D models, UVs, product mockups, and packaging previews | [Mockup guide](guide-mockups.md) | [3D and product forms](scene3d-v2.md) |
| Libraries, templates, extensions, comments, and collaboration | [Libraries and collaboration guide](guide-libraries-and-collaboration.md) | [Design systems](design-systems-v1.md) · [Extensions](extensions-v1.md) · [Collaboration](collaboration-v1.md) · [Comments](collaboration-comments.md) |

For exact flags and defaults, run `pentool <command> --help`. Many advanced
features require an explicit document migration; consult the feature reference
before changing a document's version. Read [known limits](known-limits.md) for
additional constraints and record shapes.

## Installation options

The [standard installers](../README.md#1-install) download the latest release.
Set `PENTOOL_VERSION` to select a version, or `PENTOOL_INSTALL_DIR` to choose a folder.

**macOS / Linux — install a specific version:**

```sh
curl -LsSf https://pentool.space/install.sh | PENTOOL_VERSION=v0.25.0 sh
```

**Windows PowerShell — install a specific version:**

```powershell
$env:PENTOOL_VERSION = 'v0.25.0'
irm https://pentool.space/install.ps1 | iex
```

Alternatively, download an archive for your platform from
[GitHub Releases](https://github.com/agkomyint/pentoolgg/releases), verify its
checksum against `SHA256SUMS.txt`, and add the extracted folder to your `PATH`.

## File formats and protocols

`.pen` documents are versioned JSON. Opening a document does not automatically
upgrade it. Different feature sets use different versions:

- [Legacy documents (v1–v3)](pen-format.md)
- [Ordered vector scene graph (v4)](pen-format-v4.md) and [schema](pen-format-v4.schema.json)
- Later schemas: [v5](pen-format-v5.schema.json), [v6](pen-format-v6.schema.json), [v7](pen-format-v7.schema.json), [v8](pen-format-v8.schema.json), [v9](pen-format-v9.schema.json), [v10](pen-format-v10.schema.json), [v11](pen-format-v11.schema.json), [v12](pen-format-v12.md)
- [Asset and package protocol](PROTOCOL.md)
- [Agent painting protocol](agent-painting-protocol-v1.md)
- [Design interchange](design-interchange-v1.md)

## Development and project history

- [Contributor and agent guide](../AGENTS.md)
- [Studio source, build, and architecture](studio.md)
- [Performance](performance.md)
- [Release history](https://github.com/agkomyint/pentoolgg/releases)
- [Roadmaps](roadmap/) — planning and historical acceptance criteria; check released behavior and feature references for current support.
