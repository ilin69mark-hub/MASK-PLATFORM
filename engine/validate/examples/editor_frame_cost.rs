//! What one frame of dragging a vertex actually costs, on a layer worth dragging.
//!
//! `PERF-003` in `docs/BACKLOG/MASTER_BACKLOG.md`. The only performance number the
//! editor had was `proof_panel.json` — 13 bands, 83 vertices, 27 ms — which is a
//! fixture, not a workload, and cannot be optimised against.
//!
//! Three numbers, because the three costs are paid in different places and only
//! one of them is the geometry:
//!
//! 1. **A drag frame.** What `move_vertex` does on every pointer move: parse, move
//!    the vertex, canonicalise, validate, serialise, parse again, measure. This is
//!    the one with a budget — the browser has 16 ms and shares them with drawing.
//! 2. **A release.** What the editor runs when the pointer comes up, now that
//!    `VALIDATE-001` made it run: `validate_project` over every layer.
//! 3. **The bisection share.** How much of the frame is `narrowest_neck`, which
//!    bisects the offset 28 times per region. It is the suspected expensive half
//!    and this measures it rather than asserting it.
//!
//! The JSON hops are inside the frame on purpose. The browser pays `JSON.stringify`
//! and a `serde_json` parse *twice* per pointer move, and a measurement that left
//! them out would describe a frame the editor does not run.
//!
//! # One measurement per process
//!
//! Run without arguments, this prints the table. Run with a name, it measures
//! that one thing and prints one number per region count.
//!
//! ```text
//! cargo run --release -p mask-validate --example editor_frame_cost
//! cargo run --release -p mask-validate --example editor_frame_cost frame
//! ```
//!
//! The split is not tidiness. Measured in one process, the three numbers depend on
//! what ran before them: after nine drag frames, the same validation of the same
//! 100 regions took 51 ms instead of 13, and the bisection 28 ms instead of 7,
//! because the frames leave the allocator holding freed blocks and every later
//! measurement pays for the pages. Moving the blocks around changes the published
//! numbers by a factor of four, which would make this table the least reliable
//! thing in the repository. Each measurement gets its own process, so the number
//! is the cost of the operation rather than the cost of having measured something
//! else first.
//!
//! Each figure is the fastest of its repetitions, for the same reason: a minimum
//! is the run that met the condition with the least interference. It is not enough,
//! though, and the closing note says so: a busy machine does not merely slow these
//! numbers down, it makes some of them impossible.

use std::process::Command;
use std::time::{Duration, Instant};

use mask_kernel::geom::{Contour, Point, Shape, Shapes};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{validate_with_registration, LayerInput};

const MATERIAL: &str = include_str!("../../profiles/data/matte-vinyl-090.v4.json");
const PLOTTER: &str = include_str!("../../profiles/data/knife-25mm.v2.json");

/// Regions per row and row pitch, in millimetres. Two centimetres between rows,
/// far above the material's 0.90 mm gap minimum, so this measures regions and not
/// gaps between them — the same geometry `layer_validation_cost` uses, so the two
/// tables can be read against each other.
const PER_ROW: usize = 50;
const PITCH_MM: f64 = 10.0;
const REGION_MM: (f64, f64) = (8.0, 3.0);

/// What is measured. One at a time, in its own process.
#[derive(Clone, Copy, PartialEq)]
enum What {
    Frame,
    Release,
    Neck,
}

/// Region counts to report.
const COUNTS: [usize; 4] = [100, 500, 2_000, 10_000];

/// A browser frame gets 16 ms, and the frame is what has to fit in it.
const FRAME_BUDGET_MS: f64 = 16.0;

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

fn shapes(count: usize) -> Shapes {
    Shapes {
        shapes: (0..count).map(region).collect(),
    }
}

fn layer(count: usize) -> LayerInput {
    LayerInput {
        index: 1,
        name: "Frame cost probe".to_string(),
        geometry: shapes(count),
        bridges: Vec::new(),
        application_order: None,
    }
}

/// One drag frame: the whole of `move_vertex_inner`, JSON hops included.
///
/// Returns the JSON text, so the next frame starts where a real drag's would. The
/// browser only ever holds geometry as a string; timing a warm in-memory `Shapes`
/// would be timing a different program.
fn frame(shapes_json: &str, vertex: usize, to_x: f64) -> String {
    let mut shapes: Shapes = serde_json::from_str(shapes_json).expect("the layer parses");

    // The neighbour checks `move_vertex_inner` runs before it moves anything.
    let target = mask_kernel::canonical::quantise_point(Point { x: to_x, y: 0.0 });
    let points = &mut shapes.shapes[0].outer.points;
    for n in [vertex.wrapping_sub(1), vertex + 1] {
        if let Some(p) = points.get(n) {
            let neighbour = mask_kernel::canonical::quantise_point(Point { x: p.x, y: p.y });
            assert_ne!(neighbour, target, "the probe would have been refused");
        }
    }
    points[vertex] = Point { x: to_x, y: 0.0 };

    let canonical = mask_kernel::canonicalise_shapes(&shapes);
    canonical
        .validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .expect("a moved vertex is still valid geometry");

    let text = serde_json::to_string(&canonical).expect("canonical geometry serialises");

    // `measure_layer_inner` takes the text and parses it, so the frame ends here.
    let measured: Shapes = serde_json::from_str(&text).expect("the frame's output parses");
    let narrowest = measured
        .shapes
        .iter()
        .filter_map(|s| {
            let one = Shapes::single(s.clone());
            match (
                mask_kernel::min_width(&one),
                mask_kernel::narrowest_neck(&one),
            ) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            }
        })
        .reduce(f64::min);
    assert!(narrowest.is_some(), "every region has a measurable width");

    text
}

/// Just the bisection, over the same regions, so the share is measured rather than
/// estimated: the same call, the same regions, the same count.
fn neck_only(geometry: &Shapes) {
    let narrowest = geometry
        .shapes
        .iter()
        .filter_map(|s| mask_kernel::narrowest_neck(&Shapes::single(s.clone())))
        .reduce(f64::min);
    assert!(narrowest.is_some());
}

fn best(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[0]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn profiles() -> (CuttingProfile, RuleSet) {
    let material: MaterialProfile =
        serde_json::from_str(MATERIAL).expect("the material profile parses");
    let plotter: PlotterProfile =
        serde_json::from_str(PLOTTER).expect("the plotter profile parses");
    let cutting = CuttingProfile::derive(&material, &plotter);
    let rules = RuleSet::from_profile(&cutting);
    (cutting, rules)
}

/// The one-minute load average, or `None` where it cannot be read.
///
/// Used to refuse to publish a table rather than publish a wrong one. The reason
/// is not politeness about noise: the estimator here is a minimum over a fixed
/// number of repetitions, and under sustained load a longer operation is exposed
/// to more scheduling events than a shorter one. The share column then comes out
/// above 100% — `narrowest_neck` reported as more than the whole frame that
/// contains it — which is not a slow measurement but a broken one.
fn load_average() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("sysctl")
            .args(["-n", "vm.loadavg"])
            .output()
            .ok()?;
        // `vm.loadavg` prints `{ 5.81 9.17 10.52 }` — braces included, and separated
        // by spaces, so the first whitespace-separated token is the lone string
        // `{`. Two earlier versions failed here: one parsed that token, and one
        // trimmed the braces off it and got an empty string. Both returned
        // `None`, which made this check skip itself silently on macOS — a guard
        // that never runs is worse than no guard, because it reads as one that
        // passed. So the braces are turned into separators first.
        let text = String::from_utf8_lossy(&out.stdout).replace(['{', '}'], " ");
        text.split_whitespace().next()?.parse().ok()
    }
    #[cfg(target_os = "linux")]
    {
        let first = std::fs::read_to_string("/proc/loadavg").ok()?;
        first.split_whitespace().next()?.parse().ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

/// Time one thing, at one size, in this process.
///
/// Everything the operation needs is built before the loop. Parsing the two
/// profile files and serialising the starting geometry are setup, not the work,
/// and timing them would inflate the two columns that were supposed to be
/// comparable — a version of this harness timed `profiles()` inside the loop and
/// reported a release that cost more than the frame it happens on.
///
/// `repetitions` is smaller for the large sizes because a single 10 000-region
/// frame is already seconds and the point of the large sizes is the shape of the
/// curve, not its third decimal place.
fn measure(what: What, count: usize) -> f64 {
    let repetitions = if count <= 2_000 { 9 } else { 3 };
    let geometry = shapes(count);

    // The text the frame starts from, built once: the browser already holds the
    // layer as a string and does not rebuild it to drag a vertex.
    let start_json = serde_json::to_string(&geometry).expect("the layer serialises");
    let (cutting, rules) = profiles();
    let layers = [layer(count)];

    // Warm once, so the reported run is not a cold allocator's.
    match what {
        What::Frame => {
            std::hint::black_box(frame(&start_json, 1, 0.05));
        }
        What::Release => {
            let _ = validate_with_registration(
                &layers,
                &cutting,
                &rules,
                false,
                &Default::default(),
                &Default::default(),
            );
        }
        What::Neck => neck_only(&geometry),
    }

    let mut samples = Vec::with_capacity(repetitions);
    for step in 0..repetitions {
        let started = Instant::now();
        match what {
            What::Frame => {
                // The pointer walks a vertex back and forth, as it does when a
                // user moves it. Only the frame itself is inside the timer.
                std::hint::black_box(frame(&start_json, 1, 0.05 + (step % 2) as f64 * 0.1));
            }
            What::Release => {
                let report = validate_with_registration(
                    &layers,
                    &cutting,
                    &rules,
                    false,
                    &Default::default(),
                    &Default::default(),
                );
                assert_eq!(report.layers.len(), 1);
            }
            What::Neck => neck_only(&geometry),
        }
        samples.push(started.elapsed());
    }
    ms(best(samples))
}

/// Measure one thing at every size, in this process, one number per line.
fn run_child(what: What) {
    for count in COUNTS {
        let value = measure(what, count);
        println!("{count}\t{value:.3}");
    }
}

/// Measure each thing in a process of its own and print the table.
fn run_table() {
    // Refuse to publish rather than publish a wrong one. A busy machine does not
    // merely make these numbers slower: because the estimator is a minimum over a
    // fixed number of repetitions, the longer operations are exposed to more
    // scheduling events, and `narrowest_neck` comes out *larger* than the frame
    // that contains it. That is not a slow reading, it is an impossible one, and
    // it is the kind of error that survives into a document because the table
    // looks fine.
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    // Overridable so the refusal itself can be exercised. A guard that cannot be
    // triggered cannot be tested, and an untested guard is one the reader has to
    // take on trust — which is the thing this whole example is about.
    let limit: f64 = std::env::var("MASK_PERF_MAX_LOAD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cores as f64);
    match load_average() {
        Some(load) => {
            println!("машина: средняя нагрузка {load:.1} при {cores} ядрах, порог {limit:.1}");
            if load > limit {
                println!(
                    "\nНагрузка выше порога. Таблица ниже была бы недостоверной —\n\
                     доля бисекции может превысить 100%. Закройте тяжёлое и повторите.\n\
                     Если нужно понять форму кривой, а не миллисекунды, смотрите\n\
                     колонку ×100: она от нагрузки не зависит."
                );
                return;
            }
        }
        None => println!(
            "машина: {cores} ядер, средняя нагрузка недоступна — проверка пропущена,\n\
             таблица может оказаться недостоверной. Если сомневаетесь, перемерьте\n\
             вручную: доля бисекции не должна превышать 100%."
        ),
    }

    let exe = std::env::current_exe().expect("this executable has a path");
    let column = |what: What| -> Vec<f64> {
        let name = match what {
            What::Frame => "frame",
            What::Release => "release",
            What::Neck => "neck",
        };
        let out = Command::new(&exe)
            .arg(name)
            .output()
            .unwrap_or_else(|e| panic!("{name} could not be measured: {e}"));
        assert!(
            out.status.success(),
            "{name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| {
                l.split('\t')
                    .nth(1)
                    .expect("a child prints count and value")
                    .parse()
                    .expect("the child prints a number")
            })
            .collect()
    };

    let frames = column(What::Frame);
    let releases = column(What::Release);
    let necks = column(What::Neck);
    debug_assert_eq!(frames.len(), COUNTS.len());

    println!(
        "{:>7} | {:>9} | {:>9} | {:>9} | {:>6} | {:>6} | {:>7}",
        "regions", "frame", "release", "neck", "share", "×100", "16 ms?"
    );
    let baseline = frames[0];
    for (i, count) in COUNTS.iter().enumerate() {
        let (frame, release, neck) = (frames[i], releases[i], necks[i]);
        let share = if frame > 0.0 {
            neck / frame * 100.0
        } else {
            0.0
        };
        println!(
            "{count:>7} | {frame:>6.2} ms | {release:>6.2} ms | {neck:>6.2} ms | \
             {share:>5.0}% | {:>5.0}× | {:>7}",
            if baseline > 0.0 {
                frame / baseline
            } else {
                0.0
            },
            if frame <= FRAME_BUDGET_MS {
                "да"
            } else {
                "НЕТ"
            }
        );
    }
    println!(
        "\nЧему здесь можно верить, а чему нет.\n\
         ×100 — надёжно: форма кривой одинакова в каждом прогоне, и это то, что\n\
         отвечает на вопрос «сколько регионов влезает в кадр».\n\
         Доля бисекции — примерно: держится около половины, но гуляет на десятки\n\
         процентов от прогона к прогону.\n\
         Миллисекунды — не цифра, а иллюстрация. Проверка нагрузки выше\n\
         необходима, но недостаточна: она ловит занятую машину целиком и не видит\n\
         всплеска в момент замера. При средней нагрузке 3.7 из 10 прогон дал кадр\n\
         ДОЛЬШЕ release — а это невозможно, кадр делает всю работу release и\n\
         сверху. Не переносите эти числа в документ: перемерьте на машине, которая\n\
         ничего больше не делает, и сверьте порядок — release не должен быть\n\
         быстрее кадра."
    );
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        None => run_table(),
        Some("frame") => run_child(What::Frame),
        Some("release") => run_child(What::Release),
        Some("neck") => run_child(What::Neck),
        Some(_other) => {
            eprintln!("usage: editor_frame_cost [frame|release|neck]");
            std::process::exit(2);
        }
    }
}
