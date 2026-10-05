//! Geometry primitives.
//!
//! Unit is millimetres throughout (ADR-0004). `NaN` and `Inf` are forbidden in
//! stored geometry; construction rejects them rather than propagating them.

use serde::{Deserialize, Serialize};

/// Maximum number of points accepted in a single contour (MASK_PROJECT §8).
pub const MAX_POINTS_PER_PATH: usize = 20_000;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum GeometryError {
    #[error("coordinate {index} is not finite (value: {value})")]
    NonFinite { index: usize, value: String },

    #[error("coordinate {value} mm exceeds the {limit} mm bound")]
    CoordinateOutOfBounds { value: f64, limit: f64 },

    #[error("contour has {count} points, limit is {limit}")]
    TooManyPoints { count: usize, limit: usize },

    #[error("contour needs at least {min} distinct points")]
    TooFewPoints { min: usize },

    #[error("invalid fill rule: {0}")]
    InvalidFillRule(String),

    #[error("hole {index} winds the same way as its outer contour; holes must be clockwise")]
    HoleOrientation { index: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    /// Build a point, rejecting non-finite values and out-of-bounds ones.
    ///
    /// ADR-0003 finding N-3: the geometry library accepts coordinates up to
    /// 1e12 mm, so bounds are enforced here rather than trusted downstream.
    pub fn new(x: f64, y: f64, limit: f64) -> Result<Self, GeometryError> {
        for (index, value) in [x, y].iter().enumerate() {
            if !value.is_finite() {
                return Err(GeometryError::NonFinite {
                    index,
                    value: format!("{value}"),
                });
            }
            if value.abs() > limit {
                return Err(GeometryError::CoordinateOutOfBounds {
                    value: *value,
                    limit,
                });
            }
        }
        Ok(Self { x, y })
    }

    #[must_use]
    pub fn distance_to(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// A single closed or open boundary, stored as a point ring.
///
/// `closed` is carried explicitly: an open contour is meaningful for score and
/// cut lines and must not be silently closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contour {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl Contour {
    /// Validate structure without changing the points.
    ///
    /// This does not deduplicate points or remove degenerate segments; that is
    /// canonicalisation's job and is a separate, explicit step. Keeping them
    /// apart means a validation failure can never silently repair geometry.
    pub fn validate(&self, limit: f64) -> Result<(), GeometryError> {
        if self.points.len() > MAX_POINTS_PER_PATH {
            return Err(GeometryError::TooManyPoints {
                count: self.points.len(),
                limit: MAX_POINTS_PER_PATH,
            });
        }
        for p in &self.points {
            Point::new(p.x, p.y, limit)?;
        }
        let min = if self.closed { 3 } else { 2 };
        if self.points.len() < min {
            return Err(GeometryError::TooFewPoints { min });
        }
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// One connected region: the first contour is the outer boundary, the rest are
/// holes.
///
/// Orientation is an invariant, not a convention: outer contours are
/// counter-clockwise and holes clockwise. That is what i_overlay guarantees and
/// what makes signed area correct. See ADR-0003 finding N-6.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub outer: Contour,
    pub holes: Vec<Contour>,
}

impl Shape {
    #[must_use]
    pub fn new(outer: Contour) -> Self {
        Self {
            outer,
            holes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_holes(outer: Contour, holes: Vec<Contour>) -> Self {
        Self { outer, holes }
    }

    pub fn contours(&self) -> impl Iterator<Item = &Contour> {
        std::iter::once(&self.outer).chain(self.holes.iter())
    }

    #[must_use]
    pub fn contour_count(&self) -> usize {
        1 + self.holes.len()
    }

    pub fn validate(&self, limit: f64) -> Result<(), GeometryError> {
        self.outer.validate(limit)?;
        for hole in &self.holes {
            hole.validate(limit)?;
        }
        Ok(())
    }

    /// Check that holes wind opposite to the outer contour.
    ///
    /// Signed area is the whole basis of subtraction: outer counter-clockwise is
    /// positive, holes clockwise are negative, and summing them removes the hole.
    /// A hole wound the same way adds instead, so a shape with one such hole
    /// reports the wrong area while looking perfectly well formed.
    ///
    /// This was found by a test in the web facade, not by the kernel: a hand-built
    /// shape whose hole shared the outer's winding returned 500 mm² for a figure
    /// whose true area is 300 mm². Nothing complained, because a wrong area is
    /// exactly the kind of error that hides — it flows into fidelity, into
    /// material estimates and into a manifest that then records the wrong number.
    pub fn validate_orientation(&self) -> Result<(), GeometryError> {
        let outer_area = crate::measure::signed_contour_area(&self.outer);
        if outer_area.abs() <= crate::constants::GEOM_EPS_MM {
            // Degenerate outer contour: orientation carries no information.
            return Ok(());
        }
        let outer_is_ccw = outer_area > 0.0;

        for (index, hole) in self.holes.iter().enumerate() {
            let hole_area = crate::measure::signed_contour_area(hole);
            if hole_area.abs() <= crate::constants::GEOM_EPS_MM {
                continue;
            }
            if (hole_area > 0.0) == outer_is_ccw {
                return Err(GeometryError::HoleOrientation { index });
            }
        }
        Ok(())
    }
}

/// A set of disjoint shapes, i.e. the operand or result of a boolean operation.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Shapes {
    pub shapes: Vec<Shape>,
}

impl Shapes {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn single(shape: Shape) -> Self {
        Self {
            shapes: vec![shape],
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    #[must_use]
    pub fn contour_count(&self) -> usize {
        self.shapes.iter().map(Shape::contour_count).sum()
    }

    pub fn validate(&self, limit: f64) -> Result<(), GeometryError> {
        for shape in &self.shapes {
            shape.validate(limit)?;
        }
        Ok(())
    }

    /// Validate structure and hole orientation for every region.
    pub fn validate_strict(&self, limit: f64) -> Result<(), GeometryError> {
        self.validate(limit)?;
        for shape in &self.shapes {
            shape.validate_orientation()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: f64 = 100_000.0;

    #[test]
    fn point_rejects_nan() {
        let err = Point::new(f64::NAN, 0.0, LIMIT).unwrap_err();
        assert!(matches!(err, GeometryError::NonFinite { index: 0, .. }));
    }

    #[test]
    fn point_rejects_out_of_bounds() {
        let err = Point::new(1e12, 0.0, LIMIT).unwrap_err();
        assert!(matches!(err, GeometryError::CoordinateOutOfBounds { .. }));
    }

    #[test]
    fn point_accepts_normal_geometry() {
        let p = Point::new(1412.0, 1046.0, LIMIT).unwrap();
        assert!((p.x - 1412.0).abs() < f64::EPSILON);
    }

    #[test]
    fn contour_needs_three_points_when_closed() {
        let c = Contour {
            points: vec![Point { x: 0.0, y: 0.0 }, Point { x: 1.0, y: 0.0 }],
            closed: true,
        };
        assert_eq!(
            c.validate(LIMIT),
            Err(GeometryError::TooFewPoints { min: 3 })
        );
    }

    #[test]
    fn a_hole_wound_the_same_way_as_the_outer_contour_is_rejected() {
        // Both rings counter-clockwise, so the hole would add instead of subtract:
        // a 20x20 outer with a 10x10 hole reports 500 mm² instead of 300 mm².
        let ccw = |x: f64, y: f64, w: f64, h: f64| Contour {
            points: vec![
                Point { x, y },
                Point { x: x + w, y },
                Point { x: x + w, y: y + h },
                Point { x, y: y + h },
            ],
            closed: true,
        };
        let bad = Shape::with_holes(ccw(0.0, 0.0, 20.0, 20.0), vec![ccw(5.0, 5.0, 10.0, 10.0)]);
        assert_eq!(
            bad.validate_orientation(),
            Err(GeometryError::HoleOrientation { index: 0 })
        );
        // The lenient check passes it, which is why strict validation exists.
        assert!(bad.validate(1e6).is_ok());
    }

    #[test]
    fn a_correctly_wound_hole_passes() {
        let ccw = |x: f64, y: f64, w: f64, h: f64| Contour {
            points: vec![
                Point { x, y },
                Point { x: x + w, y },
                Point { x: x + w, y: y + h },
                Point { x, y: y + h },
            ],
            closed: true,
        };
        let cw = |x: f64, y: f64, w: f64, h: f64| Contour {
            points: vec![
                Point { x, y },
                Point { x, y: y + h },
                Point { x: x + w, y: y + h },
                Point { x: x + w, y },
            ],
            closed: true,
        };
        let good = Shape::with_holes(ccw(0.0, 0.0, 20.0, 20.0), vec![cw(5.0, 5.0, 10.0, 10.0)]);
        assert!(good.validate_orientation().is_ok());
    }

    #[test]
    fn a_degenerate_outer_contour_does_not_fail_orientation() {
        let triangle = Shape::with_holes(
            Contour {
                points: vec![Point { x: 0.0, y: 0.0 }, Point { x: 0.0, y: 0.0 }],
                closed: true,
            },
            vec![Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 1.0, y: 0.0 },
                    Point { x: 1.0, y: 1.0 },
                ],
                closed: true,
            }],
        );
        // Orientation carries no information for a zero-area outer, so this must
        // not be reported as a mis-wound hole.
        assert!(triangle.validate_orientation().is_ok());
    }

    #[test]
    fn shape_counts_its_holes() {
        let rect = |y: f64| Contour {
            points: vec![
                Point { x: 0.0, y },
                Point { x: 1.0, y },
                Point { x: 1.0, y: y + 1.0 },
                Point { x: 0.0, y: y + 1.0 },
            ],
            closed: true,
        };
        let shape = Shape::with_holes(rect(0.0), vec![rect(0.2), rect(0.4)]);
        assert_eq!(shape.contour_count(), 3);
    }
}
