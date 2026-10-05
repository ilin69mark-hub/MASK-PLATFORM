//! Measurements. Every manufacturing decision rests on these numbers, so they
//! are computed from canonical geometry and reported at a defined precision.

use crate::constants::{report, MIN_WIDTH_EPS_MM};
use crate::geom::{Contour, Shape, Shapes};

/// Signed area of one contour via the shoelace formula.
///
/// Sign carries meaning: counter-clockwise is positive, clockwise negative.
/// Taking `abs()` here would *add* holes instead of subtracting them. ADR-0003
/// finding N-6 — this is easy to get wrong because it looks correct on
/// single-contour figures.
#[must_use]
pub fn signed_contour_area(contour: &Contour) -> f64 {
    let pts = &contour.points;
    if pts.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        acc += a.x * b.y - b.x * a.y;
    }
    acc / 2.0
}

/// Area of a shape with its holes subtracted.
///
/// Correct only because orientation is an invariant: outer CCW, holes CW.
#[must_use]
pub fn shape_area(shape: &Shape) -> f64 {
    shape.contours().map(signed_contour_area).sum()
}

/// Total area of a shape set.
#[must_use]
pub fn shapes_area(shapes: &Shapes) -> f64 {
    shapes.shapes.iter().map(shape_area).sum()
}

/// Perimeter of a contour. Open contours omit the closing segment.
#[must_use]
pub fn contour_perimeter(contour: &Contour) -> f64 {
    let pts = &contour.points;
    if pts.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..pts.len() - 1 {
        total += pts[i].distance_to(pts[i + 1]);
    }
    if contour.closed && pts.len() > 2 {
        total += pts[pts.len() - 1].distance_to(pts[0]);
    }
    total
}

/// Axis-aligned bounding box as `(min_x, min_y, width, height)`.
#[must_use]
pub fn bounding_box(shapes: &Shapes) -> (f64, f64, f64, f64) {
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for contour in shapes.shapes.iter().flat_map(|s| s.contours()) {
        for p in &contour.points {
            min_x = min_x.min(p.x);
            min_y = min_y.min(p.y);
            max_x = max_x.max(p.x);
            max_y = max_y.max(p.y);
        }
    }

    if !min_x.is_finite() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

/// Number of distinct connected regions, i.e. islands within one mask layer.
///
/// A region whose area falls below `min_island_mm2` is still counted here;
/// filtering it out is the Topology Engine's decision, not the counter's.
#[must_use]
pub fn island_count(shapes: &Shapes) -> usize {
    shapes.shapes.len()
}

/// Number of holes across all regions.
#[must_use]
pub fn hole_count(shapes: &Shapes) -> usize {
    shapes.shapes.iter().map(|s| s.holes.len()).sum()
}

/// Compare a measurement against a profile threshold with the defined band.
///
/// Returns true when `measured >= threshold - MIN_WIDTH_EPS_MM`. The band is
/// what keeps a boundary verdict stable instead of floating-point dependent.
#[must_use]
pub fn meets_threshold(measured: f64, threshold: f64) -> bool {
    measured >= threshold - MIN_WIDTH_EPS_MM
}

/// Convenience for diagnostics: "measured (required)" at report precision.
#[must_use]
pub fn format_measurement(measured: f64, required: f64) -> String {
    format!(
        "{:.3} mm (минимум {:.3} mm)",
        report(measured),
        report(required)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::MEASURE_EPS_MM;
    use crate::geom::Point;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn ccw_rect(x: f64, y: f64, w: f64, h: f64) -> Contour {
        // Counter-clockwise: positive signed area.
        Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
            closed: true,
        }
    }

    fn cw_rect(x: f64, y: f64, w: f64, h: f64) -> Contour {
        Contour {
            points: vec![p(x, y), p(x, y + h), p(x + w, y + h), p(x + w, y)],
            closed: true,
        }
    }

    #[test]
    fn outer_contour_area_is_positive_and_holes_negative() {
        let outer = signed_contour_area(&ccw_rect(0.0, 0.0, 10.0, 10.0));
        let hole = signed_contour_area(&cw_rect(2.0, 2.0, 6.0, 6.0));
        assert!((outer - 100.0).abs() < MEASURE_EPS_MM);
        assert!((hole + 36.0).abs() < MEASURE_EPS_MM);
    }

    #[test]
    fn shape_area_subtracts_holes() {
        // The N-6 regression: absolute values would give 136 instead of 64.
        let shape = Shape::with_holes(
            ccw_rect(0.0, 0.0, 10.0, 10.0),
            vec![cw_rect(2.0, 2.0, 6.0, 6.0)],
        );
        assert!((shape_area(&shape) - 64.0).abs() < MEASURE_EPS_MM);
    }

    #[test]
    fn degenerate_contour_has_zero_area() {
        assert!(
            signed_contour_area(&Contour {
                points: vec![p(0.0, 0.0), p(1.0, 1.0)],
                closed: false
            })
            .abs()
                < MEASURE_EPS_MM
        );
    }

    #[test]
    fn bounding_box_spans_all_contours() {
        let shapes = Shapes::single(Shape::with_holes(
            ccw_rect(0.0, 0.0, 20.0, 10.0),
            vec![cw_rect(5.0, 5.0, 2.0, 2.0)],
        ));
        assert_eq!(bounding_box(&shapes), (0.0, 0.0, 20.0, 10.0));
    }

    #[test]
    fn empty_shapes_have_a_finite_box() {
        assert_eq!(bounding_box(&Shapes::new()), (0.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn perimeter_of_closed_rect_is_twice_the_sum_of_sides() {
        assert!((contour_perimeter(&ccw_rect(0.0, 0.0, 10.0, 5.0)) - 30.0).abs() < MEASURE_EPS_MM);
    }

    #[test]
    fn threshold_band_absorbs_sub_limit_epsilon() {
        // Inside the 1e-3 band: 0.7995 >= 0.80 - 0.001.
        assert!(meets_threshold(0.7995, 0.80));
        // Exactly on the band edge.
        assert!(meets_threshold(0.7990, 0.80));
        // Clearly outside it.
        assert!(!meets_threshold(0.7980, 0.80));
    }

    #[test]
    fn measurement_message_uses_report_precision() {
        assert_eq!(
            format_measurement(0.42, 0.80),
            "0.420 mm (минимум 0.800 mm)"
        );
    }
}
