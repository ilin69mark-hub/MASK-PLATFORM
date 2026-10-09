/**
 * Finding the vertex a pointer is on.
 *
 * Pure, for the same reason `viewTransform` is: nothing here may touch the DOM, or
 * it cannot be tested in this project. The input is the geometry the editor holds
 * plus a point in millimetres; the output is which vertex, if any.
 */

import type { Contour, Shapes } from "@/lib/kernel";
import type { Point2D } from "@/lib/editor/viewTransform";

/** One vertex, addressed the way the kernel addresses it. */
export interface VertexRef {
  /** Index into `Shapes.shapes`. */
  shape: number;
  /** 0 for the outer contour, 1.. for its holes — `Shape::contours()` order. */
  contour: number;
  /** Index into the contour's points. */
  vertex: number;
  /** Where it is now, in millimetres. */
  at: Point2D;
}

export interface HitTestOptions {
  /** Grab radius in millimetres. */
  radiusMm: number;
  /**
   * Skip a last point that repeats the first.
   *
   * Some writers close a contour by repeating the first point. Every point of a
   * closed contour is a real corner otherwise — including the last, whose edge runs
   * back to the first — so the only point safe to skip is an exact duplicate.
   * `canonicalise` removes those in the kernel; this covers geometry that arrived
   * from elsewhere.
   */
  skipRepeatedFirstPoint?: boolean;
}

/** Every vertex of a shape set, in the order the kernel numbers them. */
export function verticesOf(shapes: Shapes, skipRepeatedFirstPoint = true): VertexRef[] {
  const out: VertexRef[] = [];
  shapes.shapes.forEach((shape: { outer: Contour; holes: Contour[] }, shapeIndex: number) => {
    const contours: Contour[] = [shape.outer, ...shape.holes];
    contours.forEach((contour, contourIndex) => {
      const last = contour.points.length - 1;
      contour.points.forEach((point, vertex) => {
        const first = contour.points[0];
        const repeatsFirst =
          vertex === last &&
          contour.points.length > 1 &&
          first !== undefined &&
          point.x === first.x &&
          point.y === first.y;
        if (skipRepeatedFirstPoint && repeatsFirst) return;
        out.push({
          shape: shapeIndex,
          contour: contourIndex,
          vertex,
          at: { x: point.x, y: point.y },
        });
      });
    });
  });
  return out;
}

/**
 * The nearest vertex within the radius, or nothing.
 *
 * Ties are refused rather than resolved: two vertices of a thin band can be closer
 * to the pointer than its own diameter, and picking the "first" one would make the
 * tool feel like it grabs the wrong vertex at random depending on array order.
 * Returning nothing is honest — the user moves a pixel and tries again — while a
 * coin flip would be indistinguishable from a bug.
 */
export function hitVertex(
  shapes: Shapes,
  at: Point2D,
  options: HitTestOptions
): VertexRef | null {
  const radius = options.radiusMm;
  if (!(radius > 0)) return null;

  let best: VertexRef | null = null;
  let bestDistance = Number.POSITIVE_INFINITY;
  let tied = false;

  for (const vertex of verticesOf(shapes, options.skipRepeatedFirstPoint ?? true)) {
    const distance = Math.hypot(vertex.at.x - at.x, vertex.at.y - at.y);
    if (distance > radius) continue;
    if (distance < bestDistance) {
      best = vertex;
      bestDistance = distance;
      tied = false;
    } else if (Math.abs(distance - bestDistance) < 1e-12) {
      tied = true;
    }
  }

  return tied ? null : best;
}
