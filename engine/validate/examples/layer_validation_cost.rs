//! Cost of validating a layer with many regions.
//!
//! `PERF-001` in `docs/BACKLOG/MASTER_BACKLOG.md` and the table in
//! `docs/13_ROADMAP/ROADMAP.md` §8 quote `validate_project` numbers. Those were
//! taken by hand, so a reader could not re-run them, and a change to the
//! expensive half of validation could not be compared against them. This example
//! is the measurement, in the repository, on the same geometry every time:
//! `count` regions of 8 × 3 mm, which is a realistic stencilled part rather
//! than a worst case.
//!
//! Run it from `engine/`:
//!
//! ```text
//! cargo run --release -p mask-validate --example layer_validation_cost
//! ```
//!
//! The point of the numbers is the shape of the curve, not the absolute
//! milliseconds: it is close to linear in the region count, so the budget for a
//! live editor is a question of how many regions fit in a frame, not of an
//! accidental blow-up. `narrowest_neck` is the expensive part — 28 bisection
//! steps over the offset of every region — so it is what this measures.

use std::time::Instant;

use mask_kernel::geom::{Contour, Point, Shape, Shapes};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{validate_with_nesting, LayerInput};

const MATERIAL: &str = include_str!("../../profiles/data/matte-vinyl-090.v4.json");
const PLOTTER: &str = include_str!("../../profiles/data/knife-25mm.v2.json");

/// Regions per row and row pitch, in millimetres. Two centimetres of clearance
/// between rows, which is far above the material's 0.90 mm gap minimum, so the
/// validator is measuring regions and not gaps between them.
const PER_ROW: usize = 50;
const PITCH_MM: f64 = 10.0;
const REGION_MM: (f64, f64) = (8.0, 3.0);

fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn region(index: usize) -> Shape {
    let x = (index % PER_ROW) as f64 * PITCH_MM;
    let y = (index / PER_ROW) as f64 * PITCH_MM;
    let (w, h) = REGION_MM;
    Shape::new(Contour {
        points: vec![
            point(x, y),
            point(x + w, y),
            point(x + w, y + h),
            point(x, y + h),
        ],
        closed: true,
        arcs: Vec::new(),
    })
}

fn layer(count: usize) -> LayerInput {
    LayerInput {
        index: 1,
        name: "Cost probe".to_string(),
        geometry: Shapes {
            shapes: (0..count).map(region).collect(),
        },
        bridges: Vec::new(),
        application_order: None,
    }
}

fn main() {
    let material: MaterialProfile =
        serde_json::from_str(MATERIAL).expect("the material profile parses");
    let plotter: PlotterProfile =
        serde_json::from_str(PLOTTER).expect("the plotter profile parses");
    let cutting = CuttingProfile::derive(&material, &plotter);
    let rules = RuleSet::from_profile(&cutting);

    // One untimed run, so the numbers below are not the first call's cold cache.
    let _ = validate_with_nesting(&[layer(100)], &cutting, &rules, false, &Default::default());

    println!("regions | validation");
    for count in [100_usize, 1_000, 5_000, 20_000] {
        let layers = [layer(count)];
        let repetitions = if count <= 1_000 { 5 } else { 1 };
        let start = Instant::now();
        for _ in 0..repetitions {
            let report =
                validate_with_nesting(&layers, &cutting, &rules, false, &Default::default());
            // Touch the report so nothing can be optimised away as unused.
            assert_eq!(report.layers.len(), 1);
        }
        let ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(repetitions);
        println!("{count:>7} | {ms:>8.1} ms");
    }
}
