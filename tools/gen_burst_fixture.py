#!/usr/bin/env python3
"""Generate the `burst` fixture: a fan of painted ribbons leaving an off-centre focus.

Eight layers in paint order, each a family of centrelines that the fixture format
widens into constant-width bands. It is a template rather than a test: a starting
point that measures, warns and cuts, so the editor has something real to open.

Two things here were learned the hard way and are commented where they matter:

* Ribbons are **truncated** where they leave the canvas, never clamped to it.
  Clamping drags a run of points onto the edge and leaves them coincident, and the
  validator calls the result a 0.005 mm edge — correctly.
* Ribbons **do not all start at the focus**. Staggering their inner ends is both
  truer to a painted composition and necessary: lenses leaving one point are
  within a millisecond of each other there and trip the 0.90 mm gap rule inside
  every layer.

The verdict in `_expect` is measured with `make fixtures`, not chosen here.

Usage:
    python3 tools/gen_burst_fixture.py > engine/fixtures/burst.json
"""

import json
import math
import sys

CANVAS = 280.0  # mm, square. Fits the 284 mm working field of knife-25mm, so the
                # sheet needs no nesting and can be cut in one pass.
# The focus sits near the bottom-right corner so the fan sweeps up and left
# across the whole sheet. Centred, ribbons heading away from the centre left the
# canvas within a few samples and were discarded, and the composition came out
# with two thirds of the sheet bare.
FOCUS = (248.0, 252.0)
MARGIN = 4.0
SAMPLES = 90


def inside(x, y):
    return MARGIN <= x <= CANVAS - MARGIN and MARGIN <= y <= CANVAS - MARGIN


def ribbon(angle_deg, start_mm, end_mm, sweep_deg, turn_deg):
    """Points along a ribbon leaving the focus at `angle_deg`, curving as it goes."""
    pts = []
    a0 = math.radians(angle_deg)
    sweep = math.radians(sweep_deg)
    for i in range(SAMPLES):
        t = i / (SAMPLES - 1)
        r = start_mm + (end_mm - start_mm) * t
        # Smoothstep, so the ribbon leaves the focus straight and bends late —
        # how a painted ribbon actually flows, and gentler where curvature would
        # pinch the offset.
        eased = t * t * (3.0 - 2.0 * t)
        a = a0 + math.radians(turn_deg) + sweep * eased
        x = FOCUS[0] + r * math.cos(a)
        y = FOCUS[1] + r * math.sin(a)

        if not inside(x, y):
            break
        if pts and math.hypot(x - pts[-1][0], y - pts[-1][1]) < 0.75:
            continue
        pts.append([round(x, 2), round(y, 2)])

    # A ribbon that leaves the frame after a dozen samples is not a ribbon.
    return pts if len(pts) >= 26 else []


def band(count, angle_lo, angle_hi, width, sweep, turn, offset=0.0, start=20.0):
    """A family of ribbons across an angular span, alternating their bend.

    `turn` alternates so neighbours cross instead of running parallel. Parallel
    ribbons at a constant gap are the one arrangement the validator has nothing to
    say about, and a template should exercise something.
    """
    if count == 1:
        angles = [angle_lo + offset]
    else:
        step = (angle_hi - angle_lo) / (count - 1)
        angles = [angle_lo + i * step + offset for i in range(count)]

    lines = []
    for n, angle in enumerate(angles):
        line = ribbon(
            angle,
            # Staggered inner ends, see the module docstring.
            start_mm=start + 17.0 * (n % 4),
            # Past the far corner, so every ribbon runs until it leaves the frame.
            # One that stops in open space reads as a stroke on a blank page.
            end_mm=430.0,
            sweep_deg=sweep + 9.0 * (n % 3),
            turn_deg=turn if n % 2 == 0 else -turn,
        )
        if line:
            lines.append(line)
    return {"width_mm": width, "centrelines": lines}


# Widths and counts are set by what the validator accepts, not by how dense the
# picture should look: thinner layers carry fewer ribbons, because they have the
# least room to spare against the 0.80 mm element minimum and the 0.90 mm gap.
LAYERS = [
    ("Carbon Base", band(1, 196, 196, 8.0, 15.0, 8.0, start=22.0)),
    ("Black Base", band(1, 203, 203, 7.0, 17.0, -7.0, start=58.0)),
    ("Steel Grey", band(1, 210, 210, 5.2, 13.0, 9.0, start=96.0)),
    ("Gold", band(1, 217, 217, 5.4, 16.0, -9.0, start=134.0)),
    ("Orange", band(1, 224, 224, 6.0, 14.0, 8.0, start=172.0)),
    ("Cyan", band(1, 231, 231, 5.6, 18.0, -10.0, start=210.0)),
    ("White Highlight", band(1, 238, 238, 2.4, 15.0, 8.0, start=246.0)),
    ("Gold Pinstripe", band(1, 245, 245, 1.5, 17.0, -7.0, start=282.0)),
]


def main():
    layers = [
        {
            "index": index,
            "name": name,
            "width_mm": fam["width_mm"],
            "centrelines": fam["centrelines"],
        }
        for index, (name, fam) in enumerate(LAYERS, start=1)
    ]

    doc = {
        "_comment": (
            "Fan template, eight layers in paint order, ribbons leaving an "
            f"off-centre focus at {FOCUS[0]:.0f},{FOCUS[1]:.0f} mm across a "
            f"{CANVAS:.0f} mm square that fits the 284 mm working field of "
            "knife-25mm, so the sheet needs no nesting. Every element is at or "
            "above the 0.80 mm minimum of matte-vinyl-090. The ribbons cross and "
            "form islands on purpose: that is the class of graphic the engine "
            "exists to judge. Generated by tools/gen_burst_fixture.py; the "
            "verdict in `_expect` was measured, not chosen."
        ),
        "project_id": "MASK-BURST-001",
        "revision": 1,
        "target": {
            "object": "Demo panel",
            "part": "Fan practice panel",
            "part_code": "FAN",
            "dimensions_mm": [CANVAS, CANVAS],
        },
        "layers": layers,
    }
    json.dump(doc, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()