//! Instruction text, generated from project data.
//!
//! The constraint from `ROADMAP.md` 5.6 and `AI_ARCHITECTURE.md` §4.4 is that
//! **every number in the text comes from data**. That is easy to state and easy
//! to break: someone adds a sentence with "about 0.5 mm" in it, and the text
//! stops matching the project it describes.
//!
//! So numbers are not written directly. They go through [`with_unit`], which
//! both writes the figure and records where it came from. [`verify`] then reads
//! the rendered text back and refuses it if a number appears that no recorded
//! use accounts for, or if a recorded use never made it into the text. The
//! check therefore cannot be satisfied by careful writing — only by routing
//! every figure through the builder.
//!
//! What this does *not* do is make prose true. A sentence can be wrong and still
//! contain no stray numbers. That is what a person reads for.

use mask_profiles::{MaterialProfile, PlotterProfile};
use mask_rules::Severity;
use mask_validate::{Unit, ValidationReport, Verdict};

/// One figure written into the text, with the field it was read from.
///
/// The source is a dotted path into the project, not a sentence. It has to
/// survive being wrong: `validation.layers[2].min_width_mm` can be checked
/// against the report, "the recommended width" cannot.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberUse {
    pub value: f64,
    /// The figure exactly as it appears in the text.
    ///
    /// Verification compares these, not `value`. A measured width of
    /// 0.7999996840953827 is written as `0.80`, and a verifier that insists the
    /// text contain the raw f64 would reject every correctly rounded document —
    /// which teaches people to disable the check.
    pub rendered: String,
    pub unit: Unit,
    pub source: String,
}

/// The rendered instruction and the audit trail behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct Instruction {
    pub text: String,
    pub uses: Vec<NumberUse>,
}

/// A number in the text that no recorded use accounts for.
#[derive(Debug, Clone, PartialEq)]
pub struct UnverifiedNumber {
    pub text: String,
}

impl std::fmt::Display for UnverifiedNumber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "в тексте есть число `{}`, не взятое из данных",
            self.text
        )
    }
}

/// What the instruction is written about.
pub struct InstructionInput<'a> {
    pub project_id: &'a str,
    pub revision: u32,
    pub engine_version: &'a str,
    pub material: &'a MaterialProfile,
    pub plotter: &'a PlotterProfile,
    pub report: &'a ValidationReport,
    /// Tiles the nesting plan was split into, when the project was nested.
    pub tiles: Option<u32>,
}

/// Accumulates the text and every number written into it.
struct Builder {
    text: String,
    uses: Vec<NumberUse>,
}

impl Builder {
    fn new() -> Self {
        Self {
            text: String::with_capacity(4096),
            uses: Vec::new(),
        }
    }

    fn line(&mut self, s: &str) {
        self.text.push_str(s);
        self.text.push('\n');
    }

    fn blank(&mut self) {
        self.text.push('\n');
    }

    /// Declare a figure that is already in the text rather than written by the
    /// builder — a revision number, a tile count, a version string's digits.
    fn declare(&mut self, v: f64, unit: Unit, source: &str) {
        self.uses.push(NumberUse {
            value: v,
            rendered: render_number(v, unit),
            unit,
            source: source.to_string(),
        });
    }
}

/// Render the markdown instruction.
///
/// Deterministic: the same input gives byte-identical output, because every
/// collection walked here is already in report order.
#[must_use]
pub fn render_markdown(input: &InstructionInput<'_>) -> Instruction {
    let mut b = Builder::new();

    let id = prose(input.project_id, &mut b.uses, "project.id");
    // A version is an identifier like a project id: `0.4.1` is not a figure, and
    // `version_digits` would have read it as the number 0.41.
    let version = prose(input.engine_version, &mut b.uses, "engine_version");
    let head = format!(
        "# Проект {id}\n\nРевизия {} · движок {version}",
        input.revision
    );
    b.line(&head);
    b.declare(input.revision as f64, Unit::Count, "revision");
    b.blank();

    render_verdict(&mut b, input);
    render_material(&mut b, input);
    render_cutter(&mut b, input);
    render_layers(&mut b, input);
    render_problems(&mut b, input);

    Instruction {
        text: b.text,
        uses: b.uses,
    }
}

fn render_verdict(b: &mut Builder, input: &InstructionInput<'_>) {
    let r = input.report;

    let counts = format!(
        "Ошибок: {}. Предупреждений: {}.",
        r.counts.error, r.counts.warning
    );
    b.line("## Вердикт");
    b.blank();
    b.line(verdict_ru(r.verdict));
    b.blank();
    b.line(&counts);
    b.declare(
        r.counts.error as f64,
        Unit::Count,
        "validation.counts.error",
    );
    b.declare(
        r.counts.warning as f64,
        Unit::Count,
        "validation.counts.warning",
    );

    // The most important sentence in the file. A PASS computed from unproven
    // limits is a calculation, not a fact, and the operator holding the film is
    // the one who has to know that.
    if r.verdict == Verdict::Pass && !r.limits_physically_verified {
        b.blank();
        b.line(
            "> Профиль не подтверждён физическим прогоном. Вердикт PASS означает, что \
             проект соответствует заявленным ограничениям, а не что ограничения проверены \
             на реальном материале.",
        );
    }
    b.blank();
}

fn render_material(b: &mut Builder, input: &InstructionInput<'_>) {
    let m = input.material;
    b.line("## Материал");
    b.blank();
    let mname = prose(&m.name, &mut b.uses, "material.name");
    let mcode = prose(&m.code, &mut b.uses, "material.code");
    let head = format!("**{mname}** ({mcode} версии {})", m.version);
    b.line(&head);
    b.declare(m.version as f64, Unit::Count, "material.version");
    b.blank();
    b.line("| Параметр | Значение |");
    b.line("|---|---|");

    let v = mm(m.thickness_mm, b, "material.thickness_mm");
    row(b, "Толщина", &v);
    let v = mm(
        m.minimums.line_width_mm,
        b,
        "material.minimums.line_width_mm",
    );
    row(b, "Минимальная ширина элемента", &v);
    let v = mm(m.minimums.gap_mm, b, "material.minimums.gap_mm");
    row(b, "Минимальный зазор", &v);
    let v = mm(
        m.minimums.island_width_mm,
        b,
        "material.minimums.island_width_mm",
    );
    row(b, "Минимальная ширина островка", &v);
    let v = mm(
        m.minimums.feature_length_mm,
        b,
        "material.minimums.feature_length_mm",
    );
    row(b, "Минимальная длина штриха", &v);
    let v = mm(
        m.minimums.corner_radius_mm,
        b,
        "material.minimums.corner_radius_mm",
    );
    row(b, "Минимальный радиус угла", &v);
    let v = mm(
        m.minimums.bridge_width_mm,
        b,
        "material.minimums.bridge_width_mm",
    );
    row(b, "Ширина моста", &v);
    let v = mm(
        m.physical.tolerance_registration_mm,
        b,
        "material.tolerance_registration_mm",
    );
    row(b, "Допуск регистрации", &v);
    let v = deg(
        m.physical.peel_angle_min_deg,
        b,
        "material.peel_angle_min_deg",
    );
    row(b, "Минимальный угол снятия", &v);
    b.blank();
}

fn render_cutter(b: &mut Builder, input: &InstructionInput<'_>) {
    let p = input.plotter;
    b.line("## Режущий инструмент и станок");
    b.blank();
    let pname = prose(&p.name, &mut b.uses, "plotter.name");
    let pcode = prose(&p.code, &mut b.uses, "plotter.code");
    let head = format!("**{pname}** ({pcode})");
    b.line(&head);
    b.blank();
    b.line("| Параметр | Значение |");
    b.line("|---|---|");

    let v = deg(p.blade.angle_deg, b, "plotter.blade.angle_deg");
    row(b, "Угол пера", &v);
    let v = mm(p.blade.offset_mm, b, "plotter.blade.offset_mm");
    row(b, "Смещение пера", &v);
    let v = mm(p.blade.kerf_mm, b, "plotter.blade.kerf_mm");
    row(b, "Ширина реза (kerf)", &v);
    let v = mm(
        p.blade.curve_tolerance_mm,
        b,
        "plotter.blade.curve_tolerance_mm",
    );
    row(b, "Допуск аппроксимации кривой", &v);
    let v = mm(
        p.accuracy.repeatability_mm,
        b,
        "plotter.accuracy.repeatability_mm",
    );
    row(b, "Повторяемость станка", &v);
    let w = mm(p.working_area.width_mm, b, "plotter.working_area.width_mm");
    let h = mm(
        p.working_area.length_mm,
        b,
        "plotter.working_area.length_mm",
    );
    b.line(&format!("| Рабочая область | {w} × {h} |"));
    b.blank();
}

fn render_layers(b: &mut Builder, input: &InstructionInput<'_>) {
    b.line("## Слои");
    b.blank();
    b.line("| # | Слой | Элементов | Островков | Ширина | Зазор |");
    b.line("|---|---|---|---|---|---|");

    for l in &input.report.layers {
        let idx = plain(
            &mut b.uses,
            l.index as f64,
            format!("validation.layers[{}].index", l.index),
        );
        let paths = plain(
            &mut b.uses,
            l.paths as f64,
            format!("validation.layers[{}].paths", l.index),
        );
        let islands = plain(
            &mut b.uses,
            l.islands as f64,
            format!("validation.layers[{}].islands", l.index),
        );
        let width = match l.min_width_mm {
            Some(v) => mm(
                v,
                b,
                &format!("validation.layers[{}].min_width_mm", l.index),
            ),
            None => "—".to_string(),
        };
        let gap = match l.min_gap_mm {
            Some(v) => mm(v, b, &format!("validation.layers[{}].min_gap_mm", l.index)),
            None => "—".to_string(),
        };
        let lname = prose(
            &l.name,
            &mut b.uses,
            &format!("validation.layers[{}].name", l.index),
        );
        b.line(&format!(
            "| {idx} | {lname} | {paths} | {islands} | {width} | {gap} |"
        ));
    }
    b.blank();

    if let Some(tiles) = input.tiles {
        let t = plain(
            &mut b.uses,
            tiles as f64,
            "manufacturing.nesting.tiles".to_string(),
        );
        b.line(&format!(
            "Раскладка разбита на {t} {}.",
            plural_ru(tiles, "плитку", "плитки", "плиток")
        ));
        b.blank();
    }
}

fn render_problems(b: &mut Builder, input: &InstructionInput<'_>) {
    let blocking: Vec<_> = input
        .report
        .violations
        .iter()
        .filter(|v| v.severity != Severity::Info)
        .collect();

    b.line("## Замечания");
    b.blank();

    if blocking.is_empty() {
        b.line("Нарушений, требующих внимания, нет.");
        b.blank();
        return;
    }

    for v in &blocking {
        let headline = prose(
            &v.headline,
            &mut b.uses,
            &format!("violations[{}].headline", v.code),
        );
        b.line(&format!("### {headline}"));
        b.blank();
        let reason = prose(
            &v.reason,
            &mut b.uses,
            &format!("violations[{}].reason", v.code),
        );
        b.line(&reason);

        if let (Some(measured), Some(required)) = (v.measured, v.required) {
            let m = with_unit(
                measured,
                v.unit,
                b,
                &format!("violations[{}].measured", v.code),
            );
            let r = with_unit(
                required,
                v.unit,
                b,
                &format!("violations[{}].required", v.code),
            );
            b.blank();
            b.line(&format!("Замерено {m} при требуемых {r}."));
        }

        if !v.actions.is_empty() {
            b.blank();
            for a in &v.actions {
                let label = prose(
                    &a.label,
                    &mut b.uses,
                    &format!("violations[{}].actions", v.code),
                );
                b.line(&format!("- {label}"));
            }
        }
        b.blank();
    }
}

/// One table row. Takes an already-rendered value so the figure can be recorded
/// before the text line borrows the builder again.
fn row(b: &mut Builder, label: &str, value: &str) {
    b.line(&format!("| {label} | {value} |"));
}

/// Render one number with its unit and record where it came from.
fn with_unit(v: f64, unit: Unit, b: &mut Builder, source: &str) -> String {
    let rendered = render_number(v, unit);
    b.uses.push(NumberUse {
        value: v,
        rendered: rendered.clone(),
        unit,
        source: source.to_string(),
    });
    match unit {
        Unit::Mm => format!("{rendered} мм"),
        Unit::Deg => format!("{rendered}°"),
        Unit::Mm2 => format!("{rendered} мм²"),
        Unit::Count => rendered,
    }
}

/// How a figure appears in the text, without its unit.
fn render_number(v: f64, unit: Unit) -> String {
    match unit {
        Unit::Mm => format!("{v:.2}"),
        Unit::Deg | Unit::Mm2 => format!("{v:.1}"),
        Unit::Count => format!("{v}"),
    }
}

/// Declare every figure inside a string that came from data.
///
/// Profile names carry real quantities — "Knife plotter, 25 mm holder", "Matte
/// vinyl 0.09 mm" — so the figures in them are data-derived and get declared
/// against the field they came from. Without this the verifier would report the
/// profile's own name as untraceable, and the tempting fix would be to loosen
/// the check rather than describe where the number came from.
fn prose(text: &str, uses: &mut Vec<NumberUse>, source: &str) -> String {
    for token in numbers_in(text) {
        let Ok(value) = token.parse::<f64>() else {
            continue;
        };
        uses.push(NumberUse {
            value,
            rendered: token,
            unit: Unit::Count,
            source: source.to_string(),
        });
    }
    text.to_string()
}

fn mm(v: f64, b: &mut Builder, source: &str) -> String {
    with_unit(v, Unit::Mm, b, source)
}

fn deg(v: f64, b: &mut Builder, source: &str) -> String {
    with_unit(v, Unit::Deg, b, source)
}

/// A number with no unit of its own, for table cells and counts.
fn plain(uses: &mut Vec<NumberUse>, v: f64, source: String) -> String {
    let rendered = format!("{v}");
    uses.push(NumberUse {
        value: v,
        rendered: rendered.clone(),
        unit: Unit::Count,
        source,
    });
    rendered
}

fn verdict_ru(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "**PASS** — проект можно производить по заявленным ограничениям.",
        Verdict::PassWithWarnings => {
            "**PASS с предупреждениями** — производить можно, но прочитайте замечания."
        }
        Verdict::Fail => "**FAIL** — проект нельзя производить, устраните нарушения.",
        Verdict::Fatal => "**FATAL** — нарушение блокирующее, производство невозможно.",
    }
}

fn plural_ru(n: u32, one: &'static str, few: &'static str, many: &'static str) -> &'static str {
    match n % 10 {
        1 if n % 100 != 11 => one,
        2..=4 if !(12..=14).contains(&(n % 100)) => few,
        _ => many,
    }
}

/// Check the rendered text against the recorded uses.
///
/// Two directions, because either one alone is easy to fool. A number in the
/// text that nothing accounts for means prose was written by hand. A recorded
/// use that never reached the text means the audit trail claims a provenance
/// the document does not have.
pub fn verify(instruction: &Instruction) -> Result<(), Vec<UnverifiedNumber>> {
    let found = numbers_in(&instruction.text);
    let mut problems = Vec::new();

    for token in &found {
        let Ok(value) = token.parse::<f64>() else {
            continue;
        };
        if !instruction.uses.iter().any(|u| {
            u.rendered
                .parse::<f64>()
                .is_ok_and(|r| (r - value).abs() < 1e-9)
        }) {
            problems.push(UnverifiedNumber {
                text: token.clone(),
            });
        }
    }

    for u in &instruction.uses {
        let Ok(expected) = u.rendered.parse::<f64>() else {
            continue;
        };
        let present = found
            .iter()
            .filter_map(|t| t.parse::<f64>().ok())
            .any(|v| (v - expected).abs() < 1e-9);
        if !present {
            problems.push(UnverifiedNumber {
                text: format!("{} ({})", u.value, u.source),
            });
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Every maximal run of digits and dots in the text that reads as a figure.
///
/// A run that continues an alphanumeric token is part of an identifier and not
/// a measurement: `MASK-000127`, `matte-vinyl-090` and `knife-25mm` all contain
/// digits, and treating those as figures would either demand provenance for a
/// project number or, worse, let a real figure hide inside an identifier.
fn numbers_in(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;

    let flush = |out: &mut Vec<String>, cur: &mut String| {
        if !cur.is_empty() {
            push_number(out, cur);
            cur.clear();
        }
    };

    for ch in text.chars() {
        let continues_token = ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-');
        if ch.is_ascii_digit() || ch == '.' {
            // A digit directly after a letter, hyphen or underscore belongs to
            // the token that is already being read.
            if !(in_token && cur.is_empty()) {
                cur.push(ch);
            }
            in_token = continues_token;
        } else {
            flush(&mut out, &mut cur);
            in_token = continues_token;
        }
    }
    flush(&mut out, &mut cur);
    out
}

fn push_number(out: &mut Vec<String>, raw: &str) {
    let trimmed = raw.trim_matches('.');
    if trimmed.is_empty() || !trimmed.contains(|c: char| c.is_ascii_digit()) {
        return;
    }
    if trimmed.parse::<f64>().is_ok() {
        out.push(trimmed.to_string());
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use mask_validate::{Counts, LayerMetrics, Violation};

    fn material() -> MaterialProfile {
        MaterialProfile {
            code: "matte-vinyl-090".into(),
            version: 4,
            name: "Матовый винил".into(),
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
            name: "Резак".into(),
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

    fn layer(index: u32) -> LayerMetrics {
        LayerMetrics {
            index,
            name: format!("Слой {index}"),
            paths: 3,
            islands: 1,
            holes: 0,
            area_mm2: 120.0,
            bbox: [0.0, 0.0, 20.0, 10.0],
            min_width_mm: Some(0.85),
            min_gap_mm: Some(1.10),
            min_island_area_mm2: Some(3.0),
            density_per_100cm2: 12.0,
            shortest_edge_mm: Some(2.5),
            region_widths_mm: vec![Some(0.85), Some(1.2), Some(0.9)],
        }
    }

    fn report(verified: bool) -> ValidationReport {
        ValidationReport {
            verdict: Verdict::Pass,
            counts: Counts {
                fatal: 0,
                error: 0,
                warning: 1,
                info: 0,
            },
            violations: vec![],
            layers: vec![layer(0), layer(1)],
            fidelity: Some(0.91),
            limits_physically_verified: verified,
        }
    }

    pub(super) fn input<'a>(
        m: &'a MaterialProfile,
        p: &'a PlotterProfile,
        r: &'a ValidationReport,
    ) -> InstructionInput<'a> {
        InstructionInput {
            project_id: "MASK-000127",
            revision: 3,
            engine_version: "0.4.1",
            material: m,
            plotter: p,
            report: r,
            tiles: Some(2),
        }
    }

    #[test]
    fn a_generated_instruction_verifies() {
        let (m, p, r) = (material(), plotter(), report(false));
        let i = render_markdown(&input(&m, &p, &r));
        assert!(verify(&i).is_ok(), "{:?}", verify(&i));
    }

    #[test]
    fn a_number_written_by_hand_is_caught() {
        // The whole point. Prose gets edited, someone adds a helpful reminder,
        // and the text stops describing this project.
        let (m, p, r) = (material(), plotter(), report(false));
        let mut i = render_markdown(&input(&m, &p, &r));
        i.text.push_str("\nПечатайте при 200 °C.\n");

        let err = verify(&i).expect_err("рукописное число должно быть поймано");
        assert!(
            err.iter().any(|e| e.text == "200"),
            "ожидалось число 200, {:?}",
            err
        );
    }

    #[test]
    fn a_use_that_never_reached_the_text_is_caught() {
        // The other direction: an audit trail that claims a provenance the
        // document does not have is worse than no trail.
        let (m, p, r) = (material(), plotter(), report(false));
        let mut i = render_markdown(&input(&m, &p, &r));
        i.uses.push(NumberUse {
            value: 7.77,
            rendered: "7.77".to_string(),
            unit: Unit::Mm,
            source: "fabricated".into(),
        });

        let err = verify(&i).expect_err("неиспользованная запись должна быть поймана");
        assert!(err.iter().any(|e| e.text.contains("fabricated")), "{err:?}");
    }

    #[test]
    fn every_figure_moves_when_the_data_moves() {
        // If the text were static, "generated from data" would be a claim
        // rather than a property.
        let (m, p, r) = (material(), plotter(), report(false));
        let before = render_markdown(&input(&m, &p, &r)).text;

        let mut m2 = m.clone();
        m2.minimums.line_width_mm = 1.25;
        let after = render_markdown(&input(&m2, &p, &r)).text;

        assert!(before.contains("0.80"), "{before}");
        assert!(after.contains("1.25"), "{after}");
        assert!(!after.contains("0.80"), "старое число должно уйти: {after}");
    }

    #[test]
    fn an_unproven_pass_says_so_in_the_text() {
        let (m, p) = (material(), plotter());
        let unproven = render_markdown(&input(&m, &p, &report(false)));
        assert!(unproven.text.contains("не подтверждён физическим прогоном"));

        let proven = render_markdown(&input(&m, &p, &report(true)));
        assert!(!proven.text.contains("не подтверждён физическим прогоном"));
    }

    #[test]
    fn a_violation_carries_its_measured_and_required_figures() {
        let (m, p) = (material(), plotter());
        let mut r = report(false);
        r.verdict = Verdict::Fail;
        r.counts.error = 1;
        r.violations = vec![Violation {
            code: "min_gap".into(),
            severity: Severity::Error,
            layer_index: Some(1),
            measured: Some(0.45),
            required: Some(0.90),
            unit: Unit::Mm,
            count: 1,
            headline: "Зазор 0.45 мм при требуемых 0.90 мм".into(),
            reason: "Плёнка сойдётся.".into(),
            actions: vec![],
        }];

        let i = render_markdown(&input(&m, &p, &r));
        assert!(
            i.text.contains("Замерено 0.45 мм при требуемых 0.90 мм."),
            "{}",
            i.text
        );
        assert!(verify(&i).is_ok(), "{:?}", verify(&i));
    }

    #[test]
    fn rendering_is_deterministic() {
        let (m, p, r) = (material(), plotter(), report(false));
        let a = render_markdown(&input(&m, &p, &r));
        let b = render_markdown(&input(&m, &p, &r));
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn the_verifier_ignores_prose_without_figures() {
        let (m, p, r) = (material(), plotter(), report(false));
        let mut i = render_markdown(&input(&m, &p, &r));
        i.text
            .push_str("\nРежьте на ровном свету, прохладным вечером.\n");
        assert!(verify(&i).is_ok());
    }
}
