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

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn ring(x: f64, y: f64, w: f64, h: f64) -> Contour {
        Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
            closed: true,
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
                closed: true
            }),
            None
        );
        assert_eq!(
            shortest_edge(&Contour {
                points: vec![Point { x: 1.0, y: 1.0 }],
                closed: true
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
                }),
            ],
        };
        let shortest = shortest_edge_in(&shapes).expect("edges");
        assert!(
            (shortest - 0.2).abs() < 1e-9,
            "expected the 0.2 mm edge, got {shortest:.4}"
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
