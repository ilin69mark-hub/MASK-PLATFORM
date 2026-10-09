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
 * How far apart vertices usually are — the median of each vertex's distance to
 * its closest neighbour.
 *
 * This is the scale the grab radius has to answer to. A fixed radius in pixels is
 * right on a dense mesh and useless on a coarse outline: on the studio's own
 * fixture, 26 vertices spread over a 223 × 143 mm part have a typical spacing of
 * 123 mm, so a 6 px radius — 1.2 mm, one percent of it — asks for pixel-perfect
 * aiming at the corner of a 220 mm band. The user aiming at the outline is not
 * missing the target; the target is a hundredth of the gap between targets.
 *
 * Median, not mean or minimum: a shape with one pair of almost-touching vertices
 * would otherwise shrink the radius for every other corner.
 */
export function typicalSpacingMm(shapes: Shapes): number | null {
  const vertices = verticesOf(shapes).map((v) => v.at);
  if (vertices.length < 2) return null;

  const distances: number[] = [];
  for (let i = 0; i < vertices.length; i += 1) {
    let nearest = Number.POSITIVE_INFINITY;
    for (let j = 0; j < vertices.length; j += 1) {
      if (i === j) continue;
      const a = vertices[i];
      const b = vertices[j];
      if (!a || !b) continue;
      const d = Math.hypot(a.x - b.x, a.y - b.y);
      if (d < nearest) nearest = d;
    }
    distances.push(nearest);
  }
  distances.sort((a, b) => a - b);
  const middle = Math.floor(distances.length / 2);
  // Odd count: the middle element is the median. Averaging the two middle ones is
  // the even-count rule, and applying it to an odd count pulled a 0.01 mm pair
  // halfway into the result — which is the one thing the median was chosen to
  // prevent.
  if (distances.length % 2 === 1) return distances[middle] ?? null;
  const lo = distances[middle - 1];
  const hi = distances[middle];
  return lo !== undefined && hi !== undefined ? (lo + hi) / 2 : (hi ?? lo ?? null);
}

/**
 * The radius a grab actually uses: the one the user aimed with, widened to a
 * small fraction of how far apart this geometry's vertices are.
 *
 * The fraction is deliberately modest, because widening too far is its own fault.
 * On a coarse outline a generous radius would grab whatever is nearest from
 * anywhere on the canvas — dragging the left edge while the cursor is in the
 * middle, a tool that answers every request with a different vertex.
 *
 * It only ever widens. A first version also capped it at a fraction of the
 * spacing, which shrank the radius on dense meshes to below what the user aimed
 * with — a pixel-precise tool becoming coarser the finer the geometry got, which
 * is backwards.
 */
export function grabRadiusMm(requestedMm: number, shapes: Shapes): number {
  const spacing = typicalSpacingMm(shapes);
  if (spacing === null || !Number.isFinite(spacing) || spacing <= 0) {
    return requestedMm;
  }
  return Math.max(requestedMm, spacing * 0.08);
}

/**
 * How far the nearest vertex is, in millimetres.
 *
 * Reported when a grab misses. A tool that says nothing when the pointer lands
 * 2 mm from a vertex and a tool that says nothing when it lands 400 mm from one
 * are the same silence, and they are not the same fault: the first is a radius
 * too tight, the second means the coordinates the pointer is converted through
 * are not the coordinates the dots are drawn in. Without the number, both look
 * like "the tool is broken", which is what it was being reported as.
 *
 * `null` for geometry with no vertices, where there is nothing to be near.
 */
export function nearestVertexMm(
  shapes: Shapes,
  at: Point2D,
  options: HitTestOptions = { radiusMm: 0 }
): number | null {
  const vertices = verticesOf(shapes, options.skipRepeatedFirstPoint ?? true);
  let best: number | null = null;
  for (const vertex of vertices) {
    const distance = Math.hypot(vertex.at.x - at.x, vertex.at.y - at.y);
    if (best === null || distance < best) best = distance;
  }
  return best;
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
