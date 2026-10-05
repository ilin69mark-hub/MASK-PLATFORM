#!/usr/bin/env python3
"""Validate the example fixtures against the JSON Schemas.

Also asserts that the invariants expressible in JSON Schema actually reject
bad documents. A schema that accepts everything is worse than no schema, so
negative cases are part of the check.

Usage:
    python3 docs/schema/validate_examples.py
"""

import copy
import json
import pathlib
import sys

try:
    from jsonschema import Draft202012Validator
except ImportError:
    sys.exit("jsonschema is required: pip install jsonschema")

ROOT = pathlib.Path(__file__).resolve().parent
FIXTURES = ROOT / "examples"


def load(name):
    return json.loads((ROOT / name).read_text(encoding="utf-8"))


def check_positive(schema, instance, label):
    try:
        errors = sorted(Draft202012Validator(schema).iter_errors(instance),
                        key=lambda e: list(e.path))
    except Exception as exc:  # noqa: BLE001 - broken $ref, malformed schema
        print(f"FAIL {label}: schema could not be applied: {type(exc).__name__}")
        print(f"     {str(exc)[:200]}")
        return False
    if errors:
        print(f"FAIL {label}: {len(errors)} error(s)")
        for err in errors[:5]:
            loc = "/".join(str(p) for p in err.path) or "<root>"
            print(f"     at {loc}: {err.message}")
        return False
    print(f"ok   {label}")
    return True


def check_rejects(schema, instance, label, mutate):
    mutated = copy.deepcopy(instance)
    mutate(mutated)
    try:
        errors = list(Draft202012Validator(schema).iter_errors(mutated))
    except Exception as exc:  # noqa: BLE001
        print(f"FAIL {label}: schema could not be applied: {type(exc).__name__}")
        return False
    if errors:
        print(f"ok   {label} (rejected)")
        return True
    print(f"FAIL {label}: invalid document was accepted")
    return False


def main():
    project_schema = load("mask-project.v1.json")
    manifest_schema = load("manifest.v1.json")
    project = json.loads((FIXTURES / "mask-project.v1.example.json").read_text(encoding="utf-8"))
    manifest = json.loads((FIXTURES / "manifest.v1.example.json").read_text(encoding="utf-8"))

    passed = True
    print("── schemas are themselves valid ──")
    for name, schema in (("mask-project", project_schema), ("manifest", manifest_schema)):
        try:
            Draft202012Validator.check_schema(schema)
            print(f"ok   {name}.v1.json")
        except Exception as exc:  # noqa: BLE001
            print(f"FAIL {name}.v1.json: {exc}")
            passed = False

    print("\n── examples must validate ──")
    passed &= check_positive(project_schema, project, "mask-project example")
    passed &= check_positive(manifest_schema, manifest, "manifest example")

    print("\n── invariants must reject bad documents ──")

    # I-5: PASS cannot coexist with error-severity violations.
    passed &= check_rejects(
        project_schema, project, "I-5 PASS with violations_error > 0",
        lambda d: (d["validation"].update(verdict="PASS", violations_error=3)))
    passed &= check_rejects(
        manifest_schema, manifest, "I-5 manifest PASS with error violations",
        lambda d: (d["validation"].update(verdict="PASS",
                                          violations={"error": 2, "warning": 0, "info": 0})))

    # Layer ceiling: 64 physical layers per MASK_PROJECT.md §8.
    def too_many_layers(d):
        base = d["composition"]["layers"][0]
        layers = []
        for i in range(65):
            layer = copy.deepcopy(base)
            layer["id"] = f"L{i + 1:02d}-aaaa"
            layer["index"] = i + 1
            layers.append(layer)
        d["composition"]["layers"] = layers
    passed &= check_rejects(project_schema, project, "65 layers exceeds limit 64", too_many_layers)

    # Layer index range.
    passed &= check_rejects(
        project_schema, project, "layer index 0 rejected",
        lambda d: d["composition"]["layers"][0].update(index=0))

    # Units must be millimetres.
    passed &= check_rejects(
        project_schema, project, "units other than mm rejected",
        lambda d: d["geometry"].update(units="px"))

    # Fidelity is a fraction.
    passed &= check_rejects(
        project_schema, project, "fidelity > 1 rejected",
        lambda d: d["validation"].update(fidelity=1.4))

    # Role must come from the documented enum.
    passed &= check_rejects(
        project_schema, project, "unknown layer role rejected",
        lambda d: d["composition"]["layers"][0].update(role="gloss"))

    # Relation type must come from the documented enum.
    passed &= check_rejects(
        project_schema, project, "unknown relation type rejected",
        lambda d: d["composition"]["graph"]["edges"][0].update(type="above"))

    # Project id format.
    passed &= check_rejects(
        project_schema, project, "malformed project id rejected",
        lambda d: d.update(id="mask-127"))

    # Non-uniform stretching must not be expressible as a scale policy.
    passed &= check_rejects(
        project_schema, project, "stretch-to-fit policy rejected",
        lambda d: d["geometry"]["design_space"].update(scale_policy="stretch_to_fill"))

    # A passing manifest must carry an explicit physical_proof block.
    passed &= check_rejects(
        manifest_schema, manifest, "manifest without physical_proof rejected",
        lambda d: d["validation"].pop("physical_proof"))

    # Untested limits must be declared, not silently omitted.
    passed &= check_rejects(
        manifest_schema, manifest, "manifest lacking effective_limits rejected",
        lambda d: d["manufacturing"].pop("effective_limits"))

    # sha256 fields must be hex of the right length.
    passed &= check_rejects(
        manifest_schema, manifest, "malformed sha256 rejected",
        lambda d: d["artifacts"][0].update(sha256="not-a-hash"))

    # A proof claiming performed=true must carry a date.
    passed &= check_rejects(
        manifest_schema, manifest, "performed proof without date rejected",
        lambda d: d["validation"]["physical_proof"].update(performed=True))

    print()
    if passed:
        print("RESULT: all schema checks passed")
    else:
        print("RESULT: failures present")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())