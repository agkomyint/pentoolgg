"""Generate tiny authored ICC probes; never a printing-condition profile.

Run from the repository root. Only the explicitly named print probe files and
their manifest are written. No third-party profiles or licensing data are used.
"""
import hashlib
import json
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "docs" / "fixtures"


def u32(value):
    return struct.pack(">I", value)


def fixed(value):
    return struct.pack(">i", round(value * 65536))


def xyz(values):
    return b"XYZ " + bytes(4) + b"".join(fixed(v) for v in values)


def mluc(text):
    encoded = text.encode("utf-16-be")
    return b"mluc" + bytes(4) + u32(1) + u32(12) + b"enUS" + u32(len(encoded)) + u32(28) + encoded


def profile(major):
    columns = [(0.43604, 0.22249, 0.01392),
               (0.38512, 0.71690, 0.09707),
               (0.14304, 0.06061, 0.71391)]
    description = "Pentool linear RGB numerical probe; not a press condition"
    # v2 has textDescriptionType and textType; v4 has mluc.
    ascii_desc = description.encode("ascii") + b"\0"
    desc = (b"desc" + bytes(4) + u32(len(ascii_desc)) + ascii_desc
            + u32(0) + u32(0) + bytes(2) + bytes(1) + bytes(67))
    tags = [(b"desc", mluc(description) if major == 4 else desc),
            (b"cprt", mluc("Authored by Pentool; CC0-1.0") if major == 4
             else b"text" + bytes(4) + b"Authored by Pentool; CC0-1.0\0"),
            (b"wtpt", xyz([0xF6D6 / 65536, 1, 0xD32D / 65536]))]
    tags += [(name, xyz(values)) for name, values in zip([b"rXYZ", b"gXYZ", b"bXYZ"], columns)]
    tags += [(name, b"curv" + bytes(4) + u32(0)) for name in [b"rTRC", b"gTRC", b"bTRC"]]
    return assemble(tags, major, b"mntrRGB XYZ "), columns


def assemble(tags, major, spaces):
    start = 132 + 12 * len(tags)
    table, data, shared = bytearray(), bytearray(), {}
    for name, body in tags:
        if body not in shared:
            data += bytes((-len(data)) % 4)
            shared[body] = start + len(data)
            data += body
        table += name + u32(shared[body]) + u32(len(body))
    data += bytes((-len(data)) % 4)
    header = bytearray(128)
    header[:4] = u32(start + len(data))
    header[8:12] = bytes([major, 0x40 if major == 4 else 0x10, 0, 0])
    header[12:24] = spaces
    header[24:36] = struct.pack(">6H", 2026, 1, 1, 0, 0, 0)
    header[36:40] = b"acsp"
    header[64:68] = u32(1)
    header[68:80] = b"".join(u32(v) for v in [0xF6D6, 65536, 0xD32D])
    return bytes(header + u32(len(tags)) + table + data)


def lab_xyz(lab):
    y = (lab[0] + 16) / 116
    f = [y + lab[1] / 500, y, y - lab[2] / 200]
    return [w * (v**3 if v > 6/29 else 3*(6/29)**2*(v-4/29))
            for v, w in zip(f, [0xF6D6/65536, 1, 0xD32D/65536])]


def cmyk_lab(v, black=0):
    c, m, y, k = v
    return [black+(100-black)*(1-(c+m+y+k)/4), 64*(m-c), 64*(y-(c+m)/2)]


def cmyk_probe(major=4):
    body = bytearray(b"mft2" + bytes(4) + bytes([4, 3, 2, 0]))
    body += b"".join(fixed(1 if i % 4 == 0 else 0) for i in range(9))
    body += struct.pack(">HH", 2, 2)
    body += struct.pack(">HH", 0, 65535) * 4
    for index in range(16):
        v = [(index >> (3-axis)) & 1 for axis in range(4)]
        l, a, b = cmyk_lab(v, 10 if major == 2 else 0)
        body += struct.pack(">HHH", round(l*65280/100), round((a+128)*256), round((b+128)*256))
    body += struct.pack(">HH", 0, 65535) * 3
    tags = [(b"desc", mluc("Pentool CMYK numerical probe; not a press condition")),
            (b"cprt", mluc("Authored by Pentool; CC0-1.0")),
            (b"wtpt", xyz([0xF6D6/65536, 1, 0xD32D/65536])),
            (b"A2B0", bytes(body))]
    if major == 2:
        description = b"Pentool legacy CMYK probe; black L=10; not a press condition\0"
        tags[0] = (b"desc", b"desc" + bytes(4) + u32(len(description)) + description + bytes(8+70))
        tags[1] = (b"cprt", b"text" + bytes(4) + b"Authored by Pentool; CC0-1.0\0")
    return assemble(tags, major, b"scnrCMYKLab ")


def modern_cmyk_probe():
    identity = b"curv" + bytes(4) + u32(0)
    body = bytearray(b"mAB " + bytes(4) + bytes([4, 3, 0, 0]) + bytes(20))
    body[12:16] = u32(len(body))
    body += identity * 3  # B curves
    body[24:28] = u32(len(body))
    body += bytes([2, 2, 2, 2]) + bytes(12) + bytes([2, 0, 0, 0])
    for index in range(16):
        l, a, b = cmyk_lab([(index >> (3-axis)) & 1 for axis in range(4)])
        body += struct.pack(">HHH", round(l*65535/100), round((a+128)*65535/255), round((b+128)*65535/255))
    body[28:32] = u32(len(body))
    body += identity * 4  # A curves
    tags = [(b"desc", mluc("Pentool CMYK mAB numerical probe; not a press condition")),
            (b"cprt", mluc("Authored by Pentool; CC0-1.0")),
            (b"wtpt", xyz([0xF6D6/65536, 1, 0xD32D/65536])),
            (b"A2B0", bytes(body))]
    return assemble(tags, 4, b"scnrCMYKLab ")


def main():
    fixtures = []
    for major in [2, 4]:
        body, columns = profile(major)
        name = f"print-rgb-linear-v{major}.icc"
        (ROOT / name).write_bytes(body)
        quantized = [[round(v * 65536) / 65536 for v in column] for column in columns]
        fixtures.append({"file": name, "sha256": hashlib.sha256(body).hexdigest(),
                         "expect": "ok", "license": "CC0-1.0", "item": 2,
                         "description": "Authored RGB matrix/identity-TRC probe with shared curve bytes",
                         "vectors": [{"device": sample,
                                      "xyz": [sum(quantized[col][row] * sample[col] for col in range(3))
                                              for row in range(3)], "tolerance": 1e-12}
                                     for sample in [[0, 0, 0], [1, 1, 1], [1, 0, 0],
                                                    [0, 1, 0], [0, 0, 1], [0.25, 0.5, 0.75]]]})
    for name, body, tolerance, black in [
        ("print-cmyk-lut16-v2.icc", cmyk_probe(2), 1e-12, 10),
        ("print-cmyk-lut16-v4.icc", cmyk_probe(), 1e-12, 0),
        ("print-cmyk-mab-v4.icc", modern_cmyk_probe(), 0.00003, 0),
    ]:
        (ROOT / name).write_bytes(body)
        fixtures.append({"file": name, "sha256": hashlib.sha256(body).hexdigest(),
                     "expect": "ok", "license": "CC0-1.0", "item": 2,
                     "executor": "lut", "tag": "A2B0",
                     "description": "Authored CMYK input LUT probe; all four axes affect Lab; not a press condition",
                     "vectors": [{"device": sample, "xyz": lab_xyz(cmyk_lab(sample, black)), "tolerance": tolerance}
                                 for sample in [[0,0,0,0], [1,1,1,1], [1,0,0,0],
                                                [0,1,0,0], [0,0,1,0], [0,0,0,1], [0.1,0.3,0.7,0.4]]]})
    references = ROOT / "print-press-vectors.json"
    reference = json.loads(references.read_bytes())
    press = (ROOT / reference["file"]).read_bytes()
    assert hashlib.sha256(press).hexdigest() == reference["sha256"], "changed original press profile"
    fixtures.append({"file": reference["file"], "sha256": reference["sha256"],
                     "expect": "ok", "item": 2, "executor": "press",
                     "license": "Idealliance/X-Rite: unaltered sharing; permission required for sale",
                     "source": reference["source"], "reference_file": references.name,
                     "description": "Original real CMYK printing condition; see docs/print-profile-provenance.md"})
    manifest = {"format": "pentool-print-conformance", "version": 1,
                "specification": "docs/professional-color-v1.md",
                "note": "Authored probes plus one unmodified real press profile; references identify CMM and precision.",
                "fixtures": fixtures,
                "documents": [{"file": file, "version": 8,
                               "sha256": hashlib.sha256((ROOT / file).read_bytes()).hexdigest(),
                               "description": description} for file, description in [
                                   ("print-v8.pen", "Explicit legacy-sRGB v8 catalog foundation; no assigned profile"),
                                   ("print-v8-native.pen", "Native sRGB vector paint and physical D50 Lab background"),
                                   ("print-v8-roles.pen", "Explicit working/display/output targets; embedded authored RGB monitor probe; proof remains unconfigured"),
                                   ("print-v8-gradient.pen", "Native linear RGB, physical Lab and authored four-channel CMYK interpolation ramps; not a printing condition")]],
                "previews": [{"document": "print-v8-gradient.pen", "encoding": "rgba8", "width": 64, "height": 48,
                              "sha256": "8300fcd42c767788b69baf96fa5da77bc845e4b41d5f928aa25912dd5be986c9",
                              "description": "Intentional engine-1 preview golden: top linear RGB gray ramp, middle physical Lab gray ramp, bottom authored CMYK neutral ramp. Native channels interpolate before appearance; all pixels are opaque. Not a proof or plate fixture."}],
                "references": [{"file": references.name, "sha256": hashlib.sha256(references.read_bytes()).hexdigest()}]}
    (ROOT / "print-conformance.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
