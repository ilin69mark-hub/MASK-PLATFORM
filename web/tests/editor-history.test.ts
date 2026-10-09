import assert from "node:assert/strict";
import test from "node:test";

import {
  beginDrag,
  DRAG_THRESHOLD_MM,
  editFor,
  idleDrag,
  moveDrag,
  shouldCommit,
} from "../lib/editor/dragEdit.ts";
import {
  canRedo,
  canUndo,
  emptyHistory,
  pushEdit,
  redo,
  undo,
  type History,
} from "../lib/editor/history.ts";
import type { Shapes } from "../lib/kernel.ts";

/** Two states, distinguishable without a kernel. */
const BEFORE: Shapes = { shapes: [] };
const AFTER: Shapes = {
  shapes: [
    {
      outer: { points: [{ x: 0, y: 0 }, { x: 1, y: 0 }, { x: 1, y: 1 }], closed: true },
      holes: [],
    },
  ],
};

test("a fresh history can do nothing", () => {
  const history = emptyHistory<Shapes>();
  assert.equal(canUndo(history), false);
  assert.equal(canRedo(history), false);
  assert.equal(undo(history, AFTER), null);
  assert.equal(redo(history, AFTER), null);
});

test("one edit is undone back to the state before it", () => {
  const history = pushEdit(emptyHistory<Shapes>(), BEFORE);
  const back = undo(history, AFTER);
  assert.ok(back);
  assert.deepEqual(back.shapes, BEFORE);
});

test("undo then redo returns to where it was", () => {
  let history = pushEdit(emptyHistory<Shapes>(), BEFORE);
  const back = undo(history, AFTER);
  assert.ok(back);
  history = back.history;
  assert.equal(canRedo(history), true);

  const forward = redo(history, BEFORE);
  assert.ok(forward);
  assert.deepEqual(forward.shapes, AFTER);
  assert.equal(canRedo(forward.history), false);
});

test("a new edit after an undo drops the redo branch", () => {
  let history = pushEdit(emptyHistory<Shapes>(), BEFORE);
  const back = undo(history, AFTER);
  assert.ok(back);
  history = pushEdit(back.history, back.shapes);
  assert.equal(canRedo(history), false, "the abandoned future is not reachable");
});

test("pushing the same state twice records one step", () => {
  // A drag fires on every pointer move. Recording each of them would need a
  // hundred undos to get back to where the drag began.
  let history = pushEdit(emptyHistory<Shapes>(), BEFORE);
  history = pushEdit(history, BEFORE);
  history = pushEdit(history, BEFORE);
  assert.equal(history.past.length, 1);
});

test("two layers keep two histories", () => {
  // The bug one global stack produces: switching layers and pressing undo moves a
  // different layer's geometry.
  const redBefore: Shapes = { shapes: [] };
  const redAfter: Shapes = {
    shapes: [
      {
        outer: { points: [{ x: 5, y: 5 }, { x: 6, y: 5 }, { x: 6, y: 6 }], closed: true },
        holes: [],
      },
    ],
  };
  let red: History<Shapes> = pushEdit(emptyHistory<Shapes>(), redBefore);
  let blue: History<Shapes> = emptyHistory<Shapes>();

  red = undo(red, redAfter)!.history;
  const blueBack = undo(blue, BEFORE);
  assert.equal(blueBack, null, "the blue layer has nothing to undo");
  assert.equal(canRedo(red), true, "and the red layer still has its own redo step");
});

test("a drag that did not move is not an edit", () => {
  // A click must not put a vertex on the canonical grid it was not on: that
  // changes the geometry and its hash, for a gesture that changed nothing.
  const grabbed = { shape: 0, contour: 0, vertex: 2, at: { x: 10, y: 5 } };
  let drag = beginDrag(grabbed);

  drag = moveDrag(drag, { x: 0, y: 0 }, { x: 0.01, y: 0 });
  assert.equal(shouldCommit(drag), false, "0.01 mm is a click, not a drag");

  drag = moveDrag(drag, { x: 0, y: 0 }, { x: DRAG_THRESHOLD_MM, y: 0 });
  assert.equal(shouldCommit(drag), true, `${DRAG_THRESHOLD_MM} mm counts`);
});

test("a drag measures from where it started, not from the last step", () => {
  // Otherwise a slow crawl across the canvas accumulates into an edit while a fast
  // flick in one place does not.
  const grabbed = { shape: 0, contour: 0, vertex: 2, at: { x: 10, y: 5 } };
  let drag = beginDrag(grabbed);
  const step = 0.01;
  for (let i = 0; i < 20; i += 1) {
    drag = moveDrag(drag, { x: i * step, y: 0 }, { x: (i + 1) * step, y: 0 });
  }
  assert.equal(shouldCommit(drag), true);
  assert.ok(Math.abs(drag.travelledMm - 0.2) < 1e-9);
});

test("no drag means no edit, whatever the pointer does", () => {
  const drag = moveDrag(idleDrag(), { x: 0, y: 0 }, { x: 99, y: 99 });
  assert.equal(shouldCommit(drag), false);
  assert.equal(editFor(drag, { x: 1, y: 1 }), null);
});

test("the edit carries the indices the kernel addresses", () => {
  const grabbed = { shape: 2, contour: 1, vertex: 3, at: { x: 10, y: 5 } };
  const drag = moveDrag(beginDrag(grabbed), { x: 0, y: 0 }, { x: 1, y: 0 });
  const edit = editFor(drag, { x: 12.5, y: 7.25 });
  assert.deepEqual(edit, {
    shape: 2,
    contour: 1,
    vertex: 3,
    x_mm: 12.5,
    y_mm: 7.25,
  });
});