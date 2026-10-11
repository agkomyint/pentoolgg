"""Independently evaluate authored RGB/CMYK ICC probes with Pillow/LittleCMS.

Development-only: install Pillow, then run from the repository root. Pentool's
binary and tests do not depend on Pillow or LittleCMS. This checks numerical
probes through LAB8; it does not certify press profiles or PDF/X conformance.
The report is transient and records exact profile hashes and CMM version.
"""
import hashlib
import json
import math
from pathlib import Path

from PIL import Image, ImageCms


def lab_f(value):
    delta = 6 / 29
    return value ** (1 / 3) if value > delta ** 3 else value / (3 * delta ** 2) + 4 / 29


def main():
    root = Path(__file__).resolve().parent.parent
    fixtures = root / "docs" / "fixtures"
    manifest = json.loads((fixtures / "print-conformance.json").read_bytes())
    reports = []
    for fixture in manifest["fixtures"]:
        if not (fixture["file"].startswith("print-rgb-linear-")
                or fixture["file"] in ("print-cmyk-lut16-v2.icc", "print-cmyk-lut16-v4.icc", "print-cmyk-mab-v4.icc")):
            continue
        profile_path = fixtures / fixture["file"]
        digest = hashlib.sha256(profile_path.read_bytes()).hexdigest()
        assert digest == fixture["sha256"], f"changed fixture: {profile_path}"
        source = ImageCms.getOpenProfile(str(profile_path))
        destination = ImageCms.createProfile("LAB", 5000)
        # Endpoints and primaries are exactly representable in RGB8/CMYK8. The
        # fractional matrix vector is covered by the f64 Rust test instead.
        vectors = [entry for entry in fixture["vectors"]
                   if all(v in (0, 1) for v in entry["device"])]
        samples = [tuple(int(v * 255) for v in entry["device"]) for entry in vectors]
        assert all(v in (0, 1) for entry in vectors for v in entry["device"])
        mode = "CMYK" if fixture.get("executor") == "lut" else "RGB"
        image = Image.new(mode, (len(samples), 1))
        image.putdata(samples)
        converted = ImageCms.profileToProfile(
            image, source, destination, renderingIntent=1, outputMode="LAB"
        )
        errors = []
        white = [0xF6D6 / 65536, 1.0, 0xD32D / 65536]
        for i, vector in enumerate(vectors):
            pixel = converted.getpixel((i, 0))
            x, y, z = [lab_f(v / w) for v, w in zip(vector["xyz"], white)]
            expected = [116 * y - 16, 500 * (x - y), 200 * (y - z)]
            actual = [pixel[0] * 100 / 255, pixel[1] - 128, pixel[2] - 128]
            error = math.sqrt(sum((a - b) ** 2 for a, b in zip(expected, actual)))
            assert error <= 1.5, (fixture["file"], pixel, expected, error)
            errors.append(error)
        reports.append({"file": fixture["file"], "sha256": digest,
                        "intent": "relative-colorimetric", "samples": len(vectors),
                        "lab8_delta_e76_max": max(errors), "tolerance": 1.5})
    assert len(reports) == 5, "expected v2/v4 RGB, v2/v4 legacy and modern CMYK probes"
    report = {"reference_engine": "LittleCMS", "version": ImageCms.core.littlecms_version,
              "note": "RGB/CMYK numerical probes only; not press-profile or PDF/X conformance.",
              "profiles": reports}
    output = root / "target" / "print-probe-lcms.json"
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
