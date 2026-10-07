import assert from "node:assert/strict";
import test from "node:test";

import {
  mmPerPixel,
  mmToPx,
  pxRadiusToMm,
  pxToMm,
  type DrawnSizePx,
  type ViewBoxMm,
} from "../lib/editor/viewTransform.ts";
import { hitVertex, verticesOf } from "../lib/editor/hitTest.ts";
import type { Shapes } from "../lib/kernel.ts";

/** A viewBox in millimetres and the pixels it is drawn into. */
function box(view: Partial<ViewBoxMm> = {}, drawn: Partial<DrawnSizePx> = {}) {
  return {
    view: { minX: 0, minY: 0, w: 100, h: 100, ...view },
    drawn: { width: 500, height: 500, ...drawn },
  };
}

test("a square viewBox drawn square has one scale", () => {
  const { view, drawn } = box();
  assert.equal(mmPerPixel(view, drawn), 0.2);
});

test("letterboxing uses the smaller scale, not the larger", () => {
  // A wide viewBox in a wide box: the width fits exactly, the height has room to
  // spare. Taking the larger factor would put everything on the short axis outside
  // the drawing, and a hit-test there would select nothing.
  const { view, drawn } = box({ w: 200, h: 50 }, { width: 400, height: 400 });
  // The height binds: 50 mm into 400 px is 0.125 mm per pixel, and taking the
  // width's 0.5 would map the whole vertical extent off the box.
  assert.equal(mmPerPixel(view, drawn), 0.125);
  assert.equal(mmPerPixel(view, drawn), Math.min(view.w / 400, view.h / 400));
});

test("millimetres and pixels convert both ways", () => {
  const { view, drawn } = box();
  const mm = { x: 12.5, y: 40 };
  const px = mmToPx(mm, view, drawn);
  assert.deepEqual(px, { x: 62.5, y: 200 });
  const back = pxToMm(px, view, drawn);
  assert.ok(Math.abs(back.x - mm.x) < 1e-9 && Math.abs(back.y - mm.y) < 1e-9);
});

test("a viewBox away from the origin maps correctly", () => {
  // The reason `bbox` started reporting a real origin (BUG-018): with the view at
  // 812…908, pixel 0 is 812 mm, and a conversion that assumed zero would put every
  // hit-test 812 mm off.
  const { view, drawn } = box({ minX: 812, minY: 640, w: 96, h: 72 });
  const mm = pxToMm({ x: 0, y: 0 }, view, drawn);
  assert.ok(Math.abs(mm.x - 812) < 1e-9);
  assert.ok(Math.abs(mm.y - 640) < 1e-9);
});

test("a box that has not been laid out yet does not divide by zero", () => {
  const { view, drawn } = box({}, { width: 0, height: 0 });
  assert.equal(mmPerPixel(view, drawn), 1);
  assert.deepEqual(pxToMm({ x: 3, y: 4 }, view, drawn), { x: 3, y: 4 });
});

test("a grab radius in pixels becomes millimetres at this scale", () => {
  const { view, drawn } = box({ w: 200, h: 200 }, { width: 600, height: 600 });
  // One pixel is a third of a millimetre here, and a hundredth of one on the small
  // detail. A radius fixed in millimetres could not serve both.
  assert.ok(Math.abs(pxRadiusToMm(6, view, drawn) - 2) < 1e-9);
  const small = box({ w: 5, h: 5 }, { width: 600, height: 600 });
  assert.ok(pxRadiusToMm(6, small.view, small.drawn) < 0.1);
});

/** A 10 × 5 mm square as one region. */
function squareShapes(): Shapes {
  return {
    shapes: [
      {
        outer: {
          points: [
            [0, 0],
            [10, 0],
            [10, 5],
            [0, 5],
          ],
          closed: true,
        },
        holes: [],
      },
    ],
  };
}

test("vertices are listed in the kernel's own numbering", () => {
  const found = verticesOf(squareShapes());
  assert.equal(found.length, 4, "every corner of a closed contour is real");
  assert.deepEqual(
    found.map((v) => [v.shape, v.contour, v.vertex]),
    [
      [0, 0, 0],
      [0, 0, 1],
      [0, 0, 2],
      [0, 0, 3],
    ]
  );
});

test("holes are numbered after the outer contour", () => {
  const withHole: Shapes = {
    shapes: [
      {
        outer: {
          points: [
            [0, 0],
            [20, 0],
            [20, 20],
            [0, 20],
          ],
          closed: true,
        },
        holes: [
          {
            points: [
              [5, 5],
              [5, 15],
              [15, 15],
              [15, 5],
            ],
            closed: true,
          },
        ],
      },
    ],
  };
  const found = verticesOf(withHole);
  const hole = found.filter((v) => v.contour === 1);
  assert.equal(hole.length, 4);
  assert.deepEqual(hole[0], { shape: 0, contour: 1, vertex: 0, at: { x: 5, y: 5 } });
});

test("the vertex under the pointer is found, and only inside the radius", () => {
  const shapes = squareShapes();
  const hit = hitVertex(shapes, { x: 10.1, y: 0.1 }, { radiusMm: 0.5 });
  assert.ok(hit);
  assert.equal(hit.vertex, 1, "the corner at 10,0");
  assert.equal(hitVertex(shapes, { x: 5, y: 2.5 }, { radiusMm: 0.5 }), null);
});

test("a grab radius of nothing selects nothing", () => {
  assert.equal(hitVertex(squareShapes(), { x: 0, y: 0 }, { radiusMm: 0 }), null);
});

test("two vertices the same distance away are refused rather than guessed", () => {
  // A thin band puts two vertices closer together than a pointer's own accuracy.
  // Picking the first would make the tool feel random; picking neither makes the
  // user move a pixel and try again.
  //
  // The contour is open on purpose: on a closed one the closing point is not listed
  // (it shares the first edge), so the tie cannot be set up there at all.
  const band: Shapes = {
    shapes: [
      {
        outer: {
          points: [
            [0, 0],
            [10, 0.2],
            [10, 0.4],
            [0, 0.4],
          ],
          closed: false,
        },
        holes: [],
      },
    ],
  };
  const hit = hitVertex(band, { x: 0, y: 0.2 }, { radiusMm: 0.35 });
  assert.equal(hit, null, "an exact tie must not be resolved by array order");

  // Move the pointer closer to one of them and the answer becomes definite.
  const nearer = hitVertex(band, { x: 0, y: 0.1 }, { radiusMm: 0.35 });
  assert.ok(nearer, "a nearer vertex is a definite answer");
  assert.equal(nearer.vertex, 0);
});

test("a contour that closes by repeating its first point lists that point once", () => {
  // Some writers close by repeating the first point. Listing it twice would put two
  // dots on one corner and every click there would grab the same vertex.
  const repeated: Shapes = {
    shapes: [
      {
        outer: {
          points: [
            [0, 0],
            [10, 0],
            [10, 5],
            [0, 0],
          ],
          closed: true,
        },
        holes: [],
      },
    ],
  };
  const found = verticesOf(repeated);
  assert.equal(found.length, 3, "the repeated point is not a corner");
  assert.deepEqual(
    found.map((v) => v.vertex),
    [0, 1, 2],
    "and the indices the kernel would use are unchanged"
  );
});
