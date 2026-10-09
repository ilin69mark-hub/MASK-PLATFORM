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
use mask_nesting::{fixed_grid, needs_nesting, NestRequest};
use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::{CuttingProfile, RuleSet};
use mask_validate::{validate_with_registration, BridgeRecord, LayerInput, Rect4, Verdict};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use wasm_bindgen::prelude::*;

/// What JavaScript gets back about a layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerResult {
    pub area_mm2: f64,
    /// The narrowest thing that holds the layer together.
    ///
    /// The minimum of `min_width` and `narrowest_neck` per region, which is what
    /// the validator measures (`mask_validate::measure_layer`). It used to be
    /// `min_width` alone here, and that is a number the editor must not show: a
    /// 4 mm band with a 0.2 mm whisker measured 4.000 mm in the browser while the
    /// validator measured 0.2 mm. The editor's threshold line then reported the
    /// mask as acceptable and the station rejected it — the one disagreement
    /// ADR-0013 exists to prevent.
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
    /// `[min_x, min_y, width, height]`, the same four numbers and the same order
    /// the validator publishes and `VALIDATOR.md` documents.
    ///
    /// It used to start at zero unconditionally, which was wrong wherever the
    /// geometry did not: a region at x = 812…908 reported `[0, 0, 96, 72]`. A
    /// drawing consumer that trusts the first two numbers draws in the wrong
    /// place, and a viewport built from them shows nothing at all.
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
    /// Layers that were laid out before validation because they did not fit.
    ///
    /// Empty for the un-nested call, and the reason a verdict can differ between
    /// the two entry points: a layer that only fits once it is cut into tiles
    /// passes the field rule here and fails it there. A caller that compares the
    /// two must read this field rather than wonder why.
    pub nested_layers: Vec<u32>,
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
    to_js(
        &validate_project_inner(project_json, material_json, plotter_json, false)
            .map_err(into_js)?,
    )
}

/// Validate a project the way it will actually be printed: lay out every layer
/// that does not fit the field, then check the layout.
///
/// This is the browser's counterpart to `maskctl --nest`, and the reason T-15 can
/// see geometry at all. The registration check adds the plotter's own error to
/// the worst spread between where two layers start, and "where a layer starts"
/// exists only after a layout — so without this call the browser can measure the
/// plotter and nothing else. The layout is computed here from the same
/// `fixed_grid` the station uses, never read from the document.
///
/// Layers that did not need laying out are returned in `nested_layers` as well as
/// described in `messages`, so a verdict that improved because of a decision is
/// never mistaken for a verdict that was always true.
#[wasm_bindgen]
pub fn nest_and_validate_project(
    project_json: &str,
    material_json: &str,
    plotter_json: &str,
) -> Result<JsValue, JsValue> {
    to_js(
        &validate_project_inner(project_json, material_json, plotter_json, true)
            .map_err(into_js)?,
    )
}

/// Typed project validation, callable from Rust tests.
///
/// The split exists so a claim about which checks reach the browser can be
/// proved on the host instead of asserted. Both the crossings check (T-12) and
/// the registration check (T-15) live inside `validate_with_registration`, which
/// `validate_with_nesting` calls — so whether they reach the editor is a
/// question about the *inputs* this function builds, and that is testable.
///
/// `nest` decides whether those inputs include a layout. It is the browser's
/// counterpart to `maskctl --nest`: nesting is a choice somebody makes, so the
/// validator does not make it silently.
fn validate_project_inner(
    project_json: &str,
    material_json: &str,
    plotter_json: &str,
    nest: bool,
) -> Result<ValidationResult, JsError> {
    let material: MaterialProfile = parse(material_json, "material profile")?;
    let plotter: PlotterProfile = parse(plotter_json, "plotter profile")?;
    let value: serde_json::Value = parse(project_json, "project")?;

    let cutting = CuttingProfile::derive(&material, &plotter);
    let rules = RuleSet::from_profile(&cutting);
    let verified = cutting.is_physically_verified(&material, &plotter);

    let layers = extract_layers(&value)?;
    let layout = if nest {
        lay_out(&layers, &cutting)
    } else {
        Layout::empty()
    };

    let report = validate_with_registration(
        &layers,
        &cutting,
        &rules,
        verified,
        &layout.tiles,
        &layout.anchors,
    );

    let mut messages: Vec<String> = report
        .violations
        .iter()
        .map(|v| v.headline.clone())
        .collect();
    // Say what nesting changed. Without this the verdict would quietly improve
    // for a layer that only fits once it is cut into tiles, and the reader would
    // have no way to know the difference came from a decision nobody made.
    messages.extend(layout.notes);

    Ok(ValidationResult {
        verdict: verdict_name(report.verdict).to_string(),
        error_count: report.counts.error,
        warning_count: report.counts.warning,
        info_count: report.counts.info + layout.nested.len(),
        messages,
        limits_physically_verified: report.limits_physically_verified,
        nested_layers: layout.nested,
    })
}

/// What a layout contributes to one validation run.
///
/// Its own type rather than a four-tuple, because each part is easy to mix up
/// with another: tiles are per-layer rectangles, anchors are per-layer points,
/// and passing the wrong pair of them silently turns T-15 back into a plotter
/// check.
struct Layout {
    tiles: BTreeMap<u32, Vec<Rect4>>,
    anchors: BTreeMap<u32, Point>,
    /// Layers that were laid out, in the order they were visited.
    nested: Vec<u32>,
    /// One line per laid-out or refused layer, for the caller to show.
    notes: Vec<String>,
}

impl Layout {
    fn empty() -> Self {
        Self {
            tiles: BTreeMap::new(),
            anchors: BTreeMap::new(),
            nested: Vec::new(),
            notes: Vec::new(),
        }
    }
}

/// Lay out every layer that does not fit the field, and report where each lands.
///
/// This is what puts geometry into the registration check. T-15 compares the
/// plotter's own error against the worst spread between where two layers start,
/// and "where a layer starts" only exists once the layer has been laid out. So
/// the anchors are computed here, from the same `fixed_grid` the station uses —
/// not read from the document. A document that declared its own anchors would
/// make T-15 check a claim instead of a layout, which is the trap the bridge
/// records avoided too.
///
/// Returns the tile rectangles and anchors for the validator, the layers that
/// were laid out, and one note per layer for the caller to show.
fn lay_out(layers: &[LayerInput], cutting: &CuttingProfile) -> Layout {
    let request = NestRequest {
        field_width_mm: cutting.limits.max_tile_width_mm,
        field_length_mm: cutting.limits.max_tile_length_mm,
        separation_gap_mm: cutting.limits.gap_mm,
        kerf_mm: cutting.limits.kerf_mm,
    };

    let mut tiles: BTreeMap<u32, Vec<Rect4>> = BTreeMap::new();
    let mut anchors: BTreeMap<u32, Point> = BTreeMap::new();
    let mut nested: Vec<u32> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    for layer in layers {
        if !needs_nesting(&layer.geometry, request) {
            continue;
        }
        match fixed_grid(&layer.geometry, request) {
            Ok(plan) => {
                nested.push(layer.index);
                // Content rectangles, not the tile windows: the rule asks whether
                // the cut material fits, and the CLI passes the same numbers.
                tiles.insert(
                    layer.index,
                    plan.tiles
                        .iter()
                        .map(|t| {
                            let (_, _, w, h) = mask_kernel::bounding_box(&t.content);
                            [0.0, 0.0, w, h]
                        })
                        .collect(),
                );
                if let Some(first) = plan.tiles.first() {
                    anchors.insert(
                        layer.index,
                        Point {
                            x: first.marks.part_anchor.left.x_mm,
                            y: first.marks.part_anchor.left.y_mm,
                        },
                    );
                }
                // Deliberately not worded like the field rule's own message: a
                // note and a violation are the same list here, and a caller that
                // greps for text must be able to tell a decision from a defect.
                notes.push(format!(
                    "Слой {}: шире поля ({:.0} мм) — разбит на {} плит{}, регистрация проверена по раскладке.",
                    layer.index,
                    cutting.limits.max_tile_width_mm,
                    plan.summary.tile_count,
                    plural(plan.summary.tile_count, "у", "и", ""),
                ));
            }
            // A refused layout leaves the layer to be measured whole, which is
            // what the field rule does without a plan: it will say the layer is
            // too large, and that is the true state of a layer nobody could lay
            // out.
            Err(e) => notes.push(format!(
                "Слой {}: вложение отказано ({e}) — слой измеряется целиком.",
                layer.index
            )),
        }
    }

    Layout {
        tiles,
        anchors,
        nested,
        notes,
    }
}

/// Russian plural for a tile count: 1 плитка, 2 плитки, 5 плиток.
fn plural(n: usize, one: &str, few: &str, many: &str) -> String {
    match n % 10 {
        1 if n % 100 != 11 => one.to_string(),
        2..=4 if !(12..=14).contains(&(n % 100)) => few.to_string(),
        _ => many.to_string(),
    }
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

    let (min_x, min_y, w, h) = mask_kernel::bounding_box(&shapes);

    // Both widths, per region, then the minimum across regions — the same
    // reduction the validator performs. `min_width` alone cannot see a thin
    // appendage: a 4 mm band with a 0.2 mm whisker reports 4.000 mm, because the
    // band keeps the figure intact long after the whisker has closed.
    // `narrowest_neck` counts components and sees it, which is why the validator
    // takes the smaller of the two and why the browser has to as well.
    //
    // Memoised per region, and keyed by the region's own content hash rather than
    // by its index: a drag moves one vertex, the rest of the layer has not moved,
    // and the two measurements above are 234 ms of a 242 ms frame at 2 000
    // regions. `kernel::region_cache` carries the reasoning and the measurement.
    //
    // It lives in the kernel rather than in this wrapper on purpose. A cache here
    // would leave the browser counting widths one way and the station another,
    // and the two would agree only by construction. One memo, one answer, both
    // runtimes.
    let min_width_mm = shapes
        .shapes
        .iter()
        .filter_map(mask_kernel::region_cache::region_width)
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
        bbox: [min_x, min_y, w, h],
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

/// Grow every region that is narrower than `target_width_mm`.
///
/// `widen_centreline` strokes a path to a width; this widens a *shape*, which is
/// what the editor's widen tool needs: the layer on screen is already a region,
/// not a centreline. Both walls move outward by half the shortfall, so a region
/// measured at 0.2 mm with a 0.8 mm target grows by 0.3 mm per side.
///
/// A region already at or above the target is left alone rather than shrunk to
/// it. The tool is a repair, and narrowing a mask that is already manufacturable
/// would make it worse — the measured width is the floor, the target is only the
/// goal.
///
/// The result is measured here rather than returned bare, for the same reason as
/// `move_vertex`: the number the editor shows must belong to the geometry it will
/// validate, not to what it last remembered.
#[wasm_bindgen]
pub fn widen_shape_set(shapes_json: &str, target_width_mm: f64) -> Result<JsValue, JsValue> {
    to_js(&widen_shape_set_inner(shapes_json, target_width_mm).map_err(into_js)?)
}

/// Typed widening of a shape set, callable from Rust tests.
fn widen_shape_set_inner(shapes_json: &str, target_width_mm: f64) -> Result<EditResult, JsError> {
    if !target_width_mm.is_finite() || target_width_mm <= 0.0 {
        return Err(JsError::new(
            "widen: target width must be positive and finite",
        ));
    }
    let shapes: Shapes = parse(shapes_json, "shapes")?;
    shapes
        .validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| JsError::new(e.to_string()))?;

    let mut widened = Vec::with_capacity(shapes.shapes.len());
    for (index, shape) in shapes.shapes.iter().enumerate() {
        // Same two measurements `measure_layer_inner` reduces over, so the
        // shortfall this repairs is the one the validator would have complained
        // about. Offsetting by `min_width` alone would leave a whisker untouched:
        // a whisker is held by a neck, and `min_width` cannot see it.
        let one = Shapes::single(shape.clone());
        let measured: Option<f64> = match (
            mask_kernel::min_width(&one),
            mask_kernel::narrowest_neck(&one),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let delta_mm = match measured {
            Some(m) if m < target_width_mm => (target_width_mm - m) / 2.0,
            // Already wide enough, or unmeasurable: returned unchanged, because
            // inventing a delta for a region nothing measured would reshape a
            // figure on a number that does not exist.
            _ => {
                widened.push(shape.clone());
                continue;
            }
        };
        let grown = mask_kernel::offset_shape(&one, delta_mm)
            .map_err(|e| JsError::new(format!("widen: region {index} could not be grown: {e}")))?;
        widened.push(grown.shapes.into_iter().next().ok_or_else(|| {
            JsError::new(format!(
                "widen: region {index} disappeared at +{delta_mm} mm"
            ))
        })?);
    }

    let widened = mask_kernel::canonicalise_shapes(&Shapes { shapes: widened });
    widened
        .validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| JsError::new(format!("widen: {e}")))?;

    let measured = measure_layer_inner(&to_string(&widened)?)?;
    Ok(EditResult {
        shapes: widened,
        measured,
    })
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

/// One edit to a vertex, as the editor sends it.
///
/// Indices rather than a copy of the geometry: the editor already holds the shape
/// set it drew, and sending the whole thing back to move one point would make
/// every keystroke a full serialisation of the layer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VertexEdit {
    /// Index into `Shapes::shapes`.
    pub shape: usize,
    /// 0 for the outer contour, 1.. for its holes, in `Shape::contours()` order.
    pub contour: usize,
    /// Index into the contour's points.
    pub vertex: usize,
    pub x_mm: f64,
    pub y_mm: f64,
}

/// What one edit produced: the geometry to keep, and its measurement.
///
/// Both, because the editor needs each for a different thing. The canonical
/// `shapes` go back into state so the next edit addresses the same vertex indices
/// — rebuilding the layer from `path_data` instead would renumber every contour,
/// and a hit-test would then start grabbing the wrong vertex. The measurement
/// comes back with it so the numbers on screen belong to the geometry that will be
/// validated, rather than to whatever the editor last remembered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditResult {
    pub shapes: Shapes,
    pub measured: LayerResult,
}

/// Move one vertex and return the geometry to draw and to measure.
///
/// The edit is applied inside the kernel and nowhere else. That is the whole point
/// of routing it here rather than assigning into a TypeScript array: the canonical
/// grid (ADR-0004) is the kernel's, the bounds are the kernel's, and a move that
/// collapses a contour below three points is refused by the same code that would
/// have refused it on the way in through `validate_strict`. An editor that edited
/// coordinates itself would produce geometry the station's canonical hash could
/// not reproduce.
///
/// The result is measured rather than merely returned, so the editor gets the
/// numbers for the shape it just changed in the same call — the number shown must
/// be the number of the geometry that will be validated.
#[wasm_bindgen]
pub fn move_vertex(shapes_json: &str, edit: JsValue) -> Result<JsValue, JsValue> {
    let edit: VertexEdit = serde_wasm_bindgen::from_value(edit)
        .map_err(|e| into_js(JsError::new(format!("edit: {e}"))))?;
    to_js(&move_vertex_inner(shapes_json, edit).map_err(into_js)?)
}

/// Typed vertex move, callable from Rust tests.
///
/// Takes the parsed edit rather than a `JsValue`, because `serde_wasm_bindgen`
/// cannot run off the wasm target, and a write path with no host test is a write
/// path nobody checks. The split is the same one `validate_project_inner` has.
fn move_vertex_inner(shapes_json: &str, edit: VertexEdit) -> Result<EditResult, JsError> {
    let mut shapes: Shapes = parse(shapes_json, "shapes")?;

    if edit.shape >= shapes.shapes.len() {
        return Err(JsError::new(format!(
            "edit: shape {} does not exist; the layer has {}",
            edit.shape,
            shapes.shapes.len()
        )));
    }

    // Contour 0 is the outer contour and contour n is the n-th hole, which is the
    // order `Shape::contours()` yields and the order the editor numbers them in.
    {
        let shape = &mut shapes.shapes[edit.shape];
        let contours = 1 + shape.holes.len();
        if edit.contour >= contours {
            return Err(JsError::new(format!(
                "edit: contour {} does not exist; shape {} has {}",
                edit.contour, edit.shape, contours
            )));
        }
        // Read before the mutable borrow: whether the contour is closed decides if
        // the last point is a neighbour of the first.
        let closed = match edit.contour {
            0 => shape.outer.closed,
            n => shape.holes[n - 1].closed,
        };
        let points = match edit.contour {
            0 => &mut shape.outer.points,
            n => &mut shape.holes[n - 1].points,
        };
        if edit.vertex >= points.len() {
            return Err(JsError::new(format!(
                "edit: vertex {} does not exist; the contour has {}",
                edit.vertex,
                points.len()
            )));
        }
        // Rejected here rather than left to `validate_strict`, because the reason is
        // worth naming and because the alternative is silent data loss: a move that
        // duplicates a neighbour is the normal way an editor produces a
        // zero-length edge, and `canonicalise` answers that by dropping the
        // duplicate. The user would watch a vertex vanish with no message at all.
        //
        // Every neighbour is checked, not just the next one. The previous is where
        // a drag lands most often, since it approaches the vertex from that side,
        // and on a closed contour the last point is also a neighbour of the first.
        let len = points.len();
        let mut neighbours: Vec<usize> = Vec::with_capacity(2);
        if edit.vertex > 0 {
            neighbours.push(edit.vertex - 1);
        }
        if edit.vertex + 1 < len {
            neighbours.push(edit.vertex + 1);
        }
        if closed && len > 2 && edit.vertex == 0 {
            neighbours.push(len - 1);
        }

        let target = quantised(&edit);
        for n in &neighbours {
            let Some(point) = points.get(*n) else {
                continue;
            };
            let neighbour = mask_kernel::canonical::quantise_point(Point {
                x: point.x,
                y: point.y,
            });
            if neighbour == target {
                return Err(JsError::new(format!(
                    "edit: vertex {} would land on vertex {n}; move it somewhere else",
                    edit.vertex
                )));
            }
        }
        points[edit.vertex] = to_point(edit.x_mm, edit.y_mm)?;
    }

    // Canonicalise before validating, so the result is the geometry the kernel
    // would have produced from the same intent on the server.
    let shapes = mask_kernel::canonicalise_shapes(&shapes);
    shapes
        .validate_strict(mask_kernel::MAX_COORDINATE_MM)
        .map_err(|e| JsError::new(format!("edit: {e}")))?;

    let measured = measure_layer_inner(
        &serde_json::to_string(&shapes).map_err(|e| JsError::new(e.to_string()))?,
    )?;
    Ok(EditResult { shapes, measured })
}

fn to_point(x: f64, y: f64) -> Result<Point, JsError> {
    Point::new(x, y, mask_kernel::MAX_COORDINATE_MM).map_err(|e| JsError::new(format!("edit: {e}")))
}

fn quantised(edit: &VertexEdit) -> Point {
    mask_kernel::canonical::quantise_point(Point {
        x: edit.x_mm,
        y: edit.y_mm,
    })
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
        validate_project_inner(project_json, MATERIAL, PLOTTER, false)
            .expect("the document validates")
    }

    fn validate_nested(project_json: &str) -> ValidationResult {
        validate_project_inner(project_json, MATERIAL, PLOTTER, true)
            .expect("the document validates")
    }

    /// A band too long for the working field, so nesting has something to do.
    ///
    /// The 284 mm field of `knife-25mm` is the limit, so a 900 mm band needs
    /// four tiles and lands with its first one at a known place.
    fn long_band(x: f64, y: f64) -> serde_json::Value {
        serde_json::json!({
            "outer": {
                "points": [[x, y], [x + 900.0, y], [x + 900.0, y + 60.0], [x, y + 60.0]],
                "closed": true
            },
            "holes": []
        })
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
    fn registration_measures_the_plotter_alone_without_a_layout() {
        // T-15 is `tool error + geometric spread`, and this call does not lay the
        // project out, so no anchor exists and the geometric half is zero. The
        // profiles put the tool error at 0.30 mm against a 0.35 mm budget: the
        // only way this can fail is the plotter, never the layers. The laid-out
        // path is `registration_sees_the_geometry_once_the_project_is_laid_out`.
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
            validate_project_inner(&clean, MATERIAL, &bad_plotter, false).expect("valid profiles");
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
    fn the_field_rule_measures_the_whole_layer_when_nobody_asked_for_a_layout() {
        // Without a layout the field rule falls back to the uncut layer, which
        // over-reports: a layer that nests fine is still reported as too large.
        // That direction is safe, and it is the price of not deciding on the
        // caller's behalf whether nesting happens.
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
        assert!(
            result.nested_layers.is_empty(),
            "and nothing claims to have been laid out"
        );
    }

    #[test]
    fn registration_sees_the_geometry_once_the_project_is_laid_out() {
        // BUG-014, and the question asked directly: does T-15 see the geometry?
        //
        // Two layers, each longer than the field, placed 400 mm apart. Nested,
        // they start in different places, so the bench worker must register one
        // against the other; the plotter's own 0.30 mm fits the 0.35 mm budget on
        // its own. Without a layout there is nothing to register, so the rule
        // measures the plotter alone and stays quiet.
        let apart = project(serde_json::json!([
            layer(1, "Base", long_band(0.0, 0.0)),
            layer(2, "Detail", long_band(0.0, 400.0))
        ]));

        let without_layout = validate(&apart);
        assert!(
            !without_layout
                .messages
                .iter()
                .any(|m| m.contains("не совместятся")),
            "without a layout there is no registration error to find: {:?}",
            without_layout.messages
        );

        let laid_out = validate_nested(&apart);
        assert_eq!(laid_out.nested_layers, vec![1, 2], "both layers are nested");
        assert!(
            laid_out
                .messages
                .iter()
                .any(|m| m.contains("не совместятся")),
            "T-15 must read the layout, not only the plotter: {:?}",
            laid_out.messages
        );
    }

    #[test]
    fn layers_that_land_in_the_same_place_register_nothing() {
        // The other half: a rule that fires for every nested project is as useless
        // as one that never fires. Identical geometry lands identically, so there
        // is no spread to absorb and T-15 stays quiet even though both layers
        // were laid out.
        let together = project(serde_json::json!([
            layer(1, "Base", long_band(0.0, 0.0)),
            layer(2, "Detail", long_band(0.0, 0.0))
        ]));

        let result = validate_nested(&together);
        assert_eq!(result.nested_layers.len(), 2);
        assert!(
            !result.messages.iter().any(|m| m.contains("не совместятся")),
            "two layers at one origin need no registration: {:?}",
            result.messages
        );
    }

    #[test]
    fn a_single_nested_layer_has_nothing_to_register_against() {
        // Registration is a disagreement between layers. One layer laid out on a
        // sheet is aligned to itself, and reporting an error would be a defect
        // invented by the rule.
        let single = project(serde_json::json!([layer(1, "Base", long_band(0.0, 0.0))]));
        let result = validate_nested(&single);
        assert_eq!(result.nested_layers, vec![1]);
        assert!(
            !result.messages.iter().any(|m| m.contains("не совместятся")),
            "one layer cannot disagree with itself: {:?}",
            result.messages
        );
    }

    #[test]
    fn a_layout_that_succeeds_is_reported_rather_than_silently_better() {
        // The verdict improves for a layer that only fits once it is cut, which is
        // a decision somebody made. Both the layer list and a message say so, or
        // a caller comparing the two verdicts would have no way to tell why they
        // differ.
        let wide = project(serde_json::json!([layer(
            1,
            "Base",
            square_at(0.0, 0.0, 400.0)
        )]));

        let without_layout = validate(&wide);
        assert!(
            without_layout
                .messages
                .iter()
                .any(|m| m.contains("рабочее поле")),
            "an un-nested 400 mm layer does not fit: {:?}",
            without_layout.messages
        );

        let laid_out = validate_nested(&wide);
        assert_eq!(laid_out.nested_layers, vec![1]);
        assert!(
            !laid_out.messages.iter().any(|m| m.contains("рабочее поле")),
            "a laid-out layer fits its tiles: {:?}",
            laid_out.messages
        );
        assert!(
            laid_out
                .messages
                .iter()
                .any(|m| m.contains("шире поля") && m.contains("разбит на")),
            "and the caller is told what happened: {:?}",
            laid_out.messages
        );
    }

    #[test]
    fn a_document_cannot_declare_its_own_layout() {
        // The trap the bridge records avoided, closed here as well: a document
        // that claimed its tiles used to be read by `extract_tiles`, whose shape
        // no schema defined and no artefact produced. The layout is computed now,
        // so a claim is ignored rather than believed.
        let claiming = serde_json::json!({
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

        let result = validate(&claiming.to_string());
        assert!(
            result.messages.iter().any(|m| m.contains("рабочее поле")),
            "a claimed layout is not a layout: {:?}",
            result.messages
        );
        assert!(
            result.nested_layers.is_empty(),
            "and nothing claims to have been laid out"
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

    const MATERIAL: &str = include_str!("../../profiles/data/matte-vinyl-090.v4.json");
    const PLOTTER: &str = include_str!("../../profiles/data/knife-25mm.v2.json");

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

    /// A band 4 mm wide carrying a 0.2 mm whisker: a thin *appendage*, which
    /// eroding the band deletes without changing the piece count.
    ///
    /// This is the shape both width measures are documented to miss. It is kept
    /// here to pin that the browser misses it in exactly the same way the
    /// validator does — two runtimes blind together is not a disagreement, and a
    /// test that only asserted the browser's own answer would hide one.
    fn band_with_whisker() -> String {
        serde_json::json!({
            "shapes": [{
                "outer": {
                    "points": [
                        [0.0, 0.0], [40.0, 0.0], [40.0, 4.0],
                        [20.1, 4.0], [20.0, 4.0], [20.0, 20.0],
                        [19.9, 20.0], [19.9, 4.0], [0.0, 4.0]
                    ],
                    "closed": true
                },
                "holes": []
            }]
        })
        .to_string()
    }

    /// A frame, an island inside its hole, and a 0.2 mm tab joining them.
    ///
    /// The tab is a real neck — it is the only place the piece comes apart — and
    /// this is the geometry BUG-015 was about.
    fn frame_island_and_tab() -> String {
        let frame = serde_json::json!({
            "outer": {
                "points": [[0.0,0.0],[20.0,0.0],[20.0,20.0],[0.0,20.0]],
                "closed": true
            },
            "holes": [{
                "points": [[5.0,5.0],[5.0,15.0],[15.0,15.0],[15.0,5.0]],
                "closed": true
            }]
        });
        let island = serde_json::json!({
            "outer": {
                "points": [[8.0,8.0],[12.0,8.0],[12.0,12.0],[8.0,12.0]],
                "closed": true
            },
            "holes": []
        });
        let shapes: Shapes =
            serde_json::from_value(serde_json::json!({ "shapes": [frame, island] }))
                .expect("parse");
        let (joined, _) = mask_kernel::bridge::bridge_unanchored(
            &shapes,
            mask_kernel::bridge::BridgeLimits {
                width_mm: 0.2,
                max_length_mm: 25.0,
            },
        )
        .expect("bridge");
        serde_json::to_string(&joined).expect("json")
    }

    /// What the validator publishes for the same geometry.
    ///
    /// Not a recomputation: the validator's own `report.layers[0].min_width_mm`,
    /// which is the number a submitted project is judged by.
    fn validator_width(shapes_json: &str) -> f64 {
        let geometry: Shapes = serde_json::from_str(shapes_json).expect("parse");
        let material: MaterialProfile = serde_json::from_str(MATERIAL).expect("material");
        let plotter: PlotterProfile = serde_json::from_str(PLOTTER).expect("plotter");
        let cutting = CuttingProfile::derive(&material, &plotter);
        let rules = RuleSet::from_profile(&cutting);
        let report = mask_validate::validate(
            &[LayerInput {
                index: 1,
                name: "consistency".into(),
                geometry,
                bridges: Vec::new(),
                application_order: None,
            }],
            &cutting,
            &rules,
            false,
        );
        report.layers[0]
            .min_width_mm
            .expect("the validator publishes a width for a band")
    }

    #[test]
    fn the_browser_reports_the_narrowest_neck_the_validator_measures() {
        // BUG-019. The editor's threshold line compares this number against the
        // material minimum, so a number that disagrees with the validator turns
        // the editor into a machine that certifies masks the station rejects.
        let geometry = frame_island_and_tab();

        let browser = measure_layer_inner(&geometry)
            .expect("measure")
            .min_width_mm
            .expect("a measurement");
        let validator = validator_width(&geometry);

        assert!(
            (browser - 0.2).abs() < 0.05,
            "the tab is 0.2 mm and must be the answer, got {browser:.4}"
        );
        assert!(
            (browser - validator).abs() < 1e-3,
            "the browser said {browser:.4} and the validator {validator:.4}"
        );
    }

    #[test]
    fn a_thin_appendage_is_missed_by_both_runtimes_alike() {
        // The honest counterweight to the test above. A whisker on a wide band is
        // invisible to `min_width` and to `narrowest_neck`, and now to the browser
        // too — which is only correct because the validator misses it the same
        // way. If the validator ever starts seeing it, this test fails and says so.
        let geometry = band_with_whisker();

        let browser = measure_layer_inner(&geometry)
            .expect("measure")
            .min_width_mm
            .expect("a measurement");
        let validator = validator_width(&geometry);

        assert!(
            (browser - 4.0).abs() < 0.01,
            "the documented blind spot is the band's own width, got {browser:.4}"
        );
        assert!(
            (browser - validator).abs() < 1e-3,
            "agreement matters more than the number: browser {browser:.4}, validator {validator:.4}"
        );
    }

    #[test]
    fn a_plain_band_still_measures_as_its_own_width() {
        // `narrowest_neck` is a component count, and a band has no neck. Reporting
        // the tab above must not start reporting a neck where there is none.
        let result = measure_layer_inner(&square(40.0, 4.0)).expect("measure");
        let reported = result.min_width_mm.expect("a measurement");
        assert!(
            (reported - 4.0).abs() < 0.01,
            "a 4 mm band measures 4 mm, got {reported:.4}"
        );
    }

    #[test]
    fn the_bounding_box_starts_where_the_geometry_starts() {
        // BUG-018. A viewport is built from the first two numbers, so a region
        // away from the origin has to report where it actually is — a region at
        // x = 812…908 reporting `[0, 0, 96, 72]` puts the drawing 812 mm off the
        // visible area.
        let away = serde_json::json!({
            "shapes": [{
                "outer": {
                    "points": [[812.0, 640.0], [908.0, 640.0], [908.0, 712.0], [812.0, 712.0]],
                    "closed": true
                },
                "holes": []
            }]
        })
        .to_string();

        let result = measure_layer_inner(&away).expect("measure");
        let [min_x, min_y, w, h] = result.bbox;
        assert!(
            (min_x - 812.0).abs() < 1e-9 && (min_y - 640.0).abs() < 1e-9,
            "the origin must survive, got [{min_x}, {min_y}, {w}, {h}]"
        );
        assert!((w - 96.0).abs() < 1e-9 && (h - 72.0).abs() < 1e-9);
    }

    #[test]
    fn the_bounding_box_of_a_region_at_the_origin_is_unchanged() {
        let result = measure_layer_inner(&square(10.0, 5.0)).expect("measure");
        assert_eq!(result.bbox, [0.0, 0.0, 10.0, 5.0]);
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
mod editing {
    //! The editor's write path: a vertex moves, and everything downstream of that
    //! — canonical form, bounds, degenerate contours — is the kernel's decision
    //! rather than TypeScript's.

    use super::*;
    use serde_json::json;

    fn square_json() -> String {
        json!({
            "shapes": [{
                "outer": { "points": [[0.0,0.0],[10.0,0.0],[10.0,5.0],[0.0,5.0]], "closed": true },
                "holes": []
            }]
        })
        .to_string()
    }

    /// The path data after a move, which is what the editor draws.
    fn path_after(
        shapes_json: &str,
        shape: usize,
        contour: usize,
        vertex: usize,
        x: f64,
        y: f64,
    ) -> Result<String, JsError> {
        let result = move_vertex_inner(
            shapes_json,
            VertexEdit {
                shape,
                contour,
                vertex,
                x_mm: x,
                y_mm: y,
            },
        )?;
        Ok(result
            .measured
            .path_data
            .first()
            .cloned()
            .expect("the shape still has a contour"))
    }

    #[test]
    fn a_vertex_moves_and_the_result_is_measured() {
        // The number shown must belong to the shape that was just changed, so the
        // move returns a measurement rather than geometry the editor must re-measure.
        let moved = path_after(&square_json(), 0, 0, 1, 12.0, 0.0).expect("move");
        assert!(
            moved.starts_with("M 0.0000 0.0000 L 12.0000 0.0000"),
            "the path must start from the moved vertex, got {moved}"
        );

        let result = move_vertex_inner(
            &square_json(),
            VertexEdit {
                shape: 0,
                contour: 0,
                vertex: 1,
                x_mm: 12.0,
                y_mm: 0.0,
            },
        )
        .expect("move");
        assert!(
            (result.measured.bbox[2] - 12.0).abs() < 1e-9,
            "the bounding box must follow the move, got {}",
            result.measured.bbox[2]
        );
    }

    #[test]
    fn a_move_lands_on_the_canonical_grid_not_where_the_mouse_was() {
        // ADR-0004: coordinates live on a 1e-4 mm grid. A pointer reports 12.000049
        // and the stored coordinate is 12.0000, because the geometry is what gets
        // hashed and a coordinate off the grid would produce a hash the server
        // cannot reproduce.
        let moved = path_after(&square_json(), 0, 0, 1, 12.000049, 0.0).expect("move");
        assert!(
            moved.contains("L 12.0000 0.0000"),
            "off-grid coordinates must be quantised, got {moved}"
        );
    }

    #[test]
    fn a_vertex_cannot_be_dropped_onto_its_neighbour() {
        // The ordinary way an editor produces a zero-length edge. Refused by name,
        // because "contour needs at least 3 distinct points" describes the
        // consequence and not the mistake.
        let err = path_after(&square_json(), 0, 0, 1, 0.0, 0.0).expect_err("a duplicate point");
        assert!(
            err.message.contains("would land on vertex"),
            "{}",
            err.message
        );
    }

    #[test]
    fn an_index_outside_the_geometry_is_refused_with_the_size() {
        for (shape, contour, vertex, expected) in [
            (7usize, 0usize, 0usize, "shape 7 does not exist"),
            (0, 3, 0, "contour 3 does not exist"),
            (0, 0, 9, "vertex 9 does not exist"),
        ] {
            let err = path_after(&square_json(), shape, contour, vertex, 1.0, 1.0)
                .expect_err("an index that does not exist");
            assert!(
                err.message.contains(expected),
                "expected {expected:?}, got {:?}",
                err.message
            );
        }
    }

    #[test]
    fn a_move_outside_the_coordinate_bound_is_refused_not_clamped() {
        // Clamping would produce a vertex the user did not place, in a mask that
        // gets cut. The move fails and the layer stays as it was.
        let err = path_after(&square_json(), 0, 0, 1, 1e9, 0.0).expect_err("out of bounds");
        assert!(
            err.message.contains("edit:"),
            "the refusal must name the edit, got {:?}",
            err.message
        );
    }

    #[test]
    fn a_move_that_collapses_the_contour_is_refused() {
        // Three points, and the move takes two of them onto each other.
        let triangle = json!({
            "shapes": [{
                "outer": { "points": [[0.0,0.0],[8.0,0.0],[0.0,8.0]], "closed": true },
                "holes": []
            }]
        })
        .to_string();
        let err = path_after(&triangle, 0, 0, 1, 0.0, 0.0).expect_err("a collapsed contour");
        assert!(
            err.message.contains("would land on vertex") || err.message.contains("edit:"),
            "{}",
            err.message
        );
    }

    #[test]
    fn a_hole_vertex_moves_too() {
        // The editor numbers holes after the outer contour, so contour 1 is the
        // first hole. Getting that index wrong would move the wrong boundary, and
        // it would look plausible on screen.
        let with_hole = json!({
            "shapes": [{
                "outer": { "points": [[0.0,0.0],[20.0,0.0],[20.0,20.0],[0.0,20.0]], "closed": true },
                "holes": [{
                    "points": [[5.0,5.0],[5.0,15.0],[15.0,15.0],[15.0,5.0]],
                    "closed": true
                }]
            }]
        })
        .to_string();

        let result = move_vertex_inner(
            &with_hole,
            VertexEdit {
                shape: 0,
                contour: 1,
                vertex: 1,
                x_mm: 5.0,
                y_mm: 18.0,
            },
        )
        .expect("move");
        assert_eq!(
            result.measured.path_data.len(),
            2,
            "outer contour and one hole"
        );
        assert!(
            result.measured.path_data[1].contains("L 5.0000 18.0000"),
            "the hole must be the thing that moved, got {}",
            result.measured.path_data[1]
        );
    }
}

#[cfg(test)]
mod cache_agreement {
    //! The memo must change the time a measurement takes and nothing else.
    //!
    //! `kernel::region_cache` makes the editor fast by remembering a region's width
    //! under the hash of that region's geometry. The whole design rests on one
    //! property: an answer from the cache is the answer the kernel would have
    //! computed. If that were not true, the editor would show a number the station
    //! does not agree with — which is ADR-0013's failure, reached by making the
    //! code faster instead of wrong.

    use super::*;

    fn band(width_mm: f64) -> String {
        let half = width_mm / 2.0;
        serde_json::json!({
            "shapes": [{
                "outer": {
                    "points": [[0.0, -half], [10.0, -half], [10.0, half], [0.0, half]],
                    "closed": true
                },
                "holes": []
            }]
        })
        .to_string()
    }

    fn width_of(json: &str) -> Option<f64> {
        measure_layer_inner(json).expect("measure").min_width_mm
    }

    #[test]
    fn a_cached_layer_reports_the_same_width_as_the_first_time() {
        let json = band(0.2);
        let first = width_of(&json).expect("a width");
        let second = width_of(&json).expect("a width");
        assert_eq!(first, second, "the second reading is the cached one");
        assert!((first - 0.2).abs() < 1e-2, "got {first}");
    }

    #[test]
    fn one_layer_does_not_answer_for_another() {
        // The realistic failure: the editor switches layers, and the second layer
        // reads the first one's number. A cache keyed by index would do exactly
        // this the moment two layers had the same number of regions.
        let narrow = band(0.2);
        let wide = band(3.0);
        assert!((width_of(&narrow).expect("a width") - 0.2).abs() < 1e-2);
        assert!(
            (width_of(&wide).expect("a width") - 3.0).abs() < 1e-2,
            "the wide band read the narrow one's width"
        );
        // And back again, which is what switching tabs does.
        assert!((width_of(&narrow).expect("a width") - 0.2).abs() < 1e-2);
    }

    #[test]
    fn a_moved_vertex_is_measured_afresh_rather_than_read_from_the_cache() {
        // The drag itself: the geometry changes, so the hash changes, so the old
        // entry cannot answer. Reading 0.2 mm for a band now 0.4 mm thick is the
        // number a user would see and would believe.
        //
        // An earlier version of this test moved two of the four vertices of a
        // 0.2 mm band by hand and asserted 0.4 mm. It measured 0.198 — correctly:
        // the figure it had built was not a 0.4 mm band, it was a skew quadrilateral
        // about as thick as the original. The geometry was wrong, not the cache.
        let before = band(0.2);
        let after = band(0.4);

        let first = width_of(&before).expect("a width");
        let second = width_of(&after).expect("a width");
        assert!((first - 0.2).abs() < 1e-2, "before: {first}");
        assert!(
            (second - 0.4).abs() < 1e-2,
            "the 0.4 mm band must read 0.4, got {second}"
        );
        assert!(
            (second - first).abs() > 0.1,
            "the two bands must not share a number: {first} and {second}"
        );
    }
}

#[cfg(test)]
mod repairs {
    //! The auto-fixes the editor offers: widen a narrow region, anchor an island.
    //!
    //! Both are the tools a user reaches for *instead of* fixing geometry by hand,
    //! so the tests here are about what they refuse as much as what they change.
    //! A repair that silently produced unmanufacturable geometry would be worse than
    //! no repair: the editor would certify a mask the station rejects.

    use super::*;
    use serde_json::json;

    /// A frame with a hole, and an island floating inside the hole.
    ///
    /// The island is not attached to the frame — it is a separate region inside
    /// the hole, which is what `bridge_unanchored` looks for.
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
        let island = json!({
            "outer": {
                "points": [
                    [2.0 * gap_mm, 2.0 * gap_mm],
                    [2.0 * gap_mm + 4.0, 2.0 * gap_mm],
                    [2.0 * gap_mm + 4.0, 2.0 * gap_mm + 4.0],
                    [2.0 * gap_mm, 2.0 * gap_mm + 4.0]
                ],
                "closed": true
            },
            "holes": []
        });
        serde_json::json!({ "shapes": [frame, island] }).to_string()
    }

    /// A band `width` mm thick and 100 mm long.
    fn band_json(width: f64) -> String {
        let half = width / 2.0;
        json!({
            "shapes": [{
                "outer": {
                    "points": [
                        [0.0, -half], [100.0, -half], [100.0, half], [0.0, half]
                    ],
                    "closed": true
                },
                "holes": []
            }]
        })
        .to_string()
    }

    #[test]
    fn widening_reaches_the_target_width() {
        let result = widen_shape_set_inner(&band_json(0.2), 0.8).expect("widen");
        let width = result.measured.min_width_mm.expect("a measurement");
        assert!(
            (width - 0.8).abs() < 1e-2,
            "the region should measure about 0.80 mm, got {width}"
        );
    }

    #[test]
    fn widening_grows_the_layer_in_both_directions() {
        // Both walls move: the offset is half the shortfall on each side, so the
        // band's height grows by the full difference. If only one wall moved, the
        // shape would slide instead of widen.
        let before = measure_layer_inner(&band_json(0.2)).expect("measure");
        let after = widen_shape_set_inner(&band_json(0.2), 0.8).expect("widen");
        let grew = after.measured.bbox[3] - before.bbox[3];
        assert!(
            (grew - 0.6).abs() < 1e-2,
            "0.2 mm to 0.8 mm should grow the band by 0.6 mm, got {grew}"
        );
        // The band must stay centred: the centre of the bbox must not move.
        let before_centre = before.bbox[1] + before.bbox[3] / 2.0;
        let after_centre = after.measured.bbox[1] + after.measured.bbox[3] / 2.0;
        assert!(
            (after_centre - before_centre).abs() < 1e-9,
            "the band must stay centred, but the centre moved by {}",
            after_centre - before_centre
        );
    }

    #[test]
    fn a_region_already_wide_enough_is_left_alone() {
        // The tool is a repair. Narrowing a region that already cuts to the
        // material would be the tool making things worse.
        let before = measure_layer_inner(&band_json(2.0)).expect("measure");
        let after = widen_shape_set_inner(&band_json(2.0), 0.8).expect("widen");
        assert_eq!(
            before.content_hash, after.measured.content_hash,
            "a region above the target must be returned unchanged, hash and all"
        );
    }

    #[test]
    fn widening_reaches_the_neck_not_only_the_band() {
        // A frame held to its island by a 0.2 mm tab: `min_width` sees the
        // 3 mm frame walls, so a tool built on `min_width` alone would report
        // success and leave the tab narrower than the material allows. This is
        // BUG-019 in tool form, built the same way the kernel builds it in
        // `narrowest_neck_finds_the_bridge_tab_that_min_width_missed`.
        //
        // The earlier version of this test drew the "neck" as a 0.2 mm step on
        // the boundary of a solid band and asserted it measured 0.2 mm. It
        // measured 4.0 mm, correctly: a step on the boundary is not a neck, and
        // nothing erodes through there. A neck is where the piece comes apart.
        let joined = bridge_inner(&island_in_a_frame(3.0), 0.2, 25.0).expect("bridge");
        let neck = to_string(&joined.shapes).expect("joined geometry");
        let before = measure_layer_inner(&neck).expect("measure");
        assert!(
            (before.min_width_mm.expect("a measurement") - 0.2).abs() < 0.05,
            "the tab measures 0.2 mm, got {}",
            before.min_width_mm.unwrap()
        );

        // Growing the whole region moves the tab's walls too, so the tab
        // widens rather than vanishing — and the tool acts on the number the
        // validator uses, not on the band around it.
        let after = widen_shape_set_inner(&neck, 0.8).expect("widen");
        assert!(
            after.measured.area_mm2 > before.area_mm2,
            "widening must grow the region"
        );
        assert!(
            (after.measured.min_width_mm.expect("a measurement") - 0.8).abs() < 0.1,
            "the tab should measure about 0.8 mm after widening, got {}",
            after.measured.min_width_mm.unwrap()
        );
    }

    #[test]
    fn a_target_that_is_not_a_width_is_refused() {
        for target in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let err = widen_shape_set_inner(&band_json(0.2), target)
                .expect_err("a target that is not a positive finite width");
            assert!(
                err.message.contains("positive and finite"),
                "expected the reason for {target}, got {:?}",
                err.message
            );
        }
    }

    #[test]
    fn bridging_returns_the_tabs_it_placed_and_the_validator_reads() {
        // The whole point of running this in the browser: the editor places tabs
        // and the validator checks the same records, so T-06 and T-07 have
        // something real to test rather than a declared number.
        //
        // The island is a 4×4 square inside a 200×200 frame with a 3 mm gap —
        // the same geometry the existing bridge tests use.
        let island = island_in_a_frame(3.0);

        let repaired = bridge_inner(&island, 1.2, 25.0).expect("bridge");
        assert!(
            !repaired.bridges.is_empty(),
            "a free island must be given at least one tab"
        );
        for record in &repaired.bridges {
            assert!(
                record.width_mm >= 1.2 - 1e-9,
                "a tab narrower than the material would fail T-06 on the spot: {}",
                record.width_mm
            );
            assert!(
                record.length_mm <= 25.0 + 1e-9,
                "a tab longer than the material would fail T-07: {}",
                record.length_mm
            );
        }
        assert!(
            repaired.shapes.len() < island_count(&island),
            "the tab joins the island to the frame: the region count must shrink"
        );
    }

    #[test]
    fn bridging_a_layer_with_nothing_to_anchor_reports_no_tabs() {
        // One region and nothing enclosed: no island to attach, and inventing a
        // tab would put steel where the user cut nothing.
        let single = band_json(4.0);
        let repaired = bridge_inner(&single, 1.2, 25.0).expect("bridge");
        assert!(repaired.bridges.is_empty(), "nothing to anchor");
    }

    #[test]
    fn bridging_that_cannot_reach_an_island_fails_rather_than_half_doing_it() {
        // All-or-nothing, matching `maskctl --bridge`: an editor that placed some
        // tabs and a station that placed none would disagree about one file.
        //
        // A tab width of zero cannot be cut, so no island can be anchored.
        // The kernel returns an error rather than silently placing nothing.
        let island = island_in_a_frame(3.0);
        let err = bridge_inner(&island, 0.0, 25.0).expect_err("an uncuttable tab");
        assert!(!err.message.is_empty(), "the refusal must carry a reason");
    }

    /// How many regions a JSON document declares.
    fn island_count(json_str: &str) -> usize {
        let shapes: Shapes = serde_json::from_str(json_str).expect("shapes");
        shapes.shapes.len()
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
