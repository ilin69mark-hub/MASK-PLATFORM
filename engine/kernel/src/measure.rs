//! Measurements. Every manufacturing decision rests on these numbers, so they
//! are computed from canonical geometry and reported at a defined precision.

use crate::constants::{report, MIN_WIDTH_EPS_MM};
use crate::geom::{Contour, Point, Shape, Shapes};

/// Signed area of one contour via the shoelace formula.
///
/// Sign carries meaning: counter-clockwise is positive, clockwise negative.
/// Taking `abs()` here would *add* holes instead of subtracting them. ADR-0003
/// finding N-6 — this is easy to get wrong because it looks correct on
/// single-contour figures.
#[must_use]
pub fn signed_contour_area(contour: &Contour) -> f64 {
    let pts = &contour.points;
    // Two points and one arc is a legitimate lens: the shoelace term over two
    // points is zero and the arc contributes the whole area. Guarding at three
    // points, as this did, silently reported a lens as having no area at all.
    if pts.len() < 2 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..pts.len() {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        acc += a.x * b.y - b.x * a.y;
    }
    let chordal = acc / 2.0;

    // Every arc bulges away from its chord, and the difference is the circular
    // segment. Without this a circle of radius 10 reports the area of its inscribed
    // square: 200 instead of 314.159, a silent third off.
    //
    // Accumulated separately from the shoelace term on purpose: `acc` above holds
    // the *doubled* area and only becomes an area when divided by two, while a
    // segment is already an area. Adding the segment into the same accumulator and
    // halving once at the end silently reported 257.08 for a 314.16 circle.
    let segments: f64 = contour
        .resolved_arcs()
        .iter()
        .map(|(arc, from, to)| crate::geom::arc_segment_area(arc, *from, *to))
        .sum();

    chordal + segments
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

/// The shortest edge of a contour, in millimetres.
///
/// This is what T-10 means for polygonal geometry.
///
/// The docs state the rule as `path.length >= min_feature_length_mm`, and the
/// auto-simplify step says "drop short paths". In this geometry model there are
/// no open paths — every contour is a closed loop — so the honest measurable
/// proxy is the shortest edge of that loop.
///
/// It is the right thing to measure, not merely the available one. A knife blade
/// has an offset and an angle, so it cannot turn tightly; an edge shorter than
/// the blade offset is not cut as drawn, it is rounded off by the tool. Those
/// edges are noise from tracing and from floating-point slivers, and the
/// auto-simplify step exists to remove them.
///
/// Returns `None` for a contour with fewer than two points, which has no edges.
#[must_use]
pub fn shortest_edge(contour: &Contour) -> Option<f64> {
    let pts = &contour.points;
    if pts.len() < 2 {
        return None;
    }
    let n = pts.len();
    let last = if contour.closed { n } else { n - 1 };
    let mut shortest = f64::INFINITY;
    for i in 0..last {
        shortest = shortest.min(pts[i].distance_to(pts[(i + 1) % n]));
    }
    shortest.is_finite().then_some(shortest)
}

/// The shortest edge anywhere in a set of shapes.
#[must_use]
pub fn shortest_edge_in(shapes: &Shapes) -> Option<f64> {
    shapes
        .shapes
        .iter()
        .flat_map(|s| s.contours())
        .filter_map(shortest_edge)
        .fold(None, |acc: Option<f64>, e| {
            Some(acc.map_or(e, |acc: f64| acc.min(e)))
        })
}

/// Perimeter of a contour. Open contours omit the closing segment.
#[must_use]
pub fn contour_perimeter(contour: &Contour) -> f64 {
    let pts = &contour.points;
    if pts.len() < 2 {
        return 0.0;
    }
    let n = pts.len();
    let mut total = 0.0;
    for i in 0..n.saturating_sub(1) {
        total += edge_length(contour, i, pts[i], pts[i + 1]);
    }
    if contour.closed && n > 2 {
        total += edge_length(contour, n - 1, pts[n - 1], pts[0]);
    }
    total
}

/// Length of one edge: the arc length where there is an arc, the chord otherwise.
fn edge_length(contour: &Contour, edge: usize, from: Point, to: Point) -> f64 {
    contour
        .resolved_arcs()
        .iter()
        .find(|(arc, _, _)| arc.from == edge)
        .map_or_else(
            || from.distance_to(to),
            |(arc, a, b)| crate::geom::arc_length(arc, *a, *b),
        )
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

/// Bounding box of one shape as `(min_x, min_y, width, height)`.
#[must_use]
pub fn shape_bounds(shape: &Shape) -> (f64, f64, f64, f64) {
    bounding_box(&Shapes::single(shape.clone()))
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

/// The same shape of message, in square millimetres.
///
/// Separate from the length version for the same reason as the degrees one: an
/// island area printed with "мм" is a unit error in the line the operator reads.
#[must_use]
pub fn format_area(measured: f64, required: f64) -> String {
    format!("{measured:.3} мм² (минимум {required:.3} мм²)")
}

/// The same shape of message, in degrees.
///
/// A separate function rather than a unit parameter on the millimetre one,
/// because getting the unit wrong is exactly the failure this guards against:
/// `format_measurement` on an angle printed "18.925 mm" and told the operator
/// that an angle was a length.
#[must_use]
pub fn format_degrees(measured: f64, required: f64) -> String {
    format!("{measured:.1}° (минимум {required:.1}°)")
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
            arcs: Vec::new(),
        }
    }

    fn cw_rect(x: f64, y: f64, w: f64, h: f64) -> Contour {
        Contour {
            points: vec![p(x, y), p(x, y + h), p(x + w, y + h), p(x + w, y)],
            closed: true,
            arcs: Vec::new(),
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
                closed: false,
                arcs: Vec::new(),
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

/// The narrowest curve anywhere in a set of shapes.
///
/// This is the measurement T-05 asks for, and it returns `None` for polygonal
/// geometry — correctly, not as a gap. A polyline has no radius: its corners are
/// infinitely sharp, so "minimum corner radius" would be zero everywhere and the
/// check would fail on every shape for a reason that means nothing.
///
/// Once a contour carries arcs, the number exists, and this returns it.
#[must_use]
pub fn min_corner_radius(shapes: &Shapes) -> Option<f64> {
    shapes
        .shapes
        .iter()
        .flat_map(|s| s.contours())
        .filter_map(Contour::min_arc_radius)
        .fold(None, |acc: Option<f64>, r| {
            Some(acc.map_or(r, |acc: f64| acc.min(r)))
        })
}

/// Whether any contour in the set carries a curve.
///
/// Lets the validator say "T-05 does not apply here" as a fact about the geometry
/// rather than an assumption about it.
#[must_use]
pub fn has_curves(shapes: &Shapes) -> bool {
    shapes
        .shapes
        .iter()
        .any(|s| s.contours().any(Contour::has_arcs))
}

/// Ray-casting point-in-contour test.
///
/// Used to decide whether a region sits inside another region's hole, which is
/// the only situation that requires a bridge (see `unanchored_islands`).
#[must_use]
pub fn point_in_contour(point: Point, contour: &Contour) -> bool {
    let pts = &contour.points;
    if pts.len() < 3 {
        return false;
    }

    // Boundary is treated as inside. The ray test below cannot express this
    // reliably — it is asymmetric between the left and right edge — so the
    // boundary is decided explicitly first.
    let segments = if contour.closed {
        pts.len()
    } else {
        pts.len() - 1
    };
    for i in 0..segments {
        let a = pts[i];
        let b = pts[(i + 1) % pts.len()];
        if point_on_segment(point, a, b, crate::constants::GEOM_EPS_MM) {
            return true;
        }
    }

    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (pi, pj) = (pts[i], pts[j]);
        if (pi.y > point.y) != (pj.y > point.y) {
            let t = (point.y - pi.y) / (pj.y - pi.y);
            if point.x < pi.x + t * (pj.x - pi.x) {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Whether `p` lies on segment `a..b` within `tolerance`.
#[must_use]
pub fn point_on_segment(p: Point, a: Point, b: Point, tolerance: f64) -> bool {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_sq = dx * dx + dy * dy;
    if length_sq <= tolerance * tolerance {
        return p.distance_to(a) <= tolerance;
    }
    // Project and clamp to the segment.
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length_sq).clamp(0.0, 1.0);
    let projected = Point {
        x: a.x + t * dx,
        y: a.y + t * dy,
    };
    p.distance_to(projected) <= tolerance
}

/// Regions that sit inside a hole of another region and therefore cannot be
/// applied as a separate piece.
///
/// Disjoint regions are *not* islands: a mask layer cut from one sheet yields
/// separate pieces of film, and each is applied on its own, so no bridge is
/// needed. A region inside a hole is different — the outer piece is laid down
/// and painted while the inner piece is still nested in it, so the inner piece
/// must be held by a bridge or it drops out.
///
/// Returns the indices of the islands, in input order.
#[must_use]
pub fn unanchored_islands(shapes: &Shapes) -> Vec<usize> {
    let mut islands = Vec::new();

    for (i, candidate) in shapes.shapes.iter().enumerate() {
        // Sample the candidate rather than testing one arbitrary vertex: a
        // region straddling a hole boundary is partly unanchored either way.
        let probes = sample_points(candidate, 8);
        for other in shapes.shapes.iter() {
            if other.outer.points == candidate.outer.points {
                continue;
            }
            for hole in &other.holes {
                if probes.iter().any(|p| point_in_contour(*p, hole)) {
                    islands.push(i);
                    break;
                }
            }
        }
    }

    islands.sort_unstable();
    islands.dedup();
    islands
}

/// A few interior points of a region, used for containment probing.
fn sample_points(shape: &Shape, count: usize) -> Vec<Point> {
    let (_, _, w, h) = crate::measure::shape_bounds(shape);
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let (min_x, min_y, _, _) = crate::measure::shape_bounds(shape);
    let n = count as f64;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        // Spread probes over a coprime stride so they do not land on a grid.
        let fx = ((i as f64) + 0.5) / n;
        let fy = (((i * 3) % count) as f64 + 0.5) / n;
        out.push(Point {
            x: min_x + fx * w,
            y: min_y + fy * h,
        });
    }
    out
}

#[cfg(test)]
mod island_tests {
    use super::*;
    use crate::geom::ArcSegment;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn ring(x: f64, y: f64, w: f64, h: f64) -> Contour {
        Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
            closed: true,
            arcs: Vec::new(),
        }
    }

    #[test]
    fn the_shortest_edge_is_found_including_the_closing_one() {
        // The closing edge is where a short edge usually hides: it is the segment
        // from the last point back to the first, and skipping it is the easy
        // omission.
        let contour = Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
                Point { x: 0.4, y: 0.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        };
        // The closing edge is from (0.4, 0) back to (0, 0): 0.4 mm.
        let shortest = shortest_edge(&contour).expect("edges");
        assert!(
            (shortest - 0.4).abs() < 1e-9,
            "expected the 0.4 mm closing edge, got {shortest:.4}"
        );
    }

    #[test]
    fn an_open_contour_has_no_closing_edge() {
        let contour = Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
            ],
            closed: false,
            arcs: Vec::new(),
        };
        let shortest = shortest_edge(&contour).expect("edges");
        assert!(
            (shortest - 10.0).abs() < 1e-9,
            "expected 10 mm, got {shortest:.4}"
        );
    }

    #[test]
    fn a_degenerate_contour_has_no_edges() {
        assert_eq!(
            shortest_edge(&Contour {
                points: vec![],
                closed: true,
                arcs: Vec::new(),
            }),
            None
        );
        assert_eq!(
            shortest_edge(&Contour {
                points: vec![Point { x: 1.0, y: 1.0 }],
                closed: true,
                arcs: Vec::new(),
            }),
            None
        );
    }

    #[test]
    fn the_shortest_edge_across_a_layer_is_the_smallest_of_all_of_them() {
        let shapes = Shapes {
            shapes: vec![
                Shape::new(Contour {
                    points: vec![
                        Point { x: 0.0, y: 0.0 },
                        Point { x: 10.0, y: 0.0 },
                        Point { x: 10.0, y: 10.0 },
                        Point { x: 0.0, y: 10.0 },
                    ],
                    closed: true,
                    arcs: Vec::new(),
                }),
                Shape::new(Contour {
                    points: vec![
                        Point { x: 50.0, y: 0.0 },
                        Point { x: 60.0, y: 0.0 },
                        Point { x: 60.0, y: 10.0 },
                        Point { x: 50.0, y: 10.0 },
                        Point { x: 50.2, y: 10.0 },
                    ],
                    closed: true,
                    arcs: Vec::new(),
                }),
            ],
        };
        let shortest = shortest_edge_in(&shapes).expect("edges");
        assert!(
            (shortest - 0.2).abs() < 1e-9,
            "expected the 0.2 mm edge, got {shortest:.4}"
        );
    }

    /// A circle of the given radius as four points and four quarter arcs.
    fn circle(radius: f64) -> Contour {
        let points: Vec<Point> = (0..4)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (i as f64);
                Point {
                    x: radius * a.cos(),
                    y: radius * a.sin(),
                }
            })
            .collect();
        let arcs = (0..4)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: Point { x: 0.0, y: 0.0 },
                radius,
                ccw: true,
            })
            .collect();
        Contour {
            points,
            closed: true,
            arcs,
        }
    }

    #[test]
    fn a_circle_of_arcs_measures_exactly_its_true_area() {
        // The bug this fixes: the chordal polygon of a circle of radius 10 is a
        // square of area 200, and reporting that for a cut of area 314 is a silent
        // third off — the same class of wrong number as the hole-winding bug.
        let contour = circle(10.0);
        let shapes = Shapes {
            shapes: vec![Shape::new(contour)],
        };
        let area = shapes_area(&shapes);
        let exact = std::f64::consts::PI * 100.0;
        assert!(
            (area - exact).abs() < 1e-9,
            "a circle should measure exactly, got {area} against {exact}"
        );
    }

    #[test]
    fn a_circle_of_arcs_measures_exactly_its_perimeter() {
        let contour = circle(10.0);
        let perimeter = contour_perimeter(&contour);
        let exact = 2.0 * std::f64::consts::PI * 10.0;
        assert!(
            (perimeter - exact).abs() < 1e-9,
            "got {perimeter} against {exact}"
        );
    }

    #[test]
    fn the_same_points_straight_and_curved_measure_differently() {
        // Both readings are correct for their own geometry. That they differ is the
        // point: a curve is not its chord, and a measurement that cannot tell them
        // apart is measuring the wrong thing.
        let curved = circle(10.0);
        let straight = Contour {
            points: curved.points.clone(),
            closed: true,
            arcs: Vec::new(),
        };
        let curved_area = shapes_area(&Shapes {
            shapes: vec![Shape::new(curved.clone())],
        });
        let straight_area = shapes_area(&Shapes {
            shapes: vec![Shape::new(straight.clone())],
        });
        assert!(
            (curved_area - straight_area).abs() > 100.0,
            "a curve and its chord polygon must not measure the same: {curved_area} vs {straight_area}"
        );
        assert!((contour_perimeter(&curved) - contour_perimeter(&straight)).abs() > 5.0);
    }

    #[test]
    fn a_semicircle_arc_measures_its_segment_area() {
        // Two points and one arc sweeping half a turn. The chord here is the
        // diameter, so the segment between chord and arc *is* the half disc —
        // 157.08, not "half disc minus triangle". The triangle only appears for a
        // chord that is not a diameter, and subtracting 100 was simply wrong.
        let contour = Contour {
            points: vec![Point { x: 10.0, y: 0.0 }, Point { x: -10.0, y: 0.0 }],
            arcs: vec![ArcSegment {
                from: 0,
                to: 1,
                centre: Point { x: 0.0, y: 0.0 },
                radius: 10.0,
                ccw: true,
            }],
            closed: true,
        };
        let area = signed_contour_area(&contour).abs();
        let expected = std::f64::consts::PI * 100.0 / 2.0;
        assert!(
            (area - expected).abs() < 1e-9,
            "got {area} against {expected}"
        );
    }

    #[test]
    fn an_arc_sweeping_the_wrong_way_is_a_notch_not_film() {
        let ccw = Contour {
            points: vec![Point { x: 10.0, y: 0.0 }, Point { x: -10.0, y: 0.0 }],
            arcs: vec![ArcSegment {
                from: 0,
                to: 1,
                centre: Point { x: 0.0, y: 0.0 },
                radius: 10.0,
                ccw: true,
            }],
            closed: true,
        };
        let cw = Contour {
            points: ccw.points.clone(),
            closed: true,
            arcs: vec![ArcSegment {
                from: 0,
                to: 1,
                centre: Point { x: 0.0, y: 0.0 },
                radius: 10.0,
                ccw: false,
            }],
        };
        // Reversing the direction of travel puts the film on the other side of the
        // chord, so the two areas are equal in size and opposite in sign. Both are
        // segments; neither is the same shape as the other.
        let (a, b) = (signed_contour_area(&ccw), signed_contour_area(&cw));
        assert!((a + b).abs() < 1e-9, "mirrors should cancel: {a} and {b}");
        assert!(
            a.abs() > 100.0,
            "and both should be substantial: {a} and {b}"
        );
        assert!(a * b < 0.0, "with opposite signs: {a} and {b}");
    }

    #[test]
    fn an_arc_wider_than_a_half_turn_is_measured_from_its_own_sweep() {
        // Three points so the long way round is actually determined: from
        // (10, 0) clockwise to (0, 10) passes through (7.07, -7.07) and sweeps
        // 270°, not the 90° the other way. Taking the short way would report a
        // twelfth of the real shape.
        //
        // With only two points the geometry is genuinely ambiguous — nothing says
        // which way round — so this needs the third point to be a real test.
        //
        // This tests `arc_sweep`, not a valid pipeline contour: the arc names
        // non-consecutive points, so `Contour::validate` must reject it under
        // ADR-0017. The distinction is intentional — pure angle arithmetic does not
        // certify a contour — and this test pins both halves rather than letting
        // the sweep test read as contour approval.
        let contour = Contour {
            points: vec![
                Point { x: 10.0, y: 0.0 },
                Point {
                    x: 7.0710678,
                    y: -7.0710678,
                },
                Point { x: 0.0, y: 10.0 },
            ],
            arcs: vec![ArcSegment {
                from: 0,
                to: 2,
                centre: Point { x: 0.0, y: 0.0 },
                radius: 10.0,
                ccw: false,
            }],
            closed: true,
        };
        assert!(
            matches!(
                contour.validate(crate::constants::MAX_COORDINATE_MM),
                Err(crate::geom::GeometryError::ArcIndex { .. })
            ),
            "a multi-edge run is mathematically sweepable but not pipeline-valid"
        );
        let sweep = crate::geom::arc_sweep(&contour.arcs[0], contour.points[0], contour.points[2]);
        assert!(
            (sweep + 3.0 * std::f64::consts::FRAC_PI_2).abs() < 1e-4,
            "sweep was {sweep}"
        );
    }

    #[test]
    fn two_points_and_one_arc_is_a_lens_not_nothing() {
        // Pinned because the guard that rejected it looked obviously right: a
        // polygon needs three points, and nobody was looking at a two-point
        // contour holding an arc.
        let contour = Contour {
            points: vec![Point { x: 10.0, y: 0.0 }, Point { x: -10.0, y: 0.0 }],
            arcs: vec![ArcSegment {
                from: 0,
                to: 1,
                centre: Point { x: 0.0, y: 0.0 },
                radius: 10.0,
                ccw: true,
            }],
            closed: true,
        };
        let area = signed_contour_area(&contour).abs();
        // The chord is the diameter, so the segment is the half disc.
        let expected = std::f64::consts::PI * 100.0 / 2.0;
        assert!(
            (area - expected).abs() < 1e-9,
            "got {area} against {expected}"
        );
    }

    #[test]
    fn angles_are_formatted_in_degrees_not_millimetres() {
        let text = format_degrees(18.9247, 30.0);
        assert!(text.contains('°'), "degrees must be marked: {text}");
        assert!(!text.contains("mm"), "an angle is not a length: {text}");
        assert!(text.contains("18.9"), "{text}");
        assert!(text.contains("30.0"), "{text}");
    }

    #[test]
    fn point_inside_a_ring_is_detected() {
        assert!(point_in_contour(p(5.0, 5.0), &ring(0.0, 0.0, 10.0, 10.0)));
        assert!(!point_in_contour(p(15.0, 5.0), &ring(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn a_point_exactly_on_the_boundary_counts_as_inside() {
        // Deliberate, and asymmetric on purpose. A point on the edge is
        // ambiguous for ray casting, and the two ways to resolve it are not
        // equivalent in cost:
        //
        //   inside  -> an extra bridge is suggested. Cost: a little material.
        //   outside -> a real island goes unnoticed, it falls off during peel,
        //              and it takes the paint with it.
        //
        // For a safety rule the cheap false positive is the correct bias.
        assert!(point_in_contour(p(0.0, 5.0), &ring(0.0, 0.0, 10.0, 10.0)));
        assert!(point_in_contour(p(10.0, 5.0), &ring(0.0, 0.0, 10.0, 10.0)));
    }

    #[test]
    fn a_region_inside_a_hole_is_an_unanchored_island() {
        let outer = Shape::with_holes(ring(0.0, 0.0, 20.0, 20.0), vec![ring(5.0, 5.0, 10.0, 10.0)]);
        let inner = Shape::new(ring(8.0, 8.0, 4.0, 4.0));
        let shapes = Shapes {
            shapes: vec![outer, inner],
        };
        assert_eq!(unanchored_islands(&shapes), vec![1]);
    }

    #[test]
    fn disjoint_regions_need_no_bridges() {
        // Two separate pieces of film are applied one after another, so no
        // bridge is required and reporting one would be a false positive.
        let shapes = Shapes {
            shapes: vec![
                Shape::new(ring(0.0, 0.0, 5.0, 5.0)),
                Shape::new(ring(50.0, 0.0, 5.0, 5.0)),
            ],
        };
        assert!(unanchored_islands(&shapes).is_empty());
    }

    #[test]
    fn a_bridged_island_leaves_no_hole_and_is_clean() {
        // Union of the ring, its hole and the inner patch removes the hole, so
        // the island disappears. This is why the CLI unions per layer.
        let mut shapes = Shapes {
            shapes: vec![Shape::with_holes(
                ring(0.0, 0.0, 20.0, 20.0),
                vec![ring(5.0, 5.0, 10.0, 10.0)],
            )],
        };
        shapes = crate::boolean::union(
            &shapes,
            &Shapes {
                shapes: vec![Shape::new(ring(8.0, 8.0, 4.0, 4.0))],
            },
        )
        .unwrap();
        // With the patch present there is no hole, hence no island.
        assert!(
            shapes.shapes[0].holes.is_empty(),
            "union should close the hole"
        );
        assert!(unanchored_islands(&shapes).is_empty());
    }
}
