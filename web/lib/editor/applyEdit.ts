/**
 * Applying one vertex edit to the geometry the editor holds.
 *
 * Pure apart from the injected `mover`: the kernel call itself lives in
 * `lib/kernel.ts` and needs WASM, which the unit tests do not have. Injecting it
 * keeps the rules — when history is pushed, what an undo returns — testable in
 * `node:test` without a browser.
 *
 * Two rules, both about history:
 *
 * - A failed move pushes nothing. An edit the kernel refused never existed as
 *   geometry, so there is nothing to undo back to.
 * - One drag is one undo step. A drag commits on every pointer move (so the
 *   measurement the user watches belongs to the geometry being dragged), which
 *   would otherwise record a step per frame. The owner therefore holds the
 *   pre-drag geometry in a {@link DragSession} and pushes that same snapshot for
 *   every commit in the drag; `pushEdit` ignores a repeat, so the step appears
 *   once, at the start.
 */

import { pushEdit, type History } from "@/lib/editor/history";
import type { Shapes, VertexEdit } from "@/lib/kernel";

export interface ApplyOutcome {
  shapes: Shapes;
  history: History<Shapes>;
}

/**
 * The geometry as it was when a drag began, kept until it ends.
 *
 * `null` when no drag is in progress, which is the normal state: a single click
 * that never became an edit records nothing.
 */
export interface DragSession {
  before: Shapes | null;
}

export function idleSession(): DragSession {
  return { before: null };
}

/** Called when the user grabs a vertex: the geometry is snapshotted. */
export function beginSession(current: Shapes): DragSession {
  return { before: current };
}

/** Called when the drag ends. The snapshot stops being pushed. */
export function endSession(): DragSession {
  return { before: null };
}

/**
 * Apply one edit through `mover` (normally `moveVertex`).
 *
 * `mover` throws when the kernel refuses the move — onto a neighbour, outside
 * the bounds, collapsing the contour — and then the history is returned
 * unchanged, so a refused drag does not create an undo step that undoes nothing.
 *
 * The pushed snapshot is the drag's pre-state when there is a session, so one
 * drag is one step even though it commits many times.
 */
export function applyVertexEdit(
  current: Shapes,
  history: History<Shapes>,
  edit: VertexEdit,
  mover: (shapes: Shapes, edit: VertexEdit) => { shapes: Shapes },
  session: DragSession = idleSession(),
): ApplyOutcome {
  const moved = mover(current, edit);
  // Without a session this is a single click that crossed the drag threshold
  // in a single move; the pre-edit state is the only snapshot available.
  const snapshot = session.before ?? current;
  return {
    shapes: moved.shapes,
    history: pushEdit(history, snapshot),
  };
}