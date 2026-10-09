#!/usr/bin/env python3
"""Check that every dependency has its license text collected.

`docs/LICENSES/` is the attribution record for a commercial closed-source
product, and it goes stale the moment a dependency is added without
collecting its license. Reading the directory by hand does not catch that,
so this derives the expected set from `cargo metadata` and compares.

Fails, rather than warns: a missing license text is a compliance gap, and a
check that reports one without stopping is a check that gets ignored.
"""

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LICENSES = ROOT / "docs" / "LICENSES"
NOTICE = ROOT / "NOTICE"


def main() -> int:
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1"],
            cwd=ROOT / "engine",
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    )

    crates = sorted(
        p["name"] for p in metadata["packages"] if not p["name"].startswith("mask-")
    )
    collected = {p.name for p in LICENSES.iterdir() if p.is_file()}

    missing = []
    for crate in crates:
        # A crate is covered when any file is named for it. Dual-licensed crates
        # have several, and the two reconstructed ones are marked in the name.
        if not any(name.startswith(f"{crate}-") for name in collected):
            missing.append(crate)

    print(f"зависимостей: {len(crates)}")
    print(f"файлов лицензий: {len(collected)}")

    if missing:
        print("\nбез текста лицензии:")
        for crate in missing:
            print(f"  {crate}")
        print(
            "\nСкопируйте текст в docs/LICENSES/ и пересоберите NOTICE командой\n"
            "    make licenses"
        )
        return 1

    if not NOTICE.is_file():
        print("\nNOTICE отсутствует — пересоберите его командой `make licenses`")
        return 1

    print("NOTICE: на месте")
    print("каждая зависимость имеет текст лицензии")
    return 0


if __name__ == "__main__":
    sys.exit(main())
