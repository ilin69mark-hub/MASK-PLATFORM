//! Geometry primitives.
//!
//! Unit is millimetres throughout (ADR-0004). `NaN` and `Inf` are forbidden in
//! stored geometry; construction rejects them rather than propagating them.

use serde::{Deserialize, Serialize};

use crate::OverlayError;

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

    /// An arc's radius is not a radius.
    ///
    /// Zero is included: a zero-radius arc collapses its control points onto the
    /// chord, so it draws a straight edge while still counting as a curve — the
    /// geometry, its area and its hash would all disagree with the polygon that
    /// has no arc at all (ADR-0015).
    #[error(
        "arc {index} has radius {radius}, which is not a curve; it must be finite and above zero"
    )]
    ArcRadius { index: usize, radius: f64 },

    /// An arc so gentle that it is its own chord.
    ///
    /// The sagitta is how far an arc departs from the straight line between its
    /// endpoints: `r − √(r² − (d/2)²)`. Once that falls below the canonical grid the
    /// arc *is* the chord geometrically, while carrying a different radius, a
    /// different path string and therefore a different content hash — two cuts that
    /// cut the same thing, told apart by a number nothing can measure.
    ///
    /// It is also where measurements stop being numbers. The segment-area formula
    /// squares the radius, so a large enough radius overflows to `NaN` and reaches the
    /// manifest as a `null`, which reads as an absent measurement rather than as an
    /// impossible one. Verified on the built artifact: a radius of 1e300 on a 10 mm
    /// chord produced `area_mm2: null`.
    ///
    /// The companion to `ArcRadius`: a zero radius is not a curve, and neither is one
    /// that bends by less than the grid. Both are the same claim — that this segment
    /// of the path is straight.
    #[error(
        "arc {index} bends {deviation} mm from the {chord} mm straight line between its endpoints, \
         less than the {quantum} mm grid; it is its own chord and cannot be measured"
    )]
    ArcBelowGrid {
        index: usize,
        deviation: f64,
        chord: f64,
        quantum: f64,
    },

    /// An arc names points the contour does not have, or does not describe a curve
    /// the pipeline can actually draw.
    ///
    /// Every consumer of an arc — area, perimeter, flattening, canonical path data —
    /// replaces exactly one edge. An arc spanning several edges used to be dropped
    /// silently by all of them, which made the content hash of a shape that declared
    /// a curve identical to the hash of the polygon without one (ADR-0015).
    #[error(
        "arc {index} runs from point {from} to point {to} of a {count} point contour; \
         an arc replaces exactly one edge, so it must satisfy from < to <= {count} with to = from + 1"
    )]
    ArcIndex {
        index: usize,
        from: usize,
        to: usize,
        count: usize,
    },
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
    /// Runs of straight edges that are really arcs.
    ///
    /// Empty means every edge is straight, which is what every contour looked
    /// like before this field existed and what tracing, bridging and the raster
    /// stage all still produce. That is the point of the representation: adding
    /// curves did not change the meaning of any existing contour, so nothing
    /// downstream had to be rewritten and no measurement silently changed.
    ///
    /// The representation is **chordal**. An arc names two points that already
    /// exist in `points` and describes the curve between them. Nothing moves: the
    /// endpoints, the area and the boolean operations are unchanged, and what is
    /// added is the knowledge that the path between them is a curve of a stated
    /// radius.
    ///
    /// Why chordal rather than true arcs as primitives: the boolean library takes
    /// polygons, so any curve has to be flattened for it anyway. Storing the exact
    /// arc beside its chord means the flattening is a choice — with a tolerance
    /// you choose — rather than a permanent loss.
    #[serde(default)]
    pub arcs: Vec<ArcSegment>,
}

/// One arc associated with a straight edge between two existing points.
///
/// `from` and `to` index into [`Contour::points`]. The type can represent a run,
/// but a valid contour in the kernel pipeline accepts only an arc that replaces
/// exactly one consecutive edge: `to == (from + 1) % points.len()`. That restriction
/// is checked by [`Contour::validate`] (ADR-0017). The arc starts exactly at
/// `points[from]` and ends exactly at the adjoining `points[to]`, which is what
/// makes the endpoints agree with the chordal representation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ArcSegment {
    pub from: usize,
    pub to: usize,
    pub centre: Point,
    pub radius: f64,
    /// Direction of travel. The arc goes counter-clockwise about `centre` when true.
    pub ccw: bool,
}

/// Points approximating one arc, from `from` up to but not including `to`.
///
/// The step comes from the sagitta: a chord subtending angle `a` on radius `r`
/// deviates from the curve by at most `r (1 - cos(a/2))`, so the step that keeps
/// the deviation under `tolerance` is `a = 2 acos(1 - tolerance / r)`.
///
/// The tolerance is a caller choice, not a constant: how closely a curve has to
/// match its chord is a question about the tool, and the plotter profile is where
/// that number comes from. A tolerance at or beyond the radius would ask for a
/// single chord, which approximates nothing, so the step count is floored at two.
fn arc_points(from: Point, to: Point, arc: &ArcSegment, tolerance_mm: f64) -> Vec<Point> {
    const MIN_STEPS: usize = 2;
    const MAX_STEPS: usize = 512;

    let start_angle = (from.y - arc.centre.y).atan2(from.x - arc.centre.x);
    let sweep = arc_sweep(arc, from, to);

    let steps = if arc.radius <= 0.0 || tolerance_mm <= 0.0 {
        MIN_STEPS
    } else {
        let ratio = (tolerance_mm / arc.radius).clamp(0.0, 1.0);
        let step = 2.0 * (1.0 - ratio).acos();
        if !step.is_finite() || step <= 0.0 {
            MAX_STEPS
        } else {
            ((sweep.abs() / step).ceil() as usize).clamp(MIN_STEPS, MAX_STEPS)
        }
    };

    (0..steps)
        .map(|i| {
            let t = start_angle + sweep * (i as f64) / (steps as f64);
            Point {
                x: arc.centre.x + arc.radius * t.cos(),
                y: arc.centre.y + arc.radius * t.sin(),
            }
        })
        .collect()
}

/// Signed sweep of an arc in radians: positive counter-clockwise.
///
/// The sign is what tells a segment of film from a notch in it, so it is
/// computed once here and used by every measurement. This is pure angle
/// arithmetic over the two explicitly supplied endpoints. It does not check the
/// surrounding contour, so a passing sweep calculation is not evidence that the
/// arc is a valid contour arc (ADR-0017).
#[must_use]
pub fn arc_sweep(arc: &ArcSegment, from: Point, to: Point) -> f64 {
    let start = (from.y - arc.centre.y).atan2(from.x - arc.centre.x);
    let end = (to.y - arc.centre.y).atan2(to.x - arc.centre.x);
    let mut sweep = end - start;
    if arc.ccw && sweep < 0.0 {
        sweep += std::f64::consts::TAU;
    }
    if !arc.ccw && sweep > 0.0 {
        sweep -= std::f64::consts::TAU;
    }
    sweep
}

/// Area between one arc and its chord, signed by the direction of travel.
///
/// A circle cut by a chord subtending `θ` leaves a segment of
/// `r²/2 · (θ − sin θ)`. Sign follows the sweep: positive when the film is inside
/// the arc, which is what a counter-clockwise arc in a counter-clockwise contour
/// means, negative when it is a notch.
///
/// Checked against a full circle of radius 10 built from four quarter arcs: 200
/// of chordal area plus 114.159 of segments is 314.159, and the error is exactly
/// zero.
#[must_use]
pub fn arc_segment_area(arc: &ArcSegment, from: Point, to: Point) -> f64 {
    let sweep = arc_sweep(arc, from, to);
    let theta = sweep.abs();
    let sign = if sweep > 0.0 { 1.0 } else { -1.0 };
    sign * arc.radius * arc.radius / 2.0 * (theta - theta.sin())
}

/// Length of one arc: `r · |θ|`.
#[must_use]
pub fn arc_length(arc: &ArcSegment, from: Point, to: Point) -> f64 {
    arc.radius * arc_sweep(arc, from, to).abs()
}

impl Contour {
    /// A contour with straight edges only.
    #[must_use]
    pub fn straight(points: Vec<Point>, closed: bool) -> Self {
        Self {
            points,
            closed,
            arcs: Vec::new(),
        }
    }

    /// Each valid arc together with the two points it runs between.
    ///
    /// An [`ArcSegment`] names indices, and every measurement wants the actual
    /// points, so resolving them here keeps that lookup in one place instead of
    /// repeated — and repeated differently — in each caller.
    ///
    /// Callers must use this only on a validated contour. This is the choke point
    /// through which area, perimeter, path data, and hashes consume arcs, and it
    /// implements the contour contract: exactly one consecutive edge per arc
    /// (ADR-0017).
    #[must_use]
    pub fn resolved_arcs(&self) -> Vec<(ArcSegment, Point, Point)> {
        let n = self.points.len();
        let last = if self.closed { n } else { n - 1 };
        self.arcs
            .iter()
            .filter_map(|arc| {
                // Only arcs that replace a real run between real edges are usable.
                // An arc whose indices do not name consecutive points is not
                // something to guess at.
                if arc.to != (arc.from + 1) % n || arc.from >= last {
                    return None;
                }
                Some((*arc, self.points[arc.from], self.points[arc.to % n]))
            })
            .collect()
    }

    /// Whether this contour carries any curve.
    #[must_use]
    pub fn has_arcs(&self) -> bool {
        !self.arcs.is_empty()
    }

    /// The smallest arc radius in this contour.
    ///
    /// This is the measurement T-05 asks for. It is `None` for a polyline, and
    /// that is not a gap: a polyline has no radius, which is exactly why T-05 was
    /// inapplicable to polygonal geometry. Once a contour carries arcs, the number
    /// exists and this returns it.
    #[must_use]
    pub fn min_arc_radius(&self) -> Option<f64> {
        self.arcs
            .iter()
            .map(|a| a.radius)
            .filter(|r| *r > 0.0)
            .fold(None, |acc: Option<f64>, r| {
                Some(acc.map_or(r, |acc: f64| acc.min(r)))
            })
    }

    /// Replace every arc with straight segments within `tolerance_mm`.
    ///
    /// What the boolean library and every measurement see. The tolerance is a
    /// choice, not a constant, because "how closely does this curve have to match
    /// its chord" is a question about the tool, and the plotter profile is where
    /// that number comes from.
    #[must_use]
    pub fn flattened(&self, tolerance_mm: f64) -> Contour {
        if self.arcs.is_empty() {
            return self.clone();
        }

        // Build a map from each point index to the arc that starts there.
        let mut by_start: std::collections::BTreeMap<usize, &ArcSegment> =
            std::collections::BTreeMap::new();
        for arc in &self.arcs {
            by_start.insert(arc.from, arc);
        }

        let mut points = Vec::with_capacity(self.points.len() + self.arcs.len() * 8);
        let n = self.points.len();
        let segments = if self.closed { n } else { n - 1 };
        for i in 0..segments {
            let next = (i + 1) % n;
            match by_start.get(&i) {
                Some(arc) if arc.to == next => {
                    points.extend(arc_points(
                        self.points[i],
                        self.points[next],
                        arc,
                        tolerance_mm,
                    ));
                }
                _ => points.push(self.points[i]),
            }
        }
        if self.closed {
            if let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) {
                if first.distance_to(last) <= crate::constants::GEOM_EPS_MM {
                    points.pop();
                }
            }
        } else {
            points.push(self.points[n - 1]);
        }

        Contour {
            points,
            closed: self.closed,
            arcs: Vec::new(),
        }
    }

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

        // Arcs are part of the contour, so they are checked with it.
        //
        // Order matters: an arc names two indices into `points`, so confirming that
        // the points exist has to come first. Validating them after would make every
        // arc error depend on whether the contour happened to be long enough.
        //
        // Nothing here was checked before ADR-0015, which is how a document with a
        // zero-radius arc reached `expect("curves exist")` in the validator and
        // panicked, and how an arc pointing past the end was dropped without a word
        // while the content hash still called the shape identical to a polygon.
        let count = self.points.len();
        for (index, arc) in self.arcs.iter().enumerate() {
            if !arc.radius.is_finite() || arc.radius <= 0.0 {
                return Err(GeometryError::ArcRadius {
                    index,
                    radius: arc.radius,
                });
            }
            // One arc replaces exactly one edge, and every consumer agrees on that:
            // `resolved_arcs` requires it, `flattened` matches it, and `to_path_data`
            // draws from `resolved_arcs`. An arc spanning several edges is dropped by
            // all three without a word, so the shape declared a curve, T-05 counted
            // itself as active, and the area and hash came from the polygon.
            //
            // The last edge of a closed contour wraps to point 0, so `to` there is 0
            // rather than `count` — the same rule `resolved_arcs` applies.
            let last = if self.closed { count } else { count - 1 };
            if arc.from >= last || arc.to != (arc.from + 1) % count {
                return Err(GeometryError::ArcIndex {
                    index,
                    from: arc.from,
                    to: arc.to,
                    count,
                });
            }

            // And it has to be a curve rather than a very long slightly-bent chord.
            //
            // Written as `h² / (r + √(r² − h²))` rather than `r − √(r² − h²)` because
            // the first form never computes `r²`. The second overflows to infinity for
            // radii above ~1e154, and `inf − inf` is `NaN` — which then compares false
            // against the grid and would let the worst case through. The rearranged
            // form yields the correct answer, zero, without ever forming the square.
            let from = self.points[arc.from];
            let to = self.points[arc.to];
            let chord = from.distance_to(to);
            let half = chord / 2.0;
            // A radius smaller than half the chord cannot reach both endpoints at
            // all, so there is no circle to measure and the square root would be of a
            // negative number.
            if arc.radius < half {
                return Err(GeometryError::ArcRadius {
                    index,
                    radius: arc.radius,
                });
            }
            let root = (arc.radius * arc.radius - half * half).sqrt();
            let deviation = (half * half) / (arc.radius + root);
            if deviation < crate::constants::CANON_QUANTUM_MM {
                return Err(GeometryError::ArcBelowGrid {
                    index,
                    deviation,
                    chord,
                    quantum: crate::constants::CANON_QUANTUM_MM,
                });
            }
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
    /// Flatten every curve to line segments within `tolerance_mm`.
    ///
    /// The explicit way to hand this geometry to something that only understands
    /// polygons. Operations like union and offset have no closed form for arcs, so
    /// they have to approximate, and an approximation needs a tolerance that came
    /// from the plotter profile rather than from a default somebody forgot about.
    #[must_use]
    pub fn flattened(&self, tolerance_mm: f64) -> Shapes {
        if self
            .shapes
            .iter()
            .all(|s| s.outer.arcs.is_empty() && s.holes.iter().all(|h| h.arcs.is_empty()))
        {
            return self.clone();
        }
        Shapes {
            shapes: self
                .shapes
                .iter()
                .map(|s| Shape {
                    outer: s.outer.flattened(tolerance_mm),
                    holes: s.holes.iter().map(|h| h.flattened(tolerance_mm)).collect(),
                })
                .collect(),
        }
    }

    /// Reject geometry that still carries curves.
    ///
    /// Used by the operations that are polygon-only. The failure this prevents is
    /// the one this project keeps meeting: not an approximation, but a number that
    /// reads `.points` and silently reports the geometry of the chords instead.
    /// Returning the chords' result was never acceptable; returning an error that
    /// names the tolerance to use is.
    pub fn require_polygonal(&self) -> Result<(), OverlayError> {
        let curved = self
            .shapes
            .iter()
            .any(|s| !s.outer.arcs.is_empty() || s.holes.iter().any(|h| !h.arcs.is_empty()));
        if curved {
            Err(OverlayError::CurvedGeometry)
        } else {
            Ok(())
        }
    }

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

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    const LIMIT: f64 = 100_000.0;

    /// A square with one edge replaced by an arc, so arc tests have real geometry
    /// to sit on. The arc runs along the bottom edge from (0,0) to (10,0) with its
    /// centre below, bulging the curve downward.
    fn square_with_arc(radius: f64, from: usize, to: usize) -> Contour {
        Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)],
            closed: true,
            arcs: vec![ArcSegment {
                from,
                to,
                centre: p(5.0, -radius),
                radius,
                ccw: true,
            }],
        }
    }

    #[test]
    fn an_arc_with_a_non_positive_radius_is_rejected() {
        // Reproduces a reachable panic in the validator: `has_curves` is true for a
        // non-empty arc list while `min_arc_radius` filters `r > 0.0`, so the two
        // disagreed and `expect("curves exist")` fired on `None`.
        for radius in [0.0, -1.0] {
            let err = square_with_arc(radius, 0, 1)
                .validate(LIMIT)
                .expect_err("a zero or negative radius is not a curve");
            assert!(matches!(err, GeometryError::ArcRadius { .. }), "{err:?}");
        }
    }

    #[test]
    fn a_non_finite_arc_radius_is_rejected() {
        // The module header forbids NaN and Inf in stored geometry. That promise
        // held for points and did not hold for arcs.
        let mut contour = square_with_arc(5.0, 0, 1);
        contour.arcs[0].radius = f64::NAN;
        let err = contour.validate(LIMIT).expect_err("NaN radius");
        assert!(matches!(err, GeometryError::ArcRadius { .. }), "{err:?}");
    }

    #[test]
    fn the_wrapping_arc_on_a_closed_contour_is_legal() {
        // The last edge of a closed contour runs from the final point back to point
        // 0, so its arc names `to = 0` rather than `to = count`. Rejecting that would
        // forbid rounding the seam of every closed shape — which is exactly what
        // `round_corners` produces.
        let contour = square_with_arc(5.0, 3, 0);
        assert!(
            contour.validate(LIMIT).is_ok(),
            "the closing arc is a real edge: {:?}",
            contour.validate(LIMIT)
        );
    }

    #[test]
    fn an_arc_spanning_several_edges_is_rejected() {
        // The defect ADR-0015 was written for. `resolved_arcs`, `flattened` and
        // `to_path_data` all drop such an arc in silence, so a shape that declared a
        // curve measured, hashed and drew as the polygon without one — verified to
        // produce a digest identical to that polygon's.
        let err = square_with_arc(5.0, 0, 3)
            .validate(LIMIT)
            .expect_err("one arc replaces one edge");
        assert!(matches!(err, GeometryError::ArcIndex { .. }), "{err:?}");
    }

    #[test]
    fn validated_single_edge_arcs_are_all_resolved() {
        // ADR-0017's contract from the other side: validation and resolution must
        // agree. A contour that passes validation must not lose arcs anywhere
        // downstream, or `resolved_arcs` would silently decide that a declared curve
        // was not there.
        let contour = Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)],
            closed: true,
            arcs: (0..4)
                .map(|from| ArcSegment {
                    from,
                    to: (from + 1) % 4,
                    centre: p(5.0, -1_000.0),
                    radius: 1_000.0,
                    ccw: false,
                })
                .collect(),
        };

        contour
            .validate(LIMIT)
            .expect("one-edge arcs must be pipeline-valid");
        let resolved = contour.resolved_arcs();
        assert_eq!(resolved.len(), contour.arcs.len());
        for (index, (arc, from, to)) in resolved.iter().enumerate() {
            assert_eq!((arc.from, arc.to), (index, (index + 1) % 4));
            assert_eq!(*from, contour.points[index]);
            assert_eq!(*to, contour.points[(index + 1) % 4]);
        }
    }

    /// A square whose bottom edge is replaced by an arc of the given radius, bulging
    /// downward. `ccw: false` because with the centre below the chord the short way
    /// round is clockwise — a counter-clockwise arc about that centre really is most
    /// of a circle, which is a different arc rather than a wrong answer.
    fn square_under_arc(radius: f64) -> Contour {
        Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)],
            closed: true,
            arcs: vec![ArcSegment {
                from: 0,
                to: 1,
                centre: p(5.0, -radius),
                radius,
                ccw: false,
            }],
        }
    }

    #[test]
    fn an_arc_too_flat_to_measure_is_rejected() {
        // The defect: a radius of 1e300 on a 10 mm chord is geometrically almost a
        // straight line, and the segment-area formula squares the radius, so the area
        // came out `NaN` and reached the manifest as `null` — which reads as "nothing
        // was measured" rather than "this cannot be measured".
        for radius in [1e300, 1e155, 1e6, 2e5] {
            let err = square_under_arc(radius)
                .validate(LIMIT)
                .expect_err("a curve flatter than the grid is not a curve");
            assert!(
                matches!(err, GeometryError::ArcBelowGrid { .. }),
                "r={radius} gave {err:?}"
            );
        }
    }

    #[test]
    fn the_flatness_limit_is_where_the_sagitta_says_it_is() {
        // The threshold is not a round number chosen for legibility, so it is derived
        // rather than asserted: for a chord `d` the sagitta approaches `d²/8r`, so the
        // largest acceptable radius is `d² / (8 · quantum)`. On this 10 mm contour that
        // is 125 000 mm — and a guess of 100 000 is on the wrong side of it, which is
        // why this is computed.
        let chord = 10.0;
        let critical = (chord * chord) / (8.0 * crate::constants::CANON_QUANTUM_MM);
        assert!(
            square_under_arc(critical * 2.0).validate(LIMIT).is_err(),
            "twice the critical radius must be rejected"
        );
        assert!(
            square_under_arc(critical * 0.5).validate(LIMIT).is_ok(),
            "half the critical radius must be kept"
        );
    }

    #[test]
    fn an_arc_that_bends_by_more_than_the_grid_is_kept() {
        // The rule has to leave real fillets alone, or it is just a ban on curves.
        // A 0.4 mm fillet on a 10 mm edge deviates 0.01 mm, a hundred times the grid.
        assert!(
            square_under_arc(5.0).validate(LIMIT).is_ok(),
            "a semicircular bulge must survive"
        );
        assert!(
            square_under_arc(1_000.0).validate(LIMIT).is_ok(),
            "a 1 km radius on a 10 mm chord deviates 12.5 microns, above the grid"
        );
    }

    #[test]
    fn a_radius_smaller_than_half_the_chord_is_rejected() {
        // No circle of that radius passes through both endpoints, so there is nothing
        // to measure and the square root below would be of a negative number.
        let err = square_under_arc(4.9)
            .validate(LIMIT)
            .expect_err("a 4.9 mm radius cannot span a 10 mm chord");
        assert!(matches!(err, GeometryError::ArcRadius { .. }), "{err:?}");
    }

    #[test]
    fn the_flatness_check_does_not_overflow_on_a_huge_radius() {
        // The reason the sagitta is computed as `h² / (r + √(r² − h²))`. Written as
        // `r − √(r² − h²)` it is `inf − inf` past r ≈ 1e154, and `NaN` compares
        // false against the grid, so the one input that must never be accepted would
        // sail through. Pinned because the failure is invisible on every other input.
        let contour = square_under_arc(1e155);
        let err = contour.validate(LIMIT).expect_err("must not be accepted");
        match err {
            GeometryError::ArcBelowGrid { deviation, .. } => {
                assert!(
                    deviation.is_finite() && deviation >= 0.0,
                    "deviation must be a real number, got {deviation}"
                );
            }
            other => panic!("expected a flatness rejection, got {other:?}"),
        }
    }

    #[test]
    fn an_arc_reaching_past_the_last_point_is_rejected() {
        let err = square_with_arc(5.0, 0, 99)
            .validate(LIMIT)
            .expect_err("an arc past the end names nothing");
        assert!(matches!(err, GeometryError::ArcIndex { .. }), "{err:?}");
    }

    #[test]
    fn an_arc_starting_past_the_last_point_is_rejected() {
        let err = square_with_arc(5.0, 99, 99)
            .validate(LIMIT)
            .expect_err("from past the end");
        assert!(matches!(err, GeometryError::ArcIndex { .. }), "{err:?}");
    }

    #[test]
    fn an_empty_arc_run_is_rejected() {
        // `from == to` describes no curve at all. Accepted, it would add a run that
        // draws nothing while still flipping `has_arcs`, and T-05 would report
        // itself as active over a polygon.
        let err = square_with_arc(5.0, 1, 1)
            .validate(LIMIT)
            .expect_err("an empty run is not a curve");
        assert!(matches!(err, GeometryError::ArcIndex { .. }), "{err:?}");
    }

    #[test]
    fn an_arc_that_touches_nothing_still_survives_point_counting() {
        // The regression this whole change is about: an open contour with no points
        // and a non-empty arc list slipped past the `arcs.is_empty()` early return
        // and underflowed `points.len() - 1` in `flattened`.
        let contour = Contour {
            points: Vec::new(),
            closed: false,
            arcs: vec![ArcSegment {
                from: 0,
                to: 1,
                centre: p(0.0, 0.0),
                radius: 5.0,
                ccw: true,
            }],
        };
        // Rejected by the point count, which is checked before the arcs.
        assert!(contour.validate(LIMIT).is_err());
    }

    #[test]
    fn an_empty_arc_list_means_every_edge_is_straight() {
        // The property the whole representation rests on: every contour that existed
        // before this field must keep meaning exactly what it meant.
        let contour = Contour::straight(
            vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)],
            true,
        );
        assert!(!contour.has_arcs());
        assert_eq!(contour.min_arc_radius(), None);
        // Flattening is the identity.
        let flat = contour.flattened(0.05);
        assert_eq!(flat.points, contour.points);
        assert!(flat.arcs.is_empty());
    }

    /// A circle as four points and four arcs, each a quarter turn.
    ///
    /// The arcs have to be geometrically real: both endpoints of an arc must lie
    /// on the circle of the stated radius. My first attempt used a square with one
    /// corner "rounded", where the start point sat on the centre rather than the
    /// circumference, so the sweep came out zero and the flattening silently
    /// returned four points — which is how a broken test can look like a passing
    /// one if nobody checks what it produced.
    fn circle(radius: f64) -> Contour {
        let points = vec![
            p(radius, 0.0),
            p(0.0, radius),
            p(-radius, 0.0),
            p(0.0, -radius),
        ];
        let arcs = (0..4)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: p(0.0, 0.0),
                radius,
                ccw: true,
            })
            .collect();
        Contour {
            points,
            closed: true,
            arcs,
        }
    }

    #[test]
    fn a_contour_can_carry_arcs() {
        let contour = circle(10.0);
        assert!(contour.has_arcs());
        assert!((contour.min_arc_radius().expect("a radius") - 10.0).abs() < 1e-12);
    }

    #[test]
    fn flattening_an_arc_approximates_it_within_the_tolerance() {
        let contour = circle(10.0);
        let flat = contour.flattened(0.01);
        assert!(
            flat.points.len() > contour.points.len(),
            "an arc needs more than one edge: got {}",
            flat.points.len()
        );

        // Every flattened point must sit on the circle.
        let mut worst: f64 = 0.0;
        for q in &flat.points {
            worst = worst.max((q.distance_to(p(0.0, 0.0)) - 10.0).abs());
        }
        assert!(
            worst <= 0.01 + 1e-9,
            "points should lie on the arc, worst {worst:.6}"
        );
    }

    #[test]
    fn the_flattened_arc_hugs_the_circle_to_the_tolerance() {
        // The sagitta is the real check: every chord of the flattened arc must
        // stay within the tolerance of the curve, not merely have its endpoints
        // on it.
        let flat = circle(10.0).flattened(0.05);
        let mut worst = 0.0_f64;
        for i in 0..flat.points.len() {
            let a = flat.points[i];
            let b = flat.points[(i + 1) % flat.points.len()];
            // Midpoint of the chord against the circle.
            let mid = p((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
            worst = worst.max((10.0 - mid.distance_to(p(0.0, 0.0))).abs());
        }
        assert!(
            worst <= 0.05 + 1e-9,
            "sagitta {worst:.6} exceeds the 0.05 mm tolerance"
        );
    }

    #[test]
    fn a_finer_tolerance_gives_more_segments() {
        let coarse = circle(10.0).flattened(0.5).points.len();
        let fine = circle(10.0).flattened(0.001).points.len();
        assert!(fine > coarse, "finer tolerance: {fine} vs {coarse}");
    }

    #[test]
    fn flattening_a_closed_contour_does_not_double_the_seam() {
        let flat = circle(10.0).flattened(0.02);
        assert!(flat.closed);
        let first = flat.points[0];
        let last = *flat.points.last().expect("points");
        assert!(
            first.distance_to(last) > 1e-9,
            "the seam must not double up"
        );
    }

    #[test]
    fn the_narrowest_arc_in_a_contour_is_reported() {
        let contour = Contour {
            points: vec![
                p(0.0, 0.0),
                p(10.0, 0.0),
                p(10.0, 10.0),
                p(5.0, 10.0),
                p(0.0, 10.0),
            ],
            arcs: vec![
                ArcSegment {
                    from: 2,
                    to: 3,
                    centre: p(7.5, 10.0),
                    radius: 2.5,
                    ccw: true,
                },
                ArcSegment {
                    from: 3,
                    to: 4,
                    centre: p(2.5, 10.0),
                    radius: 2.5,
                    ccw: true,
                },
            ],
            closed: true,
        };
        assert!((contour.min_arc_radius().expect("a radius") - 2.5).abs() < 1e-12);
    }

    #[test]
    fn an_arc_list_survives_serialisation() {
        let contour = Contour {
            points: vec![p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0), p(0.0, 0.0)],
            arcs: vec![ArcSegment {
                from: 3,
                to: 0,
                centre: p(0.0, 0.0),
                radius: 10.0,
                ccw: false,
            }],
            closed: true,
        };
        let text = serde_json::to_string(&contour).expect("serialise");
        let back: Contour = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back, contour);
    }

    #[test]
    fn an_old_contour_without_the_field_still_reads() {
        // Every fixture and manifest written before curves existed must keep loading.
        let json = r#"{"points":[{"x":0,"y":0},{"x":10,"y":0}],"closed":true}"#;
        let contour: Contour = serde_json::from_str(json).expect("old payloads stay readable");
        assert!(contour.arcs.is_empty());
        assert!(!contour.has_arcs());
    }

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
            arcs: Vec::new(),
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
            arcs: Vec::new(),
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
            arcs: Vec::new(),
        };
        let cw = |x: f64, y: f64, w: f64, h: f64| Contour {
            points: vec![
                Point { x, y },
                Point { x, y: y + h },
                Point { x: x + w, y: y + h },
                Point { x: x + w, y },
            ],
            closed: true,
            arcs: Vec::new(),
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
                arcs: Vec::new(),
            },
            vec![Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 1.0, y: 0.0 },
                    Point { x: 1.0, y: 1.0 },
                ],
                closed: true,
                arcs: Vec::new(),
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
            arcs: Vec::new(),
        };
        let shape = Shape::with_holes(rect(0.0), vec![rect(0.2), rect(0.4)]);
        assert_eq!(shape.contour_count(), 3);
    }
}
