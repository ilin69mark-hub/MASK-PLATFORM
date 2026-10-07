//! Browser-facing wrapper around the Mask Engine.
//!
//! Exists so that the geometry code in `mask-kernel`, `mask-rules`,
//! `mask-validate` and `mask-nesting` can run in the browser without a second
//! implementation. One implementation means a measurement cannot disagree
//! between the server and the editor — which is the failure mode that matters,
//! because it appears only at submission time.
//!
//! Contract with JavaScript (ADR-0013):
//!
//! * only strings and numbers cross the boundary, never geometry types;
//! * errors arrive as rejected promises carrying a message, never as panics;
//! * `MathMode` and the integer grid are fixed in this build, not chosen by the
//!   caller, because a caller that picks them differently would produce
//!   different hashes for the same geometry.
//!
//! The browser may *evaluate* a project. It may not *approve* one: the verdict it
//! returns is advisory, and only the server writes an approval.

use mask_kernel::bridge::{bridge_unanchored, BridgeLimits};
use mask_kernel::geom::{Point, Shapes};
use mask_kernel::{measure, ContentHash};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{validate_with_nesting, BridgeRecord, LayerInput, Rect4, Verdict};
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

/// What JavaScript gets back about a layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerResult {
    pub area_mm2: f64,
    pub min_width_mm: Option<f64>,
    /// Tightest gap between two regions, and which two.
    ///
    /// T-02. The note that used to stand here said this was missing until the check
    /// was implemented, and then it stayed that way after it *was* implemented —
    /// `gap.rs` has been here since the rule was closed, and the browser could not
    /// see any of it. That is the worst state of all: the field was deliberately
    /// withheld so its absence would be visible, and the absence stayed invisible.
    ///
    /// A user dragging one element toward another in the editor has no way to see
    /// the gap closing, and the validator's `min_gap` error then arrives at
    /// submission describing something the editor let them do.
    pub min_gap: Option<GapReport>,
    /// Regions that cross themselves, with the crossing point named.
    ///
    /// The CLI refuses such geometry before any merge touches it, because union
    /// resolves a self-intersection silently: a bow tie goes in as one contour
    /// reporting 0 mm² and comes out as two triangles, and nothing is reported.
    /// The editor had no such gate, so a vertex dragged into a bow tie showed a
    /// plausible area on screen and was only rejected on the way to the server.
    ///
    /// Reported rather than repaired, for the reason `integrity.rs` gives: resolving
    /// it changes the figure, and a mask is a physical object cut out of film.
    pub self_intersections: Vec<Crossing>,
    pub island_count: usize,
    pub hole_count: usize,
    pub bbox: [f64; 4],
    pub content_hash: String,
    /// Paths as SVG path data, already canonicalised and quantised.
    pub path_data: Vec<String>,
}

/// What JavaScript gets back about a whole project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationResult {
    pub verdict: String,
    pub error_count: usize,
    pub warning_count: usize,
    pub info_count: usize,
    /// Headline text per violation, ready to display.
    pub messages: Vec<String>,
    pub limits_physically_verified: bool,
}

/// The tightest gap in a layer, and the pair of regions it sits between.
///
/// `mm` is zero when the regions touch or overlap, which is the case that matters:
/// they are one physical piece, and there is no film left between them to hold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GapReport {
    /// Index of the first region.
    pub a: usize,
    /// Index of the second region.
    pub b: usize,
    pub mm: f64,
}

/// One point where a contour crosses itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Crossing {
    /// Index of the shape.
    pub shape: usize,
    /// Index of the contour: 0 is the outer contour, 1.. are holes.
    pub contour: usize,
    /// Segment index within the contour, counting the closing segment last.
    pub segment: usize,
    pub at: [f64; 2],
}

/// An error that is safe to show a user.
#[derive(Debug, Clone, Serialize)]
pub struct JsError {
    pub message: String,
}

impl JsError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<JsError> for wasm_bindgen::JsValue {
    fn from(e: JsError) -> Self {
        js_sys::Error::new(&e.message).into()
    }
}

/// Validate a browser evaluation bundle containing inline layer geometry.
///
/// This is not a `mask-project.v1.json` document. The production Mask Project
/// deliberately stores only coordinate-frame metadata and references vector
/// geometry through `geometry_id` (ADR-0016). Its layers cannot be checked until
/// their external geometry records are resolved, and no resolver exists while the
/// service layer is deferred. The browser therefore validates a self-contained
/// evaluation bundle: project-like metadata plus the required inline `geometry`
/// needed to perform the check. Retaining the project shape keeps field handling
/// and verdict semantics consistent without claiming that a production document
/// was validated.
#[wasm_bindgen]
pub fn validate_project(
    project_json: &str,
    material_json: &str,
    plotter_json: &str,
) -> Result<JsValue, JsValue> {
    to_js(&validate_project_inner(project_json, material_json, plotter_json).map_err(into_js)?)
}

/// Typed project validation, callable from Rust tests.
///
/// The split exists so a claim about which checks reach the browser can be
/// proved on the host instead of asserted. Both the crossings check (T-12) and
/// the registration check (T-15) live inside `validate_with_registration`, which
/// `validate_with_nesting` calls — so whether they reach the editor is a
/// question about the *inputs* this function builds, and that is testable.
fn validate_project_inner(
    project_json: &str,
    material_json: &str,
    plotter_json: &str,
) -> Result<ValidationResult, JsError> {
    let material: MaterialProfile = parse(material_json, "material profile")?;
    let plotter: PlotterProfile = parse(plotter_json, "plotter profile")?;
    let value: serde_json::Value = parse(project_json, "project")?;

    let cutting = CuttingProfile::derive(&material, &plotter);
    let rules = RuleSet::from_profile(&cutting);
    let verified = cutting.is_physically_verified(&material, &plotter);

    let layers = extract_layers(&value)?;
    let tiles = extract_tiles(&value);

    let report = validate_with_nesting(&layers, &cutting, &rules, verified, &tiles);

    Ok(ValidationResult {
        verdict: verdict_name(report.verdict).to_string(),
        error_count: report.counts.error,
        warning_count: report.counts.warning,
        info_count: report.counts.info,
        messages: report
            .violations
            .iter()
            .map(|v| v.headline.clone())
            .collect(),
        limits_physically_verified: report.limits_physically_verified,
    })
}

/// Measure one layer and return its geometry ready for drawing.
#[wasm_bindgen]
pub fn measure_layer(shapes_json: &str) -> Result<JsValue, JsValue> {
    to_js(&measure_layer_inner(shapes_json)?)
}

/// Typed measurement, callable from Rust tests without a JS value in the way.
fn measure_layer_inner(shapes_json: &str) -> Result<LayerResult, JsError> {
    let shapes: Shapes = parse(shapes_json, "shapes")?;
    shapes
        .validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| JsError::new(e.to_string()))?;

    let (_, _, w, h) = mask_kernel::bounding_box(&shapes);

    let min_width_mm = shapes
        .shapes
        .iter()
        .filter_map(|s| mask_kernel::min_width(&Shapes::single(s.clone())))
        .reduce(f64::min);

    // T-02. Absent rather than zero for a single region: one piece of film has no
    // neighbour, and a gap against nothing would be a number invented rather than
    // measured.
    let min_gap = mask_kernel::min_gap(&shapes).map(|g| GapReport {
        a: g.a,
        b: g.b,
        mm: g.mm,
    });

    // Reported, not repaired — see the field's doc comment.
    let self_intersections = mask_kernel::self_intersections(&shapes)
        .into_iter()
        .map(|c| Crossing {
            shape: c.shape,
            contour: c.contour,
            segment: c.segment,
            at: [c.at.x, c.at.y],
        })
        .collect();

    let path_data: Vec<String> = shapes
        .shapes
        .iter()
        .flat_map(|s| s.contours())
        .map(mask_kernel::to_path_data)
        .collect();

    Ok(LayerResult {
        area_mm2: measure::shapes_area(&shapes),
        min_width_mm,
        min_gap,
        self_intersections,
        island_count: shapes.shapes.len(),
        hole_count: shapes.shapes.iter().map(|s| s.holes.len()).sum(),
        bbox: [0.0, 0.0, w, h],
        content_hash: ContentHash::of(&shapes).to_string(),
        path_data,
    })
}

/// Widen a centreline to a target band width, then return the band.
///
/// This is what the editor calls for its "widen" tool, so the preview the user
/// sees is the geometry that would actually be cut.
#[wasm_bindgen]
pub fn widen_centreline(points_json: &str, target_width_mm: f64) -> Result<JsValue, JsValue> {
    to_js(&widen_centreline_inner(points_json, target_width_mm).map_err(into_js)?)
}

/// Typed widening, callable from Rust tests.
fn widen_centreline_inner(points_json: &str, target_width_mm: f64) -> Result<LayerResult, JsError> {
    let raw: Vec<[f64; 2]> = parse(points_json, "centreline")?;
    if raw.len() < 2 {
        return Err(JsError::new("a centreline needs at least two points"));
    }
    let mut points = Vec::with_capacity(raw.len());
    for (i, p) in raw.iter().enumerate() {
        points.push(
            Point::new(p[0], p[1], mask_kernel::MAX_COORDINATE_MM)
                .map_err(|e| JsError::new(format!("point {i}: {e}")))?,
        );
    }
    let band =
        mask_kernel::widen(&points, target_width_mm).map_err(|e| JsError::new(e.to_string()))?;
    measure_layer_inner(&to_string(&band)?)
}

/// Union of two shape sets.
///
/// Present because the editor builds a layer by stroking several centrelines and
/// then merging them. Doing that merge in TypeScript would be an approximation,
/// and an approximation in a preview is a lie the user cannot see.
#[wasm_bindgen]
pub fn union(a_json: &str, b_json: &str) -> Result<JsValue, JsValue> {
    let a: Shapes = parse(a_json, "shapes").map_err(into_js)?;
    let b: Shapes = parse(b_json, "shapes").map_err(into_js)?;
    a.validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| into_js(JsError::new(e.to_string())))?;
    b.validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| into_js(JsError::new(e.to_string())))?;
    let merged = mask_kernel::union(&a, &b).map_err(|e| JsError::new(e.to_string()))?;
    to_js(&merged)
}

/// The result of an auto-fix pass: repaired geometry plus what was done to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepairResult {
    /// Geometry with the tabs merged in, ready to draw and to re-validate.
    pub shapes: Shapes,
    /// The tabs that were added, in the form the validator checks.
    ///
    /// Returned as records rather than kept on the host, because the identity of
    /// a tab exists only here: merged into the geometry it is indistinguishable
    /// from any other neck, and T-06/T-07 have nothing to measure unless the tab
    /// says what it is.
    pub bridges: Vec<BridgeRecord>,
}

/// Add tabs to every island that is not attached to the surrounding film.
///
/// This is the browser half of `maskctl --bridge`. Without it the editor cannot
/// repair a layer, and without its records the validator cannot check the tabs:
/// a document could then *claim* a bridge as a number, and T-07 would validate a
/// claim rather than a geometry.
///
/// The limits are arguments rather than profile lookups because the caller owns
/// the material: passing the profile's own `bridge_width_mm` and
/// `minimums.bridge_length_mm` is what keeps the editor and the station cutting
/// the same neck. Pass a *larger* `max_length_mm` and T-07 will then report the
/// tab against the material — which is the check doing its job, not a failure of
/// the repair.
#[wasm_bindgen]
pub fn bridge_unanchored_islands(
    shapes_json: &str,
    width_mm: f64,
    max_length_mm: f64,
) -> Result<JsValue, JsValue> {
    to_js(&bridge_inner(shapes_json, width_mm, max_length_mm).map_err(into_js)?)
}

/// Typed bridging, callable from Rust tests.
///
/// All-or-nothing, exactly like the CLI: an island that cannot be reached leaves
/// the layer as it was, with the reason in the error. Keeping that behaviour
/// identical in both runtimes is the point of ADR-0013 — an editor that placed
/// some tabs and the station that placed none would disagree about the same file.
fn bridge_inner(
    shapes_json: &str,
    width_mm: f64,
    max_length_mm: f64,
) -> Result<RepairResult, JsError> {
    let shapes: Shapes = parse(shapes_json, "shapes")?;
    shapes
        .validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| JsError::new(e.to_string()))?;

    let limits = BridgeLimits {
        width_mm,
        max_length_mm,
    };
    let (repaired, bridges) =
        bridge_unanchored(&shapes, limits).map_err(|e| JsError::new(e.to_string()))?;

    Ok(RepairResult {
        shapes: repaired,
        bridges: bridges
            .iter()
            .map(|b| BridgeRecord {
                width_mm: b.width_mm,
                length_mm: b.length_mm,
                at: b.to,
            })
            .collect(),
    })
}

/// The hash scheme these digests belong to.
///
/// Exposed so the cross-runtime check can tell a stale golden file from a real
/// disagreement. When the scheme moves, every digest moves with it, and the
/// difference between "the browser is broken" and "the golden was never
/// regenerated" is the difference between debugging the wrong thing.
#[wasm_bindgen]
pub fn hash_scheme() -> u32 {
    mask_kernel::HASH_SCHEME_VERSION
}

/// Canonical content hash of a shape set, for change detection in the editor.
#[wasm_bindgen]
pub fn content_hash(shapes_json: &str) -> Result<String, JsValue> {
    let shapes: Shapes = parse(shapes_json, "shapes").map_err(into_js)?;
    Ok(ContentHash::of(&shapes).to_string())
}

/// The effective limits a project would be validated against.
///
/// The editor shows these so a user can see why something failed instead of
/// guessing at a number.
#[wasm_bindgen]
pub fn effective_limits(material_json: &str, plotter_json: &str) -> Result<JsValue, JsValue> {
    let material: MaterialProfile = parse(material_json, "material profile").map_err(into_js)?;
    let plotter: PlotterProfile = parse(plotter_json, "plotter profile").map_err(into_js)?;
    let cutting = CuttingProfile::derive(&material, &plotter);
    to_js(&cutting.limits)
}

/// Constants the editor needs in order to draw consistently with the engine.
///
/// Returned as a struct rather than hard-coded in TypeScript, so the two cannot
/// drift apart.
#[wasm_bindgen]
pub fn engine_constants() -> JsValue {
    let value = serde_json::json!({
        "canon_quantum_mm": mask_kernel::CANON_QUANTUM_MM,
        "coord_dp": mask_kernel::CANON_COORD_DP,
        "measure_report_dp": mask_kernel::MEASURE_REPORT_DP,
        "min_width_eps_mm": mask_kernel::MIN_WIDTH_EPS_MM,
        "max_coordinate_mm": mask_kernel::MAX_COORDINATE_MM,
        "lib_scale": mask_kernel::LIB_SCALE,
        "lib_math_mode": "Integer",
        "engine_version": env!("CARGO_PKG_VERSION"),
    });
    to_js(&value).expect("constants are static and cannot fail")
}

/// Version of the browser kernel, for display and mismatch checks.
#[wasm_bindgen]
pub fn engine_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

// ── helpers ───────────────────────────────────────────────────────────

fn parse<T: serde::de::DeserializeOwned>(json: &str, what: &str) -> Result<T, JsError> {
    serde_json::from_str(json).map_err(|e| JsError::new(format!("cannot parse {what}: {e}")))
}

/// Convert a typed error into something a JS caller can read.
///
/// Guarded because `js_sys::Error` needs a live JS runtime and would panic on
/// the host. Keeping the guard means the typed core stays unit-testable without
/// standing up a browser, while a real wasm build still gets a proper Error
/// object with a message.
fn into_js(e: JsError) -> JsValue {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Error::new(&e.message).into()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        JsValue::from_str(&e.message)
    }
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    // `serialize_maps_as_objects` is what makes a serde_json::Value object come
    // across as a plain JS object. The default produces a `Map`, so
    // `result.area_mm2` would be `undefined` while `result.get("area_mm2")`
    // worked — a difference visible only in JavaScript, and exactly the kind of
    // quiet split between two runtimes this design exists to prevent.
    let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
    value
        .serialize(&serializer)
        .map_err(|e| JsError::new(e.to_string()).into())
}

fn to_string<T: Serialize>(value: &T) -> Result<String, JsError> {
    serde_json::to_string(value).map_err(|e| JsError::new(e.to_string()))
}

fn verdict_name(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "PASS",
        Verdict::PassWithWarnings => "PASS_WITH_WARNINGS",
        Verdict::Fail => "FAIL",
        Verdict::Fatal => "FATAL",
    }
}

/// Pull the layer list out of a Mask Project document.
fn extract_layers(value: &serde_json::Value) -> Result<Vec<LayerInput>, JsError> {
    let layers = value
        .get("composition")
        .and_then(|c| c.get("layers"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| JsError::new("project has no composition.layers"))?;

    let mut out = Vec::with_capacity(layers.len());
    for layer in layers {
        let index = layer
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as u32;
        let name = layer
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("layer")
            .to_string();

        // Geometry may be inline or referenced. Only inline is supported here;
        // a reference needs the geometry store, which lives on the server.
        let geometry = match layer.get("geometry") {
            Some(serde_json::Value::Object(map)) if map.contains_key("shapes") => {
                let shapes: Shapes =
                    serde_json::from_value::<Shapes>(serde_json::Value::Object(map.clone()))
                        .map_err(|e| JsError::new(format!("layer {index} geometry: {e}")))?;
                // Validated here, at the boundary where untrusted JSON becomes
                // geometry. `measure_layer_inner` has always done this; `validate_project`
                // did not, and that left the document path as the one way to reach the
                // validator with geometry nothing else would accept.
                //
                // ADR-0015 closed the panic inside the kernel, and this is why that
                // was not enough on its own: a contour with no points and a non-empty
                // arc list still reached `resolved_arcs`, where `points.len() - 1`
                // underflows. Re-verified on the rebuilt artifact — the kernel check
                // cannot run because nothing validated the input first.
                //
                // The error arrives as a rejected promise rather than a trap, which is
                // the difference between the editor saying what is wrong with the
                // document and the editor becoming unusable.
                shapes
                    .validate_strict(mask_kernel::MAX_COORDINATE_MM)
                    .map_err(|e| JsError::new(format!("layer {index} geometry: {e}")))?;
                shapes
            }
            _ => {
                return Err(JsError::new(format!(
                    "layer {index} has no inline geometry; the browser kernel evaluates inline geometry only"
                )))
            }
        };

        out.push(LayerInput {
            index,
            name,
            geometry,
            bridges: extract_bridges(layer, index)?,
            // No paint order travels. T-12 still runs, because the validator
            // falls back to `index` when both orders are absent, and two layers
            // sharing an index is a crossing with nothing to resolve it. Paint
            // order is therefore implicit in layer order, and saying so is more
            // useful than implying a server decides it.
            application_order: None,
        });
    }
    Ok(out)
}

/// Read the tabs a layer declares, so T-06 and T-07 can measure them.
///
/// A tab is geometry once it is merged, and nothing can tell it from any other
/// neck afterwards — that is the whole reason `LayerInput` carries a list. The
/// records come from `bridge_unanchored_islands`, which places the tab and names
/// it in one call, so a document states what was actually built rather than
/// asserting a number.
///
/// Defaults to empty: a layer with no `bridges` field has no tabs to check, which
/// is the honest reading, and the unanchored-island rule still reports the island
/// itself.
fn extract_bridges(layer: &serde_json::Value, index: u32) -> Result<Vec<BridgeRecord>, JsError> {
    let Some(value) = layer.get("bridges") else {
        return Ok(Vec::new());
    };
    let raw: Vec<BridgeRecord> = serde_json::from_value(value.clone())
        .map_err(|e| JsError::new(format!("layer {index} bridges: {e}")))?;

    for (position, b) in raw.iter().enumerate() {
        if !b.width_mm.is_finite() || !b.length_mm.is_finite() {
            return Err(JsError::new(format!(
                "layer {index} bridge {position}: width and length must be finite"
            )));
        }
        if b.width_mm <= 0.0 {
            return Err(JsError::new(format!(
                "layer {index} bridge {position}: width {} mm is not a neck",
                b.width_mm
            )));
        }
    }
    Ok(raw)
}

/// Pull per-layer tile rectangles, when a nesting plan is present.
fn extract_tiles(value: &serde_json::Value) -> std::collections::BTreeMap<u32, Vec<Rect4>> {
    let mut map = std::collections::BTreeMap::new();
    let Some(plans) = value
        .get("nesting")
        .and_then(|n| n.get("layers"))
        .and_then(serde_json::Value::as_array)
    else {
        return map;
    };

    for plan in plans {
        let Some(index) = plan.get("layer").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(tiles) = plan.get("tiles").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let rects: Vec<Rect4> = tiles
            .iter()
            .filter_map(|t| t.get("bbox").and_then(serde_json::Value::as_array))
            .filter(|a| a.len() == 4)
            .filter_map(|a| {
                Some([
                    a[0].as_f64()?,
                    a[1].as_f64()?,
                    a[2].as_f64()?,
                    a[3].as_f64()?,
                ])
            })
            .collect();
        if !rects.is_empty() {
            map.insert(index as u32, rects);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(w: f64, h: f64) -> String {
        serde_json::json!({
            "shapes": [{
                "outer": {
                    "points": [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]],
                    "closed": true
                },
                "holes": []
            }]
        })
        .to_string()
    }

    #[test]
    fn measuring_a_square_returns_its_width_and_area() {
        let result = measure_layer_inner(&square(10.0, 5.0)).expect("measure");
        assert!((result.area_mm2 - 50.0).abs() < 1e-9);
        assert!((result.min_width_mm.expect("width") - 5.0).abs() < 1e-3);
        assert_eq!(result.island_count, 1);
    }

    #[test]
    fn the_gap_between_two_regions_reaches_the_editor() {
        // Two squares 3 mm apart measured between their facing edges. The gap is
        // checked in both axes, because a diagonal-only implementation understates
        // it and that error is invisible in the axis-aligned case.
        let two = serde_json::json!({
            "shapes": [
                { "outer": { "points": [[0.0,0.0],[5.0,0.0],[5.0,5.0],[0.0,5.0]], "closed": true },
                  "holes": [] },
                { "outer": { "points": [[8.0,0.0],[13.0,0.0],[13.0,5.0],[8.0,5.0]], "closed": true },
                  "holes": [] }
            ]
        })
        .to_string();

        let result = measure_layer_inner(&two).expect("measure");
        let gap = result.min_gap.expect("two regions have a gap");
        assert!((gap.mm - 3.0).abs() < 1e-6, "got {}", gap.mm);
        assert_eq!((gap.a, gap.b), (0, 1));
    }

    #[test]
    fn a_single_region_reports_no_gap_rather_than_a_measured_zero() {
        let result = measure_layer_inner(&square(10.0, 5.0)).expect("measure");
        assert!(
            result.min_gap.is_none(),
            "one region has no neighbour; a number here would be invented"
        );
    }

    #[test]
    fn a_bow_tie_is_reported_rather_than_measured_as_zero_area() {
        // The failure the CLI's integrity gate exists to stop. The signed rule
        // cancels two lobes of 50 mm² each and reports 0 mm², which looks like a
        // measurement rather than an error.
        let bow_tie = serde_json::json!({
            "shapes": [{
                "outer": {
                    "points": [[0.0,0.0],[10.0,10.0],[10.0,0.0],[0.0,10.0]],
                    "closed": true
                },
                "holes": []
            }]
        })
        .to_string();

        let result = measure_layer_inner(&bow_tie).expect("measure");
        assert!(
            !result.self_intersections.is_empty(),
            "a contour crossing itself must be named, not measured"
        );
        // Named precisely, so the editor can point at the crossing.
        let first = &result.self_intersections[0];
        assert!((first.at[0] - 5.0).abs() < 1e-9 && (first.at[1] - 5.0).abs() < 1e-9);
    }

    #[test]
    fn ordinary_geometry_reports_no_crossings() {
        let result = measure_layer_inner(&square(10.0, 5.0)).expect("measure");
        assert!(result.self_intersections.is_empty());
    }

    #[test]
    fn measuring_produces_canonical_path_data() {
        let result = measure_layer_inner(&square(10.0, 5.0)).expect("measure");
        assert_eq!(result.path_data.len(), 1);
        assert_eq!(
            result.path_data[0],
            "M 0.0000 0.0000 L 10.0000 0.0000 L 10.0000 5.0000 L 0.0000 5.0000 Z"
        );
    }

    #[test]
    fn a_null_coordinate_is_rejected_rather_than_propagated() {
        let bad =
            r#"{"shapes":[{"outer":{"points":[[null,0.0],[1.0,1.0]],"closed":true},"holes":[]}]}"#;
        let err = measure_layer_inner(bad).expect_err("must not be accepted");
        assert!(err.message.contains("cannot parse"), "{}", err.message);
    }

    #[test]
    fn a_non_finite_coordinate_never_reaches_the_geometry() {
        // JSON has no NaN literal, so it arrives as null. The parser must reject
        // it rather than coercing, because a zero would silently relocate a point.
        let bad = r#"{"shapes":[{"outer":{"points":[[0.0,0.0],[1.0,1.0]],"closed":true},"holes":[]},
                   {"outer":{"points":[["NaN",0.0],[2.0,2.0]],"closed":true},"holes":[]}]}"#;
        assert!(measure_layer_inner(bad).is_err());
    }

    #[test]
    fn an_out_of_bounds_coordinate_is_rejected() {
        let bad = r#"{"shapes":[{"outer":{"points":[[0.0,0.0],[1.0e12,1.0]],"closed":true},"holes":[]}]}"#;
        let err = measure_layer_inner(bad).expect_err("must not be accepted");
        assert!(
            err.message.contains("100000") || err.message.contains("bound"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_short_centreline_is_rejected_by_widen() {
        let short = serde_json::json!([[0.0, 0.0]]).to_string();
        assert!(widen_centreline_inner(&short, 0.8).is_err());
    }

    #[test]
    fn widening_returns_the_band_geometry() {
        let line = serde_json::json!([[0.0, 0.0], [100.0, 0.0]]).to_string();
        let result = widen_centreline_inner(&line, 0.80).expect("widen");
        assert!((result.min_width_mm.expect("width") - 0.80).abs() < 1e-3);
    }

    #[test]
    fn the_hash_of_identical_geometry_matches_across_calls() {
        let a = content_hash(&square(10.0, 5.0)).expect("hash");
        let b = content_hash(&square(10.0, 5.0)).expect("hash");
        assert_eq!(a, b);
    }

    /// The browser entry point must refuse malformed geometry, not trap on it.
    ///
    /// Regression for ADR-0015. `extract_layers` deserialised geometry without
    /// checking it, so a contour carrying no points and a non-empty arc list reached
    /// `resolved_arcs`, where `points.len() - 1` underflowed. On the rebuilt wasm
    /// artifact that input produced `RuntimeError: unreachable` — a panic, which in
    /// wasm does not throw but kills the module, taking every later editor call with
    /// it. `measure_layer` had always validated; `validate_project` had not, which
    /// made the document path the one unguarded way in.
    #[test]
    fn a_layer_with_arcs_and_no_points_is_refused_rather_than_trapping() {
        let bad = serde_json::json!({
            "composition": { "layers": [{
                "index": 1,
                "name": "Broken",
                "geometry": { "shapes": [{
                    "outer": {
                        "points": [],
                        "closed": false,
                        "arcs": [{ "from": 0, "to": 1, "centre": {"x": 0.0, "y": 0.0},
                                   "radius": 5.0, "ccw": true }]
                    },
                    "holes": []
                }]}
            }]}
        });

        let err = extract_layers(&bad).expect_err("an arc with no points names nothing");
        assert!(
            err.message.contains("points") || err.message.contains("TooFewPoints"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_layer_whose_arc_has_no_radius_is_refused_with_a_reason() {
        let bad = serde_json::json!({
            "composition": { "layers": [{
                "index": 1,
                "name": "Broken",
                "geometry": { "shapes": [{
                    "outer": {
                        "points": [[0,0],[10,0],[10,10],[0,10]],
                        "closed": true,
                        "arcs": [{ "from": 0, "to": 1, "centre": {"x": 0.0, "y": 0.0},
                                   "radius": 0.0, "ccw": true }]
                    },
                    "holes": []
                }]}
            }]}
        });

        let err = extract_layers(&bad).expect_err("a zero radius is not a curve");
        assert!(err.message.contains("arc"), "{}", err.message);
    }

    #[test]
    fn a_project_without_a_composition_is_rejected() {
        let bare = serde_json::json!({ "schema_version": 1 });
        let err = extract_layers(&bare).expect_err("must not be accepted");
        assert!(
            err.message.contains("composition.layers"),
            "{}",
            err.message
        );
    }

    #[test]
    fn an_empty_layer_list_is_left_for_the_validator() {
        // Not an extraction error: whether a project with no layers is valid is
        // the validator's call, and duplicating that rule here would give two
        // answers to one question.
        let empty = serde_json::json!({ "composition": { "layers": [] } });
        assert!(extract_layers(&empty)
            .expect("extraction succeeds")
            .is_empty());
    }

    #[test]
    fn a_layer_without_inline_geometry_is_rejected_not_skipped() {
        // The browser kernel evaluates inline geometry only. Silently skipping a
        // referenced layer would make the editor report a clean project while
        // checking nothing.
        let project = serde_json::json!({
            "composition": { "layers": [
                { "index": 1, "name": "Base", "geometry_id": "geo_01" }
            ]}
        });
        let err = extract_layers(&project).expect_err("must not be accepted");
        assert!(err.message.contains("inline geometry"), "{}", err.message);
    }
}

#[cfg(test)]
mod browser_reach {
    //! Which of the fifteen checks actually reach the editor.
    //!
    //! This exists because a document once claimed four of them could not reach
    //! the browser at all. That claim was half wrong, and it was wrong in the
    //! direction that hides work: T-12 does reach the editor, and T-15 reaches it
    //! with only half its inputs. A coverage claim that is wrong in the optimistic
    //! direction is worse than no claim, because it closes a task that is not
    //! closed.

    use super::*;

    const MATERIAL: &str = include_str!("../../profiles/data/matte-vinyl-090.v4.json");
    const PLOTTER: &str = include_str!("../../profiles/data/knife-25mm.v2.json");

    /// A square of `side` mm with its lower-left corner at (`x`, `y`).
    fn square_at(x: f64, y: f64, side: f64) -> serde_json::Value {
        serde_json::json!({
            "outer": {
                "points": [[x, y], [x + side, y], [x + side, y + side], [x, y + side]],
                "closed": true
            },
            "holes": []
        })
    }

    fn project(layers: serde_json::Value) -> String {
        serde_json::json!({ "composition": { "layers": layers } }).to_string()
    }

    fn layer(index: u32, name: &str, square: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "index": index,
            "name": name,
            "geometry": { "shapes": [square] }
        })
    }

    fn validate(project_json: &str) -> ValidationResult {
        validate_project_inner(project_json, MATERIAL, PLOTTER).expect("the document validates")
    }

    #[test]
    fn two_crossing_layers_in_one_order_are_reported_to_the_editor() {
        // T-12 compares overlapping layers by application order. The browser
        // sends `application_order: None` for every layer, so both layers fall
        // back to their index — and two layers with the *same* index therefore
        // collide with nothing to resolve them. The check runs; this proves it.
        let overlapping = project(serde_json::json!([
            { "index": 1, "name": "Base", "geometry": { "shapes": [square_at(0.0, 0.0, 40.0)] } },
            { "index": 1, "name": "Detail", "geometry": { "shapes": [square_at(20.0, 20.0, 40.0)] } }
        ]));

        let result = validate(&overlapping);
        assert!(
            result.messages.iter().any(|m| m.contains("перекрыва")),
            "the crossing must reach the editor: {:?}",
            result.messages
        );
    }

    #[test]
    fn layers_with_different_orders_are_not_a_crossing_error() {
        let ordered = project(serde_json::json!([
            { "index": 1, "name": "Base", "geometry": { "shapes": [square_at(0.0, 0.0, 40.0)] } },
            { "index": 2, "name": "Detail", "geometry": { "shapes": [square_at(20.0, 20.0, 40.0)] } }
        ]));

        let result = validate(&ordered);
        assert!(
            !result.messages.iter().any(|m| m.contains("перекрыва")),
            "a defined order resolves the crossing: {:?}",
            result.messages
        );
    }

    #[test]
    fn registration_measures_the_plotter_alone_and_never_the_geometry() {
        // T-15 is `tool error + geometric spread`, and no registration anchor
        // travels through this path, so the geometric half is always zero. The
        // profiles put the tool error at 0.30 mm against a 0.35 mm budget: the
        // only way this can fail is the plotter, never the layers.
        let clean = project(serde_json::json!([
            layer(1, "Base", square_at(0.0, 0.0, 40.0)),
            layer(2, "Detail", square_at(20.0, 20.0, 40.0))
        ]));
        let result = validate(&clean);
        assert!(
            !result.messages.iter().any(|m| m.contains("не совместятся")),
            "the tool fits its own budget: {:?}",
            result.messages
        );

        // A plotter that cannot hold its own registration budget is reported,
        // which shows the rule is live rather than unreachable.
        let bad_plotter = PLOTTER.replace("0.30", "0.90");
        let report =
            validate_project_inner(&clean, MATERIAL, &bad_plotter).expect("valid profiles");
        assert!(
            report.messages.iter().any(|m| m.contains("не совместятся")),
            "the rule fires on the plotter budget: {:?}",
            report.messages
        );
    }

    #[test]
    fn a_tab_the_browser_placed_is_checked_against_the_material() {
        // BUG-013. A 0.2 mm neck against a 0.80 mm minimum: the frame around it is
        // 5 mm thick, so measuring the geometry alone would call the layer fine.
        // Placing the tab and recording it is what makes T-06 fire here.
        let repair = repair_island(3.0, 0.2, 30.0);
        assert_eq!(repair.bridges.len(), 1, "one island, one tab");
        assert!((repair.bridges[0].width_mm - 0.2).abs() < 1e-9);

        let result = validate(&project(serde_json::json!([layer_with_bridges(
            1, "Base", repair
        )])));
        assert!(
            result
                .messages
                .iter()
                .any(|m| m.contains("уже минимальной ширины")),
            "T-06 must reach the editor: {:?}",
            result.messages
        );
    }

    #[test]
    fn a_tab_placed_longer_than_the_material_allows_is_reported() {
        // The placement limit and the material limit are different numbers on
        // purpose: placing a 30 mm neck is allowed by the request, and the
        // validator then refuses it against the material's 25 mm. That is T-07
        // catching a real decision, not a repair that failed.
        let repair = repair_island(40.0, 1.4, 80.0);
        // The span is corner to corner, so it is longer than the gap itself.
        assert!(
            repair.bridges[0].length_mm > 25.0,
            "the tab must exceed the material limit to be worth reporting, got {} mm",
            repair.bridges[0].length_mm
        );

        let result = validate(&project(serde_json::json!([layer_with_bridges(
            1, "Base", repair
        )])));
        assert!(
            result.messages.iter().any(|m| m.contains("длиннее")),
            "T-07 must reach the editor: {:?}",
            result.messages
        );
    }

    #[test]
    fn a_tab_that_fits_the_material_reaches_no_complaint() {
        let repair = repair_island(3.0, 1.4, 12.0);
        let result = validate(&project(serde_json::json!([layer_with_bridges(
            1, "Base", repair
        )])));
        assert!(
            !result
                .messages
                .iter()
                .any(|m| m.contains("мост") || m.contains("длиннее")),
            "a good tab must not invent a problem: {:?}",
            result.messages
        );
    }

    #[test]
    fn a_declared_tab_cannot_make_a_bad_neck_pass() {
        // The dangerous direction of trusting a document's numbers: claim a wide
        // tab while the geometry is thin. The width rule on the geometry is
        // independent of the record, so the neck is caught anyway — and it is
        // caught by measurement only because BUG-015 was fixed; before the fix a
        // 0.2 mm tab in a 200 mm part measured as 33 mm and this document would
        // have passed on both counts.
        let mut overstated = repair_island(3.0, 0.2, 12.0);
        overstated.bridges[0].width_mm = 4.0;
        let result = validate(&project(serde_json::json!([layer_with_bridges(
            1, "Base", overstated
        )])));
        assert!(
            result.messages.iter().any(|m| m.contains("Слой 1")),
            "the geometry is still measured: {:?}",
            result.messages
        );
    }

    #[test]
    fn the_field_rule_falls_back_to_the_uncut_layer_in_the_browser() {
        // `extract_tiles` reads `nesting.layers[].tiles[].bbox`, a shape that no
        // schema defines, no fixture contains and no artifact produces — pinned
        // here so the behaviour is a stated fact rather than an accident. The
        // consequence is the safe direction: with no plan, the rule measures the
        // whole layer, so a layer that would nest fine is reported as too large.
        // That is an over-report, not a silent pass, which is why it did not
        // become a defect on its own; see BUG-014 and BUG-016.
        let too_wide = project(serde_json::json!([layer(
            1,
            "Base",
            square_at(0.0, 0.0, 400.0)
        )]));
        let result = validate(&too_wide);
        assert!(
            result.messages.iter().any(|m| m.contains("рабочее поле")),
            "a 400 mm layer on a 284 mm plotter must be reported: {:?}",
            result.messages
        );

        // A document that does carry the undocumented shape is honoured, so the
        // contract is visible in one place instead of implied in two.
        let nested = serde_json::json!({
            "composition": { "layers": [{
                "index": 1,
                "name": "Base",
                "geometry": { "shapes": [square_at(0.0, 0.0, 400.0)] }
            }]},
            "nesting": { "layers": [{
                "layer": 1,
                "tiles": [{ "bbox": [0.0, 0.0, 200.0, 200.0] }]
            }]}
        });
        let result = validate(&nested.to_string());
        assert!(
            !result.messages.iter().any(|m| m.contains("рабочее поле")),
            "a plan that fits silences the field rule: {:?}",
            result.messages
        );
    }

    #[test]
    fn a_bridge_record_without_a_position_is_refused() {
        // `at` is what makes a record traceable to a place on the film. A record
        // without it is not a record of anything, and defaulting it to the origin
        // would put a tab somewhere nobody cut.
        let broken = serde_json::json!({
            "composition": { "layers": [{
                "index": 1,
                "name": "Base",
                "geometry": { "shapes": [square_at(0.0, 0.0, 40.0)] },
                "bridges": [{ "width_mm": 1.2, "length_mm": 10.0 }]
            }]}
        });
        let err = extract_layers(&broken).expect_err("a record needs a position");
        assert!(err.message.contains("bridges"), "{}", err.message);
    }

    /// A frame with an island floating `gap_mm` away inside its hole: the shape
    /// the repair exists for.
    ///
    /// The gap is a parameter because it is also the tab length the repair
    /// produces — the span between the two attachment points — so a test can ask
    /// for a short tab and a long one without a second fixture.
    fn island_in_a_frame(gap_mm: f64) -> String {
        let outer = 200.0;
        let frame = serde_json::json!({
            "outer": {
                "points": [[0.0,0.0],[outer,0.0],[outer,outer],[0.0,outer]],
                "closed": true
            },
            "holes": [{
                "points": [
                    [gap_mm, gap_mm],
                    [gap_mm, outer - gap_mm],
                    [outer - gap_mm, outer - gap_mm],
                    [outer - gap_mm, gap_mm]
                ],
                "closed": true
            }]
        });
        let island = square_at(2.0 * gap_mm, 2.0 * gap_mm, 4.0);
        serde_json::json!({ "shapes": [frame, island] }).to_string()
    }

    fn repair_island(gap_mm: f64, tab_mm: f64, max_length_mm: f64) -> RepairResult {
        bridge_inner(&island_in_a_frame(gap_mm), tab_mm, max_length_mm)
            .expect("the island is reachable")
    }

    fn layer_with_bridges(index: u32, name: &str, repair: RepairResult) -> serde_json::Value {
        serde_json::json!({
            "index": index,
            "name": name,
            "geometry": { "shapes": repair.shapes.shapes },
            "bridges": repair.bridges
        })
    }
}
#[cfg(test)]
mod consistency_tests {
    //! The point of ADR-0013: one geometry implementation, two runtimes.
    //!
    //! A measurement that differs between the browser and the server is the worst
    //! class of bug in this system, because it appears only at submission time —
    //! the editor shows a clean project and the server then rejects it.

    use super::*;

    fn square(w: f64, h: f64) -> String {
        serde_json::json!({
            "shapes": [{
                "outer": { "points": [[0.0,0.0],[w,0.0],[w,h],[0.0,h]], "closed": true },
                "holes": []
            }]
        })
        .to_string()
    }

    #[test]
    fn the_math_mode_is_the_pinned_integer_one() {
        // Read through Debug so the assertion does not need a direct dependency
        // on the geometry library; the kernel already exposes the pinned choice.
        assert_eq!(format!("{:?}", mask_kernel::lib_math_mode()), "Integer");
    }

    #[test]
    fn path_data_from_the_browser_matches_path_data_from_the_server() {
        let result = measure_layer_inner(&square(10.0, 5.0)).expect("measure");
        let server = mask_kernel::to_path_data(&mask_kernel::Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 10.0, y: 0.0 },
                Point { x: 10.0, y: 5.0 },
                Point { x: 0.0, y: 5.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        });
        assert_eq!(result.path_data[0], server);
    }

    #[test]
    fn a_hash_computed_for_the_editor_matches_the_kernels_own() {
        let shapes: Shapes = serde_json::from_str(&square(7.0, 3.0)).expect("parse");
        let from_editor =
            content_hash(&serde_json::to_string(&shapes).expect("json")).expect("hash");
        let from_kernel = mask_kernel::ContentHash::of(&shapes);
        assert_eq!(from_editor, from_kernel.to_string());
    }
}

#[cfg(test)]
mod golden_export {
    //! Writes the expected browser-visible values, so the Node runtime check can
    //! compare against the Rust side instead of against itself.
    //!
    //! Generated by `cargo test -p mask-wasm golden -- --ignored --nocapture`.
    //! Committed, because a golden file nobody regenerates is just a constant.

    use super::*;

    fn square(w: f64, h: f64) -> String {
        serde_json::json!({
            "shapes": [{
                "outer": { "points": [[0.0,0.0],[w,0.0],[w,h],[0.0,h]], "closed": true },
                "holes": []
            }]
        })
        .to_string()
    }

    fn band(width: f64) -> Shapes {
        mask_kernel::widen(
            &[Point { x: 0.0, y: 0.0 }, Point { x: 100.0, y: 0.0 }],
            width,
        )
        .expect("band")
    }

    #[test]
    #[ignore = "regenerates scripts/golden/hashes.json"]
    fn write_golden() {
        let mut doc = serde_json::Map::new();

        doc.insert(
            "square_10x5".into(),
            serde_json::json!(mask_kernel::ContentHash::of(
                &serde_json::from_str::<Shapes>(&square(10.0, 5.0)).expect("parse")
            )
            .to_string()),
        );
        doc.insert(
            "square_7x3".into(),
            serde_json::json!(mask_kernel::ContentHash::of(
                &serde_json::from_str::<Shapes>(&square(7.0, 3.0)).expect("parse")
            )
            .to_string()),
        );
        doc.insert(
            "band_0_80".into(),
            serde_json::json!(mask_kernel::ContentHash::of(&band(0.80)).to_string()),
        );
        doc.insert(
            "band_0_42".into(),
            serde_json::json!(mask_kernel::ContentHash::of(&band(0.42)).to_string()),
        );

        // The band geometry travels with the hashes. Guessing it in JavaScript
        // would test a shape nobody produces: `widen` decides its own vertex
        // order and miter placement, and those are exactly the details a hash
        // depends on.
        doc.insert(
            "band_0_80_shapes".into(),
            serde_json::to_value(band(0.80)).expect("value"),
        );
        doc.insert(
            "band_0_42_shapes".into(),
            serde_json::to_value(band(0.42)).expect("value"),
        );

        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/golden/hashes.json");
        let json = serde_json::to_string_pretty(&serde_json::Value::Object(doc)).expect("json");
        std::fs::write(&path, format!("{json}\n")).expect("write golden");
        println!("wrote {}", path.display());
    }
}
