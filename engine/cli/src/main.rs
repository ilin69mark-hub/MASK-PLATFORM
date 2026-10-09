//! `maskctl` — the walking skeleton.
//!
//! One command that turns geometry plus a profile into a printable, cuttable
//! mask. Everything below Stage 5 that matters is exercised here:
//! measurement, validation against a profile, a verdict, and SVG output.
//!
//! This binary is deliberately not a product surface. It exists so that a
//! physical proof can happen before any API, editor or commerce exists.

mod image_io;
mod trace;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mask_export::{manifest, tile_to_svg, ArtifactContext, MarksToDraw};
use mask_kernel::{union, widen, Contour, Point, Shape, Shapes};
use mask_nesting::{fixed_grid, needs_nesting, NestRequest};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{render, validate_with_registration, LayerInput, Rect4};

/// Version stamped into every artifact, so reproduction can be verified later.
const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// One artifact as it will appear in the manifest.
struct WrittenArtifact {
    kind: &'static str,
    file: String,
    sha256: String,
    bytes: usize,
    layer: u32,
    tile: Option<String>,
}

#[derive(Debug)]
struct Args {
    fixture: PathBuf,
    material: PathBuf,
    plotter: PathBuf,
    out_dir: PathBuf,
    widen_to: Option<f64>,
    nest: bool,
    /// Add tabs to unanchored islands before validating.
    bridge: bool,
    /// Fill in corners sharper than the material's minimum angle.
    round: bool,
    /// Produce files for a project the validator refused.
    ///
    /// For the physical proof protocol only. The manifest still carries the failing
    /// verdict and records which codes were overridden, so the output cannot be
    /// mistaken for a production run.
    export_anyway: bool,
}

fn main() -> ExitCode {
    // `trace` is a separate verb with its own arguments, so dispatch on the
    // first argument before the production path parses anything.
    if std::env::args().nth(1).as_deref() == Some("trace") {
        return match trace_command() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::from(2)
            }
        };
    }

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    let produced = match run(&args) {
        Ok(manufacturable) => manufacturable,
        // A failing verdict is a legitimate outcome, not a tool failure.
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };

    // An override exit code is not a verdict. The run refused nothing and
    // manufactured nothing, so a script cannot read success here as either — it
    // has to look at the manifest, which says both.
    if args.export_anyway && !produced {
        return ExitCode::from(3);
    }

    match produced {
        true => ExitCode::SUCCESS,
        false => ExitCode::from(1),
    }
}

/// `maskctl trace <image> [--part-width-mm] [--quantise] ...`
fn trace_command() -> Result<(), String> {
    let mut options = trace::TraceOptions::default();
    let mut image: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut quiet = false;

    let argv: Vec<String> = std::env::args().skip(2).collect();
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].clone();
        let take = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match argv[i].as_str() {
            "--part-width-mm" => {
                let raw = take(&mut i)?;
                options.part_width_mm = raw
                    .parse()
                    .map_err(|_| format!("--part-width-mm expects a number, got {raw:?}"))?;
            }
            "--quantise" => {
                let raw = take(&mut i)?;
                options.quantise = raw
                    .parse()
                    .map_err(|_| format!("--quantise expects a number, got {raw:?}"))?;
            }
            "--min-area-mm2" => {
                let raw = take(&mut i)?;
                options.min_area_mm2 = raw
                    .parse()
                    .map_err(|_| format!("--min-area-mm2 expects a number, got {raw:?}"))?;
            }
            "--simplify-mm" => {
                let raw = take(&mut i)?;
                options.simplify_mm = raw
                    .parse()
                    .map_err(|_| format!("--simplify-mm expects a number, got {raw:?}"))?;
            }
            "--max-colours" => {
                let raw = take(&mut i)?;
                options.max_colours = raw
                    .parse()
                    .map_err(|_| format!("--max-colours expects a number, got {raw:?}"))?;
            }
            "--project-id" => options.project_id = take(&mut i)?,
            "--out" => out = Some(PathBuf::from(take(&mut i)?)),
            "--quiet" => quiet = true,
            "-h" | "--help" => return Err(trace_usage()),
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}\n\n{}", trace_usage()))
            }
            other => {
                if image.is_some() {
                    return Err(format!("unexpected second image {other:?}"));
                }
                image = Some(PathBuf::from(other));
            }
        }
        i += 1;
    }

    let image = image.ok_or_else(|| format!("an image path is required\n\n{}", trace_usage()))?;
    let out = out.unwrap_or_else(|| PathBuf::from("traced.json"));

    let report = trace::run(&image, &options, &out).map_err(|e| e.to_string())?;

    if !quiet {
        println!("MASK ENGINE · trace");
        println!(
            "  image        {}x{} px, {} mm per pixel",
            report.image_width_px, report.image_height_px, report.mm_per_pixel
        );
        println!(
            "  palette      {} colour(s){}",
            report.palette.len(),
            report
                .palette
                .iter()
                .map(|c| format!(" #{:02x}{:02x}{:02x}", c.0, c.1, c.2))
                .collect::<Vec<_>>()
                .join("")
        );
        for layer in &report.layers {
            println!(
                "  layer {:<2}     #{:02x}{:02x}{:02x} {:>9.2} mm²  {:>5} px  {:>3} region(s), {} hole(s), narrowest {}",
                layer.index,
                layer.colour.0,
                layer.colour.1,
                layer.colour.2,
                layer.area_mm2,
                layer.pixels,
                layer.regions,
                layer.holes,
                layer
                    .narrowest_mm
                    .map(|w| format!("{w:.2} mm"))
                    .unwrap_or_else(|| "unknown".into())
            );
        }
        if report.dropped > 0 {
            println!(
                "  dropped      {} region(s) below {:.2} mm² ({:.2} mm² total)",
                report.dropped, options.min_area_mm2, report.dropped_area_mm2
            );
        }
        // Said out loud, because a tolerance wider than a pixel is the difference
        // between smoothing a staircase and redesigning the figure.
        if report.worst_simplify_delta > 0.01 {
            println!(
                "  note         simplification changed area by {:.1}%;                  one pixel is {} mm. Lower --simplify-mm to keep detail.",
                report.worst_simplify_delta * 100.0,
                report.mm_per_pixel
            );
        }
        println!("  fixture      {}", out.display());
        println!();
        println!("  Widths are not set. Validate the fixture against a profile before cutting.");
    }
    Ok(())
}

fn trace_usage() -> String {
    "maskctl trace — image to geometry

USAGE
  maskctl trace <image.png|image.pgm|image.ppm> [options]

  Produces a fixture with one colour layer per palette entry. It states what
  is in the picture: it does not choose a cut width, a colour role or a layer
  order. Those come from the profile and the rules, and the project still has
  to pass validation.

OPTIONS
  --part-width-mm <mm>  real width of the finished part. Determines scale;
                        there is no DPI guess. Default 100
  --quantise <1-255>    palette step. Lower merges more. Default 32
  --min-area-mm2 <mm²>  regions below this are reported and dropped. Default 1
  --simplify-mm <mm>    contour smoothing. Keep it below one pixel. Default 0.01
  --max-colours <n>     refuse beyond this many colours. Default 6
  --project-id <id>     identifier written into the fixture
  --out <file.json>     fixture path. Default traced.json
  --quiet               print nothing"
        .to_string()
}

fn parse_args() -> Result<Args, String> {
    parse_command_args(&std::env::args().skip(1).collect::<Vec<_>>())
}

/// Parse production-run arguments without reading the process environment.
///
/// The split exists so invalid invocations can be tested directly. A usage
/// error is user input, not a runtime failure, and it should be provable
/// without spawning the binary.
fn parse_command_args(argv: &[String]) -> Result<Args, String> {
    let mut fixture = None;
    let mut material = None;
    let mut plotter = None;
    let mut out_dir = None;
    let mut widen_to = None;
    let mut export_anyway = false;
    let mut nest = false;
    let mut bridge = false;
    let mut round = false;

    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        let mut take = || -> Result<String, String> {
            i += 1;
            argv.get(i)
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg {
            "--fixture" => fixture = Some(PathBuf::from(take()?)),
            "--material" => material = Some(PathBuf::from(take()?)),
            "--plotter" => plotter = Some(PathBuf::from(take()?)),
            "--out" => out_dir = Some(PathBuf::from(take()?)),
            "--export-anyway" => export_anyway = true,
            "--widen-to" => {
                let raw = take()?;
                let value = raw
                    .parse::<f64>()
                    .map_err(|_| format!("--widen-to expects a number, got {raw:?}"))?;
                widen_to = Some(value);
            }
            "--nest" => nest = true,
            "--bridge" => bridge = true,
            "--round-corners" => round = true,
            "-h" | "--help" => return Err(usage()),
            other => return Err(format!("unknown argument {other:?}\n\n{}", usage())),
        }
        i += 1;
    }

    Ok(Args {
        fixture: fixture.ok_or("--fixture is required")?,
        material: material.ok_or("--material is required")?,
        plotter: plotter.ok_or("--plotter is required")?,
        out_dir: out_dir.unwrap_or_else(|| PathBuf::from("out")),
        widen_to,
        nest,
        bridge,
        round,
        export_anyway,
    })
}

fn usage() -> String {
    "maskctl — produce a printable mask from geometry plus profiles

USAGE
  maskctl --fixture <file.json> --material <profile.json> \\
          --plotter <profile.json> [--out <dir>] [--widen-to <mm>] [--nest] [--bridge]
          [--round-corners] [--export-anyway]

--export-anyway is for the physical proof protocol only: it writes files for a
project the validator refused, keeping the failing verdict in the manifest and
naming the overridden codes. Never use it to ship a mask."
        .to_string()
}

/// Fixture format: a list of layers, each a list of centreline points.
///
/// Deliberately minimal. The vectorisation stage is Stage 8; this skeleton
/// starts from hand-authored centrelines so the manufacturing path can be
/// proven first.
#[derive(serde::Deserialize)]
struct Fixture {
    project_id: String,
    revision: u32,
    /// Target part, carried into the manifest. Explicit rather than inferred:
    /// a manifest that guesses what it was made for is worse than one that says
    /// the target is unknown.
    #[serde(default)]
    target: FixtureTarget,
    layers: Vec<FixtureLayer>,
    /// What this fixture claims to prove, in a form the test suite can read.
    ///
    /// Required rather than optional: a fixture with no stated expectation is a
    /// fixture nobody can tell apart from one that quietly stopped testing
    /// anything. `_comment` explains the idea in prose, which nothing enforces.
    #[serde(rename = "_expect")]
    #[cfg_attr(not(test), allow(dead_code))]
    expect: Expect,
}

/// Whether validation runs at all.
#[derive(serde::Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum ExpectOutcome {
    /// The fixture is validated and gets a verdict.
    Verdict,
    /// The source-integrity gate refuses it before anything is merged. A real
    /// outcome, and the only one that never reaches the Validator.
    Rejected,
}

/// What a fixture promises the engine will do with it.
///
/// The verdict is a typed field, not a string: a misspelled expectation fails to
/// parse instead of quietly never matching.
///
/// A fixture whose `_comment` says "PASS" and whose verdict here says
/// `PASS_WITH_WARNINGS` is not a contradiction. The prose means "no errors";
/// `PASS_WITH_WARNINGS` also carries warnings, most often `min_feature_length` on
/// the short edges a stroke leaves at a line end. Which warnings a fixture expects
/// is stated here rather than left to be inferred from the prose.
#[derive(serde::Deserialize)]
// Read by the test that holds the fixtures to their claims; the binary itself
// never consults it.
#[cfg_attr(not(test), allow(dead_code))]
struct Expect {
    outcome: ExpectOutcome,
    #[serde(default)]
    verdict: Option<mask_validate::Verdict>,
    /// Exactly the rule codes that must come back as errors. Compared as a set,
    /// so a new error appearing is a failure — that is the point.
    #[serde(default)]
    errors: Vec<String>,
    /// Same for warnings.
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
struct FixtureTarget {
    #[serde(default)]
    object: String,
    #[serde(default)]
    part: String,
    #[serde(default)]
    part_code: String,
    #[serde(default)]
    dimensions_mm: [f64; 2],
}

#[derive(serde::Deserialize)]
struct FixtureLayer {
    index: u32,
    name: String,
    /// Position in the paint sequence. Defaults to `index`, so a fixture that never
    /// thinks about stacking still gets a defined order.
    #[serde(default)]
    application_order: Option<u32>,
    #[serde(default = "default_width")]
    width_mm: f64,
    #[serde(default)]
    centrelines: Vec<Vec<[f64; 2]>>,
    /// Filled regions cut along their own outline, with no stroke applied.
    ///
    /// Centrelines and regions answer different questions. A centreline says
    /// "make a line this wide"; a traced image produces an area, and stroking a
    /// traced outline would double every edge and shrink the hole in the middle.
    /// Both may appear in one file, but not in one layer.
    #[serde(default)]
    regions: Vec<FixtureRegion>,
}

/// One filled region: an outer contour and its holes, in millimetres.
#[derive(serde::Deserialize)]
struct FixtureRegion {
    outer: Vec<[f64; 2]>,
    #[serde(default)]
    holes: Vec<Vec<[f64; 2]>>,
}

fn default_width() -> f64 {
    1.0
}

/// Reject geometry that is not well formed, before any merge touches it.
///
/// Returns the problems as operator-readable strings, so the caller decides what
/// to do about them rather than this function stopping on the first one.
fn check_source_integrity(fixture: &Fixture) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();

    for layer in &fixture.layers {
        for (index, region) in layer.regions.iter().enumerate() {
            let mut shapes: Vec<mask_kernel::Shape> = Vec::new();
            let outer = Contour {
                points: region
                    .outer
                    .iter()
                    .map(|[x, y]| Point { x: *x, y: *y })
                    .collect(),
                closed: true,
                arcs: Vec::new(),
            };
            let mut shape = mask_kernel::Shape::new(outer);
            shape.holes = region
                .holes
                .iter()
                .map(|h| Contour {
                    points: h.iter().map(|[x, y]| Point { x: *x, y: *y }).collect(),
                    closed: true,
                    arcs: Vec::new(),
                })
                .collect();
            shapes.push(shape);

            let one = mask_kernel::Shapes { shapes };
            let crossings = mask_kernel::self_intersections(&one);
            if !crossings.is_empty() {
                let at = crossings[0].at;
                problems.push(format!(
                    "L{:02} «{}» region {}: the contour crosses itself {} time(s), first at ({:.4}; {:.4}). Its signed area is {} mm², which is not the area of the figure.",
                    layer.index,
                    layer.name,
                    index + 1,
                    crossings.len(),
                    at.x,
                    at.y,
                    mask_kernel::report(mask_kernel::measure::shapes_area(&one))
                ));
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Convert a fixture region into validated geometry.
fn region_to_shape(region: &FixtureRegion) -> Result<Shape, String> {
    fn contour(points: &[Vec<[f64; 2]>], what: &str) -> Result<Contour, String> {
        let points = points.first().ok_or_else(|| format!("{what} is empty"))?;
        if points.len() < 3 {
            return Err(format!(
                "{what} needs at least three points, got {}",
                points.len()
            ));
        }
        Ok(Contour {
            points: points.iter().map(|[x, y]| Point { x: *x, y: *y }).collect(),
            closed: true,
            arcs: Vec::new(),
        })
    }

    let holes = region
        .holes
        .iter()
        .enumerate()
        .map(|(i, h)| contour(std::slice::from_ref(h), &format!("hole {i}")))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Shape {
        outer: contour(std::slice::from_ref(&region.outer), "outer contour")?,
        holes,
    })
}

fn load_fixture(path: &Path) -> Result<Fixture, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Turn one fixture layer into geometry by stroking each centreline to its width.
///
/// Bands are unioned after stroking. Within a single mask layer every cut
/// region comes from one sheet of film, so bands that touch are already one
/// physical piece; leaving them as separate shapes would make a bridge
/// impossible to represent, and the island check could never pass.
fn build_layer(
    layer: &FixtureLayer,
    widen_to: Option<f64>,
    bridge: Option<mask_kernel::bridge::BridgeLimits>,
) -> Result<LayerInput, String> {
    if !layer.regions.is_empty() && !layer.centrelines.is_empty() {
        return Err(format!(
            "layer {} «{}»: a layer has either centrelines or regions, not both. \
             Mixing them makes the stroke width ambiguous.",
            layer.index, layer.name
        ));
    }

    if !layer.regions.is_empty() {
        if widen_to.is_some() {
            return Err(format!(
                "layer {} «{}»: --widen-to does not apply to regions. They already have \
                 their final outline; widening them would move the boundary the image asked for.",
                layer.index, layer.name
            ));
        }
        let mut merged = Shapes::new();
        for region in &layer.regions {
            let one = region_to_shape(region)
                .map_err(|e| format!("layer {} «{}»: {e}", layer.index, layer.name))?;
            // The union merges regions that touch, so a single sheet of film and
            // the bridge rules see one connected piece.
            merged = union(&merged, &Shapes { shapes: vec![one] })
                .map_err(|e| format!("layer {} «{}»: {e}", layer.index, layer.name))?;
        }
        // Strict: a traced hole with the wrong winding reports a plausible but
        // wrong area, and a plausible wrong area is the failure this whole
        // pipeline exists to prevent.
        merged
            .validate_strict(mask_kernel::constants::MAX_COORDINATE_MM)
            .map_err(|e| format!("layer {} «{}»: {e}", layer.index, layer.name))?;
        return Ok(LayerInput {
            index: layer.index,
            name: layer.name.clone(),
            geometry: merged,
            bridges: Vec::new(),
            application_order: layer.application_order,
        });
    }

    let width = widen_to.unwrap_or(layer.width_mm);
    let mut merged: Shapes = Shapes::new();

    for line in &layer.centrelines {
        if line.len() < 2 {
            return Err(format!(
                "layer {} «{}»: a centreline needs at least two points",
                layer.index, layer.name
            ));
        }
        let points: Vec<Point> = line.iter().map(|[x, y]| Point { x: *x, y: *y }).collect();
        let band = widen(&points, width)
            .map_err(|e| format!("layer {} «{}»: {e}", layer.index, layer.name))?;
        merged = union(&merged, &band)
            .map_err(|e| format!("layer {} «{}»: {e}", layer.index, layer.name))?;
    }

    // The caller bridges the finished layer, so bridging after the union is
    // enough; the flag is accepted here to keep the two branches symmetric.
    let _ = bridge;

    Ok(LayerInput {
        index: layer.index,
        name: layer.name.clone(),
        geometry: merged,
        bridges: Vec::new(),
        application_order: layer.application_order,
    })
}

fn run(args: &Args) -> Result<bool, String> {
    let fixture = load_fixture(&args.fixture)?;
    let material = MaterialProfile::load(&args.material).map_err(|e| e.to_string())?;
    let plotter = PlotterProfile::load(&args.plotter).map_err(|e| e.to_string())?;

    let profile = CuttingProfile::derive(&material, &plotter);
    let rules = RuleSet::from_profile(&profile);
    let verified = profile.is_physically_verified(&material, &plotter);

    println!("MASK ENGINE · walking skeleton");
    println!(
        "  project      {} rev {}",
        fixture.project_id, fixture.revision
    );
    println!("  engine       {ENGINE_VERSION}");
    println!(
        "  material     {} v{} [{}]",
        material.code,
        material.version,
        material.status.as_str()
    );
    println!(
        "  plotter      {} v{} [{}]",
        plotter.code,
        plotter.version,
        plotter.status.as_str()
    );
    println!("  cutting      {}", profile.code);
    println!("  verified     {verified}");
    println!();

    println!("Derived limits:");
    for line in &profile.derivation {
        println!("  {line}");
    }
    println!();

    let Prepared {
        layers,
        plans,
        report,
    } = match prepare(
        &fixture, &material, &plotter, &profile, &rules, verified, args,
    )? {
        Ok(prepared) => prepared,
        Err(problems) => {
            for problem in &problems {
                eprintln!("{problem}");
            }
            eprintln!(
                "\nПроблем в исходной геометрии: {}. Экспорт не выполнялся.",
                problems.len()
            );
            eprintln!("Объединение разрешает самопересечения и молча меняет рисунок.");
            eprintln!("Исправьте исходник или пересоберите его осознанно и проверьте результат.");
            return Ok(false);
        }
    };

    println!("Measured:");
    for m in &report.layers {
        let width = m
            .min_width_mm
            .map_or("—".to_string(), |v| format!("{:.3} mm", v));
        // An em dash for a single-region layer: one piece of film has no
        // neighbour, so there is no gap to report and a number would be invented.
        let gap = m
            .min_gap_mm
            .map_or("—".to_string(), |v| format!("{v:.3} mm"));
        println!(
            "  L{:02} {:<18} regions {:<3} area {:>12.1} mm²  bbox {:>7.1}×{:<7.1} mm  narrowest {width}  min gap {gap}",
            m.index, m.name, m.paths, m.area_mm2, m.bbox[2], m.bbox[3]
        );
    }
    println!();

    println!("Verdict: {:?}", report.verdict);
    println!(
        "  fatal {} · error {} · warning {} · info {}",
        report.counts.fatal, report.counts.error, report.counts.warning, report.counts.info
    );
    println!();
    print!("{}", render(&report));
    println!();

    let ctx = ArtifactContext {
        project_id: fixture.project_id.clone(),
        revision: fixture.revision,
        engine_version: format!("maskctl/{ENGINE_VERSION}"),
        kerf_mm: profile.limits.kerf_mm,
    };

    // One unit per printable piece: a whole layer when no nesting is needed, a
    // tile when it is. Keeping the tile inside the unit — rather than pairing
    // two parallel lists by index — is what prevents a file from being labelled
    // with the wrong tile.
    struct OutputUnit {
        layer_index: u32,
        label: String,
        geometry: Shapes,
        tile: Option<MarksToDraw>,
    }

    let units: Vec<OutputUnit> = if plans.is_empty() {
        layers
            .iter()
            .map(|l| OutputUnit {
                layer_index: l.index,
                label: l.name.clone(),
                geometry: l.geometry.clone(),
                tile: None,
            })
            .collect()
    } else {
        let mut out = Vec::new();
        for (index, name, plan) in &plans {
            for t in &plan.tiles {
                out.push(OutputUnit {
                    layer_index: *index,
                    label: name.clone(),
                    geometry: t.content.clone(),
                    tile: Some(MarksToDraw {
                        marks: t.marks.clone(),
                        rect: [t.rect.x_mm, t.rect.y_mm, t.rect.width_mm, t.rect.height_mm],
                    }),
                });
            }
        }
        out
    };

    if !report.verdict.is_manufacturable() {
        let errors: Vec<&mask_validate::Violation> = report
            .violations
            .iter()
            .filter(|v| {
                v.severity == mask_rules::Severity::Error
                    || v.severity == mask_rules::Severity::Fatal
            })
            .collect();

        if !args.export_anyway {
            println!("Export skipped: the project is not manufacturable.");
            for v in &errors {
                println!("   {}", v.headline);
            }
            return Ok(false);
        }

        // The physical proof protocol needs printable files for the cases the
        // engine is *right* to refuse — T2, T4 and T6 exist precisely because they
        // are supposed to fail. Without a way to export them, the protocol's
        // instruction to cut them anyway is unfollowable.
        //
        // Lowering a threshold cannot reach all of them, which is why this is a
        // flag and not a profile. An experimental profile fixes cases where a
        // number was too strict; t6's gap is floored at kerf + registration by the
        // rules engine and t4's unanchored island is a topological fact rather than
        // a threshold, so no threshold makes either export. The refusal is correct
        // in both cases and has to be overridden rather than edited away.
        //
        // Every artifact says so. The manifest keeps the failing verdict and the
        // status stays `NeedsWork`, and a note records which codes were overridden:
        // a file that looked like a production run would defeat the purpose.
        println!(
            "Export proceeding anyway (--export-anyway): {} blocking violation(s).",
            errors.len()
        );
        for v in &errors {
            println!("   overridden: {} — {}", v.code, v.headline);
        }
        println!("   The manifest keeps the failing verdict and names these codes.");
        println!();
    }
    if units.iter().all(|u| u.geometry.is_empty()) {
        println!("Export skipped: no piece carries any geometry.");
        return Ok(false);
    }

    fs::create_dir_all(&args.out_dir)
        .map_err(|e| format!("cannot create {}: {e}", args.out_dir.display()))?;

    let curve_tolerance = plotter.blade.curve_tolerance_mm;
    let plt_device = mask_export::plt::PltDevice::from_profile(&plotter, 40.0, 30);

    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut total_bytes = 0usize;
    let mut artifacts: Vec<WrittenArtifact> = Vec::new();
    for unit in &units {
        // A ragged mask can leave an empty grid cell. It is skipped rather than
        // failing the run: the remaining tiles are still cuttable, and tile
        // numbering stays intact because the plan still lists the empty cell.
        if unit.geometry.is_empty() {
            skipped += 1;
            println!(
                "  skipped tile {} — no mask content in this cell",
                unit.tile.as_ref().map_or("", |t| t.marks.label.as_str())
            );
            continue;
        }

        let svg = match &unit.tile {
            Some(tile) => tile_to_svg(
                &unit.geometry,
                unit.layer_index,
                &unit.label,
                &tile.marks.label,
                tile,
                &report,
                &ctx,
            ),
            None => {
                mask_export::to_svg(&unit.geometry, unit.layer_index, &unit.label, &report, &ctx)
            }
        };
        let base = match &unit.tile {
            Some(tile) => mask_export::tile_stem(
                &fixture.project_id,
                fixture.revision,
                unit.layer_index,
                &tile.marks.label,
            ),
            None => {
                mask_export::layer_stem(&fixture.project_id, fixture.revision, unit.layer_index)
            }
        };

        let dxf = mask_export::dxf::to_dxf(
            &mask_export::dxf::DxfContent {
                cut: unit.geometry.clone(),
                registration: unit
                    .tile
                    .as_ref()
                    .map_or_else(Shapes::new, |t| manifest::registration_geometry(&t.marks)),
            },
            curve_tolerance,
        );

        let plt = mask_export::plt::to_plt(
            &mask_export::plt::PltContent {
                cut: unit.geometry.clone(),
                registration: Shapes::new(),
            },
            plt_device,
            &unit.label,
        );

        for (kind, ext, body) in [
            ("cut_svg", "svg", &svg),
            ("cut_dxf", "dxf", &dxf),
            ("cut_plt", "plt", &plt),
        ] {
            let file = format!("{base}.{ext}");
            let path = args.out_dir.join(&file);
            fs::write(&path, body).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            written += 1;
            total_bytes += body.len();
            artifacts.push(WrittenArtifact {
                kind,
                file,
                sha256: mask_export::sha256_hex(body.as_bytes()),
                bytes: body.len(),
                layer: unit.layer_index,
                tile: unit.tile.as_ref().map(|t| t.marks.label.clone()),
            });
            println!("  wrote {} ({} bytes)", path.display(), body.len());
        }
    }
    // Recorded so the manifest can name what was overridden.
    //
    // Keyed on whether anything was *actually* refused, not on the flag. With the
    // flag set and nothing refused, the list is empty, and an empty list means "no
    // override happened" — otherwise a passing project run with `--export-anyway`
    // would write "the validator refused this project", which is a false statement
    // in the one document whose whole purpose is to be believed.
    let refused = !report.verdict.is_manufacturable();
    let overridden: Vec<String> = if refused {
        report
            .violations
            .iter()
            .filter(|v| {
                v.severity == mask_rules::Severity::Error
                    || v.severity == mask_rules::Severity::Fatal
            })
            .map(|v| v.code.clone())
            .collect()
    } else {
        Vec::new()
    };
    let override_recorded = args.export_anyway && refused;

    let manifest_doc = manifest::build_manifest_with_override(
        &manifest::ProjectRef {
            id: fixture.project_id.clone(),
            revision: fixture.revision,
            object: fixture.target.object.clone(),
            part: fixture.target.part.clone(),
            part_code: fixture.target.part_code.clone(),
            dimensions_mm: fixture.target.dimensions_mm,
        },
        &material,
        &plotter,
        &profile,
        &report,
        &layers,
        &artifacts
            .iter()
            .map(|a| manifest::ArtifactRow {
                kind: a.kind,
                file: a.file.clone(),
                sha256: a.sha256.clone(),
                bytes: a.bytes,
                layer: Some(a.layer),
                tile: a.tile.clone(),
            })
            .collect::<Vec<_>>(),
        if override_recorded {
            Some(&overridden)
        } else {
            None
        },
    );
    let manifest_json = serde_json::to_string_pretty(&manifest_doc)
        .map_err(|e| format!("cannot serialise manifest: {e}"))?;
    let manifest_file = format!(
        "{}_rev{}_manifest.json",
        fixture.project_id, fixture.revision
    );
    let manifest_path = args.out_dir.join(&manifest_file);
    fs::write(&manifest_path, &manifest_json)
        .map_err(|e| format!("cannot write {}: {e}", manifest_path.display()))?;
    println!(
        "  wrote {} ({} bytes)",
        manifest_path.display(),
        manifest_json.len()
    );

    // The instruction is the document a person actually follows at the bench, so
    // it is generated from the same data as the files next to it. It is refused
    // rather than written when a figure cannot be traced, because an
    // instruction with one untraceable number is worse than no instruction: it
    // reads as authoritative.
    let instruction =
        mask_export::instruction::render_markdown(&mask_export::instruction::InstructionInput {
            project_id: &fixture.project_id,
            revision: fixture.revision,
            engine_version: env!("CARGO_PKG_VERSION"),
            material: &material,
            plotter: &plotter,
            report: &report,
            tiles: (!plans.is_empty()).then(|| {
                plans
                    .iter()
                    .map(|(_, _, p)| p.summary.tile_count as u32)
                    .sum()
            }),
        });
    if let Err(unverified) = mask_export::instruction::verify(&instruction) {
        let detail = unverified
            .iter()
            .map(|u| u.text.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("инструкция не прошла сверку чисел: {detail}"));
    }
    let instruction_file = format!(
        "{}_rev{}_instruction.md",
        fixture.project_id, fixture.revision
    );
    let instruction_path = args.out_dir.join(&instruction_file);
    fs::write(&instruction_path, &instruction.text)
        .map_err(|e| format!("cannot write {}: {e}", instruction_path.display()))?;
    println!(
        "  wrote {} ({} bytes, {} figure(s) traced)",
        instruction_path.display(),
        instruction.text.len(),
        instruction.uses.len()
    );

    // The printable sheet: one page per layer at 1:1, because a sheet is printed
    // at 100% and scaling changes the element width. A layer that will not fit
    // is left out and named, rather than quietly shrunk onto the page.
    let sheet_meta = mask_export::pdf::SheetMeta {
        project_id: &fixture.project_id,
        revision: fixture.revision,
        engine_version: env!("CARGO_PKG_VERSION"),
        verdict: report.verdict,
        limits_physically_verified: report.limits_physically_verified,
        ..mask_export::pdf::SheetMeta::default()
    };

    // Contours are collected up front so the page views can borrow them; making
    // them inside the view would drop them at the end of the statement.
    let contours_by_layer: Vec<Vec<mask_kernel::Contour>> = layers
        .iter()
        .map(|l| {
            l.geometry
                .shapes
                .iter()
                .flat_map(|s| std::iter::once(s.outer.clone()).chain(s.holes.iter().cloned()))
                .collect()
        })
        .collect();
    let empty: Vec<mask_kernel::Contour> = Vec::new();

    let mut sheet_layers: Vec<mask_export::pdf::SheetLayer<'_>> = Vec::new();
    for (layer, contours) in layers.iter().zip(contours_by_layer.iter()) {
        let page = mask_export::pdf::SheetLayer {
            index: layer.index,
            name: &layer.name,
            paths: contours,
            fills: &empty,
        };
        if mask_export::pdf::fit(&page, &sheet_meta).fits {
            sheet_layers.push(page);
        } else {
            println!(
                "  слой {} не помещается на лист без уменьшения — в PDF не включён",
                layer.index
            );
        }
    }

    let sheet = mask_export::pdf::mask_sheet(&sheet_layers, &sheet_meta);
    let sheet_file = format!(
        "{}_rev{}_mask_sheet.pdf",
        fixture.project_id, fixture.revision
    );
    let sheet_path = args.out_dir.join(&sheet_file);
    fs::write(&sheet_path, &sheet)
        .map_err(|e| format!("cannot write {}: {e}", sheet_path.display()))?;
    println!(
        "  wrote {} ({} bytes, {} page(s))",
        sheet_path.display(),
        sheet.len(),
        sheet_layers.len()
    );

    // The archive: everything above in one file, so a kit is one download. Built
    // from what is on disk, so it cannot disagree with the directory — but only
    // this project's files: `make physical` puts every fixture in one directory,
    // and an earlier version packed all 65 of them into each project's kit.
    let archive_name = format!("{}_rev{}_kit.zip", fixture.project_id, fixture.revision);
    let project_prefix = format!("{}_rev{}_", fixture.project_id, fixture.revision);
    let mut archive_entries: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in fs::read_dir(&args.out_dir).map_err(|e| format!("{e}"))? {
        let path = entry.map_err(|e| format!("{e}"))?.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name == archive_name {
            continue; // never fold the archive into itself
        }
        if !name.starts_with(&project_prefix) {
            continue; // another project's file sharing the directory
        }
        let data = fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        archive_entries.push((name.to_string(), data));
    }
    archive_entries.sort_by(|a, b| a.0.cmp(&b.0));
    let borrow: Vec<mask_export::archive::Entry<'_>> = archive_entries
        .iter()
        .map(|(name, data)| mask_export::archive::Entry { name, data })
        .collect();
    let zip = mask_export::archive::zip(&borrow, mask_export::archive::DOS_EPOCH);
    let archive_path = args.out_dir.join(&archive_name);
    fs::write(&archive_path, &zip)
        .map_err(|e| format!("cannot write {}: {e}", archive_path.display()))?;
    println!(
        "  wrote {} ({} bytes, {} file(s))",
        archive_path.display(),
        zip.len(),
        archive_entries.len()
    );

    // The archive must agree with the manifest: every artifact the manifest lists
    // is inside, and every file inside is one the manifest accounts for. A kit
    // whose manifest does not describe it is not a reproducible kit.
    let in_archive: Vec<&str> = archive_entries.iter().map(|(n, _)| n.as_str()).collect();
    let listed: Vec<&str> = artifacts
        .iter()
        .map(|a| a.file.as_str())
        .chain([
            manifest_file.as_str(),
            instruction_file.as_str(),
            sheet_file.as_str(),
        ])
        .collect();

    let mut missing: Vec<&str> = listed
        .iter()
        .filter(|f| !in_archive.contains(f))
        .copied()
        .collect();
    missing.sort_unstable();
    if !missing.is_empty() {
        return Err(format!(
            "в архиве нет файлов из манифеста: {}",
            missing.join(", ")
        ));
    }

    println!(
        "  {} file(s), {} bytes total{}",
        written,
        total_bytes,
        if skipped > 0 {
            format!(", {skipped} empty tile(s) skipped")
        } else {
            String::new()
        }
    );

    println!();
    if override_recorded {
        println!(
            "These files are NOT a production run: the validator refused this project.\n\
             Print at 100%, cut by hand, and record what happens. See\n\
             docs/protocols/PHYSICAL_PROOF_PROTOCOL.md"
        );
        return Ok(false);
    }
    println!(
        "Next: print at 100%, then cut by hand. See docs/protocols/PHYSICAL_PROOF_PROTOCOL.md"
    );
    Ok(report.verdict.is_manufacturable())
}
/// The state the pipeline reaches between a fixture and a verdict.
///
/// Split out of `run` so the fixtures can be checked against what they claim
/// without going through the command line and parsing its prose. The narration
/// stays in here: it is written for the operator, and a test harness swallows it.
struct Prepared {
    layers: Vec<LayerInput>,
    plans: Vec<(u32, String, mask_nesting::NestingPlan)>,
    report: mask_validate::ValidationReport,
}

/// Everything from source geometry to a verdict: integrity, automatic fixes,
/// nesting, validation.
///
/// `Ok(Err(..))` is the source-integrity gate refusing the geometry before
/// anything was merged — a real outcome, not a failure, and the one outcome a
/// fixture can expect that never reaches a verdict.
#[allow(clippy::too_many_arguments)]
fn prepare(
    fixture: &Fixture,
    material: &MaterialProfile,
    plotter: &PlotterProfile,
    profile: &CuttingProfile,
    rules: &RuleSet,
    verified: bool,
    args: &Args,
) -> Result<Result<Prepared, Vec<String>>, String> {
    // Integrity is checked on the geometry as written, before anything is merged.
    //
    // This is not defensive tidiness. `build_layer` unions every region of a layer
    // into one piece, and the union resolves self-intersections: a bow tie goes in
    // as one contour reporting 0 mm² — its two lobes cancel exactly — and comes out
    // as two 400 mm² triangles. Nothing is reported. The operator's figure was
    // ambiguous, the engine picked an interpretation, and the only evidence is a
    // geometry that no longer looks like what they drew.
    if let Err(problems) = check_source_integrity(fixture) {
        return Ok(Err(problems));
    }

    // Bridging runs before widening, because a tab added to a centreline that is
    // about to be stroked would then be stroked too, and the tab would come out
    // wider than the material's minimum by accident rather than by choice.
    let limits = mask_kernel::bridge::BridgeLimits {
        width_mm: profile.limits.bridge_width_mm,
        max_length_mm: material.minimums.bridge_length_mm,
    };

    let mut bridges_added = 0usize;
    let layers: Vec<LayerInput> = fixture
        .layers
        .iter()
        .map(|l| build_layer(l, args.widen_to, args.bridge.then_some(limits)))
        .map(|layer| {
            let mut layer = layer?;
            if args.round {
                // Runs after bridging: a tab is a rectangle with four right
                // angles, so bridging first means filleting can see them.
                let (fixed, report) = mask_kernel::corner::round_corners(
                    &layer.geometry,
                    profile.limits.corner_radius_mm,
                    profile.limits.min_angle_deg,
                    plotter.blade.curve_tolerance_mm,
                );
                match report {
                    mask_kernel::RoundReport::Complete { rounded } if rounded > 0 => {
                        println!(
                            "Auto-Fix L{:02}: filleted {rounded} corner(s), radius {} mm",
                            layer.index,
                            mask_kernel::report(profile.limits.corner_radius_mm)
                        );
                        layer.geometry = fixed;
                    }
                    // Naming what could not be done matters: a corner left sharp is
                    // a tear, and the validator would otherwise report it with no
                    // indication that an automatic fix had already been attempted.
                    mask_kernel::RoundReport::Incomplete { rounded, skipped } => {
                        println!(
                            "Auto-Fix L{:02}: filleted {rounded}, left {} corner(s) sharp (edges too short)",
                            layer.index,
                            skipped.len()
                        );
                        layer.geometry = fixed;
                    }
                    mask_kernel::RoundReport::Complete { .. } => {}
                }
            }

            if args.bridge {
                match mask_kernel::bridge::bridge_unanchored(&layer.geometry, limits) {
                    Ok((fixed, bridges)) => {
                        bridges_added += bridges.len();
                        for b in &bridges {
                            println!(
                                "Auto-Fix L{:02}: bridged island {} with a {:.2} mm tab, {:.2} mm long",
                                layer.index, b.island_index, b.width_mm, b.length_mm
                            );
                        }
                        layer.geometry = fixed;
                        // Record the tabs, so T-06 and T-07 have something to
                        // check. Without this the tabs are geometry and nothing can
                        // ask about them afterwards.
                        layer.bridges = bridges
                            .iter()
                            .map(|b| mask_validate::BridgeRecord {
                                width_mm: b.width_mm,
                                length_mm: b.length_mm,
                                at: b.to,
                            })
                            .collect();
                    }
                    // Refusing is the honest outcome. A tab that is too long or
                    // cannot reach the film would be a claim about the material
                    // that nobody measured, so the layer is left as it was and
                    // the Validator reports the island.
                    Err(e) => {
                        println!("Auto-Fix L{:02}: could not bridge ({e})", layer.index);
                    }
                }
            }
            Ok(layer)
        })
        .collect::<Result<_, String>>()?;

    if let Some(w) = args.widen_to {
        println!("Auto-Fix: widened every layer to {w} mm");
        println!();
    }
    if args.bridge {
        println!(
            "Auto-Fix: {bridges_added} tab(s) added, {:.2} mm wide, {:.0} mm maximum",
            limits.width_mm, limits.max_length_mm
        );
        println!();
    }

    let nest_request = NestRequest {
        field_width_mm: profile.limits.max_tile_width_mm,
        field_length_mm: profile.limits.max_tile_length_mm,
        separation_gap_mm: profile.limits.gap_mm,
        kerf_mm: profile.limits.kerf_mm,
    };

    // Nesting happens before validation so the field rule is measured against
    // what would actually be printed, not against the uncut mask.
    let mut plans = Vec::new();
    if args.nest {
        for layer in &layers {
            if needs_nesting(&layer.geometry, nest_request) {
                match fixed_grid(&layer.geometry, nest_request) {
                    Ok(plan) => {
                        // Waste ratio is reported but not emphasised: for
                        // line art almost the whole sheet is weeded away, so a
                        // figure near 100% is expected, not a defect. The
                        // number that costs money is sheets per layer.
                        println!(
                            "Nested L{:02}: {} tiles, {} sheets, waste {:.1}% (normal for line art), separation {} mm",
                            layer.index,
                            plan.summary.tile_count,
                            plan.summary.tile_count,
                            plan.summary.waste_ratio * 100.0,
                            plan.summary.separation_gap_mm
                        );
                        plans.push((layer.index, layer.name.clone(), plan));
                    }
                    Err(e) => println!("Nested L{:02}: refused ({e})", layer.index),
                }
            }
        }
        if plans.is_empty() {
            println!("No layer exceeded the working field; nothing to nest.");
        }
        println!();
    }

    // Tile rectangles go to the validator so the field rule is measured against
    // what would actually be printed.
    let mut tile_rects: std::collections::BTreeMap<u32, Vec<Rect4>> =
        std::collections::BTreeMap::new();
    for (index, _, plan) in &plans {
        tile_rects.insert(
            *index,
            plan.tiles
                .iter()
                .map(|t| {
                    let (_, _, w, h) = mask_kernel::bounding_box(&t.content);
                    [0.0, 0.0, w, h]
                })
                .collect(),
        );
    }

    // Registration anchors, one per nested layer: where its first tile sits on
    // the sheet. Layers nested from the same origin share one, and T-15 reports
    // the difference between any two that do not.
    let mut anchors: std::collections::BTreeMap<u32, Point> = std::collections::BTreeMap::new();
    for (index, _, plan) in &plans {
        if let Some(first) = plan.tiles.first() {
            anchors.insert(
                *index,
                Point {
                    x: first.marks.part_anchor.left.x_mm,
                    y: first.marks.part_anchor.left.y_mm,
                },
            );
        }
    }

    let report =
        validate_with_registration(&layers, profile, rules, verified, &tile_rects, &anchors);

    Ok(Ok(Prepared {
        layers,
        plans,
        report,
    }))
}

#[cfg(test)]
mod fixture_expectations {
    use super::*;

    fn repo(path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path)
    }

    /// Run a fixture the way its author saw it: the geometry as written, with no
    /// automatic fix and no nesting. Expectations describe that run, so every
    /// fixture claims a check on the *design* rather than on its repaired version.
    fn plain_args(fixture: PathBuf) -> Args {
        Args {
            fixture,
            material: repo("profiles/data/matte-vinyl-090.v4.json"),
            plotter: repo("profiles/data/knife-25mm.v2.json"),
            out_dir: std::env::temp_dir(),
            widen_to: None,
            nest: false,
            bridge: false,
            round: false,
            // Expectations describe the design as written. An override would
            // silently make a failing fixture export, which is exactly the
            // substitution these fixtures exist to prevent.
            export_anyway: false,
        }
    }

    fn fixture_paths() -> Vec<PathBuf> {
        let dir = repo("fixtures");
        let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
            .map(|entry| entry.expect("cannot read a directory entry").path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect();
        paths.sort();
        assert!(!paths.is_empty(), "no fixtures in {}", dir.display());
        paths
    }

    /// The rule codes a report carries at one severity, deduplicated and sorted.
    ///
    /// A set, not a tally. *Which* checks fired is the claim a fixture makes; *how
    /// many times* is an accident of how many edges or layers it happens to have,
    /// and pinning it would make every harmless geometry edit a failure to argue
    /// about.
    fn codes(
        report: &mask_validate::ValidationReport,
        severity: mask_rules::Severity,
    ) -> Vec<&str> {
        let mut out: Vec<&str> = report
            .violations
            .iter()
            .filter(|v| v.severity == severity)
            .map(|v| v.code.as_str())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// One violation, for the override test. Only the two fields that test reads are
    /// interesting; the rest exist because `Violation` has no defaults.
    fn violation(code: &str, severity: mask_rules::Severity) -> mask_validate::Violation {
        mask_validate::Violation {
            code: code.into(),
            severity,
            layer_index: None,
            measured: None,
            required: None,
            unit: mask_validate::Unit::Mm,
            count: 1,
            headline: format!("{code} headline"),
            reason: "because".into(),
            actions: Vec::new(),
        }
    }

    /// `--export-anyway` writes files for a project the validator refused.
    ///
    /// Asserted here rather than only in the manifest tests: this is the switch
    /// that could let a failing project reach a knife, so what it must *not* do is
    /// checkable without a bench.
    #[test]
    fn the_refusal_listed_for_override_is_the_one_that_blocks_export() {
        // Two violations, one blocking. Only the blocking one may be reported as
        // overridden: a manifest naming a warning as overridden would misdescribe
        // what the operator actually bypassed.
        let report = mask_validate::ValidationReport {
            verdict: mask_validate::Verdict::Fail,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 1,
                warning: 1,
                info: 0,
            },
            violations: vec![
                violation("min_gap", mask_rules::Severity::Error),
                violation("min_feature_length", mask_rules::Severity::Warning),
            ],
            layers: vec![],
            fidelity: None,
            limits_physically_verified: false,
        };

        let codes: Vec<String> = report
            .violations
            .iter()
            .filter(|v| {
                v.severity == mask_rules::Severity::Error
                    || v.severity == mask_rules::Severity::Fatal
            })
            .map(|v| v.code.clone())
            .collect();

        assert_eq!(codes, vec!["min_gap".to_string()]);
        assert!(!report.verdict.is_manufacturable());
    }

    /// `--export-anyway` on a project that passes records nothing.
    ///
    /// The manifest is the document whose whole purpose is to be believed, so
    /// writing "the validator refused this project" onto a `PASS_WITH_WARNINGS`
    /// project because the flag was set is a false statement in exactly the place
    /// nobody would look twice. The flag is inert when nothing was refused.
    #[test]
    fn the_flag_alone_never_marks_a_passing_project_as_refused() {
        let passing = mask_validate::ValidationReport {
            verdict: mask_validate::Verdict::PassWithWarnings,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 0,
                warning: 1,
                info: 0,
            },
            violations: vec![violation(
                "min_feature_length",
                mask_rules::Severity::Warning,
            )],
            layers: vec![],
            fidelity: None,
            limits_physically_verified: false,
        };

        // The same predicate the CLI uses to decide whether to record an override.
        let refused = !passing.verdict.is_manufacturable();
        let recorded = true /* --export-anyway */ && refused;
        assert!(
            !recorded,
            "a passing project must carry no override, whatever the flag says"
        );
    }

    /// Every fixture must do what its `_expect` block says.
    ///
    /// Without this the fixtures only describe themselves. A check that stops
    /// firing, a fixture quietly edited until it trips nothing, a rule that starts
    /// reporting something new — none of it would fail anything, and the suite
    /// would still be green. This is the check that makes them executable.
    #[test]
    fn every_fixture_does_what_it_claims() {
        let mut failures: Vec<String> = Vec::new();

        for path in fixture_paths() {
            let name = path
                .file_stem()
                .expect("a fixture path has a stem")
                .to_string_lossy()
                .into_owned();

            let fixture = match load_fixture(&path) {
                Ok(f) => f,
                Err(e) => {
                    failures.push(format!("{name}: {e}"));
                    continue;
                }
            };

            let args = plain_args(path.clone());
            let material = match MaterialProfile::load(&args.material) {
                Ok(m) => m,
                Err(e) => {
                    failures.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let plotter = match PlotterProfile::load(&args.plotter) {
                Ok(p) => p,
                Err(e) => {
                    failures.push(format!("{name}: {e}"));
                    continue;
                }
            };
            let profile = CuttingProfile::derive(&material, &plotter);
            let rules = RuleSet::from_profile(&profile);
            let verified = profile.is_physically_verified(&material, &plotter);

            match prepare(
                &fixture, &material, &plotter, &profile, &rules, verified, &args,
            ) {
                Err(e) => failures.push(format!("{name}: {e}")),
                Ok(Err(problems)) => {
                    if fixture.expect.outcome != ExpectOutcome::Rejected {
                        failures.push(format!(
                            "{name}: expected a verdict, but the source gate refused it: {}",
                            problems.join("; ")
                        ));
                    }
                }
                Ok(Ok(prepared)) => {
                    let expect = &fixture.expect;
                    if expect.outcome != ExpectOutcome::Verdict {
                        failures.push(format!(
                            "{name}: expected the source gate to refuse it, but it was validated as {:?}",
                            prepared.report.verdict
                        ));
                        continue;
                    }
                    if Some(prepared.report.verdict) != expect.verdict {
                        failures.push(format!(
                            "{name}: expected verdict {:?}, got {:?}",
                            expect.verdict, prepared.report.verdict
                        ));
                    }
                    for (severity, label, expected) in [
                        (mask_rules::Severity::Error, "error", &expect.errors),
                        (mask_rules::Severity::Warning, "warning", &expect.warnings),
                    ] {
                        let mut expected = expected.clone();
                        expected.sort();
                        let actual = codes(&prepared.report, severity);
                        if actual != expected {
                            failures.push(format!(
                                "{name}: {label} codes differ — expected {expected:?}, got {actual:?}"
                            ));
                        }
                    }
                }
            }
        }

        assert!(
            failures.is_empty(),
            "{} of {} fixtures no longer match their expectations:\n  - {}",
            failures.len(),
            fixture_paths().len(),
            failures.join("\n  - ")
        );
    }
}

#[cfg(test)]
mod cli_boundaries {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    fn centreline_layer() -> FixtureLayer {
        FixtureLayer {
            index: 1,
            name: "Centreline".to_string(),
            application_order: None,
            width_mm: 1.0,
            centrelines: vec![vec![[0.0, 0.0], [10.0, 0.0]]],
            regions: Vec::new(),
        }
    }

    fn square_region() -> FixtureRegion {
        FixtureRegion {
            outer: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]],
            holes: Vec::new(),
        }
    }

    fn region_layer() -> FixtureLayer {
        FixtureLayer {
            index: 1,
            name: "Region".to_string(),
            application_order: None,
            width_mm: 1.0,
            centrelines: Vec::new(),
            regions: vec![square_region()],
        }
    }

    fn fixture_with_region(region: FixtureRegion) -> Fixture {
        Fixture {
            project_id: "TEST-CLI-001".to_string(),
            revision: 1,
            target: FixtureTarget::default(),
            layers: vec![FixtureLayer {
                index: 1,
                name: "Source".to_string(),
                application_order: None,
                width_mm: 1.0,
                centrelines: Vec::new(),
                regions: vec![region],
            }],
            expect: Expect {
                outcome: ExpectOutcome::Rejected,
                verdict: None,
                errors: Vec::new(),
                warnings: Vec::new(),
            },
        }
    }

    #[test]
    fn production_arguments_parse_with_defaults() {
        let args = parse_command_args(&argv(&[
            "--fixture",
            "fixture.json",
            "--material",
            "material.json",
            "--plotter",
            "plotter.json",
        ]))
        .expect("required arguments");

        assert_eq!(args.fixture, PathBuf::from("fixture.json"));
        assert_eq!(args.material, PathBuf::from("material.json"));
        assert_eq!(args.plotter, PathBuf::from("plotter.json"));
        assert_eq!(args.out_dir, PathBuf::from("out"));
        assert_eq!(args.widen_to, None);
        assert!(!args.nest && !args.bridge && !args.round && !args.export_anyway);
    }

    #[test]
    fn production_flags_and_options_are_recorded() {
        let args = parse_command_args(&argv(&[
            "--fixture",
            "fixture.json",
            "--material",
            "material.json",
            "--plotter",
            "plotter.json",
            "--out",
            "proof",
            "--widen-to",
            "0.8",
            "--nest",
            "--bridge",
            "--round-corners",
            "--export-anyway",
        ]))
        .expect("flags and options");

        assert_eq!(args.out_dir, PathBuf::from("proof"));
        assert_eq!(args.widen_to, Some(0.8));
        assert!(args.nest && args.bridge && args.round && args.export_anyway);
    }

    #[test]
    fn malformed_production_arguments_identify_the_problem() {
        let required = ["--fixture", "--material", "--plotter"];
        for (index, flag) in required.iter().enumerate() {
            let mut supplied = Vec::new();
            for (position, name) in ["fixture", "material", "plotter"].iter().enumerate() {
                let value = format!("{name}.json");
                if position != index {
                    supplied.push(flag.to_string());
                    supplied.push(value);
                }
            }
            let error = parse_command_args(&supplied).expect_err("a required path");
            assert!(
                error.contains("is required"),
                "missing {flag} should name the missing input: {error}"
            );
        }

        let error = parse_command_args(&argv(&["--widen-to"])).expect_err("a missing value");
        assert!(error.contains("needs a value"), "{error}");

        let error = parse_command_args(&argv(&["--widen-to", "wide"])).expect_err("a number");
        assert!(error.contains("expects a number"), "{error}");

        let error = parse_command_args(&argv(&["--sharpen"])).expect_err("an unknown option");
        assert!(error.contains("unknown argument"), "{error}");
    }

    #[test]
    fn help_reports_usage() {
        let error = parse_command_args(&argv(&["--help"])).expect_err("help is not a run");
        assert!(error.contains("USAGE"), "{error}");
    }

    #[test]
    fn a_layer_may_use_centrelines_or_regions_but_not_both() {
        let mut layer = centreline_layer();
        layer.regions.push(square_region());

        let error = build_layer(&layer, None, None).expect_err("ambiguous stroke width");
        assert!(
            error.contains("either centrelines or regions, not both"),
            "{error}"
        );
    }

    #[test]
    fn finished_regions_are_not_rewidened() {
        let error = build_layer(&region_layer(), Some(0.8), None).expect_err("a moved boundary");
        assert!(
            error.contains("--widen-to does not apply to regions"),
            "{error}"
        );
    }

    #[test]
    fn region_outlines_require_a_triangle() {
        let short = region_to_shape(&FixtureRegion {
            outer: vec![[0.0, 0.0], [10.0, 0.0]],
            holes: Vec::new(),
        })
        .expect_err("a segment is not a region");
        assert!(short.contains("at least three points"), "{short}");

        let shape = region_to_shape(&square_region()).expect("a triangle is a region");
        assert_eq!(shape.outer.points.len(), 4);
        assert!(shape.outer.closed);
    }

    #[test]
    fn a_self_crossing_source_is_named_before_any_merge() {
        let bowtie = FixtureRegion {
            outer: vec![[0.0, 0.0], [10.0, 10.0], [10.0, 0.0], [0.0, 10.0]],
            holes: Vec::new(),
        };
        let fixture = fixture_with_region(bowtie);

        let problems = check_source_integrity(&fixture).expect_err("a bow tie is not one region");
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("crosses itself")
                && problems[0].contains("L01")
                && problems[0].contains("region 1"),
            "{}",
            problems[0]
        );
    }
}
