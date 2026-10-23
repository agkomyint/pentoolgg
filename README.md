# Pentool

![Pentool logo](examples/pentoolgg-logo.png)

**An open-source vector editor for people and AI agents. One binary, one editable file.**

Design in your browser with Pentool Studio, or create and edit artwork from the
command line. Both use the same Rust engine and readable `.pen` JSON documents.
Export to PNG, SVG, PDF, or PSD.

Pentool runs locally on **Windows, Linux, and Apple Silicon macOS**. No Node.js
service, database, or account is required. Core editing and rendering work offline;
optional AI image tools use a provider you connect.

[Download](https://github.com/agkomyint/pentoolgg/releases/latest) ·
[Documentation](docs/README.md) ·
[What's new in v0.25.0](https://github.com/agkomyint/pentoolgg/releases/tag/v0.25.0) ·
[Report a bug](https://github.com/agkomyint/pentoolgg/issues)

## Get started

### 1. Install

**macOS / Linux**

```sh
curl -LsSf https://pentool.space/install.sh | sh
```

**Windows PowerShell**

```powershell
irm https://pentool.space/install.ps1 | iex
```

The installer verifies the download's SHA-256 checksum and adds Pentool to your
user `PATH`. Restart your terminal after your first installation.

Prefer a manual download? Get an archive from [GitHub Releases](https://github.com/agkomyint/pentoolgg/releases/latest).
For a specific version or install folder, see [installation options](docs/README.md#installation-options).

### 2. Open Studio

```sh
pentool serve
```

Open **http://127.0.0.1:4711** in your browser to start drawing.

To edit a file on disk:

```sh
pentool serve my-design.pen
```

The file must already exist. Create one in Studio or use `pentool new` below.

## Create from the command line

Make a document, add editable text, export it, and open it in Studio:

```sh
pentool new hello.pen --width 1080 --height 1080
pentool text hello.pen put title --layer layer-1 --content "Hello, Pentool" --x 80 --y 220 --size 96
pentool export hello.pen hello.png
pentool serve hello.pen
```

Use `pentool --help` to browse commands and `pentool <command> --help` for details.
For automation, start with [object discovery, safe edits, and previews](docs/guide-agents.md).

## What can I make?

- **Vector artwork:** paths, shapes, editable text, layers, groups, and multiple pages.
- **Image compositions:** placed images, non-destructive adjustments, masks, and raster painting.
- **Reusable designs:** assets, components, design tokens, brand kits, and templates.
- **Larger projects:** page layouts, photography catalogs, 3D product mockups, and packaging previews.
- **3D scenes:** see [docs/scene3d-v1.md](docs/scene3d-v1.md) and `pentool scene3d --help`.
- **Shared work:** browser editing, comments, review states, and collaboration tools.

Advanced workflows have their own format requirements and limits. Find the relevant
[guide](docs/README.md#guides) before starting.

![Vector eye artwork made with Pentool](examples/eye.png)

After cloning this repository, open the editable example with
`pentool serve examples/eye.pen`. More artwork and scripts live in [examples/](examples/).

## New in v0.27.0: select, cut out and crop

Select a subject with a box, polygon or an improved magic wand, snap it to image
edges and cut it into a new transparent layer, from the CLI or Studio:
`pentool raster DOC cutout PHOTO --id car --box 120 140 400 190 --refine 25`.
See [Selecting and cutting out](docs/selecting-and-cutting-out.md).

## Studio AI (v0.25.0)

Generate images, preview candidates, and place or replace an image from Studio's
**AI** panel. Gemini also supports image variations and background removal.
Accepting a candidate creates an undoable edit.

AI is optional and uses your own provider account. Review and confirm model calls
before sending them. See [AI setup and usage](docs/ai.md), or read the
[release notes](https://github.com/agkomyint/pentoolgg/releases/tag/v0.25.0)
for the rendering, export, and CLI fixes included in this release.

## Learn more

| I want to… | Start here |
| --- | --- |
| Learn drawing, text, and page commands | [Drawing guide](docs/guide-drawing.md) |
| Automate edits or call the rendering API | [Agent and CLI guide](docs/guide-agents.md) |
| Work with images, photographs, or brushes | [Image and painting guide](docs/guide-images-and-paint.md) |
| Explore layout, print, mockups, or collaboration | [All documentation](docs/README.md) |
| Understand the file format | [Document formats](docs/README.md#file-formats-and-protocols) |
| Build or contribute | [Contributor guide](AGENTS.md) · [Studio development](docs/studio.md) |

Build the native binary from a repository checkout with `cargo install --path .`.
The browser UI is embedded; Node.js and Bun are only needed when rebuilding Studio.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
Bundled fonts use the [SIL Open Font License](assets/fonts/OFL.txt);
see [font attribution](assets/fonts/README.md).
