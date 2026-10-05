//! Offsetting and narrowness measurement.
//!
//! Offsetting is delegated to `i_overlay`. Narrowness measurement is **ours**,
//! deliberately: ADR-0003 finding N-4 showed that stroking does not collapse a
//! too-thin element, so a mask layer 0.42 mm wide is happily returned as a
//! perfectly valid 0.42 mm band. Only measurement against the profile threshold
//! can decide that it is unmanufacturable.

use crate::boolean::OverlayError;
use crate::constants::MAX_COORDINATE_MM;
use crate::geom::{Contour, Point, Shape, Shapes};
use crate::measure::bounding_box;

use i_overlay::mesh::float::outline::offset::OutlineOffset;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, OutlineStyle, StrokeStyle};
use i_overlay::mesh::math::MathMode;

type Pt = [f64; 2];

/// Number of binary-search iterations for width detection.
///
/// 28 iterations over a 10 000 mm range resolves far below 1e-4 mm, which is
/// the canonical grid. More steps would not change any printed digit.
const WIDTH_SEARCH_STEPS: u32 = 28;

/// Pinned globally, never chosen per call (ADR-0003 N-1, ADR-0004 D5).
#[must_use]
pub fn lib_math_mode() -> MathMode {
    MathMode::Integer
}

fn to_shapes(input: &Shapes) -> Vec<Vec<Vec<Pt>>> {
    input
        .shapes
        .iter()
        .map(|shape| {
            shape
                .contours()
                .map(|c| c.points.iter().map(|p| [p.x, p.y]).collect())
                .collect()
        })
        .collect()
}

fn from_shapes(raw: Vec<Vec<Vec<Pt>>>) -> Shapes {
    Shapes {
        shapes: raw
            .into_iter()
            .map(|contours| {
                let mut it = contours.into_iter().map(|pts| Contour {
                    points: pts.into_iter().map(|[x, y]| Point { x, y }).collect(),
                    closed: true,
                });
                let outer = it.next().unwrap_or(Contour {
                    points: Vec::new(),
                    closed: true,
                });
                Shape {
                    outer,
                    holes: it.collect(),
                }
            })
            .collect(),
    }
}

fn check_finite(points: &[Point]) -> Result<(), OverlayError> {
    for p in points {
        if !p.is_finite() {
            return Err(OverlayError::InvalidGeometry(
                "non-finite coordinate".into(),
            ));
        }
        if p.x.abs() > MAX_COORDINATE_MM || p.y.abs() > MAX_COORDINATE_MM {
            return Err(OverlayError::OutOfBounds {
                limit: MAX_COORDINATE_MM,
            });
        }
    }
    Ok(())
}

/// Widen a centreline to a band of the requested width.
///
/// This is the Auto-Fix `widen` operation (TOPOLOGY_ENGINE §6.1): an element
/// measured at 0.42 mm is re-stroked at 0.80 mm.
///
/// ADR-0003 measured this operation's accuracy at about 8e-8 mm, three orders
/// of magnitude below the canonical grid.
pub fn widen(centreline: &[Point], target_width_mm: f64) -> Result<Shapes, OverlayError> {
    if centreline.len() < 2 {
        return Err(OverlayError::InvalidGeometry(
            "a centreline needs at least two points".into(),
        ));
    }
    if !target_width_mm.is_finite() || target_width_mm <= 0.0 {
        return Err(OverlayError::InvalidGeometry(
            "target width must be positive and finite".into(),
        ));
    }
    check_finite(centreline)?;

    let path: Vec<Pt> = centreline.iter().map(|p| [p.x, p.y]).collect();
    let style = StrokeStyle::new(target_width_mm)
        .line_join(LineJoin::Miter(1.0))
        .start_cap(LineCap::Butt)
        .end_cap(LineCap::Butt)
        .math(lib_math_mode());
    Ok(from_shapes(path.stroke(style, false)))
}

/// Shrink (negative) or grow (positive) a closed shape.
pub fn offset_shape(shape: &Shapes, delta_mm: f64) -> Result<Shapes, OverlayError> {
    if !delta_mm.is_finite() {
        return Err(OverlayError::InvalidGeometry(
            "offset must be finite".into(),
        ));
    }
    for p in shape
        .shapes
        .iter()
        .flat_map(|s| s.contours())
        .flat_map(|c| &c.points)
    {
        check_finite(std::slice::from_ref(p))?;
    }
    let style = OutlineStyle::new(delta_mm).line_join(LineJoin::Miter(1.0));
    Ok(from_shapes(to_shapes(shape).outline(&style)))
}

/// Whether a shape survives being shrunk inward by `half_width_mm`.
#[must_use]
pub fn survives_inward_offset(shape: &Shapes, half_width_mm: f64) -> bool {
    offset_shape(shape, -half_width_mm)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

/// Narrowest width of the shape, in millimetres.
///
/// A mask element is a band of film of uniform thickness, so for a band that
/// thickness is the shortest distance across it. The band collapses once the
/// inward offset exceeds half its thickness, so the width is found by bisecting
/// on the offset delta at which the result becomes empty, then doubling.
///
/// Bisection is used rather than a direct computation because it needs no
/// assumption about the shape: a distance-field approach would be faster but
/// only as accurate as the raster grid, and this must hold to the canonical
/// grid (ADR-0004).
///
/// Returns `None` for empty input.
#[must_use]
pub fn min_width(shape: &Shapes) -> Option<f64> {
    if shape.is_empty() {
        return None;
    }

    // Search domain: half of the longest bounding-box side. A shape cannot
    // survive an inward offset larger than that, so this is a safe ceiling.
    let (_, _, w, h) = bounding_box(shape);
    let ceiling = w.max(h) / 2.0;

    let mut lo = 0.0_f64; // known to survive
    let mut hi = ceiling; // may or may not survive yet

    // If the ceiling still survives, it was not a ceiling. Double until it
    // fails rather than assuming the bound holds.
    let mut guard = 0;
    while survives_inward_offset(shape, hi) && guard < 64 {
        hi *= 2.0;
        guard += 1;
    }

    for _ in 0..WIDTH_SEARCH_STEPS {
        let mid = (lo + hi) / 2.0;
        if survives_inward_offset(shape, mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }

    Some(lo * 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::shapes_area;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Shapes {
        Shapes::single(Shape::new(Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + h), p(x, y + h)],
            closed: true,
        }))
    }

    #[test]
    fn widen_hits_the_requested_width_exactly() {
        let line = vec![p(0.0, 0.0), p(100.0, 0.0)];
        let band = widen(&line, 0.80).unwrap();
        let measured = shapes_area(&band) / 100.0;
        assert!((measured - 0.80).abs() < 1e-6, "measured {measured} mm");
    }

    #[test]
    fn widen_round_trips_measured_width() {
        for target in [0.80, 0.42, 1.60, 2.50] {
            let line = vec![p(0.0, 0.0), p(200.0, 0.0)];
            let band = widen(&line, target).unwrap();
            let measured = min_width(&band).expect("band is non-empty");
            assert!(
                (measured - target).abs() < 1e-3,
                "target {target} mm measured as {measured} mm"
            );
        }
    }

    #[test]
    fn min_width_of_a_known_band_is_that_band() {
        assert!((min_width(&rect(0.0, 0.0, 50.0, 1.0)).unwrap() - 1.0).abs() < 1e-3);
        assert!((min_width(&rect(0.0, 0.0, 50.0, 0.42)).unwrap() - 0.42).abs() < 1e-3);
    }

    #[test]
    fn min_width_of_a_square_is_its_side() {
        assert!((min_width(&rect(0.0, 0.0, 10.0, 10.0)).unwrap() - 10.0).abs() < 1e-3);
    }

    #[test]
    fn min_width_reports_none_for_empty_input() {
        assert_eq!(min_width(&Shapes::new()), None);
    }

    #[test]
    fn a_band_below_the_threshold_is_detected_as_narrow() {
        // The physical claim: 0.42 mm film cannot be cut and held for a
        // profile demanding 0.80 mm. Nothing else in the stack will notice.
        let band = widen(&[p(0.0, 0.0), p(100.0, 0.0)], 0.42).unwrap();
        let measured = min_width(&band).unwrap();
        assert!(
            measured < 0.80,
            "measured {measured} mm must fail a 0.80 mm threshold"
        );
    }

    #[test]
    fn a_band_at_the_threshold_passes() {
        let band = widen(&[p(0.0, 0.0), p(100.0, 0.0)], 0.80).unwrap();
        assert!(min_width(&band).unwrap() >= 0.80 - crate::constants::MIN_WIDTH_EPS_MM);
    }

    #[test]
    fn widen_rejects_degenerate_input() {
        assert!(widen(&[p(0.0, 0.0)], 0.8).is_err());
        assert!(widen(&[p(0.0, 0.0), p(1.0, 0.0)], 0.0).is_err());
        assert!(widen(&[p(0.0, 0.0), p(1.0, 0.0)], f64::NAN).is_err());
    }

    #[test]
    fn out_of_bounds_is_rejected() {
        let absurd = vec![p(0.0, 0.0), p(1.0e12, 0.0)];
        assert!(matches!(
            widen(&absurd, 0.8),
            Err(OverlayError::OutOfBounds { .. })
        ));
    }

    #[test]
    fn narrowing_band_never_collapses_on_its_own() {
        // N-4 made concrete: the library returns a valid, unusable band.
        let hairline = widen(&[p(0.0, 0.0), p(100.0, 0.0)], 0.000_01).unwrap();
        assert!(!hairline.is_empty());
        assert!(min_width(&hairline).unwrap() < 0.80);
    }
}
