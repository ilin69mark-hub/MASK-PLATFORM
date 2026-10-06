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
