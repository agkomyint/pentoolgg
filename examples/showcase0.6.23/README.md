# Pentool 0.6.23 showcase

`showcase.pen` is an editable v4 dashboard composed with the Pentool CLI. The PNG
and SVG files are rendered previews of the same source document.

Rebuild the showcase from the repository root:

```powershell
target\debug\pentool.exe new examples\showcase0.6.23\showcase.pen --width 1200 --height 800
target\debug\pentool.exe batch examples\showcase0.6.23\showcase.pen examples\showcase0.6.23\showcase-ops.json
target\debug\pentool.exe export examples\showcase0.6.23\showcase.pen examples\showcase0.6.23\showcase.png
target\debug\pentool.exe export examples\showcase0.6.23\showcase.pen examples\showcase0.6.23\showcase.svg
```
