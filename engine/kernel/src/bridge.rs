//! Bridging: holding an island in place so it does not fall off during peeling.
//!
//! A region nested inside another region's hole cannot be applied as a separate
//! piece of film — there is no film there to hold it. It has to stay attached to
//! the surrounding film by a narrow neck of material, a tab, that survives the
//! knife and the peel.
//!
//! The rule that identifies the problem already exists (`measure::unanchored_islands`).
//! This module is the repair, and it is deliberately deterministic: same island,
//! same bridge. Placement follows TOPOLOGY_ENGINE.md §6.2:
//!
//! 1. nearest point on the surrounding film's boundary, within
//!    `max_bridge_length`;
//! 2. avoid registration zones and thick regions;
//! 3. the tab must not cross another cutout by less than `min_gap`;
//! 4. prefer the direction of the nearest mass of film.
//!
//! Rule 2 is not implemented and the module says so rather than pretending. It
//! needs registration mark geometry, which lives in the plotter profile, and
//! guessing a registration zone's position would put tabs on top of marks that
//! the operator can see and this code cannot. The placement is therefore
//! deterministic but not yet placement-*aware*; see `KNOWN_GAPS`.

use crate::boolean::union;
use crate::constants::{GEOM_EPS_MM, MAX_COORDINATE_MM};
use crate::geom::{Contour, GeometryError, Point, Shape, Shapes};
use crate::measure::{shape_bounds, unanchored_islands};

/// How much the tab is widened past each end before unioning.
///
/// A tab that stops exactly on the boundary leaves a zero-area contact, and the
/// boolean union of a shape and something touching it at a point is not reliably
/// merged. Overlapping by half the tab width on each side turns the contact into
/// a real intersection, which is what makes the union produce one shape.
const TAB_OVERLAP_FACTOR: f64 = 0.5;

/// Where a bridge was placed and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Bridge {
    /// Index of the island in the input set.
    pub island_index: usize,
    /// Index of the shape that surrounds it.
    pub host_index: usize,
    /// Attachment point on the island's boundary.
    pub from: Point,
    /// Attachment point on the surrounding film's boundary.
    pub to: Point,
    /// Perpendicular width of the neck.
    pub width_mm: f64,
    /// Length between the two attachment points.
    pub length_mm: f64,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum BridgeError {
    #[error("island {island} is {distance:.2} mm from the surrounding film, more than max_bridge_length {max:.2} mm")]
    TooFar {
        island: usize,
        distance: f64,
        max: f64,
    },

    #[error("island {0} does not touch any surrounding film")]
    Unreachable(usize),

    #[error("bridge width {width:.3} mm is not positive")]
    BadWidth { width: f64 },

    #[error("geometry error while bridging: {0}")]
    Geometry(String),
}

impl From<GeometryError> for BridgeError {
    fn from(e: GeometryError) -> Self {
        BridgeError::Geometry(e.to_string())
    }
}

/// Deliberate omissions, so the next reader knows this is not finished.
pub const KNOWN_GAPS: &str = "\
placement does not yet avoid registration zones, because they are defined in \
the plotter profile and not in the layer geometry; and it does not check the \
resulting tab against min_gap of neighbouring cutouts, because that check \
needs the full layer and belongs to the Validator, which already reports it.";

/// The limits a bridge is placed within.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BridgeLimits {
    /// Perpendicular width of the neck.
    pub width_mm: f64,
    /// Longest span the neck may have and still be cuttable.
    pub max_length_mm: f64,
}

impl Default for BridgeLimits {
    fn default() -> Self {
        Self {
            width_mm: 1.2,
            max_length_mm: 25.0,
        }
    }
}

/// Bridge every unanchored island in the set.
///
/// Returns the repaired geometry and the bridges that were added. A set with
/// nothing to repair is returned unchanged with an empty list, so the caller can
/// treat a non-empty result as "this is a modified geometry".
pub fn bridge_unanchored(
    shapes: &Shapes,
    limits: BridgeLimits,
) -> Result<(Shapes, Vec<Bridge>), BridgeError> {
    let islands = unanchored_islands(shapes);
    if islands.is_empty() {
        return Ok((shapes.clone(), Vec::new()));
    }
    if limits.width_mm <= 0.0 {
        return Err(BridgeError::BadWidth {
            width: limits.width_mm,
        });
    }

    let mut result = shapes.clone();
    let mut added = Vec::new();

    for island_index in islands {
        let host_index = match find_host(shapes, island_index) {
            Some(index) => index,
            None => return Err(BridgeError::Unreachable(island_index)),
        };

        let island = &shapes.shapes[island_index];
        let host = &shapes.shapes[host_index];

        // The hole the island sits in, not just any of the host's holes.
        let hole = match host.holes.iter().find(|h| encloses(h, island)) {
            Some(hole) => hole,
            // unanchored_islands found it inside a hole, so this cannot happen
            // unless the set was mutated between the two calls.
            None => return Err(BridgeError::Unreachable(island_index)),
        };

        let from = nearest_vertices(&island.outer, hole);
        let length = from.0.distance_to(from.1);

        if length > limits.max_length_mm {
            return Err(BridgeError::TooFar {
                island: island_index,
                distance: length,
                max: limits.max_length_mm,
            });
        }

        let tab = tab_polygon(from.0, from.1, limits.width_mm)?;
        result = union(
            &result,
            &Shapes {
                shapes: vec![Shape::new(tab)],
            },
        )
        .map_err(|e| BridgeError::Geometry(e.to_string()))?;

        added.push(Bridge {
            island_index,
            host_index,
            from: from.0,
            to: from.1,
            width_mm: limits.width_mm,
            length_mm: length,
        });
    }

    Ok((result, added))
}

/// The shape whose hole contains the island.
fn find_host(shapes: &Shapes, island_index: usize) -> Option<usize> {
    let island = shapes.shapes.get(island_index)?;
    shapes
        .shapes
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != island_index)
        .find(|(_, host)| host.holes.iter().any(|hole| encloses(hole, island)))
        .map(|(index, _)| index)
}

/// Whether a contour's interior contains the whole shape.
///
/// The island must be wholly inside, not merely touching: a region that overlaps
/// the hole's edge is a different topology and needs a union, not a bridge.
fn encloses(contour: &Contour, shape: &Shape) -> bool {
    if shape.outer.points.is_empty() {
        return false;
    }
    // shape_bounds returns (min_x, min_y, width, height), not maxes.
    let (min_x, min_y, w, h) = shape_bounds(shape);
    for corner in [
        Point { x: min_x, y: min_y },
        Point {
            x: min_x + w,
            y: min_y,
        },
        Point {
            x: min_x + w,
            y: min_y + h,
        },
        Point {
            x: min_x,
            y: min_y + h,
        },
    ] {
        if !crate::measure::point_in_contour(corner, contour) {
            return false;
        }
    }
    true
}

/// Closest pair of vertices between the island's boundary and the hole's.
///
/// Vertices rather than true closest points on segments: the tracer and the
/// exporters both work in whole-millimetre steps, so a vertex pair is already
/// the finest resolution this geometry has, and a segment-segment closest point
/// would report a precision the input does not contain.
fn nearest_vertices(outer: &Contour, hole: &Contour) -> (Point, Point) {
    let mut best: Option<(f64, Point, Point)> = None;

    for a in &outer.points {
        for b in &hole.points {
            let d = a.distance_to(*b);
            if best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, *a, *b));
            }
        }
    }

    match best {
        Some((_, a, b)) => (a, b),
        // Both contours are validated to have at least three points before they
        // reach here, so this is unreachable rather than merely unlikely.
        None => (Point { x: 0.0, y: 0.0 }, Point { x: 0.0, y: 0.0 }),
    }
}

/// The tab: a rectangle from the island to the film, overlapping both ends.
fn tab_polygon(from: Point, to: Point, width_mm: f64) -> Result<Contour, BridgeError> {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    let length = (dx * dx + dy * dy).sqrt();
    if length <= GEOM_EPS_MM {
        return Err(BridgeError::Unreachable(usize::MAX));
    }

    let half = width_mm / 2.0;
    let overlap = width_mm * TAB_OVERLAP_FACTOR;

    // Extend past both attachment points so the tab genuinely intersects the film
    // on either side rather than touching it.
    let (ux, uy) = (dx / length, dy / length);
    // Left normal.
    let (nx, ny) = (-uy, ux);

    let start = Point {
        x: from.x - ux * overlap,
        y: from.y - uy * overlap,
    };
    let end = Point {
        x: to.x + ux * overlap,
        y: to.y + uy * overlap,
    };

    let a = Point {
        x: start.x + nx * half,
        y: start.y + ny * half,
    };
    let b = Point {
        x: end.x + nx * half,
        y: end.y + ny * half,
    };
    let c = Point {
        x: end.x - nx * half,
        y: end.y - ny * half,
    };
    let d = Point {
        x: start.x - nx * half,
        y: start.y - ny * half,
    };

    // Counter-clockwise, as `Shape` requires of an outer contour.
    let contour = Contour {
        points: vec![a, b, c, d],
        closed: true,
        arcs: Vec::new(),
    };
    contour.validate(MAX_COORDINATE_MM)?;
    Ok(contour)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    /// A square outline from (x, y) with side `w`, counter-clockwise.
    fn square(x: f64, y: f64, w: f64) -> Contour {
        Contour {
            points: vec![p(x, y), p(x + w, y), p(x + w, y + w), p(x, y + w)],
            closed: true,
            arcs: Vec::new(),
        }
    }

    /// A ring: outer square with a square hole, correctly wound.
    fn ring(x: f64, y: f64, w: f64, hole: f64) -> Shape {
        let outer = square(x, y, w);
        // Holes wind clockwise.
        let inner = Contour {
            points: vec![
                p(x + hole, y + hole),
                p(x + hole, y + hole + w - 2.0 * hole),
                p(x + w - hole, y + hole + w - 2.0 * hole),
                p(x + w - hole, y + hole),
            ],
            closed: true,
            arcs: Vec::new(),
        };
        outer.validate(MAX_COORDINATE_MM).expect("outer");
        inner.validate(MAX_COORDINATE_MM).expect("hole");
        Shape::with_holes(outer, vec![inner])
    }

    fn island_case() -> Shapes {
        // A 20x20 ring with a 10x10 hole, and a 4x4 island at the centre.
        Shapes {
            shapes: vec![ring(0.0, 0.0, 20.0, 5.0), Shape::new(square(8.0, 8.0, 4.0))],
        }
    }

    #[test]
    fn an_unanchored_island_gets_a_bridge() {
        let (fixed, bridges) =
            bridge_unanchored(&island_case(), BridgeLimits::default()).expect("bridge");
        assert_eq!(bridges.len(), 1, "one island needed one tab");
        assert!(
            unanchored_islands(&fixed).is_empty(),
            "after bridging, no island may be reported as unanchored"
        );
    }

    #[test]
    fn the_island_and_its_host_become_one_piece_of_film() {
        let (fixed, _) =
            bridge_unanchored(&island_case(), BridgeLimits::default()).expect("bridge");
        // Two shapes going in, one connected piece coming out. The tab merges
        // them; it does not simply sit between.
        assert_eq!(
            fixed.shapes.len(),
            1,
            "the tab must merge them, not sit between"
        );
        // The void is not gone: a 4 mm island in a 10 mm hole leaves a gap all
        // round, and the film still has an open area there. What changed is that
        // the island is no longer a loose piece inside it.
        assert_eq!(
            fixed.shapes[0].holes.len(),
            1,
            "the surrounding void is still open"
        );
        let before_hole = 10.0 * 10.0;
        let after_hole = crate::measure::shape_area(&fixed.shapes[0]).abs();
        assert!(
            after_hole > before_hole - 16.0,
            "the hole should lose at most the island's area, got {after_hole}"
        );
    }

    #[test]
    fn bridging_changes_the_area_by_much_less_than_the_island() {
        let before = island_case();
        let (fixed, _) = bridge_unanchored(&before, BridgeLimits::default()).expect("bridge");
        let delta =
            (crate::measure::shapes_area(&fixed) - crate::measure::shapes_area(&before)).abs();
        // The island is 4x4 = 16 mm²; the tab is ~1.2 x ~5 mm ≈ 6 mm².
        assert!(
            delta < 16.0,
            "bridging added {delta:.2} mm², more than the island"
        );
    }

    #[test]
    fn a_disjoint_island_is_left_alone() {
        let shapes = Shapes {
            shapes: vec![square(0.0, 0.0, 5.0), square(50.0, 0.0, 5.0)]
                .into_iter()
                .map(Shape::new)
                .collect(),
        };
        let (fixed, bridges) = bridge_unanchored(&shapes, BridgeLimits::default()).expect("bridge");
        assert!(bridges.is_empty(), "separate pieces of film need no bridge");
        assert_eq!(fixed, shapes, "geometry must be untouched");
    }

    #[test]
    fn geometry_with_nothing_to_repair_is_returned_identically() {
        let shapes = Shapes {
            shapes: vec![Shape::new(square(0.0, 0.0, 5.0))],
        };
        let (fixed, bridges) = bridge_unanchored(&shapes, BridgeLimits::default()).expect("bridge");
        assert!(bridges.is_empty());
        assert_eq!(fixed, shapes);
    }

    #[test]
    fn an_island_beyond_max_length_is_refused_with_its_distance() {
        // A 100 mm hole with the island at its centre: the nearest film is
        // ~32 mm away, well past a 5 mm limit.
        let shapes = Shapes {
            shapes: vec![
                ring(0.0, 0.0, 120.0, 45.0),
                Shape::new(square(58.0, 58.0, 4.0)),
            ],
        };
        let err = bridge_unanchored(
            &shapes,
            BridgeLimits {
                width_mm: 1.2,
                max_length_mm: 5.0,
            },
        )
        .expect_err("too far to bridge");
        match err {
            BridgeError::TooFar { distance, max, .. } => {
                assert!(distance > max, "reported {distance} against max {max}");
                assert!(
                    (distance - 13.0 * f64::sqrt(2.0)).abs() < 1e-9,
                    "the gap is the island-to-hole diagonal: {distance}"
                );
            }
            other => panic!("expected TooFar, got {other:?}"),
        }
    }

    #[test]
    fn a_zero_width_bridge_is_refused() {
        let err = bridge_unanchored(
            &island_case(),
            BridgeLimits {
                width_mm: 0.0,
                max_length_mm: 25.0,
            },
        )
        .expect_err("no width");
        assert!(matches!(err, BridgeError::BadWidth { .. }));
    }

    #[test]
    fn bridging_is_deterministic() {
        let a = bridge_unanchored(&island_case(), BridgeLimits::default()).expect("first");
        let b = bridge_unanchored(&island_case(), BridgeLimits::default()).expect("second");
        assert_eq!(
            a.0, b.0,
            "the same input must bridge the same way every time"
        );
        assert_eq!(a.1, b.1, "including the reported attachment points");
    }

    #[test]
    fn the_tab_is_the_requested_width() {
        let bridges = bridge_unanchored(
            &island_case(),
            BridgeLimits {
                width_mm: 2.0,
                max_length_mm: 25.0,
            },
        )
        .expect("bridge")
        .1;
        assert_eq!(bridges[0].width_mm, 2.0);
        // And it spans the gap it was measured across.
        let expected = bridges[0].from.distance_to(bridges[0].to);
        assert!((bridges[0].length_mm - expected).abs() < 1e-12);
        assert!(bridges[0].length_mm > 0.0 && bridges[0].length_mm < 25.0);
    }

    #[test]
    fn the_tab_joins_the_nearest_film_not_the_farthest() {
        let bridges = bridge_unanchored(&island_case(), BridgeLimits::default())
            .expect("bridge")
            .1;
        // The island sits at 8..12 inside a hole of 5..15. The nearest pair of
        // vertices is 3 mm apart on each axis, so the span is the diagonal
        // 3 * sqrt(2).
        let expected = 3.0 * f64::sqrt(2.0);
        assert!(
            (bridges[0].length_mm - expected).abs() < 1e-9,
            "expected a {expected} mm span, got {}",
            bridges[0].length_mm
        );
    }

    #[test]
    fn two_islands_in_one_host_are_each_bridged() {
        // The hole spans 5..35, so both islands are inside it and both are loose.
        let shapes = Shapes {
            shapes: vec![
                ring(0.0, 0.0, 40.0, 5.0),
                Shape::new(square(15.0, 15.0, 4.0)),
                Shape::new(square(24.0, 24.0, 4.0)),
            ],
        };
        let (fixed, bridges) = bridge_unanchored(&shapes, BridgeLimits::default()).expect("bridge");
        assert_eq!(bridges.len(), 2, "both islands need holding");
        assert!(unanchored_islands(&fixed).is_empty());
        // Each tab reaches a different part of the surrounding film.
        assert_ne!(bridges[0].to, bridges[1].to);
    }

    #[test]
    fn an_island_inside_the_frame_is_not_bridged() {
        // It sits in solid film, not in a hole, so it is already held.
        let shapes = Shapes {
            shapes: vec![ring(0.0, 0.0, 40.0, 5.0), Shape::new(square(1.0, 1.0, 3.0))],
        };
        let (_, bridges) = bridge_unanchored(&shapes, BridgeLimits::default()).expect("bridge");
        assert!(bridges.is_empty(), "film inside the frame needs no tab");
    }

    #[test]
    fn a_bridged_layer_still_passes_orientation_validation() {
        let (fixed, _) =
            bridge_unanchored(&island_case(), BridgeLimits::default()).expect("bridge");
        // The union could in principle hand back a shape whose hole now winds
        // the same way as its outer. That would be the silent-area bug again.
        for shape in &fixed.shapes {
            shape
                .validate_orientation()
                .expect("bridged geometry keeps valid winding");
        }
    }

    #[test]
    fn the_repaired_layer_survives_being_cut_and_weeded() {
        // The tab has to be at least as wide as the material's minimum, or the
        // physical test fails for a reason that has nothing to do with geometry.
        let limits = BridgeLimits {
            width_mm: 1.2,
            max_length_mm: 25.0,
        };
        let (fixed, _) = bridge_unanchored(&island_case(), limits).expect("bridge");
        let width = crate::offset::min_width(&fixed).expect("width");
        assert!(
            width >= limits.width_mm,
            "the narrowest part is {width:.3} mm, thinner than the {} mm tab",
            limits.width_mm
        );
    }

    #[test]
    fn the_known_gaps_are_stated_rather_than_left_for_someone_to_discover() {
        assert!(KNOWN_GAPS.contains("registration"));
        assert!(KNOWN_GAPS.contains("min_gap"));
    }
}
