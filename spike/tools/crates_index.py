#!/usr/bin/env python3
"""Read the crates.io sparse index and report recent versions for a crate."""
import json
import sys
import urllib.request


def index_path(name: str) -> str:
    n = len(name)
    if n == 1:
        return f"1/{name}"
    if n == 2:
        return f"2/{name}"
    if n == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[0:2]}/{name[2:4]}/{name}"


def main(names):
    for name in names:
        url = f"https://index.crates.io/{index_path(name)}"
        print(f"=== {name}  ->  {url}")
        try:
            with urllib.request.urlopen(url, timeout=20) as resp:
                lines = resp.read().decode().strip().splitlines()
        except Exception as exc:  # noqa: BLE001
            print(f"  ERROR: {exc}")
            continue

        rows = []
        for line in lines:
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                continue

        alive = [r for r in rows if not r.get("yanked")]
        for rec in alive[-4:]:
            feats = rec.get("features") or {}
            deps = rec.get("deps") or []
            print(
                f"  {rec['vers']:<10} rust={rec.get('rust_version') or '-':<8}"
                f" features={','.join(sorted(feats)[:6]) or '-'}"
            )
        print(f"  total versions: {len(rows)}, alive: {len(alive)}")

        latest = alive[-1] if alive else None
        if latest:
            runtime = [
                d["name"] for d in (latest.get("deps") or [])
                if d.get("kind") == "normal" and d.get("name") not in {"serde", "libc"}
            ]
            print(f"  latest deps: {', '.join(runtime[:12]) or '-'}")


if __name__ == "__main__":
    main(sys.argv[1:])