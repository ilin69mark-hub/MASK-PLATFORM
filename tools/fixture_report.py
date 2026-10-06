#!/usr/bin/env python3
"""Report every fixture against the verdict it declares in its own `_expect` block.

The fixtures are executable protection: `cargo test -p mask-cli` refuses a fixture
that stopped doing what it says it does. This script is the same comparison made
legible — which fixture proves which rule, and whether it still agrees.

It is a report, not the gate. The gate is the test, because the test fails the
build and this does not.

Usage:  fixture_report.py <engine-dir>
"""

import json
import os
import pathlib
import re
import subprocess
import sys

# The Debug spelling the CLI prints, mapped onto the spelling a fixture declares.
DECLARED_SPELLING = {
    "Pass": "PASS",
    "PassWithWarnings": "PASS_WITH_WARNINGS",
    "Fail": "FAIL",
    "Fatal": "FATAL",
}


def verdict_from(stdout):
    found = re.search(r"^Verdict: (\w+)$", stdout, re.MULTILINE)
    return DECLARED_SPELLING.get(found.group(1)) if found else None


def counts_from(stdout):
    found = re.search(r"^ +fatal (\d+) · error (\d+) · warning (\d+) · info (\d+)$",
                      stdout, re.MULTILINE)
    return tuple(int(g) for g in found.groups()) if found else None


def run(binary, engine, fixture, material, plotter, extra):
    proc = subprocess.run(
        [binary, "--fixture", fixture, "--material", material,
         "--plotter", plotter, "--out", os.environ.get("FIXTURE_REPORT_OUT", "/tmp/maskctl-fixtures"), *extra],
        capture_output=True, text=True, cwd=engine)
    return proc.stdout + proc.stderr


def main():
    if len(sys.argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    engine = pathlib.Path(sys.argv[1]).resolve()
    binary = engine / "target" / "debug" / "maskctl"
    if not binary.exists():
        print(f"{binary} does not exist; run `make build` first.", file=sys.stderr)
        return 2

    material = engine / "profiles" / "data" / "matte-vinyl-090.v4.json"
    plotter = engine / "profiles" / "data" / "knife-25mm.v2.json"

    fixtures = sorted((engine / "fixtures").glob("*.json"))
    if not fixtures:
        print("no fixtures found", file=sys.stderr)
        return 2

    print(f"{'fixture':<24} {'declared':<20} {'plain':<20} {'nest':<20} counts")
    print("-" * 96)

    failures = []
    for path in fixtures:
        name = path.stem
        declared = json.loads(path.read_text())["_expect"]
        # A fixture expected to be refused at the source gate never gets a verdict,
        # so its expected outcome is the absence of one.
        want = declared["verdict"] if declared["outcome"] == "verdict" else None

        plain = verdict_from(run(binary, engine, str(path), str(material), str(plotter), []))
        nest = verdict_from(run(binary, engine, str(path), str(material), str(plotter), ["--nest"]))
        counts = counts_from(run(binary, engine, str(path), str(material), str(plotter), []))

        if plain != want:
            failures.append(
                f"{name}: declares {want or 'REJECTED'}, gets {plain or 'no verdict'}")
        counts_text = "·".join(str(c) for c in counts) if counts else "—"

        print(f"{name:<24} {want or 'REJECTED':<20} {plain or 'rejected':<20} {nest or 'rejected':<20} {counts_text}")

    print()
    if failures:
        print(f"{len(failures)} of {len(fixtures)} fixtures no longer do what they declare:")
        for line in failures:
            print(f"  - {line}")
        return 1
    print(f"All {len(fixtures)} fixtures do what they declare.")
    print("Nesting is reported for information only: `_expect` describes the uncut design.")
    return 0


if __name__ == "__main__":
    sys.exit(main())