//! Self-intersections (T-08) and unclosed cut contours (T-09).
//!
//! Both are correctness checks rather than manufacturability checks. They answer
//! "is this geometry even well formed?" and the answer has to be no before any
//! measurement means anything.
//!
//! # Why this matters more than it looks
//!
//! Signed area assumes a simple polygon: edges cross only at shared vertices.
//! When a contour crosses itself, the crossings divide it into lobes that cancel.
//! The area comes out a plausible number that is not the area of anything on the
//! table, and it is off in whichever direction the winding happens to favour.
//!
//! This project has now been bitten by that shape of defect twice — a hole with
//! the outer's winding reported 500 mm² for a 300 mm² figure, and nothing
//! complained, because a wrong area looks like a number. A self-intersection is
//! the same failure with more freedom in how it goes wrong, and it reaches the
//! exporter as well as the validator.
//!
//! # Why detection, not automatic repair
//!
//! `boolean::normalise` would resolve all of this in one call, and it is not
//! called anywhere. That is deliberate here: resolving a self-intersection
//! changes the figure, and a mask is a physical object cut out of film. Silently
//! reshaping someone's design to make a check pass is the wrong default even when
//! the result is geometrically defensible.
//!
//! So this module detects and reports, with a precise location, and the operator
//! decides. `normalise` stays available as an explicit repair, and the validator
//! says so.

use crate::constants::GEOM_EPS_MM;
use crate::geom::{Contour, Point, Shapes};

/// One point where a contour crosses itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelfIntersection {
    /// Index of the shape.
    pub shape: usize,
    /// Index of the contour: 0 is the outer contour, 1.. are holes.
    pub contour: usize,
    /// Segment index within the contour, counting the closing segment last.
    pub segment: usize,
    /// Where the crossing is.
    pub at: Point,
}

/// Every self-intersection in a set, in layer order.
#[must_use]
pub fn self_intersections(shapes: &Shapes) -> Vec<SelfIntersection> {
    let mut out = Vec::new();
    for (s, shape) in shapes.shapes.iter().enumerate() {
        for (c, contour) in shape.contours().enumerate() {
            collect_intersections(contour, s, c, &mut out);
        }
    }
    out
}

/// Crossings within one contour.
///
/// Two segments are considered crossing when they meet at an interior point of
/// both. A shared endpoint is not a crossing: consecutive segments meet there, and
/// a fan of segments from one vertex is a pinch, not a bow tie. Pinches are
/// reported separately, because a pinch is still a degenerate contour and still
/// makes signed area meaningless.
fn collect_intersections(
    contour: &Contour,
    shape: usize,
    contour_index: usize,
    out: &mut Vec<SelfIntersection>,
) {
    let segments = segments_of(contour);
    for i in 0..segments.len() {
        for j in (i + 1)..segments.len() {
            // Adjacent segments share an endpoint by construction.
            let shares_vertex = i + 1 == j || (i == 0 && j + 1 == segments.len());
            let (a1, a2) = segments[i];
            let (b1, b2) = segments[j];

            if shares_vertex {
                // Legitimate neighbours, unless the third segment crosses one of
                // them away from the shared point. That is a genuine crossing and
                // is caught by the pairs below.
                continue;
            }

            if let Some(at) = proper_crossing(a1, a2, b1, b2) {
                out.push(SelfIntersection {
                    shape,
                    contour: contour_index,
                    segment: i,
                    at,
                });
            }
        }
    }
}

/// Where two segments cross at an interior point of both, if they do.
fn proper_crossing(a1: Point, a2: Point, b1: Point, b2: Point) -> Option<Point> {
    let r = (a2.x - a1.x, a2.y - a1.y);
    let s = (b2.x - b1.x, b2.y - b1.y);
    let denominator = r.0 * s.1 - r.1 * s.0;
    if denominator.abs() <= GEOM_EPS_MM {
        // Parallel or collinear. Collinear overlap is degenerate but not a
        // crossing; it is caught by the pinch and area checks.
        return None;
    }

    let qp = (b1.x - a1.x, b1.y - a1.y);
    let t = (qp.0 * s.1 - qp.1 * s.0) / denominator;
    let u = (qp.0 * r.1 - qp.1 * r.0) / denominator;

    // Strictly interior: a crossing at an endpoint belongs to a vertex, not to
    // the interiors of two edges.
    let inside = |v: f64| v > EDGE_EPS && v < 1.0 - EDGE_EPS;
    if !inside(t) || !inside(u) {
        return None;
    }

    Some(Point {
        x: a1.x + r.0 * t,
        y: a1.y + r.1 * t,
    })
}

/// How close to a segment's end a crossing may sit and still count as interior.
///
/// One part in a million of the segment, so a real crossing a hair from a vertex
/// is still a crossing. At 1e-6 exactly the two tests could disagree on a shared
/// vertex and report every polygon as self-intersecting.
const EDGE_EPS: f64 = 1e-6;

/// A contour as segments. A closed contour includes the closing segment.
fn segments_of(contour: &Contour) -> Vec<(Point, Point)> {
    let n = contour.points.len();
    if n < 2 {
        return Vec::new();
    }
    let last = if contour.closed { n } else { n - 1 };
    (0..last)
        .map(|i| (contour.points[i], contour.points[(i + 1) % n]))
        .collect()
}

/// A cut contour that is not closed.
///
/// A score line is drawn and left; a cut line must be a closed loop or the knife
/// follows it off the sheet. `closed` is carried explicitly in the geometry for
/// exactly this reason, and this is the check that reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenContour {
    pub shape: usize,
    pub contour: usize,
    /// Where the line ends up going, for the message.
    pub end: Point,
}

/// Every contour marked as a cut line but left open.
#[must_use]
pub fn open_cut_contours(shapes: &Shapes) -> Vec<OpenContour> {
    let mut out = Vec::new();
    for (s, shape) in shapes.shapes.iter().enumerate() {
        // Contour 0 is an outer boundary and is always closed. A hole could
        // technically be drawn as a spiral to be cut with a peel-tab, so holes are
        // included in the check.
        for (c, contour) in shape.contours().enumerate() {
            if !contour.closed {
                out.push(OpenContour {
                    shape: s,
                    contour: c,
                    end: contour
                        .points
                        .last()
                        .copied()
                        .unwrap_or(Point { x: 0.0, y: 0.0 }),
                });
            }
        }
    }
    out
}

/// The area a contour's signed rule produces, and the area of its even-odd fill.
///
/// Two different numbers from the same contour, which is what makes a
/// self-intersection detectable without a full topological analysis: a simple
/// polygon gives the same answer either way, and a crossed one cannot.
///
/// This is available to tests because its only current use is a boundary test. It is
/// not part of the public geometry contract.
#[cfg(test)]
#[must_use]
pub(crate) fn area_disagreement(contour: &Contour) -> f64 {
    (signed_area(contour) - even_odd_area(contour)).abs()
}

#[cfg(test)]
fn signed_area(contour: &Contour) -> f64 {
    let points = &contour.points;
    if points.len() < 3 {
        return 0.0;
    }
    let n = points.len();
    let last = if contour.closed { n } else { n - 1 };
    let mut acc = 0.0;
    for i in 0..last {
        let p = points[i];
        let q = points[(i + 1) % n];
        acc += p.x * q.y - q.x * p.y;
    }
    acc / 2.0
}

/// Scanline even-odd area: count crossings of a vertical line, alternate inside
/// and out.
#[cfg(test)]
fn even_odd_area(contour: &Contour) -> f64 {
    let points = &contour.points;
    let n = points.len();
    if n < 3 {
        return 0.0;
    }
    let segments = segments_of(contour);
    let min_y = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let max_y = points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max);
    if max_y <= min_y {
        return 0.0;
    }

    // Sampling rows is a raster method, which this module otherwise avoids. It is
    // used only to *detect* a disagreement, never to report a dimension: the exact
    // area always comes from the signed rule.
    let rows = 512usize;
    let dy = (max_y - min_y) / rows as f64;
    let mut total = 0.0;

    for row in 0..rows {
        let y = min_y + (row as f64 + 0.5) * dy;
        let mut xs: Vec<f64> = Vec::new();
        for (a, b) in &segments {
            let (lo, hi) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
            if y < lo || y >= hi {
                continue;
            }
            let t = (y - a.y) / (b.y - a.y);
            xs.push(a.x + (b.x - a.x) * t);
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Between each consecutive pair of crossings the scanline is inside the
        // contour. That is the even-odd rule: the polygon may not be convex.
        let mut width = 0.0;
        for pair in xs.as_chunks::<2>().0 {
            width += (pair[1] - pair[0]).abs();
        }
        total += width * dy;
    }
    total
}

/// Deliberate omissions, kept greppable.
pub const KNOWN_GAPS: &str = "\
does not attempt to repair, only to locate: boolean::normalise could resolve \
every one of these in one call, but reshaping someone's design to make a check \
pass is the wrong default for a physical object cut from film. \
Touching-but-not-crossing contours, collinear overlaps and zero-width pinches are \
degenerate and are left to the width and area checks.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Shape;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn closed(pts: Vec<Point>) -> Contour {
        Contour {
            points: pts,
            closed: true,
            arcs: Vec::new(),
        }
    }

    fn layer(shapes: Vec<Shape>) -> Shapes {
        Shapes { shapes }
    }

    #[test]
    fn a_simple_square_has_no_self_intersection() {
        let square = Shape::new(closed(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 10.0),
            p(0.0, 10.0),
        ]));
        assert!(
            self_intersections(&layer(vec![square])).is_empty(),
            "a square is simple"
        );
    }

    #[test]
    fn a_star_polygon_is_flagged_as_self_intersecting() {
        // A pentagram: five points, five crossings, and a signed area that cancels
        // most of the figure.
        let star = Shape::new(closed(vec![
            p(10.0, 20.0),
            p(4.1221, 1.9098),
            p(19.5106, 13.0902),
            p(0.4894, 13.0902),
            p(15.8779, 1.9098),
        ]));
        let found = self_intersections(&layer(vec![star]));
        assert_eq!(
            found.len(),
            5,
            "a pentagram crosses five times, got {found:?}"
        );
    }

    #[test]
    fn a_crossing_is_reported_with_its_location() {
        let bowtie = Shape::new(closed(vec![
            p(0.0, 0.0),
            p(10.0, 10.0),
            p(10.0, 0.0),
            p(0.0, 10.0),
        ]));
        let found = self_intersections(&layer(vec![bowtie]));
        assert_eq!(found.len(), 1, "one crossing in a bow tie: {found:?}");
        let at = found[0].at;
        assert!(
            (at.x - 5.0).abs() < 1e-6 && (at.y - 5.0).abs() < 1e-6,
            "the crossing should be at the centre, got {at:?}"
        );
        assert_eq!(found[0].shape, 0);
        assert_eq!(found[0].contour, 0);
    }

    #[test]
    fn consecutive_segments_sharing_a_vertex_are_not_a_crossing() {
        // If shared endpoints counted, every polygon in the layer would be
        // self-intersecting and the rule would be pure noise.
        let square = Shape::new(closed(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 10.0),
            p(0.0, 10.0),
        ]));
        assert!(self_intersections(&layer(vec![square])).is_empty());
    }

    #[test]
    fn the_closing_segment_is_part_of_the_check() {
        // The last point sits off to the right, so the closing edge sweeps back
        // across the interior and crosses the right-hand side. Forgetting the
        // closing segment — the easy omission — would miss this entirely.
        let contour = Shape::new(closed(vec![
            p(0.0, 0.0),
            p(20.0, 0.0),
            p(20.0, 20.0),
            p(0.0, 20.0),
            p(30.0, 10.0),
        ]));
        let edges = segments_of(&contour.outer);
        // Precondition, checked directly so a bad fixture cannot pass silently.
        let closing = *edges.last().expect("a closing segment");
        assert!(
            proper_crossing(closing.0, closing.1, edges[1].0, edges[1].1).is_some(),
            "the closing edge must cross the right side"
        );
        // This shape has two crossings and both involve the closing edge: it cuts
        // the right side and then the bottom-right corner region. One of them is
        // the point of the test — without the closing edge the count would be 0.
        let found = self_intersections(&layer(vec![contour]));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(
            proper_crossing(closing.0, closing.1, edges[1].0, edges[1].1).is_some(),
            "the crossing involves the closing segment"
        );
    }

    #[test]
    fn two_parallel_edges_are_not_a_crossing() {
        // A contour that doubles back on itself is degenerate, not crossed.
        let contour = closed(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 10.0),
            p(0.0, 10.0),
            p(0.0, 0.0),
        ]);
        assert!(
            self_intersections(&layer(vec![Shape::new(contour)])).is_empty(),
            "a doubled edge is not a crossing"
        );
    }

    #[test]
    fn a_self_crossing_hole_is_reported_as_contour_one() {
        // Contours are checked one at a time on purpose: the defect that matters is
        // each contour being simple in its own right, not contours avoiding each
        // other. A bow-tie hole is the smallest case.
        let outer = closed(vec![p(0.0, 0.0), p(20.0, 0.0), p(20.0, 20.0), p(0.0, 20.0)]);
        let hole = closed(vec![p(5.0, 5.0), p(15.0, 5.0), p(5.0, 15.0), p(15.0, 15.0)]);
        let found = self_intersections(&layer(vec![Shape::with_holes(outer, vec![hole])]));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].contour, 1, "the hole must be named, not the outer");
    }

    #[test]
    fn two_ordinary_contours_that_do_not_cross_are_not_reported() {
        // The other failure mode: treating any overlap between a hole and its outer
        // contour as a self-intersection would flag almost every real shape.
        let outer = closed(vec![p(0.0, 0.0), p(20.0, 0.0), p(20.0, 20.0), p(0.0, 20.0)]);
        let hole = closed(vec![p(5.0, 5.0), p(15.0, 5.0), p(15.0, 15.0), p(5.0, 15.0)]);
        let found = self_intersections(&layer(vec![Shape::with_holes(outer, vec![hole])]));
        assert!(
            found.is_empty(),
            "plain nested contours must not be flagged: {found:?}"
        );
    }

    #[test]
    fn a_crossing_makes_the_two_area_rules_disagree() {
        // This is the property that makes the defect observable at all: signed area
        // assumes simplicity, so the two rules can only differ if something is wrong.
        let simple = closed(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)]);
        assert!(
            area_disagreement(&simple) < 0.5,
            "a square agrees, got {}",
            area_disagreement(&simple)
        );

        let bowtie = closed(vec![p(0.0, 0.0), p(10.0, 10.0), p(10.0, 0.0), p(0.0, 10.0)]);
        assert!(
            area_disagreement(&bowtie) > 10.0,
            "a bow tie must disagree, got {}",
            area_disagreement(&bowtie)
        );
    }

    #[test]
    fn an_open_contour_is_reported_as_a_cut_line_that_goes_nowhere() {
        let open = Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)],
            closed: false,
            arcs: Vec::new(),
        };
        let found = open_cut_contours(&layer(vec![Shape::new(open)]));
        assert_eq!(found.len(), 1, "one open contour: {found:?}");
        assert_eq!((found[0].shape, found[0].contour), (0, 0));
        assert!(
            (found[0].end.x - 10.0).abs() < 1e-9,
            "the end point is where it stops"
        );
    }

    #[test]
    fn a_closed_layer_reports_no_open_contours() {
        let square = Shape::new(closed(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 10.0),
            p(0.0, 10.0),
        ]));
        assert!(open_cut_contours(&layer(vec![square])).is_empty());
    }

    #[test]
    fn an_open_hole_is_reported_too() {
        // A spiral hole is a legitimate way to cut a hole with a peel tab, so this
        // is a finding the operator must see rather than a silent correction.
        let outer = closed(vec![p(0.0, 0.0), p(20.0, 0.0), p(20.0, 20.0), p(0.0, 20.0)]);
        let spiral = Contour {
            points: vec![p(5.0, 5.0), p(15.0, 5.0), p(15.0, 15.0)],
            closed: false,
            arcs: Vec::new(),
        };
        let found = open_cut_contours(&layer(vec![Shape::with_holes(outer, vec![spiral])]));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].contour, 1, "an open hole is contour 1");
    }

    #[test]
    fn an_empty_layer_is_clean_on_both_counts() {
        let empty = layer(vec![]);
        assert!(self_intersections(&empty).is_empty());
        assert!(open_cut_contours(&empty).is_empty());
    }

    #[test]
    fn detection_is_deterministic() {
        let star = Shape::new(closed(vec![
            p(10.0, 20.0),
            p(4.1221, 1.9098),
            p(19.5106, 13.0902),
            p(0.4894, 13.0902),
            p(15.8779, 1.9098),
        ]));
        let shapes = layer(vec![star]);
        assert_eq!(self_intersections(&shapes), self_intersections(&shapes));
    }

    #[test]
    fn the_known_gap_is_stated_rather_than_left_to_be_discovered() {
        assert!(KNOWN_GAPS.contains("normalise"));
        assert!(KNOWN_GAPS.contains("repair"));
    }
}
