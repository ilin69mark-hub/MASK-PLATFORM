//! Cutting profile and rule set.
//!
//! A Material Profile says what the film can physically hold. A Plotter Profile
//! says where that is possible to cut and how accurately. Neither alone is the
//! rule set — the *combination* is, and that combination is what a project must
//! reference (MANUFACTURING_ENGINE.md §4).
//!
//! Every formula below is documented in `MANUFACTURING_ENGINE.md` §4 and every
//! coefficient comes from a profile field, never from a literal here.

use mask_profiles::{MaterialProfile, PlotterProfile};
use serde::{Deserialize, Serialize};

/// The effective numeric limits for one material/plotter pairing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectiveLimits {
    pub line_width_mm: f64,
    pub gap_mm: f64,
    pub island_width_mm: f64,
    /// Minimum island area, from `material.minimums.island_area_mm2`.
    pub island_area_mm2: f64,
    /// Minimum feature length, from `material.minimums.feature_length_mm`.
    pub feature_length_mm: f64,
    /// Longest cuttable tab, from `material.minimums.bridge_length_mm`.
    pub bridge_length_mm: f64,
    pub corner_radius_mm: f64,
    /// The blade's chord tolerance, carried through so the geometry pipeline can
    /// flatten curves at the tolerance that belongs to the blade doing the cutting.
    ///
    /// It used to reach only the `corner_radius` derivation and then vanish, which
    /// left every downstream consumer with no honest way to approximate a curve: no
    /// tolerance, or one invented at the call site. A derived number nobody can
    /// reach is not a derived number.
    pub curve_tolerance_mm: f64,
    /// Minimum interior angle, from `material.limits.min_angle_deg`.
    pub min_angle_deg: f64,
    pub bridge_width_mm: f64,
    pub max_features_per_100cm2: f64,
    pub max_area_mm2: f64,
    pub max_tile_width_mm: f64,
    pub max_tile_length_mm: f64,
    pub kerf_mm: f64,
    pub registration_error_mm: f64,
    /// What the material can tolerate, from `physical.tolerance_registration_mm`.
    pub registration_budget_mm: f64,
    /// Minimum peel angle from the material datasheet, in degrees.
    pub peel_angle_min_deg: f64,
    /// Declared peel rate. Free text: a new material may use a value this build
    /// has never heard of, and rejecting it would be worse than flagging it.
    pub peel_rate: String,
}

/// Material plus plotter plus the limits they jointly imply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CuttingProfile {
    pub code: String,
    pub material_code: String,
    pub material_version: u32,
    pub plotter_code: String,
    pub plotter_version: u32,
    pub limits: EffectiveLimits,
    /// Every step of the derivation, recorded so a reader can audit the number
    /// instead of trusting it.
    pub derivation: Vec<String>,
}

/// Print a length at report precision so audit notes stay readable.
///
/// These strings are meant to be read by a human deciding whether to trust a
/// number, so they carry the same rounding the UI shows (ADR-0004 §6).
fn r(value: f64) -> f64 {
    mask_kernel::report(value)
}

impl CuttingProfile {
    /// Derive the effective limits from a material and a plotter.
    #[must_use]
    pub fn derive(material: &MaterialProfile, plotter: &PlotterProfile) -> Self {
        let m = &material.minimums;
        let b = &plotter.blade;
        let mut derivation = Vec::new();

        // Every number in these notes is printed at report precision. Raw f64
        // leaks artefacts such as 0.6000000000000001 into an audit trail.
        let line_width_mm = m.line_width_mm;
        derivation.push(format!(
            "line_width = material.minimums.line_width_mm = {} (material dominates)",
            r(line_width_mm)
        ));

        // The gap must accommodate both the film's own minimum and the material
        // the knife removes, otherwise two cuts merge.
        let gap_candidate = b.kerf_mm + plotter.accuracy.max_registration_error_mm;
        let gap_mm = m.gap_mm.max(gap_candidate);
        derivation.push(format!(
            "gap = max(material {}, kerf {} + registration {} = {}) = {}",
            r(m.gap_mm),
            r(b.kerf_mm),
            r(plotter.accuracy.max_registration_error_mm),
            r(gap_candidate),
            r(gap_mm)
        ));

        let island_candidate = m.line_width_mm * 0.75;
        let island_width_mm = m.island_width_mm.max(island_candidate);
        derivation.push(format!(
            "island_width = max(material {}, line_width x 0.75 = {}) = {}",
            r(m.island_width_mm),
            r(island_candidate),
            r(island_width_mm)
        ));

        derivation.push(format!(
            "curve_tolerance = blade.curve_tolerance = {}",
            r(b.curve_tolerance_mm)
        ));

        let corner_candidate = b.curve_tolerance_mm * b.curve_multiplier;
        let corner_radius_mm = m.corner_radius_mm.max(corner_candidate);
        derivation.push(format!(
            "corner_radius = max(material {}, curve_tolerance {} x multiplier {} = {}) = {}",
            r(m.corner_radius_mm),
            r(b.curve_tolerance_mm),
            r(b.curve_multiplier),
            r(corner_candidate),
            r(corner_radius_mm)
        ));

        let bridge_candidate = b.kerf_mm * 2.0;
        let bridge_width_mm = m.bridge_width_mm.max(bridge_candidate);
        derivation.push(format!(
            "bridge_width = max(material {}, kerf x 2 = {}) = {}",
            r(m.bridge_width_mm),
            r(bridge_candidate),
            r(bridge_width_mm)
        ));

        let margin = plotter.working_area.usable_margin_mm;
        let max_tile_width_mm = plotter.working_area.width_mm - 2.0 * margin;
        let max_tile_length_mm = plotter.working_area.length_mm - 2.0 * margin;
        derivation.push(format!(
            "max_tile = working_area - 2 x margin {} = {} x {}",
            r(margin),
            r(max_tile_width_mm),
            r(max_tile_length_mm)
        ));

        Self {
            code: format!(
                "{}__v{}__v{}",
                material.code, material.version, plotter.code
            ),
            material_code: material.code.clone(),
            material_version: material.version,
            plotter_code: plotter.code.clone(),
            plotter_version: plotter.version,
            limits: EffectiveLimits {
                line_width_mm,
                gap_mm,
                island_width_mm,
                island_area_mm2: material.minimums.island_area_mm2,
                feature_length_mm: material.minimums.feature_length_mm,
                bridge_length_mm: material.minimums.bridge_length_mm,
                corner_radius_mm,
                curve_tolerance_mm: b.curve_tolerance_mm,
                min_angle_deg: material.limits.min_angle_deg,
                bridge_width_mm,
                max_features_per_100cm2: material.limits.max_density_per_100cm2,
                max_area_mm2: material.limits.max_area_mm2,
                max_tile_width_mm,
                max_tile_length_mm,
                kerf_mm: b.kerf_mm,
                registration_error_mm: plotter.accuracy.max_registration_error_mm,
                registration_budget_mm: material.physical.tolerance_registration_mm,
                peel_angle_min_deg: material.physical.peel_angle_min_deg,
                peel_rate: material.physical.peel_rate.clone(),
            },
            derivation,
        }
    }

    /// Whether both sides have been proven by cutting real film.
    #[must_use]
    pub fn is_physically_verified(
        &self,
        material: &MaterialProfile,
        plotter: &PlotterProfile,
    ) -> bool {
        material.is_physically_verified() && plotter.status.is_physically_verified()
    }
}

/// One checkable rule.
///
/// The threshold is a reference to a limit, never a literal: changing the
/// material must not require touching this definition (P-05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Fatal,
}

/// The measurable quantity a rule constrains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Measure {
    ElementMinWidth,
    MinGap,
    IslandWidth,
    IslandAnchored,
    CornerRadius,
    BridgeWidth,
    LayerDensity,
    LayerFitsField,
}

/// A single rule from a profile pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub code: String,
    pub severity: Severity,
    pub measure: Measure,
    pub auto_fixable: bool,
    /// Field of [`EffectiveLimits`] this rule compares against.
    pub threshold: String,
    /// Human-facing template. `{n}` is the count of offending elements.
    pub headline: String,
    pub reason: String,
}

impl Rule {
    /// Look up the numeric threshold for this rule.
    #[must_use]
    pub fn threshold_value(&self, limits: &EffectiveLimits) -> f64 {
        match self.threshold.as_str() {
            "line_width_mm" => limits.line_width_mm,
            "gap_mm" => limits.gap_mm,
            "island_width_mm" => limits.island_width_mm,
            "island_area_mm2" => limits.island_area_mm2,
            "feature_length_mm" => limits.feature_length_mm,
            "bridge_length_mm" => limits.bridge_length_mm,
            "registration_budget_mm" => limits.registration_budget_mm,
            "corner_radius_mm" => limits.corner_radius_mm,
            "min_angle_deg" => limits.min_angle_deg,
            "bridge_width_mm" => limits.bridge_width_mm,
            "max_features_per_100cm2" => limits.max_features_per_100cm2,
            "max_area_mm2" => limits.max_area_mm2,
            "max_tile_width_mm" => limits.max_tile_width_mm,
            "max_tile_length_mm" => limits.max_tile_length_mm,
            _ => f64::NAN,
        }
        .max(0.0)
    }
}

/// An ordered set of rules derived from a cutting profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    pub id: String,
    pub cutting_profile: String,
    pub limits: EffectiveLimits,
    pub rules: Vec<Rule>,
}

impl RuleSet {
    #[must_use]
    pub fn from_profile(profile: &CuttingProfile) -> Self {
        // Ordered cheapest and most structural first, so an early fatal stops
        // the run before expensive geometry work (VALIDATOR.md §3).
        let rules = vec![
            Rule {
                code: "min_line_width".into(),
                severity: Severity::Error,
                measure: Measure::ElementMinWidth,
                auto_fixable: true,
                threshold: "line_width_mm".into(),
                headline: "{n} элементов слишком маленькие для выбранного материала.".into(),
                reason: "Детализация ниже разрешающей способности материала.".into(),
            },
            Rule {
                code: "min_gap".into(),
                severity: Severity::Error,
                measure: Measure::MinGap,
                auto_fixable: true,
                threshold: "gap_mm".into(),
                headline: "{n} зазоров между элементами меньше {required} мм.".into(),
                reason: "Вырезы сольются или плёнка порвётся.".into(),
            },
            Rule {
                code: "min_island_width".into(),
                severity: Severity::Error,
                measure: Measure::IslandWidth,
                auto_fixable: true,
                threshold: "island_width_mm".into(),
                headline: "{n} уже минимальной ширины {required}.".into(),
                reason: "Островок такой ширины не удержится на детали.".into(),
            },
            Rule {
                code: "min_island_area".into(),
                severity: Severity::Error,
                measure: Measure::IslandWidth,
                auto_fixable: true,
                threshold: "island_area_mm2".into(),
                headline: "{n} меньше {required}.".into(),
                reason: "Слишком мало плёнки, чтобы держаться на детали.".into(),
            },
            Rule {
                code: "island_anchored".into(),
                severity: Severity::Error,
                measure: Measure::IslandAnchored,
                auto_fixable: true,
                threshold: "line_width_mm".into(),
                headline: "{n} островков не соединены с маской.".into(),
                reason: "При снятии маски островок выпадет и повредит деталь.".into(),
            },
            Rule {
                code: "crossing".into(),
                severity: Severity::Error,
                measure: Measure::MinGap,
                auto_fixable: false,
                threshold: "gap_mm".into(),
                headline: "{n} пересечений слоёв без порядка нанесения.".into(),
                reason: "В стопке плёнки два слоя в одном месте: без порядка нанесения краска ляжет неоднозначно.".into(),
            },
            Rule {
                code: "peelability".into(),
                severity: Severity::Warning,
                measure: Measure::MinGap,
                auto_fixable: false,
                threshold: "line_width_mm".into(),
                headline: "Снятие не смоделировано.".into(),
                reason: "Проверяется косвенно: через минимальную ширину и площадь островков.".into(),
            },
            Rule {
                code: "corner_radius".into(),
                severity: Severity::Error,
                measure: Measure::CornerRadius,
                auto_fixable: true,
                threshold: "corner_radius_mm".into(),
                headline: "{n} углов острее {required} мм.".into(),
                reason: "Острый внутренний угол концентрирует напряжение и рвёт плёнку.".into(),
            },
            Rule {
                code: "min_angle".into(),
                severity: Severity::Warning,
                measure: Measure::CornerRadius,
                auto_fixable: true,
                threshold: "min_angle_deg".into(),
                headline: "{n} углов острее {required}°.".into(),
                reason: "Острый внутренний угол концентрирует напряжение и рвёт плёнку.".into(),
            },
            Rule {
                code: "self_intersection".into(),
                severity: Severity::Error,
                measure: Measure::MinGap,
                auto_fixable: false,
                threshold: "gap_mm".into(),
                headline: "{n} контуров пересекаются сами с собой.".into(),
                reason: "Площадь пересекающегося контура взаимно сокращается: получается правдоподобное число, но не площадь.".into(),
            },
            Rule {
                code: "open_cut_path".into(),
                severity: Severity::Error,
                measure: Measure::MinGap,
                auto_fixable: true,
                threshold: "gap_mm".into(),
                headline: "{n} режущих контуров не замкнуты.".into(),
                reason: "Нож пойдёт по линии до края листа и вырежет не то, что задумано.".into(),
            },
            Rule {
                code: "min_feature_length".into(),
                severity: Severity::Warning,
                measure: Measure::MinGap,
                auto_fixable: true,
                threshold: "feature_length_mm".into(),
                headline: "{n} рёбер короче {required} мм.".into(),
                reason: "Нож не может довернуть на таком коротком ребре: край будет закруглён и выйдет не так, как нарисовано.".into(),
            },
            Rule {
                code: "bridge_width".into(),
                severity: Severity::Error,
                measure: Measure::BridgeWidth,
                auto_fixable: false,
                threshold: "bridge_width_mm".into(),
                headline: "{n} мостов уже {required} мм.".into(),
                reason: "Мост такой ширины оторвётся при снятии маски.".into(),
            },
            Rule {
                code: "bridge_length".into(),
                severity: Severity::Warning,
                measure: Measure::BridgeWidth,
                auto_fixable: false,
                threshold: "bridge_length_mm".into(),
                headline: "{n} мостов длиннее {required} мм.".into(),
                reason: "Слишком длинный мост оторвётся при снятии маски.".into(),
            },
            Rule {
                code: "registration".into(),
                severity: Severity::Error,
                measure: Measure::MinGap,
                auto_fixable: false,
                threshold: "registration_budget_mm".into(),
                headline: "{n} слоёв не совместятся: расхождение {} мм при бюджете {required} мм.".into(),
                reason: "Слои лягут со смещением, и рисунок разъедется.".into(),
            },
            Rule {
                code: "layer_density".into(),
                severity: Severity::Warning,
                measure: Measure::LayerDensity,
                auto_fixable: false,
                threshold: "max_features_per_100cm2".into(),
                headline: "Слой {layer}: {n} элементов на 100 см² (максимум {required}). Вырезать вручную будет долго.".into(),
                reason: "Слишком много мелых элементов для ручного выреза.".into(),
                // T-13 is an upper bound, so the shared "measured / required"
                // rendering would read as if the measurement were short of the
                // limit when it is the other way round. The validator words it
                // separately for this reason.
            },
            Rule {
                code: "layer_fits_field".into(),
                severity: Severity::Error,
                measure: Measure::LayerFitsField,
                auto_fixable: true,
                threshold: "max_tile_width_mm".into(),
                headline: "Маска {measured} мм не помещается в рабочее поле {required} мм.".into(),
                reason: "Слой шире рабочего поля плоттера, нужна раскладка на плитки.".into(),
            },
        ];

        Self {
            id: format!("rs_{}", profile.code),
            cutting_profile: profile.code.clone(),
            limits: profile.limits.clone(),
            rules,
        }
    }

    #[must_use]
    pub fn rule(&self, code: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.code == code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_profiles::{Accuracy, Blade, Limits, Minimums, Physical, ProfileStatus, WorkingArea};

    fn material() -> MaterialProfile {
        MaterialProfile {
            code: "matte-vinyl-090".into(),
            version: 4,
            name: "Matte vinyl".into(),
            family: "vinyl".into(),
            thickness_mm: 0.09,
            minimums: Minimums {
                line_width_mm: 0.80,
                gap_mm: 0.90,
                island_area_mm2: 2.40,
                feature_length_mm: 3.00,
                bridge_length_mm: 25.0,
                island_width_mm: 0.60,
                corner_radius_mm: 0.40,
                bridge_width_mm: 1.20,
            },
            physical: Physical {
                adhesion: "medium".into(),
                adhesive_system: "water_sensitive".into(),
                peel_angle_min_deg: 120.0,
                peel_rate: "slow".into(),
                max_temp_c: 80.0,
                tolerance_registration_mm: 0.35,
                surface_prep: vec![],
            },
            limits: Limits {
                max_density_per_100cm2: 180.0,
                min_angle_deg: 30.0,
                max_area_mm2: 400_000.0,
            },
            status: ProfileStatus::Active,
            verified_physical: true,
            notes: String::new(),
        }
    }

    fn plotter() -> PlotterProfile {
        PlotterProfile {
            code: "knife-25mm".into(),
            version: 2,
            name: "Knife plotter".into(),
            kind: "knife_plotter".into(),
            blade: Blade {
                angle_deg: 45.0,
                offset_mm: 0.10,
                kerf_mm: 0.15,
                curve_tolerance_mm: 0.08,
                curve_multiplier: 4.0,
            },
            working_area: WorkingArea {
                width_mm: 300.0,
                length_mm: 1200.0,
                usable_margin_mm: 8.0,
            },
            accuracy: Accuracy {
                repeatability_mm: 0.05,
                registration_method: "crosshair_marks".into(),
                max_registration_error_mm: 0.30,
            },
            status: ProfileStatus::Active,
            verified_physical: true,
        }
    }

    #[test]
    fn derivation_matches_the_documented_formulas() {
        let cp = CuttingProfile::derive(&material(), &plotter());
        // material dominates width
        assert!((cp.limits.line_width_mm - 0.80).abs() < 1e-12);
        // max(0.90, kerf 0.15 + registration 0.30) = 0.90
        assert!((cp.limits.gap_mm - 0.90).abs() < 1e-12);
        // max(0.60, 0.80 × 0.75) = 0.60
        assert!((cp.limits.island_width_mm - 0.60).abs() < 1e-12);
        // max(0.40, 0.08 × 4) = 0.40
        assert!((cp.limits.corner_radius_mm - 0.40).abs() < 1e-12);
        // max(1.20, 0.15 × 2) = 1.20
        assert!((cp.limits.bridge_width_mm - 1.20).abs() < 1e-12);
        // 300 - 16
        assert!((cp.limits.max_tile_width_mm - 284.0).abs() < 1e-12);
        assert!((cp.limits.max_tile_length_mm - 1184.0).abs() < 1e-12);
    }

    #[test]
    fn derivation_records_every_step() {
        let cp = CuttingProfile::derive(&material(), &plotter());
        assert_eq!(
            cp.derivation.len(),
            7,
            "all seven formulas must be auditable"
        );
        assert!(cp.derivation.iter().any(|d| d.starts_with("line_width")));
        assert!(cp.derivation.iter().any(|d| d.starts_with("max_tile")));
        // The seventh is the blade's chord tolerance, which the geometry pipeline
        // needs and which used to be dropped here after feeding corner_radius. A
        // limit nobody downstream can reach is not a limit, only a note.
        assert!(cp
            .derivation
            .iter()
            .any(|d| d.starts_with("curve_tolerance")));
    }

    /// The derivation count is a tripwire, so it is worth saying what it is for: a
    /// new step has to be both counted and named. Bumping the number alone would
    /// have kept the test green while hiding what changed.
    #[test]
    fn the_curve_tolerance_survives_the_derivation() {
        let cp = CuttingProfile::derive(&material(), &plotter());
        assert_eq!(cp.limits.curve_tolerance_mm, 0.08);
    }

    #[test]
    fn the_plotter_can_tighten_the_gap_beyond_the_material() {
        // A thicker blade or a sloppy machine must loosen the gap, never
        // tighten it below the film's own minimum.
        let mut p = plotter();
        p.blade.kerf_mm = 2.0;
        let cp = CuttingProfile::derive(&material(), &p);
        assert!((cp.limits.gap_mm - 2.30).abs() < 1e-12);
    }

    #[test]
    fn the_plotter_can_tighten_the_corner_radius() {
        let mut p = plotter();
        p.blade.curve_tolerance_mm = 0.50;
        let cp = CuttingProfile::derive(&material(), &p);
        // max(0.40, 0.50 × 4) = 2.00
        assert!((cp.limits.corner_radius_mm - 2.00).abs() < 1e-12);
    }

    #[test]
    fn every_rule_resolves_to_a_real_threshold() {
        let cp = CuttingProfile::derive(&material(), &plotter());
        let rs = RuleSet::from_profile(&cp);
        for rule in &rs.rules {
            let v = rule.threshold_value(&cp.limits);
            assert!(
                v.is_finite() && v > 0.0,
                "rule {} resolved to {v}",
                rule.code
            );
        }
    }

    #[test]
    fn rules_reference_real_limit_fields_never_numbers() {
        let cp = CuttingProfile::derive(&material(), &plotter());
        let rs = RuleSet::from_profile(&cp);
        // Every threshold must name an actual field of EffectiveLimits, so
        // swapping the material changes behaviour without editing the rule set
        // (P-05). A literal number here would be a hard-coded manufacturing
        // limit, which is exactly what the profile system exists to prevent.
        let fields = serde_json::to_value(&cp.limits).unwrap();
        let object = fields
            .as_object()
            .expect("EffectiveLimits serialises to an object");

        for rule in &rs.rules {
            assert!(
                object.contains_key(&rule.threshold),
                "rule {} references {:?}, which is not a field of EffectiveLimits",
                rule.code,
                rule.threshold
            );
            let value = rule.threshold_value(&cp.limits);
            assert!(
                value.is_finite() && value > 0.0,
                "rule {} resolved to no positive limit",
                rule.code
            );
        }
    }

    #[test]
    fn rule_set_is_deterministic() {
        let cp = CuttingProfile::derive(&material(), &plotter());
        assert_eq!(RuleSet::from_profile(&cp), RuleSet::from_profile(&cp));
    }

    #[test]
    fn unverified_profiles_report_as_unverified() {
        let mut m = material();
        m.status = ProfileStatus::Draft;
        let cp = CuttingProfile::derive(&m, &plotter());
        assert!(!cp.is_physically_verified(&m, &plotter()));
    }
}
