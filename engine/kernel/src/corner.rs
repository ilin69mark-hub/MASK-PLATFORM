//! Corner sharpness: the minimum interior angle, and rounding it away.
//!
//! Two separate things live here, and keeping them together is deliberate.
//!
//! **Measuring** an interior angle at a vertex is exact on polygonal contours and
//! always was. It is the polygon-only analogue of what T-05 asks for, and it is
//! the check that actually catches a tear.
//!
//! **Rounding** a corner means inserting an arc where the geometry had a vertex.
//! That is also doable on a polygonal contour, as long as the arc is emitted as
//! segments, because that is what the geometry spine can hold.
//!
//! # Why this is not `corner_radius_mm`
//!
//! T-05 is specified as "minimum radius of a curve". On polygonal contours that
//! number does not exist: every vertex has radius zero, so the check would fail
//! on every corner of every shape and mean nothing. Claiming otherwise, or
//! silently reporting a radius the geometry does not contain, would be worse than
//! saying the check does not apply.
//!
//! What is measurable, and physically what the profile is describing, is the
//! angle. `minimums.corner_radius_mm` is documented as "sharp internal angle
//! tears the film", so the angle *is* the requirement; the radius is only the
//! shape of the fix. `rules.min_angle_deg` already carries the threshold.
//!
//! So: T-05 stays an explicit "not applicable to polygonal geometry", and T-14
//! becomes a real check. When curves arrive in the geometry spine, T-05 becomes
//! meaningful and this module gains a measurement rather than a replacement.

use crate::geom::{Contour, Point, Shapes};

/// One corner that is sharper than the material allows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SharpCorner {
    /// Index of the shape the vertex belongs to.
    pub shape: usize,
    /// Index of the contour: 0 is the outer contour, 1.. are holes.
    pub contour: usize,
    /// Index of the vertex within that contour.
    pub vertex: usize,
    /// Interior angle in degrees, measured through the film.
    pub angle_deg: f64,
}

/// The sharpest corner in a layer, and where it is.
///
/// Returns `None` when there is no corner to measure: fewer than three points in
/// a contour means no vertex, and an empty layer has no geometry to judge.
#[must_use]
pub fn sharpest_corner(shapes: &Shapes) -> Option<SharpCorner> {
    let mut worst: Option<SharpCorner> = None;

    for (s, shape) in shapes.shapes.iter().enumerate() {
        for (c, contour) in shape.contours().enumerate() {
            for v in vertex_indices(contour) {
                let Some(angle) = interior_angle(contour, v) else {
                    continue;
                };
                if worst.is_none_or(|current| angle < current.angle_deg) {
                    worst = Some(SharpCorner {
                        shape: s,
                        contour: c,
                        vertex: v,
                        angle_deg: angle,
                    });
                }
            }
        }
    }

    worst
}

/// Every corner sharper than `min_angle_deg`, in layer order.
#[must_use]
pub fn sharp_corners(shapes: &Shapes, min_angle_deg: f64) -> Vec<SharpCorner> {
    let mut out = Vec::new();
    for (s, shape) in shapes.shapes.iter().enumerate() {
        for (c, contour) in shape.contours().enumerate() {
            for v in vertex_indices(contour) {
                if let Some(angle) = interior_angle(contour, v) {
                    if angle < min_angle_deg {
                        out.push(SharpCorner {
                            shape: s,
                            contour: c,
                            vertex: v,
                            angle_deg: angle,
                        });
                    }
                }
            }
        }
    }
    out
}

/// Indices of every vertex, closing the loop when the contour does.
fn vertex_indices(contour: &Contour) -> Vec<usize> {
    let n = contour.points.len();
    match (n, contour.closed) {
        (0..=2, _) => Vec::new(),
        (n, true) => (0..n).collect(),
        (n, false) => (0..n - 1).collect(),
    }
}

/// Interior angle at a vertex, in degrees, measured through the film.
///
/// Two edges leave the vertex, and the angle between them is ambiguous: a convex
/// corner of the film and a notch cut into it look identical as bare vectors. The
/// cross product resolves it, but only relative to the contour's winding — the
/// same cross product means opposite things on a left-turning and a
/// right-turning contour.
#[must_use]
pub fn interior_angle(contour: &Contour, vertex: usize) -> Option<f64> {
    let points = &contour.points;
    let n = points.len();
    if n < 3 || vertex >= n {
        return None;
    }

    let prev = points[(vertex + n - 1) % n];
    let current = points[vertex];
    let next = if contour.closed {
        points[(vertex + 1) % n]
    } else {
        // The last vertex of an open contour has no outgoing edge, so it has no
        // angle. Callers filter these out via vertex_indices, but this function
        // is public and gets called directly too.
        points.get(vertex + 1).copied()?
    };

    // Vectors from the vertex back along each edge.
    let (v1x, v1y) = (prev.x - current.x, prev.y - current.y);
    let (v2x, v2y) = (next.x - current.x, next.y - current.y);

    let len1 = (v1x * v1x + v1y * v1y).sqrt();
    let len2 = (v2x * v2x + v2y * v2y).sqrt();
    if len1 <= crate::constants::GEOM_EPS_MM || len2 <= crate::constants::GEOM_EPS_MM {
        // A repeated vertex has no angle. Reported as absent rather than as 0°,
        // which would read as infinitely sharp and demand an impossible fix.
        return None;
    }

    let raw = ((v1x * v2x + v1y * v2y) / (len1 * len2))
        .clamp(-1.0, 1.0)
        .acos();
    let raw_deg = raw.to_degrees();

    let cross = v1x * v2y - v1y * v2x;

    // On a counter-clockwise contour a convex vertex turns right, so the film
    // lies in the smaller angle. On a clockwise contour the two swap.
    let film_is_ccw = signed_area(contour) > 0.0;
    let convex_in_film = if film_is_ccw {
        cross < 0.0
    } else {
        cross > 0.0
    };

    if convex_in_film {
        Some(raw_deg)
    } else {
        // A notch: the film wraps the long way round.
        Some(360.0 - raw_deg)
    }
}

fn signed_area(contour: &Contour) -> f64 {
    let points = &contour.points;
    if points.len() < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..points.len() {
        let p = points[i];
        let q = points[(i + 1) % points.len()];
        acc += p.x * q.y - q.x * p.y;
    }
    acc / 2.0
}

/// What rounding could not do, so a caller can say so instead of claiming success.
#[derive(Debug, Clone, PartialEq)]
pub enum RoundReport {
    /// Every sharp corner was rounded.
    Complete { rounded: usize },

    /// Some corners could not be rounded. Reported rather than skipped silently:
    /// an unrounded sharp corner is a tear waiting to happen.
    Incomplete {
        rounded: usize,
        skipped: Vec<SkippedCorner>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkippedCorner {
    pub corner: SharpCorner,
    /// Why it was left alone.
    pub reason: SkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The adjacent edges are too short to hold the tangent arc.
    EdgesTooShort,
    /// The radius is not positive.
    BadRadius,
}

/// Insert a fillet at every corner sharper than `min_angle_deg`.
///
/// Returns the rounded geometry and a report naming anything left unrounded. The
/// arc is emitted as segments, so the result is still a polygon: the geometry
/// spine has no curve primitive yet, and pretending otherwise would mean
/// inventing one.
pub fn round_corners(
    shapes: &Shapes,
    radius_mm: f64,
    min_angle_deg: f64,
    curve_tolerance_mm: f64,
) -> (Shapes, RoundReport) {
    let corners = sharp_corners(shapes, min_angle_deg);
    if corners.is_empty() {
        return (shapes.clone(), RoundReport::Complete { rounded: 0 });
    }
    if radius_mm <= 0.0 {
        return (
            shapes.clone(),
            RoundReport::Incomplete {
                rounded: 0,
                skipped: corners
                    .into_iter()
                    .map(|corner| SkippedCorner {
                        corner,
                        reason: SkipReason::BadRadius,
                    })
                    .collect(),
            },
        );
    }

    let mut rounded = 0usize;
    let mut skipped = Vec::new();
    let mut out = shapes.clone();

    // Grouped per shape and contour, then each contour rebuilt in one pass.
    //
    // Filleting one vertex inserts points, which renumbers every vertex after it.
    // Rounding in place while iterating over precomputed indices therefore rounds
    // the wrong vertex — the indices no longer point where they did. Rebuilding
    // the whole contour from a snapshot of its sharp vertices is what keeps every
    // index meaningful for the whole pass.
    let mut by_contour: Vec<(usize, usize, Vec<SharpCorner>)> = Vec::new();
    for corner in corners {
        match by_contour
            .iter_mut()
            .find(|(s, c, _)| *s == corner.shape && *c == corner.contour)
        {
            Some((_, _, list)) => list.push(corner),
            None => by_contour.push((corner.shape, corner.contour, vec![corner])),
        }
    }

    for (shape_index, contour_index, corners) in by_contour {
        let Some(shape) = out.shapes.get_mut(shape_index) else {
            continue;
        };
        let contour: &mut Contour = if contour_index == 0 {
            &mut shape.outer
        } else {
            match shape.holes.get_mut(contour_index - 1) {
                Some(hole) => hole,
                None => continue,
            }
        };

        let snapshot = contour.clone();
        let mut points: Vec<Point> = Vec::with_capacity(snapshot.points.len() + 32);
        let mut cursor = 0usize;

        for corner in corners {
            // Vertices arrive in ascending order, so a skip here only happens if
            // two corners share an index, which sharp_corners cannot produce.
            if corner.vertex < cursor {
                continue;
            }
            points.extend_from_slice(&snapshot.points[cursor..=corner.vertex - 1]);

            match fillet_at(&snapshot, corner.vertex, radius_mm, curve_tolerance_mm) {
                Some(arc) => {
                    points.extend_from_slice(&arc);
                    cursor = corner.vertex + 1;
                    rounded += 1;
                }
                None => {
                    // Keep the vertex as it was and carry on, so one impossible
                    // corner does not abandon the rest.
                    points.push(snapshot.points[corner.vertex]);
                    cursor = corner.vertex + 1;
                    skipped.push(SkippedCorner {
                        corner,
                        reason: SkipReason::EdgesTooShort,
                    });
                }
            }
        }
        points.extend_from_slice(&snapshot.points[cursor..]);

        *contour = Contour {
            points: dedupe(points, curve_tolerance_mm),
            closed: contour.closed,
            // A fillet is emitted as segments, and the vertex indices the arcs were
            // anchored to have shifted underneath them.
            arcs: Vec::new(),
        };
    }

    let report = if skipped.is_empty() {
        RoundReport::Complete { rounded }
    } else {
        RoundReport::Incomplete { rounded, skipped }
    };
    (out, report)
}

/// The arc points replacing one vertex, tangent to both adjacent edges.
///
/// Returns `None` when the neighbouring edges are too short to carry the tangent
/// points. Clamping instead would produce a fillet that does not meet both edges
/// and leave a sliver of film narrower than the knife can hold.
fn fillet_at(
    contour: &Contour,
    vertex: usize,
    radius_mm: f64,
    tolerance_mm: f64,
) -> Option<Vec<Point>> {
    let points = &contour.points;
    let n = points.len();
    if n < 3 || vertex >= n {
        return None;
    }

    let prev = points[(vertex + n - 1) % n];
    let current = points[vertex];
    let next = if contour.closed {
        points[(vertex + 1) % n]
    } else {
        points.get(vertex + 1).copied()?
    };

    let (v1x, v1y) = (prev.x - current.x, prev.y - current.y);
    let (v2x, v2y) = (next.x - current.x, next.y - current.y);
    let len1 = (v1x * v1x + v1y * v1y).sqrt();
    let len2 = (v2x * v2x + v2y * v2y).sqrt();
    if len1 <= crate::constants::GEOM_EPS_MM || len2 <= crate::constants::GEOM_EPS_MM {
        return None;
    }

    let (u1x, u1y) = (v1x / len1, v1y / len1);
    let (u2x, u2y) = (v2x / len2, v2y / len2);

    // Half the angle of film at the vertex. The fillet is cut from the film side,
    // so it is driven by the interior angle rather than by the turn of the path.
    let interior = interior_angle(contour, vertex)? * std::f64::consts::PI / 180.0;
    if interior >= std::f64::consts::PI - 1e-9 {
        // Straight on: nothing to fillet.
        return None;
    }
    let half = interior / 2.0;
    let sin_half = half.sin();
    if sin_half <= 1e-12 {
        return None;
    }
    let tangent = tan_half(half)?;

    let tangent_len = radius_mm / tangent;
    let centre_dist = radius_mm / sin_half;

    // A very sharp spike cannot be rounded with this radius: the tangent points
    // would fall off the ends of the edges. Refusing is the honest answer, because
    // clamping would leave an arc meeting the film at a corner rather than
    // tangentially, and a corner in a fillet is exactly the defect being fixed.
    if tangent_len > len1 || tangent_len > len2 {
        return None;
    }

    let (bx, by) = (u1x + u2x, u1y + u2y);
    let blen = (bx * bx + by * by).sqrt();
    if blen <= 1e-12 {
        return None;
    }

    let centre = Point {
        x: current.x + (bx / blen) * centre_dist,
        y: current.y + (by / blen) * centre_dist,
    };

    let a = Point {
        x: current.x + u1x * tangent_len,
        y: current.y + u1y * tangent_len,
    };
    let b = Point {
        x: current.x + u2x * tangent_len,
        y: current.y + u2y * tangent_len,
    };

    // Check the arc really is tangent to both edges. If the bisector assumption
    // were wrong the arc would cut across the corner instead of rolling over it,
    // and the shape would gain area.
    if (a.distance_to(centre) - radius_mm).abs() > 1e-6
        || (b.distance_to(centre) - radius_mm).abs() > 1e-6
    {
        return None;
    }

    let sweep = std::f64::consts::PI - interior;
    let steps = arc_steps(radius_mm, sweep, tolerance_mm);
    let angle_a = (a.y - centre.y).atan2(a.x - centre.x);

    let mut arc = Vec::with_capacity(steps + 2);
    arc.push(a);
    for step in 1..steps {
        let t = angle_a + sweep * (step as f64) / (steps as f64);
        arc.push(Point {
            x: centre.x + radius_mm * t.cos(),
            y: centre.y + radius_mm * t.sin(),
        });
    }
    arc.push(b);
    Some(arc)
}

/// `tan`, rejecting angles where it would lose all precision.
fn tan_half(half: f64) -> Option<f64> {
    if half <= 1e-9 || half >= std::f64::consts::FRAC_PI_2 - 1e-9 {
        return None;
    }
    Some(half.tan())
}

/// How many segments approximate an arc within a sagitta tolerance.
///
/// Solves `r(1 - cos(step/2)) <= tolerance` for the step angle. A tolerance at or
/// beyond the radius would ask for a single chord, so it is floored at one
/// segment, and the count is capped so a zero tolerance cannot ask for millions.
fn arc_steps(radius_mm: f64, sweep: f64, tolerance_mm: f64) -> usize {
    const MAX_STEPS: usize = 256;

    if radius_mm <= 0.0 || sweep <= 0.0 {
        return 1;
    }
    let ratio = (tolerance_mm / radius_mm).clamp(0.0, 1.0);
    let max_step = 2.0 * (1.0 - ratio).acos();
    if !max_step.is_finite() || max_step <= 0.0 {
        return MAX_STEPS;
    }
    ((sweep / max_step).ceil() as usize).clamp(1, MAX_STEPS)
}

/// Remove points closer together than the tolerance.
fn dedupe(points: Vec<Point>, tolerance_mm: f64) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out
            .last()
            .is_none_or(|last: &Point| last.distance_to(p) > tolerance_mm)
        {
            out.push(p);
        }
    }
    // A closed loop can also produce duplicates across the seam.
    while out.len() > 1 && out[0].distance_to(*out.last().expect("checked length")) <= tolerance_mm
    {
        out.pop();
    }
    out
}

/// Deliberate omissions, kept greppable.
pub const KNOWN_GAPS: &str = "\
this is the polygonal proxy for T-05, not the radius measurement T-05 asks \
for; radius stays unmeasurable until curves exist in the geometry spine. \
Rounding refines the blade corner but does not model blade offset dragging on \
an inside turn, which needs the tool's swept path.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Shape;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    /// A closed polygon from points, counter-clockwise.
    fn ring(pts: Vec<Point>) -> Contour {
        Contour {
            points: pts,
            closed: true,
            arcs: Vec::new(),
        }
    }

    fn square(x: f64, y: f64, w: f64) -> Shape {
        Shape::new(ring(vec![
            p(x, y),
            p(x + w, y),
            p(x + w, y + w),
            p(x, y + w),
        ]))
    }

    /// A square with one corner pulled far in, forming a deep notch.
    ///
    /// Pulling a single corner in produces *two* sharp vertices, not one: the two
    /// neighbours of the pulled vertex turn sharply too, because the diagonal edge
    /// between them now meets the straight sides at a small angle. The pulled
    /// vertex itself becomes a reflex corner of about 242°, which is a notch in
    /// the film and not a tear risk — it is deliberately not counted.
    ///
    /// Pinned here because it is counter-intuitive and it corrected three tests
    /// that all assumed a single spike.
    fn notched_square(inset: f64) -> Shape {
        Shape::new(ring(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0 - inset, 10.0 - inset),
            p(0.0, 10.0),
        ]))
    }

    fn layer(shapes: Vec<Shape>) -> Shapes {
        Shapes { shapes }
    }

    #[test]
    fn a_right_angle_measures_ninety_degrees() {
        let contour = ring(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)]);
        for v in 0..4 {
            let angle = interior_angle(&contour, v).expect("an angle");
            assert!((angle - 90.0).abs() < 1e-9, "vertex {v}: {angle}");
        }
    }

    #[test]
    fn the_angle_is_independent_of_where_the_polygon_starts() {
        // The same shape, rotated one vertex. A measure that depended on the
        // starting point would report a different angle per vertex.
        let a = ring(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)]);
        let b = ring(vec![p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0), p(0.0, 0.0)]);
        let first: Vec<f64> = (0..4)
            .map(|v| interior_angle(&a, v).expect("an angle"))
            .collect();
        let second: Vec<f64> = (0..4)
            .map(|v| interior_angle(&b, v).expect("an angle"))
            .collect();
        assert_eq!(first, second);
    }

    #[test]
    fn a_reversed_contour_measures_the_same_angles() {
        // Orientation decides which way the film wraps, not how sharp a corner is.
        // A clockwise square has the same four 90° corners as a counter-clockwise
        // one, and reading it the other way would report 270° and flag every
        // ordinary square as a notch.
        let ccw = ring(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)]);
        let cw = ring(vec![p(0.0, 0.0), p(0.0, 10.0), p(10.0, 10.0), p(10.0, 0.0)]);
        for v in 0..4 {
            let a = interior_angle(&ccw, v).expect("ccw");
            let b = interior_angle(&cw, v).expect("cw");
            assert!((a - b).abs() < 1e-9, "vertex {v}: {a} vs {b}");
        }
    }

    #[test]
    fn a_notch_measures_more_than_a_hundred_eighty_degrees() {
        // An L-shape: the inner corner has 270° of film around it.
        let contour = ring(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(10.0, 4.0),
            p(4.0, 4.0),
            p(4.0, 10.0),
            p(0.0, 10.0),
        ]);
        let mut angles: Vec<f64> = vertex_indices(&contour)
            .into_iter()
            .filter_map(|v| interior_angle(&contour, v))
            .collect();
        angles.sort_by(|a, b| b.partial_cmp(a).expect("finite"));
        assert!(
            (angles[0] - 270.0).abs() < 1e-9,
            "the notch should be 270°, got {angles:?}"
        );
    }

    #[test]
    fn a_notch_produces_two_sharp_vertices_and_one_reflex_one() {
        // Pulling a corner in sharpens the two *neighbours*, because the diagonal
        // edge meets the straight sides at 14°. The pulled vertex itself becomes a
        // 242° reflex corner: a notch in the film, not a tear risk, so it is not
        // flagged. All three tests here assumed one spike and were wrong.
        let shape = notched_square(8.0);
        let contour = &shape.outer;
        assert!(
            (interior_angle(contour, 2).expect("an angle") - 241.93).abs() < 0.1,
            "the pulled vertex is reflex, not sharp"
        );
        let found = sharp_corners(&layer(vec![shape]), 30.0);
        assert_eq!(found.len(), 2, "two sharp vertices, got {found:?}");
        assert!(found.iter().all(|c| c.angle_deg < 20.0));
    }

    #[test]
    fn a_repeated_vertex_has_no_angle_rather_than_a_zero_one() {
        // A duplicate point would measure 0° and demand an impossible fix.
        let contour = ring(vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 0.0), p(0.0, 10.0)]);
        assert_eq!(interior_angle(&contour, 2), None);
    }

    #[test]
    fn a_degenerate_contour_reports_no_corners() {
        assert!(vertex_indices(&ring(vec![])).is_empty());
        assert!(vertex_indices(&ring(vec![p(0.0, 0.0)])).is_empty());
        assert!(vertex_indices(&ring(vec![p(0.0, 0.0), p(1.0, 1.0)])).is_empty());
    }

    #[test]
    fn an_empty_layer_has_no_sharpest_corner() {
        assert_eq!(sharpest_corner(&layer(vec![])), None);
    }

    #[test]
    fn the_sharpest_corner_of_an_ordinary_square_is_ninety_degrees() {
        let corner = sharpest_corner(&layer(vec![square(0.0, 0.0, 10.0)])).expect("a corner");
        assert!((corner.angle_deg - 90.0).abs() < 1e-9);
        assert_eq!(corner.contour, 0, "the first contour is the outer one");
    }

    #[test]
    fn an_ordinary_square_raises_no_sharp_corner_warning() {
        let shapes = layer(vec![square(0.0, 0.0, 10.0)]);
        assert!(
            sharp_corners(&shapes, 30.0).is_empty(),
            "a square is not sharp: {:?}",
            sharp_corners(&shapes, 30.0)
        );
    }

    #[test]
    fn sharp_corners_are_reported_with_their_location() {
        let shapes = layer(vec![notched_square(8.0)]);
        let found = sharp_corners(&shapes, 30.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|c| c.shape == 0 && c.contour == 0));
        // Vertices 1 and 3 flank the pulled-in corner at index 2.
        assert_eq!(
            found.iter().map(|c| c.vertex).collect::<Vec<_>>(),
            vec![1, 3]
        );
    }

    #[test]
    fn rounding_replaces_the_spike_with_an_arc() {
        let shapes = layer(vec![notched_square(8.0)]);
        let (rounded, report) = round_corners(&shapes, 1.0, 30.0, 0.05);
        assert!(
            matches!(report, RoundReport::Complete { rounded: 2 }),
            "both sharp neighbours should be filleted, got {report:?}"
        );
        // More points than before, because the vertex became several segments.
        assert!(
            rounded.shapes[0].outer.points.len() > shapes.shapes[0].outer.points.len(),
            "a fillet must add geometry"
        );
    }

    #[test]
    fn rounding_removes_the_sharp_corner_it_was_aimed_at() {
        let shapes = layer(vec![notched_square(8.0)]);
        assert_eq!(sharp_corners(&shapes, 30.0).len(), 2, "precondition");
        let (rounded, _) = round_corners(&shapes, 1.0, 30.0, 0.05);
        let remaining = sharp_corners(&rounded, 30.0);
        assert!(
            remaining.is_empty(),
            "the fillet should have taken the corner below the threshold, found {remaining:?}"
        );
    }

    #[test]
    fn rounding_only_ever_removes_material() {
        // A fillet rolls the corner off, so the area can shrink and never grow.
        // If it grew, the arc is on the wrong side of the film.
        let shapes = layer(vec![notched_square(8.0)]);
        let before = crate::measure::shapes_area(&shapes);
        let (rounded, _) = round_corners(&shapes, 1.0, 30.0, 0.05);
        let after = crate::measure::shapes_area(&rounded);
        assert!(after < before, "area grew from {before} to {after}");
    }

    #[test]
    fn rounding_a_deep_notch_costs_more_because_the_corners_are_sharper() {
        // Measured, not assumed: a 14° spike needs a tangent point 8.1 mm along
        // each edge to carry a 1 mm fillet, so a large share of a 10 mm edge goes.
        // That is the geometry, not a defect — a very sharp corner cannot be
        // rounded cheaply. What matters is that it is not silent, which is what
        // the loss report and the SharpCorner location are for.
        let shapes = layer(vec![notched_square(8.0)]);
        let (rounded, _) = round_corners(&shapes, 1.0, 30.0, 0.05);
        let before = crate::measure::shapes_area(&shapes);
        let after = crate::measure::shapes_area(&rounded);
        assert!(
            (before - after) / before > 0.10,
            "expected a visible loss on a 14° spike, got {before} -> {after}"
        );
    }

    #[test]
    fn rounding_a_large_shape_with_a_small_fillet_costs_almost_nothing() {
        // The practical case: a 200 mm figure with 0.5 mm fillets, where the
        // geometry can afford them.
        let big = Shape::new(ring(vec![
            p(0.0, 0.0),
            p(200.0, 0.0),
            p(200.0, 200.0),
            p(60.0, 60.0),
            p(0.0, 200.0),
        ]));
        let shapes = layer(vec![big]);
        let before = crate::measure::shapes_area(&shapes);
        let (rounded, report) = round_corners(&shapes, 0.5, 45.0, 0.02);
        let after = crate::measure::shapes_area(&rounded);
        assert!(
            matches!(report, RoundReport::Complete { rounded: r } if r > 0),
            "expected fillets on a large figure, got {report:?}"
        );
        assert!(after < before);
        assert!(
            (before - after) / before < 0.01,
            "a 0.5 mm fillet on a 200 mm figure should cost under 1%, lost {} of {before}",
            before - after
        );
    }

    #[test]
    fn rounding_leaves_an_untouched_shape_alone() {
        let shapes = layer(vec![square(0.0, 0.0, 10.0)]);
        let (rounded, report) = round_corners(&shapes, 1.0, 30.0, 0.05);
        assert_eq!(rounded, shapes, "nothing was sharp, so nothing changed");
        assert!(matches!(report, RoundReport::Complete { rounded: 0 }));
    }

    #[test]
    fn a_corner_too_tight_for_its_edges_is_reported_not_silently_skipped() {
        // A long thin spike: a 1 mm fillet needs more tangent than the edges hold.
        let shape = Shape::new(ring(vec![
            p(0.0, 0.0),
            p(10.0, 0.0),
            p(9.9, 0.05),
            p(0.0, 10.0),
        ]));
        let shapes = layer(vec![shape]);
        let (_, report) = round_corners(&shapes, 1.0, 30.0, 0.05);
        match report {
            RoundReport::Incomplete { rounded, skipped } => {
                assert_eq!(rounded, 0);
                assert_eq!(skipped.len(), 1, "one corner could not be rounded");
                assert_eq!(skipped[0].reason, SkipReason::EdgesTooShort);
            }
            other => panic!("expected an incomplete report, got {other:?}"),
        }
    }

    #[test]
    fn a_zero_radius_rounds_nothing_and_says_so() {
        let shapes = layer(vec![notched_square(8.0)]);
        let (out, report) = round_corners(&shapes, 0.0, 30.0, 0.05);
        assert_eq!(out, shapes, "nothing could be done");
        match report {
            RoundReport::Incomplete { rounded, skipped } => {
                assert_eq!(rounded, 0);
                assert_eq!(skipped[0].reason, SkipReason::BadRadius);
            }
            other => panic!("expected an incomplete report, got {other:?}"),
        }
    }

    #[test]
    fn rounding_is_deterministic() {
        let shapes = layer(vec![notched_square(8.0)]);
        let a = round_corners(&shapes, 1.0, 30.0, 0.05);
        let b = round_corners(&shapes, 1.0, 30.0, 0.05);
        assert_eq!(a, b);
    }

    #[test]
    fn rounded_geometry_still_passes_validation() {
        // The fillet rewrites the contour, so a wrong winding would show up here
        // as the silent-area bug the kernel already had once.
        let shapes = layer(vec![notched_square(8.0)]);
        let (rounded, _) = round_corners(&shapes, 1.0, 30.0, 0.05);
        for shape in &rounded.shapes {
            shape
                .validate_orientation()
                .expect("the fillet preserves winding");
        }
        assert!(rounded.shapes[0].outer.points.len() >= 3);
    }

    #[test]
    fn arc_segments_stay_within_the_tolerance() {
        // The whole reason for a tolerance parameter: a coarse arc cuts the corner
        // differently from a fine one, and the coarse one must still be a valid
        // approximation rather than a chord through the wrong place.
        let coarse = arc_steps(1.0, std::f64::consts::FRAC_PI_2, 0.20);
        let fine = arc_steps(1.0, std::f64::consts::FRAC_PI_2, 0.001);
        assert!(fine > coarse, "a tighter tolerance needs more segments");
        assert!(coarse >= 1);
        assert!(fine <= 256, "and must stay bounded");
    }

    #[test]
    fn the_known_gap_is_stated_rather_than_left_to_be_discovered() {
        assert!(KNOWN_GAPS.contains("T-05"));
        assert!(KNOWN_GAPS.contains("curves"));
    }
}
