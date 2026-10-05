//! Geometry library acceptance probe for MASK PLATFORM (ADR-0003).
//!
//! Measures the acceptance criteria against the real library instead of
//! restating its marketing. Every number printed here is measured.
//!
//! Criteria:
//!   1. offset / widen with an exact target delta
//!   2. boolean ops on the shapes we actually draw (crossings, cusps,
//!      touching edges, nested contours, self-intersections)
//!   3. determinism: identical results across repeats, engines and MathMode
//!   4. absence of panics on hostile input
//!   5. wasm32 target compiles (checked separately, see wasm_check.sh)

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::panic::{catch_unwind, AssertUnwindSafe};

use i_overlay::core::fill_rule::FillRule;
use i_overlay::core::overlay_rule::OverlayRule;
use i_overlay::float::scale::FixedScaleFloatOverlay;
use i_overlay::mesh::float::stroke::offset::StrokeOffset;
use i_overlay::mesh::float::style::{LineCap, LineJoin, StrokeStyle};
use i_overlay::mesh::math::MathMode;

/// CANON_QUANTUM_MM = 1e-4 (ADR-0004). Scale = 1 / quantum = 1e4.
const GRID: f64 = 1.0e-4;
const SCALE: f64 = 1.0 / GRID;

type Pt = [f64; 2];
type Contour = Vec<Pt>;
/// One region: first contour outer, the rest holes.
type Shape = Vec<Contour>;
/// A boolean operand / result: a list of disjoint regions.
type Shapes = Vec<Shape>;

/// Empty operand for single-argument overlay calls.
static EMPTY: Shapes = Vec::new();

fn wrap(contours: Vec<Contour>) -> Shapes {
    vec![contours]
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Contour {
    vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
}

/// Signed shoelace area of a single contour.
/// i_overlay guarantees outer contours are counter-clockwise (positive signed
/// area) and holes are clockwise (negative), so summing signed areas across a
/// shape subtracts holes automatically. Taking `abs()` per contour would add
/// holes instead — which is exactly what an early version of this probe did.
fn contour_area(contour: &Contour) -> f64 {
    let mut a = 0.0;
    for i in 0..contour.len() {
        let p = contour[i];
        let q = contour[(i + 1) % contour.len()];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a / 2.0
}

/// Area of a shape with holes subtracted (signed sum).
fn shape_area(shape: &Shape) -> f64 {
    shape.iter().map(contour_area).sum()
}

fn total_area(shapes: &Shapes) -> f64 {
    shapes.iter().map(shape_area).sum()
}

fn hole_count(shapes: &Shapes) -> usize {
    shapes.iter().map(|s| s.len().saturating_sub(1)).sum()
}

fn hash_shapes(shapes: &Shapes) -> u64 {
    let mut h = DefaultHasher::new();
    for s in shapes {
        for c in s {
            for p in c {
                p[0].to_bits().hash(&mut h);
                p[1].to_bits().hash(&mut h);
            }
            0xffu8.hash(&mut h);
        }
        0xfeu8.hash(&mut h);
    }
    h.finish()
}

fn overlay_i64(a: &Shapes, b: &Shapes, rule: OverlayRule) -> Result<Shapes, String> {
    a.overlay_with_fixed_scale_as::<i64>(b, rule, FillRule::EvenOdd, SCALE)
        .map_err(|e| format!("{e:?}"))
}

fn hr(title: &str) {
    println!("\n\x1b[1m{title}\x1b[0m");
    println!("{}", "─".repeat(title.len()));
}

fn main() {
    println!("MASK PLATFORM · geometry acceptance probe");
    println!(
        "grid = {GRID} mm ({:.1} µm), integer engine scale = {SCALE}",
        GRID * 1000.0
    );

    // ─────────────────────────────────────────────────────────────────
    hr("1. BOOLEAN on our shapes (integer engine i64, fixed scale 1e4)");

    // 1a. crossing bands — the "interlocking lines" primitive.
    let h = wrap(vec![rect(0.0, 9.5, 20.0, 10.5)]);
    let v = wrap(vec![rect(9.5, 0.0, 10.5, 20.0)]);
    match overlay_i64(&h, &v, OverlayRule::Union) {
        Ok(r) => println!(
            "  crossing union     : area {:.6} mm² (expect 39.000000), shapes {}, hash {:016x}",
            total_area(&r),
            r.len(),
            hash_shapes(&r)
        ),
        Err(e) => println!("  crossing union     : ERROR {e}"),
    }
    match overlay_i64(&h, &v, OverlayRule::Intersect) {
        Ok(r) => println!(
            "  crossing intersect : area {:.6} mm² (expect 1.000000), shapes {}",
            total_area(&r),
            r.len()
        ),
        Err(e) => println!("  crossing intersect : ERROR {e}"),
    }

    // 1b. touching edges — two rects sharing a full edge must merge.
    let left = wrap(vec![rect(0.0, 0.0, 10.0, 5.0)]);
    let right = wrap(vec![rect(10.0, 0.0, 20.0, 5.0)]);
    match overlay_i64(&left, &right, OverlayRule::Union) {
        Ok(r) => println!(
            "  touching union     : area {:.6} mm² (expect 100.000000), shapes {} [{}]",
            total_area(&r),
            r.len(),
            if r.len() == 1 { "merged" } else { "NOT MERGED" }
        ),
        Err(e) => println!("  touching union     : ERROR {e}"),
    }

    // 1c. coincident edges — identical rects.
    match overlay_i64(&left, &left, OverlayRule::Union) {
        Ok(r) => println!(
            "  coincident union   : area {:.6} mm² (expect 50.000000), shapes {}",
            total_area(&r),
            r.len()
        ),
        Err(e) => println!("  coincident union   : ERROR {e}"),
    }

    // 1d. nested contours — the hole must survive an inner union.
    // NOTE the hole spans 5..15 in both axes, i.e. 10x10 = 100 mm².
    // Signed area: outer 20x20 = +400, hole = -100, inner 1x1 = +1 => 301.
    let with_hole = wrap(vec![rect(0.0, 0.0, 20.0, 20.0), rect(5.0, 5.0, 15.0, 15.0)]);
    let inner = wrap(vec![rect(7.0, 7.0, 8.0, 8.0)]);
    match overlay_i64(&with_hole, &inner, OverlayRule::Union) {
        Ok(r) => println!(
            "  nested union       : area {:.6} mm² (expect 301.000000, hole subtracted), shapes {}, holes {}",
            total_area(&r),
            r.len(),
            hole_count(&r)
        ),
        Err(e) => println!("  nested union       : ERROR {e}"),
    }

    // 1e. self-intersecting contour (bowtie) must be resolved, not propagated.
    let bowtie = wrap(vec![vec![
        [0.0, 0.0],
        [10.0, 10.0],
        [10.0, 0.0],
        [0.0, 10.0],
    ]]);
    match overlay_i64(&bowtie, &EMPTY, OverlayRule::Union) {
        Ok(r) => println!(
            "  bowtie self-fix    : area {:.6} mm² (expect 50.000000), shapes {}",
            total_area(&r),
            r.len()
        ),
        Err(e) => println!("  bowtie self-fix    : ERROR {e}"),
    }

    // 1f. acute cusp at 10 degrees.
    let d = 10.0f64.to_radians();
    let cusp = wrap(vec![vec![
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0 * d.cos(), 20.0 * d.sin()],
    ]]);
    match overlay_i64(&cusp, &EMPTY, OverlayRule::Union) {
        Ok(r) => println!(
            "  acute cusp 10°     : shapes {}, contours {}, area {:.6} mm²",
            r.len(),
            r.iter().map(|s| s.len()).sum::<usize>(),
            total_area(&r)
        ),
        Err(e) => println!("  acute cusp 10°     : ERROR {e}"),
    }

    // ─────────────────────────────────────────────────────────────────
    hr("2. OFFSET / WIDEN with an exact target width");
    // Auto-fix 'widen' = stroke the centreline to the target band width.
    let line: Vec<Pt> = vec![[0.0, 0.0], [100.0, 0.0]];
    for target in [0.80, 0.42, 0.05, 0.01] {
        for math in [MathMode::Integer, MathMode::Float] {
            let mode = if math == MathMode::Integer {
                "Integer"
            } else {
                "Float   "
            };
            let style = StrokeStyle::new(target)
                .line_join(LineJoin::Miter(1.0))
                .start_cap(LineCap::Butt)
                .end_cap(LineCap::Butt)
                .math(math);
            match catch_unwind(AssertUnwindSafe(|| line.stroke(style, false))) {
                Ok(shapes) => {
                    let a = total_area(&shapes);
                    let measured = if a > 0.0 { a / 100.0 } else { 0.0 };
                    println!(
                        "  target {target:>5.2} mm  math={mode}  shapes {:<2} area {a:>12.6} mm²  width {measured:.6} mm  hash {:016x}",
                        shapes.len(),
                        hash_shapes(&shapes)
                    );
                }
                Err(_) => println!("  target {target:>5.2} mm  math={mode}  PANIC"),
            }
        }
    }

    hr("2b. COLLAPSE behaviour (element thinner than the target)");
    for target in [0.004, 0.001, 0.0001, 0.00001] {
        let style = StrokeStyle::new(target)
            .line_join(LineJoin::Miter(1.0))
            .start_cap(LineCap::Butt)
            .end_cap(LineCap::Butt)
            .math(MathMode::Integer);
        match catch_unwind(AssertUnwindSafe(|| line.stroke(style, false))) {
            Ok(shapes) => println!(
                "  target {target:>8.5} mm  shapes {:<2} area {:.8} mm²  {}",
                shapes.len(),
                total_area(&shapes),
                if shapes.is_empty() {
                    "collapsed"
                } else {
                    "survived"
                }
            ),
            Err(_) => println!("  target {target:>8.5} mm  PANIC"),
        }
    }

    // ─────────────────────────────────────────────────────────────────
    hr("3. DETERMINISM");

    let hashes: Vec<u64> = (0..5)
        .map(|_| {
            overlay_i64(&h, &v, OverlayRule::Union)
                .map(|s| hash_shapes(&s))
                .unwrap_or(0)
        })
        .collect();
    let stable = hashes.windows(2).all(|w| w[0] == w[1]);
    println!(
        "  boolean repeat x5  : identical = {stable}  [{:016x}]",
        hashes[0]
    );

    let e32 = h
        .overlay_with_fixed_scale_as::<i32>(&v, OverlayRule::Union, FillRule::EvenOdd, SCALE)
        .map(|s| (total_area(&s), hash_shapes(&s)));
    let e64 = overlay_i64(&h, &v, OverlayRule::Union).map(|s| (total_area(&s), hash_shapes(&s)));
    println!("  engine i32         : {e32:?}");
    println!("  engine i64         : {e64:?}");
    if let (Ok((a32, h32)), Ok((a64, h64))) = (e32, e64) {
        println!(
            "  engine agreement   : area delta {:.9} mm² ({}), hash equal = {}",
            (a32 - a64).abs(),
            if (a32 - a64).abs() < GRID {
                "within grid"
            } else {
                "EXCEEDS GRID"
            },
            h32 == h64
        );
    }

    let poly: Vec<Pt> = vec![[0.0, 0.0], [37.0, 11.0], [91.0, 4.0]];
    let mut stroke_hashes = Vec::new();
    for math in [MathMode::Integer, MathMode::Float] {
        let style = StrokeStyle::new(0.8)
            .line_join(LineJoin::Round(0.05))
            .start_cap(LineCap::Round(0.05))
            .end_cap(LineCap::Round(0.05))
            .math(math);
        stroke_hashes.push(hash_shapes(&poly.stroke(style, false)));
    }
    println!(
        "  stroke MathMode    : Integer {:016x}  Float {:016x}  equal = {}",
        stroke_hashes[0],
        stroke_hashes[1],
        stroke_hashes[0] == stroke_hashes[1]
    );

    // ─────────────────────────────────────────────────────────────────
    hr("4. PANIC SAFETY on hostile input");

    let hostile: Vec<(&str, Shapes, Shapes)> = vec![
        (
            "NaN coordinates",
            wrap(vec![vec![[f64::NAN, 0.0], [1.0, 1.0]]]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        (
            "infinite coordinate",
            wrap(vec![vec![[f64::INFINITY, 0.0], [1.0, 1.0]]]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        (
            "empty contour",
            wrap(vec![vec![]]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        (
            "single point",
            wrap(vec![vec![[0.0, 0.0]]]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        (
            "duplicate points",
            wrap(vec![vec![[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        (
            "zero-length segment",
            wrap(vec![vec![[5.0, 5.0], [5.0, 5.0], [9.0, 9.0]]]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        (
            "huge coordinates 1e12",
            wrap(vec![rect(0.0, 0.0, 1.0e12, 1.0e12)]),
            wrap(vec![rect(0.0, 0.0, 1.0, 1.0)]),
        ),
        ("both empty", wrap(vec![vec![]]), wrap(vec![vec![]])),
    ];

    let hostile_total = hostile.len();
    let mut survived = 0;
    for (name, a, b) in hostile {
        match catch_unwind(AssertUnwindSafe(|| overlay_i64(&a, &b, OverlayRule::Union))) {
            Ok(Ok(shapes)) => {
                println!(
                    "  {name:<24} ok, shapes {}, area {:.6}",
                    shapes.len(),
                    total_area(&shapes)
                );
                survived += 1;
            }
            Ok(Err(e)) => println!("  {name:<24} rejected: {e}"),
            Err(_) => println!("  {name:<24} \x1b[31mPANIC\x1b[0m"),
        }
    }
    println!("  {survived}/{hostile_total} returned a value rather than erroring");

    // ─────────────────────────────────────────────────────────────────
    hr("5. DETERMINISTIC RANDOM SWEEP (seeded LCG, 400 iterations)");
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((state >> 33) as f64 / (1u64 << 31) as f64) * 100.0
    };

    let (mut ok, mut err, mut panicked) = (0, 0, 0);
    for i in 0..400 {
        let n = 3 + (i % 5);
        let a: Contour = (0..n).map(|_| [next(), next()]).collect();
        let b: Contour = (0..n).map(|_| [next(), next()]).collect();
        let rule = match i % 4 {
            0 => OverlayRule::Union,
            1 => OverlayRule::Difference,
            2 => OverlayRule::Intersect,
            _ => OverlayRule::Xor,
        };
        match catch_unwind(AssertUnwindSafe(|| {
            overlay_i64(&wrap(vec![a.clone()]), &wrap(vec![b.clone()]), rule)
        })) {
            Ok(Ok(_)) => ok += 1,
            Ok(Err(_)) => err += 1,
            Err(_) => panicked += 1,
        }
    }
    println!("  random booleans    : ok {ok}, rejected {err}, panicked {panicked}");

    // ─────────────────────────────────────────────────────────────────
    hr("SUMMARY");
    println!("  grid {GRID} mm, engine i64, MathMode selectable.");
    println!("  rerun this binary and diff the output to confirm reproducibility.");
}
