//! Canonical form. ADR-0004.
//!
//! Two encodings of the same geometry must produce one `content_hash`, or the
//! metrics cache and the reproduction `verify` mode both report false
//! mismatches. That is why this module exists and why it is not optional.

use crate::constants::{CANON_COORD_DP, COLLINEAR_MM};
use crate::geom::{ArcSegment, Contour, Point, Shape, Shapes};

/// Quantise a point onto the canonical grid.
#[must_use]
pub fn quantise_point(p: Point) -> Point {
    Point {
        x: crate::constants::round_to(p.x, CANON_COORD_DP),
        y: crate::constants::round_to(p.y, CANON_COORD_DP),
    }
}

/// Drop consecutive points closer than `COLLINEAR_MM`.
///
/// A closed contour does not gain a duplicate closing point: the `closed` flag
/// already carries that information, and a duplicate would change the hash
/// without changing the geometry.
#[must_use]
pub fn dedupe(points: &[Point], closed: bool) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if let Some(last) = out.last() {
            if last.distance_to(*p) <= COLLINEAR_MM {
                continue;
            }
        }
        out.push(*p);
    }

    if closed {
        while out.len() > 1 {
            let first = out[0];
            let last = *out.last().expect("non-empty");
            if last.distance_to(first) <= COLLINEAR_MM {
                out.pop();
            } else {
                break;
            }
        }
    }

    out
}

/// Canonicalise a contour: quantise, then dedupe, keeping any arcs.
///
/// Arcs survive canonicalisation and are re-indexed onto the surviving points.
/// Dropping them, as this once did, meant a curve and the polygon through the
/// same points canonicalised to the same `M`/`L`/`Z` string and therefore to the
/// same digest — two cuts with different areas sharing one identity, which is the
/// one thing the hash exists to prevent.
///
/// An arc whose anchor point was removed as a duplicate is dropped rather than
/// guessed at: it would otherwise describe a curve between two points that no
/// longer mean what they meant.
#[must_use]
pub fn canonicalise(contour: &Contour) -> Contour {
    let quantised: Vec<Point> = contour.points.iter().copied().map(quantise_point).collect();
    let points = dedupe(&quantised, contour.closed);

    // Map each original index to its position in the deduped list. A removed
    // point has no position, and an arc touching one cannot be re-indexed.
    let mut remap: Vec<Option<usize>> = Vec::with_capacity(quantised.len());
    let mut cursor = 0usize;
    for q in &quantised {
        let same = cursor < points.len() && points[cursor].x == q.x && points[cursor].y == q.y;
        if same {
            remap.push(Some(cursor));
            cursor += 1;
        } else {
            remap.push(None);
        }
    }

    let arcs = contour
        .arcs
        .iter()
        .filter_map(|arc| {
            match (
                remap.get(arc.from).copied().flatten(),
                remap.get(arc.to).copied().flatten(),
            ) {
                (Some(from), Some(to)) => Some(ArcSegment { from, to, ..*arc }),
                _ => None,
            }
        })
        .collect();

    Contour {
        points,
        closed: contour.closed,
        arcs,
    }
}

/// Canonicalise a whole shape set.
#[must_use]
pub fn canonicalise_shapes(shapes: &Shapes) -> Shapes {
    Shapes {
        shapes: shapes
            .shapes
            .iter()
            .map(|shape| Shape {
                outer: canonicalise(&shape.outer),
                holes: shape.holes.iter().map(canonicalise).collect(),
            })
            .collect(),
    }
}

/// Emit absolute `M` / `L` / `C` / `Z` commands only.
///
/// Relative, shorthand and arc commands are forbidden by ADR-0004: different
/// writers spell the same path differently, and the hash must not care. Arcs are
/// not forbidden outright — the ADR's own rule is that they are converted to
/// cubic Béziers, which is what happens here.
///
/// That conversion is also what makes the hash honest. Without it, a curve and the
/// polygon through the same points produced the same `M`/`L`/`Z` string and
/// therefore the same digest: two cuts with different areas and different
/// perimeters, one identity.
#[must_use]
pub fn to_path_data(contour: &Contour) -> String {
    let arcs = contour.resolved_arcs();
    let n = contour.points.len();

    let mut out = String::new();
    if let Some(first) = contour.points.first() {
        let q = quantise_point(*first);
        out.push_str(&format!("M {} {}", format_coord(q.x), format_coord(q.y)));
    }

    // All but the closing edge. The closing edge is a curve too, and emitting it
    // as part of the loop would duplicate what `Z` does for a polygon — changing
    // the canonical string, and therefore every hash, for geometry that has not
    // changed. So it is handled after the loop.
    for i in 0..n.saturating_sub(1) {
        let from = contour.points[i];
        let to = contour.points[i + 1];
        push_edge(&mut out, &arcs, i, from, to);
    }

    if contour.closed {
        let last = n - 1;
        let from = contour.points[last];
        let to = contour.points[0];
        if arcs.iter().any(|(arc, _, _)| arc.from == last) {
            push_edge(&mut out, &arcs, last, from, to);
        }
        out.push_str(" Z");
    }
    out
}

/// Append one `L` or `C` for one edge.
fn push_edge(
    out: &mut String,
    arcs: &[(crate::geom::ArcSegment, Point, Point)],
    edge: usize,
    from: Point,
    to: Point,
) {
    match arcs.iter().find(|(arc, _, _)| arc.from == edge) {
        Some((arc, _, _)) => {
            let (raw1, raw2, end) = bezier_control_points(arc, from, to);
            let (c1, c2, end) = (
                quantise_point(raw1),
                quantise_point(raw2),
                quantise_point(end),
            );
            out.push_str(&format!(
                " C {} {}, {} {}, {} {}",
                format_coord(c1.x),
                format_coord(c1.y),
                format_coord(c2.x),
                format_coord(c2.y),
                format_coord(end.x),
                format_coord(end.y)
            ));
        }
        None => {
            let q = quantise_point(to);
            out.push_str(&format!(" L {} {}", format_coord(q.x), format_coord(q.y)));
        }
    }
}

/// The two control points of the cubic Bézier that matches an arc exactly.
///
/// For a circular arc the standard relation holds: with `k = 4/3 · tan(θ/4)`, the
/// control points sit a distance `k · r` from the endpoints along the tangents.
fn bezier_control_points(
    arc: &crate::geom::ArcSegment,
    from: Point,
    to: Point,
) -> (Point, Point, Point) {
    let sweep = crate::geom::arc_sweep(arc, from, to);
    let k = (4.0 / 3.0) * (sweep / 4.0).tan();

    let (from_angle, to_angle) = (
        (from.y - arc.centre.y).atan2(from.x - arc.centre.x),
        (to.y - arc.centre.y).atan2(to.x - arc.centre.x),
    );
    // Tangent direction at each end is the radius rotated a quarter turn.
    let t1 = Point {
        x: from.x - k * arc.radius * from_angle.sin(),
        y: from.y + k * arc.radius * from_angle.cos(),
    };
    let t2 = Point {
        x: to.x + k * arc.radius * to_angle.sin(),
        y: to.y - k * arc.radius * to_angle.cos(),
    };
    (t1, t2, to)
}

fn format_coord(value: f64) -> String {
    format!("{value:.prec$}", prec = CANON_COORD_DP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Point;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    #[test]
    fn duplicate_points_are_dropped() {
        let points = vec![p(0.0, 0.0), p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)];
        let out = dedupe(&points, true);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn points_below_collinear_threshold_are_merged() {
        let points = vec![p(0.0, 0.0), p(1e-9, 0.0), p(10.0, 0.0)];
        let out = dedupe(&points, true);
        assert_eq!(out.len(), 2, "1e-9 mm is below {COLLINEAR_MM} mm");
    }

    #[test]
    fn closed_contour_does_not_gain_a_duplicate_tail() {
        let points = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 0.0)];
        let out = dedupe(&points, true);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn open_contour_keeps_first_and_last() {
        let points = vec![p(0.0, 0.0), p(10.0, 0.0)];
        let out = dedupe(&points, false);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn canonicalisation_is_idempotent() {
        let contour = Contour {
            points: vec![p(0.0, 0.0), p(0.0, 0.0), p(1.0 / 3.0, 0.7), p(1.0, 0.0)],
            closed: true,
            arcs: Vec::new(),
        };
        let once = canonicalise(&contour);
        let twice = canonicalise(&once);
        assert_eq!(once, twice, "canon(canon(g)) must equal canon(g)");
    }

    #[test]
    fn equivalent_encodings_hash_the_same_path() {
        // The same rectangle written with different spacing must produce
        // byte-identical path data.
        let a = Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)],
            closed: true,
            arcs: Vec::new(),
        };
        let b = Contour {
            points: vec![p(0.000_000_1, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)],
            closed: true,
            arcs: Vec::new(),
        };
        assert_eq!(
            to_path_data(&canonicalise(&a)),
            to_path_data(&canonicalise(&b))
        );
    }

    #[test]
    fn path_data_uses_absolute_commands_and_fixed_decimals() {
        let c = Contour {
            points: vec![p(0.5, 1.0), p(2.0, 1.0)],
            closed: true,
            arcs: Vec::new(),
        };
        let data = to_path_data(&c);
        assert_eq!(data, "M 0.5000 1.0000 L 2.0000 1.0000 Z");
    }

    #[test]
    fn open_contour_has_no_close_command() {
        let c = Contour {
            points: vec![p(0.0, 0.0), p(2.0, 1.0)],
            closed: false,
            arcs: Vec::new(),
        };
        assert!(!to_path_data(&c).contains('Z'));
    }
}
