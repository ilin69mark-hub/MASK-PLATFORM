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

/// First delta tried when climbing towards a neck, in millimetres.
///
/// Below any physically meaningful neck, and small enough that the first bracket
/// around a 0.2 mm tab is [0.05, 0.1] mm — two offsets instead of thirty.
const NECK_SEARCH_SEED_MM: f64 = 0.05;

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
/// # What this measures, and what it does not
///
/// A mask element is a band of film of uniform thickness, so for a band that
/// thickness is the shortest distance across it. The band collapses once the
/// inward offset exceeds half its thickness, so the width is found by bisecting
/// on the offset delta at which the result becomes empty, then doubling.
///
/// **This finds the width of a shape that is one band. It does not find the
/// narrowest neck.** A shape with a thin appendage reports the band's width, and a
/// bridge tab joined to a large region reports the region's width, because the
/// method looks for the largest inward offset that leaves *anything* behind. A
/// thin neck survives that search: the surrounding mass keeps the result
/// non-empty long after the neck has closed.
///
/// Measured, on a 4 mm band with a 0.2 mm whisker:
///
/// ```text
/// plain 4 mm band        -> 4.000 mm
/// band + 0.2 mm neck     -> 4.000 mm   <- the whisker is invisible
/// ```
///
/// This is a real limitation of T-01 and it is stated rather than left for
/// someone to discover through a mask that tears. Finding the true medial axis
/// needs a distance transform or a per-part decomposition, neither of which is
/// a local change to this function, so it has not been attempted here. The tests
/// `a_thin_neck_is_invisible_and_that_is_pinned` and `a_plain_band_is_measured`
/// exist to keep the limitation honest.
///
/// Bisection is used rather than a direct computation because it needs no
/// assumption about the shape: a distance-field approach would be faster but only
/// as accurate as the raster grid, and this must hold to the canonical grid
/// (ADR-0004).
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

/// The width at which a shape stops being one connected piece, in millimetres.
///
/// This is the measure T-01 actually wants, and it is stronger than
/// [`min_width`]: a shape comes apart in two ways, and both matter.
///
/// It can **split** — a bridge tab or any neck lets one piece become two, and
/// the two then weed apart. It can **vanish** — a band erodes to nothing, and
/// that is exactly its width.
///
/// `min_width` only sees the second, which is why it missed a 0.2 mm bridge
/// tab entirely: the surrounding frame kept the result non-empty long after
/// the tab had closed. Counting components sees both, because both change the
/// count away from what it was.
///
/// A distance transform does not answer this either. It gives the *widest*
/// inscribed circle, which equals the band width for a band and says nothing
/// about a neck; the narrowest neck is `2 × min` over the medial axis, and
/// computing a medial axis is not a local change to this file. Bisection on the
/// offset topology answers it, with the library doing the geometry.
///
/// **The component count is not monotonic in the delta, so the search must not
/// assume it is.** This was measured, not assumed: a frame 40 mm across with a
/// 0.2 mm tab comes apart at 0.1 mm, is whole again at 2 mm, and comes apart
/// once more at 3.4 mm. A bisection started from the top of the range lands on
/// whichever of those boundaries its path happens to cross, which is why the
/// same tab measured 0.2 mm inside a 20 mm frame and 6.76 mm inside a 40 mm one.
/// The search therefore grows from a small delta until the count changes, and
/// bisects only the bracket it found — the first change is the neck.
///
/// Holes are never components. A ring eroded to half its bar width is still one
/// ring, and counting the hole as a second piece would report every window in a
/// stencil as a neck.
///
/// What it still misses: a thin *appendage* on a wide band. Eroding the band
/// more than half its width deletes the whisker without changing the count, so
/// this returns the band's width. That limitation is documented on `min_width`
/// and pinned by a test there.
#[must_use]
pub fn narrowest_neck(shape: &Shapes) -> Option<f64> {
    if shape.is_empty() {
        return None;
    }

    let base = component_count(shape);
    let (_, _, w, h) = bounding_box(shape);
    let ceiling = w.max(h) / 2.0;

    // "Came apart" means the count is no longer what it was — either it split
    // into more pieces, or it eroded away entirely. Testing only for "more"
    // misses the vanishing case, which is the ordinary band and is why the
    // first version of this function returned None for every band it was given.
    let came_apart = |d: f64| -> bool {
        offset_shape(shape, -d)
            .map(|s| component_count(&s) != base)
            .unwrap_or(true)
    };

    // Climb from below rather than starting at the ceiling, because the predicate
    // is not monotonic: the offset can briefly re-join pieces it had just
    // separated, and a bisection from the top would return that later crossing
    // instead of the neck. Doubling reaches the same bracket in about
    // log2(ceiling / seed) calls, so this costs a handful of offsets more than
    // the old unconditional bisection.
    let mut lo = 0.0_f64; // known to stay one piece
    let mut hi = NECK_SEARCH_SEED_MM.min(ceiling);
    let mut guard = 0;
    while !came_apart(hi) {
        lo = hi;
        hi = (hi * 2.0).min(ceiling);
        guard += 1;
        // A shape with no neck keeps holding together all the way to the
        // ceiling; stop there rather than looping on an unchanged bound.
        if hi <= lo || guard >= 64 {
            return None;
        }
    }

    // The bracket is one doubling wide, and inside it the predicate does change
    // exactly once — that is what makes it a bracket.
    for _ in 0..WIDTH_SEARCH_STEPS {
        let mid = (lo + hi) / 2.0;
        if came_apart(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }

    Some(hi * 2.0)
}

/// Connected components of a shape set.
///
/// Holes are not components: a ring eroded to half its bar width is still one
/// ring, and counting the hole as a second piece would report every window in a
/// stencil as a neck.
fn component_count(shapes: &Shapes) -> usize {
    shapes.shapes.len()
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
            arcs: Vec::new(),
        }))
    }

    #[test]
    fn a_plain_band_is_measured() {
        // The case this method is built for, and the one that works.
        for w in [0.8, 1.2, 4.0] {
            let band = rect(0.0, 0.0, 40.0, w);
            let measured = min_width(&band).expect("a width");
            assert!(
                (measured - w).abs() < 1e-3,
                "a {w} mm band measured {measured:.4}"
            );
        }
    }

    #[test]
    fn narrowest_neck_agrees_with_min_width_on_a_plain_band() {
        // They answer different questions in general, so agreement here is a check
        // that the second one is not simply returning something else.
        for w in [0.8, 1.2, 4.0] {
            let band = rect(0.0, 0.0, 40.0, w);
            let neck = narrowest_neck(&band).expect("a band comes apart");
            let width = min_width(&band).expect("a width");
            assert!(
                (neck - width).abs() < 1e-3,
                "band of {w}: neck {neck:.4} vs width {width:.4}"
            );
        }
    }

    #[test]
    fn narrowest_neck_finds_the_bridge_tab_that_min_width_missed() {
        // The whole reason this function exists. A frame, an island inside its
        // hole, and a tab joining them: the tab is the narrowest feature and the
        // only place the piece could come apart.
        for tab in [1.2, 0.6, 0.2] {
            let frame = Shape::with_holes(
                Contour {
                    points: vec![p(0.0, 0.0), p(20.0, 0.0), p(20.0, 20.0), p(0.0, 20.0)],
                    closed: true,
                    arcs: Vec::new(),
                },
                vec![Contour {
                    points: vec![p(5.0, 5.0), p(5.0, 15.0), p(15.0, 15.0), p(15.0, 5.0)],
                    closed: true,
                    arcs: Vec::new(),
                }],
            );
            let island = Shape::new(Contour {
                points: vec![p(8.0, 8.0), p(12.0, 8.0), p(12.0, 12.0), p(8.0, 12.0)],
                closed: true,
                arcs: Vec::new(),
            });
            let before = Shapes {
                shapes: vec![frame, island],
            };
            let (joined, _) = crate::bridge::bridge_unanchored(
                &before,
                crate::bridge::BridgeLimits {
                    width_mm: tab,
                    max_length_mm: 25.0,
                },
            )
            .expect("bridge");

            assert_eq!(joined.shapes.len(), 1, "the tab must make it one piece");
            let neck = narrowest_neck(&joined).expect("it comes apart somewhere");
            assert!(
                (neck - tab).abs() < 1e-2,
                "a {tab} mm tab measured {neck:.4}"
            );

            // And this is the bug: min_width sees the frame, not the tab.
            let width = min_width(&joined).expect("a width");
            assert!(
                width > tab * 2.0,
                "min_width should still be blind to the tab, got {width:.4}"
            );
        }
    }

    #[test]
    fn a_bridge_tab_measures_the_same_however_big_the_part_is() {
        // The regression this search rewrite exists for. The test above used a
        // 20 mm frame, where the answer was right. Scaling the same construction
        // up kept the 0.2 mm tab and broke the measurement: 40 mm reported
        // 6.76 mm, 80 mm reported 13.42, 200 mm reported 33.41 — a mask whose
        // island is held by a fifth of a millimetre of vinyl passing a
        // 0.80 mm minimum, on exactly the sizes that get cut.
        //
        // The cause was the search, not the offset. The component count is not
        // monotonic in the delta — at 40 mm the shape splits at 0.1 mm, is whole
        // again at 2 mm, and splits once more at 3.4 mm — so a bisection started
        // from the ceiling returned whichever crossing its path happened to find.
        for thickness in [5.0, 10.0, 20.0, 50.0, 100.0] {
            let outer = thickness * 4.0;
            let frame = Shape::with_holes(
                Contour {
                    points: vec![p(0.0, 0.0), p(outer, 0.0), p(outer, outer), p(0.0, outer)],
                    closed: true,
                    arcs: Vec::new(),
                },
                vec![Contour {
                    points: vec![
                        p(thickness, thickness),
                        p(thickness, outer - thickness),
                        p(outer - thickness, outer - thickness),
                        p(outer - thickness, thickness),
                    ],
                    closed: true,
                    arcs: Vec::new(),
                }],
            );
            let island = Shape::new(Contour {
                points: vec![
                    p(thickness + 3.0, thickness + 3.0),
                    p(thickness + 7.0, thickness + 3.0),
                    p(thickness + 7.0, thickness + 7.0),
                    p(thickness + 3.0, thickness + 7.0),
                ],
                closed: true,
                arcs: Vec::new(),
            });
            let (joined, _) = crate::bridge::bridge_unanchored(
                &Shapes {
                    shapes: vec![frame, island],
                },
                crate::bridge::BridgeLimits {
                    width_mm: 0.2,
                    max_length_mm: 10_000.0,
                },
            )
            .expect("bridge");

            let neck = narrowest_neck(&joined).expect("a tab always comes apart");
            assert!(
                (neck - 0.2).abs() < 1e-2,
                "a 0.2 mm tab in a {outer} mm part measured {neck:.4}; \
                 the answer must not depend on how big the part is"
            );
        }
    }

    #[test]
    fn a_ring_is_still_measured_as_its_own_thickness_at_every_scale() {
        // The rewrite changed how the bracket is found, so the plain case is
        // pinned too: a ring with no island has no neck other than its bar.
        for thickness in [5.0, 10.0, 20.0, 50.0] {
            let outer = thickness * 4.0;
            let ring = Shapes::single(Shape::with_holes(
                Contour {
                    points: vec![p(0.0, 0.0), p(outer, 0.0), p(outer, outer), p(0.0, outer)],
                    closed: true,
                    arcs: Vec::new(),
                },
                vec![Contour {
                    points: vec![
                        p(thickness, thickness),
                        p(thickness, outer - thickness),
                        p(outer - thickness, outer - thickness),
                        p(outer - thickness, thickness),
                    ],
                    closed: true,
                    arcs: Vec::new(),
                }],
            ));
            let neck = narrowest_neck(&ring).expect("a ring erodes");
            assert!(
                (neck - thickness).abs() < 1e-2,
                "a {thickness} mm ring measured {neck:.4}"
            );
        }
    }

    #[test]
    fn a_thick_unbridged_shape_has_no_neck() {
        // Two disjoint pieces of film are applied one after another, so there is no
        // neck between them and none should be reported.
        let two = Shapes {
            shapes: [
                rect(0.0, 0.0, 5.0, 5.0).shapes,
                rect(40.0, 0.0, 5.0, 5.0).shapes,
            ]
            .concat(),
        };
        // Each piece is 5 mm wide, so each vanishes at 5 mm. There is no neck
        // *between* them: disjoint film is applied one piece at a time.
        let neck = narrowest_neck(&two).expect("each piece comes apart on its own");
        assert!((neck - 5.0).abs() < 1e-3, "expected 5 mm, got {neck:.4}");
    }

    #[test]
    fn an_empty_layer_has_no_neck() {
        assert_eq!(narrowest_neck(&Shapes::new()), None);
    }

    #[test]
    fn narrowest_neck_is_deterministic() {
        let band = rect(0.0, 0.0, 40.0, 2.0);
        assert_eq!(narrowest_neck(&band), narrowest_neck(&band));
    }

    #[test]
    fn a_thin_neck_is_invisible_and_that_is_pinned() {
        // Pinned deliberately: this is a known limitation, not a passing test of
        // correct behaviour. A 4 mm band with a 0.2 mm whisker reports 4 mm,
        // because the method finds the largest inward offset that leaves anything
        // behind and the surrounding mass keeps doing that long after the whisker
        // has closed.
        //
        // If this assertion ever starts failing, the method changed and the doc
        // comment on `min_width` must change with it.
        let band_with_whisker = Shapes::single(Shape::new(Contour {
            points: vec![
                p(0.0, 0.0),
                p(40.0, 0.0),
                p(40.0, 4.0),
                p(20.1, 4.0),
                p(20.0, 4.0),
                p(20.0, 20.0),
                p(19.9, 20.0),
                p(19.9, 4.0),
                p(0.0, 4.0),
            ],
            closed: true,
            arcs: Vec::new(),
        }));
        let measured = min_width(&band_with_whisker).expect("a width");
        assert!(
            (measured - 4.0).abs() < 0.01,
            "the whisker is expected to be missed, got {measured:.4}; \
             if this is now measured, update the docs on min_width"
        );
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
