"""Regression smoke for spot-only composite previews (#397)."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
root = Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix="pentool-spot-smoke-") as tmp:
    doc = Path(tmp) / "spot.pen"
    doc.write_bytes((root / "docs/fixtures/print-v8.pen").read_bytes())
    subprocess.run([binary, "color", "profile", "import", str(doc), str(root / "docs/fixtures/print-CGATS21_CRPC7.icc"), "--name", "press", "--redistribution", "allowed"], check=True, stdout=subprocess.DEVNULL)
    raw = json.loads(doc.read_bytes())
    raw["pages"][0]["canvas"] = {"width": 8, "height": 2, "background": "#00000000"}
    alternate = {"space": "srgb", "channels": [0.95, 0.22, 0.03]}
    raw["color_management"]["roles"] = {"output": {"destination": {"space": "icc-cmyk", "profile": "press"}, "intent": "relative-colorimetric"}}
    raw["color_management"]["inks"] = {"schema": 1, "inks": {"gold": {"id": "gold", "name": "Ink Gold", "alternate": alternate, "provenance": {"source": "release smoke"}, "redistribution": "allowed"}}}
    raw["pages"][0]["layers"][0]["nodes"] = [{"kind": "rect", "id": "spot", "x": 2, "y": 0, "width": 2, "height": 2, "style": {"fill": {"spot": {"ink": "gold", "tint": 1}, "fallback": alternate}}}]
    doc.write_text(json.dumps(raw), encoding="utf-8")
    before = doc.read_bytes()
    for explicit in (False, True):
        out = Path(tmp) / ("explicit" if explicit else "configured")
        args = [binary, "color", "separate", str(doc), "--out", str(out), "--composite"]
        if explicit:
            args += ["--to", json.dumps({"space": "icc-cmyk", "profile": "press"}), "--intent", "relative-colorimetric"]
        result = subprocess.run(args, capture_output=True, text=True)
        assert result.returncode == 0, result.stderr
        report = json.loads(result.stdout)
        assert report["composite_preview"]["file"] == "composite-preview.png"
        assert (out / "composite-preview.png").read_bytes().startswith(b"\x89PNG")
        assert doc.read_bytes() == before
    print("Spot-only composite smoke passed (configured and explicit CMYK destinations)")
