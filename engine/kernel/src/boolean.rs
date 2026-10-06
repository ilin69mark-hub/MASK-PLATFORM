//! Boolean operations and offsetting, delegated to `i_overlay`.
//!
//! Everything here follows ADR-0003 and the amendment to ADR-0004:
//!
//! * only the fixed-scale, `Result`-returning variants are used, because the
//!   infallible ones panic on out-of-range bounds (finding N-2);
//! * the integer engine is `i64` at scale `1e4`, matching `CANON_QUANTUM_MM`
//!   (amendment, §1b);
//! * `MathMode` is pinned by the caller in one place, never chosen per call,
//!   because Integer and Float diverge on rounded joins (finding N-1).

use crate::constants::{LIB_SCALE, MAX_COORDINATE_MM};
use crate::geom::{Contour, Point, Shape, Shapes};

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::scale::FixedScaleFloatOverlay;

#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    #[error("geometry exceeds the {limit} mm coordinate bound")]
    OutOfBounds { limit: f64 },

    #[error("geometry library rejected the input: {0}")]
    Rejected(String),

    #[error("geometry is invalid: {0}")]
    InvalidGeometry(String),

    /// The operation works on polygons only, and would otherwise have quietly
    /// reported the answer for the chords.
    ///
    /// The message names where the tolerance comes from instead of quoting a
    /// number: the kernel does not own the profile, and a literal would go stale
    /// the first time a blade was measured differently.
    #[error(
        "this operation is polygonal and the input still carries curves; flatten it \
         first with shapes.flattened(tolerance_mm), taking the tolerance from your \
         plotter profile's curve tolerance"
    )]
    CurvedGeometry,
}

/// Fill rule used for mask geometry.
///
/// NonZero is correct for physical masks: it keeps a shape solid when outer
/// and hole contours wind the same way, which is what happens after a
/// careless editor operation. EvenOdd would punch unintended holes.
const FILL_RULE: FillRule = FillRule::NonZero;

/// Library coordinate type, matching our f64 millimetres.
type Pt = [f64; 2];

/// Reject coordinates the library would happily accept but we never mean to.
///
/// ADR-0003 N-3: 1e12 mm was accepted during the spike. Bounds are our
/// responsibility, not the library's.
fn check_bounds(shapes: &Shapes) -> Result<(), OverlayError> {
    for p in shapes
        .shapes
        .iter()
        .flat_map(|s| s.contours())
        .flat_map(|c| &c.points)
    {
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

fn to_shapes(subj: &Shapes) -> Vec<Vec<Vec<Pt>>> {
    subj.shapes
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
                    // The library hands back polygons. A curve has to be flattened
                    // before it reaches here and never survives a boolean
                    // operation.
                    arcs: Vec::new(),
                });
                let outer = it.next().unwrap_or(Contour {
                    points: Vec::new(),
                    closed: true,
                    arcs: Vec::new(),
                });
                Shape {
                    outer,
                    holes: it.collect(),
                }
            })
            .collect(),
    }
}

fn run(a: &Shapes, b: &Shapes, rule: OverlayRule) -> Result<Shapes, OverlayError> {
    a.require_polygonal()?;
    b.require_polygonal()?;
    check_bounds(a)?;
    check_bounds(b)?;
    let result =
        to_shapes(a).overlay_with_fixed_scale_as::<i64>(&to_shapes(b), rule, FILL_RULE, LIB_SCALE);
    match result {
        Ok(shapes) => Ok(from_shapes(shapes)),
        Err(e) => Err(OverlayError::Rejected(format!("{e:?}"))),
    }
}

/// Union of two shape sets.
pub fn union(a: &Shapes, b: &Shapes) -> Result<Shapes, OverlayError> {
    run(a, b, OverlayRule::Union)
}

/// Difference `a - b`.
pub fn difference(a: &Shapes, b: &Shapes) -> Result<Shapes, OverlayError> {
    run(a, b, OverlayRule::Difference)
}

/// Intersection `a ∩ b`.
pub fn intersection(a: &Shapes, b: &Shapes) -> Result<Shapes, OverlayError> {
    run(a, b, OverlayRule::Intersect)
}

/// Symmetric difference.
pub fn xor(a: &Shapes, b: &Shapes) -> Result<Shapes, OverlayError> {
    run(a, b, OverlayRule::Xor)
}

/// Normalise a single shape set: resolve self-intersections, drop degeneracy and
/// merge collinear edges.
///
/// This is what makes a hand-authored or AI-proposed contour valid before it is
/// measured. It is deliberately a boolean union against an empty set, so the
/// same engine rules apply everywhere.
pub fn normalise(shapes: &Shapes) -> Result<Shapes, OverlayError> {
    run(shapes, &Shapes::new(), OverlayRule::Union)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{ArcSegment, Contour, Point};

    /// A circle of four quarter arcs.
    fn curved_circle() -> Shapes {
        let r = 10.0;
        let points = (0..4)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (f64::from(i));
                Point {
                    x: r * a.cos(),
                    y: r * a.sin(),
                }
            })
            .collect();
        let arcs = (0..4)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: Point { x: 0.0, y: 0.0 },
                radius: r,
                ccw: true,
            })
            .collect();
        Shapes::single(Shape::new(Contour {
            points,
            closed: true,
            arcs,
        }))
    }

    /// The trap this closes.
    ///
    /// Every boolean ran on `.points`. Hand it a circle and it cheerfully returned
    /// the union of the inscribed squares — a number, a shape, and the wrong one,
    /// with nothing anywhere saying so.
    #[test]
    fn a_boolean_refuses_curves_instead_of_reporting_the_chords() {
        let circle = curved_circle();
        let err = union(&circle, &circle).expect_err("union must not silently flatten");
        assert!(
            matches!(err, OverlayError::CurvedGeometry),
            "unexpected error: {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains("flattened"),
            "the error must say what to do: {msg}"
        );
        assert!(
            msg.contains("profile"),
            "the error must say where the tolerance comes from: {msg}"
        );
    }

    /// Flattening first is the supported path, and it behaves as the geometry says.
    ///
    /// Inscribing a circle in a regular polygon: with sagitta `t` on radius `r` the
    /// segment angle satisfies `t = r(1 - cos(θ/2))`, so `θ ≈ sqrt(8t/r)`, and a
    /// regular n-gon falls short of the circle by `θ²/6` in relative area. Together
    /// that is `4t / 3r`.
    ///
    /// The test asserts against that relation rather than a number picked to pass,
    /// because the relation is the thing that has to hold. It also makes the cost of
    /// the profile's default visible: `curve_tolerance_mm = 0.08` on a 10 mm radius
    /// is about 1% of area, which is fine for a mask and would not be fine for a
    /// tolerance-critical measurement.
    #[test]
    fn flattening_costs_about_four_t_over_three_r() {
        let r = 10.0;
        let circle = curved_circle();
        let truth = std::f64::consts::PI * r * r;
        let area_at = |tol: f64| {
            let flat = circle.flattened(tol);
            crate::measure::shapes_area(&union(&flat, &flat).expect("polygonal union succeeds"))
        };

        // The relation is a linearisation, so it is only claimed where the
        // linearisation holds. At t/r = 0.05 it overstates the loss by half; that
        // is the approximation failing, not the flattener, and pretending otherwise
        // would put a wrong constant in the profile.
        for tol in [0.05_f64, 0.01, 0.001] {
            let measured = area_at(tol);
            let predicted_relative_loss = 4.0 * tol / (3.0 * r);
            let measured_relative_loss = (truth - measured) / truth;
            let ratio = measured_relative_loss / predicted_relative_loss;
            assert!(
                (0.9..1.15).contains(&ratio),
                "at t={tol} the loss should track 4t/3r = {predicted_relative_loss:.6}, \
                 measured {measured_relative_loss:.6} (ratio {ratio:.3})"
            );
        }

        // Outside that range the approximation is only known to be an overstatement,
        // so the claim is the weaker one that still matters: better than nothing,
        // and monotonically better as the tolerance tightens.
        let coarse = area_at(0.5);
        assert!(
            coarse > 0.9 * truth,
            "0.5 mm should still be within 10%: got {coarse}"
        );

        // The chord polygon is what this whole refactor stopped doing. It has to be
        // built explicitly, because `shapes_area` now measures the curve correctly —
        // which is the point, and also why it can no longer be used to demonstrate how
        // wrong the chords were.
        let chords = crate::measure::shapes_area(&Shapes::single(Shape::new(Contour {
            points: curved_circle().shapes[0].outer.points.clone(),
            closed: true,
            arcs: Vec::new(),
        })));
        assert!(
            chords < 0.7 * truth,
            "the chord polygon should still be visibly wrong, or this test has stopped meaning anything"
        );
        assert!(
            area_at(0.01) > 1.5 * chords,
            "one millimetre of tolerance must be worth far more than the four chords"
        );
    }

    /// And the guard must not fire on geometry that has no curves, or it is just a
    /// tax on correct callers.
    #[test]
    fn polygonal_input_is_unaffected() {
        let square = Shapes::single(Shape::new(Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 10.0 },
                Point { x: 0.0, y: 10.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        }));
        assert!(union(&square, &square).is_ok());
    }
}
