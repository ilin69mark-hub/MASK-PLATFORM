#!/usr/bin/env python3
"""Validate artifacts the engine actually produced against the JSON Schemas.

`validate_examples.py` proves the schemas accept hand-written examples. That is
not enough: the engine is free to emit a manifest the schema rejects, and it did
once, in five places at the same time. A schema nothing produces against is a
document, not a contract.

So this runs the engine on a fixture and checks the real output.

Usage:
    make demo
    python3 docs/schema/validate_generated.py
"""

import copy
import json
import pathlib
import re
import sys

try:
    from jsonschema import Draft202012Validator
except ImportError:
    sys.exit("jsonschema is required: pip install jsonschema")

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "docs" / "schema" / "manifest.v1.json"
OUT = ROOT / "engine" / "out"

# The production id pattern, taken from the schema rather than restated: a second
# copy of a pattern is a second thing to forget to update.
PRODUCTION_ID = re.compile(
    json.loads((ROOT / "docs" / "schema" / "manifest.v1.json").read_text(encoding="utf-8"))
    ["properties"]["project"]["properties"]["id"]["pattern"]
)


def load(p):
    return json.loads(p.read_text(encoding="utf-8"))


def main():
    manifests = sorted(OUT.glob("*_manifest.json"))
    if not manifests:
        print("no manifests in engine/out — run `make demo` first")
        return 1

    schema = load(SCHEMA)
    validator = Draft202012Validator(schema)
    failures = 0

    for path in manifests:
        doc = load(path)

        # A fixture cannot carry a production id — that is the whole point of
        # ADR-0014: `MASK-000127` is never reused, and a test must not be able to
        # collide with one. So a manifest built from a fixture cannot satisfy the
        # schema's id pattern, and loosening the pattern to accept both would
        # weaken the production contract to accommodate a test.
        #
        # The id is substituted here so the rest of the manifest is checked as the
        # engine emitted it. What this proves is the *shape* of the engine's
        # output; whether a given id is a real project is not the engine's
        # decision and not this script's to assert.
        checked = copy.deepcopy(doc)
        generated_id = checked["project"]["id"]
        if not PRODUCTION_ID.match(generated_id):
            checked["project"]["id"] = "MASK-000000"
            substituted = True
        else:
            substituted = False

        errors = sorted(validator.iter_errors(checked), key=lambda e: list(e.path))
        if errors:
            failures += 1
            print(f"FAIL {path.name}")
            for err in errors[:8]:
                loc = "/".join(str(p) for p in err.path) or "<root>"
                print(f"     at {loc}: {err.message[:160]}")
        else:
            proof = doc.get("validation", {}).get("physical_proof", {})
            note = "  [fixture id, substituted for the pattern]" if substituted else ""
            print(
                f"ok   {path.name}  verdict={doc['validation']['verdict']}  "
                f"physical_proof={proof.get('performed')}{note}"
            )

    # The honest-uncertainty rule: a generated manifest must never claim the
    # limits were proven when no film has been cut.
    for path in manifests:
        doc = load(path)
        proof = doc.get("validation", {}).get("physical_proof", {})
        if proof.get("performed") is True:
            print(f"FAIL {path.name}: claims performed=true with no recorded proof")
            failures += 1

    if failures:
        print(f"\nRESULT: {failures} failure(s)")
        return 1
    print("\nRESULT: every generated manifest validates and declares its proof state")
    return 0


if __name__ == "__main__":
    sys.exit(main())
