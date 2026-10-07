/**
 * Applying one vertex edit to the geometry the editor holds.
 *
 * Pure apart from the injected `mover`: the kernel call itself lives in
 * `lib/kernel.ts` and needs WASM, which the unit tests do not have. Injecting it
 * keeps the rules — when history is pushed, what an undo returns — testable in
 * `node:test` without a browser.
 *
 * The rule is: a failed move pushes nothing. An edit the kernel refused never
 * existed as geometry, so there is nothing to undo back to.
 */

import { pushEdit, type History } from "@/lib/editor/history";
import type { Shapes, VertexEdit } from "@/lib/kernel";

export interface ApplyOutcome {
  shapes: Shapes;
  history: History<Shapes>;
}

/**
 * Apply one edit through `mover` (normally `moveVertex`).
 *
 * `mover` throws when the kernel refuses the move — onto a neighbour, outside
 * the bounds, collapsing the contour — and then the history is returned
 * unchanged, so a refused drag does not create an undo step that undoes nothing.
 */
export function applyVertexEdit(
  current: Shapes,
  history: History<Shapes>,
  edit: VertexEdit,
  mover: (shapes: Shapes, edit: VertexEdit) => { shapes: Shapes },
): ApplyOutcome {
  const moved = mover(current, edit);
  return {
    shapes: moved.shapes,
    history: pushEdit(history, current),
  };
}
