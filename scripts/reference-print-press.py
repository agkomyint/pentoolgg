"""Regenerate independent CMYK press vectors with development-only LittleCMS.

The original profile is never modified. See docs/print-profile-provenance.md for
its provider and license. This script performs no network access.
"""
import argparse
import ctypes
import hashlib
import json
from pathlib import Path

from PIL import Image, ImageCms

ROOT = Path(__file__).resolve().parent.parent / "docs" / "fixtures"
DIGEST = "2b186e6dadc38883138ccd30203b2870fb71db6451d3a5d24f9590cfabd26689"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--lcms-library", required=True, help="Development LittleCMS 2.18 shared library")
    args = parser.parse_args()
    cmm = ctypes.CDLL(args.lcms_library)
    cmm.cmsOpenProfileFromMem.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    cmm.cmsOpenProfileFromMem.restype = ctypes.c_void_p
    cmm.cmsReadTag.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
    cmm.cmsReadTag.restype = ctypes.c_void_p
    cmm.cmsPipelineEvalFloat.argtypes = [ctypes.POINTER(ctypes.c_float), ctypes.POINTER(ctypes.c_float), ctypes.c_void_p]
    cmm.cmsCloseProfile.argtypes = [ctypes.c_void_p]
    cmm.cmsGetEncodedCMMversion.restype = ctypes.c_uint32
    assert cmm.cmsGetEncodedCMMversion() == 2180, "expected LittleCMS 2.18"
    path = ROOT / "print-CGATS21_CRPC7.icc"
    assert hashlib.sha256(path.read_bytes()).hexdigest() == DIGEST, "changed original press profile"
    press = ImageCms.getOpenProfile(str(path))
    lab = ImageCms.createProfile("LAB", 5000)
    cmyk_samples = [(0,0,0,0),(255,255,255,255),(255,0,0,0),(0,255,0,0),
                    (0,0,255,0),(0,0,0,255),(64,128,192,96),(23,47,83,127)]
    lab_samples = [(255,128,128),(0,128,128),(128,128,128),(180,160,128),
                   (80,100,150),(255,0,255)]
    image = Image.new("CMYK", (len(cmyk_samples),1))
    image.putdata(cmyk_samples)
    reverse_image = Image.new("LAB", (len(lab_samples),1))
    reverse_image.putdata(lab_samples)
    vectors, reverse = [], []
    rgb_path = ROOT / "print-rgb-linear-v4.icc"
    rgb_profile = ImageCms.getOpenProfile(str(rgb_path))
    rgb_samples = [(0,0,0),(255,255,255),(255,0,0),(0,255,0),
                   (0,0,255),(64,128,192),(12,24,48),(100,120,80)]
    rgb_image = Image.new("RGB", (len(rgb_samples),1))
    rgb_image.putdata(rgb_samples)
    linked_rgb, linked_cmyk = [], []
    legacy_path = ROOT / "print-cmyk-lut16-v2.icc"
    legacy_profile = ImageCms.getOpenProfile(str(legacy_path))
    legacy_vectors = []
    legacy_output = ImageCms.profileToProfile(image,legacy_profile,press,renderingIntent=0,outputMode="CMYK",flags=ImageCms.Flags.NOOPTIMIZE)
    for i,sample in enumerate(cmyk_samples):
        legacy_vectors.append({"intent":0,"input":[v/255 for v in sample],
                               "output":[v/255 for v in legacy_output.getpixel((i,0))],
                               "tolerance_channel":0.01})
    for intent in range(4):
        output = ImageCms.profileToProfile(rgb_image,rgb_profile,press,renderingIntent=intent,outputMode="CMYK",flags=ImageCms.Flags.NOOPTIMIZE)
        for i,sample in enumerate(rgb_samples):
            linked_rgb.append({"intent":intent,"input":[v/255 for v in sample],
                               "output":[v/255 for v in output.getpixel((i,0))],
                               "tolerance_channel":0.01})
        output = ImageCms.profileToProfile(image,press,rgb_profile,renderingIntent=intent,outputMode="RGB",flags=ImageCms.Flags.NOOPTIMIZE)
        for i,sample in enumerate(cmyk_samples):
            linked_cmyk.append({"intent":intent,"input":[v/255 for v in sample],
                                "output":[v/255 for v in output.getpixel((i,0))],
                                "tolerance_channel":0.01})
        output = ImageCms.profileToProfile(image,press,lab,renderingIntent=intent,outputMode="LAB",flags=ImageCms.Flags.NOOPTIMIZE)
        for i,sample in enumerate(cmyk_samples):
            pixel = output.getpixel((i,0))
            vectors.append({"intent":intent,"device":[v/255 for v in sample],
                            "lab":[pixel[0]*100/255,pixel[1]-128,pixel[2]-128],
                            "tolerance_delta_e76":1.5})
        output = ImageCms.profileToProfile(reverse_image,lab,press,renderingIntent=intent,outputMode="CMYK",flags=ImageCms.Flags.NOOPTIMIZE)
        for i,sample in enumerate(lab_samples):
            pixel = output.getpixel((i,0))
            reverse.append({"intent":intent,"lab":[sample[0]*100/255,sample[1]-128,sample[2]-128],
                            "device":[v/255 for v in pixel],"tolerance_channel":0.01})
    original = ctypes.create_string_buffer(path.read_bytes())
    handle = cmm.cmsOpenProfileFromMem(original, len(path.read_bytes()))
    assert handle, "reference CMM refused original profile"
    pipeline_vectors = []
    try:
        for intent in range(3):
            for direction, samples in [("A2B", cmyk_samples), ("B2A", lab_samples)]:
                tag = direction + str(intent)
                pipeline = cmm.cmsReadTag(handle, int.from_bytes(tag.encode("ascii"), "big"))
                assert pipeline, tag
                for sample in samples:
                    values = [v / 255 for v in sample]
                    incoming = (ctypes.c_float * len(values))(*values)
                    outgoing = (ctypes.c_float * 4)()
                    cmm.cmsPipelineEvalFloat(incoming, outgoing, pipeline)
                    pipeline_vectors.append({"tag": tag, "input": values,
                                             "output": list(outgoing)[:3 if direction == "A2B" else 4],
                                             "tolerance_channel": 0.0002})
    finally:
        cmm.cmsCloseProfile(handle)
    report = {"format":"pentool-print-reference-vectors","version":1,
              "file":path.name,"sha256":DIGEST,
              "source":"https://registry.color.org/profile-registry/CGATS21_CRPC7",
              "download":"https://registry.color.org/profile-registry/profiles/CGATS21_CRPC7.icc",
              "engine":"LittleCMS","engine_version":ImageCms.core.littlecms_version,
              "precision":"CMYK8/LAB8, no requested BPC flag, implicit v4 linking mapping, NOOPTIMIZE",
              "pipeline_vectors":pipeline_vectors,
              "legacy_profile":{"file":legacy_path.name,"sha256":hashlib.sha256(legacy_path.read_bytes()).hexdigest()},
              "legacy_to_press_vectors":legacy_vectors,
              "rgb_profile":{"file":rgb_path.name,"sha256":hashlib.sha256(rgb_path.read_bytes()).hexdigest()},
              "rgb_to_cmyk_vectors":linked_rgb,"cmyk_to_rgb_vectors":linked_cmyk,
              "vectors":vectors,"reverse_vectors":reverse}
    (ROOT / "print-press-vectors.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8",newline="\n")
    print(f"{len(vectors)} forward and {len(reverse)} reverse press vectors; LittleCMS {ImageCms.core.littlecms_version}")


if __name__ == "__main__":
    main()
