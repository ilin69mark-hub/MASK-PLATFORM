import assert from "node:assert/strict";
import test from "node:test";

import {
  applyVertexEdit,
  beginSession,
  endSession,
  idleSession,
} from "../lib/editor/applyEdit.ts";
import { canRedo, canUndo, emptyHistory, redo, undo } from "../lib/editor/history.ts";
import type { Shapes, VertexEdit } from "../lib/kernel.ts";

const BEFORE: Shapes = {
  shapes: [{ outer: { points: [[0, 0], [10, 0], [10, 5]], closed: true }, holes: [] }],
};

const AFTER: Shapes = {
  shapes: [{ outer: { points: [[0, 0], [12, 0], [10, 5]], closed: true }, holes: [] }],
};

const EDIT: VertexEdit = { shape: 0, contour: 0, vertex: 1, x_mm: 12, y_mm: 0 };

test("a successful edit returns new geometry and pushes the state before it", () => {
  const history = emptyHistory<Shapes>();
  const outcome = applyVertexEdit(BEFORE, history, EDIT, () => ({ shapes: AFTER }));
  assert.deepEqual(outcome.shapes, AFTER);
  assert.equal(canUndo(outcome.history), true);
  const back = undo(outcome.history, AFTER);
  assert.ok(back);
  assert.deepEqual(back.shapes, BEFORE);
});

test("a refused edit pushes nothing", () => {
  const history = emptyHistory<Shapes>();
  assert.throws(() =>
    applyVertexEdit(BEFORE, history, EDIT, () => {
      throw new Error("edit: vertex 1 would land on vertex 0; move it somewhere else");
    })
  );
  // The caller keeps its old history on throw — nothing to undo.
  assert.equal(canUndo(history), false);
});

test("undo then redo round-trips an applied edit", () => {
  let history = emptyHistory<Shapes>();
  const outcome = applyVertexEdit(BEFORE, history, EDIT, () => ({ shapes: AFTER }));
  history = outcome.history;
  const back = undo(history, AFTER);
  assert.ok(back);
  assert.deepEqual(back.shapes, BEFORE);
  const forward = redo(back.history, BEFORE);
  assert.ok(forward);
  assert.deepEqual(forward.shapes, AFTER);
  assert.equal(canRedo(forward.history), false);
});

test("one drag that commits many times is one undo step", () => {
  // A drag commits on every pointer move, so the measurement the user watches
  // belongs to the geometry being dragged. Without a session each of those
  // commits would push a step, and undoing the drag would take a hundred
  // presses to get back to where it started.
  const session = beginSession(BEFORE);
  let history = emptyHistory<Shapes>();
  let shapes = BEFORE;

  // Four commits, each from the evolving geometry — what a drag really does.
  for (const x of [11, 12, 13, 14]) {
    const outcome = applyVertexEdit(
      shapes,
      history,
      { ...EDIT, x_mm: x },
      () => ({ shapes: { shapes: [{ outer: { points: [[0, 0], [x, 0], [10, 5]], closed: true }, holes: [] }] } }),
      session
    );
    shapes = outcome.shapes;
    history = outcome.history;
  }

  assert.equal(history.past.length, 1, "one drag, one step");
  const back = undo(history, shapes);
  assert.ok(back);
  assert.deepEqual(back.shapes, BEFORE, "and it returns to before the drag, not mid-drag");
});

test("a drag that never committed records nothing", () => {
  // The session is taken when the user grabs a vertex, not when an edit lands.
  // A grab that never crossed the drag threshold must not leave a step.
  const session = beginSession(BEFORE);
  assert.equal(canUndo(emptyHistory<Shapes>()), false);
  assert.equal(session.before, BEFORE);
});

test("ending the session makes the next edit push its own pre-state", () => {
  // Two separate drags of the same vertex: the second must undo to where the
  // first left off, not all the way back to the original.
  const first = applyVertexEdit(BEFORE, emptyHistory<Shapes>(), EDIT, () => ({ shapes: AFTER }), beginSession(BEFORE));
  assert.deepEqual(first.history.past[0]?.shapes, BEFORE);

  const SECOND: Shapes = {
    shapes: [{ outer: { points: [[0, 0], [14, 0], [10, 5]], closed: true }, holes: [] }],
  };
  const second = applyVertexEdit(
    AFTER,
    first.history,
    { ...EDIT, x_mm: 14 },
    () => ({ shapes: SECOND }),
    beginSession(AFTER)
  );
  assert.equal(second.history.past.length, 2);
  assert.deepEqual(second.history.past[1]?.shapes, AFTER, "the second step undoes to the first drag's result");

  const back = undo(second.history, SECOND);
  assert.ok(back);
  assert.deepEqual(back.shapes, AFTER);
});

test("no session means a click is its own undo step", () => {
  // A single commit with no drag around it (a tap that crossed the threshold
  // in one move): the pre-edit state is the only snapshot there is.
  const outcome = applyVertexEdit(BEFORE, emptyHistory<Shapes>(), EDIT, () => ({ shapes: AFTER }));
  assert.deepEqual(outcome.history.past[0]?.shapes, BEFORE);
  assert.deepEqual(idleSession(), { before: null });
  assert.deepEqual(endSession(), { before: null });
});
