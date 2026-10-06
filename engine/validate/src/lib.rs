//! Manufacturing Validator.
//!
//! The only component permitted to declare a project manufacturable. Nothing
//! else — not AI, not an editor, not a generator — may produce a verdict
//! (P-01).
//!
//! Two rules govern this module:
//!
//! 1. It **never** panics. Every failure becomes a violation with a severity.
//! 2. Its messages are written for a person, with the number, the cause and the
//!    action (P-09). No error codes are shown in user-facing text.

use mask_kernel::{
    bounding_box, format_area, format_degrees, format_measurement, has_curves, intersection,
    meets_threshold, min_corner_radius, min_gap, min_width, narrowest_neck, open_cut_contours,
    self_intersections, shape_area, shapes_area, sharp_corners, shortest_edge_in,
    unanchored_islands, Contour, Point, Shape, Shapes,
};
use mask_rules::{CuttingProfile, RuleSet, Severity};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("geometry is invalid: {0}")]
    Geometry(String),
}

/// One problem found in the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub code: String,
    pub severity: Severity,
    pub layer_index: Option<u32>,
    pub measured: Option<f64>,
    pub required: Option<f64>,
    /// What the two numbers above are measured in.
    ///
    /// The renderer cannot assume millimetres: an angle check measured 18.9 and
    /// the report told the operator it was 18.9 mm. A number with the wrong unit
    /// is worse than no number, because it looks trustworthy.
    #[serde(default = "default_unit")]
    pub unit: Unit,
    pub count: usize,
    /// Short human-facing statement.
    pub headline: String,
    /// Why it matters physically.
    pub reason: String,
    /// What the user can do about it.
    pub actions: Vec<Action>,
}

/// The unit a violation's measurements are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// Millimetres. The default, because nearly every rule measures a length.
    #[default]
    Mm,
    /// Degrees.
    Deg,
    /// Square millimetres.
    Mm2,
    /// A plain count with no unit.
    Count,
}

fn default_unit() -> Unit {
    Unit::Mm
}

/// Russian plural forms for a counted noun.
///
/// "1 углов" is wrong in the first line the operator reads, and the whole
/// message is about a measurement they are meant to trust. Rust's `{}` cannot do
/// this, and hard-coding one form per violation is how "1 островков" appeared.
pub(crate) fn plural<'a>(count: usize, one: &'a str, few: &'a str, many: &'a str) -> &'a str {
    let n = count % 100;
    if (11..=14).contains(&n) {
        return many;
    }
    match n % 10 {
        1 => one,
        2..=4 => few,
        _ => many,
    }
}

/// A noun phrase with its count, correctly inflected.
pub(crate) fn counted(count: usize, one: &str, few: &str, many: &str) -> String {
    format!("{} {}", count, plural(count, one, few, many))
}

/// The participle that agrees with the count.
///
/// "1 островок не соединены" is the same defect as "1 углов": the number and the
/// word beside it contradict each other in the line the operator has to trust.
pub(crate) fn agreeing<'a>(count: usize, singular: &'a str, plural_form: &'a str) -> &'a str {
    let n = count % 100;
    if (11..=14).contains(&n) || n % 10 != 1 {
        plural_form
    } else {
        singular
    }
}

impl Unit {
    #[must_use]
    pub fn suffix(self) -> &'static str {
        match self {
            Unit::Mm => "мм",
            Unit::Deg => "°",
            Unit::Mm2 => "мм²",
            Unit::Count => "",
        }
    }
}

/// Something the reader can do about a violation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub primary: bool,
    /// Expected cost of taking this action, in fidelity.
    pub estimated_fidelity_loss: Option<f64>,
}

/// The overall outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Pass,
    PassWithWarnings,
    Fail,
    Fatal,
}

impl Verdict {
    /// Derive a verdict from the violations collected.
    ///
    /// `Fatal` wins over everything, then `error`, then `warning`. This is
    /// invariant I-5: a `Pass` can never coexist with an error.
    #[must_use]
    pub fn from_violations(violations: &[Violation]) -> Self {
        let has = |s: Severity| violations.iter().any(|v| v.severity == s);
        if has(Severity::Fatal) {
            Self::Fatal
        } else if has(Severity::Error) {
            Self::Fail
        } else if has(Severity::Warning) {
            Self::PassWithWarnings
        } else {
            Self::Pass
        }
    }

    #[must_use]
    pub fn is_manufacturable(self) -> bool {
        matches!(self, Self::Pass | Self::PassWithWarnings)
    }
}

/// Measurements taken from one layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerMetrics {
    pub index: u32,
    pub name: String,
    pub paths: usize,
    pub islands: usize,
    pub holes: usize,
    pub area_mm2: f64,
    pub bbox: [f64; 4],
    /// Narrowest measured element, in millimetres.
    ///
    /// The smaller of the band width and the narrowest neck, because they see
    /// different defects: one sees how thin a band is, the other sees where a
    /// piece comes apart. Reporting only the first let a 0.2 mm bridge tab
    /// through.
    pub min_width_mm: Option<f64>,
    /// Tightest gap between separate regions of this layer.
    ///
    /// `None` for a layer with fewer than two regions: one piece of film has no
    /// neighbour, so there is no gap to report.
    pub min_gap_mm: Option<f64>,
    /// Smallest area of any region in this layer, in square millimetres.
    pub min_island_area_mm2: Option<f64>,
    /// Regions per 100 square centimetres of the part footprint.
    pub density_per_100cm2: f64,
    /// Shortest edge anywhere in this layer, in millimetres.
    pub shortest_edge_mm: Option<f64>,
}

/// The result of one validation run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub verdict: Verdict,
    pub counts: Counts,
    pub violations: Vec<Violation>,
    pub layers: Vec<LayerMetrics>,
    pub fidelity: Option<f64>,
    /// Whether the profiles behind these limits have been proven by cutting
    /// real film. Until they are, a `Pass` is a calculation, not a fact.
    pub limits_physically_verified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub fatal: usize,
    pub error: usize,
    pub warning: usize,
    pub info: usize,
}

impl Counts {
    fn of(violations: &[Violation]) -> Self {
        let count = |s: Severity| violations.iter().filter(|v| v.severity == s).count();
        Self {
            fatal: count(Severity::Fatal),
            error: count(Severity::Error),
            warning: count(Severity::Warning),
            info: count(Severity::Info),
        }
    }
}

/// One mask layer as the validator sees it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerInput {
    pub index: u32,
    pub name: String,
    pub geometry: Shapes,
    /// Position in the paint sequence.
    ///
    /// A mask layer is one sheet of film, and layers are applied one after
    /// another. Where two of them overlap, that order decides which is on top —
    /// it is the whole resolution of a crossing in this manufacturing mode.
    ///
    /// `None` means "use the layer index", so a fixture that does not think about
    /// stacking still gets a defined order rather than a gap.
    #[serde(default)]
    pub application_order: Option<u32>,
    /// Tabs deliberately added to hold unanchored islands.
    ///
    /// Without this list a bridge is indistinguishable from any other part of the
    /// geometry, so its width and length cannot be compared against the material:
    /// T-06 and T-07 had no way to be evaluated at all. Recording a tab at the
    /// moment it is made is the only place its identity exists.
    ///
    /// Defaults on read, so existing fixtures and manifests stay valid.
    #[serde(default)]
    pub bridges: Vec<BridgeRecord>,
}

/// One tab, as it was placed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BridgeRecord {
    /// Perpendicular width of the neck.
    pub width_mm: f64,
    /// Span between the island and the film holding it.
    pub length_mm: f64,
    /// Where the tab meets the surrounding film.
    pub at: Point,
}

/// Validate a project against a rule set.
///
/// Never panics: every problem becomes a violation.
/// Validate a project that has not been nested.
#[must_use]
pub fn validate(
    layers: &[LayerInput],
    profile: &CuttingProfile,
    rules: &RuleSet,
    limits_physically_verified: bool,
) -> ValidationReport {
    validate_with_nesting(
        layers,
        profile,
        rules,
        limits_physically_verified,
        &BTreeMap::new(),
    )
}

/// Validate a project, taking any nesting plan into account.
///
/// `nested_tiles` maps a layer index to the bounding boxes of its tiles. The
/// validator deliberately takes plain rectangles rather than a nesting plan,
/// so this crate stays independent of the nesting implementation: what it needs
/// to know is the size of each printable piece, nothing more.
///
/// Width, gap and island rules are still measured on the *whole* layer. Splitting
/// a mask into tiles does not make an element wider, and measuring per tile would
/// hide an element that straddles a cut.
#[must_use]
pub fn validate_with_nesting(
    layers: &[LayerInput],
    profile: &CuttingProfile,
    rules: &RuleSet,
    limits_physically_verified: bool,
    nested_tiles: &BTreeMap<u32, Vec<Rect4>>,
) -> ValidationReport {
    validate_with_registration(
        layers,
        profile,
        rules,
        limits_physically_verified,
        nested_tiles,
        &BTreeMap::new(),
    )
}

/// Validate, additionally knowing where each layer's registration anchor sits on
/// the part.
///
/// T-15 needs this and nothing else does, so the anchors travel separately rather
/// than widening the tile map. Layers printed from the same nesting origin share an
/// anchor and contribute nothing; layers nested differently do not, and the
/// difference between those two is exactly the registration error the bench worker
/// will have to absorb.
#[must_use]
pub fn validate_with_registration(
    layers: &[LayerInput],
    profile: &CuttingProfile,
    rules: &RuleSet,
    limits_physically_verified: bool,
    nested_tiles: &BTreeMap<u32, Vec<Rect4>>,
    anchors: &BTreeMap<u32, Point>,
) -> ValidationReport {
    let mut violations = Vec::new();
    let mut metrics = Vec::new();

    // Invariant I-1: a project must have at least one layer.
    if layers.is_empty() {
        violations.push(Violation {
            code: "no_layers".into(),
            severity: Severity::Fatal,
            layer_index: None,
            measured: None,
            required: None,
            unit: Unit::Mm,
            count: 0,
            headline: "Проект не содержит ни одного слоя.".into(),
            reason: "Наложить маску нечем.".into(),
            actions: vec![Action {
                id: "add_layer".into(),
                label: "Добавить слой".into(),
                primary: true,
                estimated_fidelity_loss: None,
            }],
        });
        return finish(violations, metrics, None, limits_physically_verified);
    }

    for layer in layers {
        let measured = measure_layer(layer, profile.limits.curve_tolerance_mm);
        check_layer(
            layer,
            &measured,
            profile,
            rules,
            nested_tiles.get(&layer.index),
            &mut violations,
        );
        metrics.push(measured);
    }

    // registration (T-15) — the layers have to land on top of each other.
    //
    // Two errors add up: what the plotter itself can hold, and what the geometry
    // adds when layers are nested from different origins. The material's
    // tolerance is the budget both have to fit inside.
    if let Some(rule) = rules.rule("registration") {
        let budget = profile.limits.registration_budget_mm;
        let tool_error = profile.limits.registration_error_mm;
        let geometric = worst_anchor_spread(anchors);

        if tool_error + geometric > budget {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: None,
                measured: Some(tool_error + geometric),
                required: Some(budget),
                unit: Unit::Mm,
                count: 2,
                headline: format!(
                    "Слои не совместятся: {} от плоттера плюс {} от вложения — {}.",
                    format_measurement(tool_error, budget),
                    format_measurement(geometric, budget),
                    format_measurement(tool_error + geometric, budget)
                ),
                reason: rule.reason.to_string(),
                actions: vec![Action {
                    id: "show_marks".into(),
                    label: "Показать метки".into(),
                    primary: true,
                    estimated_fidelity_loss: None,
                }],
            });
        }
    }

    // crossing (T-12) — where two layers overlap, the paint order decides which
    // one is on top.
    //
    // The rule is written in the docs as "no crossings without a cutout in the
    // lower layer". That belongs to a single-sheet stencil, which this project does
    // not produce: each layer is one sheet of film and layers are applied in
    // sequence. Here the resolution is the order, not a hole, and this checks
    // that the order is actually defined.
    if let Some(rule) = rules.rule("crossing") {
        let unresolved: Vec<(usize, usize, f64)> = layer_crossings(layers)
            .into_iter()
            .filter(|(i, j, _)| application_order(&layers[*i]) == application_order(&layers[*j]))
            .collect();

        if !unresolved.is_empty() {
            let (a, b, area) = unresolved[0];
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: None,
                measured: Some(area),
                required: None,
                unit: Unit::Mm2,
                count: unresolved.len(),
                headline: format!(
                    "Слои {} «{}» и {} «{}» перекрываются на {} мм² без разного порядка нанесения.",
                    layers[a].index,
                    layers[a].name,
                    layers[b].index,
                    layers[b].name,
                    mask_kernel::report(area)
                ),
                reason: rule.reason.to_string(),
                actions: vec![
                    Action {
                        id: "set_order".into(),
                        label: "Задать порядок нанесения".into(),
                        primary: true,
                        estimated_fidelity_loss: None,
                    },
                    Action {
                        id: "show_elements".into(),
                        label: "Показать элементы".into(),
                        primary: false,
                        estimated_fidelity_loss: None,
                    },
                ],
            });
        }
    }

    // A project with no curves anywhere gets one note, so the check stays
    // discoverable. Without it the rule would simply never appear, which is how it
    // spent its whole life before curves existed.
    if !layers.iter().any(|l| has_curves(&l.geometry)) {
        if let Some(rule) = rules.rule("corner_radius") {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: Severity::Info,
                layer_index: None,
                measured: None,
                required: Some(profile.limits.corner_radius_mm),
                unit: Unit::Mm,
                count: 0,
                headline: "Кривых в проекте нет: минимальный радиус угла не проверяется.".into(),
                reason: "Проверка включается автоматически, как только контур содержит дугу."
                    .into(),
                actions: vec![Action {
                    id: "acknowledge".into(),
                    label: "Понятно".into(),
                    primary: false,
                    estimated_fidelity_loss: None,
                }],
            });
        }
    }

    finish(violations, metrics, None, limits_physically_verified)
}

/// Measure one layer. Geometry errors degrade to `None`, never to a panic.
/// Measure one layer, flattening curves for the measurements that cannot read them.
///
/// `min_width` and `narrowest_neck` work on polygons. Given a contour with arcs
/// they read `.points` and return the width of the chords — which for a curved
/// region is not a slightly wrong answer but a different shape, and it is the
/// shape whose narrowness decides whether the piece passes. The tolerance is the
/// profile's own, so the measurement belongs to the blade that has to cut it.
fn measure_layer(layer: &LayerInput, curve_tolerance_mm: f64) -> LayerMetrics {
    let (min_x, min_y, w, h) = bounding_box(&layer.geometry);
    let polygonal = layer.geometry.flattened(curve_tolerance_mm);

    // Each shape is a separate region of film, so the layer's narrowest element
    // is the minimum across regions. min_width works on a Shapes set, so each
    // region is wrapped rather than reimplementing the measurement.
    // The minimum over two different measures, because they see different defects.
    //
    // `min_width` is how wide a region is as a band. `narrowest_neck` is where the
    // region stops being one piece. For a band they agree; for a bridged island
    // they do not, and `narrowest_neck` is the one that would have caught a 0.2 mm
    // bridge tab passing this check.
    let min_width_mm = polygonal
        .shapes
        .iter()
        .filter_map(|shape| min_width(&Shapes::single(shape.clone())))
        .reduce(f64::min);
    let narrowest = polygonal
        .shapes
        .iter()
        .filter_map(|shape| narrowest_neck(&Shapes::single(shape.clone())))
        .reduce(f64::min);
    let narrowest_mm = match (min_width_mm, narrowest) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    LayerMetrics {
        index: layer.index,
        name: layer.name.clone(),
        paths: layer.geometry.shapes.len(),
        islands: layer.geometry.shapes.len(),
        holes: layer.geometry.shapes.iter().map(|s| s.holes.len()).sum(),
        area_mm2: shapes_area(&layer.geometry),
        bbox: [min_x, min_y, w, h],
        min_width_mm: narrowest_mm,
        min_gap_mm: min_gap(&layer.geometry).map(|g| g.mm),
        min_island_area_mm2: smallest_region_area(&layer.geometry),
        density_per_100cm2: density_per_100cm2(&layer.geometry),
        shortest_edge_mm: shortest_edge_in(&layer.geometry),
    }
}

/// The smallest region in a layer, by area.
///
/// Regions rather than islands: a loose piece of film and a piece that is joined
/// to the rest fail in the same way, and they fail the same way whether or not
/// anyone has noticed the second one is loose yet.
fn smallest_region_area(shapes: &Shapes) -> Option<f64> {
    shapes
        .shapes
        .iter()
        .map(shape_area)
        .map(f64::abs)
        .filter(|a| *a > 0.0)
        .fold(None, |acc: Option<f64>, a| {
            Some(acc.map_or(a, |acc: f64| acc.min(a)))
        })
}

/// Regions per 100 square centimetres (T-13).
///
/// Measured against the part's footprint, not the total ink area. The cost this
/// rule is about is labour: how many separate pieces of film the operator has to
/// weed and place, per unit of surface they are doing. Dividing by ink area would
/// make a sparse, hard-to-cut drawing look dense.
fn density_per_100cm2(shapes: &Shapes) -> f64 {
    let (_, _, w, h) = bounding_box(shapes);
    let footprint_mm2 = w * h;
    if footprint_mm2 <= 0.0 {
        return 0.0;
    }
    // 100 cm² = 10 000 mm².
    shapes.shapes.len() as f64 * 10_000.0 / footprint_mm2
}

/// A tile bounding box as `[min_x, min_y, width, height]`.
pub type Rect4 = [f64; 4];

/// The effective application order of a layer.
#[must_use]
fn application_order(layer: &LayerInput) -> u32 {
    layer.application_order.unwrap_or(layer.index)
}

/// Where two layers' geometry overlaps, if it does.
///
/// The overlap is real film on real film. In this manufacturing mode it is
/// resolved by which layer is applied first, not by cutting a hole — a cutout in
/// the lower layer would belong to a single-sheet stencil, which this project does
/// not produce.
fn layer_crossings(layers: &[LayerInput]) -> Vec<(usize, usize, f64)> {
    let mut out = Vec::new();
    for i in 0..layers.len() {
        for j in (i + 1)..layers.len() {
            // Every overlap is reported; deciding which ones are unresolved is the
            // caller's job. Filtering here and again at the call site made the
            // check vacuously empty.
            if let Ok(overlap) = intersection(&layers[i].geometry, &layers[j].geometry) {
                let area = shapes_area(&overlap).abs();
                if area > GEOM_AREA_EPS_MM2 {
                    out.push((i, j, area));
                }
            }
        }
    }
    out
}

/// Overlap below this is numerical noise from the boolean library, not film.
const GEOM_AREA_EPS_MM2: f64 = 1e-9;

/// The largest distance between any two layers' registration anchors.
///
/// Zero when fewer than two layers have one, which is the normal case: every
/// layer is nested from the same origin and they land together by construction.
fn worst_anchor_spread(anchors: &BTreeMap<u32, Point>) -> f64 {
    let points: Vec<Point> = anchors.values().copied().collect();
    let mut worst = 0.0_f64;
    for (i, a) in points.iter().enumerate() {
        for b in points.iter().skip(i + 1) {
            worst = worst.max(a.distance_to(*b));
        }
    }
    worst
}

/// Comparison band for dimension checks, in millimetres.
///
/// Re-exported from the kernel so the epsilon has one definition
/// (ADR-0004) rather than a second copy here.
fn constants_band() -> f64 {
    mask_kernel::MIN_WIDTH_EPS_MM
}

/// Apply the rule set to one layer.
fn check_layer(
    layer: &LayerInput,
    metrics: &LayerMetrics,
    profile: &CuttingProfile,
    rules: &RuleSet,
    tiles: Option<&Vec<Rect4>>,
    violations: &mut Vec<Violation>,
) {
    let limits = &profile.limits;

    // corner_radius (T-05) — the narrowest curve the knife must follow.
    //
    // This used to be an unconditional note that the check did not apply, because
    // polygonal geometry has no radius: a vertex is infinitely sharp, so the number
    // is zero everywhere and a check against it would fail on every shape for a
    // reason that means nothing.
    //
    // Contours now carry arcs, so the radius exists. The check is applied when the
    // geometry has curves and stays silent when it does not — which is now a fact
    // about the geometry rather than an assumption about it.
    //
    // The `if let` is the load-bearing part, and it is there rather than an `expect`
    // because the two used to disagree. `has_curves` was true for any non-empty arc
    // list while `min_corner_radius` filtered `r > 0.0`, so a contour carrying a
    // zero-radius arc made the first say yes and the second say none — and the
    // `expect` between them panicked. In wasm that does not throw, it kills the
    // module: the next call from the editor fails too.
    //
    // Arc validation (ADR-0015) makes the disagreement unreachable, so this is
    // defence in depth rather than the fix. The crate promises at its head that it
    // never panics, and that promise should not rest on an earlier call having
    // happened.
    if let Some(rule) = rules.rule("corner_radius") {
        // `min_corner_radius` is already `None` for a polygon: it folds over arcs and
        // a polygon has none. The extra `has_curves` guard the original code reached
        // for is not needed, and the two only ever disagreed because neither was
        // checked against the other's answer.
        if let Some(radius) = min_corner_radius(&layer.geometry) {
            if !meets_threshold(radius, limits.corner_radius_mm) {
                violations.push(Violation {
                    code: rule.code.clone(),
                    severity: rule.severity,
                    layer_index: Some(layer.index),
                    measured: Some(radius),
                    required: Some(limits.corner_radius_mm),
                    unit: Unit::Mm,
                    count: 1,
                    headline: format!(
                        "Слой {} «{}»: кривая уже минимального радиуса {}.",
                        layer.index,
                        layer.name,
                        format_measurement(radius, limits.corner_radius_mm)
                    ),
                    reason: rule.reason.to_string(),
                    actions: vec![
                        Action {
                            id: "widen_curve".into(),
                            label: "Увеличить радиус".into(),
                            primary: true,
                            estimated_fidelity_loss: Some(0.005),
                        },
                        Action {
                            id: "show_elements".into(),
                            label: "Показать элементы".into(),
                            primary: false,
                            estimated_fidelity_loss: None,
                        },
                    ],
                });
            }
        }
    }

    // peelability (T-11) — not modelled, and the rule exists to say so.
    //
    // Peelability is not a geometric threshold. Whether a feature survives being
    // removed depends on the film, the adhesive and the angle the operator pulls
    // at, and the only real number available is `peel_angle_min_deg`, which comes
    // from a physical test on a real sheet. Nothing in the geometry can be compared
    // against it that would mean anything.
    //
    // What geometry does cover is already checked: a feature too thin to hold is
    // T-01, too little of it to stay put is T-03, and one that will fall off is
    // T-04. So this note is not covering a gap — it is saying the remaining
    // physics is a procedure, not a measurement.
    //
    // One thing it does check: that the profile's peel rate is one this build can
    // interpret. A value nobody recognises means the declared behaviour is not
    // being applied, which would silently weaken every future check here.
    if let Some(rule) = rules.rule("peelability") {
        let known = matches!(
            profile.limits.peel_rate.as_str(),
            "slow" | "moderate" | "fast"
        );
        if known {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: Severity::Info,
                layer_index: None,
                measured: None,
                required: Some(profile.limits.line_width_mm),
                unit: Unit::Mm,
                count: 0,
                headline: "Снятие не смоделировано: это процедура, а не измерение.".into(),
                reason: format!(
                    "Плёнка снимается под углом не менее {}° со скоростью {}. Геометрия проверяется через ширину (T-01) и островки (T-03).",
                    profile.limits.peel_angle_min_deg, profile.limits.peel_rate
                ),
                actions: vec![Action { id: "acknowledge".into(), label: "Понятно".into(), primary: false, estimated_fidelity_loss: None }],
            });
        } else {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: Severity::Warning,
                layer_index: None,
                measured: None,
                required: Some(profile.limits.line_width_mm),
                unit: Unit::Mm,
                count: 0,
                headline: format!(
                    "Скорость снятия «{}» этой сборке неизвестна.",
                    profile.limits.peel_rate
                ),
                reason: "Заявленное поведение плёнки не будет применено: снятие и так не смоделировано, а теперь даже его параметры прочитать нельзя.".into(),
                actions: vec![Action { id: "review_profile".into(), label: "Проверить профиль материала".into(), primary: true, estimated_fidelity_loss: None }],
            });
        }
    }

    // min_line_width — the rule that matters most for physical survival.
    if let (Some(rule), Some(width)) = (rules.rule("min_line_width"), metrics.min_width_mm) {
        if !meets_threshold(width, limits.line_width_mm) {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: Some(width),
                required: Some(limits.line_width_mm),
                unit: Unit::Mm,
                count: 1,
                headline: format!(
                    "Слой {} «{}»: элемент уже минимума.",
                    layer.index, layer.name
                ),
                reason: rule.reason.to_string(),
                actions: autofix_and_explain(rule.auto_fixable),
            });
        }
    }

    // layer_fits_field — can the plotter reach every printable piece?
    //
    // When a nesting plan exists the check applies to each tile, because the
    // tiles are what actually gets printed. Measuring the uncut layer instead
    // would keep reporting a problem that a successful nest has already solved.
    if let Some(rule) = rules.rule("layer_fits_field") {
        match tiles {
            Some(rects) => {
                // Compared with a tolerance band. A tile that measures exactly
                // the field width does fit; without the band, float noise of
                // 1e-7 mm was enough to report it as too large.
                let too_wide = limits.max_tile_width_mm + crate::constants_band();
                let too_long = limits.max_tile_length_mm + crate::constants_band();

                let mut worst_width = 0.0_f64;
                let mut worst_length = 0.0_f64;
                let mut offending = 0usize;
                for r in rects {
                    let over_w = r[2] > too_wide;
                    let over_l = r[3] > too_long;
                    if over_w || over_l {
                        offending += 1;
                    }
                    worst_width = worst_width.max(r[2]);
                    worst_length = worst_length.max(r[3]);
                }

                if offending > 0 {
                    // Report the axis that actually failed, not the larger of
                    // the two: quoting a length against a width limit is how a
                    // reader concludes the numbers are nonsense.
                    let (measured, required) = if worst_width > too_wide {
                        (worst_width, limits.max_tile_width_mm)
                    } else {
                        (worst_length, limits.max_tile_length_mm)
                    };
                    violations.push(Violation {
                        code: rule.code.clone(),
                        severity: rule.severity,
                        layer_index: Some(layer.index),
                        measured: Some(measured),
                        required: Some(required),
                        unit: Unit::Mm,
                        count: offending,
                        headline: format!(
                            "Слой {}: {} из {} плиток не помещаются в рабочее поле.",
                            layer.index,
                            offending,
                            rects.len()
                        ),
                        reason: rule.reason.to_string(),
                        actions: vec![Action {
                            id: "nest_denser".into(),
                            label: "Разбить плотнее".into(),
                            primary: true,
                            estimated_fidelity_loss: Some(0.0),
                        }],
                    });
                }
            }
            None => {
                let w = metrics.bbox[2];
                let h = metrics.bbox[3];
                let band = crate::constants_band();
                if w > limits.max_tile_width_mm + band || h > limits.max_tile_length_mm + band {
                    let (measured, required) = if w > limits.max_tile_width_mm + band {
                        (w, limits.max_tile_width_mm)
                    } else {
                        (h, limits.max_tile_length_mm)
                    };
                    violations.push(Violation {
                        code: rule.code.clone(),
                        severity: rule.severity,
                        layer_index: Some(layer.index),
                        measured: Some(measured),
                        required: Some(required),
                        unit: Unit::Mm,
                        count: 1,
                        headline: format!(
                            "Слой {} не помещается в рабочее поле: {:.1}×{:.1} мм при {:.1}×{:.1} мм.",
                            layer.index,
                            w,
                            h,
                            limits.max_tile_width_mm,
                            limits.max_tile_length_mm
                        ),
                        reason: rule.reason.to_string(),
                        actions: vec![Action {
                            id: "nest".into(),
                            label: "Разбить на плитки".into(),
                            primary: true,
                            estimated_fidelity_loss: Some(0.0),
                        }],
                    });
                }
            }
        }
    }

    // min_gap — two regions of one layer must not come closer than the material's
    // minimum gap, or they merge while being weeded and the film between them
    // takes the neighbouring piece with it.
    //
    // Reported per pair rather than as a layer total: "somewhere there is a gap of
    // 0.4 mm" is not actionable, and the fix is always local.
    if let Some(rule) = rules.rule("min_gap") {
        let tight = min_gap(&layer.geometry);
        if let Some(gap) = tight {
            if !meets_threshold(gap.mm, limits.gap_mm) {
                violations.push(Violation {
                    code: rule.code.clone(),
                    severity: rule.severity,
                    layer_index: Some(layer.index),
                    measured: Some(gap.mm),
                    required: Some(limits.gap_mm),
                    unit: Unit::Mm,
                    count: 1,
                    // format_measurement already carries both numbers and the
                    // word «минимум», so appending it again produced
                    // "0.400 mm (минимум 0.900 mm) мм".
                    headline: format!(
                        "Слой {} «{}»: элементы {} и {} разошлись на {}.",
                        layer.index,
                        layer.name,
                        gap.a + 1,
                        gap.b + 1,
                        format_measurement(gap.mm, limits.gap_mm)
                    ),
                    reason: rule.reason.to_string(),
                    actions: vec![
                        Action {
                            id: "widen_gap".into(),
                            label: "Увеличить зазор".into(),
                            primary: true,
                            estimated_fidelity_loss: Some(0.01),
                        },
                        Action {
                            id: "merge_elements".into(),
                            label: "Объединить элементы".into(),
                            primary: false,
                            estimated_fidelity_loss: Some(0.02),
                        },
                        Action {
                            id: "show_elements".into(),
                            label: "Показать элементы".into(),
                            primary: false,
                            estimated_fidelity_loss: None,
                        },
                    ],
                });
            }
        }
    }

    // min_angle (T-14) — an interior angle below the material's minimum
    // concentrates stress in the film and tears it.
    if let Some(rule) = rules.rule("min_angle") {
        let sharp = sharp_corners(&layer.geometry, limits.min_angle_deg);
        if !sharp.is_empty() {
            let worst = sharp
                .iter()
                .map(|c| c.angle_deg)
                .fold(f64::INFINITY, f64::min);
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: Some(worst),
                required: Some(limits.min_angle_deg),
                unit: Unit::Deg,
                count: sharp.len(),
                headline: format!(
                    "Слой {} «{}»: {} острее {}.",
                    layer.index,
                    layer.name,
                    counted(sharp.len(), "угол", "угла", "углов"),
                    format_degrees(worst, limits.min_angle_deg)
                ),
                reason: rule.reason.to_string(),
                actions: vec![
                    Action {
                        id: "round_corners".into(),
                        label: "Скруглить углы".into(),
                        primary: true,
                        estimated_fidelity_loss: Some(0.005),
                    },
                    Action {
                        id: "show_elements".into(),
                        label: "Показать элементы".into(),
                        primary: false,
                        estimated_fidelity_loss: None,
                    },
                ],
            });
        }
    }

    // min_feature_length (T-10) — an edge too short for the knife to follow as
    // drawn. The blade has an offset and an angle, so it rounds off anything
    // shorter than that, and such edges are mostly tracing noise.
    //
    // A warning rather than an error: the mask is still cuttable, it just comes out
    // slightly different from the drawing.
    if let Some(rule) = rules.rule("min_feature_length") {
        if let Some(shortest) = metrics.shortest_edge_mm {
            if !meets_threshold(shortest, limits.feature_length_mm) {
                violations.push(Violation {
                    code: rule.code.clone(),
                    severity: rule.severity,
                    layer_index: Some(layer.index),
                    measured: Some(shortest),
                    required: Some(limits.feature_length_mm),
                    unit: Unit::Mm,
                    count: 1,
                    headline: format!(
                        "Слой {} «{}»: самое короткое ребро — {}. Нож его закруглит.",
                        layer.index,
                        layer.name,
                        format_measurement(shortest, limits.feature_length_mm)
                    ),
                    reason: rule.reason.to_string(),
                    actions: vec![
                        Action {
                            id: "simplify".into(),
                            label: "Упростить контур".into(),
                            primary: true,
                            estimated_fidelity_loss: Some(0.005),
                        },
                        Action {
                            id: "show_elements".into(),
                            label: "Показать элементы".into(),
                            primary: false,
                            estimated_fidelity_loss: None,
                        },
                    ],
                });
            }
        }
    }

    // bridge_width (T-06) and bridge_length (T-07) — the tabs recorded when they
    // were placed.
    //
    // These were unmeasurable before `LayerInput` carried the list. A bridge is
    // merged into the geometry and becomes indistinguishable from any other part,
    // so there was no way to ask about one. That is why the check now compares the
    // recorded numbers rather than measuring the geometry: the measurement was
    // never the problem, the identity was.
    if let Some(rule) = rules.rule("bridge_width") {
        let too_narrow: Vec<&BridgeRecord> = layer
            .bridges
            .iter()
            .filter(|b| !meets_threshold(b.width_mm, limits.bridge_width_mm))
            .collect();
        if !too_narrow.is_empty() {
            let narrowest = too_narrow
                .iter()
                .map(|b| b.width_mm)
                .fold(f64::INFINITY, f64::min);
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: Some(narrowest),
                required: Some(limits.bridge_width_mm),
                unit: Unit::Mm,
                count: too_narrow.len(),
                headline: format!(
                    "Слой {} «{}»: {} уже минимальной ширины {}. Самый узкий — {} мм.",
                    layer.index,
                    layer.name,
                    counted(too_narrow.len(), "мост", "моста", "мостов"),
                    format_measurement(narrowest, limits.bridge_width_mm),
                    mask_kernel::report(narrowest)
                ),
                reason: rule.reason.to_string(),
                actions: vec![Action {
                    id: "widen_bridge".into(),
                    label: "Расширить мост".into(),
                    primary: true,
                    estimated_fidelity_loss: Some(0.001),
                }],
            });
        }
    }

    if let Some(rule) = rules.rule("bridge_length") {
        let too_long: Vec<&BridgeRecord> = layer
            .bridges
            .iter()
            .filter(|b| b.length_mm > limits.bridge_length_mm)
            .collect();
        if !too_long.is_empty() {
            let longest = too_long.iter().map(|b| b.length_mm).fold(0.0_f64, f64::max);
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: Some(longest),
                required: Some(limits.bridge_length_mm),
                unit: Unit::Mm,
                count: too_long.len(),
                headline: format!(
                    "Слой {} «{}»: {} длиннее {}. Самый длинный — {} мм.",
                    layer.index,
                    layer.name,
                    counted(too_long.len(), "мост", "моста", "мостов"),
                    format_measurement(longest, limits.bridge_length_mm),
                    mask_kernel::report(longest)
                ),
                reason: rule.reason.to_string(),
                actions: vec![Action {
                    id: "split_bridge".into(),
                    label: "Укоротить или снять".into(),
                    primary: true,
                    estimated_fidelity_loss: Some(0.02),
                }],
            });
        }
    }

    // self_intersection (T-08) — a contour that crosses itself. Signed area
    // assumes simplicity: the crossings cut the contour into lobes that cancel, so
    // the reported area is plausible and wrong. This project has been bitten twice
    // by that shape of defect and both times nothing complained, because a wrong
    // area looks like a number.
    //
    // Errors rather than warnings. The geometry is not measurable, so a verdict
    // that let it pass would be reporting on something undefined.
    if let Some(rule) = rules.rule("self_intersection") {
        let found = self_intersections(&layer.geometry.flattened(limits.curve_tolerance_mm));
        if !found.is_empty() {
            let first = found[0];
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: None,
                required: None,
                unit: Unit::Mm,
                count: found.len(),
                headline: format!(
                    "Слой {} «{}»: {} пересечений контура. Первое — в ({}; {}).",
                    layer.index,
                    layer.name,
                    found.len(),
                    mask_kernel::report(first.at.x),
                    mask_kernel::report(first.at.y)
                ),
                reason: rule.reason.to_string(),
                actions: vec![
                    // Deliberately not "fix automatically": resolving a
                    // self-intersection reshapes the figure, and a mask is a
                    // physical object cut from film.
                    Action {
                        id: "show_elements".into(),
                        label: "Показать пересечения".into(),
                        primary: true,
                        estimated_fidelity_loss: None,
                    },
                    Action {
                        id: "normalise".into(),
                        label: "Пересобрать геометрию (меняет рисунок)".into(),
                        primary: false,
                        estimated_fidelity_loss: None,
                    },
                ],
            });
        }
    }

    // open_cut_path (T-09) — a cut contour left open. The knife follows it off the
    // sheet.
    if let Some(rule) = rules.rule("open_cut_path") {
        let open = open_cut_contours(&layer.geometry);
        if !open.is_empty() {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: None,
                required: None,
                unit: Unit::Mm,
                count: open.len(),
                headline: format!(
                    "Слой {} «{}»: {} режущих контуров не замкнуты.",
                    layer.index,
                    layer.name,
                    open.len()
                ),
                reason: rule.reason.to_string(),
                actions: vec![
                    Action {
                        id: "close_path".into(),
                        label: "Замкнуть контур".into(),
                        primary: true,
                        estimated_fidelity_loss: Some(0.0),
                    },
                    Action {
                        id: "show_elements".into(),
                        label: "Показать элементы".into(),
                        primary: false,
                        estimated_fidelity_loss: None,
                    },
                ],
            });
        }
    }

    // min_island_width and min_island_area (T-03) — a region with too little film
    // in it will not stay put. Two distinct requirements with two distinct
    // thresholds in the profile, and the docs only ever named the area, so the
    // rule table covered only the width.
    //
    // Every region is checked, not only the ones flagged as islands: a piece that
    // is joined to the rest of the film still has to hold itself up.
    if let Some(rule) = rules.rule("min_island_area") {
        let offenders: Vec<(usize, f64)> = layer
            .geometry
            .shapes
            .iter()
            .enumerate()
            .map(|(i, s)| (i, shape_area(s).abs()))
            .filter(|(_, a)| *a > 0.0 && !meets_threshold(*a, limits.island_area_mm2))
            .collect();
        if !offenders.is_empty() {
            let smallest = offenders
                .iter()
                .map(|(_, a)| *a)
                .fold(f64::INFINITY, f64::min);
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: Some(smallest),
                required: Some(limits.island_area_mm2),
                unit: Unit::Mm2,
                count: offenders.len(),
                headline: format!(
                    "Слой {} «{}»: {} меньше {}. Меньше всех — {} мм².",
                    layer.index,
                    layer.name,
                    counted(offenders.len(), "элемент", "элемента", "элементов"),
                    format_area(smallest, limits.island_area_mm2),
                    mask_kernel::report(smallest)
                ),
                reason: rule.reason.to_string(),
                actions: vec![
                    Action {
                        id: "merge_island".into(),
                        label: "Объединить с соседним элементом".into(),
                        primary: true,
                        estimated_fidelity_loss: Some(0.01),
                    },
                    Action {
                        id: "delete_island".into(),
                        label: "Удалить".into(),
                        primary: false,
                        estimated_fidelity_loss: Some(0.02),
                    },
                    Action {
                        id: "show_elements".into(),
                        label: "Показать элементы".into(),
                        primary: false,
                        estimated_fidelity_loss: None,
                    },
                ],
            });
        }
    }

    // T-13 — element density. A warning, not an error: it is about how long the
    // job takes, not whether the result can be cut at all.
    //
    // Worded separately from the shared "measured / required" line, because this is
    // an upper bound. Rendering it the other way round would read as though the
    // layer were short of the limit when it is above it.
    if let Some(rule) = rules.rule("layer_density") {
        let density = metrics.density_per_100cm2;
        if density > limits.max_features_per_100cm2 {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: Some(density),
                required: Some(limits.max_features_per_100cm2),
                unit: Unit::Count,
                count: metrics.islands,
                headline: format!(
                    "Слой {} «{}»: {:.0} элементов на 100 см², максимум {:.0}. Вырезать вручную будет долго.",
                    layer.index, layer.name, density, limits.max_features_per_100cm2
                ),
                reason: rule.reason.to_string(),
                actions: vec![Action { id: "show_elements".into(), label: "Показать элементы".into(), primary: true, estimated_fidelity_loss: None }],
            });
        }
    }

    // island_anchored — a region nested inside another region's hole cannot be
    // applied as a separate piece and must be held by a bridge.
    //
    // Disjoint regions are deliberately not flagged: a mask layer cut from one
    // sheet yields separate pieces of film, each applied on its own.
    if let Some(rule) = rules.rule("island_anchored") {
        let islands = unanchored_islands(&layer.geometry);
        if !islands.is_empty() {
            violations.push(Violation {
                code: rule.code.clone(),
                severity: rule.severity,
                layer_index: Some(layer.index),
                measured: None,
                required: Some(limits.line_width_mm),
                unit: Unit::Mm,
                count: islands.len(),
                headline: format!(
                    "Слой {} «{}»: {} {} с маской.",
                    layer.index,
                    layer.name,
                    counted(islands.len(), "островок", "островка", "островков"),
                    agreeing(islands.len(), "не соединён", "не соединены")
                ),
                reason: "При снятии маски островок выпадет и повредит деталь.".into(),
                actions: vec![
                    Action {
                        id: "add_bridge".into(),
                        label: "Добавить мосты".into(),
                        primary: true,
                        estimated_fidelity_loss: Some(0.004),
                    },
                    Action {
                        id: "merge_island".into(),
                        label: "Объединить с соседним элементом".into(),
                        primary: false,
                        estimated_fidelity_loss: Some(0.01),
                    },
                    Action {
                        id: "show_elements".into(),
                        label: "Показать элементы".into(),
                        primary: false,
                        estimated_fidelity_loss: None,
                    },
                ],
            });
        }
    }
}

fn autofix_and_explain(auto_fixable: bool) -> Vec<Action> {
    let mut actions = Vec::new();
    if auto_fixable {
        actions.push(Action {
            id: "autofix".into(),
            label: "Исправить автоматически".into(),
            primary: true,
            estimated_fidelity_loss: Some(0.02),
        });
    }
    actions.push(Action {
        id: "show_elements".into(),
        label: "Показать элементы".into(),
        primary: false,
        estimated_fidelity_loss: None,
    });
    actions.push(Action {
        id: "change_material".into(),
        label: "Выбрать другой материал".into(),
        primary: false,
        estimated_fidelity_loss: None,
    });
    actions
}

fn finish(
    violations: Vec<Violation>,
    layers: Vec<LayerMetrics>,
    fidelity: Option<f64>,
    verified: bool,
) -> ValidationReport {
    let verdict = Verdict::from_violations(&violations);
    let counts = Counts::of(&violations);
    ValidationReport {
        verdict,
        counts,
        violations,
        layers,
        fidelity,
        limits_physically_verified: verified,
    }
}

/// Render a report as the human-facing text the UI shows (P-09).
#[must_use]
pub fn render(report: &ValidationReport) -> String {
    if report.violations.is_empty() {
        return "Проблем не найдено.".to_string();
    }

    let mut out = Vec::new();
    for v in &report.violations {
        let mark = match v.severity {
            Severity::Fatal => "✖",
            Severity::Error => "✖",
            Severity::Warning => "⚠",
            Severity::Info => "ℹ",
        };
        out.push(format!("{mark} {}", v.headline));
        if let (Some(m), Some(r)) = (v.measured, v.required) {
            // An angle gets one decimal and a degree sign; a length gets the
            // canonical three (ADR-0004). Printing an angle as millimetres was a
            // real bug: "18.925 mm" for a 19° corner.
            let (m, r, suffix) = match v.unit {
                Unit::Mm => (
                    format!("{:.3}", mask_kernel::report(m)),
                    format!("{:.3}", mask_kernel::report(r)),
                    v.unit.suffix(),
                ),
                Unit::Deg => (format!("{m:.1}"), format!("{r:.1}"), v.unit.suffix()),
                Unit::Mm2 => (
                    format!("{:.3}", mask_kernel::report(m)),
                    format!("{:.3}", mask_kernel::report(r)),
                    v.unit.suffix(),
                ),
                // A count has no unit, and printing a bare number against another
                // bare number reads as a measurement with no limit attached.
                Unit::Count => (format!("{m:.0}"), format!("{r:.0}"), "на 100 см²"),
            };
            out.push(format!("   измерено {m} {suffix}, требуется {r} {suffix}"));
        }
        out.push(format!("   Почему это важно: {}", v.reason));
        let buttons: Vec<String> = v.actions.iter().map(|a| a.label.clone()).collect();
        if !buttons.is_empty() {
            out.push(format!("   [{}]", buttons.join("]  [")));
        }
        out.push(String::new());
    }

    if !report.limits_physically_verified {
        out.push(
            "Примечание: пороги взяты из datasheet и не подтверждены вырезом реального материала."
                .to_string(),
        );
    }
    out.join("\n")
}

/// Convenience: build a layer input from simple contour rings.
#[must_use]
pub fn layer_from_contours(index: u32, name: &str, contours: Vec<Vec<[f64; 2]>>) -> LayerInput {
    let shapes = contours
        .into_iter()
        .map(|pts| {
            let outer = Contour {
                points: pts.iter().map(|[x, y]| Point { x: *x, y: *y }).collect(),
                closed: true,
                arcs: Vec::new(),
            };
            Shape::new(outer)
        })
        .collect();
    LayerInput {
        index,
        name: name.to_string(),
        geometry: Shapes { shapes },
        bridges: Vec::new(),
        application_order: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::widen;

    fn profile() -> CuttingProfile {
        let material = mask_profiles::MaterialProfile {
            code: "matte-vinyl-090".into(),
            version: 4,
            name: "Matte vinyl".into(),
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
            status: mask_profiles::ProfileStatus::Active,
            verified_physical: true,
            notes: String::new(),
        };
        let plotter = mask_profiles::PlotterProfile {
            code: "knife-25mm".into(),
            version: 2,
            name: "Knife plotter".into(),
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
                registration_method: "crosshair_marks".into(),
                max_registration_error_mm: 0.30,
            },
            status: mask_profiles::ProfileStatus::Active,
            verified_physical: true,
        };
        CuttingProfile::derive(&material, &plotter)
    }

    /// A band of the given width, as one layer.
    fn band_layer(index: u32, width: f64, length: f64) -> LayerInput {
        let line = vec![
            Point {
                x: 0.0,
                y: width / 2.0,
            },
            Point {
                x: length,
                y: width / 2.0,
            },
        ];
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: widen(&line, width).unwrap(),
            bridges: Vec::new(),
            application_order: None,
        }
    }

    fn run(layers: Vec<LayerInput>) -> ValidationReport {
        let cp = profile();
        let rs = RuleSet::from_profile(&cp);
        validate(&layers, &cp, &rs, true)
    }

    #[test]
    fn a_band_at_the_threshold_passes_the_width_rule() {
        let report = run(vec![band_layer(1, 0.80, 200.0)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "min_line_width"),
            "a 0.80 mm band must satisfy a 0.80 mm minimum: {:?}",
            report.violations
        );
    }

    #[test]
    fn a_band_below_the_threshold_fails_and_explains_itself() {
        let report = run(vec![band_layer(1, 0.42, 200.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_line_width")
            .expect("a 0.42 mm band must violate a 0.80 mm minimum");
        assert_eq!(v.severity, Severity::Error);
        assert!(matches!(report.verdict, Verdict::Fail));
        assert!(
            v.reason.contains("Детализация") || v.reason.contains("плёнка") || !v.reason.is_empty()
        );
        assert!(v.actions.iter().any(|a| a.id == "autofix"));
    }

    #[test]
    fn rendered_text_names_the_measurement_and_the_limit() {
        let report = run(vec![band_layer(1, 0.42, 200.0)]);
        let text = render(&report);
        assert!(text.contains("0.420 мм"), "measured value missing: {text}");
        assert!(text.contains("0.800 мм"), "required value missing: {text}");
        assert!(
            !text.contains("ERROR"),
            "no error codes in user-facing text: {text}"
        );
    }

    /// A layer of separate rectangles at a chosen spacing, so the gap between
    /// neighbours is exactly the spacing.
    fn bars_layer(index: u32, spacing: f64) -> LayerInput {
        let shapes: Vec<Shape> = (0..3)
            .map(|i| {
                let x = i as f64 * (10.0 + spacing);
                Shape::new(Contour {
                    points: vec![
                        Point { x, y: 0.0 },
                        Point {
                            x: x + 10.0,
                            y: 0.0,
                        },
                        Point {
                            x: x + 10.0,
                            y: 5.0,
                        },
                        Point { x, y: 5.0 },
                    ],
                    closed: true,
                    arcs: Vec::new(),
                })
            })
            .collect();
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes { shapes },
            bridges: Vec::new(),
            application_order: None,
        }
    }

    #[test]
    fn a_gap_at_the_threshold_passes_the_gap_rule() {
        // The material's minimum gap is 0.90 mm.
        let report = run(vec![bars_layer(1, 0.90)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "min_gap"),
            "a 0.90 mm gap must satisfy a 0.90 mm minimum: {:?}",
            report.violations
        );
    }

    #[test]
    fn a_gap_below_the_threshold_fails() {
        let report = run(vec![bars_layer(1, 0.40)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_gap")
            .expect("a 0.40 mm gap must violate a 0.90 mm minimum");
        assert_eq!(v.severity, Severity::Error);
        assert!(matches!(report.verdict, Verdict::Fail));
        assert!(
            (v.measured.expect("measured") - 0.40).abs() < 1e-3,
            "should measure 0.40 mm, got {:?}",
            v.measured
        );
    }

    #[test]
    fn a_tight_gap_names_the_two_offending_elements() {
        let report = run(vec![bars_layer(1, 0.40)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_gap")
            .expect("violation");
        // Elements are numbered from 1 for the operator, not from 0.
        assert!(
            v.headline.contains('1') && v.headline.contains('2'),
            "{}",
            v.headline
        );
        assert!(v.actions.iter().any(|a| a.id == "widen_gap"));
    }

    #[test]
    fn the_gap_metric_is_reported_per_layer() {
        let report = run(vec![bars_layer(1, 0.40)]);
        let metrics = &report.layers[0];
        assert!(
            (metrics.min_gap_mm.expect("a gap") - 0.40).abs() < 1e-3,
            "got {:?}",
            metrics.min_gap_mm
        );
    }

    #[test]
    fn a_layer_of_one_region_reports_no_gap_at_all() {
        // One piece of film has no neighbour, so a gap number would be invented.
        let report = run(vec![band_layer(1, 0.80, 200.0)]);
        assert_eq!(report.layers[0].min_gap_mm, None);
        assert!(!report.violations.iter().any(|v| v.code == "min_gap"));
    }

    #[test]
    fn touching_regions_are_a_zero_gap_not_a_pass() {
        // Bars that share an edge: weeding merges them, so this must fail.
        let mut layer = bars_layer(1, 0.40);
        // Slide the second and third bars left until each sits flush against its
        // neighbour: 10.40 -> 10.00.
        let offset = -0.40;
        layer.geometry.shapes[1]
            .outer
            .points
            .iter_mut()
            .for_each(|p| p.x += offset);
        layer.geometry.shapes[2]
            .outer
            .points
            .iter_mut()
            .for_each(|p| p.x += offset);
        let report = run(vec![layer]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_gap")
            .expect("touching film merges during weeding");
        assert!(v.measured.expect("measured") < 1e-6, "got {:?}", v.measured);
    }

    /// A traced figure with a spike whose top vertex is `angle_deg`.
    fn spike_layer(index: u32, angle_deg: f64) -> LayerInput {
        // The apex sits 14 mm above the shoulders of the spike, so the half-angle
        // is atan(w / 14). Using the wrong vertical here silently produces a
        // polygon that is not the angle it claims — which is exactly what the first
        // version of this helper did, and the test failed for the right reason.
        let half = angle_deg.to_radians() / 2.0;
        let w = 14.0 * half.tan();
        let contour = Contour {
            points: vec![
                Point { x: 10.0, y: 2.0 },
                Point {
                    x: 10.0 + w * 2.0,
                    y: 2.0,
                },
                Point {
                    x: 10.0 + w * 2.0,
                    y: 12.0,
                },
                Point {
                    x: 10.0 + w,
                    y: 26.0,
                },
                Point { x: 10.0, y: 12.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        };
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes {
                shapes: vec![Shape::new(contour)],
            },
            bridges: Vec::new(),
            application_order: None,
        }
    }

    #[test]
    fn a_sharp_apex_is_reported_as_an_angle_not_a_radius() {
        // 18.9° is under the material's 30° minimum.
        let report = run(vec![spike_layer(1, 18.9)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_angle")
            .expect("a 19° apex must violate a 30° minimum");
        assert_eq!(
            v.severity,
            Severity::Warning,
            "T-14 is a warning, not an error"
        );
        assert_eq!(v.unit, Unit::Deg);
        assert!((v.measured.expect("measured") - 18.9).abs() < 0.2);
        assert_eq!(v.required, Some(30.0));
        assert_eq!(v.count, 1, "only the apex is sharp");
        assert!(v.actions.iter().any(|a| a.id == "round_corners"));
    }

    #[test]
    fn an_angle_at_the_threshold_passes() {
        let report = run(vec![spike_layer(1, 30.0)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "min_angle"),
            "30° must satisfy a 30° minimum: {:?}",
            report.violations
        );
    }

    #[test]
    fn an_ordinary_outline_raises_no_angle_warning() {
        // A plain square: every corner is 90°, so nothing to report. A check that
        // fires here would be noise, and noise is what made the old T-05 note
        // worth keeping as an Info in the first place.
        let report = run(vec![layer_from_contours(
            1,
            "Square",
            vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]],
        )]);
        assert!(
            !report.violations.iter().any(|v| v.code == "min_angle"),
            "a square has no sharp corners: {:?}",
            report.violations
        );
    }

    #[test]
    fn a_sharp_apex_does_not_fail_the_project_on_its_own() {
        // Severity matters: an angle under 30° is a warning, so the project is
        // still cuttable. Treating it as an error would block output for something
        // the operator can see and decide about.
        let report = run(vec![spike_layer(1, 18.9)]);
        assert!(matches!(report.verdict, Verdict::PassWithWarnings));
        assert_eq!(report.counts.error, 0);
    }

    #[test]
    fn the_rendered_angle_is_in_degrees() {
        // This is the bug: `format_measurement` on an angle printed millimetres,
        // so the report told the operator an angle was a length.
        let report = run(vec![spike_layer(1, 18.9)]);
        let text = render(&report);
        assert!(text.contains("18.9"), "measured value missing: {text}");
        assert!(
            text.contains('°'),
            "the degree sign must be present: {text}"
        );
        assert!(
            !text.contains("18.9 мм"),
            "an angle must not be printed as millimetres: {text}"
        );
    }

    #[test]
    fn lengths_still_render_in_millimetres() {
        // The unit field must not have broken the common case.
        let report = run(vec![band_layer(1, 0.42, 200.0)]);
        let text = render(&report);
        assert!(text.contains("0.420 мм"), "lengths keep their unit: {text}");
    }

    #[test]
    fn counts_are_inflected_in_the_user_facing_text() {
        // "1 углов" and "1 островков не соединены" are both wrong, in the one line
        // the operator is asked to trust.
        assert_eq!(plural(1, "угол", "угла", "углов"), "угол");
        assert_eq!(plural(2, "угол", "угла", "углов"), "угла");
        assert_eq!(plural(5, "угол", "угла", "углов"), "углов");
        assert_eq!(plural(11, "угол", "угла", "углов"), "углов");
        assert_eq!(plural(21, "угол", "угла", "углов"), "угол");

        assert_eq!(agreeing(1, "не соединён", "не соединены"), "не соединён");
        assert_eq!(agreeing(2, "не соединён", "не соединены"), "не соединены");
        assert_eq!(agreeing(11, "не соединён", "не соединены"), "не соединены");

        assert_eq!(
            counted(1, "островок", "островка", "островков"),
            "1 островок"
        );
        assert_eq!(
            counted(3, "островок", "островка", "островков"),
            "3 островка"
        );
    }

    #[test]
    fn a_self_crossing_contour_is_an_error_not_a_warning() {
        // The distinction matters: the geometry is not measurable at all, so a
        // verdict that let it through would be reporting on something undefined.
        let report = run(vec![bowtie_layer(1)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "self_intersection")
            .expect("a bow tie must be caught");
        assert_eq!(v.severity, Severity::Error);
        assert_eq!(v.count, 1);
        assert!(matches!(report.verdict, Verdict::Fail));
        // Two errors, not one: a bow tie also has zero width somewhere, so the
        // width rule fires too. Asserting one would have been asserting that the
        // bow tie is not degenerate, which is not the point.
        assert!(
            report.counts.error >= 1,
            "at least the crossing itself must be an error"
        );
    }

    #[test]
    fn a_crossing_is_reported_with_the_point_it_occurs_at() {
        // "Somewhere in this contour" is not actionable; a coordinate is.
        let report = run(vec![bowtie_layer(1)]);
        let text = render(&report);
        // Coordinates are rounded to the canonical three decimals (ADR-0004).
        assert!(text.contains('('), "the location should be shown: {text}");
        assert!(text.contains("10"), "the crossing is at (10, 10): {text}");
    }

    #[test]
    fn self_intersection_offers_no_silent_automatic_fix() {
        // Resolving a crossing reshapes the figure. A mask is a physical object cut
        // from film, so the operator decides, not the validator.
        let report = run(vec![bowtie_layer(1)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "self_intersection")
            .expect("v");
        assert!(
            !v.actions.iter().any(|a| a.id == "autofix"),
            "there must be no plain automatic fix: {:?}",
            v.actions
        );
        assert!(
            v.actions.iter().any(|a| a.id == "normalise"),
            "but the explicit rebuild must be offered: {:?}",
            v.actions
        );
    }

    #[test]
    fn an_ordinary_square_raises_no_self_intersection() {
        // Every polygon shares vertices between its own edges. Counting those would
        // make the rule fire on everything and drown the real violations.
        let report = run(vec![layer_from_contours(
            1,
            "Square",
            vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]],
        )]);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.code == "self_intersection"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn an_open_cut_contour_is_reported() {
        let open = Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 0.0 },
                Point { x: 20.0, y: 20.0 },
            ],
            closed: false,
            arcs: Vec::new(),
        };
        let layer = LayerInput {
            index: 1,
            name: "Open".into(),
            geometry: Shapes {
                shapes: vec![Shape::new(open)],
            },
            bridges: Vec::new(),
            application_order: None,
        };
        let report = run(vec![layer]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "open_cut_path")
            .expect("an unclosed cut contour must be caught");
        assert_eq!(v.severity, Severity::Error);
        assert_eq!(v.count, 1);
        assert!(matches!(report.verdict, Verdict::Fail));
    }

    #[test]
    fn a_closed_layer_raises_no_open_cut_path() {
        let report = run(vec![layer_from_contours(
            1,
            "Closed",
            vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0]]],
        )]);
        assert!(
            !report.violations.iter().any(|v| v.code == "open_cut_path"),
            "{:?}",
            report.violations
        );
    }

    /// A layer of separate square regions of a given side length.
    fn islands_layer(index: u32, side: f64, count: usize, pitch: f64) -> LayerInput {
        let shapes: Vec<Shape> = (0..count)
            .map(|i| {
                let x = i as f64 * pitch;
                Shape::new(Contour {
                    points: vec![
                        Point { x, y: 0.0 },
                        Point {
                            x: x + side,
                            y: 0.0,
                        },
                        Point {
                            x: x + side,
                            y: side,
                        },
                        Point { x, y: side },
                    ],
                    closed: true,
                    arcs: Vec::new(),
                })
            })
            .collect();
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes { shapes },
            bridges: Vec::new(),
            application_order: None,
        }
    }

    #[test]
    fn a_tiny_region_fails_the_area_rule() {
        // The material's minimum island area is 2.40 mm². A 1x1 mm piece is 1 mm².
        let report = run(vec![islands_layer(1, 1.0, 1, 10.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_island_area")
            .expect("a 1 mm² piece must violate a 2.40 mm² minimum");
        assert_eq!(v.severity, Severity::Error);
        assert_eq!(v.unit, Unit::Mm2);
        assert!((v.measured.expect("measured") - 1.0).abs() < 0.05);
        assert_eq!(v.required, Some(2.40));
    }

    #[test]
    fn a_region_at_the_area_threshold_passes() {
        // 2.5 mm on a side is 6.25 mm², comfortably over 2.40.
        let report = run(vec![islands_layer(1, 2.5, 1, 10.0)]);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.code == "min_island_area"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn the_area_rule_names_how_many_and_the_smallest() {
        let report = run(vec![islands_layer(1, 1.0, 4, 10.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_island_area")
            .expect("violation");
        assert_eq!(v.count, 4, "all four pieces are too small");
        assert!(v.headline.contains("4 элемента"), "{}", v.headline);
    }

    #[test]
    fn a_normal_band_raises_no_island_area_warning() {
        let report = run(vec![band_layer(1, 0.80, 200.0)]);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.code == "min_island_area"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn a_layer_of_many_pieces_is_flagged_as_dense() {
        // 120 pieces on a 25 x 2 mm strip: 480 per 100 cm², against a limit of 180.
        let report = run(vec![islands_layer(1, 2.0, 120, 2.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "layer_density")
            .expect("120 pieces on a narrow strip must be flagged");
        assert_eq!(
            v.severity,
            Severity::Warning,
            "T-13 is about labour, not cuttability"
        );
        assert!(v.measured.expect("measured") > 180.0, "{:?}", v.measured);
    }

    #[test]
    fn density_is_measured_against_the_footprint_not_the_ink() {
        // A few large pieces over a large part is not dense, however much ink there is.
        let report = run(vec![islands_layer(1, 60.0, 2, 80.0)]);
        assert!(
            report.layers[0].density_per_100cm2 < 10.0,
            "two big pieces on a big part is sparse, got {}",
            report.layers[0].density_per_100cm2
        );
        assert!(
            !report.violations.iter().any(|v| v.code == "layer_density"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn the_density_rule_reads_as_an_upper_bound() {
        // The shared renderer says "measured X, required Y". For an upper bound that
        // phrasing points the wrong way, so this rule is worded separately and the
        // headline has to say which way it failed.
        let report = run(vec![islands_layer(1, 2.0, 120, 2.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "layer_density")
            .expect("violation");
        assert!(v.headline.contains("максимум"), "{}", v.headline);
    }

    #[test]
    fn a_sparse_layer_raises_no_density_warning() {
        let report = run(vec![islands_layer(1, 10.0, 3, 40.0)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "layer_density"),
            "{:?}",
            report.violations
        );
    }

    /// A frame, an island inside its hole, and a tab of a given width joining them.
    fn bridged_island_layer(index: u32, tab_mm: f64) -> LayerInput {
        let frame = Shape::with_holes(
            Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 20.0, y: 0.0 },
                    Point { x: 20.0, y: 20.0 },
                    Point { x: 0.0, y: 20.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            },
            vec![Contour {
                points: vec![
                    Point { x: 5.0, y: 5.0 },
                    Point { x: 5.0, y: 15.0 },
                    Point { x: 15.0, y: 15.0 },
                    Point { x: 15.0, y: 5.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            }],
        );
        let island = Shape::new(Contour {
            points: vec![
                Point { x: 8.0, y: 8.0 },
                Point { x: 12.0, y: 8.0 },
                Point { x: 12.0, y: 12.0 },
                Point { x: 8.0, y: 12.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        });
        let before = Shapes {
            shapes: vec![frame, island],
        };
        let (joined, _) = mask_kernel::bridge::bridge_unanchored(
            &before,
            mask_kernel::bridge::BridgeLimits {
                width_mm: tab_mm,
                max_length_mm: 25.0,
            },
        )
        .expect("bridge");
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: joined,
            bridges: Vec::new(),
            application_order: None,
        }
    }

    #[test]
    fn a_too_thin_bridge_tab_fails_the_width_rule() {
        // The regression this whole change exists for. The tab is 0.2 mm against a
        // 0.80 mm minimum, and the frame around it is 5 mm thick, so the old
        // measurement reported 5 mm and passed it.
        let report = run(vec![bridged_island_layer(1, 0.2)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_line_width")
            .expect("a 0.2 mm tab must fail a 0.80 mm minimum");
        assert_eq!(v.severity, Severity::Error);
        assert!(
            (v.measured.expect("measured") - 0.2).abs() < 0.05,
            "should measure the tab, got {:?}",
            v.measured
        );
        assert!(matches!(report.verdict, Verdict::Fail));
    }

    #[test]
    fn a_bridge_tab_at_the_threshold_passes() {
        let report = run(vec![bridged_island_layer(1, 1.2)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "min_line_width"),
            "a 1.2 mm tab must satisfy a 0.80 mm minimum: {:?}",
            report.violations
        );
    }

    #[test]
    fn the_reported_metric_is_the_narrower_of_band_and_neck() {
        let report = run(vec![bridged_island_layer(1, 1.2)]);
        let reported = report.layers[0].min_width_mm.expect("a measurement");
        // The frame is 5 mm thick; the tab is 1.2. The smaller is the answer.
        assert!(
            reported < 2.0,
            "should report the tab, not the frame, got {reported:.4}"
        );
    }

    /// A wide square with one corner nudged in by `short_edge_mm`.
    ///
    /// Wide, so the width rule stays silent and the short edge is the only thing
    /// under test. An earlier version made the figure narrow as well, which meant
    /// the project failed for two reasons and the assertion about the verdict was
    /// testing the wrong one.
    fn sliver_layer(index: u32, short_edge_mm: f64) -> LayerInput {
        let contour = Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 0.0 },
                Point { x: 20.0, y: 20.0 },
                Point {
                    x: short_edge_mm,
                    y: 20.0,
                },
                Point { x: 0.0, y: 20.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        };
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes {
                shapes: vec![Shape::new(contour)],
            },
            bridges: Vec::new(),
            application_order: None,
        }
    }

    #[test]
    fn an_edge_shorter_than_the_minimum_feature_is_reported() {
        // The material asks for 3.00 mm. A 0.4 mm edge cannot be cut as drawn.
        let report = run(vec![sliver_layer(1, 0.4)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_feature_length")
            .expect("a 0.4 mm edge must be reported");
        assert_eq!(
            v.severity,
            Severity::Warning,
            "the mask is still cuttable, just not as drawn"
        );
        assert!((v.measured.expect("measured") - 0.4).abs() < 0.05);
        assert_eq!(v.required, Some(3.00));
    }

    #[test]
    fn a_layer_without_short_edges_is_not_reported() {
        // A plain square. Passing 20 to sliver_layer would not do: its nudged
        // corner would land exactly on the previous vertex and create a zero-length
        // edge, which is a duplicate point rather than a square.
        let report = run(vec![layer_from_contours(
            1,
            "Square",
            vec![vec![[0.0, 0.0], [20.0, 0.0], [20.0, 20.0], [0.0, 20.0]]],
        )]);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.code == "min_feature_length"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn a_duplicate_vertex_is_reported_rather_than_ignored() {
        // The same thing seen from the other side: two identical points make a
        // zero-length edge, and a knife cannot cut that at all.
        let report = run(vec![sliver_layer(1, 20.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "min_feature_length")
            .expect("a zero-length edge must be reported, not silently dropped");
        assert!(v.measured.expect("measured") < 1e-6);
    }

    #[test]
    fn the_shortest_edge_is_reported_per_layer() {
        let report = run(vec![sliver_layer(1, 0.4)]);
        let shortest = report.layers[0].shortest_edge_mm.expect("a measurement");
        assert!((shortest - 0.4).abs() < 0.05, "got {shortest:.4}");
    }

    #[test]
    fn a_short_feature_is_not_an_error_on_its_own() {
        // It has to be a warning, or a slightly sloppy tracing would block output
        // for something the knife will simply round off. The figure here is 20 mm
        // wide, so nothing else fires and the verdict has to stay clean.
        let report = run(vec![sliver_layer(1, 0.4)]);
        assert_eq!(report.counts.error, 0, "a short edge must not be an error");
        assert!(
            matches!(report.verdict, Verdict::PassWithWarnings),
            "{:?}",
            report.verdict
        );
        assert_eq!(report.counts.warning, 1);
    }

    /// A layer with a recorded bridge of the given dimensions.
    fn bridged_layer(index: u32, width_mm: f64, length_mm: f64) -> LayerInput {
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: widen(&[Point { x: 0.0, y: 0.0 }, Point { x: 40.0, y: 0.0 }], 1.0).unwrap(),
            bridges: vec![BridgeRecord {
                width_mm,
                length_mm,
                at: Point { x: 10.0, y: 0.0 },
            }],
            application_order: None,
        }
    }

    #[test]
    fn a_too_narrow_bridge_fails_t06() {
        // The material asks for 1.20 mm. A 0.6 mm tab peels off with the waste.
        let report = run(vec![bridged_layer(1, 0.6, 10.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "bridge_width")
            .expect("a 0.6 mm bridge must violate a 1.20 mm minimum");
        assert_eq!(v.severity, Severity::Error);
        assert!((v.measured.expect("measured") - 0.6).abs() < 1e-6);
        assert_eq!(v.required, Some(1.20));
    }

    #[test]
    fn a_bridge_at_the_minimum_width_passes() {
        let report = run(vec![bridged_layer(1, 1.2, 10.0)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "bridge_width"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn an_over_long_bridge_warns_on_t07() {
        // A warning: it is still cuttable, it just tears during the peel.
        let report = run(vec![bridged_layer(1, 1.2, 40.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "bridge_length")
            .expect("a 40 mm bridge must exceed the 25 mm maximum");
        assert_eq!(v.severity, Severity::Warning);
        assert_eq!(v.count, 1);
        assert!(
            !matches!(report.verdict, Verdict::Fail),
            "T-07 must not block output"
        );
    }

    #[test]
    fn a_layer_without_bridges_says_nothing_about_them() {
        // No list, no findings. Otherwise every layer without a tab would report a
        // zero-width bridge.
        let report = run(vec![band_layer(1, 0.80, 200.0)]);
        assert!(!report.violations.iter().any(|v| v.code == "bridge_width"));
        assert!(!report.violations.iter().any(|v| v.code == "bridge_length"));
    }

    #[test]
    fn a_bridge_recorded_on_the_wire_round_trips() {
        // The field is serde-defaulted so old manifests still read, but a recorded
        // bridge has to survive a round trip or the check is useless in the API.
        let layer = bridged_layer(1, 1.4, 12.0);
        let text = serde_json::to_string(&layer).expect("serialise");
        let back: LayerInput = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back.bridges.len(), 1);
        assert!((back.bridges[0].width_mm - 1.4).abs() < 1e-9);
        assert!((back.bridges[0].length_mm - 12.0).abs() < 1e-9);
    }

    #[test]
    fn an_old_layer_without_the_field_still_reads() {
        let json = r#"{"index":1,"name":"Old","geometry":{"shapes":[]}}"#;
        let layer: LayerInput = serde_json::from_str(json).expect("old payloads stay readable");
        assert!(layer.bridges.is_empty());
    }

    fn two_layers() -> Vec<LayerInput> {
        vec![band_layer(1, 0.80, 200.0), band_layer(2, 0.80, 200.0)]
    }

    fn anchors_at(points: &[(u32, f64, f64)]) -> BTreeMap<u32, Point> {
        points
            .iter()
            .map(|(i, x, y)| (*i, Point { x: *x, y: *y }))
            .collect()
    }

    #[test]
    fn layers_nested_from_the_same_origin_register() {
        // The plotter's own 0.30 mm fits inside the material's 0.35 mm budget, and
        // both layers share an origin, so there is nothing to report.
        let anchors = anchors_at(&[(1, 0.0, 0.0), (2, 0.0, 0.0)]);
        let report = validate_with_registration(
            &two_layers(),
            &profile(),
            &RuleSet::from_profile(&profile()),
            true,
            &BTreeMap::new(),
            &anchors,
        );
        assert!(
            !report.violations.iter().any(|v| v.code == "registration"),
            "{:?}",
            report.violations
        );
    }

    #[test]
    fn layers_nested_from_different_origins_are_reported() {
        // 0.4 mm of nesting difference on top of the plotter's 0.30 is 0.70 against
        // a 0.35 budget. The layers would land visibly apart.
        let anchors = anchors_at(&[(1, 0.0, 0.0), (2, 0.4, 0.0)]);
        let report = validate_with_registration(
            &two_layers(),
            &profile(),
            &RuleSet::from_profile(&profile()),
            true,
            &BTreeMap::new(),
            &anchors,
        );
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "registration")
            .expect("a 0.4 mm anchor difference must be reported");
        assert_eq!(v.severity, Severity::Error);
        assert!(
            (v.measured.expect("measured") - 0.70).abs() < 1e-6,
            "{:?}",
            v.measured
        );
        assert_eq!(v.required, Some(0.35));
    }

    #[test]
    fn a_single_layer_never_has_a_registration_problem() {
        // There is nothing to register against.
        let anchors = anchors_at(&[(1, 99.0, 99.0)]);
        let report = validate_with_registration(
            &[band_layer(1, 0.80, 200.0)],
            &profile(),
            &RuleSet::from_profile(&profile()),
            true,
            &BTreeMap::new(),
            &anchors,
        );
        assert!(!report.violations.iter().any(|v| v.code == "registration"));
    }

    #[test]
    fn the_two_sources_of_error_are_reported_separately() {
        // "It will not align" is not actionable; "the plotter costs 0.30 and the
        // nesting adds 0.40" is, because one of those is fixable in this project and
        // the other is not.
        let anchors = anchors_at(&[(1, 0.0, 0.0), (2, 0.4, 0.0)]);
        let report = validate_with_registration(
            &two_layers(),
            &profile(),
            &RuleSet::from_profile(&profile()),
            true,
            &BTreeMap::new(),
            &anchors,
        );
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "registration")
            .expect("v");
        assert!(v.headline.contains("плоттера"), "{}", v.headline);
        assert!(v.headline.contains("вложения"), "{}", v.headline);
    }

    /// The same profile with a peel rate overridden.
    fn with_peel_rate(rate: &str) -> CuttingProfile {
        let mut cp = profile();
        cp.limits.peel_rate = rate.to_string();
        cp
    }

    #[test]
    fn peelability_is_reported_as_a_procedure_not_a_measurement() {
        let report = run(vec![band_layer(1, 0.80, 200.0)]);
        let note = report
            .violations
            .iter()
            .find(|v| v.code == "peelability")
            .expect("the note must be present");
        assert_eq!(
            note.severity,
            Severity::Info,
            "nothing is being measured, so nothing fails"
        );
        assert!(
            note.reason.contains("120"),
            "the declared angle should be shown: {}",
            note.reason
        );
        assert!(
            note.reason.contains("T-01"),
            "it should say what is checked instead: {}",
            note.reason
        );
    }

    #[test]
    fn an_unrecognised_peel_rate_is_a_warning() {
        // A new material must not be rejected just because this build has not heard
        // of its rate, but the declared behaviour must not be silently ignored
        // either.
        let cp = with_peel_rate("unheard_of");
        let report = validate(
            &[band_layer(1, 0.80, 200.0)],
            &cp,
            &RuleSet::from_profile(&cp),
            true,
        );
        let note = report
            .violations
            .iter()
            .find(|v| v.code == "peelability")
            .expect("the note must be present");
        assert_eq!(
            note.severity,
            Severity::Warning,
            "a rate nobody can read is worth a warning"
        );
        assert!(note.headline.contains("unheard_of"), "{}", note.headline);
    }

    #[test]
    fn every_known_peel_rate_is_accepted() {
        for rate in ["slow", "moderate", "fast"] {
            let cp = with_peel_rate(rate);
            let report = validate(
                &[band_layer(1, 0.80, 200.0)],
                &cp,
                &RuleSet::from_profile(&cp),
                true,
            );
            let note = report
                .violations
                .iter()
                .find(|v| v.code == "peelability")
                .expect("note");
            assert_eq!(note.severity, Severity::Info, "{rate} should be understood");
        }
    }

    /// Two square layers at a given offset, with a chosen application order.
    fn stacked_layers(order_a: Option<u32>, order_b: Option<u32>) -> Vec<LayerInput> {
        let square_at = |index: u32, x: f64, order: Option<u32>| LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes {
                shapes: vec![Shape::new(Contour {
                    points: vec![
                        Point { x, y: 0.0 },
                        Point {
                            x: x + 20.0,
                            y: 0.0,
                        },
                        Point {
                            x: x + 20.0,
                            y: 20.0,
                        },
                        Point { x, y: 20.0 },
                    ],
                    closed: true,
                    arcs: Vec::new(),
                })],
            },
            bridges: Vec::new(),
            application_order: order,
        };
        vec![square_at(1, 0.0, order_a), square_at(2, 10.0, order_b)]
    }

    #[test]
    fn overlapping_layers_with_the_same_explicit_order_are_reported() {
        // Two 20 mm squares offset by 10, both claiming to be seventh.
        let report = run(stacked_layers(Some(7), Some(7)));
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "crossing")
            .expect("an explicit order collision must be reported");
        assert_eq!(v.severity, Severity::Error);
        assert!(
            v.measured.expect("measured") > 0.0,
            "the overlap area should be given"
        );
        assert!(
            v.headline.contains('1') && v.headline.contains('2'),
            "{}",
            v.headline
        );
        assert!(matches!(report.verdict, Verdict::Fail));
    }

    #[test]
    fn overlapping_layers_with_a_distinct_order_are_resolved() {
        // This is the point: in a stack of films, the order is the resolution.
        // Nothing is cut out, and nothing needs to be.
        let report = run(stacked_layers(Some(1), Some(2)));
        assert!(
            !report.violations.iter().any(|v| v.code == "crossing"),
            "a defined order resolves the crossing: {:?}",
            report.violations
        );
    }

    #[test]
    fn layers_that_do_not_overlap_are_silent_whatever_the_order() {
        let mut layers = stacked_layers(None, None);
        layers[1].geometry.shapes[0]
            .outer
            .points
            .iter_mut()
            .for_each(|p| p.x += 100.0);
        let report = run(layers);
        assert!(!report.violations.iter().any(|v| v.code == "crossing"));
    }

    #[test]
    fn undeclared_order_falls_back_to_the_index_and_stays_quiet() {
        // Layers are naturally applied in the order they are listed, so the index
        // is a sensible default. The alternative — treating "not declared" as
        // unknown — would fire on nearly every multi-layer mask, and noise is what
        // made the old T-05 note worth keeping as an Info in the first place.
        let report = run(stacked_layers(None, None));
        assert!(
            !report.violations.iter().any(|v| v.code == "crossing"),
            "the index fallback already gives a distinct order: {:?}",
            report.violations
        );
    }

    #[test]
    fn an_empty_project_is_fatal() {
        let report = run(vec![]);
        assert_eq!(report.verdict, Verdict::Fatal);
    }

    #[test]
    fn verdict_pass_implies_no_error_violations() {
        // Invariant I-5, asserted.
        for report in [
            run(vec![]),
            run(vec![band_layer(1, 0.42, 200.0)]),
            run(vec![band_layer(1, 0.80, 200.0)]),
        ] {
            if report.verdict.is_manufacturable() {
                assert_eq!(report.counts.error, 0, "PASS with an error violation");
            }
        }
    }

    #[test]
    fn an_oversized_layer_is_reported_as_needing_nesting() {
        let report = run(vec![band_layer(1, 0.80, 2000.0)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "layer_fits_field")
            .expect("a 2000 mm band exceeds a 284 mm field");
        assert!(v.actions.iter().any(|a| a.id == "nest"));
    }

    #[test]
    fn unverified_limits_are_disclosed_even_on_a_clean_run() {
        let cp = profile();
        let rs = RuleSet::from_profile(&cp);
        let report = validate(&[band_layer(1, 0.80, 200.0)], &cp, &rs, false);
        assert!(report.verdict.is_manufacturable());
        assert!(
            !report.limits_physically_verified,
            "a pass on datasheet limits is a calculation, not a fact"
        );
        assert!(render(&report).contains("datasheet"));
    }

    #[test]
    fn validation_never_panics_on_hostile_geometry() {
        let hostile = LayerInput {
            index: 1,
            name: "hostile".into(),
            geometry: Shapes { shapes: vec![] },
            bridges: Vec::new(),
            application_order: None,
        };
        let report = run(vec![hostile]);
        assert!(
            report.verdict.is_manufacturable(),
            "empty layer geometry is not fatal"
        );
    }

    #[test]
    fn a_nested_island_without_a_bridge_is_rejected() {
        // A ring with a hole, and a patch floating inside that hole. Applied
        // as drawn, the inner patch has nothing holding it.
        let outer = Shape::with_holes(
            Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 20.0, y: 0.0 },
                    Point { x: 20.0, y: 20.0 },
                    Point { x: 0.0, y: 20.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            },
            vec![Contour {
                points: vec![
                    Point { x: 5.0, y: 5.0 },
                    Point { x: 15.0, y: 5.0 },
                    Point { x: 15.0, y: 15.0 },
                    Point { x: 5.0, y: 15.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            }],
        );
        let patch = Shape::new(Contour {
            points: vec![
                Point { x: 8.0, y: 8.0 },
                Point { x: 12.0, y: 8.0 },
                Point { x: 12.0, y: 12.0 },
                Point { x: 8.0, y: 12.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        });
        let layer = LayerInput {
            index: 1,
            name: "Nested".into(),
            geometry: Shapes {
                shapes: vec![outer, patch],
            },
            bridges: Vec::new(),
            application_order: None,
        };
        let report = run(vec![layer]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "island_anchored")
            .expect("an unbridged nested island must be reported");
        assert_eq!(v.count, 1);
        assert!(v.actions.iter().any(|a| a.id == "add_bridge"));
    }

    #[test]
    fn a_nested_island_with_a_bridge_is_accepted() {
        // Same geometry, but the patch overlaps the ring, so a union closes the
        // hole and nothing is left floating.
        let outer = Shape::with_holes(
            Contour {
                points: vec![
                    Point { x: 0.0, y: 0.0 },
                    Point { x: 20.0, y: 0.0 },
                    Point { x: 20.0, y: 20.0 },
                    Point { x: 0.0, y: 20.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            },
            vec![Contour {
                points: vec![
                    Point { x: 5.0, y: 5.0 },
                    Point { x: 15.0, y: 5.0 },
                    Point { x: 15.0, y: 15.0 },
                    Point { x: 5.0, y: 15.0 },
                ],
                closed: true,
                arcs: Vec::new(),
            }],
        );
        let bridged = Shape::new(Contour {
            points: vec![
                Point { x: 8.0, y: 8.0 },
                Point { x: 14.0, y: 8.0 },
                Point { x: 14.0, y: 12.0 },
                Point { x: 8.0, y: 12.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        });
        let merged = mask_kernel::union(
            &Shapes {
                shapes: vec![outer],
            },
            &Shapes {
                shapes: vec![bridged],
            },
        )
        .unwrap();
        let report = run(vec![LayerInput {
            index: 1,
            name: "Bridged".into(),
            geometry: merged,
            bridges: Vec::new(),
            application_order: None,
        }]);
        assert!(
            !report
                .violations
                .iter()
                .any(|v| v.code == "island_anchored"),
            "a bridged island must not be reported: {:?}",
            report.violations
        );
    }

    #[test]
    fn disjoint_regions_are_not_flagged_as_islands() {
        let report = run(vec![band_layer(1, 0.80, 60.0), band_layer(2, 0.80, 60.0)]);
        assert!(!report
            .violations
            .iter()
            .any(|v| v.code == "island_anchored"));
    }

    #[test]
    fn validation_is_deterministic() {
        let layers = vec![band_layer(1, 0.42, 200.0), band_layer(2, 0.80, 200.0)];
        assert_eq!(run(layers.clone()), run(layers));
    }
    fn bowtie_layer(index: u32) -> LayerInput {
        let contour = Contour {
            points: vec![
                Point { x: 0.0, y: 0.0 },
                Point { x: 20.0, y: 20.0 },
                Point { x: 20.0, y: 0.0 },
                Point { x: 0.0, y: 20.0 },
            ],
            closed: true,
            arcs: Vec::new(),
        };
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes {
                shapes: vec![Shape::new(contour)],
            },
            bridges: Vec::new(),
            application_order: None,
        }
    }

    /// A layer carrying a circle of the given radius.
    fn curved_layer(index: u32, radius: f64) -> LayerInput {
        let points: Vec<Point> = (0..4)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (i as f64);
                Point {
                    x: radius * a.cos(),
                    y: radius * a.sin(),
                }
            })
            .collect();
        let arcs = (0..4)
            .map(|i| mask_kernel::ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: Point { x: 0.0, y: 0.0 },
                radius,
                ccw: true,
            })
            .collect();
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes {
                shapes: vec![Shape::new(Contour {
                    points,
                    closed: true,
                    arcs,
                })],
            },
            bridges: Vec::new(),
            application_order: None,
        }
    }

    #[test]
    fn a_curve_below_the_minimum_radius_fails_t05() {
        let report = run(vec![curved_layer(1, 0.2)]);
        let v = report
            .violations
            .iter()
            .find(|v| v.code == "corner_radius")
            .expect("a 0.2 mm curve must violate a 0.40 mm minimum");
        assert_eq!(v.severity, Severity::Error);
        assert!((v.measured.expect("measured") - 0.2).abs() < 1e-6);
        assert_eq!(v.required, Some(0.40));
    }

    /// The regression behind ADR-0015: a contour carrying an arc the arc
    /// measurement cannot report used to take the validator down.
    ///
    /// `has_curves` was true for any non-empty arc list while `min_corner_radius`
    /// filtered `r > 0.0`, so a zero-radius arc made the first say yes and the
    /// second say none, and the `expect` between them panicked. In wasm a panic
    /// does not throw — it kills the module, so the next editor call failed too.
    ///
    /// Asserted on behaviour rather than on the absence of a panic: the crate
    /// promises at its head that every problem becomes a violation, so a run that
    /// returns a report is the contract, whatever the verdict inside it is.
    #[test]
    fn an_arc_the_measurement_cannot_report_does_not_take_the_validator_down() {
        for radius in [0.0, -1.0, f64::NAN] {
            let geometry = layer(Shapes::single(Shape {
                outer: Contour {
                    points: vec![
                        Point { x: 0.0, y: 0.0 },
                        Point { x: 10.0, y: 0.0 },
                        Point { x: 10.0, y: 10.0 },
                    ],
                    closed: true,
                    arcs: vec![mask_kernel::ArcSegment {
                        from: 0,
                        to: 1,
                        centre: Point { x: 5.0, y: 0.0 },
                        radius,
                        ccw: true,
                    }],
                },
                holes: vec![],
            }));

            // Reaching here without a panic is half the assertion. The other half
            // is that nothing invents a number: an arc the measurement cannot report
            // must not produce a `corner_radius` violation carrying `measured: 0.0`,
            // which would read as "your curve is too tight" and send the operator to
            // round a corner that does not exist.
            let report = run(vec![geometry]);
            assert!(
                !report.violations.iter().any(|v| v.code == "corner_radius"),
                "a radius of {radius} is not a curve; it must not be reported as one: {:?}",
                report.violations
            );
        }
    }

    #[test]
    fn a_curve_at_the_minimum_radius_passes() {
        let report = run(vec![curved_layer(1, 0.8)]);
        assert!(
            !report.violations.iter().any(|v| v.code == "corner_radius"),
            "{:?}",
            report.violations
        );
    }
    /// A disc, or the same disc with its arcs removed.
    ///
    /// `min_width` is documented to find the width of one band, so a ring is the
    /// wrong instrument — it reports the outer diameter and says so. A disc is the
    /// right one: its width is its diameter, and that is a number T-01 acts on.
    fn disc(r: f64, with_arcs: bool) -> Shapes {
        use mask_kernel::geom::{ArcSegment, Contour, Point};
        const N: usize = 16;
        let points = (0..N)
            .map(|i| {
                let a = std::f64::consts::TAU * (i as f64 / N as f64);
                Point {
                    x: r * a.cos(),
                    y: r * a.sin(),
                }
            })
            .collect::<Vec<_>>();
        let arcs = if with_arcs {
            (0..N)
                .map(|i| ArcSegment {
                    from: i,
                    to: (i + 1) % N,
                    centre: Point { x: 0.0, y: 0.0 },
                    radius: r,
                    ccw: true,
                })
                .collect()
        } else {
            Vec::new()
        };
        Shapes::single(mask_kernel::Shape::new(Contour {
            points,
            closed: true,
            arcs,
        }))
    }

    fn layer(geometry: Shapes) -> LayerInput {
        LayerInput {
            index: 0,
            name: "disc".into(),
            geometry,
            application_order: None,
            bridges: vec![],
        }
    }

    /// The measurement must follow the curve, not the polygon inside it.
    ///
    /// `min_width` bisects on inward offsets, so whatever the contour's boundary
    /// says is what T-01 compares against the material's minimum line width.
    ///
    /// Asserted as a relationship rather than a fixed percentage: flattening must
    /// land closer to the true diameter than the chords do. A 16-gon of radius 3 is
    /// 5.885 mm across against a true 6.000, and flattening at the blade's 0.08 mm
    /// tolerance refines it to roughly 32 segments and about 5.97. The margin is
    /// real but it is about 1.5%, which is worth knowing: at a limit set exactly on
    /// this measurement, a curve and its chords could land either side of it.
    #[test]
    fn a_disc_is_measured_from_its_arcs() {
        let tolerance = profile().limits.curve_tolerance_mm;
        let truth = 6.0;
        let width_of = |geometry| {
            measure_layer(&layer(geometry), tolerance)
                .min_width_mm
                .expect("a disc has a width")
        };

        let from_arcs = width_of(disc(3.0, true));
        let from_chords = width_of(disc(3.0, false));

        assert!(
            (from_arcs - truth).abs() < (from_chords - truth).abs(),
            "flattening must be the closer answer: arcs {from_arcs}, chords {from_chords}, truth {truth}"
        );
        assert!(
            (from_arcs - truth).abs() < truth * 0.01,
            "flattening at {tolerance} mm should be within 1% of {truth}, got {from_arcs}"
        );
        assert!(
            from_chords < 0.99 * truth,
            "and the chords should still be measurably short, got {from_chords}"
        );
    }
}
