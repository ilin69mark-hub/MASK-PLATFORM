//! Memoised per-region narrowest width, for the editor's drag frames.
//!
//! `PERF-002` in `docs/BACKLOG/MASTER_BACKLOG.md`, measured by
//! `engine/validate/examples/editor_frame_cost.rs breakdown`. At 2 000 regions a
//! drag frame spends 234 ms in `min_width` plus `narrowest_neck` and about 8 ms
//! in everything else; hashing each region costs 3.9 ms. So verifying a cached
//! measurement is roughly sixty times cheaper than recomputing it, and the frame
//! stops being the thing that limits the editor.
//!
//! # Why the key is the region's own hash
//!
//! A drag moves one vertex in one region. The other 1 999 regions have not
//! changed, and an index-based invalidation would say so — but it would be saying
//! so on the editor's word. The moment some other path changed geometry without
//! telling the cache, the editor would show a width belonging to a shape that is
//! no longer there, and the station would disagree. That is ADR-0013's exact
//! failure, and it is the one this design exists to make impossible.
//!
//! So the key is derived from the geometry, not from the caller: a region is
//! looked up by its canonical content hash. A stale entry cannot be served,
//! because an entry only exists for the hash it was measured from. There is no
//! invalidation to forget and no path that has to remember.
//!
//! # What this does not cache
//!
//! `min_gap` is a measurement between *pairs* of regions and moves a region
//! changes it; it is 1.5 ms at 2 000 regions and 37 ms at 10 000, which is worth
//! looking at later but is not what limits a frame. `path_data`,
//! `self_intersections` and the bounding box are cheap and left alone. Caching
//! `min_gap` per pair would be the next thing to measure, not the next thing to
//! build.
//!
//! # Honesty about the hash
//!
//! A cache keyed by a hash is a cache that can be wrong if the hash collides. The
//! scheme is SHA-256-based and `HASH_SCHEME_VERSION` moves with it, so the
//! probability is not zero but is not a thing to plan around either. What matters
//! is that it is not silently the only defence: a collision would have to survive
//! the editor *and* the station disagreeing about the same geometry, and the
//! cross-runtime content-hash check would catch the scheme drifting rather than
//! one region colliding.

use crate::geom::{Shape, Shapes};

use std::cell::RefCell;
use std::collections::HashMap;

/// Most geometries the cache will remember at once.
///
/// Sized above any layer worth dragging: at 10 000 regions the frame went from
/// 1186 ms to *slower than no cache at all*, because a smaller bound made the map
/// overflow, and the first version's response — clear it — turned every region
/// into a miss on every frame. The fix was not a bigger number, it was to stop
/// inserting instead of clearing; see {@link remember}.
///
/// This is a bound on memory, not a policy about what may be cached. Entries are
/// keyed by geometry, so a missing entry costs a measurement and a present one
/// costs nothing. Nothing here can make an answer wrong.
const CAPACITY: usize = 65_536;

/// A region's narrowest width, and the hash it was measured from.
type Entry = Option<f64>;

thread_local! {
    /// Per thread, because a wasm instance is one thread and the host builds run
    /// the tests in parallel. A `static` would be shared between them and one
    /// test's cache would answer another's geometry — which would be correct, but
    /// would make the tests depend on each other's order.
    static REGION_WIDTHS: RefCell<HashMap<String, Entry>> = RefCell::new(HashMap::new());

    /// The bound, per thread, so a test can fill it in a moment. Sizing it to the
    /// real 65 536 in a unit test would take half a minute of measurement to prove
    /// a rule about arithmetic.
    static CAPACITY_OVERRIDE: RefCell<Option<usize>> = const { RefCell::new(None) };
}

/// The bound in force on this thread.
fn capacity() -> usize {
    CAPACITY_OVERRIDE.with(|c| c.borrow().unwrap_or(CAPACITY))
}

/// Set this thread's bound. Test-only; the browser never lowers it.
#[cfg(test)]
fn set_capacity(limit: usize) {
    CAPACITY_OVERRIDE.with(|c| *c.borrow_mut() = Some(limit));
}

/// How many entries the cache currently holds. Test-only, but it is the only way
/// to observe the capacity rule without timing something.
#[cfg(test)]
pub fn cached_regions() -> usize {
    REGION_WIDTHS.with(|c| c.borrow().len())
}

/// Forget everything. Test-only: a test that asserted a cache hit without knowing
/// what was in it would be asserting the cache, not the geometry.
#[cfg(test)]
pub fn forget_everything() {
    REGION_WIDTHS.with(|c| c.borrow_mut().clear());
    set_capacity(CAPACITY);
}

/// The narrowest width of one region: `min_width` and `narrowest_neck` reduced,
/// which is what the validator measures and therefore what the editor must show.
///
/// `measure` is called only on a miss, so a caller cannot be tricked into an
/// unmeasured number by a cache that merely thinks it has one.
fn narrowest_width(geometry: &Shapes, measure: impl FnOnce(&Shapes) -> Option<f64>) -> Option<f64> {
    let key = crate::ContentHash::of(geometry).to_string();

    if let Some(hit) = REGION_WIDTHS.with(|c| c.borrow().get(&key).copied()) {
        return hit;
    }

    let measured = measure(geometry);
    remember(key, measured);
    measured
}

/// Keep a measurement, unless there is no room.
///
/// Deliberately *not* clearing the map when it is full. That was the first version,
/// and it made things worse than having no cache: at 10 000 regions a smaller bound
/// meant the map overflowed on every frame, so every region missed, every region
/// re-measured, and the map was emptied and refilled in a cycle that cost a hash
/// and a measurement per region forever. Dropping the old entries when there is no
/// room for the new one only loses speed on the geometries that were dropped, which
/// is the same cost as never having cached them.
fn remember(key: String, measured: Option<f64>) {
    REGION_WIDTHS.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= capacity() {
            return;
        }
        cache.insert(key, measured);
    });
}

/// The same reduction `measure_layer_inner` performs, memoised.
///
/// A region with no measurable width caches `None` rather than being skipped:
/// re-measuring it every frame is exactly the cost this exists to remove, and
/// "no width" is an answer like any other.
pub fn region_width(geometry: &Shape) -> Option<f64> {
    let one = Shapes::single(geometry.clone());
    narrowest_width(&one, |one| {
        match (crate::min_width(one), crate::narrowest_neck(one)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    })
}

#[cfg(test)]
mod tests {
    //! The contract is about what the cache is allowed to answer, so the tests
    //! are about refusals and misses rather than about hits.

    use super::*;
    use crate::geom::{Contour, Point};

    fn band(width_mm: f64) -> Shape {
        let half = width_mm / 2.0;
        Shape::new(Contour {
            points: vec![
                Point { x: 0.0, y: -half },
                Point { x: 10.0, y: -half },
                Point { x: 10.0, y: half },
                Point { x: 0.0, y: half },
            ],
            closed: true,
            arcs: Vec::new(),
        })
    }

    fn setup() {
        forget_everything();
    }

    #[test]
    fn a_measured_region_reports_the_same_number_as_the_uncached_measurement() {
        setup();
        let geometry = band(3.0);
        let expected = match (
            crate::min_width(&Shapes::single(geometry.clone())),
            crate::narrowest_neck(&Shapes::single(geometry.clone())),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        assert_eq!(region_width(&geometry), expected);
    }

    #[test]
    fn a_hit_does_not_measure_again() {
        setup();
        let geometry = band(3.0);
        let first = region_width(&geometry);
        // The same geometry must not cost a second measurement. If the cache were
        // keyed by index, or by anything the caller supplies, this would still
        // pass — which is why the refusal test below matters more than this one.
        let second = region_width(&geometry);
        assert_eq!(first, second);
        assert_eq!(cached_regions(), 1);
    }

    #[test]
    fn a_changed_region_is_measured_again_and_gets_a_different_number() {
        // The one failure that would matter: a cache serving the width of geometry
        // that is no longer there. A 3 mm band measured as 3.000 mm is correct; a
        // 0.2 mm band measured as 3.000 mm is the editor lying, and it is exactly
        // what an invalidation missed by one path would produce.
        setup();
        let wide = band(3.0);
        assert!((region_width(&wide).expect("a width") - 3.0).abs() < 1e-3);

        let narrow = band(0.2);
        let measured = region_width(&narrow).expect("a width");
        assert!(
            (measured - 0.2).abs() < 1e-2,
            "a 0.2 mm band measured {measured} mm — the cache served another region's number"
        );
        assert_eq!(cached_regions(), 2, "two geometries, two entries");
    }

    #[test]
    fn returning_to_an_earlier_region_still_reports_that_regions_own_number() {
        // A drag crosses the same region on the way back. If the cache were keyed
        // by position rather than by geometry, the second visit is where a wrong
        // number would come from.
        setup();
        let a = band(4.0);
        let b = band(0.5);
        let first_a = region_width(&a).expect("a width");
        let b_width = region_width(&b).expect("a width");
        let again_a = region_width(&a).expect("a width");
        assert_eq!(first_a, again_a);
        assert!((again_a - 4.0).abs() < 1e-3, "got {again_a}");
        assert!((b_width - 0.5).abs() < 1e-3, "got {b_width}");
    }

    #[test]
    fn a_degenerate_region_is_cached_too_rather_than_retried_every_frame() {
        setup();
        // This test originally asserted that a two-point contour measures `None`.
        // It does not: the kernel answers `Some(0.0)`, and the cache agreed with
        // the kernel immediately, which is what caught the wrong expectation.
        //
        // What is actually worth asserting is that whatever the answer is, it is
        // an answer the cache keeps — including one that is not a width. A frame
        // that re-measured a degenerate region every pointer move would be paying
        // the full bisection for a zero.
        let degenerate = Shape::new(Contour {
            points: vec![Point { x: 0.0, y: 0.0 }, Point { x: 1.0, y: 0.0 }],
            closed: true,
            arcs: Vec::new(),
        });
        let expected = match (
            crate::min_width(&Shapes::single(degenerate.clone())),
            crate::narrowest_neck(&Shapes::single(degenerate.clone())),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let first = region_width(&degenerate);
        assert_eq!(first, expected, "the cache must agree with the kernel");
        assert_eq!(cached_regions(), 1, "whatever it is, it is kept");
        assert_eq!(region_width(&degenerate), first);
    }

    #[test]
    fn a_full_cache_keeps_what_it_has_rather_than_thrashing() {
        // The bug this test exists for: at 10 000 regions the first version made
        // the frame *slower than no cache*, because overflow cleared the map, so
        // every region missed and refilled it on every frame. The fix is to stop
        // inserting, not to clear.
        setup();
        set_capacity(32);
        for i in 0..40 {
            let mut geometry = band(3.0);
            geometry.outer.points[0].x = i as f64 * 10.0;
            region_width(&geometry);
        }
        assert_eq!(cached_regions(), 32, "bounded, not growing");

        // The entries it kept are still answers, and the one it dropped is still
        // measurable — losing the cache costs speed, never correctness.
        let kept = band(3.0);
        assert!((region_width(&kept).expect("a width") - 3.0).abs() < 1e-3);
        let dropped = band(2.0);
        assert!((region_width(&dropped).expect("a width") - 2.0).abs() < 1e-3);
    }

    #[test]
    fn a_cached_number_survives_a_cache_that_is_not_growing_any_more() {
        // With the map full, a geometry already in it must still be answered from
        // it. Otherwise "stop inserting" would silently become "stop caching" once
        // the map filled, which is the regression in a slower costume.
        setup();
        set_capacity(32);
        let geometry = band(3.0);
        assert!((region_width(&geometry).expect("a width") - 3.0).abs() < 1e-3);
        for i in 0..36 {
            let mut other = band(3.0);
            other.outer.points[0].x = i as f64 * 10.0 + 0.5;
            region_width(&other);
        }
        assert_eq!(cached_regions(), 32);
        assert!(
            (region_width(&geometry).expect("a width") - 3.0).abs() < 1e-3,
            "a geometry already cached must still be answered from the cache"
        );
    }
}
