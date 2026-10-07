/**
 * The state of one vertex drag.
 *
 * A pure reducer, so the rules of the tool — what a drag starts on, where the
 * vertex goes, when a move is committed — are testable without a pointer, a DOM or
 * a canvas. The component feeds it events and sends the edits it produces to the
 * kernel; it decides nothing itself.
 */

import type { VertexEdit } from "@/lib/kernel";
import type { Point2D } from "@/lib/editor/viewTransform";
import type { VertexRef } from "@/lib/editor/hitTest";

export interface DragState {
  /** The vertex being dragged, or nothing when no drag is in progress. */
  grabbed: VertexRef | null;
  /**
   * How far the vertex has already travelled, in millimetres.
   *
   * A drag that never left the radius it started in is not an edit. Committing it
   * would put a vertex on the canonical grid it was not on before — changing the
   * geometry, and its hash, because a click was registered as a drag.
   */
  travelledMm: number;
}

/** Below this, a drag is treated as a click. */
export const DRAG_THRESHOLD_MM = 0.05;

export function idleDrag(): DragState {
  return { grabbed: null, travelledMm: 0 };
}

/** Pointer pressed on a vertex: the drag begins. */
export function beginDrag(grabbed: VertexRef | null): DragState {
  return { grabbed, travelledMm: 0 };
}

/**
 * Pointer moved while dragging.
 *
 * The threshold is compared against the distance from where the drag started, not
 * from the last position: measuring per-step would let a slow drag across the
 * canvas accumulate into an edit while a fast one in a single place would not.
 */
export function moveDrag(
  state: DragState,
  from: Point2D,
  to: Point2D
): DragState {
  if (!state.grabbed) return state;
  const distance = Math.hypot(to.x - from.x, to.y - from.y);
  return { ...state, travelledMm: state.travelledMm + distance };
}

/** Has this drag gone far enough to be an edit? */
export function shouldCommit(state: DragState): boolean {
  return state.grabbed !== null && state.travelledMm >= DRAG_THRESHOLD_MM;
}

/**
 * The edit this drag produces at its current position, or nothing.
 *
 * Built here rather than in the component so the mapping from a drag to a kernel
 * call is one tested function: an off-by-one in the vertex index would move a
 * neighbouring corner and look plausible on screen.
 */
export function editFor(state: DragState, to: Point2D): VertexEdit | null {
  const grabbed = state.grabbed;
  if (!grabbed) return null;
  return {
    shape: grabbed.shape,
    contour: grabbed.contour,
    vertex: grabbed.vertex,
    x_mm: to.x,
    y_mm: to.y,
  };
}