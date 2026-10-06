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
    fixture_schema = load("fixture.v1.json")
    project = json.loads((FIXTURES / "mask-project.v1.example.json").read_text(encoding="utf-8"))
    manifest = json.loads((FIXTURES / "manifest.v1.example.json").read_text(encoding="utf-8"))

    passed = True
    print("── schemas are themselves valid ──")
    for name, schema in (("mask-project", project_schema), ("manifest", manifest_schema),
                         ("fixture", fixture_schema)):
        try:
            Draft202012Validator.check_schema(schema)
            print(f"ok   {name}.v1.json")
        except Exception as exc:  # noqa: BLE001
            print(f"FAIL {name}.v1.json: {exc}")
            passed = False

    print("\n── examples must validate ──")
    passed &= check_positive(project_schema, project, "mask-project example")
    passed &= check_positive(manifest_schema, manifest, "manifest example")

    # Every fixture in the repository, not a hand-picked example. A schema nothing
    # is checked against is documentation; these are the documents that decide
    # whether a check still fires.
    print("\n── every fixture in engine/fixtures must validate ──")
    fixture_dir = ROOT.parent.parent / "engine" / "fixtures"
    fixture_paths = sorted(fixture_dir.glob("*.json"))
    if not fixture_paths:
        print(f"FAIL no fixtures found in {fixture_dir}")
        passed = False
    for path in fixture_paths:
        doc = json.loads(path.read_text(encoding="utf-8"))
        passed &= check_positive(fixture_schema, doc, path.name)
    print(f"     {len(fixture_paths)} fixture(s)")

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

    # An override must name what it overrode. A manifest that records "we exported
    # anyway" without the codes is a manifest that has forgotten the only part a
    # reader needs.
    def override_without_codes(d):
        d["validation"]["export_override"] = {
            "reason": "because",
            "verdict_at_export": "FAIL",
            "warning": "not a production run",
        }
    passed &= check_rejects(
        manifest_schema, manifest, "export_override without its codes rejected",
        override_without_codes)

    # An override cannot claim a passing verdict: that combination is exactly the
    # "refused, but looks reviewed" state the field exists to prevent.
    def override_claiming_pass(d):
        d["validation"]["export_override"] = {
            "reason": "because",
            "codes": ["min_gap"],
            "verdict_at_export": "PASS",
            "warning": "not a production run",
        }
    passed &= check_rejects(
        manifest_schema, manifest, "export_override claiming PASS rejected",
        override_claiming_pass)

    # A proof claiming performed=true must carry a date.
    passed &= check_rejects(
        manifest_schema, manifest, "performed proof without date rejected",
        lambda d: d["validation"]["physical_proof"].update(performed=True))

    # A fixture has to state what it expects, or nothing can tell it apart from
    # one that stopped testing anything.
    sample = json.loads((fixture_dir / "proof_panel.json").read_text(encoding="utf-8"))
    passed &= check_positive(fixture_schema, sample, "fixture sample (proof_panel)")
    passed &= check_rejects(
        fixture_schema, sample, "fixture without _expect rejected",
        lambda d: d.pop("_expect"))
    # A code that no rule ever emits is rejected by the Rust test that compares the
    # expectation against the report, not here: spelling out the rule vocabulary in
    # the schema would give the rule set a second source of truth, which the whole
    # point of keeping meaning out of the schema is to avoid.
    passed &= check_rejects(
        fixture_schema, sample, "non-string rule code rejected",
        lambda d: d["_expect"].update(errors=[42]))
    passed &= check_rejects(
        fixture_schema, sample, "duplicate rule code rejected",
        lambda d: d["_expect"].update(errors=["min_gap", "min_gap"]))

    # outcome=rejected means no verdict: a document claiming both is ambiguous.
    rejected = json.loads((fixture_dir / "t10_self_intersection.json").read_text(encoding="utf-8"))
    passed &= check_positive(fixture_schema, rejected, "fixture sample (rejected outcome)")
    passed &= check_rejects(
        fixture_schema, rejected, "rejected fixture carrying a verdict rejected",
        lambda d: d["_expect"].update(verdict="FAIL"))
    def claim_a_verdict_without_one(d):
        # d["_expect"].update() would have worked here, but writing a new key is
        # clearer than merging into the old one.
        d["_expect"] = {"outcome": "verdict"}

    passed &= check_rejects(
        fixture_schema, rejected, "verdict outcome with no verdict rejected",
        claim_a_verdict_without_one)

    # Centrelines and regions in one layer leaves the stroke width ambiguous.
    passed &= check_rejects(
        fixture_schema, sample, "layer mixing centrelines and regions rejected",
        lambda d: d["layers"][0].update(regions=[{"outer": [[0, 0], [10, 0], [10, 10]]}]))

    # A production id must not be expressible here: MASK-000127 is never reused,
    # and a fixture must not be able to collide with one.
    passed &= check_rejects(
        fixture_schema, sample, "production project id rejected in a fixture",
        lambda d: d.update(project_id="MASK-000127"))

    # A region needs three points to be a figure a knife can cut.
    passed &= check_rejects(
        fixture_schema, sample, "two-point region outline rejected",
        lambda d: d["layers"][0].update(
            centrelines=[], regions=[{"outer": [[0, 0], [10, 10]]}]))

    print()
    if passed:
        print("RESULT: all schema checks passed")
    else:
        print("RESULT: failures present")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())