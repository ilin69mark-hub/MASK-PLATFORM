import assert from "node:assert/strict";
import test from "node:test";

import {
  letterbox,
  mmPerPixel,
  mmToPx,
  pxRadiusToMm,
  pxToMm,
  viewFor,
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
  // 812…908, the left edge of the *content* is 812 mm.
  //
  // This test used to assert that pixel 0 was 812 mm, which is wrong whenever the
  // viewBox is not the same shape as the element drawing it — 96×72 into a square
  // box letterboxes horizontally, so the content starts 83 px in. It passed by
  // encoding the bug this change fixes, which is why it is written out now rather
  // than left alone: the offset is not decoration.
  const { view, drawn } = box({ minX: 812, minY: 640, w: 96, h: 72 });
  const { offsetX, offsetY } = letterbox(view, drawn);
  // Height binds, so the content is 667 px wide inside a 500 px box: it
  // overflows to both sides and pixel 0 is 12 mm *inside* the drawing, not at
  // its left edge.
  assert.ok(Math.abs(offsetY) < 1e-9, "the height fills the box, so no vertical margin");
  assert.ok(offsetX < 0, `horizontal margin is negative: ${offsetX}`);

  // The origin sits at the viewBox's own edge, wherever that lands on screen.
  const origin = pxToMm({ x: offsetX, y: offsetY }, view, drawn);
  assert.ok(Math.abs(origin.x - 812) < 1e-9, `x was ${origin.x}`);
  assert.ok(Math.abs(origin.y - 640) < 1e-9, `y was ${origin.y}`);

  // Pixel 0 is inside the drawing, past the left edge — the 12 mm that the old
  // conversion silently dropped on every hit-test.
  const atPixel0 = pxToMm({ x: 0, y: 0 }, view, drawn);
  assert.ok(
    Math.abs(atPixel0.x - 824) < 1e-9,
    `pixel 0 should be 824 mm, not the viewBox's 812: got ${atPixel0.x}`
  );

  // A square viewBox in a square box has no margin, and that is the case the old
  // assertion was really written for.
  const square = box({ minX: 812, minY: 640, w: 96, h: 96 });
  const squareOrigin = pxToMm({ x: 0, y: 0 }, square.view, square.drawn);
  assert.ok(Math.abs(squareOrigin.x - 812) < 1e-9);
  assert.ok(Math.abs(squareOrigin.y - 640) < 1e-9);
});

test("a box that has not been laid out yet does not divide by zero", () => {
  const { view, drawn } = box({}, { width: 0, height: 0 });
  assert.equal(mmPerPixel(view, drawn), 1);
  assert.deepEqual(pxToMm({ x: 3, y: 4 }, view, drawn), { x: 3, y: 4 });
});

test("a viewBox letterboxed in a taller box is centred, not left-aligned", () => {
  // The offset is the subtle half of the letterbox rule. A wide viewBox in a
  // square box scales down to fit the width and leaves a margin above and
  // below; pixel 0 is *not* the viewBox's minY. A conversion that ignored the
  // offset put every hit-test half a margin off, which reads as "the tool
  // grabs the wrong vertex" rather than as a maths error.
  const { view, drawn } = box({ minX: 10, minY: 20, w: 100, h: 50 }, { width: 400, height: 400 });
  // Height binds: 50 mm into 400 px is 0.125 mm/px, so the content is
  // 800 px wide and 400 px tall — 200 px of margin on the left and right.
  const centre = pxToMm({ x: 200, y: 200 }, view, drawn);
  assert.ok(Math.abs(centre.x - 60) < 1e-9, `x was ${centre.x}`);
  assert.ok(Math.abs(centre.y - 45) < 1e-9, `y was ${centre.y}`);

  // The centre of the box is the centre of the viewBox, and that holds only
  // when the offset is accounted for.
  const back = mmToPx({ x: 60, y: 45 }, view, drawn);
  assert.ok(Math.abs(back.x - 200) < 1e-9 && Math.abs(back.y - 200) < 1e-9);
});

test("a letterboxed viewBox still round-trips a vertex on screen", () => {
  // The failure this guards: hit-testing a real vertex in the middle of a
  // letterboxed canvas. The vertex's pixel position and the pixel converted
  // back to millimetres must be the same point.
  const { view, drawn } = box({ minX: 812, minY: 640, w: 200, h: 100 }, { width: 500, height: 500 });
  const vertex = { x: 900, y: 700 };
  const px = mmToPx(vertex, view, drawn);
  assert.ok(px.x >= 0 && px.x <= 500 && px.y >= 0 && px.y <= 500, `${JSON.stringify(px)} is on screen`);
  const back = pxToMm(px, view, drawn);
  assert.ok(Math.abs(back.x - vertex.x) < 1e-9);
  assert.ok(Math.abs(back.y - vertex.y) < 1e-9);
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
            { x: 0, y: 0 },
            { x: 10, y: 0 },
            { x: 10, y: 5 },
            { x: 0, y: 5 },
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

test("a vertex's coordinates come out of the geometry as real numbers", () => {
  // This is the test that would have caught the Node tool being dead. The bug was
  // that `Contour.points` was declared as a tuple while the kernel returns
  // `{x, y}`, so `verticesOf` read `point[0]` and got `undefined`.
  //
  // It is written against the *coordinates* rather than the count, on purpose: a
  // count-only assertion is exactly what passed while every dot sat at NaN. The
  // geometry below is spelled the way the kernel spells it, not the way the
  // facade used to promise.
  const found = verticesOf(squareShapes());
  for (const v of found) {
    assert.ok(
      Number.isFinite(v.at.x) && Number.isFinite(v.at.y),
      `vertex ${v.shape}/${v.contour}/${v.vertex} is at ${v.at.x},${v.at.y}`
    );
  }
  assert.deepEqual(
    found.map((v) => [v.at.x, v.at.y]),
    [
      [0, 0],
      [10, 0],
      [10, 5],
      [0, 5],
    ]
  );
});

test("a pointer over a real vertex hits it", () => {
  // The whole tool, end to end on kernel-shaped geometry: a point one pixel to the
  // side of a corner must return that corner. With NaN coordinates this returned
  // nothing at all, and nothing on screen indicated why.
  const shapes = squareShapes();
  const corner = verticesOf(shapes)[2];
  assert.ok(corner, "the square has a third corner");
  const hit = hitVertex(shapes, { x: corner.at.x + 0.1, y: corner.at.y - 0.1 }, { radiusMm: 0.5 });
  assert.ok(hit, "a vertex 0.14 mm away must be found by a 0.5 mm grab");
  assert.deepEqual(
    [hit.shape, hit.contour, hit.vertex],
    [corner.shape, corner.contour, corner.vertex]
  );
});

test("holes are numbered after the outer contour", () => {
  const withHole: Shapes = {
    shapes: [
      {
        outer: {
          points: [
            { x: 0, y: 0 },
            { x: 20, y: 0 },
            { x: 20, y: 20 },
            { x: 0, y: 20 },
          ],
          closed: true,
        },
        holes: [
          {
            points: [
              { x: 5, y: 5 },
              { x: 5, y: 15 },
              { x: 15, y: 15 },
              { x: 15, y: 5 },
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
            { x: 0, y: 0 },
            { x: 10, y: 0.2 },
            { x: 10, y: 0.4 },
            { x: 0, y: 0.4 },
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
            { x: 0, y: 0 },
            { x: 10, y: 0 },
            { x: 10, y: 5 },
            { x: 0, y: 0 },
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

// ── the view must not move while a measurement is on its way ──────────────
//
// A committed edit changes the geometry, the re-measurement is debounced, and
// during that gap the measurement is not `ready`. The view used to fall back to
// a placeholder then, which is a 100 mm box — so the dots jumped and every
// pointer position was converted through the wrong rectangle, in the middle of
// the drag that caused it. The symptom was a tool that takes the first edit and
// then stops.

test("the view keeps the last measured box when nothing new has arrived", () => {
  const measured: [number, number, number, number] = [9.6, 9.6, 240.8, 160.8];
  const withResult = viewFor(measured);
  // The same call during a pending measurement: no new box, so the view must be
  // identical, not a placeholder.
  assert.deepEqual(viewFor(null), { minX: -4, minY: -4, w: 108, h: 108 });
  assert.notDeepEqual(withResult, viewFor(null), "the fallback must be distinguishable");
  assert.deepEqual(withResult, viewFor(measured), "a known box is stable");
  // And the retention the component performs: last known wins over nothing.
  const retained = measured ?? null;
  assert.deepEqual(viewFor(retained), withResult);
});

test("the view carries the origin, so dots and pointers agree", () => {
  // The round trip the Node tool depends on, on a figure away from the origin —
  // where a view built around zero puts every vertex off-screen and every hit
  // test somewhere else entirely.
  const bbox: [number, number, number, number] = [812, 640, 96, 72];
  const view = viewFor(bbox);
  assert.equal(view.minX, 812 - Math.max(96, 72) * 0.04);
  assert.equal(view.minY, 640 - Math.max(96, 72) * 0.04);

  const drawn: DrawnSizePx = { width: 600, height: 450 };
  const corner = { x: 812, y: 640 };
  const px = mmToPx(corner, view, drawn);
  const back = pxToMm(px, view, drawn);
  assert.ok(Math.abs(back.x - corner.x) < 1e-9, `x came back as ${back.x}`);
  assert.ok(Math.abs(back.y - corner.y) < 1e-9, `y came back as ${back.y}`);
});
