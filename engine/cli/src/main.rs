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

use mask_export::{project_svgs, ArtifactContext};
use mask_kernel::{union, widen, Point, Shapes};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{render, validate, LayerInput};

/// Version stamped into every artifact, so reproduction can be verified later.
const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

struct Args {
    fixture: PathBuf,
    material: PathBuf,
    plotter: PathBuf,
    out_dir: PathBuf,
    widen_to: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut fixture = None;
    let mut material = None;
    let mut plotter = None;
    let mut out_dir = None;
    let mut widen_to = None;

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
    })
}

fn usage() -> String {
    "maskctl — produce a printable mask from geometry plus profiles

USAGE
  maskctl --fixture <file.json> --material <profile.json> \\
          --plotter <profile.json> [--out <dir>] [--widen-to <mm>]"
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

    let report = validate(&layers, &profile, &rules, verified);

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

    let export_inputs: Vec<(u32, String, Shapes)> = layers
        .iter()
        .map(|l| (l.index, l.name.clone(), l.geometry.clone()))
        .collect();

    let svgs = match project_svgs(&export_inputs, &report, &ctx) {
        Ok(v) => v,
        Err(e) => {
            println!("Export skipped: {e}");
            return Ok(false);
        }
    };

    fs::create_dir_all(&args.out_dir)
        .map_err(|e| format!("cannot create {}: {e}", args.out_dir.display()))?;

    for (index, _name, svg) in &svgs {
        let file = mask_export::artifact_name(&fixture.project_id, fixture.revision, *index, "svg");
        let path = args.out_dir.join(&file);
        fs::write(&path, svg).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        println!("  wrote {} ({} bytes)", path.display(), svg.len());
    }

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
