import { verticesOf, type VertexRef } from "@/lib/editor/hitTest";
import type { Point2D } from "@/lib/editor/viewTransform";
import type { Shapes } from "@/lib/kernel";

/**
 * The Node tool's dots: one per vertex, the grabbed one following the
 * pointer.
 *
 * Presentational only. It owns no geometry and no pointer logic — the
 * canvas renders it and feeds it the drag state, because the pointer
 * handlers have to sit on the `<svg>` element itself (pointer capture
 * needs the element that receives the first `pointerdown`). Everything
 * testable — which vertex a point grabs, when a drag becomes an edit —
 * lives in the pure modules under `lib/editor/`.
 */

export interface NodeToolProps {
  shapes: Shapes;
  /** Grab-dot radius in millimetres, derived from pixels at the current scale. */
  radiusMm: number;
  /** The vertex being dragged, or nothing when no drag is active. */
  grabbed: VertexRef | null;
  /** Where the pointer is, in millimetres, while a drag is active. */
  pointerMm: Point2D | null;
}

export function NodeTool({ shapes, radiusMm, grabbed, pointerMm }: NodeToolProps) {
  // The grabbed vertex follows the pointer while the committed geometry
  // catches up, so the dot under the finger does not lag a frame behind.
  const shown =
    grabbed && pointerMm
      ? verticesOf(shapes).map((v) =>
          v.shape === grabbed.shape &&
          v.contour === grabbed.contour &&
          v.vertex === grabbed.vertex
            ? { ...v, at: pointerMm }
            : v
        )
      : verticesOf(shapes);

  return (
    <g className="node-tool" data-testid="node-tool" pointerEvents="none">
      {shown.map((v) => {
        const held =
          grabbed !== null &&
          v.shape === grabbed.shape &&
          v.contour === grabbed.contour &&
          v.vertex === grabbed.vertex;
        return (
          <circle
            key={`${v.shape}-${v.contour}-${v.vertex}`}
            cx={v.at.x}
            cy={v.at.y}
            // Bigger as well as red. A grab that is only a colour change is easy
            // to miss on a mask with two dozen dots, and a dot that grows under
            // the cursor says "this one, and it is yours" without reading
            // anything.
            r={held ? radiusMm * 1.6 : radiusMm}
            fill={held ? "#c41820" : "#0a7"}
            data-grabbed={held ? "true" : undefined}
          />
        );
      })}
    </g>
  );
}
