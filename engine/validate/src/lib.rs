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
    bounding_box, format_degrees, format_measurement, meets_threshold, min_gap, min_width,
    shapes_area, sharp_corners, unanchored_islands, Contour, Point, Shape, Shapes,
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
    /// Narrowest measured element, when one could be measured.
    pub min_width_mm: Option<f64>,
    /// Tightest gap between separate regions of this layer.
    ///
    /// `None` for a layer with fewer than two regions: one piece of film has no
    /// neighbour, so there is no gap to report.
    pub min_gap_mm: Option<f64>,
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
#[derive(Debug, Clone)]
pub struct LayerInput {
    pub index: u32,
    pub name: String,
    pub geometry: Shapes,
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
        let measured = measure_layer(layer);
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

    // T-05 is stated as a radius and cannot be measured on polygonal contours:
    // every vertex has radius zero, so the number is not in the geometry. Saying
    // so once per run is right — six copies would bury the real violations.
    //
    // This is not replaced by T-14, it is the same requirement measured a way the
    // geometry allows. `minimums.corner_radius_mm` is documented as "a sharp
    // internal angle tears the film", so the angle is the requirement and the
    // radius is only the shape of the fix.
    if rules.rule("corner_radius").is_some() {
        violations.push(Violation {
            code: "corner_radius".into(),
            severity: Severity::Info,
            layer_index: None,
            measured: None,
            required: Some(profile.limits.corner_radius_mm),
            unit: Unit::Mm,
            count: 0,
            headline: "Радиус углов не измеряется: контуры многоугольные.".into(),
            reason: "Острый угол проверяется правилом min_angle. Радиус станет доступен, когда геометрия будет хранить кривые.".into(),
            actions: vec![Action {
                id: "acknowledge".into(),
                label: "Понятно".into(),
                primary: false,
                estimated_fidelity_loss: None,
            }],
        });
    }

    finish(violations, metrics, None, limits_physically_verified)
}

/// Measure one layer. Geometry errors degrade to `None`, never to a panic.
fn measure_layer(layer: &LayerInput) -> LayerMetrics {
    let (min_x, min_y, w, h) = bounding_box(&layer.geometry);

    // Each shape is a separate region of film, so the layer's narrowest element
    // is the minimum across regions. min_width works on a Shapes set, so each
    // region is wrapped rather than reimplementing the measurement.
    let min_width_mm = layer
        .geometry
        .shapes
        .iter()
        .filter_map(|shape| min_width(&Shapes::single(shape.clone())))
        .reduce(f64::min);

    LayerMetrics {
        index: layer.index,
        name: layer.name.clone(),
        paths: layer.geometry.shapes.len(),
        islands: layer.geometry.shapes.len(),
        holes: layer.geometry.shapes.iter().map(|s| s.holes.len()).sum(),
        area_mm2: shapes_area(&layer.geometry),
        bbox: [min_x, min_y, w, h],
        min_width_mm,
        min_gap_mm: min_gap(&layer.geometry).map(|g| g.mm),
    }
}

/// A tile bounding box as `[min_x, min_y, width, height]`.
pub type Rect4 = [f64; 4];

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
            };
            Shape::new(outer)
        })
        .collect();
    LayerInput {
        index,
        name: name.to_string(),
        geometry: Shapes { shapes },
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
                })
            })
            .collect();
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes { shapes },
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
        };
        LayerInput {
            index,
            name: format!("Layer {index}"),
            geometry: Shapes {
                shapes: vec![Shape::new(contour)],
            },
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
    fn the_corner_radius_note_says_what_is_actually_checked() {
        // The old note said only "not measured yet", which left the operator
        // without knowing the angle was being checked instead.
        let report = run(vec![spike_layer(1, 18.9)]);
        let note = report
            .violations
            .iter()
            .find(|v| v.code == "corner_radius")
            .expect("the applicability note");
        assert!(note.reason.contains("min_angle"), "{}", note.reason);
        assert_eq!(note.severity, Severity::Info);
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
            },
            vec![Contour {
                points: vec![
                    Point { x: 5.0, y: 5.0 },
                    Point { x: 15.0, y: 5.0 },
                    Point { x: 15.0, y: 15.0 },
                    Point { x: 5.0, y: 15.0 },
                ],
                closed: true,
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
        });
        let layer = LayerInput {
            index: 1,
            name: "Nested".into(),
            geometry: Shapes {
                shapes: vec![outer, patch],
            },
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
            },
            vec![Contour {
                points: vec![
                    Point { x: 5.0, y: 5.0 },
                    Point { x: 15.0, y: 5.0 },
                    Point { x: 15.0, y: 15.0 },
                    Point { x: 5.0, y: 15.0 },
                ],
                closed: true,
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
}
