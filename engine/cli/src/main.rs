//! `maskctl` — the walking skeleton.
//!
//! One command that turns geometry plus a profile into a printable, cuttable
//! mask. Everything below Stage 5 that matters is exercised here:
//! measurement, validation against a profile, a verdict, and SVG output.
//!
//! This binary is deliberately not a product surface. It exists so that a
//! physical proof can happen before any API, editor or commerce exists.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mask_export::{tile_to_svg, ArtifactContext, MarksToDraw};
use mask_kernel::{union, widen, Point, Shapes};
use mask_nesting::{fixed_grid, needs_nesting, NestRequest};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{render, validate_with_nesting, LayerInput, Rect4};

/// Version stamped into every artifact, so reproduction can be verified later.
const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Args {
    fixture: PathBuf,
    material: PathBuf,
    plotter: PathBuf,
    out_dir: PathBuf,
    widen_to: Option<f64>,
    nest: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut fixture = None;
    let mut material = None;
    let mut plotter = None;
    let mut out_dir = None;
    let mut widen_to = None;
    let mut nest = false;

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
    })
}

fn usage() -> String {
    "maskctl — produce a printable mask from geometry plus profiles

USAGE
  maskctl --fixture <file.json> --material <profile.json> \\
          --plotter <profile.json> [--out <dir>] [--widen-to <mm>] [--nest]"
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
    layers: Vec<FixtureLayer>,
}

#[derive(serde::Deserialize)]
struct FixtureLayer {
    index: u32,
    name: String,
    #[serde(default = "default_width")]
    width_mm: f64,
    centrelines: Vec<Vec<[f64; 2]>>,
}

fn default_width() -> f64 {
    1.0
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
fn build_layer(layer: &FixtureLayer, widen_to: Option<f64>) -> Result<LayerInput, String> {
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

    Ok(LayerInput {
        index: layer.index,
        name: layer.name.clone(),
        geometry: merged,
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

    let layers: Vec<LayerInput> = fixture
        .layers
        .iter()
        .map(|l| build_layer(l, args.widen_to))
        .collect::<Result<_, _>>()?;

    if let Some(w) = args.widen_to {
        println!("Auto-Fix: widened every layer to {w} mm");
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

    let report = validate_with_nesting(&layers, &profile, &rules, verified, &tile_rects);

    println!("Measured:");
    for m in &report.layers {
        let width = m
            .min_width_mm
            .map_or("—".to_string(), |v| format!("{:.3} mm", v));
        println!(
            "  L{:02} {:<18} regions {:<3} area {:>12.1} mm²  bbox {:>7.1}×{:<7.1} mm  min width {width}",
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

    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut total_bytes = 0usize;
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
        let file = match &unit.tile {
            Some(tile) => mask_export::tile_artifact_name(
                &fixture.project_id,
                fixture.revision,
                unit.layer_index,
                &tile.marks.label,
                "svg",
            ),
            None => mask_export::artifact_name(
                &fixture.project_id,
                fixture.revision,
                unit.layer_index,
                "svg",
            ),
        };
        let path = args.out_dir.join(&file);
        fs::write(&path, &svg).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        written += 1;
        total_bytes += svg.len();
        println!("  wrote {} ({} bytes)", path.display(), svg.len());
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
    println!(
        "Next: print at 100%, then cut by hand. See docs/protocols/PHYSICAL_PROOF_PROTOCOL.md"
    );
    Ok(report.verdict.is_manufacturable())
}

fn main() -> ExitCode {
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
