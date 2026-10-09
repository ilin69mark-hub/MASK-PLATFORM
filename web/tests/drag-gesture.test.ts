// A complete pointer gesture, driven the way the canvas drives it.
//
// TEST-005 exists because the studio's interaction has been checked by
// screenshots and DOM snapshots, never by a test that fails when a gesture stops
// working. The external browser driver available here cannot dispatch input —
// `click`, `hover` and `press_key` all time out — so the gesture is exercised
// through the same pure functions `StudioCanvas` calls, with the real WASM
// artifact, the real fixture and real millimetre-to-pixel arithmetic.
//
// That covers the part a screenshot cannot: that pressing on a vertex, moving,
// and releasing changes the geometry by the distance travelled, and that the
// vertex still under the pointer afterwards is the one that moved. BUG-020,
// BUG-021 and BUG-022 were all found by looking, not by running.
//
// Usage:
//   wasm-pack build --target nodejs --out-dir ../engine/wasm/pkg-node
//   node --experimental-strip-types tests/drag-gesture.test.ts

import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import {
  isLoaded,
  loadKernel,
  measure,
  widen,
  type Contour,
  type Shape,
  type Shapes,
  type Vertex,
} from "../lib/kernel.ts";
import {
  grabRadiusMm,
  hitVertex,
  nearestVertexMm,
  verticesOf,
} from "../lib/editor/hitTest.ts";
import {
  beginDrag,
  DRAG_THRESHOLD_MM,
  editFor,
  idleDrag,
  moveDrag,
  shouldCommit,
  type DragState,
} from "../lib/editor/dragEdit.ts";
import {
  mmPerPixel,
  mmToPx,
  pxRadiusToMm,
  pxToMm,
  viewFor,
  type DrawnSizePx,
  type ViewBoxMm,
} from "../lib/editor/viewTransform.ts";

const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const ROOT = resolve(here, "..", "..");

const PKG = resolve(ROOT, "engine/wasm/pkg-node/mask_wasm.js");
const FIXTURE = resolve(ROOT, "engine/fixtures/proof_panel.json");

async function boot() {
  if (isLoaded()) return;
  await loadKernel(async () => require(PKG));
}

interface FixtureLayer {
  index: number;
  name: string;
  width_mm: number;
  centrelines: [number, number][][];
}

function firstFixtureLayer(): FixtureLayer {
  const fixture = JSON.parse(readFileSync(FIXTURE, "utf8")) as { layers: FixtureLayer[] };
  const layer = fixture.layers[0];
  assert.ok(layer, "у фикстуры есть первый слой");
  return layer;
}

/** Build one fixture layer the way `StudioFromFixture` does. */
function buildLayer(layer: FixtureLayer): Shapes {
  const parts: Shape[] = [];
  for (const line of layer.centrelines) {
    const band = widen(line, layer.width_mm);
    for (const d of band.path_data) {
      const nums = (d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
      const points: Vertex[] = [];
      for (let i = 0; i + 1 < nums.length; i += 2) {
        points.push({ x: nums[i]!, y: nums[i + 1]! });
      }
      if (points.length >= 3) {
        parts.push({ outer: { points, closed: true }, holes: [] });
      }
    }
  }
  assert.ok(parts.length > 0, "в слое есть геометрия");
  return { shapes: parts };
}

/**
 * The view the studio builds, from the same `viewFor` the canvas calls.
 *
 * It is imported rather than rebuilt here on purpose: a second implementation
 * would agree with a broken canvas only by accident.
 */
function viewOf(shapes: Shapes): ViewBoxMm {
  return viewFor(measure(shapes).bbox);
}

function pointAt(shapes: Shapes, shape: number, contour: number, vertex: number) {
  const owner = shapes.shapes[shape];
  assert.ok(owner, "фигура существует");
  const list: Contour[] = [owner.outer, ...owner.holes];
  const found = list[contour];
  assert.ok(found, "контур существует");
  const point = found.points[vertex];
  assert.ok(point, "у вершины есть координата");
  return point;
}

/**
 * A vertex that is known to exist.
 *
 * Indexed access is `T | undefined` here, and a drag test that quietly operated
 * on `undefined` would report a geometry failure instead of saying which vertex
 * was missing.
 */
function pickVertex(shapes: Shapes, index: number) {
  const vertex = verticesOf(shapes)[index];
  assert.ok(vertex, `вершина ${index} должна существовать`);
  return vertex;
}

/** The vertex in the middle, which is not on a corner of the view. */
function middleVertex(shapes: Shapes) {
  const all = verticesOf(shapes);
  assert.ok(all.length > 0, "в слое есть вершины");
  return pickVertex(shapes, Math.floor(all.length / 2));
}

function withVertex(
  shapes: Shapes,
  shape: number,
  contour: number,
  vertex: number,
  at: Vertex,
): Shapes {
  const clone = JSON.parse(JSON.stringify(shapes)) as Shapes;
  const owner = clone.shapes[shape];
  assert.ok(owner, "фигура существует");
  const list: Contour[] = [owner.outer, ...owner.holes];
  const found = list[contour];
  assert.ok(found, "контур существует");
  found.points[vertex] = at;
  return clone;
}

const DRAWN: DrawnSizePx = { width: 900, height: 620 };

/**
 * Grab radius in screen pixels, converted to millimetres.
 *
 * `mmPerPixel` is millimetres per pixel — `mmToPx` divides by it — so a radius
 * of N pixels is N * mmPerPixel. Dividing instead gives a radius smaller than a
 * pixel, which happens to find a vertex under the exact centre and fails as soon
 * as the pointer is a pixel away.
 */
const GRAB_PX = 3;

test("a pointer gesture moves the vertex it grabbed, by the distance travelled", async () => {
  await boot();
  const shapes = buildLayer(firstFixtureLayer());
  const view = viewOf(shapes);
  const mmPerPx = mmPerPixel(view, DRAWN);

      const target = middleVertex(shapes);

  // Press exactly on the drawn vertex: screen pixels in, millimetres out. The
  // canvas does this conversion on every pointer event, so the test does too
  // rather than handing the hit test coordinates it never sees.
  const pressPx = mmToPx(target.at, view, DRAWN);
  const pressMm = pxToMm(pressPx, view, DRAWN);
  const grabbed = hitVertex(shapes, pressMm, { radiusMm: GRAB_PX * mmPerPx });
  assert.ok(grabbed, "нажатие на нарисованную вершину должно её схватить");
  assert.equal(grabbed.shape, target.shape, "схвачена та же фигура");
  assert.equal(grabbed.vertex, target.vertex, "схвачена та же вершина");

  // Move by a known amount in millimetres, expressed in pixels.
  // The drag works in millimetres: `DRAG_THRESHOLD_MM` is compared against the
  // distance it is handed, so pixels would make the threshold mean pixels.
  const wantedMm = 3;
  const moveMm = { x: pressMm.x + wantedMm, y: pressMm.y };
  const before: DragState = beginDrag(grabbed);
  const after: DragState = moveDrag(before, pressMm, moveMm);

  assert.ok(shouldCommit(after), "3 мм перемещения — это правка, а не клик");

  const edit = editFor(after, moveMm);
  assert.ok(edit, "правка должна получиться");
  assert.equal(edit.shape, grabbed.shape);
  assert.equal(edit.contour, grabbed.contour);
  assert.equal(edit.vertex, grabbed.vertex);
  // `editFor` hands over where the vertex should *end up*, which is where the
  // pointer is — a drag moves the vertex to the pointer.
  const landed = { x: edit.x_mm, y: edit.y_mm };
  assert.ok(
    Math.hypot(landed.x - moveMm.x, landed.y - moveMm.y) < 1e-9,
    "правка должна ставить вершину туда, где указатель"
  );

  const moved = withVertex(shapes, edit.shape, edit.contour, edit.vertex, landed);
  const got = pointAt(moved, edit.shape, edit.contour, edit.vertex);
  const travelled = Math.hypot(got.x - target.at.x, got.y - target.at.y);
  assert.ok(
    Math.abs(travelled - wantedMm) < 1e-6,
    `вершина должна уехать на ${wantedMm} мм, уехала на ${travelled}`
  );
});

test("the geometry the gesture produced is different from what it started as", async () => {
  // Coordinates alone would pass even if the edit were never applied. This is
  // the half a screenshot cannot see: the layer really changed. The content hash
  // is used rather than the bounding box, because moving one vertex of a closed
  // contour by 4 mm need not move the bounding box at all — which would let a
  // broken edit pass a box-based check.
  await boot();
  const shapes = buildLayer(firstFixtureLayer());
  const before = measure(shapes).content_hash;

    const target = pickVertex(shapes, 0);
  const view = viewOf(shapes);
  const mmPerPx = mmPerPixel(view, DRAWN);
  const pressPx = mmToPx(target.at, view, DRAWN);
  const pressMm = pxToMm(pressPx, view, DRAWN);
  const grabbed = hitVertex(shapes, pressMm, { radiusMm: GRAB_PX * mmPerPx });
  assert.ok(grabbed);

  const moveMm = { x: pressMm.x + 4, y: pressMm.y + 4 };
  const state = moveDrag(beginDrag(grabbed), pressMm, moveMm);
  const edit = editFor(state, moveMm);
  assert.ok(edit);

  const moved = withVertex(
    shapes,
    edit.shape,
    edit.contour,
    edit.vertex,
    { x: edit.x_mm, y: edit.y_mm },
  );
  const after = measure(moved).content_hash;
  assert.notEqual(after, before, "хеш геометрии обязан измениться");
});

test("a press that never travels is not an edit", async () => {
  await boot();
  const shapes = buildLayer(firstFixtureLayer());
    const target = pickVertex(shapes, 0);
  const view = viewOf(shapes);
  const mmPerPx = mmPerPixel(view, DRAWN);
  const pressPx = mmToPx(target.at, view, DRAWN);
  const pressMm = pxToMm(pressPx, view, DRAWN);
  const grabbed = hitVertex(shapes, pressMm, { radiusMm: GRAB_PX * mmPerPx });
  assert.ok(grabbed);

  // A jitter smaller than the threshold, then release.
  const jitter = { x: pressMm.x + DRAG_THRESHOLD_MM / 2, y: pressMm.y };
  const state = moveDrag(beginDrag(grabbed), pressMm, jitter);
  assert.equal(
    shouldCommit(state),
    false,
    "дрожание руки не должно переписывать геометрию"
  );
});

test("releasing without a grab changes nothing", async () => {
  const idle = idleDrag();
  const moved = moveDrag(idle, { x: 0, y: 0 }, { x: 100, y: 100 });
  assert.equal(shouldCommit(moved), false, "без захвата правки нет");
  assert.equal(editFor(moved, { x: 100, y: 100 }), null, "и редактировать нечего");
});

test("the vertex under the pointer after the drag is the one that moved", async () => {
  // The round trip that BUG-022 was about: dots are drawn in one place and the
  // hit test looks in another. After an edit the geometry and the view have both
  // moved, so the pointer must still find the vertex it is sitting on.
  await boot();
  const shapes = buildLayer(firstFixtureLayer());
  const view = viewOf(shapes);
  const mmPerPx = mmPerPixel(view, DRAWN);

  const target = middleVertex(shapes);
  const pressPx = mmToPx(target.at, view, DRAWN);
  const pressMm = pxToMm(pressPx, view, DRAWN);
  const grabbed = hitVertex(shapes, pressMm, { radiusMm: GRAB_PX * mmPerPx });
  assert.ok(grabbed);

  const moveMm = { x: pressMm.x + 5, y: pressMm.y + 2 };
  const state = moveDrag(beginDrag(grabbed), pressMm, moveMm);
  const edit = editFor(state, moveMm);
  assert.ok(edit);

  const moved = withVertex(
    shapes,
    edit.shape,
    edit.contour,
    edit.vertex,
    { x: edit.x_mm, y: edit.y_mm },
  );

  // Where is it drawn now, and is that the same as where it sits?
  const nowMm = pointAt(moved, edit.shape, edit.contour, edit.vertex);
  const nowPx = mmToPx(nowMm, view, DRAWN);
  const roundTrip = pxToMm(nowPx, view, DRAWN);
  assert.ok(
    Math.hypot(roundTrip.x - nowMm.x, roundTrip.y - nowMm.y) < 1e-6,
    "мм → px → мм должно возвращать ту же точку"
  );

  const found = hitVertex(moved, roundTrip, { radiusMm: GRAB_PX * mmPerPx });
  assert.ok(found, "перемещённая вершина должна находиться под курсором");
  assert.equal(found.shape, edit.shape);
  assert.equal(found.vertex, edit.vertex, "и это должна быть та же вершина");

  // The distance from the pointer to the vertex it moved, in millimetres.
  const distance = nearestVertexMm(moved, roundTrip, { radiusMm: 5 * mmPerPx });
  assert.ok(distance !== null, "должна быть найдена хотя бы одна вершина");
  assert.ok(
    distance < 1e-6,
    `вершина под курсором на ${distance} мм, а не в нуле`
  );
});

test("a press on empty space grabs nothing and edits nothing", async () => {
  await boot();
  const shapes = buildLayer(firstFixtureLayer());
  const view = viewOf(shapes);
  const mmPerPx = mmPerPixel(view, DRAWN);
  const far = { x: 5, y: 5 }; // in mm, well outside the drawing
  const px = mmToPx(far, view, DRAWN);
  assert.equal(
    hitVertex(shapes, pxToMm(px, view, DRAWN), { radiusMm: GRAB_PX * mmPerPx }),
    null,
    "мимо вершины grabbing не происходит"
  );
  const state = moveDrag(beginDrag(null), pxToMm(px, view, DRAWN), { x: pxToMm(px, view, DRAWN).x + 50, y: pxToMm(px, view, DRAWN).y });
  assert.equal(shouldCommit(state), false);
});
test("a press a couple of pixels off the vertex still grabs it", async () => {
  // The half that was missing. Every other test presses exactly on the vertex,
  // so shrinking the grab radius to nothing would not have failed any of them —
  // distance zero is inside any radius. But the radius is the thing that makes
  // the tool usable: the pointer is never exactly on a dot.
  //
  // The radius is derived from the geometry (see `grabRadiusMm`), so the test
  // states the property rather than a number: a small offset is grabbed, and one
  // clearly beyond it is not.
  await boot();
  const shapes = buildLayer(firstFixtureLayer());
  const view = viewOf(shapes);
  const mmPerPx = mmPerPixel(view, DRAWN);
  const radiusPx = 3;

  const target = middleVertex(shapes);
  const centre = mmToPx(target.at, view, DRAWN);

  // Inside the radius: one and a half pixels off in both axes.
  const near = { x: centre.x + radiusPx * 0.5, y: centre.y + radiusPx * 0.5 };
  const nearMm = pxToMm(near, view, DRAWN);
  const grabbedNear = hitVertex(shapes, nearMm, { radiusMm: radiusPx * mmPerPx });
  assert.ok(grabbedNear, "нажатие рядом с вершиной должно её схватить");
  assert.equal(grabbedNear.shape, target.shape);
  assert.equal(grabbedNear.vertex, target.vertex);

  // Beyond it: the same vertex must not be grabbed from far away.
  const far = { x: centre.x + radiusPx * 6, y: centre.y + radiusPx * 6 };
  const farMm = pxToMm(far, view, DRAWN);
  const grabbedFar = hitVertex(shapes, farMm, { radiusMm: radiusPx * mmPerPx });
  assert.ok(
    !grabbedFar || grabbedFar.vertex !== target.vertex,
    " издалека вершина не должна захватываться"
  );
});

test("the grab radius never falls below the pointer-sized target", async () => {
  // The canvas asks for 9 CSS pixels converted to millimetres, then takes the
  // larger of that and a floor derived from the spacing between vertices. Both
  // halves matter and both were added to fix "dots are drawn here, the hit test
  // looks there", so both are stated here rather than implied.

  // The geometry term can only raise the radius.
  const requestedMm = 0.5;
  assert.ok(
    grabRadiusMm(requestedMm, sparseShapes()) >= requestedMm,
    "пол не может уменьшить запрошенный радиус"
  );
  // And it is monotonic: a larger request never yields a smaller radius.
  for (const a of [0.1, 0.5, 2, 9]) {
    for (const b of [0.1, 0.5, 2, 9]) {
      const ra = grabRadiusMm(a, sparseShapes());
      const rb = grabRadiusMm(b, sparseShapes());
      if (a <= b) {
        assert.ok(ra <= rb + 1e-12, `радиус убывает: ${a}→${ra}, ${b}→${rb}`);
      }
    }
  }

  // On geometry whose vertices are far apart, the floor is what saves it: at 9 px
  // on a 1600 px window the request is under half a millimetre, which no pointer
  // can hit, and the spacing term is several times larger.
  await boot();
  const view = viewOf(sparseShapes());
  const requestedOnWide = pxRadiusToMm(9, view, { width: 1600, height: 900 });
  const radius = grabRadiusMm(requestedOnWide, sparseShapes());
  assert.ok(
    radius > requestedOnWide,
    "на разреженной геометрии радиус должен быть больше пиксельного запроса"
  );
  assert.ok(
    radius > 1,
    `и не меньше миллиметра, иначе вершина не попадает под курсор: ${radius}`
  );
});

/**
 * One triangle with 300 mm edges, so the vertices are genuinely far apart.
 *
 * Two small squares placed 200 mm apart do not do this: the nearest neighbour of
 * a vertex on such a square is its own partner 2 mm away, so the median spacing
 * is 2 mm and the geometry term stays useless. The floor only earns its place
 * when the edges themselves are long.
 */
function sparseShapes(): Shapes {
  return {
    shapes: [
      {
        outer: {
          points: [
            { x: 0, y: 0 },
            { x: 300, y: 0 },
            { x: 150, y: 260 },
          ],
          closed: true,
        },
        holes: [],
      },
    ],
  } as Shapes;
}
