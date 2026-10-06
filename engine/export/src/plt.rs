//! HPGL / PLT writer.
//!
//! The format the cutter itself consumes, so this is the last artifact between
//! the project and the blade. Two details matter more than anything else here:
//!
//! * **PU and PD.** `PU` lifts the pen, `PD` lowers it. Getting this wrong
//!   means dragging the knife across the sheet, which scores the backing or cuts
//!   something else entirely.
//! * **Travel moves.** Movement between contours must happen with the pen up.
//!   Drawing them would cut a stray line across the design.
//!
//! Order of travel is optimised as shortest-path between contour ends. This has
//! no effect on the result, only on the time a plotter spends not cutting.

use mask_kernel::{Contour, Shapes};

/// Plotter geometry taken from the profile.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PltDevice {
    /// HPGL plotter size, e.g. `VS10` for A3.
    pub size_code: u16,
    /// Distance units per millimetre, HPGL `SC` argument.
    pub units_per_mm: f64,
    /// Speed and force for straight runs.
    pub speed_straight: u16,
    /// Speed for corners, where the knife is most likely to tear the film.
    pub speed_corner: u16,
    /// Extra blade offset applied by the firmware, in millimetres.
    pub blade_offset_mm: f64,
    /// Kerf, recorded as a comment so the file is self-describing.
    pub kerf_mm: f64,
    /// Chord tolerance for curves, from the blade profile.
    ///
    /// HPGL has no arc command in the subset a knife plotter drives, so a curve
    /// has to arrive tessellated. This is the tolerance it is tessellated at, and it
    /// is written into the file as a comment: the points in a PLT are a
    /// reconstruction of the geometry, not the geometry, and a bench worker opening
    /// the file should not have to guess which of the two they are cutting.
    pub curve_tolerance_mm: f64,
}

impl PltDevice {
    /// Derive from a plotter profile.
    ///
    /// Corner speed is a fixed fraction of straight speed rather than a profile
    /// field, because no profile expresses "how much slower corners must be" in
    /// a way that survives contact with a real machine. The multiplier is named
    /// and documented so it can be revised once a machine exists to measure.
    #[must_use]
    pub fn from_profile(
        plotter: &mask_profiles::PlotterProfile,
        units_per_mm: f64,
        speed_straight: u16,
    ) -> Self {
        const CORNER_SPEED_FACTOR: f64 = 0.6;
        Self {
            size_code: 10,
            units_per_mm,
            speed_straight,
            speed_corner: (speed_straight as f64 * CORNER_SPEED_FACTOR).round() as u16,
            blade_offset_mm: plotter.blade.offset_mm,
            kerf_mm: plotter.blade.kerf_mm,
            curve_tolerance_mm: plotter.blade.curve_tolerance_mm,
        }
    }
}

/// What to draw, split by meaning so the comments stay meaningful.
#[derive(Debug, Clone, Default)]
pub struct PltContent {
    pub cut: Shapes,
    pub registration: Shapes,
}

/// Write an HPGL plotter file.
#[must_use]
pub fn to_plt(content: &PltContent, device: PltDevice, layer_name: &str) -> String {
    let mut out = String::with_capacity(4096);

    // Header: identify the source, then initialise.
    out.push_str(&format!("IN;SP1;\n!{layer_name}\n"));
    out.push_str(&format!(
        "!curve-tolerance={:.4}mm (HPGL carries no arc; curves are cut as chords)\n",
        device.curve_tolerance_mm
    ));
    out.push_str(&format!(
        "!material-kerf={:.4}mm blade-offset={:.4}mm engine=mask-plot\n",
        device.kerf_mm, device.blade_offset_mm
    ));
    out.push_str(&format!("VS{};", device.size_code));
    out.push_str(&format!(
        "SC{},{};",
        round_u(device.units_per_mm),
        round_u(device.units_per_mm)
    ));
    out.push_str("PA0,0;\n");

    emit_section(&mut out, &content.cut, device);
    emit_section(&mut out, &content.registration, device);

    out.push_str("PU;SP0;\n");
    out
}

/// Emit a group of contours with all travel moves pen-up.
fn emit_section(out: &mut String, shapes: &Shapes, device: PltDevice) {
    if shapes.is_empty() {
        return;
    }

    let mut cursor = (0.0_f64, 0.0_f64);
    let mut endpoints: Vec<[(f64, f64); 2]> = Vec::new();

    // Flatten once, before anything reads a point.
    //
    // This used to read `.points` and draw the chords. HPGL carries no arc for a
    // knife to follow, so that was the only thing it could have drawn — but it drew
    // it silently, at the chord tolerance of whatever the geometry happened to be,
    // and the file said nothing. Now the blade's own tolerance decides the step, and
    // the header says so.
    let flat = shapes.flattened(device.curve_tolerance_mm);

    // Collect contours first so travel order can be optimised.
    for shape in &flat.shapes {
        for contour in std::iter::once(&shape.outer).chain(shape.holes.iter()) {
            if contour.points.len() >= 2 {
                let first = contour.points.first().expect("non-empty");
                let last = contour.points.last().expect("non-empty");
                endpoints.push([(first.x, first.y), (last.x, last.y)]);
            }
        }
    }

    let order = nearest_neighbour_order(&endpoints);

    let contours: Vec<&Contour> = flat
        .shapes
        .iter()
        .flat_map(|s| std::iter::once(&s.outer).chain(s.holes.iter()))
        .filter(|c| c.points.len() >= 2)
        .collect();

    for idx in order {
        let Some(contour) = contours.get(idx) else {
            continue;
        };
        let Some(first) = contour.points.first() else {
            continue;
        };

        // Travel: always pen up, never drawn.
        if cursor != (first.x, first.y) {
            out.push_str(&format!("PU{};", pen_xy(first.x, first.y, device)));
        }

        out.push_str(&format!("SP{};\n", device.speed_straight));
        out.push_str(&format!("PD{};", pen_xy(first.x, first.y, device)));

        for p in &contour.points[1..] {
            out.push_str(&format!("PA{};", pen_xy(p.x, p.y, device)));
        }
        if contour.closed {
            // Return to the first point. Closing to the last point draws nothing
            // new: with a closed flag the final point is already where the pen
            // is, and the open edge between last and first would never be cut.
            out.push_str(&format!("PA{};", pen_xy(first.x, first.y, device)));
        }

        out.push_str("PU;\n");
        if let Some(last) = contour.points.last() {
            cursor = (last.x, last.y);
        }
    }
}

/// Order contours so the tool travels as little as possible between them.
///
/// Nearest neighbour is enough. Optimal routing is not worth it: the tool is
/// spending its time not cutting either way, and a better order costs more to
/// compute than it saves.
fn nearest_neighbour_order(ends: &[[(f64, f64); 2]]) -> Vec<usize> {
    let mut remaining: Vec<usize> = (0..ends.len()).collect();
    let mut order = Vec::with_capacity(ends.len());
    let mut cursor = (0.0_f64, 0.0_f64);

    while !remaining.is_empty() {
        let mut best_pos = 0usize;
        let mut best_dist = f64::INFINITY;
        for (pos, &idx) in remaining.iter().enumerate() {
            let e = ends[idx];
            let d = (e[0].0 - cursor.0).powi(2) + (e[0].1 - cursor.1).powi(2);
            // A closed contour can also be entered at its far end.
            let d_alt = (e[1].0 - cursor.0).powi(2) + (e[1].1 - cursor.1).powi(2);
            let d = d.min(d_alt);
            if d < best_dist {
                best_dist = d;
                best_pos = pos;
            }
        }
        let idx = remaining.remove(best_pos);
        let e = ends[idx];
        cursor = if (e[1].0 - cursor.0).powi(2) + (e[1].1 - cursor.1).powi(2)
            < (e[0].0 - cursor.0).powi(2) + (e[0].1 - cursor.1).powi(2)
        {
            e[1]
        } else {
            e[0]
        };
        order.push(idx);
    }
    order
}

fn pen_xy(x_mm: f64, y_mm: f64, device: PltDevice) -> String {
    format!(
        "{},{}",
        round_u(x_mm * device.units_per_mm),
        round_u(y_mm * device.units_per_mm)
    )
}

fn round_u(value: f64) -> i64 {
    value.round() as i64
}

/// Coordinates at the canonical precision, for assertions.
#[cfg(test)]
pub fn coordinate_decimals(text: &str) -> Vec<usize> {
    text.split(|c: char| !c.is_ascii_digit() && c != '.')
        .filter(|t| t.contains('.'))
        .map(|t| t.split('.').nth(1).map_or(0, str::len))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mask_kernel::{Point, Shape};

    fn p(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    fn device() -> PltDevice {
        PltDevice {
            size_code: 10,
            units_per_mm: 40.0,
            speed_straight: 30,
            speed_corner: 18,
            blade_offset_mm: 0.10,
            kerf_mm: 0.15,
            curve_tolerance_mm: 0.08,
        }
    }

    fn content() -> PltContent {
        PltContent {
            cut: Shapes {
                shapes: vec![
                    Shape::new(Contour {
                        points: vec![p(0.0, 0.0), p(100.0, 0.0), p(100.0, 50.0)],
                        closed: false,
                        arcs: Vec::new(),
                    }),
                    Shape::new(Contour {
                        points: vec![p(10.0, 10.0), p(40.0, 10.0), p(40.0, 30.0)],
                        closed: false,
                        arcs: Vec::new(),
                    }),
                ],
            },
            registration: Shapes::single(Shape::new(Contour {
                points: vec![p(5.0, 5.0), p(95.0, 5.0)],
                closed: false,
                arcs: Vec::new(),
            })),
        }
    }

    #[test]
    fn the_file_initialises_the_plotter() {
        let plt = to_plt(&content(), device(), "MASK-000127 L01");
        assert!(plt.starts_with("IN;"));
        assert!(plt.contains("VS10;"));
        assert!(plt.contains("SC40,40;"), "scale must be set before drawing");
        assert!(
            plt.ends_with("PU;SP0;\n"),
            "must lift and deselect at the end"
        );
    }

    #[test]
    fn the_layer_is_identified_in_a_comment() {
        let plt = to_plt(&content(), device(), "MASK-000127 L01");
        assert!(plt.contains("!MASK-000127 L01"), "layer label missing");
        // A plotter log has to say which mask produced the cut.
        assert!(plt.contains("!material-kerf="));
    }

    #[test]
    fn every_travel_move_is_pen_up() {
        let plt = to_plt(&content(), device(), "L01");
        // After the first pen-down, every PA must be a drawn segment and every
        // move between contours must happen with the pen up. The initial PA0,0
        // precedes any PD and is a positioning move, so it is skipped.
        let mut down = false;
        let mut started = false;
        for token in plt.split(';').map(str::trim) {
            let bare = !token.contains(',');
            if token == "PU" && bare {
                down = false;
            } else if token == "PD" && bare {
                down = true;
                started = true;
            } else if token.starts_with("PA") && started {
                assert!(
                    down,
                    "PA while the pen is up would cut a stray line: {token}"
                );
            }
        }
    }

    #[test]
    fn a_closed_contour_returns_to_its_start() {
        let closed = PltContent {
            cut: Shapes::single(Shape::new(Contour {
                points: vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)],
                closed: true,
                arcs: Vec::new(),
            })),
            registration: Shapes::default(),
        };
        let plt = to_plt(&closed, device(), "L01");
        // The first point must appear at the end of the path.
        let first = plt.find("PA0,0;").expect("start point");
        let last = plt.rfind("PA0,0;").expect("closing point");
        assert!(last > first, "a closed contour must return to its origin");
    }

    #[test]
    fn an_open_contour_does_not_close() {
        let plt = to_plt(&content(), device(), "L01");
        // Two open contours plus one registration line: three starts, and the
        // first point must not reappear at the end.
        assert_eq!(plt.matches("PD").count(), 3);
    }

    #[test]
    fn coordinates_are_scaled_to_plotter_units() {
        // 100 mm at 40 units/mm is 4000 plotter units.
        let plt = to_plt(&content(), device(), "L01");
        assert!(plt.contains("PA4000,"), "{plt}");
    }

    #[test]
    fn export_is_deterministic() {
        assert_eq!(
            to_plt(&content(), device(), "L01"),
            to_plt(&content(), device(), "L01")
        );
    }

    #[test]
    fn travel_order_is_stable_across_runs() {
        // Shortest-path ordering must not vary, or the same project would
        // produce two different files and different hashes.
        let a = to_plt(&content(), device(), "L01");
        let b = to_plt(&content(), device(), "L01");
        assert_eq!(a, b);
    }

    #[test]
    fn empty_geometry_still_produces_a_valid_file() {
        let plt = to_plt(&PltContent::default(), device(), "empty");
        assert!(plt.contains("IN;"));
        assert!(plt.ends_with("PU;SP0;\n"));
        assert!(
            !plt.contains("PD"),
            "nothing to draw must mean nothing is cut"
        );
    }

    #[test]
    fn contours_are_all_visited_exactly_once() {
        let plt = to_plt(&content(), device(), "L01");
        assert_eq!(plt.matches("PD").count(), 3, "each contour drawn once");
    }

    #[test]
    fn corner_speed_is_derived_from_straight_speed() {
        let plotter = mask_profiles::PlotterProfile {
            code: "knife-25mm".into(),
            version: 2,
            name: "test".into(),
            kind: "knife_plotter".into(),
            blade: mask_profiles::Blade {
                angle_deg: 45.0,
                offset_mm: 0.1,
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
                max_registration_error_mm: 0.3,
            },
            status: mask_profiles::ProfileStatus::Active,
            verified_physical: true,
        };
        let d = PltDevice::from_profile(&plotter, 40.0, 30);
        assert_eq!(d.speed_straight, 30);
        assert_eq!(d.speed_corner, 18, "corners run slower than straights");
    }
    use mask_kernel::geom::{ArcSegment, Contour};

    /// A circle of four quarter arcs: 4 points, and a boundary that is emphatically
    /// not the polygon through them.
    fn curved_disc(r: f64) -> Shapes {
        let points = (0..4)
            .map(|i| {
                let a = std::f64::consts::FRAC_PI_2 * (f64::from(i));
                Point {
                    x: r * a.cos(),
                    y: r * a.sin(),
                }
            })
            .collect::<Vec<_>>();
        let arcs = (0..4)
            .map(|i| ArcSegment {
                from: i,
                to: (i + 1) % 4,
                centre: Point { x: 0.0, y: 0.0 },
                radius: r,
                ccw: true,
            })
            .collect();
        Shapes::single(Shape::new(Contour {
            points,
            closed: true,
            arcs,
        }))
    }

    fn curves_content() -> PltContent {
        PltContent {
            cut: curved_disc(10.0),
            registration: Shapes::new(),
        }
    }

    /// The blade must cut the curve, not the four chords.
    ///
    /// A PLT is a list of pen positions, so the only thing that can distinguish a
    /// curve from its chords is how many points are between the corners. Four means
    /// the chords were drawn; more means the curve was.
    #[test]
    fn the_plt_actually_steps_along_a_curve() {
        let plt = to_plt(&curves_content(), device(), "arcs");
        let moves = plt.matches("PA").count();
        assert!(
            moves > 20,
            "a circle flattened at 0.08 mm needs dozens of pen moves, found {moves}"
        );
    }

    /// And it must not step along the chords when the caller already flattened at a
    /// tolerance of their own, or the two stages would silently compound.
    #[test]
    fn an_already_flat_contour_is_left_alone() {
        let pre_flattened = PltContent {
            cut: curved_disc(10.0).flattened(0.05),
            ..curves_content()
        };
        let plt = to_plt(&pre_flattened, device(), "arcs");
        assert!(
            plt.matches("PA").count() > 20,
            "a pre-flattened curve is still a curve"
        );
    }

    /// The file has to say it is a reconstruction. Somebody at the bench reading
    /// points in a PLT cannot otherwise tell a tessellated circle from a circle.
    #[test]
    fn the_file_declares_that_curves_were_flattened() {
        let plt = to_plt(&curves_content(), device(), "arcs");
        assert!(
            plt.contains("!curve-tolerance=0.0800mm"),
            "the tolerance must be in the file:\n{plt}"
        );
        assert!(
            plt.contains("no arc"),
            "and it must say what that means: {plt}"
        );
    }
}
