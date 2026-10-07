/**
 * Millimetres ↔ screen pixels for the studio canvas.
 *
 * Pure, and deliberately so: there is no DOM environment in this project's tests —
 * `react-dom/server` renders markup and nothing more — so anything that needs a
 * live `SVGSVGElement` to be correct cannot be tested at all. The conversion is
 * therefore written against the two numbers it actually needs (the viewBox and the
 * drawn size) and the element only supplies them.
 *
 * The SVG user unit is one millimetre — the `viewBox` is in mm and strokes are
 * given in mm — so one scale factor covers the whole mapping.
 */

/** The rectangle of the drawing, in millimetres, as `viewBox` spells it. */
export interface ViewBoxMm {
  minX: number;
  minY: number;
  w: number;
  h: number;
}

/** How many CSS pixels the viewBox is actually drawn into. */
export interface DrawnSizePx {
  width: number;
  height: number;
}

/** A point in the drawing's coordinate system (millimetres) or in pixels. */
export interface Point2D {
  x: number;
  y: number;
}

/**
 * Millimetres per pixel.
 *
 * `preserveAspectRatio="…meet"` letterboxes rather than stretching, so both axes
 * share one factor and the smaller is the one that fits the whole viewBox. Taking
 * the larger is the tempting mistake: everything on the short axis then maps
 * outside the box, and a hit-test silently stops selecting anything near an edge.
 *
 * Falls back to 1 for a box that has not been laid out yet, so a caller with no
 * measured size gets a usable identity mapping instead of a division by zero.
 */
export function mmPerPixel(view: ViewBoxMm, drawn: DrawnSizePx): number {
  if (drawn.width <= 0 || drawn.height <= 0) return 1;
  return Math.min(view.w / drawn.width, view.h / drawn.height);
}

/** Pixel position inside the drawing box → millimetres. */
export function pxToMm(px: Point2D, view: ViewBoxMm, drawn: DrawnSizePx): Point2D {
  const scale = mmPerPixel(view, drawn);
  return { x: view.minX + px.x * scale, y: view.minY + px.y * scale };
}

/** Millimetres → pixel position inside the drawing box. */
export function mmToPx(mm: Point2D, view: ViewBoxMm, drawn: DrawnSizePx): Point2D {
  const scale = mmPerPixel(view, drawn);
  return { x: (mm.x - view.minX) / scale, y: (mm.y - view.minY) / scale };
}

/**
 * A grab radius in millimetres, derived from a radius in pixels.
 *
 * A pixel tolerance is what the user actually aims with — they see pixels — while
 * a millimetre tolerance is what the geometry is measured in. On a 200 mm part
 * drawn 600 px wide one pixel is a third of a millimetre, so a fixed millimetre
 * radius would be unhittable; on a 5 mm detail drawn 600 px wide one pixel is
 * eight microns and a millimetre radius would swallow the whole shape.
 */
export function pxRadiusToMm(radiusPx: number, view: ViewBoxMm, drawn: DrawnSizePx): number {
  return radiusPx * mmPerPixel(view, drawn);
}