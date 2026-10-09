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
 * The rectangle a measurement is drawn in: its bounding box plus a margin.
 *
 * Pure and exported because it decides where every dot lands and where every
 * pointer is converted back to — the two must be the same rectangle, or the dots
 * and the hit test disagree. It was inline in the canvas, where nothing could
 * test it.
 */
export function viewFor(bbox: [number, number, number, number] | null): ViewBoxMm {
  if (!bbox) {
    // Nothing measured yet: a 100 mm box with the same padding, so the first
    // render draws somewhere sane instead of dividing by zero.
    const fallback = 100;
    const pad = fallback * 0.04;
    return {
      minX: -pad,
      minY: -pad,
      w: fallback + pad * 2,
      h: fallback + pad * 2,
    };
  }
  const [minX, minY, w, h] = bbox;
  // Padding in millimetres, so the margin looks the same at any zoom.
  const pad = Math.max(w, h) * 0.04 || 5;
  return {
    minX: minX - pad,
    minY: minY - pad,
    w: w + pad * 2,
    h: h + pad * 2,
  };
}

/** How the viewBox sits inside the element that draws it. */
export interface Letterbox {
  /** Millimetres per pixel; one factor for both axes. */
  scale: number;
  /** Pixels from the element's left edge to the viewBox's left edge. */
  offsetX: number;
  /** Pixels from the element's top edge to the viewBox's top edge. */
  offsetY: number;
}

/**
 * The scale and the letterbox offsets, together.
 *
 * `preserveAspectRatio="…meet"` letterboxes rather than stretching: both axes
 * share one factor (the smaller is the one that fits the whole viewBox) and the
 * content is centred, leaving a margin on the other axis. Taking the larger
 * factor is the tempting mistake: everything on the short axis then maps
 * outside the box, and a hit-test silently stops selecting anything near an
 * edge. Ignoring the offset is the subtler one: on a wide viewBox in a tall
 * box, pixel 0 is not the viewBox's `minX` but a point left of it, so every
 * hit-test is off by half the margin.
 *
 * Falls back to an identity mapping for a box that has not been laid out yet,
 * so a caller with no measured size gets usable numbers instead of a division
 * by zero.
 */
export function letterbox(view: ViewBoxMm, drawn: DrawnSizePx): Letterbox {
  if (drawn.width <= 0 || drawn.height <= 0) {
    return { scale: 1, offsetX: 0, offsetY: 0 };
  }
  const scale = Math.min(view.w / drawn.width, view.h / drawn.height);
  const contentW = view.w / scale;
  const contentH = view.h / scale;
  return {
    scale,
    offsetX: (drawn.width - contentW) / 2,
    offsetY: (drawn.height - contentH) / 2,
  };
}

/** Millimetres per pixel. */
export function mmPerPixel(view: ViewBoxMm, drawn: DrawnSizePx): number {
  return letterbox(view, drawn).scale;
}

/** Pixel position inside the drawing box → millimetres. */
export function pxToMm(px: Point2D, view: ViewBoxMm, drawn: DrawnSizePx): Point2D {
  const { scale, offsetX, offsetY } = letterbox(view, drawn);
  return {
    x: view.minX + (px.x - offsetX) * scale,
    y: view.minY + (px.y - offsetY) * scale,
  };
}

/** Millimetres → pixel position inside the drawing box. */
export function mmToPx(mm: Point2D, view: ViewBoxMm, drawn: DrawnSizePx): Point2D {
  const { scale, offsetX, offsetY } = letterbox(view, drawn);
  return {
    x: (mm.x - view.minX) / scale + offsetX,
    y: (mm.y - view.minY) / scale + offsetY,
  };
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
