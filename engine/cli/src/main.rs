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

    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        // A failing verdict is a legitimate outcome, not a tool failure.
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
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
    let mut fixture = None;
    let mut material = None;
    let mut plotter = None;
    let mut out_dir = None;
    let mut widen_to = None;
    let mut nest = false;
    let mut bridge = false;
    let mut round = false;

    let argv: Vec<String> = std::env::args().skip(1).collect();
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
    })
}

fn usage() -> String {
    "maskctl — produce a printable mask from geometry plus profiles

USAGE
  maskctl --fixture <file.json> --material <profile.json> \\
          --plotter <profile.json> [--out <dir>] [--widen-to <mm>] [--nest] [--bridge] [--round-corners]"
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
            };
            let mut shape = mask_kernel::Shape::new(outer);
            shape.holes = region
                .holes
                .iter()
                .map(|h| Contour {
                    points: h.iter().map(|[x, y]| Point { x: *x, y: *y }).collect(),
                    closed: true,
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

    // Integrity is checked on the geometry as written, before anything is merged.
    //
    // This is not defensive tidiness. `build_layer` unions every region of a layer
    // into one piece, and the union resolves self-intersections: a bow tie goes in
    // as one contour reporting 0 mm² — its two lobes cancel exactly — and comes out
    // as two 400 mm² triangles. Nothing is reported. The operator's figure was
    // ambiguous, the engine picked an interpretation, and the only evidence is a
    // geometry that no longer looks like what they drew.
    if let Err(problems) = check_source_integrity(&fixture) {
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
        validate_with_registration(&layers, &profile, &rules, verified, &tile_rects, &anchors);

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
        println!("Export skipped: the project is not manufacturable.");
        for v in &report.violations {
            if v.severity == mask_rules::Severity::Error
                || v.severity == mask_rules::Severity::Fatal
            {
                println!("   {}", v.headline);
            }
        }
        return Ok(false);
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
    let manifest_doc = manifest::build_manifest(
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
    println!(
        "Next: print at 100%, then cut by hand. See docs/protocols/PHYSICAL_PROOF_PROTOCOL.md"
    );
    Ok(report.verdict.is_manufacturable())
}
