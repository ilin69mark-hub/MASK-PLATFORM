//! Minimum gap between separate pieces of a mask layer (T-02).
//!
//! Two regions of the same layer must not come closer than the material's
//! minimum gap, or they merge during weeding: the film between them tears and
//! takes the neighbouring piece with it. This is the same physical requirement
//! as the minimum line width, stated between two regions instead of across one.
//!
//! # Why not the offset method
//!
//! The obvious implementation is the one `offset::min_width` uses: grow one
//! region until it touches the other, bisecting on the delta. It was tried and
//! it is wrong, and the reason is worth recording.
//
//! `min_width` measures *across* a band, where every direction is equivalent.
//! A gap is measured *between* two shapes, where direction is the whole point. An
//! outward offset joins corners with a miter, and a miter extends the corner by
//! `d / sin(theta / 2)`. For a 90° corner that is `d x 1.414` — so growing a
//! square by 8 mm reaches 11.3 mm along its diagonal. A test with two squares 6 mm
//! apart on x and 8 mm on y, a true gap of 10 mm, reported 8.
//!
//! That error is in the dangerous direction: it understates the gap, so a project
//! with a 10 mm gap looks like it has 8. It is also invisible in the axis-aligned
//! case, which is why it survived until a diagonal test was written.
//!
//! So the distance is computed directly: the minimum over segment pairs. Exact,
//! direction-aware, and it needs no bisection at all.

use crate::boolean::intersection;
use crate::geom::{Contour, Point, Shape, Shapes};
use crate::measure::shape_bounds;

/// One measurement: how close two regions come, and which two.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    /// Index of the first region.
    pub a: usize,
    /// Index of the second region.
    pub b: usize,
    /// Distance in millimetres. Zero means the regions touch or overlap.
    pub mm: f64,
}

/// The tightest gap in a layer, and where it is.
///
/// Returns `None` for a layer with fewer than two regions: one piece of film has
/// no neighbour, and reporting a gap against nothing would invent a number.
#[must_use]
pub fn min_gap(shapes: &Shapes) -> Option<Gap> {
    if shapes.shapes.len() < 2 {
        return None;
    }

    let bounds: Vec<(f64, f64, f64, f64)> = shapes.shapes.iter().map(shape_bounds).collect();

    // Search ceiling: the largest extent in the layer. No two regions in it can
    // be further apart than that, so bisection above it is meaningless.
    let ceiling = bounds
        .iter()
        .map(|(x, y, w, h)| x + w + y + h)
        .fold(0.0_f64, f64::max)
        .max(1.0);

    let mut best: Option<Gap> = None;

    for i in 0..shapes.shapes.len() {
        for j in (i + 1)..shapes.shapes.len() {
            let (xi, yi, wi, hi) = bounds[i];
            let (xj, yj, wj, hj) = bounds[j];

            // Cheap rejection by bounding box, refined as the best shrinks.
            // Only exact for axis-aligned boxes on rectangles, which is enough:
            // this prunes work, it never decides the answer.
            let box_gap = bbox_gap((xi, yi, wi, hi), (xj, yj, wj, hj));
            if let Some(current) = best {
                if box_gap >= current.mm {
                    continue;
                }
            }

            if let Some(mm) = pair_gap(&shapes.shapes[i], &shapes.shapes[j], ceiling) {
                let candidate = Gap { a: i, b: j, mm };
                if best.is_none_or(|current| candidate.mm < current.mm) {
                    best = Some(candidate);
                }
            }
        }
    }

    best
}

/// Distance between two regions: the closest approach of their boundaries.
///
/// Returns zero when they overlap or touch, so the caller never has to think
/// about the case separately.
fn pair_gap(a: &Shape, b: &Shape, ceiling: f64) -> Option<f64> {
    // An overlap has no meaningful gap. Checked first because segment distance
    // between two overlapping outlines is positive: the boundaries do cross, but
    // the *region* distance is zero, and weeding a merged pair fails for a
    // different reason than a too-thin gap.
    if intersection(&Shapes::single(a.clone()), &Shapes::single(b.clone()))
        .is_ok_and(|i| !i.is_empty())
    {
        return Some(0.0);
    }

    let mut best = f64::INFINITY;
    for ca in a.contours() {
        for cb in b.contours() {
            let d = contour_distance(ca, cb, best);
            best = best.min(d);
        }
    }

    if best.is_finite() {
        Some(best)
    } else {
        // Two contours with no segments between them. A validated contour has at
        // least three points, so this is unreachable rather than merely unlikely;
        // reporting the ceiling keeps the number finite and therefore comparable.
        Some(ceiling)
    }
}

/// Closest approach of two contours, giving up once `ceiling` is beaten.
fn contour_distance(a: &Contour, b: &Contour, ceiling: f64) -> f64 {
    let segments_a = segments(a);
    let segments_b = segments(b);
    let mut best = f64::INFINITY;

    for (p, q) in &segments_a {
        // Cheap rejection: if the whole segment pair is further than what we
        // already have, skip the projection. Uses squared distances to avoid a
        // square root per test.
        for (r, s) in &segments_b {
            if best < ceiling {
                let box_d2 = segment_box_distance_sq(p, q, r, s);
                if box_d2 >= best * best {
                    continue;
                }
            }
            best = best.min(segment_distance(p, q, r, s));
        }
    }
    best
}

/// A contour as a list of segments. An open contour has no closing segment.
fn segments(contour: &Contour) -> Vec<(Point, Point)> {
    let n = contour.points.len();
    if n < 2 {
        return Vec::new();
    }
    let last = if contour.closed { n } else { n - 1 };
    (0..last)
        .map(|i| (contour.points[i], contour.points[(i + 1) % n]))
        .collect()
}

/// Minimum distance between two segments.
fn segment_distance(p1: &Point, p2: &Point, q1: &Point, q2: &Point) -> f64 {
    if segments_cross(p1, p2, q1, q2) {
        return 0.0;
    }
    // Four candidates: each endpoint to the opposite segment. When the segments do
    // not cross, the closest points are at least one endpoint.
    point_segment_distance(*p1, *q1, *q2)
        .min(point_segment_distance(*p2, *q1, *q2))
        .min(point_segment_distance(*q1, *p1, *p2))
        .min(point_segment_distance(*q2, *p1, *p2))
}

/// Whether two segments properly cross.
///
/// Sharing an endpoint does not count: regions that touch report a zero gap
/// through the endpoint-to-segment terms above, and treating a shared vertex as a
/// crossing would make any two regions that share a lattice point overlap.
fn segments_cross(p1: &Point, p2: &Point, q1: &Point, q2: &Point) -> bool {
    let d1 = cross(*q1, *q2, *p1);
    let d2 = cross(*q1, *q2, *p2);
    let d3 = cross(*p1, *p2, *q1);
    let d4 = cross(*p1, *p2, *q2);

    ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// Distance from a point to a segment, clamped to the segment's extent.
fn point_segment_distance(p: Point, a: Point, b: Point) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len_sq = dx * dx + dy * dy;
    if len_sq <= crate::constants::GEOM_EPS_MM * crate::constants::GEOM_EPS_MM {
        return p.distance_to(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    p.distance_to(Point {
        x: a.x + t * dx,
        y: a.y + t * dy,
    })
}

/// Squared distance between the axis-aligned boxes of two segments.
///
/// A lower bound on the true segment distance, so it is safe to reject a pair on.
fn segment_box_distance_sq(p1: &Point, p2: &Point, q1: &Point, q2: &Point) -> f64 {
    let (px1, px2) = (p1.x.min(p2.x), p1.x.max(p2.x));
    let (py1, py2) = (p1.y.min(p2.y), p1.y.max(p2.y));
    let (qx1, qx2) = (q1.x.min(q2.x), q1.x.max(q2.x));
    let (qy1, qy2) = (q1.y.min(q2.y), q1.y.max(q2.y));

    let dx = (px1 - qx2).max(qx1 - px2).max(0.0);
    let dy = (py1 - qy2).max(qy1 - py2).max(0.0);
    dx * dx + dy * dy
}

/// Lower bound on the distance between two bounding boxes.
///
/// Zero when they overlap. Axis-aligned separation on each axis, combined with
/// the Pythagorean theorem: boxes that overlap on one axis are only separated on
/// the other, which is why this is a sum-then-hypot rather than a sum.
fn bbox_gap(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> f64 {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;

    let dx = (bx - (ax + aw)).max(ax - (bx + bw)).max(0.0);
    let dy = (by - (ay + ah)).max(ay - (by + bh)).max(0.0);

    (dx * dx + dy * dy).sqrt()
}

/// The gap from each region to its nearest neighbour.
///
/// Same measurement as `min_gap`, reported per region, because the validator
/// names the offending element and "somewhere in this layer there is a gap of
/// 0.4 mm" is not actionable.
#[must_use]
pub fn gaps_per_region(shapes: &Shapes) -> Vec<Option<Gap>> {
    let mut out = vec![None; shapes.shapes.len()];
    if shapes.shapes.len() < 2 {
        return out;
    }

    let bounds: Vec<(f64, f64, f64, f64)> = shapes.shapes.iter().map(shape_bounds).collect();
    let ceiling = bounds
        .iter()
        .map(|(x, y, w, h)| x + w + y + h)
        .fold(0.0_f64, f64::max)
        .max(1.0);

    for i in 0..shapes.shapes.len() {
        for j in (i + 1)..shapes.shapes.len() {
            let box_gap = bbox_gap(bounds[i], bounds[j]);
            // Skip only when both regions already have a closer neighbour.
            if out[i].is_some_and(|g| g.mm <= box_gap) && out[j].is_some_and(|g| g.mm <= box_gap) {
                continue;
            }
            let Some(mm) = pair_gap(&shapes.shapes[i], &shapes.shapes[j], ceiling) else {
                continue;
            };
            let candidate = Gap { a: i, b: j, mm };
            if out[i].is_none_or(|g| candidate.mm < g.mm) {
                out[i] = Some(Gap { a: i, b: j, mm });
            }
            if out[j].is_none_or(|g| candidate.mm < g.mm) {
                out[j] = Some(Gap { a: i, b: j, mm });
            }
        }
    }

    out
}

/// Deliberate omissions, kept greppable.
pub const KNOWN_GAPS: &str = "\
does not measure the clearance between a region and its own hole, because that \
distance is filled film by definition and is not a weeding risk; and does not \
measure clearance to the tile edge or to registration marks, which belong to \
nesting and are checked there.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Contour, Point};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Shape {
        Shape::new(Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
            closed: true,
        })
    }

    fn layer(shapes: Vec<Shape>) -> Shapes {
        Shapes { shapes }
    }

    #[test]
    fn a_single_region_has_no_gap() {
        // One piece of film has no neighbour. Reporting a number here would
        // invent a constraint out of nothing.
        assert_eq!(min_gap(&layer(vec![rect(0.0, 0.0, 10.0, 10.0)])), None);
    }

    #[test]
    fn an_empty_layer_has_no_gap() {
        assert_eq!(min_gap(&layer(vec![])), None);
    }

    #[test]
    fn two_rectangles_report_the_space_between_them() {
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(20.0, 0.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(
            (gap.mm - 10.0).abs() < 1e-4,
            "expected 10 mm, got {}",
            gap.mm
        );
        assert_eq!((gap.a, gap.b), (0, 1));
    }

    #[test]
    fn the_gap_measures_edge_to_edge_not_centre_to_centre() {
        // Centres are 20 mm apart; the film is 6 mm apart. Reporting 20 would
        // pass a project that should fail.
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(16.0, 0.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!((gap.mm - 6.0).abs() < 1e-4, "expected 6 mm, got {}", gap.mm);
    }

    #[test]
    fn diagonal_separation_is_measured_as_a_diagonal() {
        // 6 mm on x, 8 mm on y, so the distance is 10 mm.
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(16.0, 18.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(
            (gap.mm - 10.0).abs() < 1e-3,
            "expected 10 mm, got {}",
            gap.mm
        );
    }

    #[test]
    fn touching_regions_report_zero() {
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0, 0.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(gap.mm < 1e-6, "touching film has no gap, got {}", gap.mm);
    }

    #[test]
    fn overlapping_regions_report_zero() {
        let shapes = layer(vec![rect(0.0, 0.0, 10.0, 10.0), rect(5.0, 5.0, 10.0, 10.0)]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(gap.mm < 1e-6, "overlapping film has no gap, got {}", gap.mm);
    }

    #[test]
    fn the_tightest_pair_is_the_one_reported() {
        // Three regions: 0 and 1 are far apart, 1 and 2 are close.
        let shapes = layer(vec![
            rect(0.0, 0.0, 5.0, 5.0),
            rect(40.0, 0.0, 5.0, 5.0),
            rect(50.0, 0.0, 5.0, 5.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!((gap.mm - 5.0).abs() < 1e-4, "expected 5 mm, got {}", gap.mm);
        assert_eq!((gap.a, gap.b), (1, 2), "the closest pair must be named");
    }

    #[test]
    fn each_region_reports_its_own_nearest_neighbour() {
        let shapes = layer(vec![
            rect(0.0, 0.0, 5.0, 5.0),
            rect(10.0, 0.0, 5.0, 5.0),
            rect(40.0, 0.0, 5.0, 5.0),
        ]);
        let per = gaps_per_region(&shapes);
        assert_eq!(per.len(), 3);
        assert!((per[0].expect("gap").mm - 5.0).abs() < 1e-4);
        // Region 1 sits between the other two and has two neighbours; the
        // nearest wins, and it is not region 2.
        assert!((per[1].expect("gap").mm - 5.0).abs() < 1e-4);
        assert!((per[2].expect("gap").mm - 25.0).abs() < 1e-3);
    }

    #[test]
    fn a_lone_region_in_a_layer_reports_no_neighbour() {
        let per = gaps_per_region(&layer(vec![rect(0.0, 0.0, 5.0, 5.0)]));
        assert_eq!(per, vec![None]);
    }

    #[test]
    fn bars_that_share_an_edge_touch_and_have_no_gap() {
        // Stacked bars: the boundary between them is shared, so weeding them
        // merges them. Reporting 16 mm here would pass a project that fails.
        let shapes = layer(vec![
            rect(0.0, 0.0, 20.0, 2.0),
            rect(0.0, 18.0, 20.0, 2.0),
            rect(0.0, 2.0, 2.0, 16.0),
            rect(18.0, 2.0, 2.0, 16.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(
            gap.mm < 1e-9,
            "joined bars must read as touching, got {}",
            gap.mm
        );
    }

    #[test]
    fn a_ring_with_a_real_opening_measures_the_opening() {
        // Four bars with a 1 mm air gap between their ends, as they would be if
        // each were cut separately. The closing gap is across the middle.
        let shapes = layer(vec![
            rect(0.0, 0.0, 20.0, 2.0),
            rect(0.0, 18.0, 20.0, 2.0),
            rect(0.0, 3.0, 2.0, 14.0),
            rect(18.0, 3.0, 2.0, 14.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        // 1 mm from top to left bar, 1 mm from left to right, 14 mm across.
        assert!(
            (gap.mm - 1.0).abs() < 1e-9,
            "expected the 1 mm joint, got {}",
            gap.mm
        );
    }

    #[test]
    fn a_square_inside_a_triangle_overlaps_and_has_no_gap() {
        let triangle = Shape::new(Contour {
            points: vec![p(0.0, 0.0), p(30.0, 0.0), p(0.0, 30.0)],
            closed: true,
        });
        // x + y spans 24..28 on this square, all under 30, so it is inside.
        let gap = min_gap(&layer(vec![triangle, rect(12.0, 12.0, 2.0, 2.0)])).expect("a gap");
        assert!(gap.mm < 1e-9, "an enclosed region overlaps, got {}", gap.mm);
    }

    #[test]
    fn a_triangle_measures_against_its_nearest_slanted_edge() {
        // A right triangle whose hypotenuse passes close to a small square placed
        // just outside it. The nearest approach is perpendicular to the slant, not
        // axis-aligned, which is the case an offset-based method gets wrong.
        let triangle = Shape::new(Contour {
            points: vec![p(0.0, 0.0), p(30.0, 0.0), p(0.0, 30.0)],
            closed: true,
        });
        // The square's near corner is (20, 14): x + y = 34, so it is 4 mm outside
        // the line x + y = 30, which is 4 / sqrt(2) mm of perpendicular distance.
        let square = rect(20.0, 14.0, 2.0, 2.0);
        let gap = min_gap(&layer(vec![triangle, square])).expect("a gap");
        let expected = 4.0 / f64::sqrt(2.0);
        assert!(
            (gap.mm - expected).abs() < 1e-3,
            "expected {expected:.4} mm perpendicular, got {}",
            gap.mm
        );
    }

    #[test]
    fn a_diagonal_gap_is_measured_as_a_diagonal_not_as_an_axis() {
        // This is the case the offset method got wrong: two squares 6 mm apart on
        // x and 8 mm on y. A mitered offset grew the corner by d x sqrt(2) and
        // reported 8 mm for a true gap of 10.
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(16.0, 18.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(
            (gap.mm - 10.0).abs() < 1e-6,
            "expected 10 mm, got {} — the miter corner is overstating the reach",
            gap.mm
        );
    }

    #[test]
    fn the_measurement_agrees_with_the_line_width_method() {
        // The gap between two bars is the width a single bar of the same
        // separation would measure. Two different code paths reporting the same
        // number is the point: if they diverge, one of them is wrong.
        let bar = rect(0.0, 0.0, 10.0, 4.0);
        let width = crate::offset::min_width(&Shapes::single(bar.clone())).expect("width");

        let apart = 4.0;
        let shapes = layer(vec![bar, rect(10.0 + apart, 0.0, 10.0, 4.0)]);
        let gap = min_gap(&shapes).expect("a gap");

        assert!(
            (gap.mm - apart).abs() < 1e-3,
            "gap {} should match the {apart} mm the bars were placed at",
            gap.mm
        );
        assert!(
            (width - 4.0).abs() < 1e-3,
            "the same shape measures {width} mm wide"
        );
    }

    #[test]
    fn sub_millimetre_gaps_are_still_measured() {
        // 0.4 mm is below the 0.8 mm minimum width and 0.9 mm minimum gap: the
        // exact case that must not round away into a pass.
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.4, 0.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!((gap.mm - 0.4).abs() < 1e-3, "got {}", gap.mm);
    }

    #[test]
    fn measurement_is_deterministic() {
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(13.7, 4.2, 8.0, 8.0),
            rect(40.0, 40.0, 6.0, 6.0),
        ]);
        let first = min_gap(&shapes).expect("a gap");
        let second = min_gap(&shapes).expect("a gap");
        assert_eq!(first, second);
    }

    #[test]
    fn a_negative_offset_search_never_goes_below_zero() {
        // Two regions that touch: the bisection must not report a negative gap,
        // which would pass a threshold check it should fail.
        let shapes = layer(vec![
            rect(0.0, 0.0, 10.0, 10.0),
            rect(10.0, 10.0, 10.0, 10.0),
        ]);
        let gap = min_gap(&shapes).expect("a gap");
        assert!(gap.mm >= 0.0, "a gap cannot be negative, got {}", gap.mm);
    }

    #[test]
    fn the_known_gaps_are_stated_rather_than_left_to_be_discovered() {
        assert!(KNOWN_GAPS.contains("own hole"));
        assert!(KNOWN_GAPS.contains("registration"));
    }

    #[test]
    fn bounding_box_gap_is_a_true_lower_bound() {
        // The pruning must never report more than the true distance, or it
        // would discard a pair that needs measuring.
        assert_eq!(
            bbox_gap((0.0, 0.0, 10.0, 10.0), (20.0, 0.0, 10.0, 10.0)),
            10.0
        );
        assert_eq!(
            bbox_gap((0.0, 0.0, 10.0, 10.0), (5.0, 5.0, 10.0, 10.0)),
            0.0
        );
        assert_eq!(
            bbox_gap((0.0, 0.0, 10.0, 10.0), (16.0, 18.0, 10.0, 10.0)),
            10.0
        );
    }
}
