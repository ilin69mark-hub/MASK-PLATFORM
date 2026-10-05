//! Canonical form. ADR-0004.
//!
//! Two encodings of the same geometry must produce one `content_hash`, or the
//! metrics cache and the reproduction `verify` mode both report false
//! mismatches. That is why this module exists and why it is not optional.

use crate::constants::{CANON_COORD_DP, COLLINEAR_MM};
use crate::geom::{Contour, Point, Shape, Shapes};

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

/// Canonicalise a contour: quantise, then dedupe.
#[must_use]
pub fn canonicalise(contour: &Contour) -> Contour {
    let points = dedupe(
        &contour
            .points
            .iter()
            .copied()
            .map(quantise_point)
            .collect::<Vec<_>>(),
        contour.closed,
    );
    Contour {
        points,
        closed: contour.closed,
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

/// Emit absolute `M` / `L` / `Z` commands only.
///
/// Relative, shorthand and arc commands are forbidden by ADR-0004: different
/// writers spell the same path differently, and the hash must not care.
#[must_use]
pub fn to_path_data(contour: &Contour) -> String {
    let mut out = String::new();
    for (i, p) in contour.points.iter().enumerate() {
        let q = quantise_point(*p);
        let x = format_coord(q.x);
        let y = format_coord(q.y);
        if i == 0 {
            out.push_str(&format!("M {x} {y}"));
        } else {
            out.push_str(&format!(" L {x} {y}"));
        }
    }
    if contour.closed {
        out.push_str(" Z");
    }
    out
}

/// Format a coordinate with exactly `CANON_COORD_DP` decimals.
///
/// Fixed decimals rather than trimmed floats: `0.5` and `0.5000` are the same
/// point but different bytes.
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
        };
        let b = Contour {
            points: vec![p(0.000_000_1, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)],
            closed: true,
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
        };
        let data = to_path_data(&c);
        assert_eq!(data, "M 0.5000 1.0000 L 2.0000 1.0000 Z");
    }

    #[test]
    fn open_contour_has_no_close_command() {
        let c = Contour {
            points: vec![p(0.0, 0.0), p(2.0, 1.0)],
            closed: false,
        };
        assert!(!to_path_data(&c).contains('Z'));
    }
}
