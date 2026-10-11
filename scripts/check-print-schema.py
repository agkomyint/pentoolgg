"""Validate developing print fixtures and color grammar with offline JSON Schema.

Development-only: requires jsonschema. No dependency is added to Pentool's binary
or Rust checks. All schema references are registered locally; retrieval is refused.
Rust remains responsible for ICC bytes, channel/header agreement, finite physical
conversion and the other semantic constraints described in the color contract.
"""
import hashlib
import importlib.metadata
import json
from pathlib import Path

from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from referencing.jsonschema import DRAFT202012
from referencing.exceptions import NoSuchResource


def refuse_retrieval(uri):
    raise NoSuchResource(ref=uri)


def main():
    root = Path(__file__).resolve().parent.parent
    docs = root / "docs"
    registry = Registry(retrieve=refuse_retrieval)
    schemas = {}
    schema_hashes = {}
    for path in sorted(docs.glob("*.schema.json")):
        data = path.read_bytes()
        schema = json.loads(data)
        schema_hashes[path.name] = hashlib.sha256(data).hexdigest()
        resource = Resource.from_contents(schema, default_specification=DRAFT202012)
        schemas[path.name] = schema
        for identifier in (path.as_uri(), schema.get("$id"),
                           "https://pentool.space/schema/" + path.name):
            if identifier:
                registry = registry.with_resource(identifier, resource)
    for name in ("pen-format-v8.schema.json", "color-value-v1.schema.json"):
        Draft202012Validator.check_schema(schemas[name])
    scene = Draft202012Validator(schemas["pen-format-v8.schema.json"], registry=registry)
    color = Draft202012Validator(schemas["color-value-v1.schema.json"], registry=registry)
    destination = Draft202012Validator(
        {"$ref": schemas["color-value-v1.schema.json"]["$id"] + "#/$defs/destination"},
        registry=registry)
    role_sets = Draft202012Validator(
        {"$ref": schemas["pen-format-v8.schema.json"]["$id"] + "#/$defs/colorManagement/properties/roles"},
        registry=registry)
    manifest = json.loads((docs / "fixtures/print-conformance.json").read_bytes())
    documents = []
    for entry in manifest["documents"]:
        data = (docs / "fixtures" / entry["file"]).read_bytes()
        digest = hashlib.sha256(data).hexdigest()
        assert digest == entry["sha256"], f"changed fixture: {entry['file']}"
        scene.validate(json.loads(data))
        documents.append({"file": entry["file"], "sha256": digest})
    valid = [
        {"space": "srgb", "channels": [0, 0.5, 1]},
        {"space": "display-p3:linear", "channels": [0.1, 0.2, 0.3], "alpha": 0.4},
        {"space": "lab-d50", "channels": [-10, 999, -888]},
        {"space": "icc-gray", "profile": "gray", "channels": [0.5]},
        {"space": "icc-rgb", "profile": "profile:rgb", "channels": [0.1, 0.2, 0.3]},
        {"space": "icc-cmyk", "profile": "sha256:" + "0" * 64, "channels": [0.1, 0.2, 0.3, 0.4]},
    ]
    invalid = [
        {"space": "srgb", "channels": [0, 1]},
        {"space": "srgb", "channels": [-0.1, 0, 1]},
        {"space": "srgb", "channels": [0, 0, 1.1]},
        {"space": "srgb", "channels": [0, 0, 1], "profile": "rgb"},
        {"space": "lab-d50", "channels": [0, 0, 0], "alpha": 1.1},
        {"space": "icc-gray", "channels": [0.5]},
        {"space": "icc-cmyk", "profile": "press", "channels": [0, 0, 0]},
        {"space": "icc-rgb", "profile": "../escape", "channels": [0, 0, 0]},
        {"space": "spot", "channels": [0.5]},
        {"space": "srgb", "channels": [0, 0, 0], "unknown": True},
    ]
    for sample in valid:
        color.validate(sample)
    for sample in invalid:
        assert not color.is_valid(sample), f"unexpected schema acceptance: {sample}"
    valid_destinations = [
        {"space": "display-p3:linear"}, {"space": "lab-d50"},
        {"space": "icc-gray", "profile": "gray"},
        {"space": "icc-rgb", "profile": "profile:rgb"},
        {"space": "icc-cmyk", "profile": "sha256:" + "0" * 64},
    ]
    invalid_destinations = [
        {"space": "srgb", "profile": "rgb"},
        {"space": "lab-d50", "profile": "lab"},
        {"space": "icc-rgb"},
        {"space": "icc-gray", "profile": "../escape"},
        {"space": "icc-cmyk", "profile": "press", "channels": [0, 0, 0, 0]},
        {"space": "spot"},
    ]
    for value in valid_destinations:
        destination.validate(value)
    for value in invalid_destinations:
        assert not destination.is_valid(value), f"unexpected destination acceptance: {value}"
    def setting(space, profile=None):
        target = {"space": space}
        if profile is not None:
            target["profile"] = profile
        return {"destination": target, "intent": "relative-colorimetric"}
    valid_roles = [{}, {"working": setting("lab-d50"), "proof": setting("icc-cmyk", "press"),
                       "display": setting("display-p3"), "output": setting("icc-gray", "gray")}]
    invalid_roles = [None, {"unknown": setting("srgb")}, {"working": {"destination": {"space": "srgb"}}},
                     {"display": setting("lab-d50")}, {"display": setting("icc-cmyk", "press")},
                     {"proof": setting("srgb")}, {"output": setting("lab-d50")},
                     {"working": {**setting("srgb"), "ignored": True}},
                     {"working": {**setting("srgb"), "intent": "unknown"}},
                     {"working": setting("icc-rgb")}]
    for value in valid_roles:
        role_sets.validate(value)
    for value in invalid_roles:
        assert not role_sets.is_valid(value), f"unexpected role grammar acceptance: {value}"
    gradient = Draft202012Validator(
        {"$ref": schemas["pen-format-v8.schema.json"]["$id"] + "#/$defs/fillDefinition"},
        registry=registry)
    stops = [{"position": 0, "color": "#000"}, {"position": 1, "color": "#fff"}]
    base = {"kind": "linear", "stops": stops}
    valid_gradients = [base, {**base, "space": "linear"},
                       {**base, "space": {"space": "lab-d50"}, "intent": "relative-colorimetric"},
                       {**base, "kind": "conic", "space": {"space": "icc-cmyk", "profile": "press"}, "intent": "perceptual"},
                       {"ref": "ramp", "fallback": base}]
    invalid_gradients = [{**base, "space": {"space": "srgb"}},
                         {**base, "space": {"space": "icc-rgb"}, "intent": "perceptual"},
                         {**base, "space": {"space": "srgb", "channels": [0,0,0]}, "intent": "perceptual"},
                         {**base, "space": {"space": "srgb"}, "intent": "unknown"},
                         {**base, "space": "srgb", "intent": "perceptual"},
                         {**base, "space": []},
                         {**base, "kind": "solid", "space": {"space": "srgb"}, "intent": "perceptual"}]
    for value in valid_gradients:
        gradient.validate(value)
    for value in invalid_gradients:
        assert not gradient.is_valid(value), f"unexpected gradient grammar acceptance: {value}"
    report = {"ok": True, "validator": "jsonschema", "validator_version": importlib.metadata.version("jsonschema"),
              "offline": True, "documents": documents, "valid_samples": len(valid),
              "invalid_samples": len(invalid), "valid_destinations": len(valid_destinations),
              "invalid_destinations": len(invalid_destinations), "valid_role_sets": len(valid_roles),
              "invalid_role_sets": len(invalid_roles), "registered_schemas": schema_hashes,
              "valid_gradients": len(valid_gradients), "invalid_gradients": len(invalid_gradients),
              "scope": "developing-v8-json-grammar"}
    target = root / "target/print-schema.json"
    target.parent.mkdir(exist_ok=True)
    target.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
