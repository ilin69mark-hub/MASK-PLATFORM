import assert from "node:assert/strict";
import test from "node:test";

import { applyVertexEdit } from "../lib/editor/applyEdit.ts";
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
