//! Production Manifest generation.
//!
//! The manifest is the file a third party reads first: it says whether the
//! project was ever validated, under which profiles, and whether the artifacts
//! are intact. It is generated from the same data the validator used, never from
//! anything typed in by hand.

use crate::layer_stem;
use mask_kernel::{Point, Shapes};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::CuttingProfile;
use mask_rules::Severity;
use mask_validate::{LayerInput, ValidationReport};
use serde_json::{json, Value};

/// One artifact row as recorded in the manifest.
#[derive(Debug, Clone)]
pub struct ArtifactRow {
    pub kind: &'static str,
    pub file: String,
    pub sha256: String,
    pub bytes: usize,
    pub layer: Option<u32>,
    pub tile: Option<String>,
}

/// Fixture identity and target, mirrored from the project.
pub struct ProjectRef {
    pub id: String,
    pub revision: u32,
    pub object: String,
    pub part: String,
    pub part_code: String,
    pub dimensions_mm: [f64; 2],
}

/// Print a length at report precision.
///
/// Without this the manifest carried `0.6000000000000001` for a limit a person
/// would read as 0.60 mm, and a printed manifest that disagrees with the screen
/// is a defect even when the difference is 1e-16.
fn r(value: f64) -> f64 {
    mask_kernel::report(value)
}

/// Deterministic short id for a layer, from its name.
///
/// The short suffix has to be stable: it appears in printed tile labels and in
/// stored file names, so it cannot depend on iteration order or a clock.
fn layer_id(name: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    // Exactly four hex characters: the pattern is a contract shared with file
    // names and printed tile labels, and a wider id would silently stop matching.
    format!("{hash:04x}")[..4].to_string()
}

/// Map a layer name onto a documented role.
///
/// A name is the only signal the walking skeleton has, so this stays deliberately
/// coarse. Once paint specs are carried on the layer itself, the name is no
/// longer consulted.
fn role_for(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.contains("shadow") {
        "shadow"
    } else if lower.contains("highlight") || lower.contains("white") {
        "highlight"
    } else if lower.contains("detail") || lower.contains("accent") {
        "detail"
    } else if lower.contains("stencil") {
        "stencil"
    } else {
        "base"
    }
}

/// Build the manifest document.
#[must_use]
pub fn build_manifest(
    project: &ProjectRef,
    material: &MaterialProfile,
    plotter: &PlotterProfile,
    cutting: &CuttingProfile,
    report: &ValidationReport,
    layers: &[LayerInput],
    artifacts: &[ArtifactRow],
) -> Value {
    build_manifest_with_override(
        project, material, plotter, cutting, report, layers, artifacts, None,
    )
}

/// Build the manifest, optionally recording that a refusal was overridden.
///
/// The override cannot be inferred from the report: the verdict and the status are
/// deliberately left alone, so a `FAIL` document stays a `FAIL` document and reads
/// as `NeedsWork`. A manifest that quietly looked manufacturable would be worse
/// than no manifest, because the whole reason these files exist is that somebody is
/// about to put a knife to them.
///
/// Eight inputs, one of them the override that only a single caller supplies.
// Bundling them into a struct would hide which arguments a caller actually has to
// think about, and this function is read as one question: "what does a manifest for
// this run say?".
#[allow(clippy::too_many_arguments)]
pub fn build_manifest_with_override(
    project: &ProjectRef,
    material: &MaterialProfile,
    plotter: &PlotterProfile,
    cutting: &CuttingProfile,
    report: &ValidationReport,
    layers: &[LayerInput],
    artifacts: &[ArtifactRow],
    // Rule codes the operator overrode, if any.
    overridden_codes: Option<&[String]>,
) -> Value {
    let limits = &cutting.limits;

    let override_value = match overridden_codes {
        None => Value::Null,
        Some(codes) => json!({
            "reason": "the validator refused this project and the files were produced anyway",
            "codes": codes,
            "verdict_at_export": verdict_str(report),
            "warning": "NOT A PRODUCTION RUN. These artifacts exist to be cut by hand under observation.",
        }),
    };

    let layer_rows: Vec<Value> = layers
        .iter()
        .map(|l| {
            let metrics = report.layers.iter().find(|m| m.index == l.index);
            let tiles: Vec<Value> = artifacts
                .iter()
                .filter(|a| a.layer == Some(l.index))
                .filter(|a| a.kind == "cut_svg")
                .filter_map(|a| a.tile.clone())
                .map(|tile| {
                    let file = format!(
                        "{}_{}.svg",
                        layer_stem(&project.id, project.revision, l.index),
                        tile
                    );
                    let sha = artifacts
                        .iter()
                        .find(|a| a.kind == "cut_svg" && a.file == file)
                        .map_or_else(String::new, |a| a.sha256.clone());
                    json!({ "id": tile, "file": file, "sha256": sha })
                })
                .collect();

            json!({
                "index": l.index,
                "id": format!("L{:02}-{}", l.index, &layer_id(&l.name)),
                "name": l.name,
                "role": role_for(&l.name),
                "elements": {
                    "paths": metrics.map_or(0, |m| m.paths),
                    "islands": metrics.map_or(0, |m| m.islands),
                    "min_width_mm": metrics.and_then(|m| m.min_width_mm).map(r),
                },
                "tiles": tiles,
            })
        })
        .collect();

    let artifact_rows: Vec<Value> = artifacts
        .iter()
        .map(|a| {
            let mut row = json!({
                "kind": a.kind,
                "file": a.file,
                "sha256": a.sha256,
                "bytes": a.bytes,
            });
            if let Some(layer) = a.layer {
                row["layer"] = json!(layer);
            }
            if let Some(tile) = &a.tile {
                row["tile"] = json!(tile);
            }
            row
        })
        .collect();

    // Warning headlines are not duplicated into the manifest: the count lives in
    // `violations.warning` and the text lives in the validation report. Copying
    // both would create a second place for the same fact to go stale.
    debug_assert_eq!(
        report.counts.warning,
        report
            .violations
            .iter()
            .filter(|v| v.severity == Severity::Warning)
            .count(),
        "warning count and violation list must agree"
    );

    json!({
        "manifest_version": 1,
        "project": {
            "id": project.id,
            "revision": project.revision,
            "status": status_for(report),
        },
        "target": {
            "object": &project.object,
            "part": &project.part,
            "part_code": &project.part_code,
            "dimensions_mm": project.dimensions_mm,
        },
        "manufacturing": {
            "material": {
                "code": material.code,
                "version": material.version,
                "status": material.status.as_str(),
            },
            "plotter": { "code": plotter.code, "version": plotter.version },
            "cutting_profile": cutting.code,
            "kerf_mm": r(limits.kerf_mm),
            "effective_limits": {
                "line_width_mm": r(limits.line_width_mm),
                "gap_mm": r(limits.gap_mm),
                "island_width_mm": r(limits.island_width_mm),
                "corner_radius_mm": r(limits.corner_radius_mm),
                "bridge_width_mm": r(limits.bridge_width_mm),
                "max_features_per_100cm2": r(limits.max_features_per_100cm2),
                "max_tile_width_mm": r(limits.max_tile_width_mm),
                "max_tile_length_mm": r(limits.max_tile_length_mm),
            },
        },
        "validation": {
            "verdict": verdict_str(report),
            "violations": {
                "error": report.counts.error,
                "warning": report.counts.warning,
                "info": report.counts.info,
            },
            // The whole point of this field: a Pass computed against datasheet
            // limits has never met a sheet of film.
            "physical_proof": {
                "performed": false,
                "notes": "Material not procured. Limits are datasheet values, not measurements.",
            },
            // Present only when the export went ahead against the verdict above.
            // Absent otherwise, so its presence is the signal — an explicit
            // "override": false would be a field someone could set to make a
            // refusing run look reviewed.
            "export_override": override_value,
        },
        "lineage": {
            "engine_version": env!("CARGO_PKG_VERSION"),
            "rule_engine_version": env!("CARGO_PKG_VERSION"),
            "lib_math_mode": "Integer",
        },
        "layers": layer_rows,
        "artifacts": artifact_rows,
        "signatures": {
            "generated_by": format!("export-engine/{}", env!("CARGO_PKG_VERSION")),
        },
    })
}

/// Project status implied by the verdict.
fn status_for(report: &ValidationReport) -> &'static str {
    if report.verdict.is_manufacturable() {
        "NeedsReview"
    } else {
        "NeedsWork"
    }
}

fn verdict_str(report: &ValidationReport) -> String {
    match report.verdict {
        mask_validate::Verdict::Pass => "PASS",
        mask_validate::Verdict::PassWithWarnings => "PASS_WITH_WARNINGS",
        mask_validate::Verdict::Fail => "FAIL",
        mask_validate::Verdict::Fatal => "FATAL",
    }
    .to_string()
}

/// Registration geometry for the DXF layer: the two anchor pairs as thin lines.
///
/// The marks are drawn on their own layer so a bench worker can delete them in
/// CAD without touching the cut geometry.
#[must_use]
pub fn registration_geometry(marks: &mask_nesting::RegistrationMarks) -> Shapes {
    let mut out = Shapes::new();
    let pairs = [
        (marks.part_anchor.left, marks.part_anchor.right),
        (marks.tile_anchor.left, marks.tile_anchor.right),
    ];
    for (a, b) in pairs {
        let line = mask_kernel::widen(
            &[
                Point {
                    x: a.x_mm,
                    y: a.y_mm,
                },
                Point {
                    x: b.x_mm,
                    y: b.y_mm,
                },
            ],
            0.2,
        )
        .unwrap_or_default();
        out.shapes.extend(line.shapes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::{Contour, Shape};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn rect() -> Shapes {
        Shapes::single(Shape::new(Contour {
            points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 5.0), p(0.0, 5.0)],
            closed: true,
            arcs: Vec::new(),
        }))
    }

    #[test]
    fn the_manifest_declares_that_nothing_has_been_proven() {
        let report = ValidationReport {
            verdict: mask_validate::Verdict::Pass,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 0,
                warning: 0,
                info: 0,
            },
            violations: vec![],
            layers: vec![],
            fidelity: Some(0.9),
            limits_physically_verified: false,
        };
        let json = build_manifest(
            &project_ref("MASK-000001"),
            &material(),
            &plotter(),
            &CuttingProfile::derive(&material(), &plotter()),
            &report,
            &[],
            &[],
        );
        assert_eq!(json["validation"]["physical_proof"]["performed"], false);
        assert_eq!(json["validation"]["verdict"], "PASS");
    }

    #[test]
    fn an_override_keeps_the_failing_verdict_and_names_the_codes() {
        // The point of the field: files that exist to be cut under observation must
        // not read as a production run. The verdict stays FAIL, the status stays
        // NeedsWork, and the codes are named so the refusal is readable from the
        // manifest alone.
        let report = ValidationReport {
            verdict: mask_validate::Verdict::Fail,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 1,
                warning: 0,
                info: 0,
            },
            violations: vec![],
            layers: vec![],
            fidelity: None,
            limits_physically_verified: false,
        };
        let codes = vec!["min_gap".to_string()];
        let json = build_manifest_with_override(
            &project_ref("MASK-000003"),
            &material(),
            &plotter(),
            &CuttingProfile::derive(&material(), &plotter()),
            &report,
            &[],
            &[],
            Some(&codes),
        );

        assert_eq!(json["validation"]["verdict"], "FAIL");
        assert_eq!(json["project"]["status"], "NeedsWork");
        assert_eq!(json["validation"]["export_override"]["codes"][0], "min_gap");
        assert_eq!(
            json["validation"]["export_override"]["verdict_at_export"],
            "FAIL"
        );
    }

    #[test]
    fn no_override_means_no_field_rather_than_a_false_one() {
        // Presence is the signal. An explicit `false` would be a field someone
        // could set to make a refusing run look reviewed.
        let report = ValidationReport {
            verdict: mask_validate::Verdict::Pass,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 0,
                warning: 0,
                info: 0,
            },
            violations: vec![],
            layers: vec![],
            fidelity: None,
            limits_physically_verified: false,
        };
        let json = build_manifest(
            &project_ref("MASK-000004"),
            &material(),
            &plotter(),
            &CuttingProfile::derive(&material(), &plotter()),
            &report,
            &[],
            &[],
        );
        assert!(
            json["validation"]["export_override"].is_null(),
            "an ordinary run must carry no override field"
        );
    }

    #[test]
    fn a_failing_report_maps_to_a_non_manufacturable_status() {
        let report = ValidationReport {
            verdict: mask_validate::Verdict::Fail,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 3,
                warning: 0,
                info: 0,
            },
            violations: vec![],
            layers: vec![],
            fidelity: None,
            limits_physically_verified: false,
        };
        let json = build_manifest(
            &project_ref("MASK-000002"),
            &material(),
            &plotter(),
            &CuttingProfile::derive(&material(), &plotter()),
            &report,
            &[],
            &[],
        );
        assert_eq!(json["validation"]["verdict"], "FAIL");
        assert_eq!(json["project"]["status"], "NeedsWork");
    }

    #[test]
    fn the_manifest_records_the_effective_limits_not_just_the_profile() {
        let report = ValidationReport {
            verdict: mask_validate::Verdict::Pass,
            counts: mask_validate::Counts {
                fatal: 0,
                error: 0,
                warning: 0,
                info: 0,
            },
            violations: vec![],
            layers: vec![],
            fidelity: None,
            limits_physically_verified: false,
        };
        let cp = CuttingProfile::derive(&material(), &plotter());
        let json = build_manifest(
            &project_ref("MASK-000003"),
            &material(),
            &plotter(),
            &cp,
            &report,
            &[],
            &[],
        );
        // A reader must never have to re-derive these.
        assert_eq!(
            json["manufacturing"]["effective_limits"]["line_width_mm"],
            0.8
        );
        assert_eq!(
            json["manufacturing"]["effective_limits"]["max_tile_width_mm"],
            284.0
        );
    }

    fn project_ref(id: &str) -> ProjectRef {
        ProjectRef {
            id: id.into(),
            revision: 1,
            object: "Audi A4 B9".into(),
            part: "Hood".into(),
            part_code: "HOOD".into(),
            dimensions_mm: [1412.0, 1046.0],
        }
    }

    fn material() -> MaterialProfile {
        MaterialProfile {
            code: "matte-vinyl-090".into(),
            version: 4,
            name: "t".into(),
            family: "vinyl".into(),
            thickness_mm: 0.09,
            minimums: mask_profiles::Minimums {
                line_width_mm: 0.80,
                gap_mm: 0.90,
                island_area_mm2: 2.40,
                island_width_mm: 0.60,
                feature_length_mm: 3.00,
                corner_radius_mm: 0.40,
                bridge_width_mm: 1.20,
                bridge_length_mm: 25.0,
            },
            physical: mask_profiles::Physical {
                adhesion: "medium".into(),
                adhesive_system: "water_sensitive".into(),
                peel_angle_min_deg: 120.0,
                peel_rate: "slow".into(),
                max_temp_c: 80.0,
                tolerance_registration_mm: 0.35,
                surface_prep: vec![],
            },
            limits: mask_profiles::Limits {
                max_density_per_100cm2: 180.0,
                min_angle_deg: 30.0,
                max_area_mm2: 400_000.0,
            },
            status: mask_profiles::ProfileStatus::Draft,
            verified_physical: false,
            notes: String::new(),
        }
    }

    fn plotter() -> PlotterProfile {
        PlotterProfile {
            code: "knife-25mm".into(),
            version: 2,
            name: "t".into(),
            kind: "knife_plotter".into(),
            blade: mask_profiles::Blade {
                angle_deg: 45.0,
                offset_mm: 0.10,
                kerf_mm: 0.15,
                curve_tolerance_mm: 0.08,
                curve_multiplier: 4.0,
            },
            working_area: mask_profiles::WorkingArea {
                width_mm: 300.0,
                length_mm: 1200.0,
                usable_margin_mm: 8.0,
            },
            accuracy: mask_profiles::Accuracy {
                repeatability_mm: 0.05,
                registration_method: "crosshair".into(),
                max_registration_error_mm: 0.30,
            },
            status: mask_profiles::ProfileStatus::Draft,
            verified_physical: true,
        }
    }

    #[test]
    fn registration_geometry_produces_both_anchor_pairs() {
        let marks = mask_nesting::RegistrationMarks {
            part_anchor: mask_nesting::MarkPoints {
                left: mask_nesting::MarkPoint {
                    x_mm: 10.0,
                    y_mm: 50.0,
                },
                right: mask_nesting::MarkPoint {
                    x_mm: 270.0,
                    y_mm: 50.0,
                },
            },
            tile_anchor: mask_nesting::MarkPoints {
                left: mask_nesting::MarkPoint {
                    x_mm: 140.0,
                    y_mm: 10.0,
                },
                right: mask_nesting::MarkPoint {
                    x_mm: 140.0,
                    y_mm: 90.0,
                },
            },
            orientation: true,
            label: "A1".into(),
        };
        let geo = registration_geometry(&marks);
        assert!(!geo.is_empty());
        assert!(mask_kernel::shapes_area(&geo) > 0.0);
    }

    #[test]
    fn the_geometry_fixture_is_usable() {
        assert!(!rect().is_empty());
    }

    #[test]
    fn layer_short_ids_are_stable() {
        assert_eq!(layer_id("Black Base"), layer_id("Black Base"));
        assert_ne!(layer_id("Black Base"), layer_id("Red Base"));
        assert_eq!(layer_id("Black Base").len(), 4);
    }

    #[test]
    fn layer_names_map_onto_documented_roles() {
        assert_eq!(role_for("01 Black Base"), "base");
        assert_eq!(role_for("02 Black Shadow"), "shadow");
        assert_eq!(role_for("05 White"), "highlight");
        assert_eq!(role_for("06 Detail"), "detail");
        assert_eq!(role_for("Gold Accent"), "detail");
    }
}
