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

import json
import pathlib
import sys

try:
    from jsonschema import Draft202012Validator
except ImportError:
    sys.exit("jsonschema is required: pip install jsonschema")

ROOT = pathlib.Path(__file__).resolve().parents[2]
SCHEMA = ROOT / "docs" / "schema" / "manifest.v1.json"
OUT = ROOT / "engine" / "out"


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
        errors = sorted(validator.iter_errors(doc), key=lambda e: list(e.path))
        if errors:
            failures += 1
            print(f"FAIL {path.name}")
            for err in errors[:8]:
                loc = "/".join(str(p) for p in err.path) or "<root>"
                print(f"     at {loc}: {err.message[:160]}")
        else:
            proof = doc.get("validation", {}).get("physical_proof", {})
            print(
                f"ok   {path.name}  verdict={doc['validation']['verdict']}  "
                f"physical_proof={proof.get('performed')}"
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
